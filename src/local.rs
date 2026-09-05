//! Local scope: machine-local instruction files (CLAUDE.local.md, AGENTS.override.md) and local disables for one Git worktree.
//! The ownership-state file keeps its `overlays.toml` name and emitted `overlay` error text is unchanged.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::config::{GlobalConfig, Paths, atomic_create, atomic_write, sha256_hex};
use crate::deploy::unified_diff;

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
struct Owned {
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
struct State {
    #[serde(default)]
    disables: std::collections::BTreeMap<String, Owned>,
    #[serde(default)]
    managed: std::collections::BTreeMap<String, Owned>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Exclusion {
    path: PathBuf,
    pattern: String,
    needs_write: bool,
}

impl LocalRepository {
    pub(crate) fn discover(start: &Path, paths: &Paths) -> Result<Self, String> {
        let root = crate::project::worktree_root(start).map_err(|error| {
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
        Ok(self
            .load_state()?
            .disables
            .get(&self.disable_state_key(runtime))
            .and_then(|owned| owned.disabled_source.as_deref())
            .map(PathBuf::from))
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
                const CANDIDATES: [&str; 5] = [
                    "AGENTS.override.md",
                    "AGENTS.md",
                    "AGENTS.MD",
                    "CLAUDE.md",
                    "CLAUDE.MD",
                ];
                if filename == "AGENTS.override.md" {
                    return Err("Pi's selected override cannot disable itself".into());
                }
                if !CANDIDATES.contains(&filename) {
                    return Err(format!(
                        "{} is not a Pi instruction candidate",
                        source.display()
                    ));
                }
                let selected = CANDIDATES
                    .iter()
                    .map(|name| parent.join(name))
                    .find(|candidate| candidate.is_file());
                if selected.as_deref() != Some(source.as_path()) {
                    let selected = selected
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|| "no candidate".into());
                    return Err(format!(
                        "{} is not selected by Pi; {selected} takes priority",
                        source.display()
                    ));
                }
            }
        }
        Ok(source)
    }

