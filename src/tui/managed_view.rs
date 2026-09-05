use std::path::PathBuf;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};

use super::active_theme;
use super::document::{DocumentState, render_document, render_plain_document};
use super::inspection::{
    InstructionInspection, ManagedRef, global_diagnostic_text, managed_workspace, symlink_info,
    symlink_target_status,
};
use crate::config::GlobalConfig;
use crate::local::LocalRepository;
use crate::project::Workspace;

/// Borrowed managed view inputs from the session's accepted observations.
pub(super) struct ManagedViewInput<'a> {
    pub(super) global: &'a Option<GlobalConfig>,
    pub(super) global_active_profile: &'a Option<String>,
    pub(super) inspection: &'a InstructionInspection,
    pub(super) local_repository: &'a Option<LocalRepository>,
    pub(super) project: &'a Option<Workspace>,
    pub(super) project_root: &'a PathBuf,
}

/// Renders the accepted composition and sections without rereading instruction files.
pub(super) fn render_managed(
    frame: &mut Frame<'_>,
    input: &ManagedViewInput<'_>,
    document: &mut DocumentState,
    section_index: &mut usize,
    area: Rect,
    reference: &ManagedRef,
) {
    let workspace = match managed_workspace(&input.inspection.managed, reference) {
        Ok(workspace) => workspace,
        Err(error) => {
            let error = if matches!(reference, ManagedRef::Global { .. }) {
                global_diagnostic_text(&error)
            } else {
                error
            };
            render_document(
                frame,
                document,
                area,
                "Managed target unavailable",
                "",
                &error,
                false,
            );
            return;
        }
    };
    let entries = workspace.sections.len() + 1;
    (*section_index) = (*section_index).min(entries.saturating_sub(1));
    let link = symlink_info(input.inspection, &workspace.target);
    let mut metadata = vec![Line::from(vec![
        Span::styled("Target       ", Style::new().fg(active_theme().muted)),
        Span::raw(workspace.target.display().to_string()),
    ])];
    if let Some(info) = &link {
        metadata.extend([
            Line::from(vec![
                Span::styled("Type         ", Style::new().fg(active_theme().muted)),
                Span::raw("symlink"),
            ]),
            Line::from(vec![
                Span::styled("Points to    ", Style::new().fg(active_theme().muted)),
                Span::raw(info.target.display().to_string()),
            ]),
            Line::from(vec![
                Span::styled("Destination  ", Style::new().fg(active_theme().muted)),
                Span::raw(symlink_target_status(
                    input
                        .global
                        .as_ref()
                        .zip(input.global_active_profile.as_deref()),
                    input.inspection,
                    input.local_repository,
                    input.project,
                    input.project_root,
                    &workspace.target,
                    info,
                )),
            ]),
            Line::from(vec![
                Span::styled("Link         ", Style::new().fg(active_theme().muted)),
                Span::raw("not managed by mdmanager.ai"),
            ]),
            Line::from(vec![
                Span::styled("Contents     ", Style::new().fg(active_theme().muted)),
                Span::raw(if workspace.difference.is_none() {
                    "matches composition"
                } else {
                    "differs from composition"
                }),
            ]),
        ]);
    }
    metadata.extend([
        Line::from(vec![
            Span::styled("Status       ", Style::new().fg(active_theme().muted)),
            Span::raw(workspace.status.clone()),
        ]),
        Line::from(vec![
            Span::styled("Composition  ", Style::new().fg(active_theme().muted)),
            Span::raw(format!(
                "{} Section{} · read-only",
                workspace.sections.len(),
                if workspace.sections.len() == 1 {
                    ""
                } else {
                    "s"
                }
            )),
        ]),
    ]);
    let metadata_height = u16::try_from(metadata.len())
        .unwrap_or(u16::MAX)
        .saturating_add(2);
    let [metadata_area, _, body] = Layout::vertical([
        Constraint::Length(metadata_height),
        Constraint::Length(1),
        Constraint::Min(3),
    ])
    .areas(area);
    frame.render_widget(
        Paragraph::new(metadata)
            .block(
                Block::default()
                    .title(format!(" {} ", workspace.title))
                    .border_style(Style::new().fg(active_theme().dim))
                    .borders(Borders::ALL),
            )
            .wrap(Wrap { trim: false }),
        metadata_area,
    );
    let mut rows = vec![ListItem::new("  Generated document")];
    rows.extend(
        workspace
            .sections
            .iter()
            .enumerate()
            .map(|(index, section)| ListItem::new(format!("  {} {}", index + 1, section.name))),
    );
    if body.width >= 100 {
        let [list_area, _, preview_area] = Layout::horizontal([
            Constraint::Percentage(36),
            Constraint::Length(1),
            Constraint::Min(40),
        ])
        .areas(body);
        let mut state = ListState::default().with_selected(Some(*section_index));
        frame.render_stateful_widget(
            List::new(rows)
                .block(
                    Block::default()
                        .title(" Composition ")
                        .borders(Borders::ALL),
                )
                .highlight_style(active_theme().selected())
                .highlight_symbol("▸ "),
            list_area,
            &mut state,
        );
        let (title, content) = if (*section_index) == 0 {
            (" Generated document ".into(), workspace.rendered.clone())
        } else {
            workspace.sections.get((*section_index) - 1).map_or_else(
                || (" Section ".into(), String::new()),
                |section| (format!(" {} ", section.name), section.content.clone()),
            )
        };
        render_plain_document(frame, document, preview_area, &title, &content, true, false);
    } else {
        document.max_scroll = 0;
        let mut state = ListState::default().with_selected(Some(*section_index));
        frame.render_stateful_widget(
            List::new(rows)
                .block(
                    Block::default()
                        .title(" Composition ")
                        .borders(Borders::ALL),
                )
                .highlight_style(active_theme().selected())
                .highlight_symbol("▸ "),
            body,
            &mut state,
        );
    }
}

