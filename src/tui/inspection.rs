use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::{GlobalConfig, target_display_name};
use crate::context::{Audit, ContextRuntime, ContextSource, SourceGroup, SourceState};
use crate::deploy::{self, GlobalTargetStatus};
use crate::local::LocalRepository;
use crate::project::Workspace;
use crate::{local, project};

/// Inputs observed together during a session refresh. Open document paths include history.
pub(super) struct InspectionInput<'a> {
    pub(super) context: &'a Audit,
    pub(super) global: &'a Option<GlobalConfig>,
    pub(super) global_active_profile: &'a Option<String>,
    pub(super) global_error: &'a Option<String>,
    pub(super) global_sources: &'a [(ContextRuntime, ContextSource)],
    pub(super) local_repository: &'a Option<LocalRepository>,
    pub(super) project: &'a Option<Workspace>,
    pub(super) project_error: &'a Option<String>,
    pub(super) project_root: &'a PathBuf,
    pub(super) document_paths: Vec<&'a Path>,
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum InstructionStatus {
    Project(project::ProjectTargetStatus),
    Local(local::LocalTargetStatus),
    Global(GlobalTargetStatus),
}

impl InstructionStatus {
    pub(super) fn is_current(self) -> bool {
        matches!(
            self,
            Self::Project(project::ProjectTargetStatus::Current)
                | Self::Local(local::LocalTargetStatus::Current)
                | Self::Global(GlobalTargetStatus::Current)
        )
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Project(status) => status.label(),
            Self::Local(status) => status.label(),
            Self::Global(status) => status.label(),
        }
    }
}

pub(super) struct FileObservation {
    content: Result<String, String>,
    pub(super) symlink: Option<SymlinkInfo>,
    pub(super) canonical: Option<PathBuf>,
    pub(super) regular: bool,
}

pub(super) struct InstructionInspection {
    pub(super) context: Audit,
    pub(super) home_items: Vec<HomeItem>,
    pub(super) managed: Vec<(ManagedRef, Result<ManagedWorkspace, String>)>,
    pub(super) files: HashMap<PathBuf, FileObservation>,
    pub(super) personal_section_targets: HashMap<String, Vec<&'static str>>,
}

impl InstructionInspection {
    pub(super) fn new(context: Audit) -> Self {
        Self {
            context,
            home_items: Vec::new(),
            managed: Vec::new(),
            files: HashMap::new(),
            personal_section_targets: HashMap::new(),
        }
    }

    // These observations form one refresh generation, not an atomic filesystem snapshot.
    pub(super) fn observe(input: &InspectionInput<'_>) -> Self {
        let mut snapshot = Self {
            home_items: input.observe_home_items(),
            ..Self::new(input.context.clone())
        };
        let mut references = Vec::new();
        let mut paths = vec![input.project_root.to_owned()];
        if let Some(project) = &input.project {
            references.extend(
                project
                    .target_names()
                    .map(|id| ManagedRef::Project(id.to_owned())),
            );
            paths.extend(
                project
                    .manifest
                    .sections
                    .iter()
                    .filter_map(|section| project.section_path(&section.id)),
            );
        }
        if let Some(repository) = &input.local_repository {
            for target in [local::ManagedTarget::Agents, local::ManagedTarget::Claude] {
                if repository.managed_exists(target) {
                    references.push(ManagedRef::Local(target));
                }
                paths.push(repository.root.join(target.filename()));
            }
        }
        for (_, filename) in project::PROJECT_TARGETS {
            paths.push(input.project_root.join(filename));
        }
        if let Some(global) = &input.global {
            for profile in global.profile_names() {
                if let Ok(targets) = global.profile_target_names(profile) {
                    references.extend(targets.map(|target| ManagedRef::Global {
                        profile: profile.to_owned(),
                        target: target.to_owned(),
                    }));
                }
            }
            paths.extend(
                global
                    .target_names()
                    .filter_map(|id| global.target_path(id).ok()),
            );
            for section in &global.manifest.sections {
                paths.extend(global.section_path(&section.id));
                if let Some(repository) = &input.local_repository {
                    snapshot.personal_section_targets.insert(
                        section.id.clone(),
                        repository.personal_section_targets(&section.id),
                    );
                }
            }
        }
        for reference in references {
            let workspace = observe_managed_workspace(input, &reference);
            if let Ok(workspace) = &workspace {
                paths.push(workspace.target.clone());
                paths.extend(
                    workspace
                        .sections
                        .iter()
                        .map(|section| section.path.clone()),
                );
            }
            snapshot.managed.push((reference, workspace));
        }
        paths.extend(
            input
                .context
                .sources
                .iter()
                .map(|source| source.path.clone()),
        );
        paths.extend(
            input
                .global_sources
                .iter()
                .map(|(_, source)| source.path.clone()),
        );
        paths.extend(input.document_paths.iter().map(|path| path.to_path_buf()));
        for path in paths {
            if let Some(parent) = path.parent() {
                snapshot.observe_document(parent);
            }
            snapshot.observe_document(&path);
        }
        snapshot
    }

