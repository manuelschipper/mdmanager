use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::env;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant, UNIX_EPOCH};

use walkdir::{DirEntry, WalkDir};

use super::inspection::{InspectionInput, InstructionInspection};
use crate::config::{GlobalConfig, Paths};
use crate::context::{
    Audit, ContextRuntime, ContextSource, SourceGroup, SourceState, claude_config_dir,
    claude_managed_dir, pi_agent_dir,
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
    // Context sources and their imports.
    sources: Vec<PathBuf>,
    global_targets: Vec<PathBuf>,
    watch_root: Option<PathBuf>,
}

impl WatchInput {
    pub(super) fn new(
        audit: &Audit,
        paths: &Paths,
        global: Option<&GlobalConfig>,
        watch_root: &Option<PathBuf>,
    ) -> Self {
        Self {
            paths: paths.clone(),
            directory: audit.directory.clone(),
            sources: source_paths(audit),
            global_targets: global.map_or_else(Vec::new, |global| {
                global
                    .target_names()
                    .filter_map(|target| global.target_path(target).ok())
                    .collect()
            }),
            watch_root: watch_root.clone(),
        }
    }
}

fn hash_tree(path: &Path, hasher: &mut DefaultHasher, project_tree: bool) {
    hash_dependency_tree(path, hasher, project_tree, false);
}

fn hash_dependency_tree(
    path: &Path,
    hasher: &mut DefaultHasher,
    project_tree: bool,
    rule_directory: bool,
) {
    if !path.exists() {
        path.hash(hasher);
        0_u8.hash(hasher);
        return;
    }
    let walker = WalkDir::new(path)
        .follow_links(rule_directory)
        .into_iter()
        .filter_entry(|entry| !ignored_watch_entry(entry));
    for entry in walker.flatten() {
        // Claude and Cursor rule directories share the resolver's linked traversal.
        if !rule_directory
            && (entry.path().ends_with(".claude/rules") || entry.path().ends_with(".cursor/rules"))
            && entry.path().is_dir()
        {
            hash_dependency_tree(entry.path(), hasher, false, true);
        }
        // A directory's own metadata changes whenever anything inside it is added or removed;
        // only the files discovery reads, and where linked directories point, count.
        if entry.file_type().is_dir() {
            if entry.path_is_symlink() {
                entry.path().hash(hasher);
                fs::read_link(entry.path()).ok().hash(hasher);
            }
            continue;
        }
        if (project_tree && !relevant_project_file(entry.path()))
            || (rule_directory && !rule_file(entry.path()))
        {
            continue;
        }
        entry.path().hash(hasher);
        if entry.path_is_symlink() {
            fs::read_link(entry.path()).ok().hash(hasher);
        }
        let metadata = if entry.file_type().is_symlink() && entry.path().is_dir() {
            fs::symlink_metadata(entry.path())
        } else {
            fs::metadata(entry.path())
        };
        if let Ok(metadata) = metadata {
            metadata.len().hash(hasher);
            metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_nanos())
                .hash(hasher);
        }
    }
}

fn source_paths(audit: &Audit) -> Vec<PathBuf> {
    audit
        .sources
        .iter()
        .flat_map(|source| std::iter::once(&source.path).chain(&source.imports))
        .cloned()
        .collect()
}

fn hash_context_sources(sources: &[PathBuf]) -> u64 {
    let mut hasher = DefaultHasher::new();
    for source in sources {
        hash_tree(source, &mut hasher, false);
    }
    hasher.finish()
}

/// The watch signature keeps the accepted audit's own sources apart so a finished Claude
/// descendant scan can rehash just those without walking every watched tree.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct WatchSignature {
    files: u64,
    sources: u64,
}

impl WatchSignature {
    pub(super) fn observe_context(&mut self, audit: &Audit) {
        self.sources = hash_context_sources(&source_paths(audit));
    }
}

