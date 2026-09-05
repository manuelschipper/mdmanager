use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use similar::TextDiff;

use crate::config::{GlobalConfig, atomic_write, sha256_hex};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct State {
    active_profile: Option<String>,
    #[serde(default)]
    targets: IndexMap<String, AppliedTarget>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AppliedTarget {
    path: String,
    hash: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Global target status: how a configured Global target file on disk compares with
/// the active Profile's rendered composition. `label()` owns the displayed status
/// vocabulary shared by the CLI and TUI.
pub(crate) enum GlobalTargetStatus {
    Current,
    Stale,
    Modified,
    Unmanaged,
    Missing,
    Symlink,
    DanglingSymlink,
}

impl GlobalTargetStatus {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Stale => "out of sync",
            Self::Modified => "changed on disk",
            Self::Unmanaged => "existing · not managed by mdmanager.ai",
            Self::Missing => "not found",
            Self::Symlink => "symlink",
            Self::DanglingSymlink => "dangling symlink",
        }
    }

    pub(crate) const fn needs_force(self) -> bool {
        matches!(
            self,
            Self::Modified | Self::Unmanaged | Self::Symlink | Self::DanglingSymlink
        )
    }
}

#[derive(Clone, Debug)]
pub(crate) struct TargetView {
    pub(crate) id: String,
    pub(crate) path: PathBuf,
    pub(crate) expected: String,
    pub(crate) status: GlobalTargetStatus,
    pub(crate) diff: String,
    fingerprint: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct ApplyReport {
    pub(crate) id: String,
    pub(crate) previous: GlobalTargetStatus,
    pub(crate) backup: Option<PathBuf>,
}

pub(crate) fn active_profile(global: &GlobalConfig) -> Result<Option<String>, String> {
    let profile = recorded_active_profile(global)?;
    if let Some(profile) = &profile
        && !global.manifest.profiles.contains_key(profile)
    {
        return Err(format!(
            "active Profile `{profile}` no longer exists in {}; run `mdmanager doctor`",
            global.paths.config.display()
        ));
    }
    Ok(profile)
}

pub(crate) fn recorded_active_profile(global: &GlobalConfig) -> Result<Option<String>, String> {
    Ok(load_state(global)?.active_profile)
}

pub(crate) fn matching_profiles_on_disk(global: &GlobalConfig) -> Result<Vec<String>, String> {
    let mut matching = Vec::new();
    for profile in global.profile_names() {
        if matching_targets_on_disk(global, profile)?.len()
            == global.profile_target_names(profile)?.count()
        {
            matching.push(profile.to_owned());
        }
    }
    Ok(matching)
}

pub(crate) fn matching_targets_on_disk(
    global: &GlobalConfig,
    profile: &str,
) -> Result<Vec<String>, String> {
    let mut matching = Vec::new();
    for target in global.profile_target_names(profile)? {
        let path = global.target_path(target)?;
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("cannot inspect {}: {error}", path.display())),
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            continue;
        }
        let bytes = fs::read(&path)
            .map_err(|error| format!("cannot read deployed target {}: {error}", path.display()))?;
        if bytes == global.render(profile, target)?.as_bytes() {
            matching.push(target.to_owned());
        }
    }
    Ok(matching)
}

pub(crate) fn repair_state(global: &GlobalConfig, profile: &str) -> Result<(), String> {
    global.ensure_unchanged()?;
    if !global.manifest.profiles.contains_key(profile) {
        return Err(format!(
            "unknown profile {profile}; expected one of: {}",
            global.profile_names().collect::<Vec<_>>().join(", ")
        ));
    }
    let matching = matching_targets_on_disk(global, profile)?
        .into_iter()
        .collect::<HashSet<_>>();
    let mut state = State {
        active_profile: Some(profile.to_owned()),
        targets: IndexMap::new(),
    };
    for target in global
        .profile_target_names(profile)?
        .filter(|target| matching.contains(*target))
    {
        let path = global.target_path(target)?;
        let expected = global.render(profile, target)?;
        state.targets.insert(
            target.to_owned(),
            AppliedTarget {
                path: path.display().to_string(),
                hash: sha256_hex(expected.as_bytes()),
            },
        );
    }
    save_state(global, &state)
}

pub(crate) fn clear_state(global: &GlobalConfig) -> Result<(), String> {
    global.ensure_unchanged()?;
    save_state(global, &State::default())
}

