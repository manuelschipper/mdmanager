use super::inspection::loaded_at_startup;
use std::path::PathBuf;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use super::active_theme;
use super::context_ui::ContextScanStatus;
use super::document::{
    DocumentState, bounded, pad_right, relative_message, render_document, render_plain_document,
    truncate_end,
};
use super::inspection::HomeItem;
use super::inspection::{
    InstructionInspection, ManagedRef, managed_workspace, symlink_info, symlink_target_status,
};
use crate::config::{GlobalConfig, Paths};
use crate::context::{
    Audit, ContextRuntime, ContextSource, SourceGroup, SourceState, display_path,
};
use crate::local::{self, LocalRepository};
use crate::project::Workspace;

/// Borrowed context view inputs from the session's accepted observations.
pub(super) struct ContextViewInput<'a> {
    pub(super) context_entry_index: usize,
    pub(super) context_nested_expanded: bool,
    pub(super) global: &'a Option<GlobalConfig>,
    pub(super) global_active_profile: &'a Option<String>,
    pub(super) inspection: &'a InstructionInspection,
    pub(super) local_repository: &'a Option<LocalRepository>,
    pub(super) paths: &'a Paths,
    pub(super) project: &'a Option<Workspace>,
    pub(super) project_root: &'a PathBuf,
    pub(super) scan_spinner: Option<&'static str>,
    pub(super) scan_status: &'a ContextScanStatus,
    pub(super) source_index: usize,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum ContextEntry {
    Source(usize),
    Nested,
}

