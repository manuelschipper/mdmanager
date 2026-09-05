use super::inspection::HomeItem;
use std::path::{Path, PathBuf};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState};
use unicode_width::UnicodeWidthStr;

use super::active_theme;
use super::context_ui::ContextScanStatus;
use super::document::{COMPACT_WIDTH, DocumentState, pad_right, relative_message};
use super::inspection::loaded_at_startup;
use super::inspection::{
    InstructionInspection, InstructionStatus, ManagedRef, SymlinkInfo, SymlinkResolution,
    managed_destination, managed_workspace, symlink_info,
};
use crate::config::{GlobalConfig, Paths, target_display_name};
use crate::context::{ContextRuntime, ContextSource, SourceGroup, display_path};
use crate::deploy::GlobalTargetStatus;
use crate::local::{self, LocalRepository};
use crate::project::{self, Workspace};

const HOME_MIN_WIDTH: u16 = 80;

/// Borrowed overview inputs from the session's accepted observations.
pub(super) struct OverviewInput<'a> {
    pub(super) global: &'a Option<GlobalConfig>,
    pub(super) global_active_profile: &'a Option<String>,
    pub(super) global_sources: &'a Vec<(ContextRuntime, ContextSource)>,
    pub(super) inspection: &'a InstructionInspection,
    pub(super) is_home: bool,
    pub(super) local_repository: &'a Option<LocalRepository>,
    pub(super) message: &'a Option<(bool, String)>,
    pub(super) paths: &'a Paths,
    pub(super) project: &'a Option<Workspace>,
    pub(super) project_root: &'a PathBuf,
    pub(super) scan_spinner: Option<&'static str>,
    pub(super) scan_status: &'a ContextScanStatus,
}

#[derive(Clone, Copy)]
enum HomeSignal {
    Current,
    Changed,
    External,
    Missing,
    Invalid,
}

pub(super) fn home_item_key(item: &HomeItem) -> String {
    match item {
        HomeItem::Context => "context".into(),
        HomeItem::ProjectInvalid => "project-invalid".into(),
        HomeItem::ProjectMissing(id) => format!("project-missing:{id}"),
        HomeItem::ProjectTarget(id) => format!("project:{id}"),
        HomeItem::ProjectExternal { id, .. } => format!("project-external:{id}"),
        HomeItem::LocalDisable(runtime, _) => format!("local-disable:{runtime:?}"),
        HomeItem::LocalInvalid(_) => "local-invalid".into(),
        HomeItem::LocalMissing(target) => format!("local-missing:{}", target.id()),
        HomeItem::LocalExternal(target, _) => format!("local-external:{}", target.id()),
        HomeItem::LocalTarget(target) => format!("local:{}", target.id()),
        HomeItem::GlobalInvalid => "global-invalid".into(),
        HomeItem::GlobalSetup => "global-setup".into(),
        HomeItem::GlobalExternal(index) => format!("global-external:{index}"),
        HomeItem::GlobalTarget(id) => format!("global:{id}"),
        HomeItem::Library => "library".into(),
    }
}

pub(super) fn home_target_reference(
    input: &OverviewInput<'_>,
    home_index: &usize,
) -> Option<ManagedRef> {
    match input.inspection.home_items.clone().get(*home_index)? {
        HomeItem::ProjectTarget(id) => Some(ManagedRef::Project(id.clone())),
        HomeItem::LocalTarget(target) => Some(ManagedRef::Local(*target)),
        HomeItem::GlobalTarget(target) => {
            input
                .global_active_profile
                .as_ref()
                .map(|profile| ManagedRef::Global {
                    profile: profile.clone(),
                    target: target.clone(),
                })
        }
        _ => None,
    }
}

