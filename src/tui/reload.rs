use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant, UNIX_EPOCH};

use walkdir::WalkDir;

use super::inspection::{InspectionInput, InstructionInspection};
use crate::config::{GlobalConfig, Paths};
use crate::context::{
    Audit, ContextRuntime, ContextSource, NestedAgentsMd, SourceGroup, SourceState,
    claude_config_dir, claude_filename_matches, claude_managed_dir, claude_settings_files,
    codex_home, codex_settings, is_rule_path, pi_agent_dir,
};
use crate::deploy;
use crate::git_worktree::Topology;
use crate::local::LocalRepository;
use crate::project::Workspace;

const AUTO_RELOAD_INTERVAL: Duration = Duration::from_secs(1);

/// Paths the watch signature covers, copied from the session's accepted observations so the
/// reload worker can hash them.
pub(super) struct WatchInput {
    paths: Paths,
    directory: PathBuf,
    known: Vec<PathBuf>,
}

impl WatchInput {
    pub(super) fn new(inspection: &InstructionInspection, paths: &Paths) -> Self {
        Self {
            known: known_paths(inspection),
            ..Self::before_discovery(paths, &inspection.context.directory)
        }
    }

    // Before the first discovery no file has been observed yet.
    fn before_discovery(paths: &Paths, directory: &Path) -> Self {
        Self {
            paths: paths.clone(),
            directory: directory.to_owned(),
            known: Vec::new(),
        }
    }
}

/// Every path the accepted generation read or looked for: Context sources and their imports,
/// and each file its inspection observed, such as Project, Local and Global targets, Sections,
/// and open documents.
fn known_paths(inspection: &InstructionInspection) -> Vec<PathBuf> {
    let mut known = inspection
        .context
        .sources
        .iter()
        .flat_map(|source| std::iter::once(&source.path).chain(&source.imports))
        .chain(inspection.files.keys())
        .cloned()
        .collect::<Vec<_>>();
    known.sort();
    known.dedup();
    known
}

// A path counts only as a file discovery could read, so a missing path, a directory, and a
// directory's own metadata all hash alike.
fn hash_file(path: &Path, hasher: &mut DefaultHasher) {
    let Ok(metadata) = fs::metadata(path) else {
        // A dangling link still records where it points.
        if fs::symlink_metadata(path).is_ok() {
            path.hash(hasher);
            fs::read_link(path).ok().hash(hasher);
        }
        return;
    };
    if metadata.is_dir() {
        return;
    }
    path.hash(hasher);
    // The resolved path changes when a linked directory anywhere above the file is
    // retargeted, whatever the new file's metadata.
    fs::canonicalize(path).ok().hash(hasher);
    fs::read_link(path).ok().hash(hasher);
    metadata.len().hash(hasher);
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .hash(hasher);
}

fn hash_known(path: &Path) -> u64 {
    let mut hasher = DefaultHasher::new();
    hash_file(path, &mut hasher);
    hasher.finish()
}

// Every file below `root`, such as mdmanager's own configuration, Library and state.
fn hash_all(root: &Path, hasher: &mut DefaultHasher) {
    for entry in WalkDir::new(root).into_iter().flatten() {
        hash_file(entry.path(), hasher);
    }
}

// A Claude or Cursor rule directory: files with the runtime's extension, following links and
// skipping no directory, as `push_rules` and `cursor_rule_files` do.
fn hash_rules(root: &Path, rule_extension: &str, hasher: &mut DefaultHasher) {
    for entry in WalkDir::new(root).follow_links(true).into_iter().flatten() {
        if entry.file_type().is_file()
            && entry
                .path()
                .extension()
                .and_then(|extension| extension.to_str())
                == Some(rule_extension)
        {
            hash_file(entry.path(), hasher);
        }
    }
}

// The files Claude's subfolder scan (`scan_claude_descendants`) can report, walked the same
// way: below the launch directory, skipping `.git` and the launch directory's own `.claude`,
// and entering linked directories only inside rule directories. Nested `AGENTS.md` counts only
// where the scan's own selection reports it; every file that selection depends on, such as
// settings and CLAUDE files, is watched too.
fn hash_descendants(directory: &Path, paths: &Paths, hasher: &mut DefaultHasher) {
    let agents_md = NestedAgentsMd::new(directory, paths);
    let launch_claude = directory.join(".claude");
    let walker = WalkDir::new(directory)
        .follow_links(true)
        .min_depth(1)
        .into_iter()
        .filter_entry(|entry| {
            entry.file_name() != ".git"
                && entry.path() != launch_claude
                && (!entry.path().is_symlink()
                    || !entry.path().is_dir()
                    || is_rule_path(entry.path()))
        });
    for entry in walker.flatten() {
        let path = entry.path();
        if !entry.file_type().is_file() {
            continue;
        }
        let nested_instruction = path.parent() != Some(directory)
            && !is_rule_path(path)
            && (claude_filename_matches(path, "CLAUDE.md")
                || claude_filename_matches(path, "CLAUDE.local.md"));
        let nested_rule =
            path.extension().is_some_and(|extension| extension == "md") && is_rule_path(path);
        if nested_instruction || nested_rule || agents_md.reports(directory, path) {
            hash_file(path, hasher);
        }
    }
}

/// Watched state: one hash over the locations discovery probes, and one hash per file the
/// accepted generation knows. Per-file hashes let a finished Claude descendant scan or a runtime
/// switch add the files it newly knows while each file already known keeps the hash accepted
/// with its observation, so an edit made since then is still a difference.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct WatchSignature {
    candidates: u64,
    known: BTreeMap<PathBuf, u64>,
}