fn hash_ancestor_candidates(
    directory: &Path,
    watch_root: Option<&Path>,
    hasher: &mut DefaultHasher,
) {
    for ancestor in directory
        .ancestors()
        .filter(|ancestor| watch_root.is_none_or(|root| !ancestor.starts_with(root)))
    {
        for name in [
            "AGENTS.override.md",
            "AGENTS.md",
            "AGENTS.MD",
            "CLAUDE.md",
            "CLAUDE.MD",
            "CLAUDE.local.md",
            ".cursorrules",
            ".claude/CLAUDE.md",
            ".claude/AGENTS.md",
            ".claude/settings.json",
            ".claude/settings.local.json",
        ] {
            hash_tree(&ancestor.join(name), hasher, false);
        }
        hash_tree(&ancestor.join(".claude/rules"), hasher, true);
        hash_tree(&ancestor.join(".cursor/rules"), hasher, true);
    }
}

fn ignored_watch_entry(entry: &DirEntry) -> bool {
    entry.depth() > 0
        && entry.file_type().is_dir()
        && matches!(
            entry.file_name().to_str(),
            Some(".git" | "target" | "node_modules")
        )
}

fn inside(path: &Path, parent: &str, child: &str) -> bool {
    path.components()
        .zip(path.components().skip(1))
        .any(|(left, right)| left.as_os_str() == parent && right.as_os_str() == child)
}

fn extension(path: &Path) -> Option<&str> {
    path.extension().and_then(|extension| extension.to_str())
}

/// Claude reads `.md` rules and Cursor reads `.mdc` rules.
fn rule_file(path: &Path) -> bool {
    matches!(extension(path), Some("md" | "mdc"))
}

fn relevant_project_file(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    matches!(
        name,
        "AGENTS.md"
            | "AGENTS.MD"
            | "AGENTS.override.md"
            | "CLAUDE.md"
            | "CLAUDE.MD"
            | "CLAUDE.local.md"
            | ".cursorrules"
    ) || (matches!(name, "settings.json" | "settings.local.json")
        && path
            .parent()
            .is_some_and(|parent| parent.ends_with(".claude")))
        || path
            .components()
            .any(|component| component.as_os_str() == ".mdmanager")
        || (extension(path) == Some("md") && inside(path, ".claude", "rules"))
        || (extension(path) == Some("mdc") && inside(path, ".cursor", "rules"))
}

pub(super) fn current_watch_signature(input: &WatchInput) -> WatchSignature {
    let mut hasher = DefaultHasher::new();
    for path in [
        input.paths.config.clone(),
        input.paths.data_dir.clone(),
        input.paths.state_dir.clone(),
    ] {
        hash_tree(&path, &mut hasher, false);
    }
    // Global targets can live anywhere; `mdmanager status` compares each configured path.
    for path in &input.global_targets {
        hash_tree(path, &mut hasher, false);
    }
    let codex_home = if env::var_os("HOME").as_deref() == Some(input.paths.home.as_os_str()) {
        env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| input.paths.home.join(".codex"))
    } else {
        input.paths.home.join(".codex")
    };
    let claude_config = claude_config_dir(&input.paths);
    let pi_agent = pi_agent_dir(&input.paths);
    for path in [
        claude_config.join("CLAUDE.md"),
        claude_config.join("settings.json"),
        claude_config.join("settings.local.json"),
        claude_managed_dir().join("CLAUDE.md"),
        claude_managed_dir().join("managed-settings.json"),
        claude_managed_dir().join("managed-settings.d"),
        codex_home.join("AGENTS.override.md"),
        codex_home.join("AGENTS.md"),
        codex_home.join("config.toml"),
        pi_agent.join("AGENTS.override.md"),
        pi_agent.join("AGENTS.md"),
        pi_agent.join("AGENTS.MD"),
        pi_agent.join("CLAUDE.md"),
        pi_agent.join("CLAUDE.MD"),
    ] {
        hash_tree(&path, &mut hasher, false);
    }
    hash_dependency_tree(&claude_config.join("rules"), &mut hasher, false, true);
    if let Ok(main_checkout) = crate::git_worktree::worktree_root(&input.directory)
        .and_then(|root| crate::git_worktree::main_worktree(&root))
    {
        hash_tree(
            &main_checkout.join(".claude/settings.local.json"),
            &mut hasher,
            false,
        );
    }
    if let Some(root) = &input.watch_root {
        hash_tree(root, &mut hasher, true);
    }
    hash_ancestor_candidates(&input.directory, input.watch_root.as_deref(), &mut hasher);
    WatchSignature {
        files: hasher.finish(),
        sources: hash_context_sources(&input.sources),
    }
}