pub(super) fn home_canvas_width(input: &OverviewInput<'_>, available: u16) -> u16 {
    let item_width = input
        .inspection
        .home_items
        .clone()
        .iter()
        .map(|item| {
            home_line(input, item)
                .width()
                .max(home_group_title(input, home_group(item)).width())
                + 4
        })
        .max()
        .unwrap_or(0);
    let header_width = home_header_line(input).map_or(0, |line| line.width() + 2);
    let footer_width = UnicodeWidthStr::width(
        "↑↓ select   enter open   d difference   r runtime   t theme   ? help   q quit",
    ) + 2;
    let desired = item_width
        .max(header_width)
        .max(footer_width)
        .clamp(usize::from(HOME_MIN_WIDTH), usize::from(COMPACT_WIDTH));
    u16::try_from(desired)
        .unwrap_or(COMPACT_WIDTH)
        .min(available)
}

pub(super) fn home_canvas_height(input: &OverviewInput<'_>) -> u16 {
    let items = input.inspection.home_items.clone();
    let mut groups = 0usize;
    let mut previous = "";
    for item in &items {
        let group = home_group(item);
        if group != previous {
            groups += 1;
            previous = group;
        }
    }
    let rows = items.len() + groups + groups.saturating_sub(1);
    u16::try_from(rows)
        .unwrap_or(u16::MAX)
        .saturating_add(8)
        .max(16)
}

pub(super) fn home_group(item: &HomeItem) -> &'static str {
    match item {
        HomeItem::Context => "CONTEXT",
        HomeItem::ProjectInvalid
        | HomeItem::ProjectMissing(_)
        | HomeItem::ProjectTarget(_)
        | HomeItem::ProjectExternal { .. } => "PROJECT",
        HomeItem::LocalDisable(_, _)
        | HomeItem::LocalInvalid(_)
        | HomeItem::LocalMissing(_)
        | HomeItem::LocalExternal(_, _)
        | HomeItem::LocalTarget(_) => "LOCAL",
        HomeItem::GlobalInvalid
        | HomeItem::GlobalSetup
        | HomeItem::GlobalExternal(_)
        | HomeItem::GlobalTarget(_) => "GLOBAL",
        HomeItem::Library => "LIBRARY",
    }
}

fn home_group_title(input: &OverviewInput<'_>, group: &str) -> String {
    match group {
        "CONTEXT" => format!("CONTEXT · {}", input.inspection.context.runtime.label()),
        "PROJECT" => format!(
            "PROJECT INSTRUCTIONS · {} · committed + shared",
            input
                .project_root
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("project")
        ),
        "LOCAL" => "LOCAL INSTRUCTIONS · this repository + this machine".into(),
        "GLOBAL" => match (&input.global, &input.global_active_profile) {
            (Some(_), Some(profile)) => {
                format!("GLOBAL INSTRUCTIONS · PROFILE {profile} (active)")
            }
            (Some(_), None) => "GLOBAL INSTRUCTIONS · no active Profile".into(),
            _ => "GLOBAL INSTRUCTIONS · this machine".into(),
        },
        "LIBRARY" => "LIBRARY".into(),
        _ => group.into(),
    }
}

