//! Local scope: machine-local instruction files (CLAUDE.local.md, AGENTS.override.md) and local disables for one Git worktree.
//! The ownership-state file keeps its `overlays.toml` name and emitted `overlay` error text is unchanged.

mod claude_settings;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::config::{GlobalConfig, Paths, atomic_create, atomic_write, sha256_hex};
use crate::context::PI_INSTRUCTION_CANDIDATES;
use crate::deploy::unified_diff;
use crate::git_worktree::git;

const VERIFIED_CODEX_EMPTY_OVERRIDE_VERSIONS: [&str; 1] = ["0.147.0"];

/// Local-disable runtime: a runtime with a verified local disable mechanism.
/// Unsupported: Codex local disable; `parse()` rejects `codex` with the reason.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DisableRuntime {
    Claude,
    Pi,
}

impl DisableRuntime {
    pub(crate) fn parse(raw: &str) -> Result<Self, String> {
        match raw {
            "claude" => Ok(Self::Claude),
            "pi" => Ok(Self::Pi),
            "codex" => Err("Codex has no verified local-disable mechanism".into()),
            _ => Err(format!(
                "unknown local-disable runtime {raw}; expected claude or pi"
            )),
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Pi => "Pi",
        }
    }

    pub(crate) const fn key(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Pi => "pi",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Local target status: how AGENTS.override.md or CLAUDE.local.md compares with the
/// rendered Local composition. `External` means the file exists but mdmanager does
/// not own it. `label()` owns its displayed vocabulary.
pub(crate) enum LocalTargetStatus {
    Current,
    Stale,
    Modified,
    Missing,
    External,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ManagedTarget {
    Agents,
    Claude,
}

impl ManagedTarget {
    pub(crate) fn parse(raw: &str) -> Result<Self, String> {
        match raw {
            "agents" => Ok(Self::Agents),
            "claude" => Ok(Self::Claude),
            _ => Err(format!(
                "unknown local target {raw}; expected agents or claude"
            )),
        }
    }

    pub(crate) const fn id(self) -> &'static str {
        match self {
            Self::Agents => "agents",
            Self::Claude => "claude",
        }
    }

    pub(crate) const fn filename(self) -> &'static str {
        match self {
            Self::Agents => "AGENTS.override.md",
            Self::Claude => "CLAUDE.local.md",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DisableStatus {
    None,
    Owned,
    Modified,
    Missing,
}

impl DisableStatus {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Owned => "owned",
            Self::Modified => "modified",
            Self::Missing => "missing",
        }
    }
}

impl LocalTargetStatus {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Stale => "out of sync",
            Self::Modified => "changed on disk",
            Self::Missing => "not found",
            Self::External => "existing · not managed by mdmanager.ai",
        }
    }
}

#[derive(Clone, Debug)]
/// Local repository: Local Instructions store for one Git worktree, discovered from a
/// launch directory inside it. Its data_dir and state_path (`overlays.toml`) live under
/// ~/.mdmanager keyed by the SHA-256 of the common Git directory so linked worktrees share
/// them, while worktree_key distinguishes the worktree.
pub(crate) struct LocalRepository {
    pub(crate) root: PathBuf,
    pub(crate) common_git_dir: PathBuf,
    paths: Paths,
    worktree_key: String,
    pub(crate) data_dir: PathBuf,
    pub(crate) state_path: PathBuf,
}

// Explicit unlock also releases the lock if a concurrently spawned Git process briefly
// inherited the descriptor before exec. The stable lock file itself is never removed.
struct LocalMutationLock(fs::File);

impl Drop for LocalMutationLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