impl WatchSignature {
    /// Hashed before a refresh reads disk. Accepting a refresh with this signature, rather than
    /// one taken after discovery, keeps an edit that lands while discovery runs a difference
    /// for the next check.
    pub(super) fn observe(input: &WatchInput) -> Self {
        Self {
            candidates: hash_candidates(&input.paths, &input.directory),
            known: input
                .known
                .iter()
                .map(|path| (path.clone(), hash_known(path)))
                .collect(),
        }
    }

    /// The signature before the session's first discovery, when no file is known yet.
    pub(super) fn startup(paths: &Paths, directory: &Path) -> Self {
        Self::observe(&WatchInput::before_discovery(paths, directory))
    }

    /// The signature for observations that know `known`: paths this signature hashed keep
    /// that hash, and only newly known paths are hashed now.
    fn extended(&self, known: &[PathBuf]) -> Self {
        Self {
            candidates: self.candidates,
            known: known
                .iter()
                .map(|path| {
                    let hash = self
                        .known
                        .get(path)
                        .copied()
                        .unwrap_or_else(|| hash_known(path));
                    (path.clone(), hash)
                })
                .collect(),
        }
    }

    pub(super) fn observe_known(&mut self, inspection: &InstructionInspection) {
        *self = self.extended(&known_paths(inspection));
    }
}

/// Names Claude, Codex, Cursor or Pi look for in the launch directory and every ancestor.
const ANCESTOR_CANDIDATES: [&str; 8] = [
    "AGENTS.override.md",
    "AGENTS.md",
    "AGENTS.MD",
    "CLAUDE.md",
    "CLAUDE.MD",
    "CLAUDE.local.md",
    ".claude/CLAUDE.md",
    ".claude/AGENTS.md",
];

/// Files and trees discovery looks for where it looks for them, whether or not they exist yet,
/// so creating one is noticed. Files the accepted generation already knows are hashed
/// separately.
fn hash_candidates(paths: &Paths, directory: &Path) -> u64 {
    let mut hasher = DefaultHasher::new();
    hash_all(&paths.data_dir, &mut hasher);
    if !paths.config.starts_with(&paths.data_dir) {
        hash_file(&paths.config, &mut hasher);
    }
    // The state directory is walked itself even inside the data directory: the data walk does
    // not enter a linked state directory, so retargeting one would go unnoticed.
    hash_all(&paths.state_dir, &mut hasher);

    let claude_config = claude_config_dir(paths);
    let codex_home = codex_home(paths);
    let codex = codex_settings(&codex_home);
    let pi_agent = pi_agent_dir(paths);
    for path in [
        claude_managed_dir().join("CLAUDE.md"),
        claude_config.join("CLAUDE.md"),
        codex_home.join("AGENTS.override.md"),
        codex_home.join("AGENTS.md"),
        codex_home.join("config.toml"),
        pi_agent.join("AGENTS.override.md"),
        pi_agent.join("AGENTS.md"),
        pi_agent.join("AGENTS.MD"),
        pi_agent.join("CLAUDE.md"),
        pi_agent.join("CLAUDE.MD"),
    ] {
        hash_file(&path, &mut hasher);
    }
    hash_rules(&claude_config.join("rules"), "md", &mut hasher);
    // Claude's settings chain includes the main checkout's local settings from a linked
    // worktree; new managed drop-ins join it.
    for path in claude_settings_files(directory, paths) {
        hash_file(&path, &mut hasher);
    }
    hash_all(
        &claude_managed_dir().join("managed-settings.d"),
        &mut hasher,
    );

    for ancestor in directory.ancestors() {
        for name in ANCESTOR_CANDIDATES {
            hash_file(&ancestor.join(name), &mut hasher);
        }
        hash_rules(&ancestor.join(".claude/rules"), "md", &mut hasher);
        hash_rules(&ancestor.join(".cursor/rules"), "mdc", &mut hasher);
    }

    // Codex reads its fallback filenames only from its project root, the nearest ancestor with
    // a root marker, down to the launch directory; a marker appearing or disappearing on that
    // chain moves the root.
    for ancestor in directory.ancestors() {
        for name in &codex.fallback_names {
            hash_file(&ancestor.join(name), &mut hasher);
        }
        let markers = codex
            .root_markers
            .iter()
            .map(|marker| ancestor.join(marker).exists())
            .collect::<Vec<_>>();
        markers.hash(&mut hasher);
        if markers.contains(&true) {
            break;
        }
    }

    // The Git worktree root is asked for on every check, so a repository created or removed
    // during the session changes Cursor's root, Project discovery and the subfolder scope.
    let worktree = crate::git_worktree::worktree_root(directory).ok();
    worktree.hash(&mut hasher);
    hash_file(
        &worktree
            .as_deref()
            .unwrap_or(directory)
            .join(".cursorrules"),
        &mut hasher,
    );
    // Project discovery looks for a manifest from the launch directory up to the worktree root.
    let canonical = fs::canonicalize(directory).unwrap_or_else(|_| directory.to_owned());
    for ancestor in canonical.ancestors() {
        hash_file(&ancestor.join(".mdmanager/project.toml"), &mut hasher);
        if worktree.as_deref() == Some(ancestor) {
            break;
        }
    }
    // Each declared Section entry counts by where it resolves, not by its contents: contents
    // inside the Project are known files, and contents outside it are never read.
    for section in crate::project::declared_sections(directory) {
        section.path.hash(&mut hasher);
        fs::symlink_metadata(&section.path)
            .map(|metadata| metadata.file_type().is_symlink())
            .ok()
            .hash(&mut hasher);
        fs::read_link(&section.path).ok().hash(&mut hasher);
        fs::canonicalize(&section.path).ok().hash(&mut hasher);
    }
    // Subfolder discovery reloads only inside Git worktrees.
    if worktree.is_some() {
        hash_descendants(directory, paths, &mut hasher);
    }
    hasher.finish()
}

