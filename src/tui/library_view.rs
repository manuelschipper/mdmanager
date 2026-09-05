use std::collections::HashMap;
use std::path::Path;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::active_theme;
use super::document::{
    COMPACT_WIDTH, DocumentState, bounded, fit_column, pad_right, render_info, truncate_end,
};
use super::inspection::{ManagedRef, ManagedWorkspace, managed_workspace};
use crate::config::{GlobalConfig, target_display_name};
use crate::project::{self, Workspace};

/// Borrowed library view inputs from the session's accepted observations.
pub(super) struct LibraryViewInput<'a> {
    pub(super) global: &'a Option<GlobalConfig>,
    pub(super) global_active_profile: &'a Option<String>,
    pub(super) managed: &'a [(ManagedRef, Result<ManagedWorkspace, String>)],
    pub(super) personal_section_targets: &'a HashMap<String, Vec<&'static str>>,
    pub(super) project: &'a Option<Workspace>,
}

#[derive(Clone, PartialEq)]
pub(super) enum LibraryEntry {
    Profile(String),
    PersonalSection(String),
    ProjectSection(String),
}

pub(super) fn personal_section_usage(input: &LibraryViewInput<'_>, id: &str) -> String {
    let profiles = input.global.as_ref().map_or_else(Vec::new, |global| {
        global
            .manifest
            .profiles
            .iter()
            .filter(|(_, targets)| {
                targets
                    .values()
                    .any(|ids| ids.iter().any(|entry| entry == id))
            })
            .map(|(profile, _)| profile.as_str())
            .collect::<Vec<_>>()
    });
    let local_targets = input
        .personal_section_targets
        .get(id)
        .cloned()
        .unwrap_or_default();
    let mut usage = if profiles.is_empty() {
        String::new()
    } else if profiles.len() > 1
        && input
            .global
            .as_ref()
            .is_some_and(|global| profiles.len() == global.manifest.profiles.len())
    {
        "used by all Profiles".into()
    } else {
        format!("used by {}", profiles.join(", "))
    };
    if !local_targets.is_empty() {
        if !usage.is_empty() {
            usage.push_str(" · ");
        }
        usage.push_str(&format!("private in {}", local_targets.join(", ")));
    }
    if usage.is_empty() {
        "not currently used".into()
    } else {
        usage
    }
}

pub(super) fn project_section_usage(project: &Workspace, id: &str) -> String {
    let targets = project
        .manifest
        .targets
        .iter()
        .filter(|(_, target)| target.sections.iter().any(|section| section == id))
        .map(|(target, _)| project::target_filename(target).expect("validated Project target"))
        .collect::<Vec<_>>();
    if targets.is_empty() {
        "not currently used".into()
    } else {
        targets.join(", ")
    }
}

pub(super) fn profile_deploys(global: &GlobalConfig, profile: &str, target: &str) -> bool {
    global
        .profile_target_names(profile)
        .ok()
        .is_some_and(|ids| ids.collect::<Vec<_>>().contains(&target))
}

pub(super) fn profile_target_reference(
    input: &LibraryViewInput<'_>,
    profile: &str,
    index: usize,
) -> Option<ManagedRef> {
    input.global.as_ref().and_then(|global| {
        let target = global.target_names().nth(index)?;
        profile_deploys(global, profile, target).then(|| ManagedRef::Global {
            profile: profile.to_owned(),
            target: target.to_owned(),
        })
    })
}

