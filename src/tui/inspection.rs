use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::{
    App, HomeItem, ManagedRef, ManagedSection, ManagedWorkspace, SymlinkInfo, SymlinkResolution,
    View, global_comparison,
};
use crate::config::target_display_name;
use crate::context::Audit;
use crate::deploy::{self, GlobalTargetStatus};
use crate::{local, project};

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
    pub(super) fn observe(app: &App) -> Self {
        let mut snapshot = Self {
            home_items: app.observe_home_items(),
            ..Self::new(app.context.audit.clone())
        };
        let mut references = Vec::new();
        let mut paths = vec![app.project_root.clone()];
        if let Some(project) = &app.project {
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
        if let Some(repository) = &app.local_repository {
            for target in [local::ManagedTarget::Agents, local::ManagedTarget::Claude] {
                if repository.managed_exists(target) {
                    references.push(ManagedRef::Local(target));
                }
                paths.push(repository.root.join(target.filename()));
            }
        }
        for (_, filename) in project::PROJECT_TARGETS {
            paths.push(app.project_root.join(filename));
        }
        if let Some(global) = &app.global {
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
                if let Some(repository) = &app.local_repository {
                    snapshot.personal_section_targets.insert(
                        section.id.clone(),
                        repository.personal_section_targets(&section.id),
                    );
                }
            }
        }
        for reference in references {
            let workspace = observe_managed_workspace(app, &reference);
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
            app.context
                .audit
                .sources
                .iter()
                .map(|source| source.path.clone()),
        );
        paths.extend(
            app.global_sources
                .iter()
                .map(|(_, source)| source.path.clone()),
        );
        for view in std::iter::once(&app.view).chain(app.history.iter().map(|(view, _)| view)) {
            if let View::File { path, .. } | View::SectionDocument { path, .. } = view {
                paths.push(path.clone());
            }
        }
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

impl App {
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
    app: &App,
    reference: &ManagedRef,
) -> Result<ManagedWorkspace, String> {
    match reference {
        ManagedRef::Project(id) => {
            let project = app
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
            let local_repository = app
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
            let global = app
                .global
                .as_ref()
                .ok_or_else(|| "Global configuration is unavailable".to_owned())?;
            let inspected = deploy::inspect_one(global, profile, target)?;
            let active = app.global_active_profile.as_deref() == Some(profile.as_str());
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
