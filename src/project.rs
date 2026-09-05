use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::process::Command;

use indexmap::IndexMap;
use serde::Deserialize;

use crate::config::{atomic_create, atomic_write, target_display_name};
use crate::deploy::unified_diff;
use crate::git_worktree::worktree_root;
use crate::section::{Section, render_format_1, validate_id, validate_relative_path};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Target {
    pub(crate) sections: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    pub(crate) format: u32,
    #[serde(default)]
    pub(crate) sections: Vec<Section>,
    #[serde(default)]
    pub(crate) targets: IndexMap<String, Target>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Project target status: how the root AGENTS.md or CLAUDE.md compares with the
/// committed Project composition. `label()` owns its displayed vocabulary.
pub(crate) enum ProjectTargetStatus {
    Current,
    OutOfSync,
    Missing,
}

impl ProjectTargetStatus {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::OutOfSync => "out of sync",
            Self::Missing => "not found",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct TargetView {
    pub(crate) id: String,
    pub(crate) path: PathBuf,
    pub(crate) expected: String,
    pub(crate) deployed: String,
    pub(crate) status: ProjectTargetStatus,
    pub(crate) diff: String,
}

#[derive(Clone, Debug)]
pub(crate) struct CreatePlan {
    pub(crate) section_path: PathBuf,
    pub(crate) target_path: PathBuf,
    pub(crate) content: String,
    manifest_path: PathBuf,
    reviewed_manifest: Option<String>,
    manifest_source: String,
}

impl CreatePlan {
    pub(crate) fn manifest_diff(&self) -> String {
        unified_diff(
            self.reviewed_manifest.as_deref().unwrap_or(""),
            &self.manifest_source,
            self.reviewed_manifest
                .as_ref()
                .map_or("/dev/null".to_owned(), |_| {
                    self.manifest_path.display().to_string()
                })
                .as_str(),
            &self.manifest_path.display().to_string(),
        )
    }

    pub(crate) fn apply(&self) -> Result<Workspace, String> {
        let current = match fs::read_to_string(&self.manifest_path) {
            Ok(source) => Some(source),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(format!(
                    "cannot read {}: {error}",
                    self.manifest_path.display()
                ));
            }
        };
        if current != self.reviewed_manifest {
            return Err("project manifest changed after review".into());
        }
        if self.target_path.exists() {
            return Err(format!(
                "refusing to overwrite {}",
                self.target_path.display()
            ));
        }
        if self.section_path.exists() {
            return Err(format!(
                "refusing to overwrite {}",
                self.section_path.display()
            ));
        }
        if self.reviewed_manifest.is_none() {
            fs::create_dir_all(
                self.section_path
                    .parent()
                    .ok_or_else(|| "project Section has no parent directory".to_owned())?,
            )
            .map_err(|error| format!("cannot create project Sections directory: {error}"))?;
        }
        atomic_create(&self.section_path, self.content.as_bytes())?;
        let result = if self.reviewed_manifest.is_some() {
            atomic_write(&self.manifest_path, self.manifest_source.as_bytes())
        } else {
            atomic_create(&self.manifest_path, self.manifest_source.as_bytes())
        };
        if let Err(error) = result {
            let _ = fs::remove_file(&self.section_path);
            return Err(error);
        }
        Workspace::load(&self.manifest_path)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Workspace {
    pub(crate) root: PathBuf,
    pub(crate) manifest_path: PathBuf,
    pub(crate) source: String,
    pub(crate) manifest: Manifest,
    contents: IndexMap<String, String>,
}

impl Workspace {
    pub(crate) fn discover(start: &Path) -> Result<Option<Self>, String> {
        let start = canonical_directory(start)?;
        let worktree = worktree_root(&start).ok();
        for ancestor in start.ancestors() {
            let manifest = ancestor.join(".mdmanager/project.toml");
            if manifest.is_file() {
                return Self::load(&manifest).map(Some);
            }
            if worktree.as_deref() == Some(ancestor) {
                break;
            }
        }
        Ok(None)
    }

    pub(crate) fn load(manifest_path: &Path) -> Result<Self, String> {
        let source = fs::read_to_string(manifest_path)
            .map_err(|error| format!("cannot read {}: {error}", manifest_path.display()))?;
        let manifest: Manifest = toml::from_str(&source)
            .map_err(|error| format!("invalid {}: {error}", manifest_path.display()))?;
        let root = manifest_path
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| format!("{} has no project root", manifest_path.display()))?
            .to_owned();
        validate(&manifest)?;
        validate_target_paths(&root, &manifest)?;
        let contents = read_sections(&root, &manifest)?;
        Ok(Self {
            root,
            manifest_path: manifest_path.to_owned(),
            source,
            manifest,
            contents,
        })
    }

    pub(crate) fn target_names(&self) -> impl Iterator<Item = &str> {
        self.manifest.targets.keys().map(String::as_str)
    }

    pub(crate) fn target_path(&self, id: &str) -> Result<PathBuf, String> {
        Ok(self.root.join(target_filename(id)?))
    }

    pub(crate) fn composition(&self, id: &str) -> Result<&[String], String> {
        self.manifest
            .targets
            .get(id)
            .map(|target| target.sections.as_slice())
            .ok_or_else(|| format!("project has no {id} target"))
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
        self.section(id)
            .map(|section| self.root.join(".mdmanager").join(&section.path))
    }

    pub(crate) fn render(&self, id: &str) -> Result<String, String> {
        let sections = self.composition(id)?;
        self.render_sections(sections)
    }

    pub(crate) fn render_sections(&self, sections: &[String]) -> Result<String, String> {
        let contents = sections
            .iter()
            .map(|section| {
                self.contents
                    .get(section)
                    .map(String::as_str)
                    .ok_or_else(|| format!("section {section} has no loaded content"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(render_format_1(&contents))
    }

    pub(crate) fn inspect(&self, id: &str) -> Result<TargetView, String> {
        let expected = self.render(id)?;
        let path = self.target_path(id)?;
        reject_target_symlink(&path)?;
        let deployed = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
        };
        let status = if !path.exists() {
            ProjectTargetStatus::Missing
        } else if deployed == expected {
            ProjectTargetStatus::Current
        } else {
            ProjectTargetStatus::OutOfSync
        };
        let old = if status == ProjectTargetStatus::Missing {
            "/dev/null".to_owned()
        } else {
            path.display().to_string()
        };
        let diff = if status == ProjectTargetStatus::Current {
            "No changes.".to_owned()
        } else {
            unified_diff(&deployed, &expected, &old, &format!("rendered:{id}"))
        };
        Ok(TargetView {
            id: id.to_owned(),
            path,
            expected,
            deployed,
            status,
            diff,
        })
    }

    pub(crate) fn inspect_all(&self) -> Result<Vec<TargetView>, String> {
        self.target_names().map(|id| self.inspect(id)).collect()
    }

    pub(crate) fn apply(&self, id: &str) -> Result<ProjectTargetStatus, String> {
        let reviewed = self.inspect(id)?;
        let current = Self::load(&self.manifest_path)?.inspect(id)?;
        if reviewed.status != current.status
            || reviewed.expected != current.expected
            || reviewed.deployed != current.deployed
        {
            return Err("project target changed after review; inspect and confirm again".into());
        }
        if current.status != ProjectTargetStatus::Current {
            atomic_write(&current.path, current.expected.as_bytes())?;
        }
        Ok(current.status)
    }
}

pub(crate) fn init(root: &Path) -> Result<Workspace, String> {
    let manifest = root.join(".mdmanager/project.toml");
    if manifest.exists() {
        return Err(format!("refusing to overwrite {}", manifest.display()));
    }
    fs::create_dir_all(root.join(".mdmanager/sections"))
        .map_err(|error| format!("cannot create project Sections directory: {error}"))?;
    atomic_create(&manifest, b"format = 1\n")?;
    Workspace::load(&manifest)
}

pub(crate) fn create_plan(root: &Path, id: &str, content: &str) -> Result<CreatePlan, String> {
    target_filename(id)?;
    if content.trim().is_empty() {
        return Err("project instructions cannot be empty".into());
    }
    let target_path = root.join(target_filename(id)?);
    reject_target_symlink(&target_path)?;
    let manifest_path = root.join(".mdmanager/project.toml");
    let reviewed_manifest = match fs::read_to_string(&manifest_path) {
        Ok(source) => {
            let workspace = Workspace::load(&manifest_path)?;
            if workspace.manifest.targets.contains_key(id) {
                return Err(format!("project already manages {id}"));
            }
            if workspace
                .manifest
                .sections
                .iter()
                .any(|section| section.id == id)
            {
                return Err(format!("project already defines section {id}"));
            }
            Some(source)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("cannot read {}: {error}", manifest_path.display())),
    };
    if target_path.exists() {
        return Err(format!(
            "{} already exists; adopt it instead",
            target_path.display()
        ));
    }
    let base = reviewed_manifest.as_deref().unwrap_or("format = 1\n");
    let section_relative = format!("sections/{id}.md");
    let manifest_source = format!(
        "{}\n\n[[sections]]\nid = \"{id}\"\nname = \"{}\"\npath = \"{section_relative}\"\n\n[targets.{id}]\nsections = [\"{id}\"]\n",
        base.trim_end(),
        target_display_name(id)
    );
    parse_source(&manifest_source)?;
    Ok(CreatePlan {
        section_path: root.join(".mdmanager").join(section_relative),
        target_path,
        content: content.into(),
        manifest_path,
        reviewed_manifest,
        manifest_source,
    })
}

pub(crate) fn adopt(root: &Path, id: &str) -> Result<Workspace, String> {
    let target = root.join(target_filename(id)?);
    reject_target_symlink(&target)?;
    let bytes = fs::read(&target)
        .map_err(|error| format!("cannot read adoption source {}: {error}", target.display()))?;
    let content =
        String::from_utf8(bytes).map_err(|_| format!("{} is not UTF-8", target.display()))?;
    if content.is_empty() {
        return Err(format!("cannot adopt empty {}", target.display()));
    }
    let manifest_path = root.join(".mdmanager/project.toml");
    if !manifest_path.exists() {
        init(root)?;
    }
    let workspace = Workspace::load(&manifest_path)?;
    if workspace.manifest.targets.contains_key(id) {
        return Err(format!("project already manages {id}"));
    }
    if workspace
        .manifest
        .sections
        .iter()
        .any(|section| section.id == id)
    {
        return Err(format!("project already defines section {id}"));
    }
    let section_relative = format!("sections/{id}.md");
    let section_path = root.join(".mdmanager").join(&section_relative);
    if section_path.exists() {
        return Err(format!("refusing to overwrite {}", section_path.display()));
    }
    let appended = format!(
        "{}\n[[sections]]\nid = \"{id}\"\nname = \"{}\"\npath = \"{section_relative}\"\n\n[targets.{id}]\nsections = [\"{id}\"]\n",
        workspace.source.trim_end(),
        target_display_name(id)
    );
    let parsed = parse_source(&appended)?;
    let rendered = render_format_1(&[content.as_str()]);
    if rendered.as_bytes() != content.as_bytes() {
        return Err("adoption render is not byte-identical; no files were changed".into());
    }
    atomic_create(&section_path, content.as_bytes())?;
    if let Err(error) = atomic_write(&manifest_path, appended.as_bytes()) {
        let _ = fs::remove_file(&section_path);
        return Err(error);
    }
    let loaded = Workspace::load(&manifest_path)?;
    if loaded.manifest.targets.len() != parsed.targets.len() {
        return Err("adoption verification failed".into());
    }
    Ok(loaded)
}

fn parse_source(source: &str) -> Result<Manifest, String> {
    let manifest: Manifest =
        toml::from_str(source).map_err(|error| format!("invalid project manifest: {error}"))?;
    validate(&manifest)?;
    Ok(manifest)
}

fn validate(manifest: &Manifest) -> Result<(), String> {
    if manifest.format != 1 {
        return Err(format!(
            "unsupported project render format {}",
            manifest.format
        ));
    }
    let mut ids = HashSet::new();
    for section in &manifest.sections {
        validate_id("project section", &section.id)?;
        if !ids.insert(section.id.as_str()) {
            return Err(format!("duplicate project section {}", section.id));
        }
        if section.name.trim().is_empty() {
            return Err(format!("project section {} has an empty name", section.id));
        }
        validate_relative_path("project Section", &section.path)?;
    }
    for (id, target) in &manifest.targets {
        target_filename(id)?;
        if target.sections.is_empty() {
            return Err(format!(
                "project target {id} must contain at least one Section"
            ));
        }
        let mut seen = HashSet::new();
        for section in &target.sections {
            if !ids.contains(section.as_str()) {
                return Err(format!(
                    "project target {id} refers to unknown section {section}"
                ));
            }
            if !seen.insert(section) {
                return Err(format!("project target {id} repeats section {section}"));
            }
        }
    }
    Ok(())
}

fn validate_target_paths(root: &Path, manifest: &Manifest) -> Result<(), String> {
    for id in manifest.targets.keys() {
        reject_target_symlink(&root.join(target_filename(id)?))?;
    }
    Ok(())
}

fn reject_target_symlink(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(format!(
            "project target {} is a symlink; mdmanager.ai will not replace it",
            path.display()
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "cannot inspect project target {}: {error}",
            path.display()
        )),
    }
}

fn read_sections(root: &Path, manifest: &Manifest) -> Result<IndexMap<String, String>, String> {
    let mut contents = IndexMap::new();
    for section in &manifest.sections {
        let path = root.join(".mdmanager").join(&section.path);
        let content = fs::read_to_string(&path)
            .map_err(|error| format!("cannot read project Section {}: {error}", path.display()))?;
        if content.is_empty() {
            return Err(format!("project Section {} is empty", path.display()));
        }
        contents.insert(section.id.clone(), content);
    }
    Ok(contents)
}

/// Supported Project targets and their root filenames; unknown target ids are errors.
pub(crate) const PROJECT_TARGETS: [(&str, &str); 2] =
    [("agents", "AGENTS.md"), ("claude", "CLAUDE.md")];

/// Project target filename policy, independent of display labels.
pub(crate) fn target_filename(id: &str) -> Result<&'static str, String> {
    PROJECT_TARGETS
        .iter()
        .find(|(target, _)| *target == id)
        .map(|(_, filename)| *filename)
        .ok_or_else(|| format!("unknown project target {id}; expected agents or claude"))
}

fn canonical_directory(path: &Path) -> Result<PathBuf, String> {
    let path = fs::canonicalize(path).map_err(|error| {
        format!(
            "cannot resolve launch directory {}: {error}",
            path.display()
        )
    })?;
    if !path.is_dir() {
        return Err(format!("{} is not a directory", path.display()));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn workspace(source: &str, sections: &[(&str, &str)]) -> (TempDir, Workspace) {
        let temp = TempDir::new().unwrap();
        let mdmanager = temp.path().join(".mdmanager");
        fs::create_dir_all(mdmanager.join("sections")).unwrap();
        fs::write(mdmanager.join("project.toml"), source).unwrap();
        for (path, content) in sections {
            fs::write(mdmanager.join(path), content).unwrap();
        }
        let loaded = Workspace::load(&mdmanager.join("project.toml")).unwrap();
        (temp, loaded)
    }

    #[test]
    fn format_one_preserves_one_section_exactly() {
        assert_eq!(
            render_format_1(&["# A\r\n\r\nText\r\n"]),
            "# A\r\n\r\nText\r\n"
        );
    }

    #[test]
    fn format_one_joins_only_adjacent_line_endings() {
        assert_eq!(
            render_format_1(&["# A\n\n", "\r\n## B\r\n"]),
            "# A\n\n## B\n"
        );
    }

    #[test]
    fn discovery_stops_at_the_git_worktree_root() {
        let temp = TempDir::new().unwrap();
        fs::create_dir_all(temp.path().join(".mdmanager")).unwrap();
        fs::write(
            temp.path().join(".mdmanager/project.toml"),
            "invalid = true\n",
        )
        .unwrap();
        let repository = temp.path().join("repository");
        fs::create_dir(&repository).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .current_dir(&repository)
                .status()
                .unwrap()
                .success()
        );

        assert!(Workspace::discover(&repository).unwrap().is_none());
    }

    #[test]
    fn status_is_stateless() {
        let source = "format = 1\n\n[[sections]]\nid = \"common\"\nname = \"Common\"\npath = \"sections/common.md\"\n\n[targets.agents]\nsections = [\"common\"]\n";
        let (temp, workspace) = workspace(source, &[("sections/common.md", "hello\n")]);
        assert_eq!(
            workspace.inspect("agents").unwrap().status,
            ProjectTargetStatus::Missing
        );
        fs::write(temp.path().join("AGENTS.md"), "different\n").unwrap();
        assert_eq!(
            workspace.inspect("agents").unwrap().status,
            ProjectTargetStatus::OutOfSync
        );
        fs::write(temp.path().join("AGENTS.md"), "hello\n").unwrap();
        assert_eq!(
            workspace.inspect("agents").unwrap().status,
            ProjectTargetStatus::Current
        );
    }

    #[test]
    fn create_plan_adds_management_before_the_root_target() {
        let repository = TempDir::new().unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .current_dir(repository.path())
                .status()
                .unwrap()
                .success()
        );

        let plan = create_plan(repository.path(), "agents", "# Project\n").unwrap();
        let workspace = plan.apply().unwrap();

        assert_eq!(
            fs::read_to_string(repository.path().join(".mdmanager/sections/agents.md")).unwrap(),
            "# Project\n"
        );
        assert_eq!(
            workspace.inspect("agents").unwrap().status,
            ProjectTargetStatus::Missing
        );
        assert!(!repository.path().join("AGENTS.md").exists());

        workspace.apply("agents").unwrap();
        assert_eq!(
            fs::read_to_string(repository.path().join("AGENTS.md")).unwrap(),
            "# Project\n"
        );
    }

    #[test]
    fn create_plan_names_the_section_with_the_target_display_name() {
        let repository = TempDir::new().unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .current_dir(repository.path())
                .status()
                .unwrap()
                .success()
        );

        let plan = create_plan(repository.path(), "agents", "# Project\n").unwrap();
        let manifest = parse_source(&plan.manifest_source).unwrap();

        assert_eq!(manifest.sections.len(), 1);
        assert_eq!(manifest.sections[0].name, target_display_name("agents"));
        assert_eq!(manifest.sections[0].id, "agents");
    }

    #[test]
    fn create_plan_reports_an_already_managed_target() {
        let repository = TempDir::new().unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .current_dir(repository.path())
                .status()
                .unwrap()
                .success()
        );
        create_plan(repository.path(), "agents", "# Project\n")
            .unwrap()
            .apply()
            .unwrap();

        let error = create_plan(repository.path(), "agents", "# Again\n").unwrap_err();

        assert_eq!(error, "project already manages agents");
    }

    #[cfg(unix)]
    #[test]
    fn project_management_never_replaces_target_symlinks() {
        use std::os::unix::fs::symlink;

        let repository = TempDir::new().unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .current_dir(repository.path())
                .status()
                .unwrap()
                .success()
        );
        fs::write(repository.path().join("AGENTS.md"), "shared\n").unwrap();
        symlink("AGENTS.md", repository.path().join("CLAUDE.md")).unwrap();

        let error = adopt(repository.path(), "claude").unwrap_err();
        assert!(error.contains("CLAUDE.md is a symlink"));
        assert!(!repository.path().join(".mdmanager/project.toml").exists());

        fs::remove_file(repository.path().join("CLAUDE.md")).unwrap();
        fs::write(repository.path().join("CLAUDE.md"), "claude\n").unwrap();
        let workspace = adopt(repository.path(), "claude").unwrap();
        fs::remove_file(repository.path().join("CLAUDE.md")).unwrap();
        symlink("AGENTS.md", repository.path().join("CLAUDE.md")).unwrap();

        let error = workspace.apply("claude").unwrap_err();
        assert!(error.contains("CLAUDE.md is a symlink"));
        assert!(
            fs::symlink_metadata(repository.path().join("CLAUDE.md"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
}
