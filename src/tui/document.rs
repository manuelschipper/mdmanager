use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::Clear;
use std::path::Path;
use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::active_theme;
use super::markdown::RenderMode;
use crate::theme::Theme;

const LINE_HIGHLIGHT_DURATION: Duration = Duration::from_millis(1200);

const READABLE_WIDTH: u16 = 100;

pub(super) const COMPACT_WIDTH: u16 = 120;

#[derive(Default)]
/// Viewport and prompts belong to one TUI session; wrapping determines scroll offsets.
pub(super) struct DocumentState {
    pub(super) render_mode: RenderMode,
    pub(super) scroll: u16,
    pub(super) max_scroll: u16,
    pub(super) document_width: u16,
    pub(super) document_height: u16,
    pub(super) highlighted_line: Option<(usize, Instant)>,
    pub(super) search: Option<SearchPrompt>,
    last_search: Option<(String, usize)>,
    pub(super) line_prompt: Option<LinePrompt>,
    layout: Option<DocumentLayout>,
}

/// Parsed lines and wrapped-row starts of the last laid-out document. Draws,
/// searches and line jumps reuse them, so their cost does not grow with every
/// frame; a change of content, render mode, theme or content width rebuilds them.
struct DocumentLayout {
    content: String,
    mode: RenderMode,
    theme: Theme,
    lines: Vec<Line<'static>>,
    // Zero until wrapped; real widths are at least 1.
    width: u16,
    // `row_starts[i]` is the first wrapped row of line `i`; the last entry is the row total.
    row_starts: Vec<usize>,
}

impl DocumentLayout {
    fn cached<'a>(slot: &'a mut Option<Self>, content: &str, mode: RenderMode) -> &'a mut Self {
        let theme = active_theme();
        if !slot.as_ref().is_some_and(|layout| {
            layout.mode == mode && layout.theme == theme && layout.content == content
        }) {
            *slot = Some(Self {
                content: content.to_owned(),
                mode,
                theme,
                lines: mode.lines(content),
                width: 0,
                row_starts: Vec::new(),
            });
        }
        slot.as_mut().unwrap()
    }

    fn wrap(&mut self, width: u16) {
        if self.width == width {
            return;
        }
        self.width = width;
        self.row_starts = std::iter::once(0)
            .chain(self.lines.iter().scan(0, |rows, line| {
                *rows += Paragraph::new(line.clone())
                    .wrap(Wrap { trim: false })
                    .line_count(width)
                    .max(1);
                Some(*rows)
            }))
            .collect();
    }

    fn rows(&self) -> usize {
        *self.row_starts.last().unwrap()
    }
}

pub(super) struct LinePrompt {
    pub(super) input: String,
}

pub(super) struct SearchPrompt {
    pub(super) query: String,
}