pub(crate) fn inspect(global: &GlobalConfig, profile: &str) -> Result<Vec<TargetView>, String> {
    let state = load_state(global)?;
    global
        .profile_target_names(profile)?
        .map(|target| inspect_target(global, &state, profile, target))
        .collect()
}

pub(crate) fn inspect_one(
    global: &GlobalConfig,
    profile: &str,
    target: &str,
) -> Result<TargetView, String> {
    let state = load_state(global)?;
    inspect_target(global, &state, profile, target)
}

fn inspect_target(
    global: &GlobalConfig,
    state: &State,
    profile: &str,
    target: &str,
) -> Result<TargetView, String> {
    let expected = global.render(profile, target)?;
    let path = global.target_path(target)?;
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("cannot inspect {}: {error}", path.display())),
    };

    let (status, old, fingerprint, symlink) = if let Some(metadata) = metadata {
        if metadata.file_type().is_dir() {
            return Err(format!(
                "target path {} is a directory; inspect it and move it out of the target path before applying",
                path.display()
            ));
        }
        let (bytes, dangling) = match fs::read(&path) {
            Ok(bytes) => (bytes, false),
            Err(error)
                if metadata.file_type().is_symlink()
                    && matches!(
                        error.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::IsADirectory
                    ) =>
            {
                (vec![], error.kind() == std::io::ErrorKind::NotFound)
            }
            Err(error) => {
                return Err(format!(
                    "cannot read deployed target {}: {error}",
                    path.display()
                ));
            }
        };
        let target_hash = sha256_hex(&bytes);
        let symlink = if metadata.file_type().is_symlink() {
            let destination = fs::read_link(&path)
                .map_err(|error| format!("cannot inspect symlink {}: {error}", path.display()))?;
            Some((destination, dangling))
        } else {
            None
        };
        let fingerprint = if let Some((destination, _)) = &symlink {
            format!("symlink:{}:{target_hash}", destination.display())
        } else {
            format!("file:{target_hash}")
        };
        let old = String::from_utf8_lossy(&bytes).into_owned();
        let applied = state
            .targets
            .get(target)
            .filter(|applied| applied.path == path.display().to_string());
        let status = if let Some((_, dangling)) = &symlink {
            if *dangling {
                GlobalTargetStatus::DanglingSymlink
            } else {
                GlobalTargetStatus::Symlink
            }
        } else {
            match applied {
                None => GlobalTargetStatus::Unmanaged,
                Some(_) if bytes == expected.as_bytes() => GlobalTargetStatus::Current,
                Some(previous) if previous.hash == target_hash => GlobalTargetStatus::Stale,
                Some(_) => GlobalTargetStatus::Modified,
            }
        };
        (status, old, Some(fingerprint), symlink)
    } else {
        (GlobalTargetStatus::Missing, String::new(), None, None)
    };

    let old_label = if let Some((destination, dangling)) = symlink {
        format!(
            "{} -> {}{}",
            path.display(),
            destination.display(),
            if dangling { " (dangling)" } else { "" }
        )
    } else if status == GlobalTargetStatus::Missing {
        "/dev/null".to_owned()
    } else {
        path.display().to_string()
    };
    let diff = if status == GlobalTargetStatus::Current {
        "No changes.".into()
    } else {
        unified_diff(
            &old,
            &expected,
            &old_label,
            &format!("expected: {profile}/{target}"),
        )
    };
    Ok(TargetView {
        id: target.into(),
        path,
        expected,
        status,
        diff,
        fingerprint,
    })
}

pub(crate) fn apply(
    global: &GlobalConfig,
    profile: &str,
    selected: Option<&[String]>,
    force: bool,
    activate: bool,
) -> Result<Vec<ApplyReport>, String> {
    apply_inner(global, profile, selected, force, activate, None)
}

pub(crate) fn apply_reviewed(
    global: &GlobalConfig,
    profile: &str,
    reviewed: &[TargetView],
    activate: bool,
) -> Result<Vec<ApplyReport>, String> {
    let selected = reviewed
        .iter()
        .map(|view| view.id.clone())
        .collect::<Vec<_>>();
    apply_inner(
        global,
        profile,
        Some(&selected),
        true,
        activate,
        Some(reviewed),
    )
}

