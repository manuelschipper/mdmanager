use std::collections::HashSet;
use std::env;
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use indexmap::IndexMap;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use toml_edit::{DocumentMut, Item, Table, value};

use crate::section::{Section, validate_id, validate_relative_path};
#[derive(Clone, Debug)]
pub(crate) struct Paths {
    pub(crate) home: PathBuf,
    pub(crate) config: PathBuf,
    pub(crate) data_dir: PathBuf,
    pub(crate) state_dir: PathBuf,
}

impl Paths {
    pub(crate) fn discover() -> Result<Self, String> {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| "HOME is not set".to_owned())?;
        if !home.is_absolute() {
            return Err("HOME must be an absolute path".into());
        }
        let data_dir = home.join(".mdmanager");
        Ok(Self {
            config: data_dir.join("mdmanager.toml"),
            state_dir: data_dir.join("state"),
            data_dir,
            home,
        })
    }

    #[cfg(test)]
    pub(crate) fn for_home(home: &Path) -> Self {
        let data_dir = home.join(".mdmanager");
        Self {
            config: data_dir.join("mdmanager.toml"),
            state_dir: data_dir.join("state"),
            data_dir,
            home: home.to_owned(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Target {
    pub(crate) path: String,
    pub(crate) title: String,
}

/// Display name of a managed target id, such as `claude` -> `Claude`, shared by the Global,
/// Project, and Local CLI and TUI surfaces. A Global target's `title` is the rendered document
/// heading, not this label.
pub(crate) fn target_display_name(id: &str) -> String {
    let mut chars = id.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Ui {
    #[serde(default = "default_theme")]
    pub(crate) theme: String,
}

impl Default for Ui {
    fn default() -> Self {
        Self {
            theme: default_theme(),
        }
    }
}

fn default_theme() -> String {
    crate::theme::DEFAULT_NAME.into()
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    #[serde(default)]
    pub(crate) ui: Ui,
    #[serde(default)]
    pub(crate) sections: Vec<Section>,
    #[serde(default)]
    pub(crate) targets: IndexMap<String, Target>,
    #[serde(default)]
    pub(crate) profiles: IndexMap<String, IndexMap<String, Vec<String>>>,
}

#[derive(Clone, Debug)]
/// The loaded Global configuration from `~/.mdmanager/mdmanager.toml`, including Personal
/// Sections, Global Targets, Profiles, and the UI theme; the repository scope lives in
/// `project::Workspace`.
pub(crate) struct GlobalConfig {
    pub(crate) paths: Paths,
    pub(crate) source: String,
    pub(crate) manifest: Manifest,
    contents: IndexMap<String, String>,
}

impl GlobalConfig {
    pub(crate) fn load(paths: &Paths) -> Result<Self, String> {
        let source = match fs::read_to_string(&paths.config) {
            Ok(source) => source,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(format!(
                    "not configured\n\nExpected configuration: {}\nAsk your coding agent to run `mdmanager docs start` and help configure mdmanager.ai.",
                    paths.config.display()
                ));
            }
            Err(error) => {
                return Err(format!("cannot read {}: {error}", paths.config.display()));
            }
        };
        let manifest: Manifest = toml::from_str(&source)
            .map_err(|error| format!("invalid {}: {error}", paths.config.display()))?;
        validate_manifest(&manifest, paths)?;

        let contents = read_sections(&manifest, paths)?;

        Ok(Self {
            paths: paths.clone(),
            source,
            manifest,
            contents,
        })
    }

    pub(crate) fn theme(&self) -> &str {
        &self.manifest.ui.theme
    }

    pub(crate) fn profile_names(&self) -> impl Iterator<Item = &str> {
        self.manifest.profiles.keys().map(String::as_str)
    }

    pub(crate) fn target_names(&self) -> impl Iterator<Item = &str> {
        self.manifest.targets.keys().map(String::as_str)
    }

    /// Catalog target IDs this Profile deploys, in catalog order.
    pub(crate) fn profile_target_names<'a>(
        &'a self,
        profile: &str,
    ) -> Result<impl Iterator<Item = &'a str> + 'a, String> {
        let deployed = self.profile_targets(profile)?;
        Ok(self
            .manifest
            .targets
            .keys()
            .filter(|id| deployed.contains_key(*id))
            .map(String::as_str))
    }

    pub(crate) fn composition(&self, profile: &str, target: &str) -> Result<&[String], String> {
        let targets = self.profile_targets(profile)?;
        if !self.manifest.targets.contains_key(target) {
            return Err(format!(
                "unknown target {target}; expected one of: {}",
                self.target_names().collect::<Vec<_>>().join(", ")
            ));
        }
        targets
            .get(target)
            .map(Vec::as_slice)
            .ok_or_else(|| format!("profile {profile} has no {target} composition"))
    }

    fn profile_targets(&self, profile: &str) -> Result<&IndexMap<String, Vec<String>>, String> {
        self.manifest.profiles.get(profile).ok_or_else(|| {
            format!(
                "unknown profile {profile}; expected one of: {}",
                self.profile_names().collect::<Vec<_>>().join(", ")
            )
        })
    }

    pub(crate) fn section(&self, id: &str) -> Option<&Section> {
        self.manifest
            .sections
            .iter()
            .find(|section| section.id == id)
    }

    pub(crate) fn section_content(&self, id: &str) -> Option<&str> {
        self.contents.get(id).map(String::as_str)
    }

    pub(crate) fn section_path(&self, id: &str) -> Option<PathBuf> {
        let root = self.paths.config.parent()?;
        self.section(id).map(|section| root.join(&section.path))
    }

    pub(crate) fn target_path(&self, id: &str) -> Result<PathBuf, String> {
        let target = self.manifest.targets.get(id).ok_or_else(|| {
            format!(
                "unknown target {id}; expected one of: {}",
                self.target_names().collect::<Vec<_>>().join(", ")
            )
        })?;
        expand_target_path(&target.path, &self.paths.home)
    }

    pub(crate) fn render(&self, profile: &str, target: &str) -> Result<String, String> {
        let ids = self.composition(profile, target)?;
        self.render_sections(target, ids)
    }

    pub(crate) fn ensure_unchanged(&self) -> Result<(), String> {
        let current = Self::load(&self.paths)?;
        if current.source != self.source || current.contents != self.contents {
            return Err("source configuration changed on disk; reload and review it".into());
        }
        Ok(())
    }

    pub(crate) fn render_sections(&self, target: &str, ids: &[String]) -> Result<String, String> {
        let target_config = self.manifest.targets.get(target).ok_or_else(|| {
            format!(
                "unknown target {target}; expected one of: {}",
                self.target_names().collect::<Vec<_>>().join(", ")
            )
        })?;
        let mut output = format!("# {}\n", target_config.title.trim());
        for id in ids {
            let content = self
                .contents
                .get(id)
                .ok_or_else(|| format!("section {id} has no loaded content"))?;
            output.push('\n');
            output.push_str(content.trim_end_matches(['\r', '\n']));
            output.push('\n');
        }
        Ok(output)
    }

    pub(crate) fn render_personal_sections(&self, ids: &[String]) -> Result<String, String> {
        let contents = ids
            .iter()
            .map(|id| {
                self.contents
                    .get(id)
                    .map(String::as_str)
                    .ok_or_else(|| format!("unknown personal Section {id}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(crate::section::render_format_1(&contents))
    }
}

fn validate_manifest(manifest: &Manifest, paths: &Paths) -> Result<(), String> {
    if manifest.sections.is_empty() {
        return Err("manifest must define at least one section".into());
    }
    if manifest.targets.is_empty() != manifest.profiles.is_empty() {
        return Err("manifest must define both targets and profiles, or neither".into());
    }

    let mut section_ids = HashSet::new();
    for section in &manifest.sections {
        validate_id("section", &section.id)?;
        if !section_ids.insert(section.id.as_str()) {
            return Err(format!("duplicate section id {}", section.id));
        }
        if section.name.trim().is_empty() {
            return Err(format!("section {} has an empty name", section.id));
        }
        validate_relative_path("section", &section.path)?;
    }

    let mut output_paths = HashSet::new();
    for (id, target) in &manifest.targets {
        validate_id("target", id)?;
        if target.title.trim().is_empty() {
            return Err(format!("target {id} has an empty title"));
        }
        let output = expand_target_path(&target.path, &paths.home)?;
        if !matches!(
            output.file_name().and_then(|name| name.to_str()),
            Some("AGENTS.md" | "CLAUDE.md")
        ) {
            return Err(format!(
                "target {id} path must end with AGENTS.md or CLAUDE.md"
            ));
        }
        if !output_paths.insert(output) {
            return Err(format!("multiple targets use path {}", target.path));
        }
    }

    for (profile_id, targets) in &manifest.profiles {
        validate_id("profile", profile_id)?;
        if targets.is_empty() {
            return Err(format!(
                "profile {profile_id} must contain at least one target"
            ));
        }
        for (target_id, ids) in targets {
            if !manifest.targets.contains_key(target_id) {
                return Err(format!(
                    "profile {profile_id} refers to unknown target {target_id}"
                ));
            }
            if ids.is_empty() {
                return Err(format!(
                    "profile {profile_id}/{target_id} must contain at least one section"
                ));
            }
            let mut seen = HashSet::new();
            for id in ids {
                if !section_ids.contains(id.as_str()) {
                    return Err(format!(
                        "profile {profile_id}/{target_id} refers to unknown section {id}"
                    ));
                }
                if !seen.insert(id) {
                    return Err(format!(
                        "profile {profile_id}/{target_id} contains section {id} more than once"
                    ));
                }
            }
        }
    }
    Ok(())
}

fn read_sections(manifest: &Manifest, paths: &Paths) -> Result<IndexMap<String, String>, String> {
    let root = paths
        .config
        .parent()
        .ok_or_else(|| "configuration path has no parent directory".to_owned())?;
    let mut contents = IndexMap::new();
    for section in &manifest.sections {
        let path = root.join(&section.path);
        let content = fs::read_to_string(&path)
            .map_err(|error| format!("cannot read section {}: {error}", path.display()))?;
        let content = content.replace("\r\n", "\n");
        if content.trim().is_empty() {
            return Err(format!("section {} is empty", path.display()));
        }
        contents.insert(section.id.clone(), content);
    }
    Ok(contents)
}

fn expand_target_path(raw: &str, home: &Path) -> Result<PathBuf, String> {
    let path = if let Some(relative) = raw.strip_prefix("~/") {
        validate_relative_path("target", relative)?;
        home.join(relative)
    } else {
        let path = PathBuf::from(raw);
        if !path.is_absolute()
            || path
                .components()
                .any(|component| matches!(component, Component::ParentDir))
        {
            return Err(format!(
                "target path must be absolute or start with ~/: {raw}"
            ));
        }
        path
    };
    if path == home || !path.starts_with(home) {
        return Err(format!("target path must be beneath HOME: {raw}"));
    }
    Ok(path)
}

/// Replacement atomic file writes replace the destination, preserving ordinary-file permissions (Unix: 0644 for new files).
/// Callers must establish ownership and validate symlinks. Syncs the file, not the parent directory.
pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    let mut temporary = NamedTempFile::new_in(parent).map_err(|error| {
        format!(
            "cannot create temporary file in {}: {error}",
            parent.display()
        )
    })?;
    temporary
        .write_all(bytes)
        .map_err(|error| format!("cannot write temporary file: {error}"))?;
    set_replacement_permissions(path, temporary.as_file())?;
    temporary
        .as_file_mut()
        .sync_all()
        .map_err(|error| format!("cannot sync temporary file: {error}"))?;
    #[cfg(test)]
    write_hook::before_persist(path)
        .map_err(|error| format!("cannot replace {}: {error}", path.display()))?;
    temporary
        .persist(path)
        .map_err(|error| format!("cannot replace {}: {}", path.display(), error.error))?;
    Ok(())
}

pub(crate) fn set_theme(paths: &Paths, theme: &str) -> Result<(), String> {
    crate::theme::resolve(theme)?;
    let source = fs::read_to_string(&paths.config)
        .map_err(|error| format!("cannot read {}: {error}", paths.config.display()))?;
    let manifest: Manifest = toml::from_str(&source)
        .map_err(|error| format!("invalid {}: {error}", paths.config.display()))?;
    validate_manifest(&manifest, paths)?;
    let mut document = source
        .parse::<DocumentMut>()
        .map_err(|error| format!("invalid {}: {error}", paths.config.display()))?;
    let ui = document
        .entry("ui")
        .or_insert_with(|| Item::Table(Table::new()));
    let ui = ui
        .as_table_mut()
        .ok_or_else(|| "ui configuration must be a table".to_owned())?;
    let decor = ui
        .get("theme")
        .and_then(Item::as_value)
        .map(|value| value.decor().clone());
    let mut selected = value(theme);
    if let (Some(decor), Some(value)) = (decor, selected.as_value_mut()) {
        *value.decor_mut() = decor;
    }
    ui["theme"] = selected;
    atomic_write(&paths.config, document.to_string().as_bytes())
}

/// No-clobber atomic file writes fail if the destination exists; new files use 0644 on Unix.
/// Callers own ownership and symlink validation. Syncs the file, not the parent directory.
pub(crate) fn atomic_create(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temporary = prepare_atomic_create(path, bytes)
        .map_err(|error| format!("cannot prepare {}: {error}", path.display()))?;
    persist_atomic_create(temporary, path)
        .map_err(|error| format!("cannot create {}: {}", path.display(), error.error))?;
    Ok(())
}

/// Prepare and sync create-only bytes in the destination directory; callers must use
/// persist_noclobber, retaining its typed I/O error when reserving numbered backups.
pub(crate) fn prepare_atomic_create(path: &Path, bytes: &[u8]) -> std::io::Result<NamedTempFile> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "file has no parent directory",
        )
    })?;
    fs::create_dir_all(parent)?;
    let mut temporary = NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    set_created_permissions(temporary.as_file())?;
    temporary.as_file_mut().sync_all()?;
    Ok(temporary)
}

/// Commit create-only bytes, returning the created file or the temporary file and typed
/// persistence error so backup reservations can retry only AlreadyExists collisions.
pub(crate) fn persist_atomic_create(
    temporary: NamedTempFile,
    path: &Path,
) -> Result<fs::File, tempfile::PersistError> {
    #[cfg(test)]
    if let Err(error) = write_hook::before_persist(path) {
        return Err(tempfile::PersistError {
            error,
            file: temporary,
        });
    }
    temporary.persist_noclobber(path)
}

/// Lowercase hex SHA-256 digest of `bytes`. This is the ownership hash stored in the Global
/// deployment state (`state.toml`) and the Local `overlays.toml`, and the directory key derived
/// from Git paths under the data and state directories; the encoding must stay byte-identical so
/// hashes already recorded on disk keep verifying.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        write!(output, "{byte:02x}").expect("writing to a String cannot fail");
    }
    output
}