/// Renders accepted document text; never reads its source path from disk.
pub(super) fn render_document(
    frame: &mut Frame<'_>,
    document: &mut DocumentState,
    area: Rect,
    title: &str,
    about: &str,
    content: &str,
    numbered: bool,
) {
    if about.is_empty() {
        render_plain_document(
            frame,
            document,
            area,
            &format!(" {title} "),
            content,
            true,
            numbered,
        );
        return;
    }
    let inner_width = area.width.saturating_sub(2).max(1);
    let max_about_height = area.height.saturating_sub(4);
    let mut displayed_about = about.to_owned();
    let mut lines = Paragraph::new(displayed_about.as_str())
        .wrap(Wrap { trim: false })
        .line_count(inner_width);
    if lines.saturating_add(2) > usize::from(max_about_height) {
        // Keep usage metadata and Contents reachable when the full path would consume the screen.
        displayed_about = about
            .lines()
            .map(|line| {
                if let Some(path) = line.strip_prefix("Path: ") {
                    format!(
                        "Path: {}",
                        truncate_start(path, usize::from(inner_width).saturating_sub(6))
                    )
                } else {
                    line.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        lines = Paragraph::new(displayed_about.as_str())
            .wrap(Wrap { trim: false })
            .line_count(inner_width);
    }
    let about_height = u16::try_from(lines)
        .unwrap_or(u16::MAX)
        .saturating_add(2)
        .min(max_about_height);
    let [about_area, _, content_area] = Layout::vertical([
        Constraint::Length(about_height),
        Constraint::Length(1),
        Constraint::Min(3),
    ])
    .areas(area);
    frame.render_widget(
        Paragraph::new(displayed_about)
            .block(
                Block::default()
                    .title(" About ")
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
        content_area,
        " Contents ",
        content,
        true,
        numbered,
    );
}

pub(super) fn render_plain_document(
    frame: &mut Frame<'_>,
    document: &mut DocumentState,
    area: Rect,
    title: &str,
    content: &str,
    readable: bool,
    numbered: bool,
) {
    let mode = document.render_mode;
    render_lines(
        frame, document, area, title, content, mode, readable, numbered,
    );
}

pub(super) fn render_raw_document(
    frame: &mut Frame<'_>,
    document: &mut DocumentState,
    area: Rect,
    title: &str,
    content: &str,
) {
    render_lines(
        frame,
        document,
        area,
        title,
        content,
        RenderMode::Raw,
        false,
        false,
    );
}

#[allow(clippy::too_many_arguments)]
fn render_lines(
    frame: &mut Frame<'_>,
    document: &mut DocumentState,
    area: Rect,
    title: &str,
    content: &str,
    mode: RenderMode,
    readable: bool,
    numbered: bool,
) {
    let block = Block::default()
        .title(title)
        .title_style(Style::new().fg(active_theme().secondary))
        .border_style(Style::new().fg(active_theme().dim))
        .borders(Borders::ALL);
    let inner = block.inner(area);
    let width = if readable {
        inner.width.min(READABLE_WIDTH)
    } else {
        inner.width
    };
    let x = if numbered {
        inner.x
    } else {
        inner.x + inner.width.saturating_sub(width) / 2
    };
    let readable_area = Rect::new(x, inner.y, width, inner.height);
    let layout = DocumentLayout::cached(&mut document.layout, content, mode);
    let number_width = layout.lines.len().max(1).to_string().len().max(3);
    let gutter_width = if numbered {
        u16::try_from(number_width)
            .unwrap_or(u16::MAX)
            .saturating_add(3)
            .min(readable_area.width.saturating_sub(1))
    } else {
        0
    };
    let [gutter_area, content_area] =
        Layout::horizontal([Constraint::Length(gutter_width), Constraint::Min(1)])
            .areas(readable_area);
    let highlighted = document
        .highlighted_line
        .and_then(|(line, started)| (started.elapsed() < LINE_HIGHLIGHT_DURATION).then_some(line));
    if document.highlighted_line.is_some() && highlighted.is_none() {
        document.highlighted_line = None;
    }
    layout.wrap(content_area.width.max(1));
    document.document_width = content_area.width.max(1);
    document.document_height = content_area.height;
    document.max_scroll = scroll_max(layout.rows(), content_area.height);
    document.scroll = document.scroll.min(document.max_scroll);
    // Lay out only the source lines that reach the viewport; `skip` hides the
    // wrapped rows of the first one that lie above it.
    let scroll = usize::from(document.scroll);
    let first = layout
        .row_starts
        .partition_point(|start| *start <= scroll)
        .saturating_sub(1);
    let end = layout
        .row_starts
        .partition_point(|start| *start < scroll + usize::from(content_area.height))
        .clamp(first, layout.lines.len());
    let skip = u16::try_from(scroll - layout.row_starts[first]).unwrap_or(u16::MAX);
    let text = Text::from(
        (first..end)
            .map(|index| {
                let mut line = layout.lines[index].clone();
                if highlighted == Some(index + 1) {
                    let highlight = Style::new()
                        .fg(active_theme().background)
                        .bg(active_theme().warning);
                    for span in &mut line.spans {
                        span.style = span.style.patch(highlight);
                    }
                    line.style(highlight)
                } else {
                    line
                }
            })
            .collect::<Vec<_>>(),
    );
    frame.render_widget(block, area);
    if numbered {
        let mut gutter = Vec::new();
        for index in first..end {
            gutter.push(Line::styled(
                format!("{:>number_width$} │ ", index + 1),
                Style::new().fg(active_theme().muted),
            ));
            let wrapped = layout.row_starts[index + 1] - layout.row_starts[index];
            gutter.extend((1..wrapped).map(|_| {
                Line::styled(
                    format!("{:number_width$} │ ", ""),
                    Style::new().fg(active_theme().muted),
                )
            }));
        }
        frame.render_widget(Paragraph::new(gutter).scroll((skip, 0)), gutter_area);
    }
    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .scroll((skip, 0)),
        content_area,
    );
}

pub(super) fn render_info(
    frame: &mut Frame<'_>,
    document: &mut DocumentState,
    screen: Rect,
    title: &str,
    text: &str,
) {
    let (area, wrapped_lines) = info_panel_geometry(screen, text);
    let block = Block::default()
        .title(format!(" mdmanager.ai · {title} "))
        .title_style(
            Style::new()
                .fg(active_theme().primary)
                .add_modifier(Modifier::BOLD),
        )
        .border_style(Style::new().fg(active_theme().primary))
        .style(Style::new().bg(active_theme().surface_raised))
        .borders(Borders::ALL);
    let inner = block.inner(area);
    let [content_area, controls_area] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).areas(inner);
    let text_area = Rect::new(
        content_area.x.saturating_add(2),
        content_area.y.saturating_add(1),
        content_area.width.saturating_sub(4),
        content_area.height.saturating_sub(2),
    );
    document.max_scroll = wrapped_lines.saturating_sub(text_area.height);
    document.scroll = document.scroll.min(document.max_scroll);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .scroll((document.scroll, 0)),
        text_area,
    );
    let controls = if document.max_scroll > 0 {
        "↑↓ scroll · Shift+↑/↓ page · Esc back"
    } else {
        "Esc back"
    };
    frame.render_widget(
        Paragraph::new(controls)
            .block(Block::default().borders(Borders::TOP))
            .alignment(Alignment::Center)
            .style(Style::new().fg(active_theme().muted)),
        controls_area,
    );
}

fn scroll_max(line_count: usize, height: u16) -> u16 {
    u16::try_from(line_count)
        .unwrap_or(u16::MAX)
        .saturating_sub(height)
}

pub(super) fn pad_right(value: &str, width: usize) -> String {
    let padding = width.saturating_sub(UnicodeWidthStr::width(value));
    format!("{value}{}", " ".repeat(padding))
}

pub(super) fn fit_column(value: &str, width: usize) -> String {
    pad_right(&truncate_end(value, width), width)
}

pub(super) fn truncate_start(value: &str, width: usize) -> String {
    if UnicodeWidthStr::width(value) <= width {
        return value.to_owned();
    }
    if width <= 1 {
        return "…".repeat(width);
    }
    let mut suffix = String::new();
    let mut used = 1;
    for character in value.chars().rev() {
        let character_width = character.width().unwrap_or(0);
        if used + character_width > width {
            break;
        }
        suffix.insert(0, character);
        used += character_width;
    }
    format!("…{suffix}")
}

pub(super) fn truncate_end(value: &str, width: usize) -> String {
    if UnicodeWidthStr::width(value) <= width {
        return value.to_owned();
    }
    if width <= 1 {
        return "…".repeat(width);
    }
    let mut prefix = String::new();
    let mut used = 1;
    for character in value.chars() {
        let character_width = character.width().unwrap_or(0);
        if used + character_width > width {
            break;
        }
        prefix.push(character);
        used += character_width;
    }
    format!("{prefix}…")
}

pub(super) fn bounded(area: Rect, max_width: u16, max_height: u16) -> Rect {
    let [vertical] = Layout::vertical([Constraint::Length(area.height.min(max_height))])
        .flex(Flex::Center)
        .areas(area);
    let [centered] = Layout::horizontal([Constraint::Length(area.width.min(max_width))])
        .flex(Flex::Center)
        .areas(vertical);
    centered
}

pub(super) fn bounded_around(screen: Rect, anchor: Rect, max_width: u16, max_height: u16) -> Rect {
    let width = screen.width.min(max_width);
    let height = screen.height.min(max_height);
    let center_x = anchor.x.saturating_add(anchor.width / 2);
    let center_y = anchor.y.saturating_add(anchor.height / 2);
    let max_x = screen.x.saturating_add(screen.width.saturating_sub(width));
    let max_y = screen
        .y
        .saturating_add(screen.height.saturating_sub(height));
    Rect::new(
        center_x.saturating_sub(width / 2).clamp(screen.x, max_x),
        center_y.saturating_sub(height / 2).clamp(screen.y, max_y),
        width,
        height,
    )
}

pub(super) fn info_panel_geometry(screen: Rect, text: &str) -> (Rect, u16) {
    let text_width = screen.width.min(88).saturating_sub(6).max(1);
    let wrapped_lines = u16::try_from(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .line_count(text_width),
    )
    .unwrap_or(u16::MAX);
    let max_height = screen.height.saturating_sub(4).clamp(8, 30);
    let height = wrapped_lines.saturating_add(6).max(8).min(max_height);
    (bounded(screen, 88, height), wrapped_lines)
}

impl DocumentState {
    pub(super) fn go_to_line(&mut self, text: &str, input: String) -> (bool, String) {
        let layout = DocumentLayout::cached(&mut self.layout, text, self.render_mode);
        let count = layout.lines.len();
        if count == 0 {
            return (false, "Document has no lines".into());
        }
        let Ok(line) = input.parse::<usize>() else {
            return (false, format!("Line must be between 1 and {count}"));
        };
        if line == 0 || line > count {
            return (false, format!("Line must be between 1 and {count}"));
        }
        layout.wrap(self.document_width.max(1));
        let offset = layout.row_starts[line - 1];
        self.scroll = u16::try_from(offset)
            .unwrap_or(u16::MAX)
            .saturating_sub(self.document_height / 2)
            .min(self.max_scroll);
        self.highlighted_line = Some((line, Instant::now()));
        (true, format!("Line {line}"))
    }

    pub(super) fn apply_search(
        &mut self,
        text: &str,
        query: String,
        raw: bool,
    ) -> Option<(bool, String)> {
        if query.is_empty() {
            return None;
        }
        let needle = query.to_lowercase();
        let lines = text.lines().collect::<Vec<_>>();
        if lines.is_empty() {
            return None;
        }
        let width = self.document_width.max(1);
        let mode = if raw {
            RenderMode::Raw
        } else {
            self.render_mode
        };
        let layout = DocumentLayout::cached(&mut self.layout, text, mode);
        layout.wrap(width);
        let displayed = &layout.lines;
        let matches = lines
            .iter()
            .enumerate()
            .filter_map(|(index, line)| {
                line.to_lowercase().find(&needle)?;
                let visible = displayed[index].to_string();
                let Some(match_start) = visible.to_lowercase().find(&needle) else {
                    return Some(layout.row_starts[index]);
                };
                let match_end = match_start + needle.len();
                let mut folded_bytes = 0;
                let end = visible.char_indices().find_map(|(byte, character)| {
                    folded_bytes += character.to_lowercase().map(char::len_utf8).sum::<usize>();
                    (folded_bytes >= match_end).then_some(byte + character.len_utf8())
                })?;
                let within_line = Paragraph::new(&visible[..end])
                    .wrap(Wrap { trim: false })
                    .line_count(width)
                    .saturating_sub(1);
                Some(layout.row_starts[index] + within_line)
            })
            .collect::<Vec<_>>();
        // The last match can lie below the maximum viewport origin. Retain its
        // identity so repeating search advances even when scrolling was clamped.
        let after = self
            .last_search
            .as_ref()
            .filter(|(previous, offset)| {
                previous == &needle
                    && self.scroll
                        == u16::try_from(*offset)
                            .unwrap_or(u16::MAX)
                            .min(self.max_scroll)
            })
            .map_or(usize::from(self.scroll), |(_, offset)| *offset);
        let found = matches
            .iter()
            .copied()
            .find(|offset| *offset > after)
            .or_else(|| matches.first().copied());
        if let Some(offset) = found {
            self.last_search = Some((needle, offset));
            self.scroll = u16::try_from(offset)
                .unwrap_or(u16::MAX)
                .min(self.max_scroll);
            Some((true, format!("Found “{query}”")))
        } else {
            Some((false, format!("No match for “{query}”")))
        }
    }
}

pub(super) fn relative_message(project_root: &Path, message: &str) -> String {
    message.replace(
        &project_root.display().to_string(),
        project_root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("."),
    )
}

pub(super) fn render_search(frame: &mut Frame<'_>, document: &DocumentState) {
    let query = &document.search.as_ref().unwrap().query;
    let area = bounded(frame.area(), 68, 5);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(format!("Search: {}_\nenter find · esc cancel", query)).block(
            Block::default()
                .title(" Find in document ")
                .title_style(
                    Style::new()
                        .fg(active_theme().primary)
                        .add_modifier(Modifier::BOLD),
                )
                .border_style(Style::new().fg(active_theme().primary))
                .style(Style::new().bg(active_theme().surface_raised))
                .borders(Borders::ALL),
        ),
        area,
    );
}