    pub(crate) fn apply_disable(&self, plan: &DisablePlan) -> Result<(), String> {
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
                atomic_create(&plan.output, b"")?;
                state.disables.insert(
                    state_key.clone(),
                    Owned {
                        path: plan.output.display().to_string(),
                        hash: sha256_hex(b""),
                        disabled_source: Some(plan.source.display().to_string()),
                        exclusion_path: plan
                            .exclusion
                            .as_ref()
                            .filter(|exclusion| exclusion.needs_write)
                            .map(|exclusion| exclusion.path.display().to_string()),
                        exclusion_pattern: plan
                            .exclusion
                            .as_ref()
                            .filter(|exclusion| exclusion.needs_write)
                            .map(|exclusion| exclusion.pattern.clone()),
                        created_container: false,
                        original_content: None,
                    },
                );
            }
            DisableRuntime::Claude => {
                let settings_worktree = self.claude_settings_worktree()?;
                ensure_untracked(&settings_worktree, &plan.output)?;
                if let Some(exclusion) = &plan.exclusion {
                    apply_exclusion(exclusion, &settings_worktree, &plan.output)?;
                }
                let old = plan
                    .reviewed_output
                    .clone()
                    .unwrap_or_else(|| "{}\n".into());
                let created = plan.reviewed_output.is_none();
                let object = parse_json_object(&old, &plan.output)?;
                let excludes = object.get("claudeMdExcludes").and_then(Value::as_array);
                let source = plan.source.display().to_string();
                if excludes.is_some_and(|entries| {
                    entries.iter().any(|entry| entry.as_str() == Some(&source))
                }) {
                    return Err("the selected Claude source is already excluded".into());
                }
                let rendered = insert_json_array_string(&old, "claudeMdExcludes", &source)?;
                atomic_write(&plan.output, rendered.as_bytes())?;
                state.disables.insert(
                    state_key,
                    Owned {
                        path: plan.output.display().to_string(),
                        hash: sha256_hex(rendered.as_bytes()),
                        disabled_source: Some(plan.source.display().to_string()),
                        exclusion_path: plan
                            .exclusion
                            .as_ref()
                            .filter(|exclusion| exclusion.needs_write)
                            .map(|exclusion| exclusion.path.display().to_string()),
                        exclusion_pattern: plan
                            .exclusion
                            .as_ref()
                            .filter(|exclusion| exclusion.needs_write)
                            .map(|exclusion| exclusion.pattern.clone()),
                        created_container: created,
                        original_content: (!created).then_some(old),
                    },
                );
            }
        }
        self.save_state(&state)
    }

    pub(crate) fn restore_disable(&self, runtime: DisableRuntime) -> Result<(), String> {
        let mut state = self.load_state()?;
        let state_key = self.disable_state_key(runtime);
        let owned = state
            .disables
            .get(&state_key)
            .cloned()
            .ok_or_else(|| format!("no mdmanager.ai-owned {} disable", runtime.key()))?;
        let path = PathBuf::from(&owned.path);
        let bytes = fs::read(&path)
            .map_err(|error| format!("cannot read owned overlay {}: {error}", path.display()))?;
        let unchanged = sha256_hex(&bytes) == owned.hash;
        match runtime {
            DisableRuntime::Pi => {
                if !unchanged {
                    return Err(format!(
                        "{} changed after mdmanager.ai wrote it; refusing restore",
                        path.display()
                    ));
                }
                fs::remove_file(&path)
                    .map_err(|error| format!("cannot remove {}: {error}", path.display()))?;
                remove_owned_exclusion(&owned)?;
            }
            DisableRuntime::Claude => {
                if unchanged && owned.created_container {
                    fs::remove_file(&path)
                        .map_err(|error| format!("cannot remove {}: {error}", path.display()))?;
                } else if unchanged {
                    let original = owned.original_content.as_deref().ok_or_else(|| {
                        "Claude ownership record does not contain the original settings".to_owned()
                    })?;
                    atomic_write(&path, original.as_bytes())?;
                } else {
                    let source = String::from_utf8(bytes)
                        .map_err(|_| format!("{} is not UTF-8", path.display()))?;
                    let disabled_source = owned.disabled_source.as_deref().ok_or_else(|| {
                        "Claude ownership record does not contain its disabled source".to_owned()
                    })?;
                    let rendered =
                        remove_json_array_string(&source, "claudeMdExcludes", disabled_source)?;
                    atomic_write(&path, rendered.as_bytes())?;
                }
                remove_owned_exclusion(&owned)?;
            }
        }
        state.disables.remove(&state_key);
        self.save_state(&state)
    }

    pub(crate) fn disable_status(&self, runtime: DisableRuntime) -> Result<DisableStatus, String> {
        let state = self.load_state()?;
        let Some(owned) = state.disables.get(&self.disable_state_key(runtime)) else {
            return Ok(DisableStatus::None);
        };
        match fs::read(&owned.path) {
            Ok(bytes) if runtime == DisableRuntime::Claude => {
                let active = String::from_utf8(bytes).ok().is_some_and(|source| {
                    let Some(disabled_source) = owned.disabled_source.as_deref() else {
                        return false;
                    };
                    parse_json_object(&source, Path::new(&owned.path))
                        .ok()
                        .and_then(|object| object.get("claudeMdExcludes").cloned())
                        .and_then(|value| value.as_array().cloned())
                        .is_some_and(|entries| {
                            entries
                                .iter()
                                .any(|entry| entry.as_str() == Some(disabled_source))
                        })
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
        let output = self.root.join(target.filename());
        if output.exists() {
            return Err(format!("refusing to overwrite {}", output.display()));
        }
        self.personal_library_section(section)?;
        self.create_local_target(target, section)
    }

    pub(crate) fn adopt_managed(&self, target: ManagedTarget, section: &str) -> Result<(), String> {
        let output = self.root.join(target.filename());
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
                disabled_source: None,
                exclusion_path: None,
                exclusion_pattern: None,
                created_container: false,
                original_content: None,
            },
        );
        self.save_state(&state)
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
        let deployed = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
        };
        let state = self.load_state()?;
        let managed_key = self.managed_state_key(target);
        let owned = state.managed.get(&managed_key);
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
            return Err(format!(
                "{} is {}; refusing to overwrite it",
                view.path.display(),
                view.status.label()
            ));
        }
        if exclusion.needs_write {
            apply_exclusion(&exclusion, &self.root, &view.path)?;
        }
        if view.status != LocalTargetStatus::Current {
            atomic_write(&view.path, view.rendered.as_bytes())?;
        }
        let mut state = self.load_state()?;
        state.managed.insert(
            self.managed_state_key(target),
            Owned {
                path: view.path.display().to_string(),
                hash: sha256_hex(view.rendered.as_bytes()),
                disabled_source: None,
                exclusion_path: exclusion
                    .needs_write
                    .then(|| exclusion.path.display().to_string()),
                exclusion_pattern: exclusion.needs_write.then_some(exclusion.pattern),
                created_container: false,
                original_content: None,
            },
        );
        self.save_state(&state)?;
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
        if output.exists() {
            return Err(format!("refusing to overwrite {}", output.display()));
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
        let settings_worktree = self.claude_settings_worktree()?;
        let output = settings_worktree.join(".claude/settings.local.json");
        ensure_untracked(&settings_worktree, &output)?;
        let exclusion = self.exclusion_for(&settings_worktree, &output)?;
        let reviewed_output = match fs::read_to_string(&output) {
            Ok(source) => Some(source),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(format!("cannot read {}: {error}", output.display())),
        };
        if let Some(source_text) = &reviewed_output {
            let object = parse_json_object(source_text, &output)?;
            if object
                .get("claudeMdExcludes")
                .and_then(Value::as_array)
                .is_some_and(|entries| {
                    entries
                        .iter()
                        .any(|entry| entry.as_str() == Some(source.to_string_lossy().as_ref()))
                })
            {
                return Err("the selected Claude source is already excluded".into());
            }
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

    fn claude_settings_worktree(&self) -> Result<PathBuf, String> {
        let output = git(&self.root, &["worktree", "list", "--porcelain", "-z"])?;
        let path = output
            .strip_prefix("worktree ")
            .and_then(|output| output.split_once('\0'))
            .map(|(path, _)| path)
            .filter(|path| !path.is_empty())
            .ok_or_else(|| "git did not report a main worktree".to_owned())?;
        let path = fs::canonicalize(path)
            .map_err(|error| format!("cannot resolve reported main worktree: {error}"))?;
        let is_worktree = git(&path, &["rev-parse", "--is-inside-work-tree"])?;
        if is_worktree.trim() == "true" {
            Ok(path)
        } else {
            // Submodules and separate Git directories report their Git directory here.
            Ok(self.root.clone())
        }
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
            Ok(source) => toml::from_str(&source)
                .map_err(|error| format!("invalid {}: {error}", self.state_path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
            Err(error) => Err(format!(
                "cannot read {}: {error}",
                self.state_path.display()
            )),
        }
    }

    fn save_state(&self, state: &State) -> Result<(), String> {
        let source = toml::to_string_pretty(state)
            .map_err(|error| format!("cannot serialize overlay state: {error}"))?;
        atomic_write(&self.state_path, source.as_bytes())
    }
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

fn parse_json_object(source: &str, path: &Path) -> Result<Map<String, Value>, String> {
    let value: Value = serde_json::from_str(source)
        .map_err(|error| format!("invalid {}: {error}", path.display()))?;
    value
        .as_object()
        .cloned()
        .ok_or_else(|| format!("{} must contain a JSON object", path.display()))
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

fn insert_json_array_string(source: &str, key: &str, value: &str) -> Result<String, String> {
    let value = serde_json::to_string(value)
        .map_err(|error| format!("cannot serialize Claude exclusion: {error}"))?;
    if let Some((open, close)) = json_array_range(source, key)? {
        let interior = &source[open + 1..close];
        let insertion = if interior.trim().is_empty() {
            value
        } else if interior.contains('\n') {
            let indent = interior
                .trim_end()
                .rsplit('\n')
                .next()
                .unwrap_or_default()
                .chars()
                .take_while(|character| character.is_whitespace())
                .collect::<String>();
            format!(",\n{indent}{value}")
        } else {
            format!(", {value}")
        };
        let insert_at = close - interior.len() + interior.trim_end().len();
        let mut output = source.to_owned();
        output.insert_str(insert_at, &insertion);
        return Ok(output);
    }

    let (open, close) = top_level_object_range(source)?;
    let interior = &source[open + 1..close];
    let property = format!("\"{key}\": [{value}]");
    let insertion = if interior.trim().is_empty() {
        format!("\n  {property}\n")
    } else if interior.contains('\n') {
        let indent = interior
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or_default()
            .chars()
            .take_while(|character| character.is_whitespace())
            .collect::<String>();
        format!(",\n{indent}{property}")
    } else {
        format!(", {property}")
    };
    let insert_at = open + 1 + interior.trim_end().len();
    let mut output = source.to_owned();
    output.insert_str(insert_at, &insertion);
    Ok(output)
}

fn remove_json_array_string(source: &str, key: &str, value: &str) -> Result<String, String> {
    parse_json_object(source, Path::new("Claude settings"))?;
    let (open, close) = json_array_range(source, key)?
        .ok_or_else(|| format!("Claude settings no longer contain {key}"))?;
    let needle = serde_json::to_string(value)
        .map_err(|error| format!("cannot serialize Claude exclusion: {error}"))?;
    for (relative, _) in source[open + 1..close].match_indices(&needle) {
        let start = open + 1 + relative;
        let end = start + needle.len();
        let before = source[open..start].trim_end();
        let after = source[end..=close].trim_start();
        if !matches!(before.as_bytes().last(), Some(b'[' | b','))
            || !matches!(after.as_bytes().first(), Some(b',' | b']'))
        {
            continue;
        }

        let mut output = source.to_owned();
        if after.starts_with(',') {
            let comma = end + source[end..=close].len() - after.len();
            let leading_whitespace = open + before.len();
            output.replace_range(leading_whitespace..comma + 1, "");
        } else if before.ends_with(',') {
            let comma = open + before.len() - 1;
            output.replace_range(comma..end, "");
        } else {
            output.replace_range(start..end, "");
        }
        return Ok(output);
    }
    Err(format!(
        "Claude settings no longer contain the mdmanager.ai-owned exclusion {value}"
    ))
}

fn json_array_range(source: &str, key: &str) -> Result<Option<(usize, usize)>, String> {
    let bytes = source.as_bytes();
    let mut index = 0;
    let mut object_depth = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                let end = json_string_end(bytes, index)?;
                if object_depth == 1 {
                    let decoded: String = serde_json::from_str(&source[index..end])
                        .map_err(|error| format!("invalid JSON property: {error}"))?;
                    let mut next = skip_json_space(bytes, end);
                    if decoded == key && bytes.get(next) == Some(&b':') {
                        next = skip_json_space(bytes, next + 1);
                        if bytes.get(next) != Some(&b'[') {
                            return Err(format!("{key} is not an array"));
                        }
                        return Ok(Some((next, matching_json_bracket(bytes, next)?)));
                    }
                }
                index = end;
                continue;
            }
            b'{' => object_depth += 1,
            b'}' => object_depth = object_depth.saturating_sub(1),
            _ => {}
        }
        index += 1;
    }
    Ok(None)
}

fn top_level_object_range(source: &str) -> Result<(usize, usize), String> {
    let bytes = source.as_bytes();
    let mut index = skip_json_space(bytes, 0);
    if bytes.get(index) != Some(&b'{') {
        return Err("Claude settings must contain a JSON object".into());
    }
    let open = index;
    index += 1;
    let mut depth = 1usize;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                index = json_string_end(bytes, index)?;
                continue;
            }
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Ok((open, index));
                }
            }
            _ => {}
        }
        index += 1;
    }
    Err("Claude settings JSON object is not closed".into())
}