    fn observe_document(&mut self, path: &Path) {
        if self.files.contains_key(path) {
            return;
        }
        self.files.insert(
            path.to_owned(),
            FileObservation {
                content: fs::read_to_string(path)
                    .map_err(|error| format!("Cannot read {}: {error}", path.display())),
                symlink: observe_symlink(path),
                canonical: fs::canonicalize(path).ok(),
                regular: fs::symlink_metadata(path)
                    .is_ok_and(|metadata| metadata.file_type().is_file()),
            },
        );
    }

    pub(super) fn document(&self, path: &Path) -> Result<String, String> {
        self.files
            .get(path)
            .map(|file| file.content.clone())
            .unwrap_or_else(|| Err(format!("Document is unavailable: {}", path.display())))
    }

    pub(super) fn canonical(&self, path: &Path) -> Option<PathBuf> {
        self.files.get(path).and_then(|file| file.canonical.clone())
    }
}

impl InspectionInput<'_> {
    fn observe_home_items(&self) -> Vec<HomeItem> {
        let mut items = vec![HomeItem::Context];
        if self.project_error.is_some() {
            items.push(HomeItem::ProjectInvalid);
        } else if self.local_repository.is_some() {
            for (id, name) in project::PROJECT_TARGETS {
                let path = self.project_root.join(name);
                if self
                    .project
                    .as_ref()
                    .is_some_and(|project| project.manifest.targets.contains_key(id))
                {
                    items.push(HomeItem::ProjectTarget(id.into()));
                } else if path.is_file() {
                    items.push(HomeItem::ProjectExternal {
                        id: id.into(),
                        path,
                    });
                } else {
                    items.push(HomeItem::ProjectMissing(id.into()));
                }
            }
        }
        if let Some(local_repository) = &self.local_repository {
            for runtime in [local::DisableRuntime::Claude, local::DisableRuntime::Pi] {
                let status = local_repository
                    .disable_status(runtime)
                    .unwrap_or(local::DisableStatus::Missing);
                if status != local::DisableStatus::None {
                    items.push(HomeItem::LocalDisable(runtime, status));
                }
            }
            if let Some(error) = local_repository.local_manifest_error() {
                items.push(HomeItem::LocalInvalid(error));
            } else {
                for target in [local::ManagedTarget::Agents, local::ManagedTarget::Claude] {
                    let path = local_repository.root.join(target.filename());
                    if local_repository.managed_exists(target) {
                        items.push(HomeItem::LocalTarget(target));
                    } else if path.is_file() {
                        items.push(HomeItem::LocalExternal(target, path));
                    } else {
                        items.push(HomeItem::LocalMissing(target));
                    }
                }
            }
        }
        if self.global_error.is_some() {
            items.push(HomeItem::GlobalInvalid);
        }
        if let Some(global) = &self.global {
            let ids: Vec<String> = match self.global_active_profile.as_deref() {
                Some(profile) => global
                    .profile_target_names(profile)
                    .map(|ids| ids.map(str::to_owned).collect())
                    .unwrap_or_default(),
                None => global.target_names().map(str::to_owned).collect(),
            };
            items.extend(ids.into_iter().map(HomeItem::GlobalTarget));
            items.extend((0..self.global_sources.len()).map(HomeItem::GlobalExternal));
        } else {
            items.extend((0..self.global_sources.len()).map(HomeItem::GlobalExternal));
            if self.global_error.is_none() && self.global_sources.is_empty() {
                items.push(HomeItem::GlobalSetup);
            }
        }
        if self.global.is_some() || self.project.is_some() {
            items.push(HomeItem::Library);
        }
        items
    }
}

