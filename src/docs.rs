//! Documentation embedded in the installed binary.

struct Topic {
    name: &'static str,
    summary: &'static str,
    contents: &'static str,
    max_bytes: usize,
}

macro_rules! topic {
    ($name:literal, $summary:literal, $path:literal, $max_bytes:literal) => {
        Topic {
            name: $name,
            summary: $summary,
            contents: include_str!($path),
            max_bytes: $max_bytes,
        }
    };
}

const TOPICS: &[Topic] = &[
    topic!(
        "start",
        "Use the agent-driven CLI and inspection TUI workflow.",
        "../docs/start.md",
        9_216
    ),
    topic!(
        "context",
        "Understand managed and observed Markdown context.",
        "../docs/context.md",
        3_072
    ),
    topic!(
        "claude",
        "Understand Claude files, rules, imports, and exclusions.",
        "../docs/claude.md",
        3_072
    ),
    topic!(
        "codex",
        "Understand Codex AGENTS.md discovery.",
        "../docs/codex.md",
        2_048
    ),
    topic!(
        "cursor",
        "Understand Cursor AGENTS.md and Project Rules.",
        "../docs/cursor.md",
        2_048
    ),
    topic!(
        "pi",
        "Understand Pi instruction-file selection.",
        "../docs/pi.md",
        2_048
    ),
    topic!(
        "concepts",
        "Understand awareness, ownership, rendering, and status.",
        "../docs/concepts.md",
        3_072
    ),
    topic!(
        "configuration",
        "Define Global, Project, and Local compositions plus UI preferences.",
        "../docs/configuration.md",
        5_120
    ),
    topic!(
        "tui",
        "Browse Context, Markdown, status, and differences.",
        "../docs/tui.md",
        5_120
    ),
    topic!(
        "cli",
        "Use the agent-facing command and write surface.",
        "../docs/cli.md",
        3_072
    ),
    topic!(
        "migrate",
        "Move existing instructions into mdmanager.ai safely.",
        "../docs/migrate.md",
        4_096
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
        .map(|topic| {
            debug_assert!(topic.contents.len() <= topic.max_bytes);
            topic.contents.to_owned()
        })
        .ok_or_else(|| format!("documentation topic not found: {name}; run `mdmanager docs`"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_is_unique_and_within_its_budgets() {
        for topic in TOPICS {
            assert!(
                topic.contents.len() <= topic.max_bytes,
                "{} is {} bytes; budget is {}",
                topic.name,
                topic.contents.len(),
                topic.max_bytes
            );
        }
        let mut names = TOPICS.iter().map(|topic| topic.name).collect::<Vec<_>>();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), TOPICS.len());
    }

    #[test]
    fn index_lists_every_topic() {
        let index = render(None).unwrap();
        for topic in TOPICS {
            assert!(index.lines().any(|line| line.starts_with(topic.name)));
        }
    }

    #[test]
    fn topic_names_are_exact() {
        for topic in TOPICS {
            assert_eq!(render(Some(topic.name)).unwrap(), topic.contents);
        }
        assert!(render(Some("missing")).is_err());
        assert!(render(Some("../start")).is_err());
    }
}