#[derive(Clone, Debug)]
pub(crate) struct DisablePlan {
    pub(crate) runtime: DisableRuntime,
    pub(crate) source: PathBuf,
    pub(crate) output: PathBuf,
    pub(crate) text: String,
    reviewed_output: Option<String>,
    exclusion: Option<Exclusion>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ManagedView {
    pub(crate) path: PathBuf,
    pub(crate) rendered: String,
    pub(crate) status: LocalTargetStatus,
    pub(crate) diff: String,
    // Content comparison is independent of deployment ownership and CLI labels.
    pub(crate) difference: Option<String>,
    pub(crate) sections: Vec<LocalSection>,
    pub(crate) ordered: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LocalSection {
    pub(crate) id: String,
    pub(crate) name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ManagedPlan {
    target: ManagedTarget,
    pub(crate) view: ManagedView,
    exclusion: Exclusion,
}

impl ManagedPlan {
    pub(crate) fn exclusion_write(&self) -> Option<(&str, &Path)> {
        self.exclusion
            .needs_write
            .then_some((&self.exclusion.pattern, self.exclusion.path.as_path()))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct LocalManifest {
    format: u32,
    targets: std::collections::BTreeMap<String, LocalTarget>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct LocalTarget {
    sections: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RawOwned {
    path: String,
    hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    disabled_source: Option<String>,
    #[serde(default)]
    exclusion_path: Option<String>,
    #[serde(default)]
    exclusion_pattern: Option<String>,
    #[serde(default)]
    created_container: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    original_content: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RawState {
    #[serde(default)]
    disables: std::collections::BTreeMap<String, RawOwned>,
    #[serde(default)]
    managed: std::collections::BTreeMap<String, RawOwned>,
}

// Only validated ownership enters Local operations; RawState preserves overlays.toml's format.
#[derive(Clone, Debug)]
struct Owned {
    path: String,
    hash: String,
    exclusion: Option<OwnedExclusion>,
}

#[derive(Clone, Debug)]
struct OwnedExclusion {
    path: String,
    pattern: String,
}

#[derive(Clone, Debug)]
/// Restore removes a settings container we created, or restores the original bytes when unchanged.
enum ClaudeRecovery {
    Created,
    Existing(String),
}

#[derive(Clone, Debug)]
enum DisableOwned {
    Pi {
        owned: Owned,
        source: String,
    },
    Claude {
        owned: Owned,
        source: String,
        recovery: ClaudeRecovery,
    },
}

impl DisableOwned {
    fn owned(&self) -> &Owned {
        match self {
            Self::Pi { owned, .. } | Self::Claude { owned, .. } => owned,
        }
    }

    fn source(&self) -> &str {
        match self {
            Self::Pi { source, .. } | Self::Claude { source, .. } => source,
        }
    }
}

#[derive(Clone, Debug, Default)]
struct State {
    disables: std::collections::BTreeMap<String, DisableOwned>,
    managed: std::collections::BTreeMap<String, Owned>,
}

impl RawOwned {
    fn ownership(&self) -> Result<Owned, String> {
        let exclusion = match (&self.exclusion_path, &self.exclusion_pattern) {
            (Some(path), Some(pattern)) => Some(OwnedExclusion {
                path: path.clone(),
                pattern: pattern.clone(),
            }),
            (None, None) => None,
            _ => return Err("overlay ownership requires both exclusion path and pattern".into()),
        };
        Ok(Owned {
            path: self.path.clone(),
            hash: self.hash.clone(),
            exclusion,
        })
    }
}

// Validate recovery relationships at the serialized boundary so mutations receive coherent ownership.
impl TryFrom<RawState> for State {
    type Error = String;

    fn try_from(raw: RawState) -> Result<Self, String> {
        let mut state = Self::default();
        for (key, raw) in raw.managed {
            if raw.disabled_source.is_some()
                || raw.created_container
                || raw.original_content.is_some()
            {
                return Err("managed overlay ownership contains disable recovery data".into());
            }
            state.managed.insert(key, raw.ownership()?);
        }
        for (key, raw) in raw.disables {
            let owned = raw.ownership()?;
            let source = raw
                .disabled_source
                .ok_or("overlay disable ownership requires its disabled source")?;
            let disable = match key.split(':').next() {
                Some("pi")
                    if key.starts_with("pi:")
                        && !raw.created_container
                        && raw.original_content.is_none() =>
                {
                    DisableOwned::Pi { owned, source }
                }
                Some("claude") if key == "claude" => {
                    let recovery = match (raw.created_container, raw.original_content) {
                        (true, None) => ClaudeRecovery::Created,
                        (false, Some(original)) => {
                            claude_settings::parse_json_object(&original, Path::new(&owned.path))?;
                            ClaudeRecovery::Existing(original)
                        }
                        _ => {
                            return Err(
                                "Claude overlay ownership has inconsistent settings recovery data"
                                    .into(),
                            );
                        }
                    };
                    DisableOwned::Claude {
                        owned,
                        source,
                        recovery,
                    }
                }
                _ => return Err("invalid overlay disable ownership kind or recovery data".into()),
            };
            state.disables.insert(key, disable);
        }
        Ok(state)
    }
}

impl From<&Owned> for RawOwned {
    fn from(owned: &Owned) -> Self {
        Self {
            path: owned.path.clone(),
            hash: owned.hash.clone(),
            exclusion_path: owned
                .exclusion
                .as_ref()
                .map(|exclusion| exclusion.path.clone()),
            exclusion_pattern: owned
                .exclusion
                .as_ref()
                .map(|exclusion| exclusion.pattern.clone()),
            disabled_source: None,
            created_container: false,
            original_content: None,
        }
    }
}

impl From<&State> for RawState {
    fn from(state: &State) -> Self {
        Self {
            managed: state
                .managed
                .iter()
                .map(|(key, owned)| (key.clone(), owned.into()))
                .collect(),
            disables: state
                .disables
                .iter()
                .map(|(key, disable)| {
                    let mut raw = RawOwned::from(disable.owned());
                    raw.disabled_source = Some(disable.source().into());
                    if let DisableOwned::Claude { recovery, .. } = disable {
                        match recovery {
                            ClaudeRecovery::Created => raw.created_container = true,
                            ClaudeRecovery::Existing(original) => {
                                raw.original_content = Some(original.clone())
                            }
                        }
                    }
                    (key.clone(), raw)
                })
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Exclusion {
    path: PathBuf,
    pattern: String,
    needs_write: bool,
}

impl Exclusion {
    fn owned(&self) -> Option<OwnedExclusion> {
        self.needs_write.then(|| OwnedExclusion {
            path: self.path.display().to_string(),
            pattern: self.pattern.clone(),
        })
    }
}

fn validate_local_target(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(()),
        Ok(_) => Err(format!(
            "local target {} is not an ordinary file",
            path.display()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "cannot inspect local target {}: {error}",
            path.display()
        )),
    }
}

impl LocalRepository {
    pub(crate) fn discover(start: &Path, paths: &Paths) -> Result<Self, String> {
        let root = crate::git_worktree::worktree_root(start).map_err(|error| {
            if error == "project management requires a Git worktree" {
                "local management requires a Git worktree".to_owned()
            } else {
                error
            }
        })?;
        let common_output = git(&root, &["rev-parse", "--git-common-dir"])?;
        let common_raw = PathBuf::from(common_output.trim());
        let common_git_dir = fs::canonicalize(if common_raw.is_absolute() {
            common_raw
        } else {
            root.join(common_raw)
        })
        .map_err(|error| format!("cannot resolve common Git directory: {error}"))?;
        let key = sha256_hex(common_git_dir.to_string_lossy().as_bytes());
        let worktree_key = sha256_hex(root.to_string_lossy().as_bytes());
        Ok(Self {
            root,
            common_git_dir,
            paths: paths.clone(),
            data_dir: paths.data_dir.join("projects").join(&key),
            state_path: paths
                .state_dir
                .join("projects")
                .join(&key)
                .join("overlays.toml"),
            worktree_key,
        })
    }

    // A stable inode beside state serializes all Local read-modify-write operations.
    // Dropping the handle releases the OS lock, including on failure or process exit.
    fn lock_mutation(&self) -> Result<LocalMutationLock, String> {
        let path = self.state_path.with_file_name("overlays.lock");
        fs::create_dir_all(path.parent().unwrap())
            .map_err(|error| format!("cannot create Local lock directory: {error}"))?;
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| format!("cannot open Local lock {}: {error}", path.display()))?;
        file.lock()
            .map_err(|error| format!("cannot lock Local repository: {error}"))?;
        let lock = LocalMutationLock(file);
        #[cfg(test)]
        local_commit_boundary("locked")?;
        Ok(lock)
    }

    fn validate_owned_destination(
        &self,
        owned: &Owned,
        worktree: &Path,
        output: &Path,
    ) -> Result<(), String> {
        let relative = output
            .strip_prefix(worktree)
            .map_err(|_| "invalid Local ownership destination outside worktree".to_owned())?;
        if Path::new(&owned.path) != output
            || owned.exclusion.as_ref().is_some_and(|exclusion| {
                Path::new(&exclusion.path) != self.common_git_dir.join("info/exclude")
                    || exclusion.pattern != format!("/{}", relative.to_string_lossy())
            })
        {
            return Err("invalid Local ownership destination; inspect overlays.toml and recover manually; stored paths will not be rewritten".into());
        }
        Ok(())
    }

    fn validate_disable_ownership(&self, disable: &DisableOwned) -> Result<(), String> {
        let source = Path::new(disable.source());
        if !source.is_absolute()
            || source
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            return Err("invalid Local ownership source scope".into());
        }
        let parent = source.parent().ok_or("invalid Local ownership source")?;
        let filename = source
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let (worktree, output) = match disable {
            DisableOwned::Pi { .. } => {
                if !source.starts_with(&self.root)
                    || filename == "AGENTS.override.md"
                    || !PI_INSTRUCTION_CANDIDATES.contains(&filename)
                {
                    return Err("invalid Pi ownership source scope or filename".into());
                }
                (self.root.clone(), parent.join("AGENTS.override.md"))
            }
            DisableOwned::Claude { recovery, .. } => {
                if !matches!(filename, "CLAUDE.md" | "CLAUDE.local.md")
                    || source
                        .ancestors()
                        .any(|ancestor| ancestor.ends_with(".claude/rules"))
                {
                    return Err("invalid Claude ownership source scope or filename".into());
                }
                if let ClaudeRecovery::Existing(original) = recovery
                    && claude_settings::contains_exclusion(
                        original,
                        Path::new(&disable.owned().path),
                        disable.source(),
                    )?
                {
                    return Err(
                        "invalid Claude ownership recovery: source was already excluded".into(),
                    );
                }
                let root = crate::git_worktree::main_worktree(&self.root)?;
                let output = root.join(".claude/settings.local.json");
                // Creation validates the source's repository. Replay its exact logical
                // source and recovery bytes to check that binding after a source directory
                // or linked worktree disappears; recovery never accesses the source.
                let original = match recovery {
                    ClaudeRecovery::Created => "{}\n",
                    ClaudeRecovery::Existing(original) => original,
                };
                let rendered =
                    claude_settings::insert_exclusion(original, &output, disable.source())?;
                if sha256_hex(rendered.as_bytes()) != disable.owned().hash {
                    return Err("invalid Claude ownership source or recovery; inspect overlays.toml and recover manually".into());
                }
                (root, output)
            }
        };
        self.validate_owned_destination(disable.owned(), &worktree, &output)
    }

    pub(crate) fn disable_plan(
        &self,
        runtime: DisableRuntime,
        source: &Path,
    ) -> Result<DisablePlan, String> {
        let source = self.validate_disable_source(runtime, source)?;
        match self.disable_status(runtime)? {
            DisableStatus::Owned => {
                return Err(format!(
                    "{} already has an mdmanager.ai-owned local disable",
                    runtime.key()
                ));
            }
            DisableStatus::Modified => {
                return Err(format!(
                    "{} local disable changed after mdmanager.ai wrote it; resolve the modified file first",
                    runtime.key()
                ));
            }
            DisableStatus::Missing => {
                return Err(format!(
                    "{} local disable file is missing; resolve the ownership record first",
                    runtime.key()
                ));
            }
            DisableStatus::None => {}
        }
        match runtime {
            DisableRuntime::Pi => self.pi_disable_plan(source),
            DisableRuntime::Claude => self.claude_disable_plan(source),
        }
    }

    pub(crate) fn disabled_source(
        &self,
        runtime: DisableRuntime,
    ) -> Result<Option<PathBuf>, String> {
        let state = self.load_state()?;
        let owned = state.disables.get(&self.disable_state_key(runtime));
        if let Some(owned) = owned {
            self.validate_disable_ownership(owned)?;
        }
        Ok(owned.map(|owned| PathBuf::from(owned.source())))
    }

    fn validate_disable_source(
        &self,
        runtime: DisableRuntime,
        source: &Path,
    ) -> Result<PathBuf, String> {
        let source = std::path::absolute(source)
            .map_err(|error| format!("cannot resolve source {}: {error}", source.display()))?;
        let source = if runtime == DisableRuntime::Claude {
            logical_launch_path(&source)
        } else {
            source
        };
        let metadata = fs::symlink_metadata(&source)
            .map_err(|error| format!("cannot inspect source {}: {error}", source.display()))?;
        if metadata.file_type().is_symlink() {
            let target = fs::read_link(&source)
                .map(|target| format!(" to {}", target.display()))
                .unwrap_or_default();
            return Err(format!(
                "cannot disable symlinked source {}{target}; {} symlink exclusion behavior is unverified",
                source.display(),
                runtime.key()
            ));
        }
        if !metadata.is_file() {
            return Err(format!(
                "disable source is not a file: {}",
                source.display()
            ));
        }
        let parent = source
            .parent()
            .ok_or_else(|| "disable source has no parent directory".to_owned())?;
        let resolved_parent = fs::canonicalize(parent)
            .map_err(|error| format!("cannot resolve {}: {error}", parent.display()))?;
        let resolved_root = fs::canonicalize(&self.root)
            .map_err(|error| format!("cannot resolve {}: {error}", self.root.display()))?;
        if !resolved_parent.starts_with(&resolved_root) {
            return Err("local disable source must be inside the Git worktree".into());
        }

        let filename = source
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| "disable source filename is not UTF-8".to_owned())?;
        match runtime {
            DisableRuntime::Claude => {
                let is_rule = source
                    .ancestors()
                    .any(|ancestor| ancestor.ends_with(".claude/rules"));
                if is_rule || !matches!(filename, "CLAUDE.md" | "CLAUDE.local.md") {
                    return Err(format!(
                        "{} is not a Claude instruction file mdmanager.ai can disable",
                        source.display()
                    ));
                }
            }
            DisableRuntime::Pi => {
                if filename == "AGENTS.override.md" {
                    return Err("Pi's selected override cannot disable itself".into());
                }
                if !PI_INSTRUCTION_CANDIDATES.contains(&filename) {
                    return Err(format!(
                        "{} is not a Pi instruction candidate",
                        source.display()
                    ));
                }
                let selected = PI_INSTRUCTION_CANDIDATES
                    .iter()
                    .map(|name| parent.join(name))
                    .find(|candidate| candidate.is_file());
                if selected.as_deref() != Some(source.as_path()) {
                    let selected = selected
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|| "no candidate".into());
                    return Err(partial_recovery(format!(
                        "{} is not selected by Pi; {selected} takes priority",
                        source.display()
                    )));
                }
            }
        }
        Ok(source)
    }

    /// Revalidates the reviewed output and Git exclusion before either is changed.
    pub(crate) fn apply_disable(&self, plan: &DisablePlan) -> Result<(), String> {
        let _lock = self.lock_mutation()?;
        let current = self.disable_plan(plan.runtime, &plan.source)?;
        if current.source != plan.source
            || current.output != plan.output
            || current.text != plan.text
            || current.reviewed_output != plan.reviewed_output
            || current.exclusion != plan.exclusion
        {
            return Err("local disable changed after review; inspect and confirm again".into());
        }
        let mut state = self.load_state()?;
        let state_key = self.disable_state_key(plan.runtime);
        if state.disables.contains_key(&state_key) {
            return Err(format!(
                "{} already has an mdmanager.ai-owned local disable",
                plan.runtime.key()
            ));
        }
        match plan.runtime {
            DisableRuntime::Pi => {
                if plan.output.exists() {
                    return Err(format!("refusing to overwrite {}", plan.output.display()));
                }
                ensure_untracked(&self.root, &plan.output)?;
                if let Some(exclusion) = &plan.exclusion {
                    apply_exclusion(exclusion, &self.root, &plan.output)?;
                }
                #[cfg(test)]
                local_commit_boundary("disable-output").map_err(partial_recovery)?;
                atomic_create(&plan.output, b"").map_err(partial_recovery)?;
                state.disables.insert(
                    state_key.clone(),
                    DisableOwned::Pi {
                        owned: Owned {
                            path: plan.output.display().to_string(),
                            hash: sha256_hex(b""),
                            exclusion: plan.exclusion.as_ref().and_then(Exclusion::owned),
                        },
                        source: plan.source.display().to_string(),
                    },
                );
            }
            DisableRuntime::Claude => {
                let settings_worktree = crate::git_worktree::main_worktree(&self.root)?;
                ensure_untracked(&settings_worktree, &plan.output)?;
                if let Some(exclusion) = &plan.exclusion {
                    apply_exclusion(exclusion, &settings_worktree, &plan.output)?;
                }
                let old = plan
                    .reviewed_output
                    .clone()
                    .unwrap_or_else(|| "{}\n".into());
                let created = plan.reviewed_output.is_none();
                let source = plan.source.display().to_string();
                let rendered = claude_settings::insert_exclusion(&old, &plan.output, &source)?;
                #[cfg(test)]
                local_commit_boundary("disable-output").map_err(partial_recovery)?;
                atomic_write(&plan.output, rendered.as_bytes()).map_err(partial_recovery)?;
                state.disables.insert(
                    state_key,
                    DisableOwned::Claude {
                        owned: Owned {
                            path: plan.output.display().to_string(),
                            hash: sha256_hex(rendered.as_bytes()),
                            exclusion: plan.exclusion.as_ref().and_then(Exclusion::owned),
                        },
                        source,
                        recovery: if created {
                            ClaudeRecovery::Created
                        } else {
                            ClaudeRecovery::Existing(old)
                        },
                    },
                );
            }
        }
        self.save_state(&state).map_err(partial_recovery)
    }

    /// Restores only owned disable data, preserving unrelated Claude settings edits.
    pub(crate) fn restore_disable(&self, runtime: DisableRuntime) -> Result<(), String> {
        let _lock = self.lock_mutation()?;
        let mut state = self.load_state()?;
        let state_key = self.disable_state_key(runtime);
        let disable = state
            .disables
            .get(&state_key)
            .ok_or_else(|| format!("no mdmanager.ai-owned {} disable", runtime.key()))?;
        self.validate_disable_ownership(disable)?;
        let owned = disable.owned();
        let path = PathBuf::from(&owned.path);
        validate_local_target(&path)?;
        let bytes = fs::read(&path).map_err(|error| {
            partial_recovery(format!(
                "cannot read owned overlay {}: {error}",
                path.display()
            ))
        })?;
        let unchanged = sha256_hex(&bytes) == owned.hash;
        let replacement = match disable {
            DisableOwned::Pi { .. } => {
                if !unchanged {
                    return Err(format!(
                        "{} changed after mdmanager.ai wrote it; refusing restore",
                        path.display()
                    ));
                }
                None
            }
            DisableOwned::Claude {
                source, recovery, ..
            } => {
                if unchanged {
                    match recovery {
                        ClaudeRecovery::Created => None,
                        ClaudeRecovery::Existing(original) => Some(original.clone()),
                    }
                } else {
                    let settings = String::from_utf8(bytes)
                        .map_err(|_| format!("{} is not UTF-8", path.display()))?;
                    Some(
                        claude_settings::remove_exclusion(&settings, source)
                            .map_err(partial_recovery)?,
                    )
                }
            }
        };
        let exclusion_edit = owned_exclusion_removal(owned)?;
        if let Some(replacement) = replacement {
            atomic_write(&path, replacement.as_bytes())?;
        } else {
            fs::remove_file(&path)
                .map_err(|error| format!("cannot remove {}: {error}", path.display()))?;
        }
        #[cfg(test)]
        local_commit_boundary("restore-exclusion").map_err(partial_recovery)?;
        if let Some((path, content)) = exclusion_edit {
            atomic_write(&path, content.as_bytes()).map_err(partial_recovery)?;
        }
        state.disables.remove(&state_key);
        self.save_state(&state).map_err(partial_recovery)
    }

    pub(crate) fn disable_status(&self, runtime: DisableRuntime) -> Result<DisableStatus, String> {
        let state = self.load_state()?;
        let Some(owned) = state.disables.get(&self.disable_state_key(runtime)) else {
            return Ok(DisableStatus::None);
        };
        self.validate_disable_ownership(owned)?;
        validate_local_target(Path::new(&owned.owned().path))?;
        let disabled_source = owned.source();
        let owned = owned.owned();
        match fs::read(&owned.path) {
            Ok(bytes) if runtime == DisableRuntime::Claude => {
                let active = String::from_utf8(bytes).ok().is_some_and(|source| {
                    claude_settings::contains_exclusion(
                        &source,
                        Path::new(&owned.path),
                        disabled_source,
                    )
                    .unwrap_or(false)
                });
                Ok(if active {
                    DisableStatus::Owned
                } else {
                    DisableStatus::Modified
                })
            }
            Ok(bytes) if sha256_hex(&bytes) == owned.hash => Ok(DisableStatus::Owned),
            Ok(_) => Ok(DisableStatus::Modified),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(DisableStatus::Missing)
            }
            Err(error) => Err(format!("cannot read owned overlay {}: {error}", owned.path)),
        }
    }

    pub(crate) fn create_managed(
        &self,
        target: ManagedTarget,
        section: &str,
    ) -> Result<(), String> {
        let _lock = self.lock_mutation()?;
        let output = self.root.join(target.filename());
        validate_local_target(&output)?;
        if output.exists() {
            return Err(format!("refusing to overwrite {}", output.display()));
        }
        self.personal_library_section(section)?;
        self.create_local_target(target, section)
    }

    pub(crate) fn adopt_managed(&self, target: ManagedTarget, section: &str) -> Result<(), String> {
        let _lock = self.lock_mutation()?;
        let output = self.root.join(target.filename());
        validate_local_target(&output)?;
        if !output.is_file() {
            return Err(format!("{} does not exist", output.display()));
        }
        self.create_manifest_from_override(target, section, &output)
    }

    fn create_manifest_from_override(
        &self,
        target: ManagedTarget,
        section: &str,
        output: &Path,
    ) -> Result<(), String> {
        let state = self.load_state()?;
        if let Some(owned) = state.managed.get(&self.managed_state_key(target)) {
            self.validate_owned_destination(owned, &self.root, output)?;
        }
        let content = fs::read_to_string(output)
            .map_err(|error| format!("cannot read {}: {error}", output.display()))?;
        if content.is_empty() {
            return Err("empty local instructions cannot be adopted".into());
        }
        let library = GlobalConfig::load(&self.paths)?;
        let rendered = library.render_personal_sections(&[section.into()])?;
        if rendered != content {
            return Err(format!(
                "personal Section {section} does not render byte-for-byte as {}",
                output.display()
            ));
        }
        ensure_untracked(&self.root, output)?;
        let exclusion = self.exclusion_for(&self.root, output)?;
        if exclusion.needs_write {
            return Err(format!(
                "existing override is not ignored; add {} to {} before adoption",
                exclusion.pattern,
                exclusion.path.display()
            ));
        }
        self.create_local_target(target, section)?;
        let mut state = self.load_state()?;
        state.managed.insert(
            self.managed_state_key(target),
            Owned {
                path: output.display().to_string(),
                hash: sha256_hex(content.as_bytes()),
                exclusion: None,
            },
        );
        self.save_state(&state).map_err(partial_recovery)
    }

    pub(crate) fn managed_exists(&self, target: ManagedTarget) -> bool {
        self.load_local_manifest()
            .is_ok_and(|manifest| manifest.targets.contains_key(target.id()))
    }

    pub(crate) fn local_manifest_error(&self) -> Option<String> {
        self.local_manifest_path()
            .exists()
            .then(|| self.load_local_manifest().err())
            .flatten()
    }

    pub(crate) fn personal_section_targets(&self, id: &str) -> Vec<&'static str> {
        let Ok(manifest) = self.load_local_manifest() else {
            return Vec::new();
        };
        [ManagedTarget::Agents, ManagedTarget::Claude]
            .into_iter()
            .filter(|target| {
                manifest
                    .targets
                    .get(target.id())
                    .is_some_and(|target| target.sections.iter().any(|section| section == id))
            })
            .map(ManagedTarget::filename)
            .collect()
    }

    pub(crate) fn managed_section_path_for(
        &self,
        target: ManagedTarget,
        id: &str,
    ) -> Result<PathBuf, String> {
        let target = self.load_local_target(target)?;
        if !target.sections.iter().any(|candidate| candidate == id) {
            return Err(format!("unknown Local Section {id}"));
        }
        GlobalConfig::load(&self.paths)?
            .section_path(id)
            .ok_or_else(|| format!("unknown personal Section {id}"))
    }

    pub(crate) fn render_managed(&self, target: ManagedTarget) -> Result<String, String> {
        let target = self.load_local_target(target)?;
        GlobalConfig::load(&self.paths)?.render_personal_sections(&target.sections)
    }

    pub(crate) fn inspect_managed(&self, target: ManagedTarget) -> Result<ManagedView, String> {
        let local_target = self.load_local_target(target)?;
        let rendered = self.render_managed(target)?;
        let path = self.root.join(target.filename());
        validate_local_target(&path)?;
        let state = self.load_state()?;
        let managed_key = self.managed_state_key(target);
        let owned = state.managed.get(&managed_key);
        if let Some(owned) = owned {
            self.validate_owned_destination(owned, &self.root, &path)?;
        }
        let deployed = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
        };
        let status = if !path.exists() {
            LocalTargetStatus::Missing
        } else if let Some(owned) = owned {
            let deployed_hash = sha256_hex(deployed.as_bytes());
            if deployed == rendered {
                LocalTargetStatus::Current
            } else if deployed_hash == owned.hash {
                LocalTargetStatus::Stale
            } else {
                LocalTargetStatus::Modified
            }
        } else {
            LocalTargetStatus::External
        };
        let diff = if status == LocalTargetStatus::Current {
            "No changes.".into()
        } else {
            unified_diff(
                &deployed,
                &rendered,
                &path.display().to_string(),
                "rendered local instructions",
            )
        };
        let difference = (deployed != rendered).then(|| diff.clone());
        let library = GlobalConfig::load(&self.paths)?;
        let sections = local_target
            .sections
            .iter()
            .map(|id| {
                library
                    .section(id)
                    .map(|section| LocalSection {
                        id: section.id.clone(),
                        name: section.name.clone(),
                    })
                    .ok_or_else(|| format!("unknown personal Section {id}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ManagedView {
            path,
            rendered,
            status,
            diff,
            difference,
            sections,
            ordered: local_target.sections,
        })
    }

    pub(crate) fn managed_plan(&self, target: ManagedTarget) -> Result<ManagedPlan, String> {
        let view = self.inspect_managed(target)?;
        if !matches!(
            view.status,
            LocalTargetStatus::External | LocalTargetStatus::Modified
        ) {
            ensure_untracked(&self.root, &view.path)?;
        }
        let exclusion = self.exclusion_for(&self.root, &view.path)?;
        Ok(ManagedPlan {
            target,
            view,
            exclusion,
        })
    }

    pub(crate) fn apply_managed(&self, plan: &ManagedPlan) -> Result<LocalTargetStatus, String> {
        let _lock = self.lock_mutation()?;
        let current = self.managed_plan(plan.target)?;
        if current != *plan {
            return Err(
                "managed local instructions changed after review; inspect and confirm again".into(),
            );
        }
        let ManagedPlan {
            target,
            view,
            exclusion,
        } = current;
        if matches!(
            view.status,
            LocalTargetStatus::External | LocalTargetStatus::Modified
        ) {
            return Err(partial_recovery(format!(
                "{} is {}; refusing to overwrite it",
                view.path.display(),
                view.status.label()
            )));
        }
        let mut state = self.load_state()?;
        if exclusion.needs_write {
            apply_exclusion(&exclusion, &self.root, &view.path)?;
        }
        #[cfg(test)]
        local_commit_boundary("managed-output").map_err(partial_recovery)?;
        if view.status != LocalTargetStatus::Current {
            atomic_write(&view.path, view.rendered.as_bytes()).map_err(partial_recovery)?;
        }
        state.managed.insert(
            self.managed_state_key(target),
            Owned {
                path: view.path.display().to_string(),
                hash: sha256_hex(view.rendered.as_bytes()),
                exclusion: exclusion.owned().or_else(|| {
                    state
                        .managed
                        .get(&self.managed_state_key(target))
                        .and_then(|owned| owned.exclusion.clone())
                }),
            },
        );
        self.save_state(&state).map_err(partial_recovery)?;
        Ok(view.status)
    }

    fn pi_disable_plan(&self, source: PathBuf) -> Result<DisablePlan, String> {
        let codex = match codex_version() {
            Ok(version) if VERIFIED_CODEX_EMPTY_OVERRIDE_VERSIONS.contains(&version.as_str()) => {
                format!(
                    "Codex compatibility\n  The installed Codex skips the empty override, so {} still loads for Codex.",
                    source.display()
                )
            }
            Ok(_) => format!(
                "Codex compatibility warning\n  The installed Codex version has not been verified to skip empty overrides. This override may also suppress {} for Codex in this directory.",
                source.display()
            ),
            Err(_) => format!(
                "Codex compatibility warning\n  Codex was not detected. If Codex is used later or elsewhere, this override may also suppress {} for Codex in this directory.",
                source.display()
            ),
        };
        let directory = source
            .parent()
            .ok_or_else(|| "source has no parent directory".to_owned())?;
        let output = directory.join("AGENTS.override.md");
        validate_local_target(&output)?;
        if output.exists() {
            return Err(partial_recovery(format!(
                "refusing to overwrite {}",
                output.display()
            )));
        }
        ensure_untracked(&self.root, &output)?;
        let exclusion = self.exclusion_for(&self.root, &output)?;
        let exclusion_text = exclusion.needs_write.then(|| {
            format!(
                "\n\nGit exclusion\n  {} in {}",
                exclusion.pattern,
                exclusion.path.display()
            )
        });
        Ok(DisablePlan {
            runtime: DisableRuntime::Pi,
            source: source.clone(),
            output: output.clone(),
            text: format!(
                "Shared file\n  {} will not be modified.\n\nCreate\n  {}\n  zero bytes{}\n\nPi\n  The override shadows this directory's regular candidate.\n\n{codex}\n\nA new Pi session is required.",
                source.display(),
                output.display(),
                exclusion_text.as_deref().unwrap_or_default(),
            ),
            reviewed_output: None,
            exclusion: Some(exclusion),
        })
    }

    fn claude_disable_plan(&self, source: PathBuf) -> Result<DisablePlan, String> {
        let settings_worktree = crate::git_worktree::main_worktree(&self.root)?;
        let output = settings_worktree.join(".claude/settings.local.json");
        validate_local_target(&output)?;
        ensure_untracked(&settings_worktree, &output)?;
        let exclusion = self.exclusion_for(&settings_worktree, &output)?;
        let reviewed_output = match fs::read_to_string(&output) {
            Ok(source) => Some(source),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(format!("cannot read {}: {error}", output.display())),
        };
        if let Some(source_text) = &reviewed_output
            && claude_settings::contains_exclusion(
                source_text,
                &output,
                source.to_string_lossy().as_ref(),
            )?
        {
            return Err(partial_recovery(
                "the selected Claude source is already excluded".into(),
            ));
        }
        let exclusion_text = exclusion.needs_write.then(|| {
            format!(
                "\n\nGit exclusion\n  {} in {}",
                exclusion.pattern,
                exclusion.path.display()
            )
        });
        Ok(DisablePlan {
            runtime: DisableRuntime::Claude,
            source: source.clone(),
            output: output.clone(),
            text: format!(
                "Shared file\n  {} will not be modified.\n\nEdit\n  {}\n  add {} to claudeMdExcludes{}\n\nThe setting is machine-local and shared by this repository's worktrees.\nA new Claude session is required.",
                source.display(),
                output.display(),
                source.display(),
                exclusion_text.as_deref().unwrap_or_default(),
            ),
            reviewed_output,
            exclusion: Some(exclusion),
        })
    }

    fn exclusion_for(&self, worktree: &Path, output: &Path) -> Result<Exclusion, String> {
        let relative = output
            .strip_prefix(worktree)
            .map_err(|_| format!("{} is outside the worktree", output.display()))?;
        let pattern = format!("/{}", relative.to_string_lossy());
        let ignored = git_status(
            worktree,
            &["check-ignore", "-q", "--", &relative.to_string_lossy()],
        )?;
        Ok(Exclusion {
            path: self.common_git_dir.join("info/exclude"),
            pattern,
            needs_write: !ignored,
        })
    }

    fn local_manifest_path(&self) -> PathBuf {
        self.data_dir.join("local.toml")
    }

    fn personal_library_section(&self, id: &str) -> Result<(), String> {
        if GlobalConfig::load(&self.paths)?.section(id).is_none() {
            return Err(format!("unknown personal Section {id}"));
        }
        Ok(())
    }

    fn disable_state_key(&self, runtime: DisableRuntime) -> String {
        match runtime {
            DisableRuntime::Claude => "claude".into(),
            DisableRuntime::Pi => format!("pi:{}", self.worktree_key),
        }
    }

    fn managed_state_key(&self, target: ManagedTarget) -> String {
        format!("{}:{}", target.id(), self.worktree_key)
    }

    fn load_local_manifest(&self) -> Result<LocalManifest, String> {
        let path = self.local_manifest_path();
        let source = fs::read_to_string(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let manifest: LocalManifest = toml::from_str(&source)
            .map_err(|error| format!("invalid {}: {error}", path.display()))?;
        if manifest.format != 1 {
            return Err(format!(
                "unsupported local instruction format {}",
                manifest.format
            ));
        }
        for id in manifest.targets.keys() {
            ManagedTarget::parse(id)?;
        }
        for (target_id, target) in &manifest.targets {
            if target.sections.is_empty() {
                return Err(format!(
                    "Local Instructions target {target_id} must contain at least one personal Section"
                ));
            }
            let mut seen = std::collections::HashSet::new();
            for section in &target.sections {
                if !seen.insert(section) {
                    return Err(format!(
                        "Local Instructions target {target_id} repeats personal Section {section}"
                    ));
                }
            }
        }
        Ok(manifest)
    }

    fn load_local_target(&self, target: ManagedTarget) -> Result<LocalTarget, String> {
        self.load_local_manifest()?
            .targets
            .get(target.id())
            .cloned()
            .ok_or_else(|| format!("managed {} instructions do not exist", target.id()))
    }

    fn create_local_target(&self, target: ManagedTarget, section: &str) -> Result<(), String> {
        let state = self.load_state()?;
        if let Some(owned) = state.managed.get(&self.managed_state_key(target)) {
            self.validate_owned_destination(owned, &self.root, &self.root.join(target.filename()))?;
        }
        let path = self.local_manifest_path();
        let exists = path.is_file();
        let mut manifest = if exists {
            self.load_local_manifest()?
        } else {
            LocalManifest {
                format: 1,
                targets: std::collections::BTreeMap::new(),
            }
        };
        if manifest.targets.contains_key(target.id()) {
            return Err(format!(
                "managed {} instructions already exist",
                target.id()
            ));
        }
        manifest.targets.insert(
            target.id().into(),
            LocalTarget {
                sections: vec![section.into()],
            },
        );
        let source = toml::to_string_pretty(&manifest)
            .map_err(|error| format!("cannot serialize local instruction manifest: {error}"))?;
        if exists {
            atomic_write(&path, source.as_bytes())
        } else {
            atomic_create(&path, source.as_bytes())
        }
    }

    fn load_state(&self) -> Result<State, String> {
        match fs::read_to_string(&self.state_path) {
            Ok(source) => toml::from_str::<RawState>(&source)
                .map_err(|error| error.to_string())
                .and_then(State::try_from)
                .map_err(|error| format!("invalid {}: {error}", self.state_path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
            Err(error) => Err(format!(
                "cannot read {}: {error}",
                self.state_path.display()
            )),
        }
    }

    fn save_state(&self, state: &State) -> Result<(), String> {
        #[cfg(test)]
        local_commit_boundary("state")?;
        let source = toml::to_string_pretty(&RawState::from(state))
            .map_err(|error| format!("cannot serialize overlay state: {error}"))?;
        atomic_write(&self.state_path, source.as_bytes())
    }
}

fn partial_recovery(error: String) -> String {
    format!(
        "{error}; Local writes may be partial: inspect output, info/exclude and overlays.toml before retrying; see docs/configuration.md for manual recovery"
    )
}

#[cfg(test)]
type LocalCommitHook = Box<dyn FnMut(&str) -> Result<(), String>>;

#[cfg(test)]
thread_local! {
    static LOCAL_COMMIT_HOOK: std::cell::RefCell<Option<LocalCommitHook>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn local_commit_boundary(stage: &str) -> Result<(), String> {
    LOCAL_COMMIT_HOOK.with_borrow_mut(|hook| match hook {
        Some(hook) => hook(stage),
        None => Ok(()),
    })
}

fn codex_version() -> Result<String, String> {
    let output = Command::new("codex")
        .arg("--version")
        .output()
        .map_err(|error| format!("cannot detect Codex version: {error}"))?;
    if !output.status.success() {
        return Err("cannot detect Codex version".into());
    }
    let source = String::from_utf8(output.stdout)
        .map_err(|_| "Codex version output is not UTF-8".to_owned())?;
    source
        .split_whitespace()
        .find(|part| {
            part.chars()
                .next()
                .is_some_and(|character| character.is_ascii_digit())
        })
        .map(str::to_owned)
        .ok_or_else(|| "cannot parse Codex version".to_owned())
}

fn logical_launch_path(path: &Path) -> PathBuf {
    let Ok(current) = std::env::current_dir() else {
        return path.to_owned();
    };
    let Some(pwd) = std::env::var_os("PWD").map(PathBuf::from) else {
        return path.to_owned();
    };
    if !pwd.is_absolute()
        || fs::canonicalize(&pwd).ok() != fs::canonicalize(&current).ok()
        || !path.starts_with(&current)
    {
        return path.to_owned();
    }
    pwd.join(path.strip_prefix(current).unwrap())
}

fn ensure_untracked(root: &Path, path: &Path) -> Result<(), String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| format!("{} is outside the worktree", path.display()))?;
    if git_status(
        root,
        &[
            "ls-files",
            "--error-unmatch",
            "--",
            &relative.to_string_lossy(),
        ],
    )? {
        return Err(format!(
            "refusing to write tracked overlay {}",
            path.display()
        ));
    }
    Ok(())
}

fn apply_exclusion(exclusion: &Exclusion, worktree: &Path, output: &Path) -> Result<(), String> {
    if !exclusion.needs_write {
        return Ok(());
    }
    let relative = output
        .strip_prefix(worktree)
        .map_err(|_| format!("{} is outside the worktree", output.display()))?;
    let mut source = fs::read_to_string(&exclusion.path).unwrap_or_default();
    let original = source.clone();
    if !source.is_empty() && !source.ends_with('\n') {
        source.push('\n');
    }
    source.push_str(&exclusion.pattern);
    source.push('\n');
    atomic_write(&exclusion.path, source.as_bytes())?;
    let validation = git_status(
        worktree,
        &["check-ignore", "-q", "--", &relative.to_string_lossy()],
    )
    .and_then(|ignored| {
        ignored.then_some(()).ok_or_else(|| {
            format!(
                "Git exclusion {} does not ignore {}",
                exclusion.pattern,
                output.display()
            )
        })
    });
    if let Err(error) = validation {
        atomic_write(&exclusion.path, original.as_bytes())?;
        return Err(error);
    }
    Ok(())
}

fn owned_exclusion_removal(owned: &Owned) -> Result<Option<(PathBuf, String)>, String> {
    let Some(OwnedExclusion { path, pattern }) = &owned.exclusion else {
        return Ok(None);
    };
    let path = PathBuf::from(path);
    let source = fs::read_to_string(&path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let mut removed = false;
    let output = source
        .split_inclusive('\n')
        .filter(|line| {
            if !removed && line.trim_end_matches(['\r', '\n']) == pattern {
                removed = true;
                false
            } else {
                true
            }
        })
        .collect::<String>();
    Ok(Some((path, output)))
}

fn git_status(directory: &Path, arguments: &[&str]) -> Result<bool, String> {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(directory)
        .output()
        .map_err(|error| format!("cannot run git: {error}"))?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(format!(
            "git {} failed: {}",
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn repository() -> (TempDir, TempDir, LocalRepository) {
        let home = TempDir::new().unwrap();
        let repository = TempDir::new().unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .current_dir(repository.path())
                .status()
                .unwrap()
                .success()
        );
        let paths = Paths::for_home(home.path());
        let local_repository = LocalRepository::discover(repository.path(), &paths).unwrap();
        (home, repository, local_repository)
    }

    fn personal_section(paths: &Paths, content: &str) {
        fs::create_dir_all(paths.data_dir.join("sections")).unwrap();
        fs::write(
            &paths.config,
            "[[sections]]\nid = \"local\"\nname = \"Local instructions\"\npath = \"sections/local.md\"\n",
        )
        .unwrap();
        fs::write(paths.data_dir.join("sections/local.md"), content).unwrap();
    }

    #[test]
    fn disable_rejects_sources_the_runtime_does_not_select() {
        let (_home, repository, local_repository) = repository();
        fs::write(repository.path().join("AGENTS.md"), "agents\n").unwrap();
        fs::write(repository.path().join("CLAUDE.md"), "claude\n").unwrap();
        fs::create_dir(repository.path().join(".pi")).unwrap();
        fs::write(repository.path().join(".pi/SYSTEM.md"), "system\n").unwrap();

        let wrong_claude = local_repository
            .disable_plan(DisableRuntime::Claude, &repository.path().join("AGENTS.md"))
            .unwrap_err();
        assert!(wrong_claude.contains("not a Claude instruction file"));

        let shadowed_pi = local_repository
            .disable_plan(DisableRuntime::Pi, &repository.path().join("CLAUDE.md"))
            .unwrap_err();
        assert!(shadowed_pi.contains("AGENTS.md takes priority"));

        let system_prompt = local_repository
            .disable_plan(DisableRuntime::Pi, &repository.path().join(".pi/SYSTEM.md"))
            .unwrap_err();
        assert!(system_prompt.contains("not a Pi instruction candidate"));
    }

    #[cfg(unix)]
    #[test]
    fn disable_rejects_a_symlink_without_rewriting_its_identity() {
        use std::os::unix::fs::symlink;

        let (_home, repository, local_repository) = repository();
        fs::write(repository.path().join("AGENTS.md"), "shared\n").unwrap();
        symlink("AGENTS.md", repository.path().join("CLAUDE.md")).unwrap();

        let error = local_repository
            .disable_plan(DisableRuntime::Claude, &repository.path().join("CLAUDE.md"))
            .unwrap_err();

        assert!(error.contains("CLAUDE.md"));
        assert!(error.contains("symlinked source"));
        assert!(!local_repository.state_path.exists());
    }

    #[test]
    fn claude_disable_reuses_an_existing_git_exclusion() {
        let (_home, repository, local_repository) = repository();
        let source = repository.path().join("CLAUDE.md");
        fs::write(&source, "claude\n").unwrap();
        fs::create_dir(repository.path().join(".claude")).unwrap();
        fs::write(
            repository.path().join(".git/info/exclude"),
            "/.claude/settings.local.json\n",
        )
        .unwrap();

        let plan = local_repository
            .disable_plan(DisableRuntime::Claude, &source)
            .unwrap();

        assert_eq!(plan.source, source);
        assert!(plan.text.contains(&source.display().to_string()));
        assert!(!plan.exclusion.as_ref().unwrap().needs_write);
        local_repository.apply_disable(&plan).unwrap();
        assert_eq!(
            local_repository
                .disabled_source(DisableRuntime::Claude)
                .unwrap(),
            Some(source)
        );
    }

    #[test]
    fn claude_disable_uses_a_separate_git_dirs_worktree() {
        let home = TempDir::new().unwrap();
        let fixture = TempDir::new().unwrap();
        let worktree = fixture.path().join("work/repository");
        let git_dir = fixture.path().join("gitdirs/repository.git");
        fs::create_dir_all(worktree.parent().unwrap()).unwrap();
        fs::create_dir_all(git_dir.parent().unwrap()).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q", "--separate-git-dir"])
                .arg(&git_dir)
                .arg(&worktree)
                .status()
                .unwrap()
                .success()
        );
        let source = worktree.join("CLAUDE.md");
        fs::write(&source, "claude\n").unwrap();
        let local_repository =
            LocalRepository::discover(&worktree, &Paths::for_home(home.path())).unwrap();

        // Git metadata parents must not contribute Claude settings.
        let decoy = fixture.path().join("gitdirs/.claude");
        fs::create_dir_all(&decoy).unwrap();
        fs::write(
            decoy.join("settings.local.json"),
            serde_json::json!({"claudeMdExcludes": [source.display().to_string()]}).to_string(),
        )
        .unwrap();
        let audit = crate::context::Audit::resolve(
            crate::context::ContextRuntime::Claude,
            &worktree,
            &Paths::for_home(home.path()),
            None,
        );
        assert!(audit.sources.iter().any(|entry| entry.path == source
            && matches!(entry.state, crate::context::SourceState::Startup)));
        fs::remove_dir_all(&decoy).unwrap();

        let plan = local_repository
            .disable_plan(DisableRuntime::Claude, &source)
            .unwrap();
        assert_eq!(plan.output, worktree.join(".claude/settings.local.json"));
        local_repository.apply_disable(&plan).unwrap();
        let audit = crate::context::Audit::resolve(
            crate::context::ContextRuntime::Claude,
            &worktree,
            &Paths::for_home(home.path()),
            None,
        );
        assert!(audit.sources.iter().any(|entry| entry.path == source
            && matches!(entry.state, crate::context::SourceState::Excluded(_))));

        assert!(worktree.join(".claude/settings.local.json").is_file());
        assert!(!fixture.path().join("gitdirs/.claude").exists());
        assert!(
            fs::read_to_string(git_dir.join("info/exclude"))
                .unwrap()
                .lines()
                .any(|line| line == "/.claude/settings.local.json")
        );
    }

    #[test]
    fn claude_disable_uses_a_submodules_worktree() {
        let home = TempDir::new().unwrap();
        let fixture = TempDir::new().unwrap();
        let origin = fixture.path().join("origin");
        let superproject = fixture.path().join("superproject");
        fs::create_dir(&origin).unwrap();
        fs::create_dir(&superproject).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .current_dir(&origin)
                .status()
                .unwrap()
                .success()
        );
        for (key, value) in [("user.email", "test@example.com"), ("user.name", "Test")] {
            assert!(
                Command::new("git")
                    .args(["config", key, value])
                    .current_dir(&origin)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        fs::write(origin.join("CLAUDE.md"), "claude\n").unwrap();
        assert!(
            Command::new("git")
                .args(["add", "CLAUDE.md"])
                .current_dir(&origin)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .args(["commit", "-qm", "fixture"])
                .current_dir(&origin)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .current_dir(&superproject)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .args(["-c", "protocol.file.allow=always", "submodule", "add", "-q"])
                .arg(&origin)
                .arg("submodule")
                .current_dir(&superproject)
                .status()
                .unwrap()
                .success()
        );
        let worktree = superproject.join("submodule");
        let source = worktree.join("CLAUDE.md");
        let local_repository =
            LocalRepository::discover(&worktree, &Paths::for_home(home.path())).unwrap();

        // Git metadata parents must not contribute Claude settings.
        let decoy = superproject.join(".git/modules/.claude");
        fs::create_dir_all(&decoy).unwrap();
        fs::write(
            decoy.join("settings.local.json"),
            serde_json::json!({"claudeMdExcludes": [source.display().to_string()]}).to_string(),
        )
        .unwrap();
        let audit = crate::context::Audit::resolve(
            crate::context::ContextRuntime::Claude,
            &worktree,
            &Paths::for_home(home.path()),
            None,
        );
        assert!(audit.sources.iter().any(|entry| entry.path == source
            && matches!(entry.state, crate::context::SourceState::Startup)));
        fs::remove_dir_all(&decoy).unwrap();

        let plan = local_repository
            .disable_plan(DisableRuntime::Claude, &source)
            .unwrap();
        assert_eq!(plan.output, worktree.join(".claude/settings.local.json"));
        local_repository.apply_disable(&plan).unwrap();
        let audit = crate::context::Audit::resolve(
            crate::context::ContextRuntime::Claude,
            &worktree,
            &Paths::for_home(home.path()),
            None,
        );
        assert!(audit.sources.iter().any(|entry| entry.path == source
            && matches!(entry.state, crate::context::SourceState::Excluded(_))));

        assert!(worktree.join(".claude/settings.local.json").is_file());
        assert!(!superproject.join(".git/modules/.claude").exists());
        assert!(
            fs::read_to_string(superproject.join(".git/modules/submodule/info/exclude"))
                .unwrap()
                .lines()
                .any(|line| line == "/.claude/settings.local.json")
        );
    }

    #[test]
    fn exclusion_write_rejects_a_higher_priority_negation() {
        let (_home, repository, local_repository) = repository();
        let output = repository.path().join("AGENTS.override.md");
        let exclude = repository.path().join(".git/info/exclude");
        let original_exclude = fs::read_to_string(&exclude).unwrap();
        fs::write(
            repository.path().join(".gitignore"),
            "!/AGENTS.override.md\n",
        )
        .unwrap();

        let exclusion = local_repository
            .exclusion_for(repository.path(), &output)
            .unwrap();
        assert!(exclusion.needs_write);
        assert!(apply_exclusion(&exclusion, repository.path(), &output).is_err());
        assert_eq!(fs::read_to_string(exclude).unwrap(), original_exclude);
        assert!(!output.exists());
    }

    #[test]
    fn git_status_distinguishes_a_negative_result_from_a_git_failure() {
        let (_home, repository, _local_repository) = repository();
        let outside = TempDir::new().unwrap();

        assert_eq!(
            git_status(repository.path(), &["check-ignore", "-q", "--", "missing"]),
            Ok(false)
        );
        assert!(git_status(outside.path(), &["check-ignore", "-q", "--", "missing"]).is_err());
    }

    #[test]
    fn claude_disable_preserves_settings_text_and_restore_is_exact() {
        let (_home, repository, local_repository) = repository();
        let source = repository.path().join("CLAUDE.md");
        let settings = repository.path().join(".claude/settings.local.json");
        let original = "{\n    \"z-last\": true,\n    \"nested\": { \"keep\": 1 }\n}\n";
        fs::write(&source, "claude\n").unwrap();
        fs::create_dir(repository.path().join(".claude")).unwrap();
        fs::write(&settings, original).unwrap();
        fs::write(
            repository.path().join(".git/info/exclude"),
            "/.claude/settings.local.json\n",
        )
        .unwrap();

        let plan = local_repository
            .disable_plan(DisableRuntime::Claude, &source)
            .unwrap();
        local_repository.apply_disable(&plan).unwrap();
        let disabled = fs::read_to_string(&settings).unwrap();
        assert_eq!(
            disabled,
            format!(
                "{{\n    \"z-last\": true,\n    \"nested\": {{ \"keep\": 1 }},\n    \"claudeMdExcludes\": [{}]\n}}\n",
                serde_json::to_string(&source.display().to_string()).unwrap()
            )
        );

        local_repository
            .restore_disable(DisableRuntime::Claude)
            .unwrap();
        assert_eq!(fs::read_to_string(settings).unwrap(), original);
        assert_eq!(
            fs::read_to_string(repository.path().join(".git/info/exclude")).unwrap(),
            "/.claude/settings.local.json\n"
        );
    }

    #[test]
    fn claude_disable_rejects_settings_created_after_review() {
        let (_home, repository, local_repository) = repository();
        let source = repository.path().join("CLAUDE.md");
        let settings = repository.path().join(".claude/settings.local.json");
        fs::write(&source, "claude\n").unwrap();
        let plan = local_repository
            .disable_plan(DisableRuntime::Claude, &source)
            .unwrap();
        fs::create_dir(repository.path().join(".claude")).unwrap();
        fs::write(&settings, "{\"permissions\": {}}\n").unwrap();

        let error = local_repository.apply_disable(&plan).unwrap_err();

        assert!(error.contains("changed after review"));
        assert_eq!(
            fs::read_to_string(settings).unwrap(),
            "{\"permissions\": {}}\n"
        );
        assert!(
            !fs::read_to_string(repository.path().join(".git/info/exclude"))
                .unwrap()
                .lines()
                .any(|line| line == "/.claude/settings.local.json")
        );
    }

    #[test]
    fn disable_status_detects_modified_and_missing_owned_files() {
        let (_home, repository, local_repository) = repository();
        let source = repository.path().join("CLAUDE.md");
        let settings = repository.path().join(".claude/settings.local.json");
        fs::write(&source, "claude\n").unwrap();
        fs::create_dir(repository.path().join(".claude")).unwrap();
        fs::write(
            repository.path().join(".git/info/exclude"),
            "/.claude/settings.local.json\n",
        )
        .unwrap();
        let plan = local_repository
            .disable_plan(DisableRuntime::Claude, &source)
            .unwrap();
        local_repository.apply_disable(&plan).unwrap();
        assert_eq!(
            local_repository
                .disable_status(DisableRuntime::Claude)
                .unwrap(),
            DisableStatus::Owned
        );

        fs::write(&settings, "{}\n").unwrap();
        assert_eq!(
            local_repository
                .disable_status(DisableRuntime::Claude)
                .unwrap(),
            DisableStatus::Modified
        );
        fs::remove_file(&settings).unwrap();
        assert_eq!(
            local_repository
                .disable_status(DisableRuntime::Claude)
                .unwrap(),
            DisableStatus::Missing
        );
    }

    #[test]
    fn claude_disable_survives_unrelated_settings_changes() {
        let (_home, repository, local_repository) = repository();
        let source = repository.path().join("CLAUDE.md");
        let settings = repository.path().join(".claude/settings.local.json");
        fs::write(&source, "claude\n").unwrap();
        fs::create_dir(repository.path().join(".claude")).unwrap();
        fs::write(&settings, "{\n  \"permissions\": {}\n}\n").unwrap();
        fs::write(
            repository.path().join(".git/info/exclude"),
            "/.claude/settings.local.json\n",
        )
        .unwrap();

        let plan = local_repository
            .disable_plan(DisableRuntime::Claude, &source)
            .unwrap();
        local_repository.apply_disable(&plan).unwrap();
        let changed = fs::read_to_string(&settings).unwrap().replace(
            "\"permissions\": {}",
            "\"permissions\": {\"allow\": [\"Bash\"]}",
        );
        fs::write(&settings, changed).unwrap();

        assert_eq!(
            local_repository
                .disable_status(DisableRuntime::Claude)
                .unwrap(),
            DisableStatus::Owned
        );
        local_repository
            .restore_disable(DisableRuntime::Claude)
            .unwrap();
        let restored = fs::read_to_string(settings).unwrap();
        assert!(restored.contains("\"allow\": [\"Bash\"]"));
        assert!(!restored.contains(&source.display().to_string()));
    }

    #[test]
    fn claude_rules_are_audit_only() {
        let (_home, repository, local_repository) = repository();
        let rule = repository.path().join(".claude/rules/CLAUDE.md");
        fs::create_dir_all(rule.parent().unwrap()).unwrap();
        fs::write(&rule, "rule\n").unwrap();

        let error = local_repository
            .disable_plan(DisableRuntime::Claude, &rule)
            .unwrap_err();
        assert!(error.contains("not a Claude instruction file"));
    }

    #[test]
    #[cfg(unix)]
    fn managed_symlinks_refuse_adoption_and_reviewed_writes_without_effects() {
        use std::os::unix::fs::symlink;
        for target in [ManagedTarget::Agents, ManagedTarget::Claude] {
            for dangling in [false, true] {
                let (_home, repository, repo) = repository();
                personal_section(&repo.paths, "local\n");
                let output = repository.path().join(target.filename());
                let referent = repository.path().join("referent");
                if !dangling {
                    fs::write(&referent, "local\n").unwrap();
                }
                symlink(&referent, &output).unwrap();
                let exclusion = repository.path().join(".git/info/exclude");
                let before_exclusion = fs::read(&exclusion).unwrap();
                assert!(repo.adopt_managed(target, "local").is_err());
                assert!(repo.create_managed(target, "local").is_err());
                assert!(!repo.local_manifest_path().exists());
                assert!(!repo.state_path.exists());
                fs::remove_file(&output).unwrap();
                repo.create_managed(target, "local").unwrap();
                let plan = repo.managed_plan(target).unwrap();
                repo.apply_managed(&plan).unwrap();
                let plan = repo.managed_plan(target).unwrap();
                let manifest = fs::read(repo.local_manifest_path()).unwrap();
                let state = fs::read(&repo.state_path).unwrap();
                let owned_exclusion = fs::read(&exclusion).unwrap();
                assert_ne!(owned_exclusion, before_exclusion);
                fs::remove_file(&output).unwrap();
                symlink(&referent, &output).unwrap();
                assert!(repo.inspect_managed(target).is_err());
                assert!(repo.managed_plan(target).is_err());
                assert!(repo.apply_managed(&plan).is_err());
                assert_eq!(fs::read_link(&output).unwrap(), referent);
                if dangling {
                    assert!(!referent.exists());
                } else {
                    assert_eq!(fs::read(&referent).unwrap(), b"local\n");
                }
                assert_eq!(fs::read(repo.local_manifest_path()).unwrap(), manifest);
                assert_eq!(fs::read(&repo.state_path).unwrap(), state);
                assert_eq!(fs::read(exclusion).unwrap(), owned_exclusion);
            }
        }
    }

    fn fail_local_commit(stage: &'static str) {
        LOCAL_COMMIT_HOOK.set(Some(Box::new(move |current| {
            if current == stage {
                Err(format!("injected {stage} failure"))
            } else {
                Ok(())
            }
        })));
    }

    fn clear_local_failure_and_check_unlock(repo: &LocalRepository) {
        LOCAL_COMMIT_HOOK.set(None);
        let handle = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(repo.state_path.with_file_name("overlays.lock"))
            .unwrap();
        handle.try_lock().unwrap();
    }

    #[test]
    fn disable_partial_commits_preserve_bytes_and_refuse_unowned_retry() {
        for runtime in [DisableRuntime::Pi, DisableRuntime::Claude] {
            for stage in ["disable-output", "state"] {
                let (_home, directory, repo) = repository();
                personal_section(&repo.paths, "local\n");
                repo.create_managed(ManagedTarget::Claude, "local").unwrap();
                repo.apply_managed(&repo.managed_plan(ManagedTarget::Claude).unwrap())
                    .unwrap();
                let state = fs::read(&repo.state_path).unwrap();
                let source = directory.path().join(if runtime == DisableRuntime::Pi {
                    "AGENTS.md"
                } else {
                    "CLAUDE.md"
                });
                fs::write(&source, "shared\n").unwrap();
                let settings = directory.path().join(".claude/settings.local.json");
                let original = "{\n  \"keep\": { \"user\": true },\n  \"claudeMdExcludes\": [\"/user.md\"]\n}\n";
                if runtime == DisableRuntime::Claude {
                    fs::create_dir_all(settings.parent().unwrap()).unwrap();
                    fs::write(&settings, original).unwrap();
                }
                let plan = repo.disable_plan(runtime, &source).unwrap();
                let before = fs::read(&plan.output).ok();
                let exclusion = plan.exclusion.as_ref().unwrap();
                let old_exclusion = fs::read_to_string(&exclusion.path).unwrap();
                fail_local_commit(stage);
                assert!(repo.apply_disable(&plan).unwrap_err().contains("partial"));
                clear_local_failure_and_check_unlock(&repo);
                let fresh = LocalRepository::discover(directory.path(), &repo.paths).unwrap();
                assert_eq!(fresh.disable_status(runtime).unwrap(), DisableStatus::None);
                assert_eq!(fs::read(&repo.state_path).unwrap(), state);
                assert_eq!(
                    fs::read_to_string(&exclusion.path).unwrap(),
                    format!("{old_exclusion}{}\n", exclusion.pattern)
                );
                assert_eq!(fs::read(&source).unwrap(), b"shared\n");
                assert_eq!(
                    fs::read(directory.path().join("CLAUDE.local.md")).unwrap(),
                    b"local\n"
                );
                if stage == "disable-output" {
                    assert_eq!(fs::read(&plan.output).ok(), before);
                    fresh
                        .apply_disable(&fresh.disable_plan(runtime, &source).unwrap())
                        .unwrap();
                    fresh.restore_disable(runtime).unwrap();
                    assert_eq!(fs::read(&plan.output).ok(), before);
                } else {
                    let written = fs::read_to_string(&plan.output).unwrap();
                    if runtime == DisableRuntime::Claude {
                        assert_eq!(
                            written,
                            claude_settings::insert_exclusion(
                                original,
                                &settings,
                                source.to_str().unwrap()
                            )
                            .unwrap()
                        );
                    } else {
                        assert!(written.is_empty());
                    }
                    assert!(
                        fresh
                            .disable_plan(runtime, &source)
                            .unwrap_err()
                            .contains("partial")
                    );
                    // A user edit between attempts must never be mistaken for recoverable ownership.
                    fs::write(&plan.output, format!("{written} ")).unwrap();
                    assert!(fresh.disable_plan(runtime, &source).is_err());
                    assert_eq!(
                        fs::read_to_string(&plan.output).unwrap(),
                        format!("{written} ")
                    );
                }
                assert_eq!(fresh.load_state().unwrap().managed.len(), 1);
            }
        }
    }

    #[test]
    fn restore_partial_commits_keep_recovery_and_user_edits() {
        for mode in 0..4 {
            for stage in ["restore-exclusion", "state"] {
                let (_home, directory, repo) = repository();
                let runtime = if mode == 0 {
                    DisableRuntime::Pi
                } else {
                    DisableRuntime::Claude
                };
                let source =
                    directory
                        .path()
                        .join(if mode == 0 { "AGENTS.md" } else { "CLAUDE.md" });
                fs::write(&source, "shared\n").unwrap();
                let settings = directory.path().join(".claude/settings.local.json");
                let original = "{\n  \"keep\": 1\n}\n";
                if mode >= 2 {
                    fs::create_dir_all(settings.parent().unwrap()).unwrap();
                    fs::write(&settings, original).unwrap();
                }
                let exclusion_path = repo.common_git_dir.join("info/exclude");
                let user_exclusion = "/user-rule\r\n# untouched\n";
                fs::write(&exclusion_path, user_exclusion).unwrap();
                // Keep an unrelated ownership entry through every failed commit and retry.
                personal_section(&repo.paths, "local\n");
                repo.create_managed(ManagedTarget::Claude, "local").unwrap();
                repo.apply_managed(&repo.managed_plan(ManagedTarget::Claude).unwrap())
                    .unwrap();
                let mut before_exclusion = fs::read(&exclusion_path).unwrap();
                let plan = repo.disable_plan(runtime, &source).unwrap();
                repo.apply_disable(&plan).unwrap();
                let restored = if mode == 3 {
                    let settings = fs::read_to_string(&plan.output)
                        .unwrap()
                        .replace("\"keep\": 1", "\"keep\": 2, \"user-edit\": true");
                    fs::write(&plan.output, &settings).unwrap();
                    Some(
                        claude_settings::remove_exclusion(&settings, source.to_str().unwrap())
                            .unwrap(),
                    )
                } else {
                    (mode == 2).then(|| original.to_owned())
                };
                let state = fs::read(&repo.state_path).unwrap();
                let mut exclusion = fs::read(&exclusion_path).unwrap();
                exclusion.extend_from_slice(b"# late user bytes without newline");
                before_exclusion.extend_from_slice(b"# late user bytes without newline");
                fs::write(&exclusion_path, &exclusion).unwrap();
                fail_local_commit(stage);
                assert!(
                    repo.restore_disable(runtime)
                        .unwrap_err()
                        .contains("partial")
                );
                clear_local_failure_and_check_unlock(&repo);
                let fresh = LocalRepository::discover(directory.path(), &repo.paths).unwrap();
                assert_eq!(fs::read(&repo.state_path).unwrap(), state);
                assert_eq!(
                    fs::read(&exclusion_path).unwrap(),
                    if stage == "state" {
                        before_exclusion
                    } else {
                        exclusion.clone()
                    }
                );
                assert_eq!(
                    fs::read(&plan.output).ok(),
                    restored.map(String::into_bytes)
                );
                assert_eq!(
                    fresh.disable_status(runtime).unwrap(),
                    if mode >= 2 {
                        DisableStatus::Modified
                    } else {
                        DisableStatus::Missing
                    }
                );
                assert!(
                    fresh
                        .restore_disable(runtime)
                        .unwrap_err()
                        .contains("partial")
                );
                let edited = if mode == 0 {
                    "user instructions\n"
                } else {
                    "{\"user-edit\": true}\n"
                };
                fs::write(&plan.output, edited).unwrap();
                assert!(fresh.restore_disable(runtime).is_err());
                assert_eq!(fs::read_to_string(&plan.output).unwrap(), edited);
                assert_eq!(fs::read(&repo.state_path).unwrap(), state);
                assert_eq!(fresh.load_state().unwrap().managed.len(), 1);
                assert_eq!(fs::read(&source).unwrap(), b"shared\n");
            }
        }
    }

    #[test]
    fn managed_partial_commits_do_not_adopt_unowned_output() {
        for stage in ["managed-output", "state"] {
            let (_home, directory, repo) = repository();
            personal_section(&repo.paths, "local\n");
            repo.create_managed(ManagedTarget::Agents, "local").unwrap();
            let plan = repo.managed_plan(ManagedTarget::Agents).unwrap();
            let manifest = fs::read(repo.local_manifest_path()).unwrap();
            let old_exclusion = fs::read_to_string(&plan.exclusion.path).unwrap();
            fail_local_commit(stage);
            assert!(repo.apply_managed(&plan).unwrap_err().contains("partial"));
            clear_local_failure_and_check_unlock(&repo);
            let fresh = LocalRepository::discover(directory.path(), &repo.paths).unwrap();
            assert!(!fresh.state_path.exists());
            assert_eq!(fs::read(repo.local_manifest_path()).unwrap(), manifest);
            assert_eq!(
                fs::read_to_string(&plan.exclusion.path).unwrap(),
                format!("{old_exclusion}{}\n", plan.exclusion.pattern)
            );
            let plan = fresh.managed_plan(ManagedTarget::Agents).unwrap();
            if stage == "managed-output" {
                assert_eq!(plan.view.status, LocalTargetStatus::Missing);
                fresh.apply_managed(&plan).unwrap();
            } else {
                assert_eq!(plan.view.status, LocalTargetStatus::External);
                assert!(fresh.apply_managed(&plan).unwrap_err().contains("partial"));
                fs::write(&plan.view.path, "user edit\n").unwrap();
                assert!(
                    fresh
                        .apply_managed(&fresh.managed_plan(ManagedTarget::Agents).unwrap())
                        .is_err()
                );
                assert_eq!(fs::read(&plan.view.path).unwrap(), b"user edit\n");
                assert!(!fresh.state_path.exists());
            }
            fresh
                .create_managed(ManagedTarget::Claude, "local")
                .unwrap();
            fresh
                .apply_managed(&fresh.managed_plan(ManagedTarget::Claude).unwrap())
                .unwrap();
            assert_eq!(
                fresh.load_state().unwrap().managed.len(),
                if stage == "managed-output" { 2 } else { 1 }
            );
        }
    }

    #[test]
    fn selected_ownership_refuses_foreign_destinations() {
        for runtime in [DisableRuntime::Pi, DisableRuntime::Claude] {
            let (_home, directory, repo) = repository();
            let source = directory.path().join(if runtime == DisableRuntime::Pi {
                "AGENTS.md"
            } else {
                "CLAUDE.md"
            });
            fs::write(&source, "shared\n").unwrap();
            let plan = repo.disable_plan(runtime, &source).unwrap();
            repo.apply_disable(&plan).unwrap();
            let valid = fs::read_to_string(&repo.state_path).unwrap();
            let output = fs::read(&plan.output).unwrap();
            let exclude_path = repo.common_git_dir.join("info/exclude");
            let exclude = fs::read(&exclude_path).unwrap();
            let foreign = TempDir::new().unwrap();
            let sentinel = foreign.path().join("sentinel");
            fs::write(&sentinel, &output).unwrap();
            for case in 0..5 {
                let mut raw: RawState = toml::from_str(&valid).unwrap();
                let owned = raw
                    .disables
                    .get_mut(&repo.disable_state_key(runtime))
                    .unwrap();
                match case {
                    0 => owned.path = sentinel.display().to_string(),
                    1 => owned.exclusion_path = Some(sentinel.display().to_string()),
                    2 => owned.exclusion_pattern = Some("/foreign".into()),
                    3 => {
                        owned.disabled_source =
                            Some(directory.path().join("wrong.md").display().to_string())
                    }
                    4 => {
                        owned.disabled_source = Some(
                            foreign
                                .path()
                                .join(source.file_name().unwrap())
                                .display()
                                .to_string(),
                        )
                    }
                    _ => unreachable!(),
                }
                let invalid = toml::to_string_pretty(&raw).unwrap();
                fs::write(&repo.state_path, &invalid).unwrap();
                assert!(repo.restore_disable(runtime).is_err(), "case {case}");
                assert!(repo.disable_status(runtime).is_err());
                assert_eq!(fs::read(&sentinel).unwrap(), output);
                assert_eq!(fs::read(&plan.output).unwrap(), output);
                assert_eq!(fs::read(&exclude_path).unwrap(), exclude);
                assert_eq!(fs::read_to_string(&repo.state_path).unwrap(), invalid);
            }
            fs::write(&repo.state_path, valid).unwrap();
            fs::remove_file(&source).unwrap();
            repo.restore_disable(runtime).unwrap();

            let subdirectory = directory.path().join("sub");
            fs::create_dir(&subdirectory).unwrap();
            let source = subdirectory.join(source.file_name().unwrap());
            fs::write(&source, "shared\n").unwrap();
            let plan = repo.disable_plan(runtime, &source).unwrap();
            repo.apply_disable(&plan).unwrap();
            let state = fs::read(&repo.state_path).unwrap();
            let exclusion = fs::read(&exclude_path).unwrap();
            fs::remove_dir_all(&subdirectory).unwrap();
            let fresh = LocalRepository::discover(directory.path(), &repo.paths).unwrap();
            if runtime == DisableRuntime::Claude {
                assert_eq!(fresh.disable_status(runtime).unwrap(), DisableStatus::Owned);
                fresh.restore_disable(runtime).unwrap();
                assert!(!plan.output.exists());
                assert_eq!(fresh.disable_status(runtime).unwrap(), DisableStatus::None);
                assert!(
                    !fs::read_to_string(&exclude_path)
                        .unwrap()
                        .lines()
                        .any(|line| line == "/.claude/settings.local.json")
                );
            } else {
                assert_eq!(
                    fresh.disable_status(runtime).unwrap(),
                    DisableStatus::Missing
                );
                let error = fresh.restore_disable(runtime).unwrap_err();
                assert!(error.contains("manual recovery"));
                assert_eq!(fs::read(&repo.state_path).unwrap(), state);
                assert_eq!(fs::read(&exclude_path).unwrap(), exclusion);
                assert!(!subdirectory.exists());
            }
        }
        for target in [ManagedTarget::Agents, ManagedTarget::Claude] {
            let (_home, directory, repo) = repository();
            personal_section(&repo.paths, "local\n");
            repo.create_managed(target, "local").unwrap();
            repo.apply_managed(&repo.managed_plan(target).unwrap())
                .unwrap();
            let plan = repo.managed_plan(target).unwrap();
            let valid = fs::read_to_string(&repo.state_path).unwrap();
            let sentinel = directory.path().join("foreign");
            fs::write(&sentinel, "foreign\n").unwrap();
            let exclusion = fs::read(repo.common_git_dir.join("info/exclude")).unwrap();
            for case in 0..3 {
                let mut raw: RawState = toml::from_str(&valid).unwrap();
                let owned = raw
                    .managed
                    .get_mut(&repo.managed_state_key(target))
                    .unwrap();
                match case {
                    0 => owned.path = sentinel.display().to_string(),
                    1 => owned.exclusion_path = Some(sentinel.display().to_string()),
                    2 => owned.exclusion_pattern = Some("/foreign".into()),
                    _ => unreachable!(),
                }
                let invalid = toml::to_string_pretty(&raw).unwrap();
                fs::write(&repo.state_path, &invalid).unwrap();
                assert!(repo.apply_managed(&plan).is_err());
                assert_eq!(fs::read(&sentinel).unwrap(), b"foreign\n");
                assert_eq!(fs::read(&plan.view.path).unwrap(), b"local\n");
                assert_eq!(
                    fs::read(repo.common_git_dir.join("info/exclude")).unwrap(),
                    exclusion
                );
                assert_eq!(fs::read_to_string(&repo.state_path).unwrap(), invalid);
            }
        }
    }

    #[test]
    fn claude_settings_symlinks_refuse_review_apply_and_restore() {
        use std::os::unix::fs::symlink;
        for dangling in [false, true] {
            for boundary in 0..3 {
                let (_home, directory, repo) = repository();
                let source = directory.path().join("CLAUDE.md");
                fs::write(&source, "shared\n").unwrap();
                let plan = repo.disable_plan(DisableRuntime::Claude, &source).unwrap();
                if boundary == 2 {
                    repo.apply_disable(&plan).unwrap();
                }
                let state = fs::read(&repo.state_path).ok();
                let exclusion = fs::read(repo.common_git_dir.join("info/exclude")).unwrap();
                let referent = directory.path().join("referent");
                if !dangling {
                    fs::write(&referent, "{}\n").unwrap();
                }
                fs::create_dir_all(plan.output.parent().unwrap()).unwrap();
                if boundary == 2 {
                    fs::remove_file(&plan.output).unwrap();
                }
                symlink(&referent, &plan.output).unwrap();
                let result = match boundary {
                    0 => repo
                        .disable_plan(DisableRuntime::Claude, &source)
                        .map(|_| ()),
                    1 => repo.apply_disable(&plan),
                    2 => repo.restore_disable(DisableRuntime::Claude),
                    _ => unreachable!(),
                };
                assert!(result.unwrap_err().contains("ordinary file"));
                if boundary == 0 {
                    assert!(!repo.state_path.with_file_name("overlays.lock").exists());
                }
                assert_eq!(fs::read_link(&plan.output).unwrap(), referent);
                assert_eq!(
                    fs::read(&referent).ok(),
                    (!dangling).then(|| b"{}\n".to_vec())
                );
                assert_eq!(fs::read(&repo.state_path).ok(), state);
                assert_eq!(
                    fs::read(repo.common_git_dir.join("info/exclude")).unwrap(),
                    exclusion
                );
            }
        }
    }

    #[test]
    fn tracked_managed_outputs_refuse_apply_and_adoption() {
        for target in [ManagedTarget::Agents, ManagedTarget::Claude] {
            for adoption in [false, true] {
                let (_home, directory, repo) = repository();
                personal_section(&repo.paths, "local\n");
                let output = directory.path().join(target.filename());
                let plan = if adoption {
                    None
                } else {
                    repo.create_managed(target, "local").unwrap();
                    repo.apply_managed(&repo.managed_plan(target).unwrap())
                        .unwrap();
                    Some(repo.managed_plan(target).unwrap())
                };
                fs::write(&output, "local\n").unwrap();
                git(directory.path(), &["add", "-f", target.filename()]).unwrap();
                let manifest = fs::read(repo.local_manifest_path()).ok();
                let state = fs::read(&repo.state_path).ok();
                let exclusion = fs::read(repo.common_git_dir.join("info/exclude")).unwrap();
                let result = match plan {
                    Some(plan) => repo.apply_managed(&plan).map(|_| ()),
                    None => repo.adopt_managed(target, "local"),
                };
                assert!(result.unwrap_err().contains("tracked"));
                assert_eq!(fs::read(output).unwrap(), b"local\n");
                assert_eq!(fs::read(repo.local_manifest_path()).ok(), manifest);
                assert_eq!(fs::read(&repo.state_path).ok(), state);
                assert_eq!(
                    fs::read(repo.common_git_dir.join("info/exclude")).unwrap(),
                    exclusion
                );
            }
        }
    }

    #[test]
    fn malformed_ownership_blocks_local_mutations_before_any_effect() {
        let (_home, repository, repo) = repository();
        personal_section(&repo.paths, "local\n");
        let source = repository.path().join("CLAUDE.md");
        fs::write(&source, "shared\n").unwrap();
        fs::write(
            repository.path().join(".git/info/exclude"),
            "/CLAUDE.local.md\n",
        )
        .unwrap();
        let disable_plan = repo.disable_plan(DisableRuntime::Claude, &source).unwrap();
        repo.apply_disable(&disable_plan).unwrap();
        repo.create_managed(ManagedTarget::Agents, "local").unwrap();
        let managed_plan = repo.managed_plan(ManagedTarget::Agents).unwrap();
        let valid = fs::read_to_string(&repo.state_path).unwrap();
        let manifest = fs::read(repo.local_manifest_path()).unwrap();
        let settings = fs::read(&disable_plan.output).unwrap();
        let exclusion_path = repository.path().join(".git/info/exclude");
        let exclusion = fs::read(&exclusion_path).unwrap();
        for case in 0..8 {
            let mut raw: RawState = toml::from_str(&valid).unwrap();
            let owned = raw.disables.get_mut("claude").unwrap();
            match case {
                0 => owned.disabled_source = None,
                1 => owned.exclusion_path = None,
                2 => owned.exclusion_pattern = None,
                3 => owned.created_container = false,
                4 => owned.original_content = Some("{}".into()),
                5 => {
                    owned.created_container = false;
                    owned.original_content = Some("invalid JSON".into());
                }
                6 => {
                    let owned = owned.clone();
                    raw.managed
                        .insert(repo.managed_state_key(ManagedTarget::Agents), owned);
                }
                7 => {
                    let owned = owned.clone();
                    raw.disables
                        .insert(repo.disable_state_key(DisableRuntime::Pi), owned);
                }
                _ => unreachable!(),
            }
            let malformed = toml::to_string_pretty(&raw).unwrap();
            fs::write(&repo.state_path, &malformed).unwrap();
            assert!(
                repo.restore_disable(DisableRuntime::Claude).is_err(),
                "case {case}"
            );
            assert!(repo.apply_disable(&disable_plan).is_err(), "case {case}");
            assert!(repo.apply_managed(&managed_plan).is_err(), "case {case}");
            assert!(
                repo.create_managed(ManagedTarget::Claude, "local").is_err(),
                "case {case}"
            );
            let adopt_output = repository.path().join("CLAUDE.local.md");
            fs::write(&adopt_output, "local\n").unwrap();
            assert!(
                repo.adopt_managed(ManagedTarget::Claude, "local").is_err(),
                "case {case}"
            );
            assert_eq!(fs::read(&adopt_output).unwrap(), b"local\n");
            fs::remove_file(adopt_output).unwrap();
            assert!(!managed_plan.view.path.exists());
            assert_eq!(fs::read(repo.local_manifest_path()).unwrap(), manifest);
            assert_eq!(fs::read(&disable_plan.output).unwrap(), settings);
            assert_eq!(fs::read(&exclusion_path).unwrap(), exclusion);
            assert_eq!(fs::read_to_string(&repo.state_path).unwrap(), malformed);
        }
        fs::write(&repo.state_path, valid).unwrap();
        fs::remove_file(&exclusion_path).unwrap();
        assert!(repo.restore_disable(DisableRuntime::Claude).is_err());
        assert_eq!(fs::read(&disable_plan.output).unwrap(), settings);
    }

    #[test]
    fn managed_status_distinguishes_stale_and_modified() {
        let home = TempDir::new().unwrap();
        let repository = TempDir::new().unwrap();
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(repository.path())
            .status()
            .unwrap();
        fs::write(repository.path().join("AGENTS.md"), "shared\n").unwrap();
        let paths = Paths::for_home(home.path());
        personal_section(&paths, "shared\n");
        let repo = LocalRepository::discover(repository.path(), &paths).unwrap();
        repo.create_managed(ManagedTarget::Agents, "local").unwrap();
        let plan = repo.managed_plan(ManagedTarget::Agents).unwrap();
        repo.apply_managed(&plan).unwrap();
        assert_eq!(
            repo.inspect_managed(ManagedTarget::Agents).unwrap().status,
            LocalTargetStatus::Current
        );
        fs::write(
            repo.managed_section_path_for(ManagedTarget::Agents, "local")
                .unwrap(),
            "changed\n",
        )
        .unwrap();
        assert_eq!(
            repo.inspect_managed(ManagedTarget::Agents).unwrap().status,
            LocalTargetStatus::Stale
        );
        fs::write(repository.path().join("AGENTS.override.md"), "external\n").unwrap();
        assert_eq!(
            repo.inspect_managed(ManagedTarget::Agents).unwrap().status,
            LocalTargetStatus::Modified
        );
    }

    #[test]
    fn managed_override_can_start_without_a_project_agents_file() {
        let home = TempDir::new().unwrap();
        let repository = TempDir::new().unwrap();
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(repository.path())
            .status()
            .unwrap();
        let paths = Paths::for_home(home.path());
        personal_section(&paths, "# Only on this machine\n");
        let repo = LocalRepository::discover(repository.path(), &paths).unwrap();

        repo.create_managed(ManagedTarget::Agents, "local").unwrap();

        assert!(!repository.path().join("AGENTS.override.md").exists());
        assert_eq!(
            repo.render_managed(ManagedTarget::Agents).unwrap(),
            "# Only on this machine\n"
        );
        let plan = repo.managed_plan(ManagedTarget::Agents).unwrap();
        repo.apply_managed(&plan).unwrap();
        assert_eq!(
            fs::read_to_string(repository.path().join("AGENTS.override.md")).unwrap(),
            "# Only on this machine\n"
        );
    }

    #[test]
    fn managed_apply_rejects_an_exclusion_change_after_review() {
        let home = TempDir::new().unwrap();
        let repository = TempDir::new().unwrap();
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(repository.path())
            .status()
            .unwrap();
        let paths = Paths::for_home(home.path());
        personal_section(&paths, "# Local\n");
        let repo = LocalRepository::discover(repository.path(), &paths).unwrap();
        repo.create_managed(ManagedTarget::Agents, "local").unwrap();
        let plan = repo.managed_plan(ManagedTarget::Agents).unwrap();
        let exclusion = repository.path().join(".git/info/exclude");
        let mut source = fs::read_to_string(&exclusion).unwrap();
        source.push_str("/AGENTS.override.md\n");
        fs::write(exclusion, source).unwrap();

        let error = repo.apply_managed(&plan).unwrap_err();

        assert!(error.contains("changed after review"));
        assert!(!repository.path().join("AGENTS.override.md").exists());
    }

    #[test]
    fn managed_apply_repairs_a_missing_exclusion_for_current_content() {
        let home = TempDir::new().unwrap();
        let repository = TempDir::new().unwrap();
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(repository.path())
            .status()
            .unwrap();
        let paths = Paths::for_home(home.path());
        personal_section(&paths, "# Local\n");
        let repo = LocalRepository::discover(repository.path(), &paths).unwrap();
        repo.create_managed(ManagedTarget::Agents, "local").unwrap();
        let plan = repo.managed_plan(ManagedTarget::Agents).unwrap();
        repo.apply_managed(&plan).unwrap();
        let exclusion = repository.path().join(".git/info/exclude");
        let source = fs::read_to_string(&exclusion)
            .unwrap()
            .lines()
            .filter(|line| *line != "/AGENTS.override.md")
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        fs::write(&exclusion, source).unwrap();

        let plan = repo.managed_plan(ManagedTarget::Agents).unwrap();

        assert_eq!(plan.view.status, LocalTargetStatus::Current);
        assert!(plan.exclusion_write().is_some());
        repo.apply_managed(&plan).unwrap();
        let owned_state = fs::read(&repo.state_path).unwrap();
        let current = repo.managed_plan(ManagedTarget::Agents).unwrap();
        repo.apply_managed(&current).unwrap();
        assert_eq!(fs::read(&repo.state_path).unwrap(), owned_state);
        assert!(
            fs::read_to_string(exclusion)
                .unwrap()
                .lines()
                .any(|line| line == "/AGENTS.override.md")
        );
    }

    #[test]
    fn local_manifest_keeps_both_managed_targets() {
        let home = TempDir::new().unwrap();
        let repository = TempDir::new().unwrap();
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(repository.path())
            .status()
            .unwrap();
        let paths = Paths::for_home(home.path());
        personal_section(&paths, "# Local\n");
        let repo = LocalRepository::discover(repository.path(), &paths).unwrap();

        repo.create_managed(ManagedTarget::Agents, "local").unwrap();
        repo.create_managed(ManagedTarget::Claude, "local").unwrap();
        assert!(
            repo.create_managed(ManagedTarget::Agents, "local")
                .unwrap_err()
                .contains("already exist")
        );

        assert!(repo.managed_exists(ManagedTarget::Agents));
        assert!(repo.managed_exists(ManagedTarget::Claude));
        assert_eq!(
            repo.render_managed(ManagedTarget::Agents).unwrap(),
            "# Local\n"
        );
        assert_eq!(
            repo.render_managed(ManagedTarget::Claude).unwrap(),
            "# Local\n"
        );
    }

    #[test]
    fn malformed_local_manifest_does_not_claim_managed_targets() {
        let home = TempDir::new().unwrap();
        let repository = TempDir::new().unwrap();
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(repository.path())
            .status()
            .unwrap();
        let paths = Paths::for_home(home.path());
        let repo = LocalRepository::discover(repository.path(), &paths).unwrap();
        fs::create_dir_all(&repo.data_dir).unwrap();
        fs::write(
            repo.data_dir.join("local.toml"),
            "format = 1\nordered = [\"local\"]\n",
        )
        .unwrap();

        assert!(repo.local_manifest_error().is_some());
        assert!(!repo.managed_exists(ManagedTarget::Agents));
        assert!(!repo.managed_exists(ManagedTarget::Claude));
    }

    #[test]
    fn adopt_reports_content_mismatch_before_missing_exclusion() {
        let home = TempDir::new().unwrap();
        let repository = TempDir::new().unwrap();
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(repository.path())
            .status()
            .unwrap();
        let paths = Paths::for_home(home.path());
        personal_section(&paths, "expected\n");
        fs::write(repository.path().join("AGENTS.override.md"), "different\n").unwrap();
        let repo = LocalRepository::discover(repository.path(), &paths).unwrap();

        let error = repo
            .adopt_managed(ManagedTarget::Agents, "local")
            .unwrap_err();

        assert!(error.contains("does not render byte-for-byte"));
        assert!(!error.contains("not ignored"));
    }

    #[test]
    fn adopt_directs_the_missing_exclusion_without_writing_it() {
        let home = TempDir::new().unwrap();
        let repository = TempDir::new().unwrap();
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(repository.path())
            .status()
            .unwrap();
        let paths = Paths::for_home(home.path());
        personal_section(&paths, "expected\n");
        fs::write(repository.path().join("AGENTS.override.md"), "expected\n").unwrap();
        let repo = LocalRepository::discover(repository.path(), &paths).unwrap();
        let exclusion = repository.path().join(".git/info/exclude");
        let before = fs::read_to_string(&exclusion).unwrap();

        let error = repo
            .adopt_managed(ManagedTarget::Agents, "local")
            .unwrap_err();

        assert!(error.contains("/AGENTS.override.md"));
        assert!(error.contains(&exclusion.display().to_string()));
        assert_eq!(fs::read_to_string(exclusion).unwrap(), before);
        assert!(!repo.local_manifest_path().exists());
    }

    #[test]
    fn linked_worktrees_share_composition_but_track_outputs_separately() {
        let home = TempDir::new().unwrap();
        let main = TempDir::new().unwrap();
        let linked_parent = TempDir::new().unwrap();
        let linked = linked_parent.path().join("linked");
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(main.path())
            .status()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(main.path())
            .status()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(main.path())
            .status()
            .unwrap();
        fs::write(main.path().join("AGENTS.md"), "shared\n").unwrap();
        Command::new("git")
            .args(["add", "AGENTS.md"])
            .current_dir(main.path())
            .status()
            .unwrap();
        Command::new("git")
            .args(["commit", "-qm", "fixture"])
            .current_dir(main.path())
            .status()
            .unwrap();
        assert!(
            Command::new("git")
                .args(["worktree", "add", "-qb", "linked", linked.to_str().unwrap()])
                .current_dir(main.path())
                .status()
                .unwrap()
                .success()
        );

        let paths = Paths::for_home(home.path());
        personal_section(&paths, "shared\n");
        let main_repo = LocalRepository::discover(main.path(), &paths).unwrap();
        let linked_repo = LocalRepository::discover(&linked, &paths).unwrap();
        assert_eq!(main_repo.data_dir, linked_repo.data_dir);
        assert_eq!(main_repo.state_path, linked_repo.state_path);
        main_repo
            .create_managed(ManagedTarget::Agents, "local")
            .unwrap();
        let plan = main_repo.managed_plan(ManagedTarget::Agents).unwrap();
        assert_eq!(
            plan.exclusion_write().unwrap().1,
            main.path().join(".git/info/exclude")
        );
        linked_repo
            .create_managed(ManagedTarget::Claude, "local")
            .unwrap();
        let linked_plan = linked_repo.managed_plan(ManagedTarget::Claude).unwrap();
        let (locked_tx, locked_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            let first = scope.spawn(|| {
                LOCAL_COMMIT_HOOK.set(Some(Box::new(move |stage| {
                    if stage == "locked" {
                        locked_tx.send(()).unwrap();
                        release_rx.recv().unwrap();
                    }
                    Ok(())
                })));
                let result = main_repo.apply_managed(&plan);
                LOCAL_COMMIT_HOOK.set(None);
                result
            });
            locked_rx.recv().unwrap();
            // A separately opened handle proves kernel exclusion, without a timing assertion.
            let contender = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(linked_repo.state_path.with_file_name("overlays.lock"))
                .unwrap();
            let contention = contender.try_lock();
            if contention.is_ok() {
                contender.unlock().unwrap();
            }
            let (attempt_tx, attempt_rx) = std::sync::mpsc::channel();
            let linked_writer = &linked_repo;
            let second = scope.spawn(move || {
                attempt_tx.send(()).unwrap();
                linked_writer.apply_managed(&linked_plan)
            });
            attempt_rx.recv().unwrap();
            release_tx.send(()).unwrap();
            first.join().unwrap().unwrap();
            second.join().unwrap().unwrap();
            assert!(matches!(contention, Err(std::fs::TryLockError::WouldBlock)));
        });
        let state = main_repo.load_state().unwrap();
        assert!(
            state
                .managed
                .contains_key(&main_repo.managed_state_key(ManagedTarget::Agents))
        );
        assert!(
            state
                .managed
                .contains_key(&linked_repo.managed_state_key(ManagedTarget::Claude))
        );
        let exclusion = fs::read_to_string(main_repo.common_git_dir.join("info/exclude")).unwrap();
        assert!(exclusion.lines().any(|line| line == "/AGENTS.override.md"));
        assert!(exclusion.lines().any(|line| line == "/CLAUDE.local.md"));
        assert_eq!(
            linked_repo
                .inspect_managed(ManagedTarget::Agents)
                .unwrap()
                .status,
            LocalTargetStatus::Missing
        );
        let plan = linked_repo.managed_plan(ManagedTarget::Agents).unwrap();
        assert!(plan.exclusion_write().is_none());
        linked_repo.apply_managed(&plan).unwrap();
        assert_eq!(
            main_repo
                .inspect_managed(ManagedTarget::Agents)
                .unwrap()
                .status,
            LocalTargetStatus::Current
        );
        assert_eq!(
            linked_repo
                .inspect_managed(ManagedTarget::Agents)
                .unwrap()
                .status,
            LocalTargetStatus::Current
        );

        let claude = linked.join("CLAUDE.md");
        fs::write(&claude, "claude\n").unwrap();
        let plan = linked_repo
            .disable_plan(DisableRuntime::Claude, &claude)
            .unwrap();
        assert_eq!(plan.output, main.path().join(".claude/settings.local.json"));
        linked_repo.apply_disable(&plan).unwrap();
        assert!(main.path().join(".claude/settings.local.json").is_file());
        assert!(!linked.join(".claude/settings.local.json").exists());
        fs::remove_file(&claude).unwrap();
        assert_eq!(
            main_repo.disable_status(DisableRuntime::Claude).unwrap(),
            DisableStatus::Owned
        );
        main_repo.restore_disable(DisableRuntime::Claude).unwrap();
        assert!(!plan.output.exists());
        assert_eq!(main_repo.load_state().unwrap().managed.len(), 3);

        for settings_case in 0..3 {
            if settings_case > 0 {
                git(
                    main.path(),
                    &["worktree", "add", linked.to_str().unwrap(), "linked"],
                )
                .unwrap();
            }
            let original = "{\"user\": 1, \"claudeMdExcludes\": [\"/unrelated.md\"]}\n";
            if settings_case > 0 {
                fs::write(&plan.output, original).unwrap();
            }
            fs::write(&claude, "claude\n").unwrap();
            let plan = linked_repo
                .disable_plan(DisableRuntime::Claude, &claude)
                .unwrap();
            linked_repo.apply_disable(&plan).unwrap();
            if settings_case == 2 {
                let settings = fs::read_to_string(&plan.output).unwrap();
                fs::write(&plan.output, settings.replace("\"user\": 1", "\"user\": 2")).unwrap();
            }
            let managed = fs::read_to_string(&main_repo.state_path).unwrap();
            let managed: RawState = toml::from_str(&managed).unwrap();
            let managed = toml::to_string(&managed.managed).unwrap();
            let exclusion = fs::read_to_string(main_repo.common_git_dir.join("info/exclude"))
                .unwrap()
                .replace("/.claude/settings.local.json\n", "");
            git(
                main.path(),
                &["worktree", "remove", "--force", linked.to_str().unwrap()],
            )
            .unwrap();
            let fresh = LocalRepository::discover(main.path(), &paths).unwrap();
            assert_eq!(
                fresh.disable_status(DisableRuntime::Claude).unwrap(),
                DisableStatus::Owned
            );
            assert_eq!(
                fresh.disable_status(DisableRuntime::Pi).unwrap(),
                DisableStatus::None
            );
            fresh.restore_disable(DisableRuntime::Claude).unwrap();
            if settings_case == 0 {
                assert!(!plan.output.exists());
            } else {
                assert_eq!(
                    fs::read_to_string(&plan.output).unwrap(),
                    original.replace(
                        "\"user\": 1",
                        if settings_case == 2 {
                            "\"user\": 2"
                        } else {
                            "\"user\": 1"
                        }
                    )
                );
            }
            let restored = fs::read_to_string(&fresh.state_path).unwrap();
            let restored: RawState = toml::from_str(&restored).unwrap();
            assert!(restored.disables.is_empty());
            assert_eq!(toml::to_string(&restored.managed).unwrap(), managed);
            assert_eq!(
                fs::read_to_string(fresh.common_git_dir.join("info/exclude")).unwrap(),
                exclusion
            );
        }
    }
}