pub(super) fn render_line_prompt(frame: &mut Frame<'_>, document: &DocumentState) {
    let input = &document.line_prompt.as_ref().unwrap().input;
    let area = bounded(frame.area(), 68, 5);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(format!("Go to line: {input}_\nenter go · esc cancel")).block(
            Block::default()
                .title(" Go to line ")
                .title_style(
                    Style::new()
                        .fg(active_theme().primary)
                        .add_modifier(Modifier::BOLD),
                )
                .border_style(Style::new().fg(active_theme().primary))
                .style(Style::new().bg(active_theme().surface_raised))
                .borders(Borders::ALL),
        ),
        area,
    );
}

/// Returns a submitted search query; editing and cancellation remain in document state.
pub(super) fn handle_search_key(document: &mut DocumentState, key: KeyEvent) -> Option<String> {
    match key.code {
        KeyCode::Esc => document.search = None,
        KeyCode::Backspace => {
            document.search.as_mut().unwrap().query.pop();
        }
        KeyCode::Enter => {
            let query = document.search.take().unwrap().query;
            return Some(query);
        }
        KeyCode::Char(character)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            document.search.as_mut().unwrap().query.push(character);
        }
        _ => {}
    }
    None
}

/// Returns a submitted line number; App supplies the current numbered document.
pub(super) fn handle_line_prompt_key(
    document: &mut DocumentState,
    key: KeyEvent,
) -> Option<String> {
    match key.code {
        KeyCode::Esc => document.line_prompt = None,
        KeyCode::Backspace => {
            document.line_prompt.as_mut().unwrap().input.pop();
        }
        KeyCode::Enter => {
            let input = document.line_prompt.take().unwrap().input;
            return Some(input);
        }
        KeyCode::Char(character) if character.is_ascii_digit() => {
            document.line_prompt.as_mut().unwrap().input.push(character);
        }
        _ => {}
    }
    None
}