/// One refresh generation of Project, Local and Global discovery, Context resolution and
/// inspection, observed together off the input thread.
pub(super) struct Observations {
    pub(super) project: Option<Workspace>,
    pub(super) project_error: Option<String>,
    pub(super) watch_root: Option<PathBuf>,
    pub(super) project_root: PathBuf,
    pub(super) global: Option<GlobalConfig>,
    pub(super) global_error: Option<String>,
    pub(super) global_active_profile: Option<String>,
    pub(super) local_repository: Option<LocalRepository>,
    pub(super) global_sources: Vec<(ContextRuntime, ContextSource)>,
    // Its `context` is the selected runtime's Audit, before any Claude descendant scan.
    pub(super) inspection: InstructionInspection,
    // The watch signature of these observations' own inputs.
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
    ) -> Self {
        let (project, project_error) = match Workspace::discover(directory) {
            Ok(project) => (project, None),
            Err(error) => (None, Some(error)),
        };
        let watch_root = crate::git_worktree::worktree_root(directory).ok();
        let project_root = project.as_ref().map_or_else(
            || watch_root.clone().unwrap_or_else(|| directory.to_owned()),
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
        let signature = current_watch_signature(&WatchInput::new(
            &context,
            paths,
            global.as_ref(),
            &watch_root,
        ));
        Self {
            project,
            project_error,
            watch_root,
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
    topology.revalidate(&request.watch.directory);
    let changed = signature.is_none_or(|signature| {
        topology.scope(|| current_watch_signature(&request.watch)) != signature
    });
    let observations = changed.then(|| {
        // A refresh asks Git once for its generation instead of trusting answers kept between
        // quiet checks.
        topology.forget();
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
        let signature = requested.is_none().then_some(self.signature);
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
        self.signature = observations.signature;
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
        let before_churn = current_watch_signature(&app.watch_input());
        for churn in [
            "notes/build.log",
            "__pycache__/x.pyc",
            ".AGENTS.md.swp",
            ".venv/lib/settings.json",
            ".swarmr/report.md",
            ".claude/rules/.seed.md.swp",
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
        let nested = repository.path().join("notes/CLAUDE.md");
        fs::write(&nested, "# Nested\n").unwrap();
        let with_nested = current_watch_signature(&app.watch_input());
        assert_ne!(with_nested, before_churn);
        fs::remove_file(nested).unwrap();
        assert_ne!(current_watch_signature(&app.watch_input()), with_nested);

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
        for ignored in [".git", "target", "node_modules"] {
            fs::create_dir_all(linked.join("controls").join(ignored).join("watch-control"))
                .unwrap();
        }

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
        for ignored in [".git", "target", "node_modules"] {
            fs::write(
                linked
                    .join("controls")
                    .join(ignored)
                    .join("watch-control/CLAUDE.md"),
                "ignored",
            )
            .unwrap();
        }
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
        assert!(app.watch_root.is_none());
        let initial = current_watch_signature(&app.watch_input());

        fs::write(directory.join("cache/nested/unrelated.txt"), "changed\n").unwrap();
        assert_eq!(current_watch_signature(&app.watch_input()), initial);

        fs::write(directory.join("AGENTS.md"), "# Instructions\n").unwrap();
        assert_ne!(current_watch_signature(&app.watch_input()), initial);
    }
}