#[cfg(test)]
pub(super) fn current_watch_signature(input: &WatchInput) -> WatchSignature {
    WatchSignature::observe(input)
}

/// One refresh generation of Project, Local and Global discovery, Context resolution and
/// inspection, observed together off the input thread.
pub(super) struct Observations {
    pub(super) project: Option<Workspace>,
    pub(super) project_error: Option<String>,
    pub(super) project_root: PathBuf,
    pub(super) global: Option<GlobalConfig>,
    pub(super) global_error: Option<String>,
    pub(super) global_active_profile: Option<String>,
    pub(super) local_repository: Option<LocalRepository>,
    pub(super) global_sources: Vec<(ContextRuntime, ContextSource)>,
    // Its `context` is the selected runtime's Audit, before any Claude descendant scan.
    pub(super) inspection: InstructionInspection,
    // The watch signature of these observations' own inputs, hashed before discovery read them.
    pub(super) signature: WatchSignature,
}

impl Observations {
    pub(super) fn discover(
        paths: &Paths,
        directory: &Path,
        runtime: ContextRuntime,
        global: Option<GlobalConfig>,
        global_error: Option<String>,
        document_paths: &[PathBuf],
        baseline: &WatchSignature,
    ) -> Self {
        let (project, project_error) = match Workspace::discover(directory) {
            Ok(project) => (project, None),
            Err(error) => (None, Some(error)),
        };
        // A Project that fails to load still declares its Sections; observing them keeps a
        // missing or broken Section watched until it is restored. Like the Project loader, this
        // never reads a Section that resolves outside `.mdmanager/`.
        let document_paths = document_paths
            .iter()
            .cloned()
            .chain(
                project_error
                    .is_some()
                    .then(|| crate::project::declared_sections(directory))
                    .into_iter()
                    .flatten()
                    .filter(|section| !section.outside)
                    .map(|section| section.path),
            )
            .collect::<Vec<_>>();
        let worktree_root = crate::git_worktree::worktree_root(directory).ok();
        let project_root = project.as_ref().map_or_else(
            || worktree_root.unwrap_or_else(|| directory.to_owned()),
            |project| project.root.clone(),
        );
        let local_repository = LocalRepository::discover(directory, paths).ok();
        let (global_active_profile, global_error) =
            match global.as_ref().map_or(Ok(None), deploy::active_profile) {
                Ok(profile) => (profile, global_error),
                Err(error) => (None, Some(error)),
            };
        let global_sources = discovered_global_sources(
            paths,
            directory,
            global.as_ref(),
            global_active_profile.as_deref(),
        );
        let context = Audit::resolve(runtime, directory, paths, global.as_ref());
        let inspection = InstructionInspection::observe(&InspectionInput {
            context: &context,
            global: &global,
            global_active_profile: &global_active_profile,
            global_error: &global_error,
            global_sources: &global_sources,
            local_repository: &local_repository,
            project: &project,
            project_error: &project_error,
            project_root: &project_root,
            document_paths: document_paths.iter().map(PathBuf::as_path).collect(),
        });
        let signature = baseline.extended(&known_paths(&inspection));
        Self {
            project,
            project_error,
            project_root,
            global,
            global_error,
            global_active_profile,
            local_repository,
            global_sources,
            inspection,
            signature,
        }
    }
}

fn discovered_global_sources(
    paths: &Paths,
    directory: &Path,
    global: Option<&GlobalConfig>,
    active_profile: Option<&str>,
) -> Vec<(ContextRuntime, ContextSource)> {
    let configured = global.map_or_else(Vec::new, |global| {
        let ids: Vec<&str> = match active_profile {
            Some(profile) => global
                .profile_target_names(profile)
                .map(|ids| ids.collect())
                .unwrap_or_default(),
            None => global.target_names().collect(),
        };
        ids.into_iter()
            .filter_map(|id| global.target_path(id).ok())
            .collect()
    });
    let mut seen = HashSet::new();
    ContextRuntime::ALL
        .into_iter()
        .flat_map(|runtime| {
            // Cursor has no user instruction file; its always-applied home rules play that role,
            // and both Cursor runtimes report the same files.
            let cursor = matches!(
                runtime,
                ContextRuntime::Cursor | ContextRuntime::CursorAgent
            );
            Audit::resolve(runtime, directory, paths, None)
                .sources
                .into_iter()
                .filter(move |source| {
                    (source.scope == "user" || cursor && source.scope == "user rule")
                        && source.group == SourceGroup::Startup
                        && matches!(
                            source.state,
                            SourceState::Startup
                                | SourceState::SelectedEmpty
                                | SourceState::Truncated { .. }
                        )
                })
                .take(if cursor { usize::MAX } else { 1 })
                .map(move |source| (runtime, source))
        })
        .filter(|(_, source)| {
            !configured.contains(&source.path) && seen.insert(source.path.clone())
        })
        .collect()
}

/// What the session asks the reload worker to check, taken from its accepted observations.
pub(super) struct RefreshRequest {
    pub(super) watch: WatchInput,
    pub(super) runtime: ContextRuntime,
    // Open and historical documents, observed again by a refresh.
    pub(super) document_paths: Vec<PathBuf>,
}

struct Finished {
    topology: Topology,
    observations: Option<Observations>,
}