fn apply_inner(
    global: &GlobalConfig,
    profile: &str,
    selected: Option<&[String]>,
    force: bool,
    activate: bool,
    reviewed: Option<&[TargetView]>,
) -> Result<Vec<ApplyReport>, String> {
    global.ensure_unchanged()?;
    if !global.manifest.profiles.contains_key(profile) {
        return Err(format!(
            "unknown profile {profile}; expected one of: {}",
            global.profile_names().collect::<Vec<_>>().join(", ")
        ));
    }
    let deployed = global
        .profile_target_names(profile)?
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let target_ids = selected.map_or_else(|| deployed.clone(), <[String]>::to_vec);
    if target_ids.is_empty() {
        return Err("no targets selected".into());
    }
    if target_ids.iter().collect::<HashSet<_>>().len() != target_ids.len() {
        return Err("a target was selected more than once".into());
    }

    let mut views = Vec::new();
    for target in &target_ids {
        if !global.manifest.targets.contains_key(target) {
            return Err(format!(
                "unknown target {target}; expected one of: {}",
                global.target_names().collect::<Vec<_>>().join(", ")
            ));
        }
        views.push(inspect_one(global, profile, target)?);
    }
    if activate && target_ids.len() != deployed.len() {
        return Err("activating a profile requires applying every target in the profile".into());
    }
    if let Some(reviewed) = reviewed
        && (views.len() != reviewed.len()
            || views.iter().zip(reviewed).any(|(current, reviewed)| {
                current.id != reviewed.id
                    || current.path != reviewed.path
                    || current.expected != reviewed.expected
                    || current.status != reviewed.status
                    || current.fingerprint != reviewed.fingerprint
            }))
    {
        return Err("a target changed after review; inspect and confirm again".into());
    }
    if !force && let Some(view) = views.iter().find(|view| view.status.needs_force()) {
        return Err(format!(
            "{} is {}; inspect its diff and retry with --force",
            view.path.display(),
            view.status.label()
        ));
    }

    let mut reports = Vec::new();
    let mut state = load_state(global)?;
    state
        .targets
        .retain(|target, _| global.manifest.targets.contains_key(target));
    for view in &views {
        let backup = if view.status.needs_force() {
            Some(backup_target(global, &view.id, &view.path)?)
        } else {
            None
        };
        if view.status != GlobalTargetStatus::Current {
            atomic_write(&view.path, view.expected.as_bytes())?;
        }
        state.targets.insert(
            view.id.clone(),
            AppliedTarget {
                path: view.path.display().to_string(),
                hash: sha256_hex(view.expected.as_bytes()),
            },
        );
        save_state(global, &state)?;
        reports.push(ApplyReport {
            id: view.id.clone(),
            previous: view.status,
            backup,
        });
    }

    if activate {
        state.targets.retain(|target, _| deployed.contains(target));
        state.active_profile = Some(profile.into());
        save_state(global, &state)?;
    }
    Ok(reports)
}

fn state_path(global: &GlobalConfig) -> PathBuf {
    global.paths.state_dir.join("state.toml")
}

fn load_state(global: &GlobalConfig) -> Result<State, String> {
    let path = state_path(global);
    match fs::read_to_string(&path) {
        Ok(source) => {
            toml::from_str(&source).map_err(|error| format!("invalid {}: {error}", path.display()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
        Err(error) => Err(format!("cannot read {}: {error}", path.display())),
    }
}

fn save_state(global: &GlobalConfig, state: &State) -> Result<(), String> {
    let source = toml::to_string_pretty(state)
        .map_err(|error| format!("cannot serialize state: {error}"))?;
    atomic_write(&state_path(global), source.as_bytes())
}

fn backup_target(global: &GlobalConfig, id: &str, path: &Path) -> Result<PathBuf, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect {} for backup: {error}", path.display()))?;
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error)
            if metadata.file_type().is_symlink()
                && matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::IsADirectory
                ) =>
        {
            let destination = fs::read_link(path).map_err(|link_error| {
                format!(
                    "cannot read symlink {} for backup: {link_error}",
                    path.display()
                )
            })?;
            let label = if error.kind() == std::io::ErrorKind::NotFound {
                "dangling symlink"
            } else {
                "symlink"
            };
            format!("{label} -> {}\n", destination.display()).into_bytes()
        }
        Err(error) => {
            return Err(format!(
                "cannot read {} for backup: {error}",
                path.display()
            ));
        }
    };
    let directory = global.paths.data_dir.join("backups");
    fs::create_dir_all(&directory)
        .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
    for number in 0_u64.. {
        let name = if number == 0 {
            format!("{id}.bak")
        } else {
            format!("{id}.{number}.bak")
        };
        let backup = directory.join(name);
        match fs::symlink_metadata(&backup) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                atomic_write(&backup, &bytes)?;
                return Ok(backup);
            }
            Err(error) => {
                return Err(format!("cannot inspect {}: {error}", backup.display()));
            }
        }
    }
    unreachable!()
}