fn observe_symlink(path: &Path) -> Option<SymlinkInfo> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.file_type().is_symlink() {
        return None;
    }
    let target = fs::read_link(path).ok()?;
    let resolution = match fs::canonicalize(path) {
        Ok(resolved) => SymlinkResolution::Resolved(resolved),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => SymlinkResolution::Missing,
        Err(_) => SymlinkResolution::Unresolved,
    };
    Some(SymlinkInfo { target, resolution })
}

fn observe_managed_workspace(
    input: &InspectionInput<'_>,
    reference: &ManagedRef,
) -> Result<ManagedWorkspace, String> {
    match reference {
        ManagedRef::Project(id) => {
            let project = input
                .project
                .as_ref()
                .ok_or_else(|| "Project configuration is unavailable".to_owned())?;
            let inspected = project.inspect(id)?;
            let sections = project
                .composition(id)?
                .iter()
                .filter_map(|section_id| {
                    let section = project.section(section_id)?;
                    let path = project.section_path(section_id)?;
                    Some(ManagedSection {
                        id: section_id.clone(),
                        name: section.name.clone(),
                        content: project
                            .section_content(section_id)
                            .unwrap_or_default()
                            .to_owned(),
                        path,
                    })
                })
                .collect();
            Ok(ManagedWorkspace {
                title: format!(
                    "Project · {}",
                    project::target_filename(id).expect("validated Project target")
                ),
                status: inspected.status.label().into(),
                target: inspected.path,
                rendered: inspected.expected,
                difference: inspected.difference,
                instruction_status: InstructionStatus::Project(inspected.status),
                sections,
            })
        }
        ManagedRef::Local(target) => {
            let local_repository = input
                .local_repository
                .as_ref()
                .ok_or_else(|| "Local repository state is unavailable".to_owned())?;
            let inspected = local_repository.inspect_managed(*target)?;
            let sections = inspected
                .ordered
                .iter()
                .filter_map(|section_id| {
                    let section = inspected
                        .sections
                        .iter()
                        .find(|section| &section.id == section_id)?;
                    let path = local_repository
                        .managed_section_path_for(*target, section_id)
                        .ok()?;
                    Some(ManagedSection {
                        id: section_id.clone(),
                        name: section.name.clone(),
                        content: fs::read_to_string(&path)
                            .unwrap_or_else(|error| format!("Cannot read Section: {error}")),
                        path,
                    })
                })
                .collect();
            Ok(ManagedWorkspace {
                title: format!("Local Instructions · {}", target.filename()),
                status: inspected.status.label().into(),
                target: inspected.path,
                rendered: inspected.rendered,
                difference: inspected.difference,
                instruction_status: InstructionStatus::Local(inspected.status),
                sections,
            })
        }
        ManagedRef::Global { profile, target } => {
            let global = input
                .global
                .as_ref()
                .ok_or_else(|| "Global configuration is unavailable".to_owned())?;
            let inspected = deploy::inspect_one(global, profile, target)?;
            let active = input.global_active_profile.as_deref() == Some(profile.as_str());
            let status = if active {
                inspected.status.label().into()
            } else {
                global_comparison(&inspected)
            };
            let sections = global
                .composition(profile, target)?
                .iter()
                .filter_map(|section_id| {
                    let section = global.section(section_id)?;
                    let path = global.section_path(section_id)?;
                    Some(ManagedSection {
                        id: section_id.clone(),
                        name: section.name.clone(),
                        content: global
                            .section_content(section_id)
                            .unwrap_or_default()
                            .to_owned(),
                        path,
                    })
                })
                .collect();
            Ok(ManagedWorkspace {
                title: format!(
                    "Global Profile {profile}{} · {}",
                    if active { "" } else { " (not active)" },
                    target_display_name(target)
                ),
                status,
                target: inspected.path,
                rendered: inspected.expected,
                difference: inspected.difference,
                instruction_status: InstructionStatus::Global(inspected.status),
                sections,
            })
        }
    }
}

pub(super) struct ManagedDestination {
    pub(super) scope: &'static str,
    pub(super) target: String,
    pub(super) status: Option<InstructionStatus>,
}

#[derive(Clone)]
pub(super) enum SymlinkResolution {
    Resolved(PathBuf),
    Missing,
    Unresolved,
}

#[derive(Clone)]
pub(super) struct SymlinkInfo {
    pub(super) target: PathBuf,
    pub(super) resolution: SymlinkResolution,
}