/// Compares the watch signature with `signature` and refreshes only when it changed; without
/// a signature it refreshes unconditionally.
fn check(
    request: RefreshRequest,
    signature: Option<WatchSignature>,
    mut topology: Topology,
) -> Finished {
    let kept = topology.revalidate(&request.watch.directory);
    let baseline = topology.scope(|| WatchSignature::observe(&request.watch));
    let changed = signature.is_none_or(|signature| baseline != signature);
    let observations = changed.then(|| {
        // A refresh asks Git once for its generation instead of trusting answers kept between
        // quiet checks; answers this check already asked for again are this generation's.
        if kept {
            topology.forget();
        }
        topology.scope(|| {
            let paths = &request.watch.paths;
            let (global, global_error) = if paths.config.exists() {
                match GlobalConfig::load(paths) {
                    Ok(global) => (Some(global), None),
                    Err(error) => (None, Some(error)),
                }
            } else {
                (None, None)
            };
            Observations::discover(
                paths,
                &request.watch.directory,
                request.runtime,
                global,
                global_error,
                &request.document_paths,
                &baseline,
            )
        })
    });
    Finished {
        topology,
        observations,
    }
}

struct Pending {
    generation: u64,
    runtime: ContextRuntime,
    notify: bool,
    receiver: Receiver<Finished>,
}

/// A refresh the session accepted from the worker; `notify` announces a detected disk change.
pub(super) struct Refreshed {
    pub(super) observations: Observations,
    pub(super) notify: bool,
}

/// Runs watch checks and refreshes on a worker thread, at most one at a time, and hands the
/// session only results that are still current.
pub(super) struct ReloadWatcher {
    pub(super) signature: WatchSignature,
    topology: Topology,
    // Raised by every started check and every explicit refresh request; a result is current only
    // while its generation is the latest.
    generation: u64,
    pending: Option<Pending>,
    // A refresh the session asked for; `Some(true)` announces it as a disk change.
    requested: Option<bool>,
    last_watch: Instant,
}

impl ReloadWatcher {
    pub(super) fn new(signature: WatchSignature, topology: Topology) -> Self {
        Self {
            signature,
            topology,
            generation: 0,
            pending: None,
            requested: None,
            last_watch: Instant::now(),
        }
    }

    /// Git answers of the accepted generation, for discovery the session runs itself.
    pub(super) fn topology(&mut self) -> &mut Topology {
        &mut self.topology
    }

    /// Discards the result of any running check.
    pub(super) fn invalidate(&mut self) {
        self.generation += 1;
    }

    /// Discards any running check and refreshes as soon as the worker is free.
    pub(super) fn request_refresh(&mut self, notify: bool) {
        self.invalidate();
        self.requested = Some(notify || self.requested == Some(true));
    }

    pub(super) fn due(&self) -> bool {
        self.pending.is_none()
            && (self.requested.is_some() || self.last_watch.elapsed() >= AUTO_RELOAD_INTERVAL)
    }