/// The chain stays selected while its preview appears beside it or below it when space permits.
pub(super) fn render_context(
    frame: &mut Frame<'_>,
    input: &ContextViewInput<'_>,
    document: &mut DocumentState,
    context_preview: &mut bool,
    area: Rect,
) {
    let scan_in_progress = input.inspection.context.runtime == ContextRuntime::Claude
        && matches!(
            input.scan_status,
            ContextScanStatus::NotStarted | ContextScanStatus::Scanning
        );
    let has_scan_errors = match input.scan_status {
        ContextScanStatus::Failed => true,
        ContextScanStatus::Complete { unreadable } => !unreadable.is_empty(),
        _ => false,
    };
    if input.inspection.context.sources.is_empty()
        && !scan_in_progress
        && input.inspection.context.warnings.is_empty()
        && !has_scan_errors
    {
        (*context_preview) = false;
        document.max_scroll = 0;
        frame.render_widget(
            Paragraph::new("No persistent Markdown instructions found for this runtime.")
                .block(
                    Block::default()
                        .title(format!(
                            " {} · resolved load chain ",
                            input.inspection.context.runtime.label()
                        ))
                        .title_style(
                            Style::new()
                                .fg(active_theme().secondary)
                                .add_modifier(Modifier::BOLD),
                        )
                        .border_style(Style::new().fg(active_theme().dim))
                        .style(Style::new().bg(active_theme().surface))
                        .borders(Borders::ALL),
                )
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: false }),
            bounded(area, 72, 5),
        );
        return;
    }
    let (list_area, detail_area) = if input.inspection.context.sources.is_empty() {
        (area, None)
    } else if area.width >= 100 {
        let [list, _, detail] = Layout::horizontal([
            Constraint::Percentage(45),
            Constraint::Length(1),
            Constraint::Min(40),
        ])
        .areas(area);
        (list, Some(detail))
    } else if area.height >= 30 {
        let [list, _, detail] = Layout::vertical([
            Constraint::Percentage(45),
            Constraint::Length(1),
            Constraint::Min(12),
        ])
        .areas(area);
        (list, Some(detail))
    } else {
        (area, None)
    };
    (*context_preview) = detail_area.is_some();
    let entries = context_entries(&input.inspection.context, input.context_nested_expanded);
    let selected = entries.get(input.context_entry_index).copied();
    let mut selected_row = None;
    let mut items = Vec::new();
    if input.inspection.context.sources.is_empty() && !scan_in_progress {
        items.push(ListItem::new(
            "No persistent Markdown instructions found for this runtime.",
        ));
        items.push(ListItem::new(""));
    }
    for warning in &input.inspection.context.warnings {
        items.push(ListItem::new(Line::styled(
            format!("  ⚠ {}", relative_message(input.project_root, warning)),
            Style::new().fg(active_theme().warning),
        )));
        items.push(ListItem::new(""));
    }
    for group in [
        SourceGroup::Startup,
        SourceGroup::PathFiltered,
        SourceGroup::Nested,
    ] {
        let source_indices = input
            .inspection
            .context
            .sources
            .iter()
            .enumerate()
            .filter(|(_, source)| source.group == group)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if group == SourceGroup::Nested
            && input.inspection.context.runtime != ContextRuntime::Claude
        {
            continue;
        }
        if source_indices.is_empty() && group == SourceGroup::PathFiltered {
            continue;
        }
        if group == SourceGroup::Nested {
            if source_indices.is_empty() {
                let before = items.len();
                push_scan_errors(&mut items, input);
                if items.len() > before {
                    items.push(ListItem::new(""));
                }
                continue;
            }
            if selected == Some(ContextEntry::Nested) {
                selected_row = Some(items.len());
            }
            let files = if source_indices.len() == 1 {
                "file"
            } else {
                "files"
            };
            items.push(ListItem::new(Line::styled(
                format!("{} · {} {files}", group.label(), source_indices.len()),
                Style::new().add_modifier(Modifier::BOLD),
            )));
            if !input.context_nested_expanded {
                push_scan_errors(&mut items, input);
                items.push(ListItem::new(""));
                continue;
            }
        } else {
            let count = if group == SourceGroup::Startup {
                source_indices
                    .iter()
                    .filter(|index| loaded_at_startup(&input.inspection.context.sources[**index]))
                    .count()
            } else {
                source_indices.len()
            };
            items.push(ListItem::new(Line::styled(
                format!("{} · {count}", group.label()),
                Style::new().add_modifier(Modifier::BOLD),
            )));
        }
        let mut startup_position = 0;
        for index in source_indices {
            if selected == Some(ContextEntry::Source(index)) {
                selected_row = Some(items.len());
            }
            let position = if group == SourceGroup::Startup
                && loaded_at_startup(&input.inspection.context.sources[index])
            {
                startup_position += 1;
                Some(startup_position)
            } else {
                None
            };
            items.push(context_source_item(
                &input.inspection.context.sources[index],
                position,
                usize::from(list_area.width.saturating_sub(4)),
            ));
        }
        if group == SourceGroup::Nested {
            push_scan_errors(&mut items, input);
        }
        items.push(ListItem::new(""));
    }
    let selected_source = match selected {
        Some(ContextEntry::Source(index)) => input.inspection.context.sources.get(index).cloned(),
        _ => None,
    };
    let mut state = ListState::default().with_selected(selected_row);
    frame.render_stateful_widget(
        List::new(items)
            .block(
                Block::default()
                    .title(
                        if input.inspection.context.runtime == ContextRuntime::Claude
                            && matches!(
                                input.scan_status,
                                ContextScanStatus::NotStarted | ContextScanStatus::Scanning
                            )
                        {
                            let scan = input.scan_spinner.map_or_else(
                                || "checking subfolders…".into(),
                                |spinner| format!("{spinner} checking subfolders…"),
                            );
                            Line::from(vec![
                                Span::styled(
                                    format!(" {} · ", input.inspection.context.runtime.label()),
                                    Style::new()
                                        .fg(active_theme().secondary)
                                        .add_modifier(Modifier::BOLD),
                                ),
                                Span::styled(scan, Style::new().fg(active_theme().muted)),
                                Span::raw(" "),
                            ])
                        } else {
                            Line::raw(format!(
                                " {} · resolved load chain ",
                                input.inspection.context.runtime.label()
                            ))
                        },
                    )
                    .title_style(
                        Style::new()
                            .fg(active_theme().secondary)
                            .add_modifier(Modifier::BOLD),
                    )
                    .border_style(Style::new().fg(active_theme().dim))
                    .style(Style::new().bg(active_theme().surface))
                    .borders(Borders::ALL),
            )
            .highlight_style(active_theme().selected())
            .highlight_symbol("▸ "),
        list_area,
        &mut state,
    );
    if let Some(detail_area) = detail_area {
        if let Some(source) = selected_source {
            render_source_panel(frame, input, document, detail_area, &source, false);
        } else {
            frame.render_widget(
                Paragraph::new("Select a source to preview its status and Markdown.")
                    .block(
                        Block::default()
                            .title(" Source ")
                            .title_style(Style::new().fg(active_theme().secondary))
                            .border_style(Style::new().fg(active_theme().dim))
                            .borders(Borders::ALL),
                    )
                    .wrap(Wrap { trim: false }),
                detail_area,
            );
            document.max_scroll = 0;
        }
    } else {
        document.max_scroll = 0;
    }
}