/// Browses Profiles and both Personal and Project Sections without changing configuration.
pub(super) fn render_library(
    frame: &mut Frame<'_>,
    input: &LibraryViewInput<'_>,
    document: &mut DocumentState,
    section_index: &mut usize,
    screen: Rect,
) {
    document.scroll = 0;
    document.max_scroll = 0;
    let entries = library_entries(input);
    if entries.is_empty() {
        render_info(
            frame,
            document,
            screen,
            "Library",
            "Global configuration is unavailable.",
        );
        return;
    }
    (*section_index) = (*section_index).min(entries.len() - 1);
    let row_width = usize::from(screen.width.min(COMPACT_WIDTH).saturating_sub(4));
    let content_width = row_width.saturating_sub(2);
    let name_width = if content_width >= 70 {
        18
    } else {
        (content_width / 3).max(12)
    };
    let path_width = if content_width >= 70 {
        26
    } else {
        (content_width.saturating_sub(name_width) / 2).max(12)
    };
    let usage_width = content_width
        .saturating_sub(name_width)
        .saturating_sub(path_width)
        .saturating_sub(2);
    let mut rows = Vec::new();
    let mut selected_row = 0;
    let mut previous_group = "";
    for (index, entry) in entries.iter().enumerate() {
        let group = match entry {
            LibraryEntry::Profile(_) => "PROFILES",
            LibraryEntry::PersonalSection(_) => "PERSONAL SECTIONS",
            LibraryEntry::ProjectSection(_) => "THIS PROJECT",
        };
        if group != previous_group {
            if !rows.is_empty() {
                rows.push(ListItem::new(""));
            }
            let count = entries
                .iter()
                .filter(|candidate| {
                    matches!(
                        (entry, candidate),
                        (LibraryEntry::Profile(_), LibraryEntry::Profile(_))
                            | (
                                LibraryEntry::PersonalSection(_),
                                LibraryEntry::PersonalSection(_)
                            )
                            | (
                                LibraryEntry::ProjectSection(_),
                                LibraryEntry::ProjectSection(_)
                            )
                    )
                })
                .count();
            rows.push(ListItem::new(Line::styled(
                format!("{group} · {count}"),
                Style::new()
                    .fg(active_theme().secondary)
                    .add_modifier(Modifier::BOLD),
            )));
            previous_group = group;
        }
        if index == (*section_index) {
            selected_row = rows.len();
        }
        let line = match entry {
            LibraryEntry::Profile(profile) => {
                let badge = if input.global_active_profile.as_deref() == Some(profile.as_str()) {
                    "active"
                } else {
                    "not active"
                };
                Line::raw(format!("  {} {badge}", pad_right(profile, 16)))
            }
            LibraryEntry::PersonalSection(id) => {
                let (name, path) = input
                    .global
                    .as_ref()
                    .and_then(|global| {
                        global
                            .section(id)
                            .map(|section| (section.name.clone(), section.path.clone()))
                    })
                    .unwrap_or_else(|| (id.clone(), String::new()));
                Line::raw(format!(
                    "  {} {} {}",
                    fit_column(&name, name_width),
                    fit_column(&path, path_width),
                    fit_column(&personal_section_usage(input, id), usage_width)
                ))
            }
            LibraryEntry::ProjectSection(id) => {
                let (name, path) = input
                    .project
                    .as_ref()
                    .and_then(|project| {
                        project
                            .section(id)
                            .map(|section| (section.name.clone(), section.path.clone()))
                    })
                    .unwrap_or_else(|| (id.clone(), String::new()));
                let usage = input.project.as_ref().map_or_else(
                    || "not currently used".into(),
                    |project| project_section_usage(project, id),
                );
                Line::raw(format!(
                    "  {} {} {}",
                    fit_column(&name, name_width),
                    fit_column(&path, path_width),
                    fit_column(&usage, usage_width)
                ))
            }
        };
        rows.push(ListItem::new(line));
    }
    let area = library_panel_area(screen, rows.len());
    let block = Block::default()
        .title(" mdmanager.ai · Library ")
        .title_style(
            Style::new()
                .fg(active_theme().primary)
                .add_modifier(Modifier::BOLD),
        )
        .border_style(Style::new().fg(active_theme().primary))
        .style(Style::new().bg(active_theme().surface_raised))
        .borders(Borders::ALL);
    let inner = block.inner(area);
    let [list_area, controls_area] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).areas(inner);
    frame.render_widget(block, area);
    let mut state = ListState::default().with_selected(Some(selected_row));
    frame.render_stateful_widget(
        List::new(rows)
            .highlight_style(active_theme().selected())
            .highlight_symbol("▸ "),
        list_area,
        &mut state,
    );
    frame.render_widget(
        Paragraph::new("↑↓ select · Enter inspect · Esc back")
            .block(Block::default().borders(Borders::TOP))
            .alignment(Alignment::Center)
            .style(Style::new().fg(active_theme().muted)),
        controls_area,
    );
}