pub(crate) fn unified_diff(old: &str, new: &str, old_label: &str, new_label: &str) -> String {
    TextDiff::from_lines(old, new)
        .unified_diff()
        .context_radius(3)
        .header(old_label, new_label)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Paths;
    use tempfile::TempDir;

    fn fixture() -> (TempDir, Paths, GlobalConfig) {
        let temp = TempDir::new().unwrap();
        let paths = Paths::for_home(temp.path());
        let root = paths.config.parent().unwrap();
        fs::create_dir_all(root.join("sections")).unwrap();
        fs::write(root.join("sections/common.md"), "### Common\n\nFirst\n").unwrap();
        fs::write(
            &paths.config,
            r#"
[[sections]]
id = "common"
name = "Common"
path = "sections/common.md"

[targets.claude]
path = "~/.claude/CLAUDE.md"
title = "Global Claude"

[profiles.default]
claude = ["common"]
"#,
        )
        .unwrap();
        let global = GlobalConfig::load(&paths).unwrap();
        (temp, paths, global)
    }

    #[test]
    fn distinguishes_stale_and_modified_targets() {
        let (_temp, paths, global) = fixture();
        assert_eq!(
            inspect(&global, "default").unwrap()[0].status,
            GlobalTargetStatus::Missing
        );
        apply(&global, "default", None, false, true).unwrap();
        assert_eq!(
            inspect(&global, "default").unwrap()[0].status,
            GlobalTargetStatus::Current
        );

        let section = paths.config.parent().unwrap().join("sections/common.md");
        fs::write(&section, "### Common\n\nSecond\n").unwrap();
        let changed = GlobalConfig::load(&paths).unwrap();
        assert_eq!(
            inspect(&changed, "default").unwrap()[0].status,
            GlobalTargetStatus::Stale
        );

        fs::write(changed.target_path("claude").unwrap(), "manual\n").unwrap();
        assert_eq!(
            inspect(&changed, "default").unwrap()[0].status,
            GlobalTargetStatus::Modified
        );
    }

    #[test]
    fn modified_target_requires_force_and_is_backed_up() {
        let (_temp, _paths, global) = fixture();
        let target = global.target_path("claude").unwrap();
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, "manual\n").unwrap();
        assert!(
            apply(&global, "default", None, false, true)
                .unwrap_err()
                .contains("--force")
        );
        let report = apply(&global, "default", None, true, true).unwrap();
        let backup = report[0].backup.as_ref().unwrap();
        assert_eq!(fs::read_to_string(backup).unwrap(), "manual\n");
        assert_eq!(
            fs::read_to_string(target).unwrap(),
            global.render("default", "claude").unwrap()
        );
    }

    #[test]
    fn repeated_forced_applies_keep_every_backup() {
        let (_temp, paths, global) = fixture();
        let target = global.target_path("claude").unwrap();
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, "first manual edit\n").unwrap();
        let first = apply(&global, "default", None, true, true).unwrap();

        fs::write(&target, "second manual edit\n").unwrap();
        let second = apply(&global, "default", None, true, true).unwrap();

        assert_ne!(first[0].backup, second[0].backup);
        assert_eq!(
            fs::read_to_string(first[0].backup.as_ref().unwrap()).unwrap(),
            "first manual edit\n"
        );
        assert_eq!(
            fs::read_to_string(second[0].backup.as_ref().unwrap()).unwrap(),
            "second manual edit\n"
        );
        assert!(paths.data_dir.join("backups/claude.bak").exists());
        assert!(paths.data_dir.join("backups/claude.1.bak").exists());
    }

    #[test]
    fn changing_a_target_path_makes_the_new_file_unmanaged() {
        let (_temp, paths, global) = fixture();
        apply(&global, "default", None, false, true).unwrap();
        let source = fs::read_to_string(&paths.config)
            .unwrap()
            .replace("~/.claude/CLAUDE.md", "~/.claude/AGENTS.md");
        fs::write(&paths.config, source).unwrap();
        let changed = GlobalConfig::load(&paths).unwrap();
        let target = changed.target_path("claude").unwrap();
        fs::write(&target, changed.render("default", "claude").unwrap()).unwrap();
        assert_eq!(
            inspect(&changed, "default").unwrap()[0].status,
            GlobalTargetStatus::Unmanaged
        );
    }

    #[cfg(unix)]
    #[test]
    fn forced_apply_replaces_a_migration_symlink() {
        use std::os::unix::fs::symlink;

        let (_temp, paths, global) = fixture();
        let wrapper = paths.config.parent().unwrap().join("wrapper.md");
        fs::write(&wrapper, "old wrapper\n").unwrap();
        let target = global.target_path("claude").unwrap();
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        symlink(&wrapper, &target).unwrap();

        let view = inspect(&global, "default").unwrap().remove(0);
        assert_eq!(view.status, GlobalTargetStatus::Symlink);
        assert!(view.diff.contains(&format!("-> {}", wrapper.display())));
        apply(&global, "default", None, true, true).unwrap();
        assert!(
            !fs::symlink_metadata(&target)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::read_to_string(target).unwrap(),
            global.render("default", "claude").unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn forced_apply_replaces_and_records_a_dangling_symlink() {
        use std::os::unix::fs::symlink;

        let (_temp, _paths, global) = fixture();
        let target = global.target_path("claude").unwrap();
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        symlink("missing-wrapper.md", &target).unwrap();

        let view = inspect(&global, "default").unwrap().remove(0);
        assert_eq!(view.status, GlobalTargetStatus::DanglingSymlink);
        assert!(view.diff.contains("-> missing-wrapper.md (dangling)"));

        let report = apply(&global, "default", None, true, true).unwrap();
        let backup = report[0].backup.as_ref().unwrap();
        assert_eq!(
            fs::read_to_string(backup).unwrap(),
            "dangling symlink -> missing-wrapper.md\n"
        );
        assert!(
            !fs::symlink_metadata(target)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[cfg(unix)]
    #[test]
    fn forced_apply_replaces_and_records_a_directory_symlink() {
        use std::os::unix::fs::symlink;

        let (_temp, _paths, global) = fixture();
        let target = global.target_path("claude").unwrap();
        fs::create_dir_all(target.parent().unwrap().join("old-tree")).unwrap();
        symlink("old-tree", &target).unwrap();

        let report = apply(&global, "default", None, true, true).unwrap();
        let backup = report[0].backup.as_ref().unwrap();
        assert_eq!(fs::read_to_string(backup).unwrap(), "symlink -> old-tree\n");
        assert!(fs::symlink_metadata(target).unwrap().file_type().is_file());
    }

    #[test]
    fn apply_refuses_source_changes_made_after_review() {
        let (_temp, paths, global) = fixture();
        fs::write(
            paths.config.parent().unwrap().join("sections/common.md"),
            "changed after load\n",
        )
        .unwrap();
        let error = apply(&global, "default", None, false, true).unwrap_err();
        assert!(error.contains("source configuration changed on disk"));
        assert!(!global.target_path("claude").unwrap().exists());
    }

    #[test]
    fn apply_rejects_duplicate_target_selection() {
        let (_temp, _paths, global) = fixture();
        let targets = vec!["claude".to_owned(), "claude".to_owned()];
        assert!(
            apply(&global, "default", Some(&targets), false, false)
                .unwrap_err()
                .contains("more than once")
        );
    }

    #[test]
    fn reviewed_apply_refuses_a_target_changed_after_review() {
        let (_temp, _paths, global) = fixture();
        let target = global.target_path("claude").unwrap();
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, "before review\n").unwrap();
        let reviewed = inspect(&global, "default").unwrap();
        fs::write(&target, "after review\n").unwrap();

        let error = apply_reviewed(&global, "default", &reviewed, true).unwrap_err();
        assert!(error.contains("changed after review"));
        assert_eq!(fs::read_to_string(target).unwrap(), "after review\n");
    }

    #[test]
    fn directory_target_has_a_directed_error() {
        let (_temp, _paths, global) = fixture();
        fs::create_dir_all(global.target_path("claude").unwrap()).unwrap();
        assert!(
            inspect(&global, "default")
                .unwrap_err()
                .contains("is a directory")
        );
    }

    #[cfg(unix)]
    #[test]
    fn apply_preserves_existing_target_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let (_temp, _paths, global) = fixture();
        let target = global.target_path("claude").unwrap();
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, "manual\n").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o640)).unwrap();

        apply(&global, "default", None, true, true).unwrap();
        assert_eq!(
            fs::metadata(target).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }

    #[test]
    fn apply_prunes_state_for_removed_targets() {
        let (_temp, paths, global) = fixture();
        apply(&global, "default", None, false, true).unwrap();
        let source = global
            .source
            .replace("targets.claude", "targets.codex")
            .replace("~/.claude/CLAUDE.md", "~/.codex/AGENTS.md")
            .replace("Global Claude", "Global Codex")
            .replace("claude =", "codex =");
        fs::write(&paths.config, source).unwrap();
        let changed = GlobalConfig::load(&paths).unwrap();

        apply(&changed, "default", None, false, true).unwrap();

        let state = load_state(&changed).unwrap();
        assert_eq!(
            state.targets.keys().map(String::as_str).collect::<Vec<_>>(),
            ["codex"]
        );
    }

    fn subset_fixture() -> (TempDir, Paths, GlobalConfig) {
        let temp = TempDir::new().unwrap();
        let paths = Paths::for_home(temp.path());
        let root = paths.config.parent().unwrap();
        fs::create_dir_all(root.join("sections")).unwrap();
        fs::write(root.join("sections/common.md"), "### Common\n\nFirst\n").unwrap();
        fs::write(
            &paths.config,
            r#"
[[sections]]
id = "common"
name = "Common"
path = "sections/common.md"

[targets.pi]
path = "~/.pi/agent/AGENTS.md"
title = "Global Pi"

[targets.claude]
path = "~/.claude/CLAUDE.md"
title = "Global Claude"

[profiles.default]
pi = ["common"]
claude = ["common"]

[profiles.work]
claude = ["common"]
"#,
        )
        .unwrap();
        let global = GlobalConfig::load(&paths).unwrap();
        (temp, paths, global)
    }

    #[test]
    fn apply_does_not_create_an_omitted_pi_target() {
        let (_temp, _paths, global) = subset_fixture();
        apply(&global, "work", None, false, true).unwrap();
        assert!(global.target_path("claude").unwrap().exists());
        assert!(!global.target_path("pi").unwrap().exists());
        assert!(apply(&global, "work", Some(&["pi".to_owned()]), false, false).is_err());
        assert!(!global.target_path("pi").unwrap().exists());
        assert_eq!(
            inspect(&global, "work")
                .unwrap()
                .iter()
                .map(|view| view.id.as_str())
                .collect::<Vec<_>>(),
            ["claude"]
        );
    }

    #[test]
    fn activating_a_partial_profile_prunes_omitted_ownership() {
        let (_temp, _paths, global) = subset_fixture();
        apply(&global, "default", None, false, true).unwrap();
        let pi = global.target_path("pi").unwrap();
        let leftover = fs::read_to_string(&pi).unwrap();
        assert!(!leftover.is_empty());

        apply(&global, "work", None, false, true).unwrap();

        assert_eq!(fs::read_to_string(&pi).unwrap(), leftover);
        let state = load_state(&global).unwrap();
        assert_eq!(
            state.targets.keys().map(String::as_str).collect::<Vec<_>>(),
            ["claude"]
        );
        assert_eq!(state.active_profile.as_deref(), Some("work"));
    }

    #[test]
    fn matching_and_repair_use_the_profile_subset() {
        let (_temp, _paths, global) = subset_fixture();
        apply(&global, "default", None, false, true).unwrap();
        fs::write(global.target_path("pi").unwrap(), "manual pi\n").unwrap();

        assert_eq!(
            matching_targets_on_disk(&global, "work").unwrap(),
            ["claude"]
        );
        assert_eq!(matching_profiles_on_disk(&global).unwrap(), ["work"]);

        repair_state(&global, "work").unwrap();
        let state = load_state(&global).unwrap();
        assert_eq!(
            state.targets.keys().map(String::as_str).collect::<Vec<_>>(),
            ["claude"]
        );
        assert_eq!(
            fs::read_to_string(global.target_path("pi").unwrap()).unwrap(),
            "manual pi\n"
        );
    }
}