    pub(super) fn start(&mut self, request: RefreshRequest) {
        let requested = self.requested.take();
        let signature = requested.is_none().then(|| self.signature.clone());
        let topology = self.topology.clone();
        let runtime = request.runtime;
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let _ = sender.send(check(request, signature, topology));
        });
        self.generation += 1;
        self.pending = Some(Pending {
            generation: self.generation,
            runtime,
            notify: requested.unwrap_or(true),
            receiver,
        });
    }

    /// Returns a finished refresh while it is current: the session still shows `runtime` and
    /// requested no newer generation since the check started.
    pub(super) fn poll(&mut self, runtime: ContextRuntime) -> Option<Refreshed> {
        let finished = match self.pending.as_ref()?.receiver.try_recv() {
            Ok(finished) => Some(finished),
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => None,
        };
        let pending = self.pending.take()?;
        self.last_watch = Instant::now();
        let finished = finished?;
        self.topology = finished.topology;
        let observations = finished.observations?;
        if pending.generation != self.generation || pending.runtime != runtime {
            return None;
        }
        self.signature = observations.signature.clone();
        Some(Refreshed {
            observations,
            notify: pending.notify,
        })
    }

    #[cfg(test)]
    pub(super) fn is_pending(&self) -> bool {
        self.pending.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::fixture;
    use super::super::*;
    use super::*;
    use std::fs;
    use std::process::Command;

    use tempfile::TempDir;

    #[test]
    fn watcher_ignores_runtime_session_churn() {
        let (home, repository, _paths, app) = fixture();
        let initial = current_watch_signature(&app.watch_input());
        let transcript = home
            .path()
            .join(".claude/projects/session/transcript.jsonl");
        fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        fs::write(transcript, "session output\n").unwrap();
        assert_eq!(current_watch_signature(&app.watch_input()), initial);

        // Files no discovery reads change their directories' metadata, not the signature.
        let rules = repository.path().join(".claude/rules");
        fs::create_dir_all(&rules).unwrap();
        fs::write(rules.join("seed.md"), "rule\n").unwrap();
        fs::create_dir_all(repository.path().join(".cursor/rules")).unwrap();
        let before_churn = current_watch_signature(&app.watch_input());
        for churn in [
            "notes/build.log",
            "__pycache__/x.pyc",
            ".AGENTS.md.swp",
            ".venv/lib/settings.json",
            ".swarmr/report.md",
            ".claude/rules/.seed.md.swp",
            // Each rule directory loads only its own runtime's rule format.
            ".claude/rules/cursor-format.mdc",
            ".cursor/rules/claude-format.md",
            // Runtime-like names count only where a resolver looks for them.
            ".swarmr/reviews/.cursorrules",
            ".swarmr/reviews/.mdmanager/build.log",
            ".swarmr/reviews/.claude/settings.json",
        ] {
            let path = repository.path().join(churn);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, "churn\n").unwrap();
            assert_eq!(
                current_watch_signature(&app.watch_input()),
                before_churn,
                "{churn}"
            );
            fs::remove_file(&path).unwrap();
            assert_eq!(
                current_watch_signature(&app.watch_input()),
                before_churn,
                "{churn}"
            );
        }
        // Creating or removing a rule directory that holds no supported rule is churn too.
        let cursor = repository.path().join(".cursor");
        fs::remove_dir_all(&cursor).unwrap();
        assert_eq!(current_watch_signature(&app.watch_input()), before_churn);
        fs::create_dir_all(cursor.join("rules")).unwrap();
        fs::write(cursor.join("rules/first.md"), "churn\n").unwrap();
        assert_eq!(current_watch_signature(&app.watch_input()), before_churn);
        fs::remove_dir_all(&cursor).unwrap();

        // Claude's subfolder scan reports instructions inside dependency and build directories.
        for nested in [
            "notes/CLAUDE.md",
            "node_modules/a/CLAUDE.md",
            "target/a/CLAUDE.md",
        ] {
            let nested = repository.path().join(nested);
            fs::create_dir_all(nested.parent().unwrap()).unwrap();
            fs::write(&nested, "# Nested\n").unwrap();
            let with_nested = current_watch_signature(&app.watch_input());
            assert_ne!(with_nested, before_churn, "{nested:?}");
            fs::remove_file(&nested).unwrap();
            assert_eq!(current_watch_signature(&app.watch_input()), before_churn);
        }

        let instruction = home.path().join(".claude/CLAUDE.md");
        fs::write(instruction, "# Global instructions\n").unwrap();
        assert_ne!(current_watch_signature(&app.watch_input()), initial);
    }

    #[test]
    fn runtime_switch_discards_a_running_check() {
        let (_home, repository, _paths, mut app) = fixture();
        fs::write(repository.path().join("AGENTS.md"), "# Changed\n").unwrap();
        app.watcher.request_refresh(true);
        app.poll_reload();
        assert!(app.watcher.is_pending());
        let pi = ContextRuntime::ALL
            .iter()
            .position(|runtime| *runtime == ContextRuntime::Pi)
            .unwrap();
        app.select_runtime(pi);
        while app.watcher.is_pending() {
            std::thread::yield_now();
            app.poll_reload();
        }
        // The Claude refresh finished but was discarded, so the change is still unaccepted
        // and the next check finds it.
        assert_eq!(app.context.audit.runtime, ContextRuntime::Pi);
        assert!(app.message.is_none());
        assert_ne!(
            app.watcher.signature,
            current_watch_signature(&app.watch_input())
        );
        app.reload_from_disk();
        assert_eq!(app.inspection.context.runtime, ContextRuntime::Pi);
        assert_eq!(
            app.watcher.signature,
            current_watch_signature(&app.watch_input())
        );
    }

    #[test]
    fn watcher_tracks_instruction_names_from_configuration_and_filesystem_case() {
        let (home, repository, _paths, app) = fixture();
        let codex = home.path().join(".codex");
        fs::create_dir_all(&codex).unwrap();
        fs::write(
            codex.join("config.toml"),
            "project_doc_fallback_filenames = [\"PROJECT_GUIDE.md\"]\n",
        )
        .unwrap();
        let nested = repository.path().join("nested");
        fs::create_dir(&nested).unwrap();
        // Codex reads fallback names only from its project root down to the launch directory.
        for (created, read) in [
            (repository.path().join("PROJECT_GUIDE.md"), true),
            (nested.join("PROJECT_GUIDE.md"), false),
            (
                repository.path().join(".swarmr/reviews/PROJECT_GUIDE.md"),
                false,
            ),
        ] {
            let before = current_watch_signature(&app.watch_input());
            fs::create_dir_all(created.parent().unwrap()).unwrap();
            fs::write(&created, "created\n").unwrap();
            assert_eq!(
                current_watch_signature(&app.watch_input()) != before,
                read,
                "{created:?}"
            );
        }

        // Claude's subfolder scan accepts another spelling only where the filesystem resolves
        // it to CLAUDE.md.
        let before = current_watch_signature(&app.watch_input());
        fs::write(nested.join("ClAuDe.md"), "mixed case\n").unwrap();
        let case_insensitive = nested.join("CLAUDE.md").exists();
        assert_eq!(
            current_watch_signature(&app.watch_input()) != before,
            case_insensitive
        );
    }

    #[cfg(unix)]
    #[test]
    fn watcher_follows_linked_claude_directories_like_the_rule_resolver() {
        use std::os::unix::fs::symlink;

        let (_home, repository, _paths, app) = fixture();
        let external = TempDir::new().unwrap();
        let first = external.path().join("first");
        let second = external.path().join("second");
        fs::create_dir_all(first.join("rules")).unwrap();
        fs::create_dir_all(second.join("rules")).unwrap();
        fs::write(second.join("rules/retargeted.md"), "rule\n").unwrap();
        let link = repository.path().join(".claude");
        symlink(&first, &link).unwrap();

        let before = current_watch_signature(&app.watch_input());
        fs::write(first.join("rules/added.md"), "rule\n").unwrap();
        assert_ne!(current_watch_signature(&app.watch_input()), before);

        // The resolver skips no directory inside a rule tree.
        let before = current_watch_signature(&app.watch_input());
        fs::create_dir_all(first.join("rules/node_modules")).unwrap();
        fs::write(first.join("rules/node_modules/new-rule.md"), "rule\n").unwrap();
        assert_ne!(current_watch_signature(&app.watch_input()), before);

        let before = current_watch_signature(&app.watch_input());
        let staged = repository.path().join(".claude-next");
        symlink(&second, &staged).unwrap();
        fs::rename(&staged, &link).unwrap();
        assert_ne!(current_watch_signature(&app.watch_input()), before);

        // A retarget to a rule tree with the same names, sizes and times still counts.
        let third = external.path().join("third");
        fs::create_dir_all(third.join("rules")).unwrap();
        let original = second.join("rules/retargeted.md");
        let copy = third.join("rules/retargeted.md");
        fs::write(&copy, "rule\n").unwrap();
        fs::File::options()
            .write(true)
            .open(&copy)
            .unwrap()
            .set_modified(fs::metadata(&original).unwrap().modified().unwrap())
            .unwrap();
        let before = current_watch_signature(&app.watch_input());
        symlink(&third, &staged).unwrap();
        fs::rename(&staged, &link).unwrap();
        assert_ne!(current_watch_signature(&app.watch_input()), before);

        // A linked directory with no supported rule changes nothing the resolver reads.
        let before = current_watch_signature(&app.watch_input());
        let empty = external.path().join("empty");
        fs::create_dir(&empty).unwrap();
        symlink(&empty, third.join("rules/linked-empty")).unwrap();
        assert_eq!(current_watch_signature(&app.watch_input()), before);
    }

    #[test]
    fn refresh_accepts_the_state_hashed_before_discovery() {
        let (_home, repository, paths, app) = fixture();
        let baseline = WatchSignature::observe(&app.watch_input());
        // An edit that lands while discovery runs must still differ at the next check.
        fs::write(
            repository.path().join("AGENTS.md"),
            "# Edited mid-refresh\n",
        )
        .unwrap();
        let observations = Observations::discover(
            &paths,
            &app.inspection.context.directory,
            app.context.audit.runtime,
            None,
            None,
            &[],
            &baseline,
        );
        assert_eq!(observations.signature, baseline);
        let refreshed = WatchInput::new(&observations.inspection, &paths);
        assert_ne!(current_watch_signature(&refreshed), observations.signature);
    }

    #[test]
    fn non_git_watcher_tracks_known_project_sections() {
        let home = TempDir::new().unwrap();
        let directory = TempDir::new().unwrap();
        fs::write(directory.path().join("AGENTS.md"), "# Agents\n").unwrap();
        crate::project::adopt(directory.path(), "agents").unwrap();
        let mut app = App::new_at(
            Paths::for_home(home.path()),
            None,
            directory.path().to_owned(),
        )
        .unwrap();
        let section = app
            .project
            .as_ref()
            .unwrap()
            .section_path("agents")
            .unwrap();
        assert!(crate::git_worktree::worktree_root(directory.path()).is_err());
        let before = current_watch_signature(&app.watch_input());
        fs::write(&section, "# Edited Section\n").unwrap();
        assert_ne!(current_watch_signature(&app.watch_input()), before);
        app.reload_from_disk();
        assert_eq!(
            &*app.inspection.document(&section).unwrap(),
            "# Edited Section\n"
        );
    }

    #[test]
    fn watcher_widens_to_subfolders_when_a_repository_is_created() {
        let home = TempDir::new().unwrap();
        let directory = TempDir::new().unwrap();
        let mut app = App::new_at(
            Paths::for_home(home.path()),
            None,
            directory.path().to_owned(),
        )
        .unwrap();
        let before = current_watch_signature(&app.watch_input());
        crate::git_worktree::git(directory.path(), &["init", "-q"]).unwrap();
        assert_ne!(current_watch_signature(&app.watch_input()), before);
        app.reload_from_disk();
        let before = current_watch_signature(&app.watch_input());
        fs::create_dir(directory.path().join("sub")).unwrap();
        fs::write(directory.path().join("sub/CLAUDE.md"), "# Sub\n").unwrap();
        assert_ne!(current_watch_signature(&app.watch_input()), before);
    }

    // A Project whose Section name no subfolder rule matches, so only Project discovery reads it.
    fn common_project(root: &Path) {
        fs::create_dir_all(root.join(".mdmanager/sections")).unwrap();
        fs::write(root.join(".mdmanager/sections/common.md"), "# Common\n").unwrap();
        fs::write(
            root.join(".mdmanager/project.toml"),
            "format = 1\n[[sections]]\nid = \"common\"\nname = \"Common\"\npath = \"sections/common.md\"\n\n[targets.agents]\nsections = [\"common\"]\n",
        )
        .unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn watcher_keeps_a_missing_project_section_watched() {
        let (_home, repository, _paths, mut app) = fixture();
        common_project(repository.path());
        app.reload_from_disk();
        let section = app
            .project
            .as_ref()
            .unwrap()
            .section_path("common")
            .unwrap();
        let content = fs::read_to_string(&section).unwrap();
        fs::remove_file(&section).unwrap();
        app.reload_from_disk();
        assert!(app.project_error.is_some());
        // Restoring the Section is a change while the Project still fails to load.
        fs::write(&section, content).unwrap();
        assert_ne!(
            current_watch_signature(&app.watch_input()),
            app.watcher.signature
        );
        app.reload_from_disk();
        assert!(app.project_error.is_none());

        // A Section linked outside `.mdmanager/` fails to load; its target's contents are never
        // read or watched, but replacing the link still recovers.
        let outside = TempDir::new().unwrap();
        let target = outside.path().join("common.md");
        fs::write(&target, "# Outside\n").unwrap();
        fs::remove_file(&section).unwrap();
        std::os::unix::fs::symlink(&target, &section).unwrap();
        assert_ne!(
            current_watch_signature(&app.watch_input()),
            app.watcher.signature
        );
        app.reload_from_disk();
        assert!(app.project_error.is_some());
        assert!(!app.inspection.files.contains_key(&section));
        fs::write(&target, "# Outside, edited\n").unwrap();
        assert_eq!(
            current_watch_signature(&app.watch_input()),
            app.watcher.signature
        );
        fs::remove_file(&section).unwrap();
        fs::write(&section, "# Common\n").unwrap();
        assert_ne!(
            current_watch_signature(&app.watch_input()),
            app.watcher.signature
        );
        app.reload_from_disk();
        assert!(app.project_error.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn watcher_follows_a_linked_state_directory() {
        use std::os::unix::fs::symlink;

        let (home, _repository, paths, app) = fixture();
        let external = TempDir::new().unwrap();
        for name in ["first", "second"] {
            fs::create_dir(external.path().join(name)).unwrap();
            fs::write(external.path().join(name).join("state.toml"), name).unwrap();
        }
        fs::create_dir_all(&paths.data_dir).unwrap();
        symlink(external.path().join("first"), &paths.state_dir).unwrap();
        let before = current_watch_signature(&app.watch_input());
        let staged = home.path().join("state-next");
        symlink(external.path().join("second"), &staged).unwrap();
        fs::rename(&staged, &paths.state_dir).unwrap();
        assert_ne!(current_watch_signature(&app.watch_input()), before);
    }

    #[test]
    fn context_changes_keep_pending_edits_to_known_files() {
        let home = TempDir::new().unwrap();
        let directory = TempDir::new().unwrap();
        common_project(directory.path());
        let mut app = App::new_at(
            Paths::for_home(home.path()),
            None,
            directory.path().to_owned(),
        )
        .unwrap();
        let section = app
            .project
            .as_ref()
            .unwrap()
            .section_path("common")
            .unwrap();
        // An edit lands before the next check, then the runtime switch adds new Context files.
        fs::write(&section, "# Edited before the check\n").unwrap();
        app.select_runtime(
            ContextRuntime::ALL
                .iter()
                .position(|runtime| *runtime == ContextRuntime::Pi)
                .unwrap(),
        );
        assert_ne!(
            current_watch_signature(&app.watch_input()),
            app.watcher.signature
        );
    }

    #[test]
    fn watcher_follows_claude_selection_of_nested_agents_md() {
        let (home, repository, _paths, app) = fixture();
        let before = current_watch_signature(&app.watch_input());
        fs::create_dir(repository.path().join("notes")).unwrap();
        fs::write(repository.path().join("notes/AGENTS.md"), "# Notes\n").unwrap();
        assert_ne!(current_watch_signature(&app.watch_input()), before);

        fs::create_dir_all(home.path().join(".claude")).unwrap();
        fs::write(
            home.path().join(".claude/settings.json"),
            r#"{"enabledPlugins": {"cc-plugin-agents-md@builtin": false}}"#,
        )
        .unwrap();
        let before = current_watch_signature(&app.watch_input());
        let draft = repository.path().join(".swarmr/reviews/AGENTS.md");
        fs::create_dir_all(draft.parent().unwrap()).unwrap();
        fs::write(&draft, "# Draft\n").unwrap();
        assert_eq!(current_watch_signature(&app.watch_input()), before);
    }

    #[test]
    fn watcher_tracks_instruction_files_above_the_repository() {
        let home = TempDir::new().unwrap();
        let workspace = TempDir::new().unwrap();
        let repository = workspace.path().join("repo");
        let directory = repository.join("child");
        fs::create_dir_all(&directory).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .current_dir(&repository)
                .status()
                .unwrap()
                .success()
        );
        let ancestor = workspace.path().join("CLAUDE.md");
        fs::write(&ancestor, "# Before\n").unwrap();
        let paths = Paths::for_home(home.path());
        fs::create_dir_all(paths.config.parent().unwrap().join("sections")).unwrap();
        fs::write(
            paths.config.parent().unwrap().join("sections/common.md"),
            "# Common\n",
        )
        .unwrap();
        fs::write(
            &paths.config,
            r#"
[[sections]]
id = "common"
name = "Common"
path = "sections/common.md"

[targets.custom]
path = "~/custom/AGENTS.md"
title = "Custom"

[profiles.default]
custom = ["common"]
"#,
        )
        .unwrap();
        let global = GlobalConfig::load(&paths).unwrap();
        let app = App::new_at(paths, Some(global), directory).unwrap();
        let signature = |app: &App| current_watch_signature(&app.watch_input());
        let initial = signature(&app);
        fs::write(ancestor, "# After\n").unwrap();
        assert_ne!(signature(&app), initial);

        // A Global target outside every runtime's default location still refreshes Home.
        let before = signature(&app);
        let custom = home.path().join("custom/AGENTS.md");
        fs::create_dir_all(custom.parent().unwrap()).unwrap();
        fs::write(custom, "# Edited\n").unwrap();
        assert_ne!(signature(&app), before);
    }

    #[test]
    fn watcher_discovers_new_cursor_rules() {
        use crate::context::SourceState;

        for git_repository in [false, true] {
            let home = TempDir::new().unwrap();
            let repository = TempDir::new().unwrap();
            if git_repository {
                assert!(
                    Command::new("git")
                        .args(["init", "-q"])
                        .current_dir(repository.path())
                        .status()
                        .unwrap()
                        .success()
                );
            }
            let rules = repository.path().join(".cursor/rules");
            fs::create_dir_all(&rules).unwrap();
            let candidates = vec![rules.join("new.mdc")];
            #[cfg(unix)]
            let (_external, candidates) = {
                use std::os::unix::fs::symlink;

                let mut candidates = candidates;
                let external = TempDir::new().unwrap();
                fs::create_dir_all(external.path().join("existing/deep")).unwrap();
                symlink(external.path(), rules.join("shared")).unwrap();
                symlink(external.path(), external.path().join("existing/cycle")).unwrap();
                candidates.push(rules.join("shared/added.mdc"));
                candidates.push(rules.join("shared/existing/deep/new.mdc"));
                (external, candidates)
            };
            let mut app = App::new_at(
                Paths::for_home(home.path()),
                None,
                repository.path().to_owned(),
            )
            .unwrap();
            app.select_runtime(
                ContextRuntime::ALL
                    .iter()
                    .position(|runtime| *runtime == ContextRuntime::Cursor)
                    .unwrap(),
            );
            let signature = |app: &App| current_watch_signature(&app.watch_input());
            let state = |app: &App, path: &Path| {
                app.context
                    .audit
                    .sources
                    .iter()
                    .find(|source| source.path == path)
                    .map(|source| source.state.clone())
            };
            for rule in candidates {
                for content in [
                    Some("---\nalwaysApply: true\n---\nNew rule\n"),
                    Some("---\nglobs: ['src/**']\n---\nChanged rule\n"),
                    None,
                ] {
                    let before = signature(&app);
                    let previous = state(&app, &rule);
                    if let Some(content) = content {
                        fs::write(&rule, content).unwrap();
                    } else {
                        fs::remove_file(&rule).unwrap();
                    }
                    assert_ne!(signature(&app), before, "undetected change: {rule:?}");
                    assert_eq!(state(&app, &rule), previous);
                    app.reload_from_disk();
                    match content {
                        Some(content) if content.contains("alwaysApply") => {
                            assert_eq!(state(&app, &rule), Some(SourceState::Startup));
                        }
                        Some(_) => assert!(matches!(
                            state(&app, &rule),
                            Some(SourceState::Conditional(_))
                        )),
                        None => assert_eq!(state(&app, &rule), None),
                    }
                    assert_eq!(app.watcher.signature, signature(&app));
                }
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn watcher_refreshes_linked_worktree_settings_and_external_claude_rules() {
        use crate::context::SourceState;
        use std::os::unix::fs::symlink;

        let home = TempDir::new().unwrap();
        let workspace = TempDir::new().unwrap();
        let main = workspace.path().join("main");
        let linked = workspace.path().join("linked");
        fs::create_dir(&main).unwrap();
        for arguments in [
            vec!["init", "-q"],
            vec![
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "--allow-empty",
                "-qm",
                "initial",
            ],
            vec!["worktree", "add", "-qb", "linked", linked.to_str().unwrap()],
        ] {
            assert!(
                Command::new("git")
                    .args(arguments)
                    .current_dir(&main)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let source = linked.join("CLAUDE.md");
        fs::write(&source, "shared").unwrap();
        fs::create_dir_all(main.join(".claude")).unwrap();
        fs::create_dir_all(linked.join(".claude/rules")).unwrap();
        let external = workspace.path().join("external");
        fs::create_dir_all(external.join("existing/deep")).unwrap();
        symlink(&external, linked.join(".claude/rules/shared")).unwrap();
        symlink(&external, external.join("existing/cycle")).unwrap();
        let unrelated = workspace.path().join("unrelated");
        fs::create_dir(&unrelated).unwrap();
        symlink(&unrelated, linked.join("ordinary-link")).unwrap();
        // Claude's subfolder scan skips only `.git` directories.
        fs::create_dir_all(linked.join("controls/.git/watch-control")).unwrap();

        let mut app = App::new_at(Paths::for_home(home.path()), None, linked.clone()).unwrap();
        let signature = |app: &App| current_watch_signature(&app.watch_input());
        let state = |app: &App, path: &Path| {
            app.context
                .audit
                .sources
                .iter()
                .find(|source| source.path == path)
                .map(|source| source.state.clone())
        };
        assert_eq!(state(&app, &source), Some(SourceState::Startup));
        let initial = signature(&app);
        fs::write(unrelated.join("CLAUDE.md"), "unrelated").unwrap();
        fs::write(
            linked.join("controls/.git/watch-control/CLAUDE.md"),
            "ignored",
        )
        .unwrap();
        assert_eq!(signature(&app), initial);
        let settings = main.join(".claude/settings.local.json");
        fs::write(
            &settings,
            serde_json::json!({"claudeMdExcludes": [source]}).to_string(),
        )
        .unwrap();
        assert_ne!(signature(&app), initial);
        assert_eq!(state(&app, &source), Some(SourceState::Startup));
        app.reload_from_disk();
        assert!(matches!(
            state(&app, &source),
            Some(SourceState::Excluded(_))
        ));
        assert_eq!(app.watcher.signature, signature(&app));

        let rule = linked.join(".claude/rules/shared/existing/deep/new.md");
        for content in [
            Some("new rule"),
            Some("---\npaths: ['src/**']\n---\nchanged rule"),
            None,
        ] {
            let before = signature(&app);
            let previous = state(&app, &rule);
            if let Some(content) = content {
                fs::write(&rule, content).unwrap();
            } else {
                fs::remove_file(&rule).unwrap();
            }
            assert_ne!(signature(&app), before);
            assert_eq!(state(&app, &rule), previous);
            app.reload_from_disk();
            match content {
                Some("new rule") => assert_eq!(state(&app, &rule), Some(SourceState::Startup)),
                Some(_) => assert!(matches!(
                    state(&app, &rule),
                    Some(SourceState::Conditional(_))
                )),
                None => assert_eq!(state(&app, &rule), None),
            }
            assert_eq!(app.watcher.signature, signature(&app));
        }
        let before = signature(&app);
        fs::remove_file(settings).unwrap();
        assert_ne!(signature(&app), before);
        app.reload_from_disk();
        assert_eq!(state(&app, &source), Some(SourceState::Startup));
    }

    #[test]
    fn non_git_watcher_tracks_candidates_without_scanning_the_directory_tree() {
        let home = TempDir::new().unwrap();
        let directory = home.path().join("workspace");
        fs::create_dir_all(directory.join("cache/nested")).unwrap();
        let app = App::new_at(Paths::for_home(home.path()), None, directory.clone()).unwrap();
        assert!(crate::git_worktree::worktree_root(&directory).is_err());
        let initial = current_watch_signature(&app.watch_input());

        fs::write(directory.join("cache/nested/unrelated.txt"), "changed\n").unwrap();
        assert_eq!(current_watch_signature(&app.watch_input()), initial);

        fs::write(directory.join("AGENTS.md"), "# Instructions\n").unwrap();
        assert_ne!(current_watch_signature(&app.watch_input()), initial);
    }
}
