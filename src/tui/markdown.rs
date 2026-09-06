use pulldown_cmark::{Event, HeadingLevel, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::active_theme;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum RenderMode {
    #[default]
    Markdown,
    Raw,
}

impl RenderMode {
    pub(super) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "markdown" => Ok(Self::Markdown),
            "raw" => Ok(Self::Raw),
            _ => Err(format!("unknown render mode: {value}")),
        }
    }

    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Markdown => "markdown",
            Self::Raw => "raw",
        }
    }

    pub(super) fn toggle(self) -> Self {
        match self {
            Self::Markdown => Self::Raw,
            Self::Raw => Self::Markdown,
        }
    }

    pub(super) fn lines(self, source: &str) -> Vec<Line<'static>> {
        if self == Self::Raw {
            return source
                .lines()
                .map(|line| Line::raw(line.to_owned()))
                .collect();
        }
        render(source)
    }
}

/// Keep one output line per source line, including empty slots for Markdown delimiters.
/// Parser offsets place styled spans on their source lines before document.rs wraps them.
fn render(source: &str) -> Vec<Line<'static>> {
    let source_lines = source.lines().collect::<Vec<_>>();
    let mut lines = vec![Line::default(); source_lines.len()];
    if lines.is_empty() {
        return lines;
    }
    let starts = std::iter::once(0)
        .chain(source.match_indices('\n').map(|(offset, _)| offset + 1))
        .collect::<Vec<_>>();
    let line_at = |offset| starts.partition_point(|start| *start <= offset) - 1;
    let mut style = Style::new();
    let mut styles = Vec::new();
    let mut lists = Vec::new();
    let mut links = Vec::new();
    let mut code_block = false;
    for (event, range) in Parser::new(source).into_offset_iter() {
        let row = line_at(range.start).min(lines.len() - 1);
        match event {
            Event::Start(tag) => {
                styles.push(style);
                match tag {
                    Tag::Heading { level, .. } => {
                        style = style.fg(active_theme().primary);
                        if matches!(level, HeadingLevel::H1 | HeadingLevel::H2) {
                            style = style.add_modifier(Modifier::BOLD);
                        }
                    }
                    Tag::Strong => style = style.add_modifier(Modifier::BOLD),
                    Tag::Emphasis => style = style.add_modifier(Modifier::ITALIC),
                    Tag::CodeBlock(_) => code_block = true,
                    Tag::List(number) => lists.push(number.is_some()),
                    Tag::Item => {
                        let source_line = source_lines[row];
                        let trimmed = source_line.trim_start();
                        let indent = &source_line[..source_line.len() - trimmed.len()];
                        let marker = if lists.last() == Some(&true) {
                            trimmed.split_whitespace().next().unwrap_or_default()
                        } else {
                            "•"
                        };
                        lines[row]
                            .spans
                            .push(Span::raw(format!("{indent}{marker} ")));
                    }
                    Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } => {
                        links.push(dest_url);
                    }
                    _ => {}
                }
            }
            Event::End(tag) => {
                match tag {
                    TagEnd::CodeBlock => code_block = false,
                    TagEnd::List(_) => {
                        lists.pop();
                    }
                    TagEnd::Link | TagEnd::Image => {
                        let url = links.pop().unwrap();
                        let row = line_at(range.end.saturating_sub(1)).min(lines.len() - 1);
                        lines[row].spans.push(Span::styled(
                            format!(" ({url})"),
                            style.add_modifier(Modifier::DIM),
                        ));
                    }
                    _ => {}
                }
                style = styles.pop().unwrap();
            }
            Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => {
                for (index, text) in text.lines().enumerate() {
                    if code_block {
                        lines[row + index] = Line::styled(
                            format!("    {}", source_lines[row + index]),
                            Style::new().fg(active_theme().muted),
                        );
                    } else {
                        lines[row + index]
                            .spans
                            .push(Span::styled(text.to_owned(), style));
                    }
                }
            }
            Event::Code(text) => {
                let raw = &source[range];
                let code_style = style.fg(active_theme().muted);
                if raw.contains('\n') {
                    // CommonMark joins multiline code spans; retain their source rows here.
                    let delimiter = raw.bytes().take_while(|byte| *byte == b'`').count();
                    let body = &raw[delimiter..raw.len() - delimiter];
                    for (index, part) in body.split('\n').enumerate() {
                        lines[row + index].spans.push(Span::styled(
                            part.trim_end_matches('\r').to_owned(),
                            code_style,
                        ));
                    }
                } else {
                    lines[row]
                        .spans
                        .push(Span::styled(text.into_string(), code_style));
                }
            }
            Event::Rule => lines[row] = Line::raw(source_lines[row].to_owned()),
            _ => {}
        }
    }
    lines
}