pub(super) fn home_header_line(input: &OverviewInput<'_>) -> Option<Line<'static>> {
    if !input.is_home {
        return None;
    }
    if let Some((ok, text)) = &input.message {
        return Some(Line::styled(
            format!(
                "{} {}",
                if *ok { "●" } else { "!" },
                relative_message(input.project_root, text)
            ),
            Style::new().fg(if *ok {
                active_theme().success
            } else {
                active_theme().error
            }),
        ));
    }
    Some(Line::from(vec![
        Span::styled(
            "Ask your coding agent to read ",
            Style::new().fg(active_theme().muted),
        ),
        Span::styled(
            "mdmanager docs start",
            Style::new()
                .fg(active_theme().secondary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" to get started.", Style::new().fg(active_theme().muted)),
    ]))
}

/// Home rows use the accepted inspection generation and remain read-only.
pub(super) fn render_home(
    frame: &mut Frame<'_>,
    input: &OverviewInput<'_>,
    document: &mut DocumentState,
    home_index: &mut usize,
    area: Rect,
) {
    document.scroll = 0;
    document.max_scroll = 0;
    let items = input.inspection.home_items.clone();
    (*home_index) = (*home_index).min(items.len().saturating_sub(1));
    let mut rows = Vec::new();
    let mut selected_row = 0;
    let mut previous_group = "";
    for (index, item) in items.iter().enumerate() {
        let group = home_group(item);
        if group != previous_group {
            if !rows.is_empty() {
                rows.push(ListItem::new(""));
            }
            let title = home_group_title(input, group);
            rows.push(ListItem::new(Line::styled(
                title,
                Style::new()
                    .fg(active_theme().secondary)
                    .add_modifier(Modifier::BOLD),
            )));
            previous_group = group;
        }
        if index == (*home_index) {
            selected_row = rows.len();
        }
        rows.push(ListItem::new(home_line(input, item)));
    }
    let mut state = ListState::default().with_selected(Some(selected_row));
    frame.render_stateful_widget(
        List::new(rows)
            .block(
                Block::default()
                    .border_style(Style::new().fg(active_theme().dim))
                    .style(Style::new().bg(active_theme().surface))
                    .borders(Borders::ALL),
            )
            .highlight_style(active_theme().selected())
            .highlight_symbol("▸ "),
        area,
        &mut state,
    );
}

fn home_line(input: &OverviewInput<'_>, item: &HomeItem) -> Line<'static> {
    match item {
        HomeItem::Context => {
            let startup = input
                .inspection
                .context
                .sources
                .iter()
                .filter(|source| loaded_at_startup(source))
                .count();
            let relevant = input
                .inspection
                .context
                .sources
                .iter()
                .filter(|source| {
                    matches!(
                        source.group,
                        SourceGroup::PathFiltered | SourceGroup::Nested
                    )
                })
                .count();
            let sources = if startup == 1 { "source" } else { "sources" };
            let mut text = format!("  {startup} {sources} at startup");
            if relevant > 0 {
                text.push_str(&format!(" · {relevant} load when relevant"));
            }
            let mut spans = vec![Span::raw(text)];
            if matches!(
                input.scan_status,
                ContextScanStatus::NotStarted | ContextScanStatus::Scanning
            ) {
                spans.push(Span::styled(
                    input.scan_spinner.map_or_else(
                        || " · checking subfolders…".into(),
                        |spinner| format!(" · {spinner} checking subfolders…"),
                    ),
                    Style::new().fg(active_theme().muted),
                ));
            }
            Line::from(spans)
        }
        HomeItem::ProjectInvalid => Line::styled(
            "  project.toml is invalid · Enter details",
            Style::new().fg(active_theme().error),
        ),
        HomeItem::ProjectMissing(id) => home_status_line(
            project::target_filename(id).expect("validated Project target"),
            20,
            "not found",
            HomeSignal::Missing,
        ),
        HomeItem::ProjectTarget(id) => {
            let status = input
                .project
                .as_ref()
                .and_then(|_| {
                    managed_workspace(
                        &input.inspection.managed,
                        &ManagedRef::Project(id.to_owned()),
                    )
                    .ok()
                })
                .map_or("invalid", |view| view.instruction_status.label());
            home_status_line(
                project::target_filename(id).expect("validated Project target"),
                20,
                &format!("managed by mdmanager.ai · {status}"),
                managed_home_signal(input, &ManagedRef::Project(id.clone())),
            )
        }
        HomeItem::ProjectExternal { path, .. } => {
            let file = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("instruction file");
            let status = symlink_info(input.inspection, path).map_or_else(
                || "existing · not managed by mdmanager.ai".into(),
                |info| symlink_home_status(input, &info),
            );
            home_status_line(file, 20, &status, external_home_signal(input, path))
        }
        HomeItem::LocalDisable(runtime, status) => {
            let status = match status {
                local::DisableStatus::Owned => "excluded locally",
                local::DisableStatus::Modified => "exclusion changed outside mdmanager.ai",
                local::DisableStatus::Missing => "owned exclusion file is missing",
                local::DisableStatus::None => "not configured",
            };
            Line::raw(format!("  {} {status}", pad_right(runtime.label(), 12)))
        }
        HomeItem::LocalInvalid(_) => Line::styled(
            "  Local Instructions unavailable · Enter details",
            Style::new().fg(active_theme().error),
        ),
        HomeItem::LocalMissing(target) => {
            home_status_line(target.filename(), 20, "not found", HomeSignal::Missing)
        }
        HomeItem::LocalExternal(target, path) => {
            let status = symlink_info(input.inspection, path).map_or_else(
                || "existing · not managed by mdmanager.ai".into(),
                |info| symlink_home_status(input, &info),
            );
            home_status_line(
                target.filename(),
                20,
                &status,
                external_home_signal(input, path),
            )
        }
        HomeItem::LocalTarget(target) => {
            let status = input
                .local_repository
                .as_ref()
                .and_then(|_| {
                    managed_workspace(&input.inspection.managed, &ManagedRef::Local(*target)).ok()
                })
                .map_or_else(
                    || "invalid".into(),
                    |view| {
                        if view.instruction_status
                            == InstructionStatus::Local(local::LocalTargetStatus::External)
                        {
                            view.instruction_status.label().into()
                        } else {
                            format!(
                                "managed by mdmanager.ai · {}",
                                view.instruction_status.label()
                            )
                        }
                    },
                );
            home_status_line(
                target.filename(),
                20,
                &status,
                managed_home_signal(input, &ManagedRef::Local(*target)),
            )
        }
        HomeItem::GlobalInvalid => Line::styled(
            if input.global.is_some() {
                "  Deployment state needs attention · Enter details"
            } else {
                "  Global Instructions unavailable · Enter details"
            },
            Style::new().fg(active_theme().error),
        ),
        HomeItem::GlobalSetup => Line::styled(
            "  No global instruction files found · Enter details",
            Style::new().fg(active_theme().muted),
        ),
        HomeItem::GlobalExternal(index) => input.global_sources.get(*index).map_or_else(
            || Line::raw("  unknown global source"),
            |(runtime, source)| {
                let status = symlink_info(input.inspection, &source.path).map_or_else(
                    || "existing · not managed by mdmanager.ai".into(),
                    |info| symlink_home_status(input, &info),
                );
                Line::raw(format!(
                    "  {} {} {status}",
                    pad_right(runtime.label(), 8),
                    pad_right(&source.display, 28)
                ))
            },
        ),
        HomeItem::GlobalTarget(id) => {
            let status = input
                .global
                .as_ref()
                .and_then(|global| global.target_path(id).ok())
                .and_then(|path| {
                    symlink_info(input.inspection, &path)
                        .map(|info| global_symlink_home_status(input, id, &path, &info))
                })
                .unwrap_or_else(|| match (&input.global, &input.global_active_profile) {
                    (Some(_), Some(profile)) => managed_workspace(
                        &input.inspection.managed,
                        &ManagedRef::Global {
                            profile: profile.to_owned(),
                            target: id.to_owned(),
                        },
                    )
                    .map_or_else(
                        |_| "invalid".into(),
                        |view| {
                            if view.instruction_status
                                == InstructionStatus::Global(GlobalTargetStatus::Unmanaged)
                            {
                                view.instruction_status.label().into()
                            } else {
                                format!(
                                    "managed by mdmanager.ai · {}",
                                    view.instruction_status.label()
                                )
                            }
                        },
                    ),
                    _ => "not applied yet".into(),
                });
            home_status_line(
                &target_display_name(id),
                12,
                &status,
                if input
                    .global
                    .as_ref()
                    .and_then(|global| global.target_path(id).ok())
                    .is_some_and(|path| symlink_info(input.inspection, &path).is_some())
                {
                    HomeSignal::Invalid
                } else {
                    input
                        .global_active_profile
                        .as_ref()
                        .map_or(HomeSignal::Missing, |profile| {
                            managed_home_signal(
                                input,
                                &ManagedRef::Global {
                                    profile: profile.clone(),
                                    target: id.clone(),
                                },
                            )
                        })
                },
            )
        }
        HomeItem::Library => {
            let profiles = input
                .global
                .as_ref()
                .map_or(0, |global| global.profile_names().count());
            let personal = input
                .global
                .as_ref()
                .map_or(0, |global| global.manifest.sections.len());
            let project = input
                .project
                .as_ref()
                .map_or(0, |project| project.manifest.sections.len());
            let sections = personal + project;
            Line::from(vec![
                Span::raw("  Browse Profiles & Sections"),
                Span::styled(
                    format!(
                        "  · {profiles} Profile{} · {sections} Section{}",
                        if profiles == 1 { "" } else { "s" },
                        if sections == 1 { "" } else { "s" }
                    ),
                    Style::new().fg(active_theme().muted),
                ),
            ])
        }
    }
}

fn managed_home_signal(input: &OverviewInput<'_>, reference: &ManagedRef) -> HomeSignal {
    managed_workspace(&input.inspection.managed, reference).map_or(HomeSignal::Invalid, |view| {
        match view.instruction_status {
            InstructionStatus::Project(project::ProjectTargetStatus::Current)
            | InstructionStatus::Local(local::LocalTargetStatus::Current)
            | InstructionStatus::Global(GlobalTargetStatus::Current) => HomeSignal::Current,
            InstructionStatus::Project(project::ProjectTargetStatus::OutOfSync)
            | InstructionStatus::Local(local::LocalTargetStatus::Stale)
            | InstructionStatus::Global(GlobalTargetStatus::Stale) => HomeSignal::Changed,
            InstructionStatus::Local(local::LocalTargetStatus::External)
            | InstructionStatus::Global(GlobalTargetStatus::Unmanaged) => HomeSignal::External,
            InstructionStatus::Project(project::ProjectTargetStatus::Missing)
            | InstructionStatus::Local(local::LocalTargetStatus::Missing)
            | InstructionStatus::Global(GlobalTargetStatus::Missing) => HomeSignal::Missing,
            _ => HomeSignal::Invalid,
        }
    })
}

fn external_home_signal(input: &OverviewInput<'_>, path: &Path) -> HomeSignal {
    if symlink_info(input.inspection, path).is_some() {
        HomeSignal::Invalid
    } else {
        HomeSignal::External
    }
}

fn home_status_line(label: &str, width: usize, status: &str, signal: HomeSignal) -> Line<'static> {
    let (signal, color) = match signal {
        HomeSignal::Current => ("●", active_theme().success),
        HomeSignal::Changed => ("◐", active_theme().warning),
        HomeSignal::External => ("◇", active_theme().secondary),
        HomeSignal::Missing => ("○", active_theme().muted),
        HomeSignal::Invalid => ("!", active_theme().error),
    };
    Line::from(vec![
        Span::raw(format!("  {}", pad_right(label, width))),
        Span::styled(format!("{signal} {status}"), Style::new().fg(color)),
    ])
}

fn symlink_home_status(input: &OverviewInput<'_>, info: &SymlinkInfo) -> String {
    let target = if info.target.is_absolute() {
        display_path(
            &info.target,
            input.paths,
            &input.inspection.context.directory,
        )
    } else {
        info.target.display().to_string()
    };
    match &info.resolution {
        SymlinkResolution::Resolved(resolved) => {
            if let Some(destination) = managed_destination(
                input
                    .global
                    .as_ref()
                    .zip(input.global_active_profile.as_deref()),
                input.inspection,
                input.local_repository,
                input.project,
                resolved,
            ) {
                format!(
                    "symlink → {target} · target managed · {}",
                    destination
                        .status
                        .map_or("invalid", InstructionStatus::label)
                )
            } else {
                format!("symlink → {target} · target not managed by mdmanager.ai")
            }
        }
        SymlinkResolution::Missing => format!("broken symlink → {target}"),
        SymlinkResolution::Unresolved => format!("unresolved symlink → {target}"),
    }
}

pub(super) fn global_symlink_home_status(
    input: &OverviewInput<'_>,
    id: &str,
    path: &Path,
    info: &SymlinkInfo,
) -> String {
    let target = if info.target.is_absolute() {
        display_path(
            &info.target,
            input.paths,
            &input.inspection.context.directory,
        )
    } else {
        info.target.display().to_string()
    };
    let SymlinkResolution::Resolved(resolved) = &info.resolution else {
        return match &info.resolution {
            SymlinkResolution::Missing => format!("broken symlink → {target}"),
            SymlinkResolution::Unresolved => format!("unresolved symlink → {target}"),
            SymlinkResolution::Resolved(_) => unreachable!(),
        };
    };
    let destination = managed_destination(
        input
            .global
            .as_ref()
            .zip(input.global_active_profile.as_deref()),
        input.inspection,
        input.local_repository,
        input.project,
        resolved,
    )
    .map_or_else(
        || "target not managed by mdmanager.ai".into(),
        |destination| {
            if destination
                .status
                .is_some_and(InstructionStatus::is_current)
            {
                "target managed".into()
            } else {
                format!(
                    "target managed · {}",
                    destination
                        .status
                        .map_or("invalid", InstructionStatus::label)
                )
            }
        },
    );
    let comparison = input
        .global
        .as_ref()
        .zip(input.global_active_profile.as_deref())
        .and_then(|(global, profile)| global.render(profile, id).ok())
        .zip(input.inspection.document(path).ok())
        .map_or("no active Profile", |(expected, deployed)| {
            if expected == deployed {
                "matches Profile"
            } else {
                "differs from Profile"
            }
        });
    format!("symlink → {target} · {destination} · {comparison}")
}

#[cfg(test)]
mod tests {
    use super::super::tests::{draw, draw_at, fixture, global_fixture};
    use super::super::*;
    use super::*;
    use std::fs;

    use super::super::inspection::symlink_info;

    #[test]
    fn home_keeps_the_existing_groups() {
        let (_home, _repository, _paths, mut app) = fixture();
        let screen = draw(&mut app);
        assert!(screen.contains("CONTEXT"));
        assert!(screen.contains("CONTEXT · Claude"));
        assert!(screen.contains(" at startup"));
        assert!(!screen.contains("0 conditional"));
        assert!(!screen.contains("Enter inspect"));
        assert!(screen.contains("PROJECT INSTRUCTIONS"));
        assert!(screen.contains("LOCAL INSTRUCTIONS"));
        assert!(screen.contains("GLOBAL INSTRUCTIONS"));
        assert!(screen.contains("AGENTS.md           ◇ existing · not managed by mdmanager.ai"));
        assert!(screen.contains("AGENTS.override.md  ○ not found"));
        assert!(screen.contains("CLAUDE.local.md     ○ not found"));
        assert!(!screen.contains("would replace"));
        assert!(!screen.contains("would load after"));
        assert!(screen.contains("mdmanager docs start"));
        let lines = screen.lines().collect::<Vec<_>>();
        let box_bottom = lines.iter().rposition(|line| line.contains('└')).unwrap();
        let guide = lines
            .iter()
            .position(|line| line.contains("mdmanager docs start"))
            .unwrap();
        let context = lines
            .iter()
            .position(|line| line.contains("CONTEXT"))
            .unwrap();
        let menu = lines
            .iter()
            .position(|line| line.contains("↑↓ select"))
            .unwrap();
        assert!(guide < context);
        assert_eq!(menu, box_bottom + 2);

        app.message = Some((true, "Updated from disk".into()));
        let screen = draw(&mut app);
        assert!(screen.contains("Updated from disk"));
    }

    #[test]
    fn global_external_files_use_the_same_ownership_wording() {
        let (home, _repository, _paths, mut app) = fixture();
        fs::create_dir_all(home.path().join(".claude")).unwrap();
        fs::write(home.path().join(".claude/CLAUDE.md"), "# Global\n").unwrap();

        app.reload_from_disk();
        let screen = draw(&mut app);

        let global = screen
            .lines()
            .find(|line| line.contains("~/.claude/CLAUDE.md"))
            .unwrap();
        assert!(global.contains("existing · not managed by mdmanager.ai"));
    }

    #[test]
    fn home_width_is_responsive_and_documents_use_the_screen() {
        let (_home, repository, _paths, mut app) = fixture();
        assert_eq!(
            home_canvas_width(
                &OverviewInput {
                    global: &app.global,
                    global_active_profile: &app.global_active_profile,
                    global_sources: &app.global_sources,
                    inspection: &app.inspection,
                    is_home: matches!(app.view, View::Home),
                    local_repository: &app.local_repository,
                    message: &app.message,
                    paths: &app.paths,
                    project: &app.project,
                    project_root: &app.project_root,
                    scan_spinner: app.context.scan_spinner(),
                    scan_status: &app.context.scan_status
                },
                60
            ),
            60
        );
        assert!(
            home_canvas_width(
                &OverviewInput {
                    global: &app.global,
                    global_active_profile: &app.global_active_profile,
                    global_sources: &app.global_sources,
                    inspection: &app.inspection,
                    is_home: matches!(app.view, View::Home),
                    local_repository: &app.local_repository,
                    message: &app.message,
                    paths: &app.paths,
                    project: &app.project,
                    project_root: &app.project_root,
                    scan_spinner: app.context.scan_spinner(),
                    scan_status: &app.context.scan_status
                },
                180
            ) < COMPACT_WIDTH
        );
        let home = draw_at(&mut app, 180, 60);
        let home_title = home
            .lines()
            .find(|line| line.contains("┌ mdmanager.ai "))
            .unwrap();
        assert!(
            home_title
                .chars()
                .position(|character| character == '┌')
                .unwrap()
                > 20
        );
        app.open(View::File {
            title: "AGENTS.md".into(),
            path: repository.path().join("AGENTS.md"),
            about: "Project file".into(),
        });
        let document = draw_at(&mut app, 180, 60);
        let title = document
            .lines()
            .find(|line| line.contains("┌ mdmanager.ai "))
            .unwrap();
        assert!(
            title
                .chars()
                .position(|character| character == '┌')
                .unwrap()
                <= 1
        );
    }

    #[test]
    fn home_shows_the_global_summary_and_no_active_profile() {
        let (_home, _repository, _paths, mut app) = global_fixture();
        let screen = draw(&mut app);
        assert!(screen.contains("GLOBAL INSTRUCTIONS · no active Profile"));
        assert!(screen.contains("2 Profiles · 3 Sections"));
        assert!(screen.contains("not applied yet"));
        assert!(!screen.contains("not activated"));
        assert!(!screen.contains("out of sync"));

        app.home_index = app
            .home_items()
            .iter()
            .position(|item| home_item_key(item) == "global:claude")
            .unwrap();
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(app.view, View::Info { .. }));
        let screen = draw(&mut app);
        assert!(screen.contains("No Global Profile is active"));
    }

    #[test]
    fn configured_global_symlink_compares_destination_with_the_active_profile() {
        use std::os::unix::fs::symlink;

        let (home, repository, paths, _app) = global_fixture();
        fs::create_dir_all(home.path().join(".claude")).unwrap();
        fs::create_dir_all(home.path().join(".codex")).unwrap();
        let global = GlobalConfig::load(&paths).unwrap();
        deploy::apply(&global, "default", None, false, true).unwrap();
        let claude = home.path().join(".claude/CLAUDE.md");
        fs::remove_file(&claude).unwrap();
        symlink(home.path().join(".codex/AGENTS.md"), &claude).unwrap();
        let app = App::new_at(paths, Some(global), repository.path().to_owned()).unwrap();
        let info = symlink_info(&app.inspection, &claude).unwrap();

        let status = global_symlink_home_status(
            &OverviewInput {
                global: &app.global,
                global_active_profile: &app.global_active_profile,
                global_sources: &app.global_sources,
                inspection: &app.inspection,
                is_home: matches!(app.view, View::Home),
                local_repository: &app.local_repository,
                message: &app.message,
                paths: &app.paths,
                project: &app.project,
                project_root: &app.project_root,
                scan_spinner: app.context.scan_spinner(),
                scan_status: &app.context.scan_status,
            },
            "claude",
            &claude,
            &info,
        );
        assert!(status.contains("target managed"));
        assert!(status.contains("differs from Profile"));
    }
}