fn matching_json_bracket(bytes: &[u8], open: usize) -> Result<usize, String> {
    let mut index = open + 1;
    let mut depth = 1usize;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                index = json_string_end(bytes, index)?;
                continue;
            }
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(index);
                }
            }
            _ => {}
        }
        index += 1;
    }
    Err("claudeMdExcludes array is not closed".into())
}

fn json_string_end(bytes: &[u8], start: usize) -> Result<usize, String> {
    let mut index = start + 1;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 2,
            b'"' => return Ok(index + 1),
            _ => index += 1,
        }
    }
    Err("unterminated JSON string".into())
}

fn skip_json_space(bytes: &[u8], mut index: usize) -> usize {
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    index
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

fn remove_owned_exclusion(owned: &Owned) -> Result<(), String> {
    let (Some(path), Some(pattern)) = (&owned.exclusion_path, &owned.exclusion_pattern) else {
        return Ok(());
    };
    let path = PathBuf::from(path);
    let source = fs::read_to_string(&path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let mut removed = false;
    let mut output = source
        .lines()
        .filter(|line| {
            if !removed && *line == pattern {
                removed = true;
                false
            } else {
                true
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    if !output.is_empty() {
        output.push('\n');
    }
    atomic_write(&path, output.as_bytes())
}

fn git(directory: &Path, arguments: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(directory)
        .output()
        .map_err(|error| format!("cannot run git: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout).map_err(|_| "git returned non-UTF-8 output".to_owned())
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

    #[test]
    fn pi_disable_warns_when_codex_compatibility_is_not_verified() {
        let (_home, repository, local_repository) = repository();
        let source = repository.path().join("AGENTS.md");
        fs::write(&source, "agents\n").unwrap();

        let plan = local_repository
            .disable_plan(DisableRuntime::Pi, &source)
            .unwrap();
        assert!(plan.text.contains("Codex"));
        assert!(plan.text.contains("Codex compatibility"));
        assert!(!plan.text.contains("fixture"));
        assert!(plan.exclusion.as_ref().unwrap().needs_write);
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
    fn claude_disable_plan_keeps_the_logical_source_path() {
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

        let plan = local_repository
            .disable_plan(DisableRuntime::Claude, &source)
            .unwrap();
        assert_eq!(plan.output, worktree.join(".claude/settings.local.json"));
        local_repository.apply_disable(&plan).unwrap();

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

        let plan = local_repository
            .disable_plan(DisableRuntime::Claude, &source)
            .unwrap();
        assert_eq!(plan.output, worktree.join(".claude/settings.local.json"));
        local_repository.apply_disable(&plan).unwrap();

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
    fn inserts_into_an_existing_exclusion_array_without_reformatting() {
        let source = "{\n  \"before\": 1,\n  \"claudeMdExcludes\": [\n    \"/before.md\"\n  ],\n  \"after\": 2\n}\n";
        let updated = insert_json_array_string(source, "claudeMdExcludes", "/new.md").unwrap();
        assert_eq!(
            updated,
            "{\n  \"before\": 1,\n  \"claudeMdExcludes\": [\n    \"/before.md\",\n    \"/new.md\"\n  ],\n  \"after\": 2\n}\n"
        );
    }

    #[test]
    fn removes_only_the_owned_exclusion_without_reformatting() {
        let source = "{\n  \"before\": 1,\n  \"claudeMdExcludes\": [\n    \"/before.md\",\n    \"/owned.md\",\n    \"/after.md\"\n  ],\n  \"after\": 2\n}\n";
        let updated = remove_json_array_string(source, "claudeMdExcludes", "/owned.md").unwrap();
        assert_eq!(
            updated,
            "{\n  \"before\": 1,\n  \"claudeMdExcludes\": [\n    \"/before.md\",\n    \"/after.md\"\n  ],\n  \"after\": 2\n}\n"
        );
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
        main_repo.apply_managed(&plan).unwrap();
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
    }
}