fn context_source_item(
    source: &ContextSource,
    position: Option<usize>,
    width: usize,
) -> ListItem<'static> {
    let prefix = position.map_or_else(|| "  · ".into(), |position| format!("  {position} "));
    let remaining = width.saturating_sub(UnicodeWidthStr::width(prefix.as_str()));
    let status = source_list_status(&source.state);
    let status_width = status
        .as_ref()
        .map_or(0, |(status, _)| UnicodeWidthStr::width(*status));
    let source_width = remaining.saturating_sub(status_width + usize::from(status.is_some()));
    let mut spans = vec![
        Span::raw(prefix),
        Span::raw(pad_right(
            &truncate_end(&source.display, source_width),
            source_width,
        )),
    ];
    if let Some((status, style)) = status {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(status, style));
    }
    ListItem::new(Line::from(spans))
}

pub(super) fn source_list_status(state: &SourceState) -> Option<(&'static str, Style)> {
    match state {
        SourceState::Shadowed(_) => Some(("skipped", Style::new().fg(active_theme().muted))),
        SourceState::Empty | SourceState::SelectedEmpty => {
            Some(("empty", Style::new().fg(active_theme().muted)))
        }
        SourceState::Excluded(_) => Some(("excluded", Style::new().fg(active_theme().muted))),
        SourceState::Truncated { .. } => {
            Some(("truncated", Style::new().fg(active_theme().warning)))
        }
        SourceState::Ambiguous(_) => Some(("verify", Style::new().fg(active_theme().warning))),
        SourceState::Unreadable(_) => {
            Some(("cannot read", Style::new().fg(active_theme().warning)))
        }
        SourceState::Startup
        | SourceState::Conditional(_)
        | SourceState::Relevant(_)
        | SourceState::Nested(_) => None,
    }
}

fn source_status(input: &ContextViewInput<'_>, source: &ContextSource) -> String {
    match &source.state {
        SourceState::Startup => source.scope.clone(),
        SourceState::Conditional(reason)
        | SourceState::Relevant(reason)
        | SourceState::Nested(reason) => reason.clone(),
        SourceState::Excluded(reason) => {
            format!(
                "excluded · {}",
                relative_message(input.project_root, reason)
            )
        }
        SourceState::Shadowed(path) => format!(
            "skipped · {} takes priority",
            display_path(path, input.paths, &input.inspection.context.directory)
        ),
        SourceState::Empty => "empty · not loaded".into(),
        SourceState::SelectedEmpty => "selected · empty".into(),
        SourceState::Truncated { included, total } => {
            format!("loads {included} of {total} bytes")
        }
        SourceState::Ambiguous(reason) => format!("needs verification · {reason}"),
        SourceState::Unreadable(reason) => format!("cannot read · {reason}"),
    }
}

fn source_ownership(input: &ContextViewInput<'_>, source: &ContextSource) -> Option<String> {
    if let Some(managed) = &source.managed {
        return Some(format!("global · managed · {}", managed.target));
    }
    if let Some(ownership) = input.project.as_ref().and_then(|project| {
        project.target_names().find_map(|id| {
            (project.target_path(id).ok().as_deref() == Some(source.path.as_path())).then(|| {
                let status = managed_workspace(
                    &input.inspection.managed,
                    &ManagedRef::Project(id.to_owned()),
                )
                .map_or("invalid", |view| view.instruction_status.label());
                format!("project · managed · {status}")
            })
        })
    }) {
        return Some(ownership);
    }
    let local_repository = input.local_repository.as_ref()?;
    for target in [local::ManagedTarget::Agents, local::ManagedTarget::Claude] {
        if source.path == local_repository.root.join(target.filename())
            && input.inspection.home_items.iter().any(
                |item| matches!(item, HomeItem::LocalTarget(candidate) if *candidate == target),
            )
        {
            let status = managed_workspace(&input.inspection.managed, &ManagedRef::Local(target))
                .map_or("invalid", |view| view.instruction_status.label());
            return Some(format!("local · managed · {status}"));
        }
    }
    None
}