#[derive(Clone)]
pub(super) struct ManagedWorkspace {
    pub(super) title: String,
    pub(super) status: String,
    pub(super) target: PathBuf,
    pub(super) rendered: String,
    pub(super) difference: Option<String>,
    pub(super) instruction_status: InstructionStatus,
    pub(super) sections: Vec<ManagedSection>,
}

#[derive(Clone)]
pub(super) struct ManagedSection {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) path: PathBuf,
    pub(super) content: String,
}

#[derive(Clone, PartialEq)]
pub(super) enum ManagedRef {
    Project(String),
    Local(local::ManagedTarget),
    // The browsed Profile is part of the reference so reloads and history cannot
    // silently switch it to another Profile.
    Global { profile: String, target: String },
}

fn global_comparison(view: &deploy::TargetView) -> String {
    if view.status == GlobalTargetStatus::Missing {
        "target file not found".into()
    } else if view.difference.is_none() {
        "matches current file".into()
    } else {
        "differs from current file".into()
    }
}

pub(super) fn symlink_info(inspection: &InstructionInspection, path: &Path) -> Option<SymlinkInfo> {
    inspection
        .files
        .get(path)
        .and_then(|file| file.symlink.clone())
}

fn regular_file_resolves_to(
    inspection: &InstructionInspection,
    path: &Path,
    resolved: &Path,
) -> bool {
    inspection
        .files
        .get(path)
        .is_some_and(|file| file.regular && file.canonical.as_deref() == Some(resolved))
}

pub(super) fn managed_destination(
    global_profile: Option<(&GlobalConfig, &str)>,
    inspection: &InstructionInspection,
    local_repository: &Option<LocalRepository>,
    project: &Option<Workspace>,
    resolved: &Path,
) -> Option<ManagedDestination> {
    if let Some(destination) = project.as_ref().and_then(|project| {
        project.target_names().find_map(|id| {
            let path = project.target_path(id).ok()?;
            regular_file_resolves_to(inspection, &path, resolved).then(|| ManagedDestination {
                scope: "Project",
                target: project::target_filename(id)
                    .expect("validated Project target")
                    .into(),
                status: managed_workspace(&inspection.managed, &ManagedRef::Project(id.to_owned()))
                    .ok()
                    .map(|view| view.instruction_status),
            })
        })
    }) {
        return Some(destination);
    }

    if let Some(destination) = local_repository.as_ref().and_then(|local_repository| {
        [local::ManagedTarget::Agents, local::ManagedTarget::Claude]
            .into_iter()
            .find_map(|target| {
                let path = local_repository.root.join(target.filename());
                (inspection.home_items.iter().any(
                    |item| matches!(item, HomeItem::LocalTarget(candidate) if *candidate == target),
                ) && regular_file_resolves_to(inspection, &path, resolved))
                .then(|| ManagedDestination {
                    scope: "Local",
                    target: target.filename().into(),
                    status: managed_workspace(&inspection.managed, &ManagedRef::Local(target))
                        .ok()
                        .map(|view| view.instruction_status),
                })
            })
    }) {
        return Some(destination);
    }

    let (global, profile) = global_profile?;
    global.profile_target_names(profile).ok()?.find_map(|id| {
        let path = global.target_path(id).ok()?;
        regular_file_resolves_to(inspection, &path, resolved).then(|| ManagedDestination {
            scope: "Global",
            target: path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(id)
                .into(),
            status: managed_workspace(
                &inspection.managed,
                &ManagedRef::Global {
                    profile: profile.to_owned(),
                    target: id.to_owned(),
                },
            )
            .ok()
            .map(|view| view.instruction_status),
        })
    })
}

pub(super) fn symlink_target_status(
    global_profile: Option<(&GlobalConfig, &str)>,
    inspection: &InstructionInspection,
    local_repository: &Option<LocalRepository>,
    project: &Option<Workspace>,
    project_root: &Path,
    path: &Path,
    info: &SymlinkInfo,
) -> String {
    match &info.resolution {
        SymlinkResolution::Resolved(resolved) => {
            if let Some(destination) = managed_destination(
                global_profile,
                inspection,
                local_repository,
                project,
                resolved,
            ) {
                format!(
                    "{} {} · managed by mdmanager.ai · {}",
                    destination.scope,
                    destination.target,
                    destination
                        .status
                        .map_or("invalid", InstructionStatus::label)
                )
            } else {
                let project_root = inspection.canonical(project_root);
                let source_is_project = path
                    .parent()
                    .and_then(|parent| inspection.canonical(parent))
                    .zip(project_root.as_ref())
                    .is_some_and(|(parent, root)| parent.starts_with(root));
                if source_is_project && project_root.is_some_and(|root| !resolved.starts_with(root))
                {
                    "outside the project · not managed by mdmanager.ai".into()
                } else {
                    "existing file · not managed by mdmanager.ai".into()
                }
            }
        }
        SymlinkResolution::Missing => "missing".into(),
        SymlinkResolution::Unresolved => "cannot resolve".into(),
    }
}