/// Differences use the same accepted managed observation as the composition browser.
pub(super) fn render_diff(
    frame: &mut Frame<'_>,
    input: &ManagedViewInput<'_>,
    document: &mut DocumentState,
    area: Rect,
    reference: &ManagedRef,
) {
    match managed_workspace(&input.inspection.managed, reference) {
        Ok(workspace) => {
            let about = format!(
                "{}\nGenerated composition compared with the file on disk.\nAsk your coding agent to inspect and apply the intended change.",
                workspace.status
            );
            let [about_area, _, diff_area] = Layout::vertical([
                Constraint::Length(5),
                Constraint::Length(1),
                Constraint::Min(3),
            ])
            .areas(area);
            frame.render_widget(
                Paragraph::new(about)
                    .block(
                        Block::default()
                            .title(" Difference ")
                            .title_style(Style::new().fg(active_theme().secondary))
                            .border_style(Style::new().fg(active_theme().dim))
                            .borders(Borders::ALL),
                    )
                    .wrap(Wrap { trim: false }),
                about_area,
            );
            render_plain_document(
                frame,
                document,
                diff_area,
                " Unified difference ",
                workspace.difference.as_deref().unwrap_or_default(),
                false,
                false,
            );
        }
        Err(error) => render_document(frame, document, area, "Difference", "", &error, false),
    }
}

/// Generated documents use the same accepted composition as the managed browser.
pub(super) fn render_managed_document(
    frame: &mut Frame<'_>,
    inspection: &InstructionInspection,
    document: &mut DocumentState,
    body: Rect,
    reference: &ManagedRef,
) {
    match managed_workspace(&inspection.managed, reference) {
        Ok(workspace) => {
            let about = format!(
                "Managed target · {}\nStatus: {}\nPath: {}\nGenerated from {} Section{}",
                workspace.title,
                workspace.status,
                workspace.target.display(),
                workspace.sections.len(),
                if workspace.sections.len() == 1 {
                    ""
                } else {
                    "s"
                }
            );
            render_document(
                frame,
                document,
                body,
                "Generated document",
                &about,
                &workspace.rendered,
                true,
            );
        }
        Err(error) => {
            render_document(frame, document, body, "Managed target", "", &error, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{draw, draw_at, fixture};
    use super::super::*;
    use super::*;
    use std::fs;

    #[test]
    fn managed_project_exposes_composition_and_document() {
        let (_home, repository, paths, _app) = fixture();
        project::adopt(repository.path(), "agents").unwrap();
        let mut app = App::new_at(paths, None, repository.path().to_owned()).unwrap();
        app.open(View::Managed(ManagedRef::Project("agents".into())));
        let screen = draw(&mut app);
        assert!(screen.contains("Generated document"));
        assert!(screen.contains("Composition"));
    }

    #[test]
    fn managed_difference_toggles_back_to_composition() {
        let (_home, repository, paths, _app) = fixture();
        let project = project::adopt(repository.path(), "agents").unwrap();
        fs::write(
            project.section_path("agents").unwrap(),
            "# Changed by agent\n",
        )
        .unwrap();
        let mut app = App::new_at(paths, None, repository.path().to_owned()).unwrap();
        app.open(View::Managed(ManagedRef::Project("agents".into())));
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
        );
        assert!(matches!(app.view, View::Diff(_)));
        let screen = draw_at(&mut app, 160, 40);
        assert!(screen.contains("Unified difference"));
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
        );
        assert!(matches!(app.view, View::Managed(_)));
    }

    #[test]
    fn narrow_managed_view_does_not_advertise_preview_scrolling() {
        let (_home, repository, paths, _app) = fixture();
        project::adopt(repository.path(), "agents").unwrap();
        let mut app = App::new_at(paths, None, repository.path().to_owned()).unwrap();
        app.open(View::Managed(ManagedRef::Project("agents".into())));

        let screen = draw_at(&mut app, 80, 30);
        assert!(!screen.contains("preview"));
    }
}