fn set_replacement_permissions(path: &Path, file: &fs::File) -> Result<(), String> {
    if let Ok(metadata) = fs::symlink_metadata(path)
        && metadata.file_type().is_file()
    {
        file.set_permissions(metadata.permissions())
            .map_err(|error| {
                format!(
                    "cannot preserve permissions for {}: {error}",
                    path.display()
                )
            })?;
    } else {
        set_created_permissions(file)
            .map_err(|error| format!("cannot set created file permissions: {error}"))?;
    }
    Ok(())
}

fn set_created_permissions(file: &fs::File) -> std::io::Result<()> {
    #[cfg(unix)]
    file.set_permissions(fs::Permissions::from_mode(0o644))?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod write_hook {
    use std::cell::RefCell;
    use std::io;
    use std::path::{Path, PathBuf};

    struct Hook {
        path: PathBuf,
        skip: usize,
        action: Box<dyn FnOnce() -> io::Result<()>>,
    }

    thread_local! {
        static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) };
    }

    pub(crate) struct Guard;

    impl Drop for Guard {
        fn drop(&mut self) {
            HOOK.with(|hook| hook.borrow_mut().take());
        }
    }

    // Scoped to the calling test thread and removed before invocation so a second
    // reservation can finish while the first is paused immediately before persistence.
    pub(crate) fn install(
        path: &Path,
        skip: usize,
        action: impl FnOnce() -> io::Result<()> + 'static,
    ) -> Guard {
        HOOK.with(|hook| {
            assert!(hook.borrow().is_none());
            *hook.borrow_mut() = Some(Hook {
                path: path.to_owned(),
                skip,
                action: Box::new(action),
            });
        });
        Guard
    }

    pub(crate) fn before_persist(path: &Path) -> io::Result<()> {
        let action = HOOK.with(|hook| {
            let mut hook = hook.borrow_mut();
            let current = hook.as_mut()?;
            if current.path != path {
                return None;
            }
            if current.skip > 0 {
                current.skip -= 1;
                return None;
            }
            hook.take().map(|hook| hook.action)
        });
        match action {
            Some(action) => action(),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn sha256_hex_is_the_lowercase_hex_digest() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    fn global(source: &str, sections: &[(&str, &str)]) -> (TempDir, Result<GlobalConfig, String>) {
        let temp = TempDir::new().unwrap();
        let paths = Paths::for_home(temp.path());
        fs::create_dir_all(paths.config.parent().unwrap()).unwrap();
        for (path, content) in sections {
            let path = paths.config.parent().unwrap().join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        }
        fs::write(&paths.config, source).unwrap();
        let loaded = GlobalConfig::load(&paths);
        (temp, loaded)
    }

    const VALID: &str = r#"
[[sections]]
id = "common"
name = "Common"
path = "sections/common.md"

[targets.claude]
path = "~/.claude/CLAUDE.md"
title = "Global Claude"

[profiles.default]
claude = ["common"]
"#;

    #[test]
    fn loaders_reject_paths_before_touching_outputs_or_ownership() {
        let (temp, loaded) = global(VALID, &[("sections/common.md", "private source")]);
        let paths = loaded.unwrap().paths;
        let target = temp.path().join(".claude/CLAUDE.md");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, "target sentinel").unwrap();
        fs::create_dir_all(&paths.state_dir).unwrap();
        let state = paths.state_dir.join("state.toml");
        fs::write(&state, "state sentinel").unwrap();
        let outside = TempDir::new().unwrap();
        let outside_target = outside.path().join("CLAUDE.md");
        fs::write(&outside_target, "outside sentinel").unwrap();
        for destination in [
            ".claude/CLAUDE.md".to_owned(),
            outside_target.display().to_string(),
            paths.home.display().to_string(),
            format!("{}/.claude/../CLAUDE.md", paths.home.display()),
            "~/../CLAUDE.md".to_owned(),
        ] {
            fs::write(
                &paths.config,
                VALID.replace("~/.claude/CLAUDE.md", &destination),
            )
            .unwrap();
            assert!(GlobalConfig::load(&paths).is_err(), "{destination}");
            assert_eq!(fs::read(&target).unwrap(), b"target sentinel");
            assert_eq!(fs::read(&state).unwrap(), b"state sentinel");
            assert_eq!(fs::read(&outside_target).unwrap(), b"outside sentinel");
        }
        fs::write(
            &paths.config,
            VALID.replace("~/.claude/CLAUDE.md", target.to_str().unwrap()),
        )
        .unwrap();
        assert_eq!(
            GlobalConfig::load(&paths)
                .unwrap()
                .target_path("claude")
                .unwrap(),
            target
        );

        let project_manifest = paths.data_dir.join("project.toml");
        let project_target = paths.home.join("AGENTS.md");
        fs::write(&project_target, "project sentinel").unwrap();
        let project_source = "format = 1\n[[sections]]\nid = \"common\"\nname = \"Common\"\npath = \"sections/common.md\"\n[targets.agents]\nsections = [\"common\"]\n";
        for section in [
            outside_target.display().to_string(),
            "../CLAUDE.md".to_owned(),
            "sections/../../CLAUDE.md".to_owned(),
        ] {
            fs::write(&paths.config, VALID.replace("sections/common.md", &section)).unwrap();
            fs::write(
                &project_manifest,
                project_source.replace("sections/common.md", &section),
            )
            .unwrap();
            let global_error = GlobalConfig::load(&paths).unwrap_err();
            let project_error = crate::project::Workspace::load(&project_manifest).unwrap_err();
            assert!(global_error.contains("relative path"), "{global_error}");
            assert!(project_error.contains("relative path"), "{project_error}");
            assert_eq!(fs::read(&target).unwrap(), b"target sentinel");
            assert_eq!(fs::read(&project_target).unwrap(), b"project sentinel");
            assert_eq!(fs::read(&state).unwrap(), b"state sentinel");
            assert_eq!(fs::read(&outside_target).unwrap(), b"outside sentinel");
        }
        fs::write(&project_manifest, project_source).unwrap();
        assert!(crate::project::Workspace::load(&project_manifest).is_ok());
    }

    #[test]
    fn atomic_create_preserves_occupied_entries() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("output");
        fs::write(&path, "sentinel").unwrap();
        assert!(atomic_create(&path, b"replacement").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"sentinel");

        #[cfg(unix)]
        for resolving in [true, false] {
            use std::os::unix::fs::{MetadataExt, symlink};
            fs::remove_file(&path).unwrap();
            let referent = temp.path().join("referent");
            if resolving {
                fs::write(&referent, "referent sentinel").unwrap();
            } else {
                fs::remove_file(&referent).unwrap();
            }
            symlink(&referent, &path).unwrap();
            let before = fs::symlink_metadata(&path).unwrap();
            assert!(atomic_create(&path, b"replacement").is_err());
            assert_eq!(fs::read_link(&path).unwrap(), referent);
            let after = fs::symlink_metadata(&path).unwrap();
            assert_eq!((before.dev(), before.ino()), (after.dev(), after.ino()));
            if resolving {
                assert_eq!(fs::read(&referent).unwrap(), b"referent sentinel");
            } else {
                assert!(!referent.exists());
            }
        }
    }

    #[test]
    fn renders_normalized_document() {
        let (_temp, global) = global(
            VALID,
            &[("sections/common.md", "### Common\r\n\r\nText\r\n")],
        );
        assert_eq!(
            global.unwrap().render("default", "claude").unwrap(),
            "# Global Claude\n\n### Common\n\nText\n"
        );
    }

    #[test]
    fn set_theme_adds_ui_without_reformatting_the_manifest() {
        let (temp, global) = global(
            &format!("# Personal instructions\n{VALID}"),
            &[("sections/common.md", "### Common\n")],
        );
        let paths = global.unwrap().paths;

        set_theme(&paths, "nord").unwrap();

        let source = fs::read_to_string(&paths.config).unwrap();
        assert!(source.starts_with("# Personal instructions\n"));
        assert!(source.contains("[ui]\ntheme = \"nord\""));
        assert_eq!(GlobalConfig::load(&paths).unwrap().theme(), "nord");
        drop(temp);
    }

    #[test]
    fn set_theme_replaces_the_value_and_preserves_its_comment() {
        let source = format!("[ui]\ntheme = \"gruvbox-dark\" # preferred\n{VALID}");
        let (_temp, global) = global(&source, &[("sections/common.md", "### Common\n")]);
        let paths = global.unwrap().paths;

        set_theme(&paths, "nord").unwrap();

        let source = fs::read_to_string(&paths.config).unwrap();
        assert!(source.contains("theme = \"nord\" # preferred"));
    }

    #[test]
    fn personal_library_can_exist_without_global_targets() {
        let source = r#"
[[sections]]
id = "local"
name = "Local"
path = "sections/local.md"
"#;
        let (_temp, global) = global(source, &[("sections/local.md", "private\n")]);
        let global = global.unwrap();
        assert_eq!(global.theme(), crate::theme::DEFAULT_NAME);
        assert_eq!(global.profile_names().count(), 0);
        assert_eq!(
            global.render_personal_sections(&["local".into()]).unwrap(),
            "private\n"
        );
    }

    #[test]
    fn rejects_unknown_sections() {
        let source = VALID.replace("[\"common\"]", "[\"missing\"]");
        let (_temp, global) = global(&source, &[("sections/common.md", "text")]);
        assert!(global.unwrap_err().contains("unknown section missing"));
    }

    #[test]
    fn distinguishes_unknown_profiles_and_targets() {
        let (_temp, global) = global(VALID, &[("sections/common.md", "text")]);
        let global = global.unwrap();
        let profile = global.render("missing", "claude").unwrap_err();
        assert!(profile.contains("unknown profile missing"));
        assert!(profile.contains("default"));
        let target = global.render("default", "missing").unwrap_err();
        assert!(target.contains("unknown target missing"));
        assert!(target.contains("claude"));
    }

    #[test]
    fn preserves_manifest_target_order() {
        let source = VALID
            .replace(
                "[targets.claude]",
                "[targets.pi]\npath = \"~/.pi/agent/AGENTS.md\"\ntitle = \"Global Pi\"\n\n[targets.codex]\npath = \"~/.codex/AGENTS.md\"\ntitle = \"Global Codex\"\n\n[targets.claude]",
            )
            .replace(
                "claude = [\"common\"]",
                "pi = [\"common\"]\ncodex = [\"common\"]\nclaude = [\"common\"]",
            );
        let (_temp, global) = global(&source, &[("sections/common.md", "text")]);
        assert_eq!(
            global.unwrap().target_names().collect::<Vec<_>>(),
            ["pi", "codex", "claude"]
        );
    }

    #[test]
    fn a_profile_may_omit_catalog_targets() {
        let source = VALID
            .replace(
                "[targets.claude]",
                "[targets.pi]\npath = \"~/.pi/agent/AGENTS.md\"\ntitle = \"Global Pi\"\n\n[targets.codex]\npath = \"~/.codex/AGENTS.md\"\ntitle = \"Global Codex\"\n\n[targets.claude]",
            )
            .replace("claude = [\"common\"]", "claude = [\"common\"]\ncodex = [\"common\"]");
        let (_temp, global) = global(&source, &[("sections/common.md", "text")]);
        let global = global.unwrap();
        assert_eq!(
            global
                .profile_target_names("default")
                .unwrap()
                .collect::<Vec<_>>(),
            ["codex", "claude"]
        );
        assert!(global.render("default", "pi").is_err());
    }

    #[test]
    fn rejects_an_empty_profile() {
        let source = VALID.replace("claude = [\"common\"]", "");
        let (_temp, global) = global(&source, &[("sections/common.md", "text")]);
        assert!(global.is_err());
    }

    #[test]
    fn rejects_empty_section_files() {
        let (_temp, global) = global(VALID, &[("sections/common.md", " \n\t")]);
        assert!(global.unwrap_err().contains("is empty"));
    }

    #[test]
    fn rejects_targets_outside_the_supported_file_families() {
        let source = VALID.replace("~/.claude/CLAUDE.md", "~/.claude/rules/shared.md");
        let (_temp, global) = global(&source, &[("sections/common.md", "text")]);
        assert!(
            global
                .unwrap_err()
                .contains("must end with AGENTS.md or CLAUDE.md")
        );
    }

    #[test]
    fn rejects_an_empty_composition() {
        let source = VALID.replace("claude = [\"common\"]", "claude = []");
        let (_temp, global) = global(&source, &[("sections/common.md", "text")]);
        let error = global.unwrap_err();
        assert!(error.contains("must contain at least one section"));
    }

    #[cfg(unix)]
    #[test]
    fn atomic_creates_are_readable_by_other_users() {
        let temp = TempDir::new().unwrap();
        let created = temp.path().join("created.md");
        let written = temp.path().join("written.md");

        atomic_create(&created, b"created\n").unwrap();
        atomic_write(&written, b"written\n").unwrap();

        assert_eq!(
            fs::metadata(created).unwrap().permissions().mode() & 0o777,
            0o644
        );
        assert_eq!(
            fs::metadata(written).unwrap().permissions().mode() & 0o777,
            0o644
        );
    }
}