/// The named Profile is the browsing identity, independent of the active Profile.
pub(super) fn render_global_profile(
    frame: &mut Frame<'_>,
    input: &LibraryViewInput<'_>,
    document: &mut DocumentState,
    section_index: &mut usize,
    screen: Rect,
    profile: &str,
) {
    document.scroll = 0;
    document.max_scroll = 0;
    let Some((active, targets)) = input.global.as_ref().and_then(|global| {
        global.manifest.profiles.contains_key(profile).then(|| {
            let active = input.global_active_profile.as_deref() == Some(profile);
            let deployed: Vec<&str> = global
                .profile_target_names(profile)
                .map(|ids| ids.collect())
                .unwrap_or_default();
            let targets = global
                .target_names()
                .map(|id| {
                    let path = global.manifest.targets[id].path.clone();
                    if !deployed.contains(&id) {
                        return (
                            id.to_owned(),
                            path,
                            "—".into(),
                            "not in this Profile".into(),
                        );
                    }
                    let composition = global
                        .composition(profile, id)
                        .map_or_else(|_| "invalid".into(), |ids| ids.join(" + "));
                    let status = managed_workspace(
                        input.managed,
                        &ManagedRef::Global {
                            profile: profile.to_owned(),
                            target: id.to_owned(),
                        },
                    )
                    .map_or_else(|_| "invalid".into(), |view| view.status);
                    (id.to_owned(), path, composition, status)
                })
                .collect::<Vec<_>>();
            (active, targets)
        })
    }) else {
        render_info(
            frame,
            document,
            screen,
            "Global Profile",
            "The Global Profile is unavailable.",
        );
        return;
    };
    (*section_index) = (*section_index).min(targets.len().saturating_sub(1));
    let area = profile_panel_area(screen, targets.len());
    let row_width = usize::from(area.width.saturating_sub(4));
    let content_width = row_width.saturating_sub(2);
    let target_width = targets
        .iter()
        .map(|(id, _, _, _)| UnicodeWidthStr::width(target_display_name(id).as_str()))
        .max()
        .unwrap_or(0)
        .min(if content_width >= 60 { 12 } else { 8 });
    let filename_width = targets
        .iter()
        .map(|(_, path, _, _)| {
            let filename = Path::new(path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(path);
            UnicodeWidthStr::width(filename)
        })
        .max()
        .unwrap_or(0)
        .min(if content_width >= 60 { 18 } else { 14 });
    let remaining = content_width
        .saturating_sub(target_width)
        .saturating_sub(filename_width)
        .saturating_sub(3);
    let desired_status_width = targets
        .iter()
        .map(|(_, _, _, status)| UnicodeWidthStr::width(status.as_str()))
        .max()
        .unwrap_or(0);
    let minimum_composition_width = if remaining >= 40 { 24 } else { remaining / 2 };
    let status_width =
        desired_status_width.min(remaining.saturating_sub(minimum_composition_width));
    let composition_width = remaining.saturating_sub(status_width);
    let rows = targets
        .iter()
        .map(|(id, path, composition, status)| {
            let filename = Path::new(path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(path);
            ListItem::new(format!(
                "  {} {} {} {status}",
                fit_column(&target_display_name(id), target_width),
                fit_column(filename, filename_width),
                fit_column(composition, composition_width),
                status = truncate_end(status, status_width)
            ))
        })
        .collect::<Vec<_>>();
    let block = Block::default()
        .title(format!(
            " mdmanager.ai · Profile {profile} · {} ",
            if active { "active" } else { "not active" }
        ))
        .title_style(
            Style::new()
                .fg(active_theme().primary)
                .add_modifier(Modifier::BOLD),
        )
        .border_style(Style::new().fg(active_theme().primary))
        .style(Style::new().bg(active_theme().surface_raised))
        .borders(Borders::ALL);
    let inner = block.inner(area);
    let [intro_area, list_area, controls_area] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(2),
    ])
    .areas(inner);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(Text::from(vec![
            Line::styled(
                "A Profile deploys the catalog targets it names.",
                Style::new().fg(active_theme().muted),
            ),
            Line::styled(
                "COMPOSITIONS",
                Style::new()
                    .fg(active_theme().secondary)
                    .add_modifier(Modifier::BOLD),
            ),
        ])),
        intro_area,
    );
    let mut state = ListState::default().with_selected(Some(*section_index));
    frame.render_stateful_widget(
        List::new(rows)
            .highlight_style(active_theme().selected())
            .highlight_symbol("▸ "),
        list_area,
        &mut state,
    );
    let difference = if profile_target_reference(input, profile, *section_index)
        .and_then(|reference| managed_workspace(input.managed, &reference).ok())
        .is_some_and(|workspace| workspace.difference.is_some())
    {
        " · d difference"
    } else {
        ""
    };
    frame.render_widget(
        Paragraph::new(format!("↑↓ select · Enter open{difference} · Esc back"))
            .block(Block::default().borders(Borders::TOP))
            .alignment(Alignment::Center)
            .style(Style::new().fg(active_theme().muted)),
        controls_area,
    );
}