#[cfg(test)]
mod tests {

    use super::super::tests::{draw_at, fixture};
    use super::super::*;
    use std::fs;

    #[test]
    fn document_navigation_reveals_matches_and_page_boundaries() {
        let (_home, repository, _paths, mut app) = fixture();
        let content = format!(
            "FIRST-BOUNDARY\n{}\nneedle-alpha\n{} needle-beta\n{}\nneedle-gamma\nLAST-BOUNDARY",
            "**ordinary** line\n".repeat(40),
            "**İ界 wrapped words** ".repeat(300),
            "**ordinary** line\n".repeat(40),
        );
        fs::write(repository.path().join("AGENTS.md"), content).unwrap();
        app.reload_from_disk();
        app.open(View::File {
            title: "AGENTS.md".into(),
            path: repository.path().join("AGENTS.md"),
            about: "Project file".into(),
        });
        for mode in [RenderMode::Markdown, RenderMode::Raw] {
            app.document.render_mode = mode;
            app.document.scroll = 0;
            assert!(draw_at(&mut app, 80, 20).contains("FIRST-BOUNDARY"));
            for expected in [
                "needle-alpha",
                "needle-beta",
                "needle-gamma",
                "needle-alpha",
            ] {
                app.apply_search("NEEDLE".into());
                let screen = draw_at(&mut app, 80, 20);
                assert!(screen.contains(expected), "{expected} missing: {screen}");
            }
            for _ in 0..100 {
                handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT));
            }
            assert!(draw_at(&mut app, 80, 20).contains("LAST-BOUNDARY"));
            handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT));
            assert!(draw_at(&mut app, 80, 20).contains("LAST-BOUNDARY"));
            for _ in 0..100 {
                handle_key(&mut app, KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT));
            }
            assert!(draw_at(&mut app, 80, 20).contains("FIRST-BOUNDARY"));
            handle_key(&mut app, KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT));
            assert!(draw_at(&mut app, 80, 20).contains("FIRST-BOUNDARY"));
        }
    }

    #[test]
    fn go_to_line_centers_and_highlights_the_source_line() {
        let (_home, repository, _paths, mut app) = fixture();
        let content = (1..=100)
            .map(|line| match line {
                20 | 22 => "```".into(),
                21 => "# verbatim code".into(),
                1..=70 => format!("## {}", "**wrapped words** ".repeat(12)),
                _ => format!("line {line}"),
            })
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(repository.path().join("AGENTS.md"), content).unwrap();
        app.reload_from_disk();
        app.open(View::File {
            title: "AGENTS.md".into(),
            path: repository.path().join("AGENTS.md"),
            about: "Project file".into(),
        });
        draw_at(&mut app, 80, 24);

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE),
        );
        assert!(draw_at(&mut app, 80, 24).contains("Go to line: _"));
        for digit in ['8', '0'] {
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char(digit), KeyModifiers::NONE),
            );
        }
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert!(app.document.scroll > 0);
        assert!(matches!(app.document.highlighted_line, Some((80, _))));
        let screen = draw_at(&mut app, 80, 24);
        let row = screen
            .lines()
            .find(|line| line.contains("line 80"))
            .unwrap();
        assert_eq!(row.split('│').nth(1).unwrap().trim(), "80");
    }

    #[test]
    fn markdown_styles_preserve_source_rows_and_code_bytes() {
        let source = "# Heading\n## Heading\n### Heading\n\n- **bold** and *italic* with `code`\n  - nested\n\n7. ordered\n3. retained\n\n[link](https://example.com)\n```rust\n# **verbatim**\n\n  indented\n```\nlast\n`multi\nline`";
        let lines = RenderMode::Markdown.lines(source);
        assert_eq!(lines.len(), source.lines().count());
        for (row, bold) in [(0, true), (1, true), (2, false)] {
            assert_eq!(lines[row].spans.len(), 1);
            let span = &lines[row].spans[0];
            assert!(!span.content.starts_with('#'));
            assert_eq!(span.style.fg, Some(active_theme().primary));
            assert_eq!(span.style.add_modifier.contains(Modifier::BOLD), bold);
        }
        assert!(lines[3].spans.is_empty());
        assert_eq!(lines[4].spans[0].content, "• ");
        for modifier in [Modifier::BOLD, Modifier::ITALIC] {
            assert!(
                lines[4]
                    .spans
                    .iter()
                    .any(|span| span.style.add_modifier.contains(modifier))
            );
        }
        assert!(
            lines[4]
                .spans
                .iter()
                .any(|span| span.style.fg == Some(active_theme().muted)
                    && !span.content.contains('`'))
        );
        assert_eq!(lines[5].spans[0].content, "  • ");
        for row in [7, 8] {
            assert!(
                lines[row].to_string().starts_with(
                    source
                        .lines()
                        .nth(row)
                        .unwrap()
                        .split_whitespace()
                        .next()
                        .unwrap()
                )
            );
        }
        assert!(
            lines[10]
                .spans
                .last()
                .unwrap()
                .style
                .add_modifier
                .contains(Modifier::DIM)
        );
        for row in [11, 15] {
            assert!(lines[row].spans.is_empty());
        }
        for row in [12, 13, 14] {
            assert_eq!(
                lines[row].to_string(),
                format!("    {}", source.lines().nth(row).unwrap())
            );
            assert_eq!(lines[row].style.fg, Some(active_theme().muted));
        }
        for row in [17, 18] {
            assert_eq!(lines[row].spans.len(), 1);
            assert_eq!(lines[row].spans[0].style.fg, Some(active_theme().muted));
            assert_eq!(
                lines[row].to_string(),
                source.lines().nth(row).unwrap().trim_matches('`')
            );
        }
        assert_eq!(
            RenderMode::Raw
                .lines(source)
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            source.lines().collect::<Vec<_>>()
        );
    }

    #[test]
    fn long_info_is_height_capped_and_scrollable() {
        let (_home, _repository, _paths, mut app) = fixture();
        app.open(View::Info {
            kind: DiagnosticKind::General,
            title: "Configuration error".into(),
            text: (1..=80)
                .map(|line| format!("error detail {line}"))
                .collect::<Vec<_>>()
                .join("\n"),
        });

        let screen = draw_at(&mut app, 160, 50);
        assert!(screen.contains("error detail 1"));
        assert!(!screen.contains("error detail 80"));
        for _ in 0..80 {
            handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT));
        }
        assert!(draw_at(&mut app, 160, 50).contains("error detail 80"));
    }

    #[test]
    fn numbered_viewport_follows_wrapped_rows_at_every_scroll() {
        let (_home, repository, _paths, mut app) = fixture();
        let content = (1..=60)
            .map(|line| {
                if line % 3 == 0 {
                    format!("**L{line}** ").repeat(30)
                } else {
                    format!("L{line}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(repository.path().join("AGENTS.md"), content).unwrap();
        app.reload_from_disk();
        app.open(View::File {
            title: "AGENTS.md".into(),
            path: repository.path().join("AGENTS.md"),
            about: "Project file".into(),
        });
        // Mode and width changes must relayout: each visible row carries only
        // its own line's tokens beside the right gutter number or a continuation.
        for (mode, width) in [
            (RenderMode::Markdown, 80),
            (RenderMode::Raw, 80),
            (RenderMode::Raw, 54),
            (RenderMode::Markdown, 54),
        ] {
            app.document.render_mode = mode;
            app.document.scroll = 0;
            draw_at(&mut app, width, 24);
            let max_scroll = app.document.max_scroll;
            assert!(max_scroll > 0);
            let mut previous_rows = Vec::new();
            for scroll in 0..=max_scroll + 1 {
                app.document.scroll = scroll;
                let screen = draw_at(&mut app, width, 24);
                let rows = screen
                    .lines()
                    .skip_while(|row| !row.contains("Contents"))
                    .skip(1)
                    .take_while(|row| !row.contains('└'))
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                // One scroll step moves the viewport by exactly one wrapped row.
                if (1..=max_scroll).contains(&scroll) {
                    assert_eq!(rows[..rows.len() - 1], previous_rows[1..], "{screen}");
                }
                let mut current = None;
                let mut last = None;
                for row in &rows {
                    let mut parts = row.split('│').skip(1);
                    let number = parts.next().unwrap().trim();
                    let text = parts.next().unwrap().trim();
                    if !number.is_empty() {
                        let number = number.parse::<usize>().unwrap();
                        if let Some(previous) = current {
                            assert_eq!(number, previous + 1, "{screen}");
                        }
                        current = Some(number);
                    }
                    assert!(!text.is_empty(), "{screen}");
                    assert!(mode == RenderMode::Raw || !text.contains('*'), "{screen}");
                    for token in text.split_whitespace() {
                        let token = token.trim_matches('*');
                        let line = current.unwrap_or_else(|| token[1..].parse::<usize>().unwrap());
                        assert_eq!(token, format!("L{line}"), "{screen}");
                        last = Some(line);
                    }
                }
                if scroll >= max_scroll {
                    assert_eq!(app.document.scroll, max_scroll);
                    assert_eq!(last, Some(60), "{screen}");
                }
                previous_rows = rows;
            }
        }
    }
}