fn push_scan_errors(items: &mut Vec<ListItem<'static>>, input: &ContextViewInput<'_>) {
    match &input.scan_status {
        ContextScanStatus::Complete { unreadable } => {
            for path in unreadable {
                items.push(ListItem::new(Line::styled(
                    format!(
                        "  Could not read {}",
                        display_path(path, input.paths, &input.inspection.context.directory)
                    ),
                    Style::new().fg(active_theme().warning),
                )));
            }
        }
        ContextScanStatus::Failed => items.push(ListItem::new(Line::styled(
            "  Could not inspect subfolder instructions",
            Style::new().fg(active_theme().warning),
        ))),
        _ => {}
    }
}

pub(super) fn render_source(
    frame: &mut Frame<'_>,
    input: &ContextViewInput<'_>,
    document: &mut DocumentState,
    area: Rect,
) {
    if let Some(source) = input
        .inspection
        .context
        .sources
        .get(input.source_index)
        .cloned()
    {
        render_source_panel(frame, input, document, area, &source, true);
    } else {
        render_document(
            frame,
            document,
            area,
            "Source",
            "",
            "No source selected.",
            false,
        );
    }
}

fn render_source_panel(
    frame: &mut Frame<'_>,
    input: &ContextViewInput<'_>,
    document: &mut DocumentState,
    area: Rect,
    source: &ContextSource,
    numbered: bool,
) {
    let reason = source
        .state
        .reason(input.paths, &input.inspection.context.directory)
        .unwrap_or_else(|| "selected by the runtime".into());
    let link = symlink_info(input.inspection, &source.path);
    let ownership = source_ownership(input, source)
        .unwrap_or_else(|| "existing file · mdmanager.ai only inspects it".into());
    let mut metadata = vec![Line::from(vec![
        Span::styled("Path       ", Style::new().fg(active_theme().muted)),
        Span::raw(source.display.clone()),
    ])];
    if let Some(info) = &link {
        metadata.extend([
            Line::from(vec![
                Span::styled("Type       ", Style::new().fg(active_theme().muted)),
                Span::raw("symlink"),
            ]),
            Line::from(vec![
                Span::styled("Points to  ", Style::new().fg(active_theme().muted)),
                Span::raw(info.target.display().to_string()),
            ]),
            Line::from(vec![
                Span::styled("Target     ", Style::new().fg(active_theme().muted)),
                Span::raw(symlink_target_status(
                    input
                        .global
                        .as_ref()
                        .zip(input.global_active_profile.as_deref()),
                    input.inspection,
                    input.local_repository,
                    input.project,
                    input.project_root,
                    &source.path,
                    info,
                )),
            ]),
            Line::from(vec![
                Span::styled("Link       ", Style::new().fg(active_theme().muted)),
                Span::raw("not managed by mdmanager.ai"),
            ]),
        ]);
    }
    metadata.extend([
        Line::from(vec![
            Span::styled("Status     ", Style::new().fg(active_theme().muted)),
            Span::styled(source_status(input, source), source_style(&source.state)),
        ]),
        Line::from(vec![
            Span::styled("Scope      ", Style::new().fg(active_theme().muted)),
            Span::raw(source.scope.clone()),
        ]),
        Line::from(vec![
            Span::styled("Why        ", Style::new().fg(active_theme().muted)),
            Span::raw(reason),
        ]),
    ]);
    if link.is_none() {
        metadata.push(Line::from(vec![
            Span::styled("Ownership  ", Style::new().fg(active_theme().muted)),
            Span::raw(ownership),
        ]));
    }
    if !source.imports.is_empty() {
        metadata.push(Line::from(vec![
            Span::styled("Imports    ", Style::new().fg(active_theme().muted)),
            Span::raw(
                source
                    .imports
                    .iter()
                    .map(|path| {
                        display_path(path, input.paths, &input.inspection.context.directory)
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
        ]));
    }
    let metadata_height = u16::try_from(metadata.len())
        .unwrap_or(u16::MAX)
        .saturating_add(2)
        .min(area.height.saturating_sub(4));
    let [metadata_area, _, document_area] = Layout::vertical([
        Constraint::Length(metadata_height),
        Constraint::Length(1),
        Constraint::Min(3),
    ])
    .areas(area);
    frame.render_widget(
        Paragraph::new(metadata)
            .block(
                Block::default()
                    .title(" About ")
                    .title_style(Style::new().fg(active_theme().secondary))
                    .border_style(Style::new().fg(active_theme().dim))
                    .borders(Borders::ALL),
            )
            .wrap(Wrap { trim: false }),
        metadata_area,
    );
    render_plain_document(
        frame,
        document,
        document_area,
        " Contents ",
        &source.content,
        true,
        numbered,
    );
}

fn source_style(state: &SourceState) -> Style {
    match state {
        SourceState::Startup | SourceState::SelectedEmpty => {
            Style::new().fg(active_theme().success)
        }
        SourceState::Shadowed(_) | SourceState::Empty | SourceState::Excluded(_) => {
            Style::new().fg(active_theme().muted)
        }
        SourceState::Conditional(_) | SourceState::Relevant(_) | SourceState::Nested(_) => {
            Style::new().fg(active_theme().secondary)
        }
        SourceState::Truncated { .. } | SourceState::Ambiguous(_) | SourceState::Unreadable(_) => {
            Style::new().fg(active_theme().warning)
        }
    }
}

pub(super) fn context_entries(audit: &Audit, nested_expanded: bool) -> Vec<ContextEntry> {
    let mut entries = audit
        .sources
        .iter()
        .enumerate()
        .filter(|(_, source)| source.group != SourceGroup::Nested)
        .map(|(index, _)| ContextEntry::Source(index))
        .collect::<Vec<_>>();
    if audit.runtime == ContextRuntime::Claude
        && audit
            .sources
            .iter()
            .any(|source| source.group == SourceGroup::Nested)
    {
        let nested_at = entries.len();
        entries.insert(nested_at, ContextEntry::Nested);
        if nested_expanded {
            let nested = audit
                .sources
                .iter()
                .enumerate()
                .filter(|(_, source)| source.group == SourceGroup::Nested)
                .map(|(index, _)| ContextEntry::Source(index))
                .collect::<Vec<_>>();
            entries.splice(nested_at + 1..nested_at + 1, nested);
        }
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::super::tests::{draw, draw_at, finish_context_scan, fixture};
    use super::super::*;
    use super::*;
    use std::fs;

    use ratatui::{Terminal, backend::TestBackend};
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn source_metadata_is_separate_from_markdown() {
        let (_home, repository, _paths, mut app) = fixture();
        app.open(View::File {
            title: "AGENTS.md".into(),
            path: repository.path().join("AGENTS.md"),
            about: "Project file · existing · not managed".into(),
        });
        let screen = draw(&mut app);
        assert!(screen.contains("About"));
        assert!(screen.contains("Contents"));
        assert!(screen.contains("Project file · existing · not managed"));
        assert!(screen.contains("# Existing"));
    }

    #[test]
    fn claude_nested_sources_expand_in_place() {
        let (_home, repository, _paths, mut app) = fixture();
        fs::create_dir(repository.path().join("child")).unwrap();
        fs::write(repository.path().join("child/CLAUDE.md"), "# Child\n").unwrap();
        app.context.reload_and_scan(app.global.as_ref());
        app.open(View::Context);
        assert!(
            !context_entries(&app.inspection.context, app.context_nested_expanded)
                .contains(&ContextEntry::Nested)
        );
        assert!(draw(&mut app).contains("checking subfolders…"));
        finish_context_scan(&mut app);
        let screen = draw(&mut app);
        assert!(screen.contains("SUBFOLDER INSTRUCTIONS · 1 file"));
        app.context_entry_index =
            context_entries(&app.inspection.context, app.context_nested_expanded)
                .iter()
                .position(|entry| *entry == ContextEntry::Nested)
                .unwrap();
        assert!(app.message.is_none());
        assert!(draw(&mut app).contains("enter show"));
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let screen = draw(&mut app);
        assert!(screen.contains("SUBFOLDER INSTRUCTIONS · 1 file"));
        assert!(screen.contains("enter hide"));
        assert!(screen.contains("child/CLAUDE.md"));

        handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        let stacked = draw_at(&mut app, 90, 36);
        assert!(stacked.contains("child/CLAUDE.md"));
        assert!(stacked.contains("About"));

        app.context_entry_index = 0;
        let backend = TestBackend::new(100, 36);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let screen = terminal.backend().to_string();
        let (row, line) = screen
            .lines()
            .enumerate()
            .find(|(_, line)| line.contains("SUBFOLDER INSTRUCTIONS"))
            .unwrap();
        let column = UnicodeWidthStr::width(&line[..line.find("SUBFOLDER").unwrap()]);
        let color = terminal
            .backend()
            .buffer()
            .cell((u16::try_from(column).unwrap(), u16::try_from(row).unwrap()))
            .unwrap()
            .fg;
        assert_ne!(color, active_theme().success);
        assert_ne!(color, active_theme().secondary);
    }

    #[test]
    fn empty_context_retains_scan_diagnostics_without_a_selection() {
        let (_home, repository, _paths, mut app) = fixture();
        app.open(View::Context);
        finish_context_scan(&mut app);
        app.inspection.context.sources.clear();
        app.context.audit.sources.clear();
        assert!(app.inspection.context.warnings.is_empty());
        let normal = draw(&mut app);
        assert!(normal.contains("No persistent Markdown"));
        assert!(!normal.contains("SUBFOLDER INSTRUCTIONS"));
        assert!(context_entries(&app.inspection.context, false).is_empty());
        assert!(!app.context_preview);

        app.context.scan_status = ContextScanStatus::Complete {
            unreadable: vec![repository.path().join("unreadable-marker")],
        };
        assert!(draw(&mut app).contains("unreadable-marker"));

        app.context.scan_status = ContextScanStatus::Failed;
        let backend = TestBackend::new(100, 36);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        assert!(
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .any(|cell| cell.fg == active_theme().warning
                    && cell.symbol().chars().any(char::is_alphabetic))
        );
        assert!(
            terminal
                .backend()
                .to_string()
                .contains("No persistent Markdown")
        );
        handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(app.view, View::Context));

        app.context.scan_status = ContextScanStatus::Scanning;
        assert!(draw(&mut app).contains("checking subfolders"));
        assert!(!app.context_preview);
    }

    #[test]
    fn context_scan_status_is_dim() {
        let (_home, _repository, _paths, mut app) = fixture();
        app.context.reload_and_scan(app.global.as_ref());
        app.open(View::Context);
        let backend = TestBackend::new(100, 36);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let screen = terminal.backend().to_string();
        let (row, line) = screen
            .lines()
            .enumerate()
            .find(|(_, line)| line.contains("checking subfolders"))
            .unwrap();
        let column = UnicodeWidthStr::width(&line[..line.find("checking").unwrap()]);

        assert_eq!(
            terminal
                .backend()
                .buffer()
                .cell((u16::try_from(column).unwrap(), u16::try_from(row).unwrap()))
                .unwrap()
                .fg,
            active_theme().muted
        );
    }

    #[test]
    fn context_list_only_labels_exceptional_states() {
        assert!(source_list_status(&SourceState::Startup).is_none());
        assert!(source_list_status(&SourceState::Conditional("condition".into())).is_none());
        assert!(source_list_status(&SourceState::Nested("subfolder".into())).is_none());
        assert_eq!(
            source_list_status(&SourceState::Empty).map(|(label, _)| label),
            Some("empty")
        );
        assert_eq!(
            source_list_status(&SourceState::Unreadable("error".into())).map(|(label, _)| label),
            Some("cannot read")
        );
    }
}