pub(super) fn library_panel_area(screen: Rect, row_count: usize) -> Rect {
    let height = u16::try_from(row_count)
        .unwrap_or(u16::MAX)
        .saturating_add(4)
        .clamp(8, 30)
        .min(screen.height.saturating_sub(4).max(8));
    bounded(screen, COMPACT_WIDTH, height)
}

pub(super) fn profile_panel_area(screen: Rect, row_count: usize) -> Rect {
    let height = u16::try_from(row_count)
        .unwrap_or(u16::MAX)
        .saturating_add(6)
        .clamp(9, 30)
        .min(screen.height.saturating_sub(4).max(9));
    bounded(screen, COMPACT_WIDTH, height)
}

pub(super) fn library_entries(input: &LibraryViewInput<'_>) -> Vec<LibraryEntry> {
    let mut entries = input.global.as_ref().map_or_else(Vec::new, |global| {
        global
            .profile_names()
            .map(|profile| LibraryEntry::Profile(profile.to_owned()))
            .chain(
                global
                    .manifest
                    .sections
                    .iter()
                    .map(|section| LibraryEntry::PersonalSection(section.id.clone())),
            )
            .collect()
    });
    if let Some(project) = &input.project {
        entries.extend(
            project
                .manifest
                .sections
                .iter()
                .map(|section| LibraryEntry::ProjectSection(section.id.clone())),
        );
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::super::tests::{draw, draw_at, global_fixture, open_library};
    use super::super::*;
    use super::*;
    use std::fs;

    #[test]
    fn library_lists_every_profile_and_section_with_usage() {
        let (_home, _repository, _paths, mut app) = global_fixture();
        open_library(&mut app);
        let screen = draw(&mut app);
        assert!(screen.contains("mdmanager.ai · Library"));
        assert!(screen.contains("PROFILES"));
        assert!(screen.contains("SECTIONS"));
        assert!(screen.contains("default"));
        assert!(screen.contains("work"));
        assert!(screen.contains("not active"));
        assert!(screen.contains("sections/scratch.md"));
        assert!(screen.contains("used by all Profiles"));
        assert!(screen.contains("used by default"));
        assert!(screen.contains("not currently used"));
        assert!(!screen.contains("apply"));

        let wide = draw_at(&mut app, 160, 50);
        let title = wide
            .lines()
            .find(|line| line.contains("mdmanager.ai · Library"))
            .unwrap();
        let left = title
            .chars()
            .position(|character| character == '┌')
            .unwrap();
        let right = title
            .chars()
            .position(|character| character == '┐')
            .unwrap();
        assert!(left > 20);
        assert!(right - left <= usize::from(COMPACT_WIDTH));
        assert!(!wide.contains("PROJECT INSTRUCTIONS"));

        handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(app.view, View::Home));
    }

    #[test]
    fn library_section_opens_with_usage_metadata() {
        let (_home, _repository, _paths, mut app) = global_fixture();
        open_library(&mut app);
        app.section_index = 4;
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(app.view, View::SectionDocument { .. }));
        let screen = draw(&mut app);
        assert!(screen.contains("Used by: not currently used"));
        assert!(screen.contains("# Scratch"));
    }

    #[test]
    fn library_includes_unused_and_used_project_sections() {
        let (_home, repository, _paths, mut app) = global_fixture();
        fs::write(repository.path().join("AGENTS.md"), "# Project agents\n").unwrap();
        project::adopt(repository.path(), "agents").unwrap();
        let manifest = repository.path().join(".mdmanager/project.toml");
        let mut source = fs::read_to_string(&manifest).unwrap();
        source.push_str(
            "\n[[sections]]\nid = \"draft\"\nname = \"Draft\"\npath = \"sections/draft.md\"\n",
        );
        fs::write(&manifest, source).unwrap();
        fs::write(
            repository.path().join(".mdmanager/sections/draft.md"),
            "# Draft\n",
        )
        .unwrap();
        app.reload_from_disk();
        open_library(&mut app);

        let screen = draw(&mut app);
        assert!(screen.contains("THIS PROJECT · 2"));
        assert!(screen.contains("Agents"));
        assert!(screen.contains("AGENTS.md"));
        assert!(screen.contains("Draft"));
        assert!(screen.contains("not currently used"));

        app.section_index = library_entries(&LibraryViewInput {
            global: &app.global,
            global_active_profile: &app.global_active_profile,
            managed: &app.inspection.managed,
            personal_section_targets: &app.inspection.personal_section_targets,
            project: &app.project,
        })
        .len()
            - 1;
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(app.view, View::SectionDocument { .. }));
        let screen = draw(&mut app);
        assert!(screen.contains("Project Section · committed with this repository"));
        assert!(screen.contains("Used by: not currently used"));
    }

    #[test]
    fn browsing_an_inactive_profile_uses_comparison_wording() {
        let (_home, _repository, _paths, mut app) = global_fixture();
        deploy::apply(app.global.as_ref().unwrap(), "default", None, false, true).unwrap();
        app.reload_from_disk();
        assert!(draw(&mut app).contains("GLOBAL INSTRUCTIONS · PROFILE default (active)"));

        open_library(&mut app);
        assert!(draw(&mut app).contains("active"));
        app.section_index = 1;
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(&app.view, View::GlobalProfile(profile) if profile == "work"));
        let screen = draw(&mut app);
        assert!(screen.contains("Profile work · not active"));
        assert!(screen.contains("differs from current file"));
        assert!(screen.contains("matches current file"));
        assert!(!screen.contains("out of sync"));

        let wide = draw_at(&mut app, 160, 50);
        let title = wide
            .lines()
            .find(|line| line.contains("mdmanager.ai · Profile work · not active"))
            .unwrap();
        let left = title
            .chars()
            .position(|character| character == '┌')
            .unwrap();
        let right = title
            .chars()
            .position(|character| character == '┐')
            .unwrap();
        assert!(left > 20);
        assert!(right - left <= usize::from(COMPACT_WIDTH));
        assert!(!wide.contains("PROJECT INSTRUCTIONS"));

        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(
            &app.view,
            View::Managed(ManagedRef::Global { profile, target })
                if profile == "work" && target == "claude"
        ));
        let screen = draw(&mut app);
        assert!(screen.contains("(not active)"));
        assert!(screen.contains("differs from current file"));

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
        );
        assert!(matches!(app.view, View::Diff(_)));
        assert!(draw_at(&mut app, 160, 40).contains("Unified difference"));

        for expected in ["managed", "profile", "library", "home"] {
            handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
            match expected {
                "managed" => assert!(matches!(app.view, View::Managed(_))),
                "profile" => assert!(matches!(app.view, View::GlobalProfile(_))),
                "library" => assert!(matches!(app.view, View::LibraryBrowser)),
                _ => assert!(matches!(app.view, View::Home)),
            }
        }
    }

    #[test]
    fn the_active_profile_keeps_deployment_status_wording() {
        let (_home, _repository, _paths, mut app) = global_fixture();
        deploy::apply(app.global.as_ref().unwrap(), "default", None, false, true).unwrap();
        app.reload_from_disk();
        open_library(&mut app);
        app.section_index = 0;
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(&app.view, View::GlobalProfile(profile) if profile == "default"));
        let screen = draw(&mut app);
        assert!(screen.contains("Profile default · active"));
        assert!(screen.contains("current"));
        assert!(!screen.contains("matches current file"));
        assert!(!screen.contains("differs from current file"));
    }

    #[test]
    fn wide_profile_rows_show_the_full_composition() {
        let (_home, _repository, paths, mut app) = global_fixture();
        let source = fs::read_to_string(&paths.config).unwrap().replace(
            "claude = [\"common\", \"claude\"]",
            "claude = [\"common\", \"claude\", \"delegation-rules\"]",
        ) + "\n[[sections]]\nid = \"delegation-rules\"\nname = \"Delegation Rules\"\npath = \"sections/delegation-rules.md\"\n";
        fs::write(&paths.config, source).unwrap();
        fs::write(
            paths
                .config
                .parent()
                .unwrap()
                .join("sections/delegation-rules.md"),
            "# Delegation Rules\n",
        )
        .unwrap();
        app.reload_from_disk();
        open_library(&mut app);
        app.section_index = 0;
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        let screen = draw_at(&mut app, 160, 50);
        assert!(screen.contains("common + claude + delegation-rules"));
        let reference = ManagedRef::Global {
            profile: "default".into(),
            target: "claude".into(),
        };
        app.open(View::Managed(reference.clone()));
        app.section_index = 2;
        let selected = managed_workspace(&app.inspection.managed, &reference)
            .unwrap()
            .sections[1]
            .path
            .clone();
        let source = fs::read_to_string(&paths.config).unwrap().replace(
            "claude = [\"common\", \"claude\", \"delegation-rules\"]",
            "claude = [\"claude\", \"common\", \"delegation-rules\"]",
        );
        fs::write(&paths.config, source).unwrap();
        app.reload_from_disk();
        assert_eq!(
            managed_workspace(&app.inspection.managed, &reference)
                .unwrap()
                .sections[app.section_index - 1]
                .path,
            selected
        );
    }
}