pub(super) fn external_file_about(
    global_profile: Option<(&GlobalConfig, &str)>,
    inspection: &InstructionInspection,
    local_repository: &Option<LocalRepository>,
    project: &Option<Workspace>,
    project_root: &Path,
    kind: &str,
    path: &Path,
) -> String {
    if let Err(error) = inspection.document(path) {
        return format!(
            "{kind}
{error}"
        );
    }
    let Some(info) = symlink_info(inspection, path) else {
        return format!(
            "{kind} · existing · not managed by mdmanager.ai\nPath: {}",
            path.display()
        );
    };
    format!(
        "{kind}\nPath: {}\nType: symlink\nPoints to: {}\nLink: not managed by mdmanager.ai\nTarget: {}",
        path.display(),
        info.target.display(),
        symlink_target_status(
            global_profile,
            inspection,
            local_repository,
            project,
            project_root,
            path,
            &info
        )
    )
}

pub(super) fn managed_workspace(
    managed: &[(ManagedRef, Result<ManagedWorkspace, String>)],
    reference: &ManagedRef,
) -> Result<ManagedWorkspace, String> {
    managed
        .iter()
        .find(|(candidate, _)| candidate == reference)
        .map(|(_, workspace)| workspace.clone())
        .unwrap_or_else(|| Err("Managed target is unavailable".into()))
}

pub(super) fn global_diagnostic_text(error: &str) -> String {
    if error.contains("mdmanager doctor") {
        error.to_owned()
    } else {
        format!("{error}\n\nAction: run `mdmanager doctor` for diagnosis and safe recovery.")
    }
}

#[derive(Clone)]
pub(super) enum HomeItem {
    Context,
    ProjectInvalid,
    ProjectMissing(String),
    ProjectTarget(String),
    ProjectExternal { id: String, path: PathBuf },
    LocalDisable(local::DisableRuntime, local::DisableStatus),
    LocalInvalid(String),
    LocalMissing(local::ManagedTarget),
    LocalExternal(local::ManagedTarget, PathBuf),
    LocalTarget(local::ManagedTarget),
    GlobalInvalid,
    GlobalSetup,
    GlobalExternal(usize),
    GlobalTarget(String),
    Library,
}

/// Startup load order excludes candidates that the runtime does not actually load.
pub(super) fn loaded_at_startup(source: &ContextSource) -> bool {
    source.group == SourceGroup::Startup
        && matches!(
            source.state,
            SourceState::Startup | SourceState::Truncated { .. }
        )
}

#[cfg(test)]
mod tests {
    use super::super::tests::{fixture, global_fixture};
    use super::super::*;
    use super::*;
    use std::fs;

    use super::super::inspection::{
        InstructionStatus, SymlinkResolution, managed_destination, symlink_info,
    };
    use crate::deploy::GlobalTargetStatus;
    use tempfile::TempDir;

    #[test]
    fn symlink_destination_preserves_link_ownership_and_finds_managed_target() {
        use std::os::unix::fs::symlink;

        let (_home, repository, paths, _app) = fixture();
        project::adopt(repository.path(), "agents").unwrap();
        let link = repository.path().join("CLAUDE.md");
        symlink("AGENTS.md", &link).unwrap();
        let app = App::new_at(paths, None, repository.path().to_owned()).unwrap();

        let info = symlink_info(&app.inspection, &link).unwrap();
        assert_eq!(info.target, Path::new("AGENTS.md"));
        let SymlinkResolution::Resolved(resolved) = info.resolution else {
            panic!("managed target should resolve");
        };
        let destination = managed_destination(
            app.global
                .as_ref()
                .zip(app.global_active_profile.as_deref()),
            &app.inspection,
            &app.local_repository,
            &app.project,
            &resolved,
        )
        .unwrap();
        assert_eq!(destination.scope, "Project");
        assert_eq!(destination.target, "AGENTS.md");
        assert!(
            destination
                .status
                .is_some_and(InstructionStatus::is_current)
        );
    }

