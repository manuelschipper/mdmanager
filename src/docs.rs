//! Documentation embedded in the installed binary.

struct Topic {
    name: &'static str,
    summary: &'static str,
    contents: &'static str,
}

macro_rules! topic {
    ($name:literal, $summary:literal, $path:literal) => {
        Topic {
            name: $name,
            summary: $summary,
            contents: include_str!($path),
        }
    };
}

const TOPICS: &[Topic] = &[
    topic!(
        "start",
        "Use the agent-driven CLI and inspection TUI workflow.",
        "../docs/start.md"
    ),
    topic!(
        "context",
        "Understand managed and observed Markdown context.",
        "../docs/context.md"
    ),
    topic!(
        "claude",
        "Understand Claude files, rules, imports, and exclusions.",
        "../docs/claude.md"
    ),
    topic!(
        "codex",
        "Understand Codex AGENTS.md discovery.",
        "../docs/codex.md"
    ),
    topic!(
        "cursor",
        "Understand Cursor AGENTS.md and Project Rules.",
        "../docs/cursor.md"
    ),
    topic!(
        "pi",
        "Understand Pi instruction-file selection.",
        "../docs/pi.md"
    ),
    topic!(
        "concepts",
        "Understand awareness, ownership, rendering, and status.",
        "../docs/concepts.md"
    ),
    topic!(
        "configuration",
        "Define Global, Project, and Local compositions plus UI preferences.",
        "../docs/configuration.md"
    ),
    topic!(
        "tui",
        "Browse Context, Markdown, status, and differences.",
        "../docs/tui.md"
    ),
    topic!(
        "cli",
        "Use the agent-facing command and write surface.",
        "../docs/cli.md"
    ),
    topic!(
        "migrate",
        "Move existing instructions into mdmanager.ai safely.",
        "../docs/migrate.md"
    ),
];

pub(crate) fn render(name: Option<&str>) -> Result<String, String> {
    let Some(name) = name else {
        let width = TOPICS
            .iter()
            .map(|topic| topic.name.len())
            .max()
            .unwrap_or(0);
        return Ok(TOPICS
            .iter()
            .map(|topic| format!("{:<width$}  {}", topic.name, topic.summary))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n");
    };
    TOPICS
        .iter()
        .find(|topic| topic.name == name)
        .map(|topic| topic.contents.to_owned())
        .ok_or_else(|| format!("documentation topic not found: {name}; run `mdmanager docs`"))
}