    #[test]
    fn broken_symlink_is_not_mistaken_for_an_unmanaged_file() {
        use std::os::unix::fs::symlink;

        let (_home, repository, _paths, mut app) = fixture();
        let link = repository.path().join("CLAUDE.md");
        symlink("missing.md", &link).unwrap();
        app.refresh_inspection();

        let info = symlink_info(&app.inspection, &link).unwrap();
        assert_eq!(info.target, Path::new("missing.md"));
        assert!(matches!(info.resolution, SymlinkResolution::Missing));
    }

    #[test]
    fn equal_unmanaged_global_content_has_no_difference() {
        let (_home, _repository, _paths, mut app) = global_fixture();
        let global = app.global.as_ref().unwrap();
        let target = global.target_path("claude").unwrap();
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, global.render("default", "claude").unwrap()).unwrap();
        app.reload_from_disk();
        let reference = ManagedRef::Global {
            profile: "default".into(),
            target: "claude".into(),
        };
        let workspace = managed_workspace(&app.inspection.managed, &reference).unwrap();
        assert!(
            workspace.instruction_status
                == InstructionStatus::Global(GlobalTargetStatus::Unmanaged)
        );
        assert!(workspace.difference.is_none());
        app.open(View::Managed(reference.clone()));
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
        );
        assert!(matches!(app.view, View::Managed(_)));

        fs::write(&target, "# Different\n").unwrap();
        app.reload_from_disk();
        // Changing presentation cannot change difference availability.
        for (_, workspace) in &mut app.inspection.managed {
            if let Ok(workspace) = workspace {
                workspace.status = "No changes.".into();
            }
        }
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
        );
        assert!(matches!(app.view, View::Diff(_)));
        fs::write(
            &target,
            app.global
                .as_ref()
                .unwrap()
                .render("default", "claude")
                .unwrap(),
        )
        .unwrap();
        app.reload_from_disk();
        assert!(matches!(app.view, View::Managed(_)));
    }

    #[test]
    fn configured_global_profile_keeps_external_symlink_aliases_visible() {
        use std::os::unix::fs::symlink;

        let home = TempDir::new().unwrap();
        let repository = TempDir::new().unwrap();
        let paths = Paths::for_home(home.path());
        fs::create_dir_all(paths.config.parent().unwrap().join("sections")).unwrap();
        fs::create_dir_all(home.path().join(".codex")).unwrap();
        fs::create_dir_all(home.path().join(".claude")).unwrap();
        fs::write(
            &paths.config,
            r#"
[[sections]]
id = "common"
name = "Common"
path = "sections/common.md"

[targets.codex]
path = "~/.codex/AGENTS.md"
title = "Global Agents"

[profiles.default]
codex = ["common"]
"#,
        )
        .unwrap();
        fs::write(
            paths.config.parent().unwrap().join("sections/common.md"),
            "# Common\n",
        )
        .unwrap();
        let global = GlobalConfig::load(&paths).unwrap();
        deploy::apply(&global, "default", None, false, true).unwrap();
        let alias = home.path().join(".claude/CLAUDE.md");
        symlink(home.path().join(".codex/AGENTS.md"), &alias).unwrap();

        let app = App::new_at(paths, Some(global), repository.path().to_owned()).unwrap();
        let (_, source) = app
            .global_sources
            .iter()
            .find(|(runtime, _)| *runtime == ContextRuntime::Claude)
            .unwrap();
        let info = symlink_info(&app.inspection, &source.path).unwrap();
        let SymlinkResolution::Resolved(resolved) = info.resolution else {
            panic!("global alias should resolve");
        };
        let destination = managed_destination(
            app.global
                .as_ref()
                .zip(app.global_active_profile.as_deref()),
            &app.inspection,
            &app.local_repository,
            &app.project,
            &resolved,
        )
        .unwrap();
        assert_eq!(destination.scope, "Global");
        assert!(
            destination
                .status
                .is_some_and(InstructionStatus::is_current)
        );
        assert!(
            app.home_items()
                .iter()
                .any(|item| matches!(item, HomeItem::GlobalExternal(_)))
        );
    }
}
