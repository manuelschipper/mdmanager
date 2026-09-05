use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use walkdir::{DirEntry, WalkDir};

use crate::config::{GlobalConfig, Paths};
use crate::deploy;

/// Pi candidate order: the first existing file wins, even an empty override.
pub(crate) const PI_INSTRUCTION_CANDIDATES: [&str; 5] = [
    "AGENTS.override.md",
    "AGENTS.md",
    "AGENTS.MD",
    "CLAUDE.md",
    "CLAUDE.MD",
];

/// Context runtime: the coding agent whose loaded instruction files an `Audit` explains.
/// Selected by `mdmanager context --runtime` and the TUI picker.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ContextRuntime {
    Claude,
    Codex,
    Cursor,
    Pi,
}

impl ContextRuntime {
    pub(crate) const ALL: [Self; 4] = [Self::Claude, Self::Codex, Self::Cursor, Self::Pi];

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
            Self::Cursor => "Cursor",
            Self::Pi => "Pi",
        }
    }

    pub(crate) fn parse(raw: &str) -> Result<Self, String> {
        match raw {
            "claude" => Ok(Self::Claude),
            "codex" => Ok(Self::Codex),
            "cursor" => Ok(Self::Cursor),
            "pi" => Ok(Self::Pi),
            _ => Err(format!(
                "unknown runtime {raw}; expected claude, codex, cursor, or pi"
            )),
        }
    }

    pub(crate) const fn id(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Cursor => "cursor",
            Self::Pi => "pi",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SourceGroup {
    Startup,
    PathFiltered,
    Nested,
}

impl SourceGroup {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Startup => "LOADS AT STARTUP",
            Self::PathFiltered => "LOADS WHEN RELEVANT",
            Self::Nested => "SUBFOLDER INSTRUCTIONS",
        }
    }
}

#[derive(Debug)]
pub(crate) struct ClaudeScan {
    pub(crate) sources: Vec<ContextSource>,
    pub(crate) unreadable: Vec<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SourceState {
    Startup,
    Conditional(String),
    Relevant(String),
    Nested(String),
    Excluded(String),
    Shadowed(PathBuf),
    Empty,
    SelectedEmpty,
    Truncated { included: usize, total: usize },
    Ambiguous(String),
    Unreadable(String),
}

impl SourceState {
    pub(crate) const fn label(&self) -> &'static str {
        match self {
            Self::Startup => "at startup",
            Self::Conditional(_) => "path filter",
            Self::Relevant(_) => "when relevant",
            Self::Nested(_) => "on demand",
            Self::Excluded(_) => "excluded",
            Self::Shadowed(_) => "not selected",
            Self::Empty | Self::SelectedEmpty => "empty",
            Self::Truncated { .. } => "partial",
            Self::Ambiguous(_) => "uncertain",
            Self::Unreadable(_) => "unreadable",
        }
    }

    pub(crate) fn reason(&self, paths: &Paths, directory: &Path) -> Option<String> {
        match self {
            Self::Conditional(reason)
            | Self::Relevant(reason)
            | Self::Nested(reason)
            | Self::Excluded(reason)
            | Self::Ambiguous(reason)
            | Self::Unreadable(reason) => Some(reason.clone()),
            Self::Shadowed(path) => Some(format!(
                "not loaded because {} takes priority",
                display_path(path, paths, directory)
            )),
            Self::Truncated { included, total } => {
                Some(format!("included {included} of {total} bytes"))
            }
            Self::SelectedEmpty => {
                Some("selected by the runtime; contributes no instruction text".into())
            }
            Self::Startup | Self::Empty => None,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ManagedSource {
    pub(crate) target: String,
    pub(crate) profile: Option<String>,
    pub(crate) status: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct ContextSource {
    pub(crate) path: PathBuf,
    pub(crate) display: String,
    pub(crate) scope: String,
    pub(crate) group: SourceGroup,
    pub(crate) state: SourceState,
    pub(crate) content: String,
    pub(crate) imports: Vec<PathBuf>,
    pub(crate) managed: Option<ManagedSource>,
}

#[derive(Clone, Debug)]
pub(crate) struct Audit {
    pub(crate) runtime: ContextRuntime,
    pub(crate) directory: PathBuf,
    pub(crate) sources: Vec<ContextSource>,
    pub(crate) summary: String,
    // Warnings are data; summary formatting belongs to the public CLI boundary.
    pub(crate) warnings: Vec<String>,
}

impl Audit {
    pub(crate) fn formatted_summary(&self) -> String {
        let mut summary = self.summary.clone();
        for warning in &self.warnings {
            summary.push_str(&format!(" · Warning: {warning}"));
        }
        summary
    }

    pub(crate) fn resolve(
        runtime: ContextRuntime,
        directory: &Path,
        paths: &Paths,
        global: Option<&GlobalConfig>,
    ) -> Self {
        let directory = directory.to_owned();
        let mut audit = match runtime {
            ContextRuntime::Claude => resolve_claude(&directory, paths),
            ContextRuntime::Codex => resolve_codex(&directory, paths),
            ContextRuntime::Cursor => resolve_cursor(&directory, paths),
            ContextRuntime::Pi => resolve_pi(&directory, paths),
        };
        if let Some(global) = global {
            annotate_managed(&mut audit, global);
        }
        audit
    }

    pub(crate) fn add_claude_scan(&mut self, mut scan: ClaudeScan, global: Option<&GlobalConfig>) {
        let existing = self
            .sources
            .iter()
            .map(|source| source.path.clone())
            .collect::<HashSet<_>>();
        scan.sources
            .retain(|source| !existing.contains(&source.path));
        self.sources.extend(scan.sources);
        sort_sources(&mut self.sources);
        if let Some(global) = global {
            annotate_managed(self, global);
        }
    }
}

fn resolve_claude(directory: &Path, paths: &Paths) -> Audit {
    let mut sources = Vec::new();
    let mut seen = HashSet::new();
    let config_dir = claude_config_dir(paths);

    #[cfg(target_os = "macos")]
    let policy = Path::new("/Library/Application Support/ClaudeCode/CLAUDE.md");
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let policy = Path::new("/etc/claude-code/CLAUDE.md");
    #[cfg(target_os = "windows")]
    let policy = Path::new(r"C:\Program Files\ClaudeCode\CLAUDE.md");
    #[cfg(not(any(
        target_os = "macos",
        target_os = "linux",
        target_os = "android",
        target_os = "windows"
    )))]
    let policy = Path::new("/etc/claude-code/CLAUDE.md");

    push_loaded(
        &mut sources,
        &mut seen,
        policy,
        "managed policy",
        paths,
        directory,
    );
    push_loaded(
        &mut sources,
        &mut seen,
        &config_dir.join("CLAUDE.md"),
        "user",
        paths,
        directory,
    );
    push_rules(
        &mut sources,
        &mut seen,
        &config_dir.join("rules"),
        "user rule",
        paths,
        directory,
    );

    for ancestor in ancestors_from_root(directory) {
        let direct = ancestor.join("CLAUDE.md");
        let nested = ancestor.join(".claude/CLAUDE.md");
        let direct_exists = regular_file(&direct);
        let nested_exists = regular_file(&nested);
        if direct_exists && nested_exists && !seen.contains(&nested) {
            let reason = "both CLAUDE.md locations exist at this directory level; their order is undocumented";
            push_with_state(
                &mut sources,
                &mut seen,
                &direct,
                &scope_for(&ancestor, directory),
                SourceState::Ambiguous(reason.into()),
                paths,
                directory,
            );
            push_with_state(
                &mut sources,
                &mut seen,
                &nested,
                &scope_for(&ancestor, directory),
                SourceState::Ambiguous(reason.into()),
                paths,
                directory,
            );
        } else {
            push_loaded(
                &mut sources,
                &mut seen,
                &direct,
                &scope_for(&ancestor, directory),
                paths,
                directory,
            );
            push_loaded(
                &mut sources,
                &mut seen,
                &nested,
                &scope_for(&ancestor, directory),
                paths,
                directory,
            );
        }
        push_loaded(
            &mut sources,
            &mut seen,
            &ancestor.join("CLAUDE.local.md"),
            "local",
            paths,
            directory,
        );
        push_rules(
            &mut sources,
            &mut seen,
            &ancestor.join(".claude/rules"),
            "project rule",
            paths,
            directory,
        );
    }

    annotate_claude_imports(&mut sources, paths);
    apply_claude_exclusions(&mut sources, directory, paths);
    sort_sources(&mut sources);

    let startup = sources
        .iter()
        .filter(|source| matches!(source.state, SourceState::Startup))
        .count();
    Audit {
        runtime: ContextRuntime::Claude,
        directory: directory.to_owned(),
        sources,
        warnings: Vec::new(),
        summary: format!("{startup} load at startup"),
    }
}

pub(crate) fn scan_claude_descendants(
    directory: &Path,
    paths: &Paths,
    mut seen: HashSet<PathBuf>,
) -> ClaudeScan {
    let mut sources = Vec::new();
    let mut unreadable = Vec::new();
    let entries = WalkDir::new(directory)
        .follow_links(true)
        .min_depth(1)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|entry| {
            entry.file_name() != ".git"
                && (!entry.path().is_symlink()
                    || !entry.path().is_dir()
                    || is_rule_path(entry.path()))
        });

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                if let Some(path) = error.path()
                    && is_claude_instruction_path(path)
                {
                    unreadable.push(path.to_owned());
                }
                continue;
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let name = entry.file_name().to_string_lossy();
        if matches!(name.as_ref(), "CLAUDE.md" | "CLAUDE.local.md")
            && path.parent().is_some_and(|parent| parent != directory)
            && !is_rule_path(path)
        {
            push_with_state(
                &mut sources,
                &mut seen,
                path,
                "nested instruction",
                SourceState::Nested(format!(
                    "Claude can load this after working with files under {}",
                    display_path(path.parent().unwrap_or(directory), paths, directory)
                )),
                paths,
                directory,
            );
        } else if path.extension().is_some_and(|extension| extension == "md")
            && is_rule_path(path)
            && !seen.contains(path)
        {
            let content = fs::read_to_string(path).unwrap_or_default();
            let state = claude_rule_state(
                &content,
                SourceState::Nested(
                    "Claude can load this after working with files below its nested rule directory"
                        .into(),
                ),
            );
            push_with_state(
                &mut sources,
                &mut seen,
                path,
                "nested rule",
                state,
                paths,
                directory,
            );
        }
    }

    annotate_claude_imports(&mut sources, paths);
    apply_claude_exclusions(&mut sources, directory, paths);
    sort_sources(&mut sources);
    ClaudeScan {
        sources,
        unreadable,
    }
}

fn apply_claude_exclusions(sources: &mut [ContextSource], directory: &Path, paths: &Paths) {
    let mut settings = vec![claude_config_dir(paths).join("settings.json")];
    for ancestor in ancestors_from_root(directory) {
        settings.push(ancestor.join(".claude/settings.json"));
        settings.push(ancestor.join(".claude/settings.local.json"));
    }
    if let Ok(main_checkout) = crate::git_worktree::worktree_root(directory)
        .and_then(|root| crate::git_worktree::main_worktree(&root))
    {
        settings.push(main_checkout.join(".claude/settings.local.json"));
    }
    settings.sort();
    settings.dedup();
    let mut patterns = Vec::new();
    for settings_path in settings {
        let Ok(source) = fs::read_to_string(&settings_path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&source) else {
            continue;
        };
        let Some(entries) = value
            .get("claudeMdExcludes")
            .and_then(serde_json::Value::as_array)
        else {
            continue;
        };
        patterns.extend(
            entries
                .iter()
                .filter_map(serde_json::Value::as_str)
                .filter_map(|raw| {
                    glob::Pattern::new(raw)
                        .ok()
                        .map(|pattern| (raw.to_owned(), pattern, settings_path.clone()))
                }),
        );
    }
    for source in sources {
        if source.scope == "managed policy" {
            continue;
        }
        if let Some((_, _, settings_path)) = patterns.iter().find(|(raw, pattern, _)| {
            pattern.matches_path(&source.path)
                || (Path::new(raw).is_absolute()
                    && fs::canonicalize(raw).ok() == fs::canonicalize(&source.path).ok())
        }) {
            source.state = SourceState::Excluded(format!(
                "excluded by {}",
                display_path(settings_path, paths, directory)
            ));
        }
    }
}

fn annotate_claude_imports(sources: &mut [ContextSource], paths: &Paths) {
    for source in sources {
        let name = source.path.file_name().and_then(|name| name.to_str());
        if !matches!(name, Some("CLAUDE.md" | "CLAUDE.local.md")) {
            continue;
        }
        let parent = source.path.parent().unwrap_or(Path::new("."));
        source.imports = claude_imports(&source.content, parent, &paths.home);
    }
}

fn claude_imports(content: &str, parent: &Path, home: &Path) -> Vec<PathBuf> {
    let mut imports = markdown_without_code(content)
        .split_whitespace()
        .filter_map(|word| word.strip_prefix('@'))
        .map(|word| word.trim_start_matches(['\'', '"', '(', '[', '{']))
        .map(|word| word.trim_end_matches(['\'', '"', ')', ']', '}', ',', ';', ':', '.', '!', '?']))
        .filter(|word| !word.is_empty())
        .filter(|word| {
            Path::new(word)
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
        })
        .map(|word| {
            if let Some(relative) = word.strip_prefix("~/") {
                home.join(relative)
            } else {
                let path = Path::new(word);
                if path.is_absolute() {
                    path.to_owned()
                } else {
                    parent.join(path)
                }
            }
        })
        .collect::<Vec<_>>();
    imports.sort();
    imports.dedup();
    imports
}

fn markdown_without_code(content: &str) -> String {
    let mut output = String::with_capacity(content.len());
    let mut fence: Option<(char, usize)> = None;
    for line in content.lines() {
        let trimmed = line.trim_start();
        let marker = trimmed
            .chars()
            .next()
            .filter(|character| matches!(character, '`' | '~'));
        let marker_len = marker.map_or(0, |marker| {
            trimmed
                .chars()
                .take_while(|character| *character == marker)
                .count()
        });
        if let Some((open_marker, open_len)) = fence {
            if marker == Some(open_marker) && marker_len >= open_len {
                fence = None;
            }
            output.push('\n');
            continue;
        }
        if marker_len >= 3 {
            fence = marker.map(|marker| (marker, marker_len));
            output.push('\n');
            continue;
        }

        let mut remainder = line;
        while let Some(start) = remainder.find('`') {
            output.push_str(&remainder[..start]);
            let ticks = remainder[start..]
                .chars()
                .take_while(|character| *character == '`')
                .count();
            let delimiter = "`".repeat(ticks);
            let after_open = &remainder[start + ticks..];
            let Some(end) = after_open.find(&delimiter) else {
                output.push_str(&remainder[start..]);
                remainder = "";
                break;
            };
            output.push(' ');
            remainder = &after_open[end + ticks..];
        }
        output.push_str(remainder);
        output.push('\n');
    }
    output
}

fn resolve_codex(directory: &Path, paths: &Paths) -> Audit {
    let process_home = env::var_os("HOME").map(PathBuf::from);
    let codex_home = if process_home.as_ref() == Some(&paths.home) {
        env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| paths.home.join(".codex"))
    } else {
        paths.home.join(".codex")
    };
    let settings = codex_settings(&codex_home);
    let mut sources = Vec::new();

    push_priority_candidates(
        &mut sources,
        &[
            codex_home.join("AGENTS.override.md"),
            codex_home.join("AGENTS.md"),
        ],
        "user",
        true,
        None,
        paths,
        directory,
    );

    let root = directory
        .ancestors()
        .find(|ancestor| {
            settings
                .root_markers
                .iter()
                .any(|marker| ancestor.join(marker).exists())
        })
        .unwrap_or(directory);
    let mut remaining = settings.max_bytes;
    for ancestor in ancestors_from(root, directory) {
        let mut candidates = vec![
            ancestor.join("AGENTS.override.md"),
            ancestor.join("AGENTS.md"),
        ];
        candidates.extend(
            settings
                .fallback_names
                .iter()
                .map(|name| ancestor.join(name)),
        );
        push_priority_candidates(
            &mut sources,
            &candidates,
            &scope_for(&ancestor, directory),
            true,
            Some(&mut remaining),
            paths,
            directory,
        );
    }

    let loaded = sources
        .iter()
        .filter(|source| {
            matches!(
                source.state,
                SourceState::Startup | SourceState::Truncated { .. }
            )
        })
        .count();
    let used = settings.max_bytes.saturating_sub(remaining);
    Audit {
        runtime: ContextRuntime::Codex,
        directory: directory.to_owned(),
        sources,
        warnings: settings.warning.into_iter().collect(),
        summary: format!(
            "{loaded} load at startup · {used}/{} project instruction bytes",
            settings.max_bytes,
        ),
    }
}

fn resolve_cursor(directory: &Path, paths: &Paths) -> Audit {
    let root = directory
        .ancestors()
        .find(|ancestor| ancestor.join(".git").exists())
        .unwrap_or(directory);
    let mut sources = Vec::new();
    let mut seen = HashSet::new();

    for ancestor in ancestors_from(root, directory) {
        let agents = ancestor.join("AGENTS.md");
        if ancestor == root {
            push_loaded(
                &mut sources,
                &mut seen,
                &agents,
                &scope_for(&ancestor, directory),
                paths,
                directory,
            );
        } else if regular_file(&agents) {
            push_with_state(
                &mut sources,
                &mut seen,
                &agents,
                &scope_for(&ancestor, directory),
                SourceState::Relevant(format!(
                    "Cursor applies this when working with files under {}",
                    display_path(&ancestor, paths, directory)
                )),
                paths,
                directory,
            );
        }
        push_cursor_rules(
            &mut sources,
            &mut seen,
            &ancestor.join(".cursor/rules"),
            paths,
            directory,
        );
    }

    let claude = root.join("CLAUDE.md");
    if regular_file(&claude) {
        push_with_state(
            &mut sources,
            &mut seen,
            &claude,
            &scope_for(root, directory),
            SourceState::Ambiguous(
                "loaded by Cursor CLI; Cursor editor behavior is not documented".into(),
            ),
            paths,
            directory,
        );
    }

    sort_sources(&mut sources);
    let startup = sources
        .iter()
        .filter(|source| matches!(source.state, SourceState::Startup))
        .count();
    Audit {
        runtime: ContextRuntime::Cursor,
        directory: directory.to_owned(),
        sources,
        warnings: Vec::new(),
        summary: format!(
            "{startup} load at startup · User and Team Rules live in Cursor settings and are not inspectable"
        ),
    }
}

fn push_cursor_rules(
    sources: &mut Vec<ContextSource>,
    seen: &mut HashSet<PathBuf>,
    root: &Path,
    paths: &Paths,
    directory: &Path,
) {
    if !root.is_dir() {
        return;
    }
    let mut rules = WalkDir::new(root)
        .follow_links(true)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "mdc")
        })
        .map(DirEntry::into_path)
        .collect::<Vec<_>>();
    rules.sort();
    for rule in rules {
        let state = fs::read_to_string(&rule).map_or_else(
            |error| SourceState::Unreadable(error.to_string()),
            |content| cursor_rule_state(&content),
        );
        push_with_state(
            sources,
            seen,
            &rule,
            "project rule",
            state,
            paths,
            directory,
        );
    }
}

fn cursor_rule_state(content: &str) -> SourceState {
    let frontmatter = match rule_frontmatter(content) {
        Ok(Some(header)) => header,
        Err(reason) => return SourceState::Ambiguous(reason.into()),
        Ok(None) => {
            return SourceState::Ambiguous("Cursor rule has no complete YAML frontmatter".into());
        }
    };
    if frontmatter_value(&frontmatter, "alwaysApply") == Some("true") {
        return SourceState::Startup;
    }
    let globs = frontmatter_list(&frontmatter, "globs");
    if !globs.is_empty() {
        return SourceState::Conditional(format!("declares globs: {}", globs.join(", ")));
    }
    if let Some(description) = frontmatter_value(&frontmatter, "description")
        && !description.is_empty()
    {
        return SourceState::Relevant(format!(
            "Cursor decides when this rule is relevant: {description}"
        ));
    }
    SourceState::Relevant("loaded when explicitly selected in Cursor".into())
}

/// Rule frontmatter models complete delimiter headers and simple lists, not general YAML.
/// Unterminated rule frontmatter is ambiguous for both runtimes.
fn rule_frontmatter(content: &str) -> Result<Option<Vec<&str>>, &'static str> {
    let mut lines = content.lines();
    if lines.next() != Some("---") {
        return Ok(None);
    }
    let mut frontmatter = Vec::new();
    for line in lines {
        if line == "---" {
            return Ok(Some(frontmatter));
        }
        frontmatter.push(line);
    }
    Err("rule frontmatter has no closing --- delimiter")
}

fn frontmatter_value<'a>(frontmatter: &'a [&str], key: &str) -> Option<&'a str> {
    frontmatter.iter().find_map(|line| {
        line.trim_start()
            .strip_prefix(key)
            .and_then(|value| value.strip_prefix(':'))
            .map(str::trim)
            .map(|value| value.trim_matches(['\'', '"']))
    })
}

fn frontmatter_list(frontmatter: &[&str], key: &str) -> Vec<String> {
    let Some(start) = frontmatter.iter().position(|line| {
        line.trim_start()
            .strip_prefix(key)
            .is_some_and(|value| value.starts_with(':'))
    }) else {
        return Vec::new();
    };
    let inline = frontmatter[start]
        .trim_start()
        .strip_prefix(key)
        .and_then(|value| value.strip_prefix(':'))
        .unwrap_or("")
        .trim();
    if !inline.is_empty() {
        return inline
            .trim_matches(['[', ']'])
            .split(',')
            .map(|value| value.trim().trim_matches(['\'', '"']).to_owned())
            .filter(|value| !value.is_empty())
            .collect();
    }
    frontmatter
        .iter()
        .skip(start + 1)
        .take_while(|line| line.trim_start().starts_with('-') || line.trim().is_empty())
        .filter_map(|line| line.trim().strip_prefix('-'))
        .map(|value| value.trim().trim_matches(['\'', '"']).to_owned())
        .filter(|value| !value.is_empty())
        .collect()
}

fn resolve_pi(directory: &Path, paths: &Paths) -> Audit {
    let mut sources = Vec::new();
    let global = pi_agent_dir(paths);
    push_priority_candidates(
        &mut sources,
        &PI_INSTRUCTION_CANDIDATES
            .iter()
            .map(|name| global.join(name))
            .collect::<Vec<_>>(),
        "user",
        false,
        None,
        paths,
        directory,
    );
    for ancestor in ancestors_from_root(directory) {
        push_priority_candidates(
            &mut sources,
            &PI_INSTRUCTION_CANDIDATES
                .iter()
                .map(|name| ancestor.join(name))
                .collect::<Vec<_>>(),
            &scope_for(&ancestor, directory),
            false,
            None,
            paths,
            directory,
        );
    }
    let loaded = sources
        .iter()
        .filter(|source| matches!(source.state, SourceState::Startup))
        .count();
    Audit {
        runtime: ContextRuntime::Pi,
        directory: directory.to_owned(),
        sources,
        warnings: Vec::new(),
        summary: format!("{loaded} load at startup"),
    }
}

#[derive(Debug)]
struct CodexSettings {
    root_markers: Vec<String>,
    fallback_names: Vec<String>,
    max_bytes: usize,
    warning: Option<String>,
}

#[derive(Deserialize)]
struct RawCodexSettings {
    project_root_markers: Option<Vec<String>>,
    project_doc_fallback_filenames: Option<Vec<String>>,
    project_doc_max_bytes: Option<usize>,
}

fn codex_settings(home: &Path) -> CodexSettings {
    let mut settings = CodexSettings {
        root_markers: vec![".git".into()],
        fallback_names: Vec::new(),
        max_bytes: 32 * 1024,
        warning: None,
    };
    let path = home.join("config.toml");
    let source = match fs::read_to_string(&path) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return settings,
        Err(error) => {
            settings.warning = Some(format!("cannot read {}: {error}", path.display()));
            return settings;
        }
    };
    let value = match toml::from_str::<RawCodexSettings>(&source) {
        Ok(value) => value,
        Err(error) => {
            settings.warning = Some(format!("invalid {}: {error}", path.display()));
            return settings;
        }
    };
    if let Some(markers) = value.project_root_markers
        && !markers.is_empty()
    {
        settings.root_markers = markers;
    }
    if let Some(names) = value.project_doc_fallback_filenames {
        settings.fallback_names = names;
    }
    if let Some(value) = value.project_doc_max_bytes {
        if value > 0 {
            settings.max_bytes = value;
        } else {
            settings.warning = Some(
                "project_doc_max_bytes is zero; using the documented default until boundary behavior is verified"
                    .into(),
            );
        }
    }
    settings
}

fn push_priority_candidates(
    sources: &mut Vec<ContextSource>,
    candidates: &[PathBuf],
    scope: &str,
    skip_empty: bool,
    mut remaining: Option<&mut usize>,
    paths: &Paths,
    directory: &Path,
) {
    let existing = candidates
        .iter()
        .filter(|path| regular_file(path))
        .collect::<Vec<_>>();
    let winner = existing
        .iter()
        .find(|path| {
            !skip_empty
                || fs::read(path)
                    .map(|bytes| !bytes.is_empty())
                    .unwrap_or(false)
        })
        .map(|path| (*path).clone());
    for path in existing {
        let bytes = fs::read(path);
        let state = match bytes {
            Err(error) => SourceState::Unreadable(error.to_string()),
            Ok(bytes) if bytes.is_empty() => {
                if winner.as_ref().is_some_and(|winner| winner == path) {
                    SourceState::SelectedEmpty
                } else {
                    SourceState::Empty
                }
            }
            Ok(bytes) if winner.as_ref().is_some_and(|winner| winner == path) => {
                if let Some(remaining) = remaining.as_deref_mut() {
                    let included = bytes.len().min(*remaining);
                    *remaining -= included;
                    if included < bytes.len() {
                        SourceState::Truncated {
                            included,
                            total: bytes.len(),
                        }
                    } else {
                        SourceState::Startup
                    }
                } else {
                    SourceState::Startup
                }
            }
            Ok(_) => SourceState::Shadowed(winner.clone().unwrap_or_default()),
        };
        push_source(
            sources,
            path,
            scope,
            SourceGroup::Startup,
            state,
            paths,
            directory,
        );
    }
}

fn push_rules(
    sources: &mut Vec<ContextSource>,
    seen: &mut HashSet<PathBuf>,
    root: &Path,
    scope: &str,
    paths: &Paths,
    directory: &Path,
) {
    if !root.is_dir() {
        return;
    }
    let mut rules = WalkDir::new(root)
        .follow_links(true)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "md")
        })
        .map(DirEntry::into_path)
        .collect::<Vec<_>>();
    rules.sort();
    for rule in rules {
        let content = fs::read_to_string(&rule).unwrap_or_default();
        let state = claude_rule_state(&content, SourceState::Startup);
        push_with_state(sources, seen, &rule, scope, state, paths, directory);
    }
}

pub(crate) fn claude_config_dir(paths: &Paths) -> PathBuf {
    runtime_config_dir(paths, "CLAUDE_CONFIG_DIR", ".claude")
}

pub(crate) fn pi_agent_dir(paths: &Paths) -> PathBuf {
    runtime_config_dir(paths, "PI_CODING_AGENT_DIR", ".pi/agent")
}

fn runtime_config_dir(paths: &Paths, variable: &str, default: &str) -> PathBuf {
    let process_home = env::var_os("HOME").map(PathBuf::from);
    let configured = (process_home.as_ref() == Some(&paths.home))
        .then(|| env::var_os(variable))
        .flatten()
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    match configured {
        Some(path) => path
            .strip_prefix("~")
            .map_or(path.clone(), |relative| paths.home.join(relative)),
        None => paths.home.join(default),
    }
}

fn rule_paths(content: &str) -> Result<Vec<String>, &'static str> {
    Ok(rule_frontmatter(content)?
        .map(|header| frontmatter_list(&header, "paths"))
        .unwrap_or_default())
}

fn claude_rule_state(content: &str, unconditional: SourceState) -> SourceState {
    match rule_paths(content) {
        Err(reason) => SourceState::Ambiguous(reason.into()),
        Ok(patterns) if patterns.is_empty() => unconditional,
        Ok(patterns) => {
            SourceState::Conditional(format!("declares paths: {}", patterns.join(", ")))
        }
    }
}

fn push_loaded(
    sources: &mut Vec<ContextSource>,
    seen: &mut HashSet<PathBuf>,
    path: &Path,
    scope: &str,
    paths: &Paths,
    directory: &Path,
) {
    if regular_file(path) {
        push_with_state(
            sources,
            seen,
            path,
            scope,
            SourceState::Startup,
            paths,
            directory,
        );
    }
}

fn push_with_state(
    sources: &mut Vec<ContextSource>,
    seen: &mut HashSet<PathBuf>,
    path: &Path,
    scope: &str,
    state: SourceState,
    paths: &Paths,
    directory: &Path,
) {
    let group = match state {
        SourceState::Conditional(_) | SourceState::Relevant(_) => SourceGroup::PathFiltered,
        SourceState::Nested(_) => SourceGroup::Nested,
        _ => SourceGroup::Startup,
    };
    if seen.insert(path.to_owned()) {
        push_source(sources, path, scope, group, state, paths, directory);
    }
}

fn push_source(
    sources: &mut Vec<ContextSource>,
    path: &Path,
    scope: &str,
    group: SourceGroup,
    mut state: SourceState,
    paths: &Paths,
    directory: &Path,
) {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) => {
            state = SourceState::Unreadable(error.to_string());
            String::new()
        }
    };
    sources.push(ContextSource {
        path: path.to_owned(),
        display: display_path(path, paths, directory),
        scope: scope.into(),
        group,
        state,
        content,
        imports: Vec::new(),
        managed: None,
    });
}

fn annotate_managed(audit: &mut Audit, global: &GlobalConfig) {
    let Some(active) = deploy::active_profile(global).ok().flatten() else {
        return;
    };
    let views = deploy::inspect(global, &active).ok();
    let Ok(targets) = global.profile_target_names(&active) else {
        return;
    };
    for target in targets {
        let Ok(path) = global.target_path(target) else {
            continue;
        };
        let status = views.as_ref().and_then(|views| {
            views
                .iter()
                .find(|view| view.id == target)
                .map(|view| view.status.label().to_owned())
        });
        for source in &mut audit.sources {
            if source.path == path {
                source.managed = Some(ManagedSource {
                    target: target.to_owned(),
                    profile: Some(active.clone()),
                    status: status.clone(),
                });
            }
        }
    }
}

fn sort_sources(sources: &mut [ContextSource]) {
    sources.sort_by_key(|source| match source.group {
        SourceGroup::Startup => 0,
        SourceGroup::PathFiltered => 1,
        SourceGroup::Nested => 2,
    });
}

fn regular_file(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|metadata| metadata.is_file())
}

fn is_rule_path(path: &Path) -> bool {
    let components = path.components().collect::<Vec<_>>();
    components
        .windows(2)
        .any(|pair| pair[0].as_os_str() == ".claude" && pair[1].as_os_str() == "rules")
}

fn is_claude_instruction_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| matches!(name, "CLAUDE.md" | "CLAUDE.local.md"))
        || is_rule_path(path)
}

fn ancestors_from_root(path: &Path) -> Vec<PathBuf> {
    let mut ancestors = path.ancestors().map(Path::to_owned).collect::<Vec<_>>();
    ancestors.reverse();
    ancestors
}

fn ancestors_from(root: &Path, directory: &Path) -> Vec<PathBuf> {
    ancestors_from_root(directory)
        .into_iter()
        .skip_while(|ancestor| ancestor != root)
        .collect()
}

fn scope_for(path: &Path, directory: &Path) -> String {
    if path == directory {
        "launch directory".into()
    } else {
        "project ancestor".into()
    }
}

pub(crate) fn display_path(path: &Path, paths: &Paths, directory: &Path) -> String {
    if let Ok(relative) = path.strip_prefix(directory) {
        if relative.as_os_str().is_empty() {
            return ".".into();
        }
        return format!("./{}", relative.display());
    }
    if let Ok(relative) = path.strip_prefix(&paths.home) {
        if relative.as_os_str().is_empty() {
            return "~".into();
        }
        return format!("~/{}", relative.display());
    }
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn paths(home: &Path) -> Paths {
        Paths::for_home(home)
    }

    #[test]
    fn claude_separates_startup_and_conditional_sources() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("repo");
        let cwd = root.join("app");
        fs::create_dir_all(cwd.join("tests")).unwrap();
        fs::create_dir_all(cwd.join(".claude/rules")).unwrap();
        fs::write(root.join("CLAUDE.md"), "root").unwrap();
        fs::write(cwd.join("CLAUDE.md"), "app").unwrap();
        fs::write(cwd.join("tests/CLAUDE.md"), "tests").unwrap();
        fs::write(
            cwd.join(".claude/rules/testing.md"),
            "---\npaths:\n  - 'tests/**'\n---\nTest rules\n",
        )
        .unwrap();

        let mut audit = Audit::resolve(ContextRuntime::Claude, &cwd, &paths(temp.path()), None);
        assert!(audit.sources.iter().any(|source| {
            source.path == cwd.join("CLAUDE.md") && matches!(source.state, SourceState::Startup)
        }));
        assert!(
            audit
                .sources
                .iter()
                .all(|source| source.path != cwd.join("tests/CLAUDE.md"))
        );

        let seen = audit
            .sources
            .iter()
            .map(|source| source.path.clone())
            .collect();
        let scan = scan_claude_descendants(&cwd, &paths(temp.path()), seen);
        audit.add_claude_scan(scan, None);
        assert!(audit.sources.iter().any(|source| {
            source.path == cwd.join("tests/CLAUDE.md") && source.group == SourceGroup::Nested
        }));
        assert!(audit.sources.iter().any(|source| {
            source.path == cwd.join(".claude/rules/testing.md")
                && source.group == SourceGroup::PathFiltered
        }));
    }

    #[cfg(unix)]
    #[test]
    fn descendant_scan_ignores_unrelated_traversal_errors() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let cwd = temp.path().join("repo");
        fs::create_dir_all(cwd.join("nested")).unwrap();
        symlink("missing", cwd.join("unrelated")).unwrap();
        symlink("missing", cwd.join("nested/CLAUDE.md")).unwrap();

        let scan = scan_claude_descendants(&cwd, &paths(temp.path()), HashSet::new());

        assert!(!scan.unreadable.contains(&cwd.join("unrelated")));
        assert!(scan.unreadable.contains(&cwd.join("nested/CLAUDE.md")));
    }

    #[test]
    fn claude_user_file_does_not_create_a_home_level_ambiguity() {
        let temp = TempDir::new().unwrap();
        let cwd = temp.path().join("repo");
        fs::create_dir_all(temp.path().join(".claude")).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        fs::write(temp.path().join(".claude/CLAUDE.md"), "user").unwrap();
        fs::write(temp.path().join("CLAUDE.md"), "home project").unwrap();

        let audit = Audit::resolve(ContextRuntime::Claude, &cwd, &paths(temp.path()), None);
        for source in audit.sources.iter().filter(|source| {
            source.path == temp.path().join(".claude/CLAUDE.md")
                || source.path == temp.path().join("CLAUDE.md")
        }) {
            assert!(matches!(source.state, SourceState::Startup));
        }
    }

    #[cfg(unix)]
    #[test]
    fn claude_follows_symlinked_rule_files_and_directories() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let cwd = temp.path().join("repo");
        let shared = temp.path().join("shared-rules");
        fs::create_dir_all(cwd.join(".claude/rules")).unwrap();
        fs::create_dir_all(&shared).unwrap();
        fs::write(shared.join("directory.md"), "directory rule").unwrap();
        fs::write(temp.path().join("file.md"), "file rule").unwrap();
        symlink(&shared, cwd.join(".claude/rules/shared")).unwrap();
        symlink(
            temp.path().join("file.md"),
            cwd.join(".claude/rules/file.md"),
        )
        .unwrap();

        let audit = Audit::resolve(ContextRuntime::Claude, &cwd, &paths(temp.path()), None);
        assert!(
            audit
                .sources
                .iter()
                .any(|source| source.path == cwd.join(".claude/rules/file.md"))
        );
        assert!(
            audit
                .sources
                .iter()
                .any(|source| source.path == cwd.join(".claude/rules/shared/directory.md"))
        );
    }

    #[test]
    fn claude_local_exclusion_is_visible_in_context() {
        let temp = TempDir::new().unwrap();
        let cwd = temp.path().join("repo");
        fs::create_dir_all(cwd.join(".claude")).unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .current_dir(&cwd)
                .status()
                .unwrap()
                .success()
        );
        let instruction = cwd.join("CLAUDE.md");
        fs::write(&instruction, "project").unwrap();
        fs::write(
            cwd.join(".claude/settings.local.json"),
            format!(
                "{{\"claudeMdExcludes\":[{}]}}",
                serde_json::to_string(&instruction.display().to_string()).unwrap()
            ),
        )
        .unwrap();

        let audit = Audit::resolve(ContextRuntime::Claude, &cwd, &paths(temp.path()), None);
        let source = audit
            .sources
            .iter()
            .find(|source| source.path == instruction)
            .unwrap();
        assert!(matches!(source.state, SourceState::Excluded(_)));
        assert!(
            source
                .state
                .reason(&paths(temp.path()), &cwd)
                .unwrap()
                .contains(".claude/settings.local.json")
        );
    }

    #[test]
    fn claude_exclusion_patterns_match_absolute_paths() {
        let temp = TempDir::new().unwrap();
        let cwd = temp.path().join("repo");
        fs::create_dir_all(cwd.join(".claude")).unwrap();
        let instruction = cwd.join("CLAUDE.md");
        fs::write(&instruction, "project").unwrap();
        fs::write(
            cwd.join(".claude/settings.local.json"),
            "{\"claudeMdExcludes\":[\"CLAUDE.md\"]}",
        )
        .unwrap();

        let audit = Audit::resolve(ContextRuntime::Claude, &cwd, &paths(temp.path()), None);
        let source = audit
            .sources
            .iter()
            .find(|source| source.path == instruction)
            .unwrap();
        assert!(matches!(source.state, SourceState::Startup));
    }

    #[cfg(unix)]
    #[test]
    fn claude_exclusion_through_a_symlink_matches_the_resolved_source() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let cwd = temp.path().join("repo");
        let linked = temp.path().join("linked-repo");
        fs::create_dir_all(cwd.join(".claude")).unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .current_dir(&cwd)
                .status()
                .unwrap()
                .success()
        );
        let instruction = cwd.join("CLAUDE.md");
        fs::write(&instruction, "project").unwrap();
        symlink(&cwd, &linked).unwrap();
        let linked_instruction = linked.join("CLAUDE.md");
        fs::write(
            cwd.join(".claude/settings.local.json"),
            format!(
                "{{\"claudeMdExcludes\":[{}]}}",
                serde_json::to_string(&linked_instruction.display().to_string()).unwrap()
            ),
        )
        .unwrap();

        let audit = Audit::resolve(ContextRuntime::Claude, &cwd, &paths(temp.path()), None);
        let source = audit
            .sources
            .iter()
            .find(|source| source.path == instruction)
            .unwrap();
        assert!(matches!(source.state, SourceState::Excluded(_)));
    }

    #[test]
    fn codex_skips_empty_overrides_and_applies_the_project_budget() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("repo");
        let cwd = root.join("app");
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(temp.path().join(".codex")).unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(
            temp.path().join(".codex/config.toml"),
            "project_doc_max_bytes = 5\n",
        )
        .unwrap();
        assert_eq!(codex_settings(&temp.path().join(".codex")).max_bytes, 5);
        fs::write(root.join("AGENTS.override.md"), "").unwrap();
        fs::write(root.join("AGENTS.md"), "123").unwrap();
        fs::write(cwd.join("AGENTS.md"), "456789").unwrap();
        fs::write(temp.path().join(".codex/AGENTS.md"), "user instructions").unwrap();

        let audit = Audit::resolve(ContextRuntime::Codex, &cwd, &paths(temp.path()), None);
        for path in [root.join("AGENTS.md"), temp.path().join(".codex/AGENTS.md")] {
            assert!(
                audit
                    .sources
                    .iter()
                    .any(|source| source.path == path && source.state == SourceState::Startup)
            );
        }
        assert!(audit.sources.iter().any(|source| {
            source.path == root.join("AGENTS.override.md")
                && matches!(source.state, SourceState::Empty)
        }));
        assert!(audit.sources.iter().any(|source| {
            source.path == cwd.join("AGENTS.md")
                && matches!(
                    source.state,
                    SourceState::Truncated {
                        included: 2,
                        total: 6
                    }
                )
        }));
    }

    #[test]
    fn codex_reports_invalid_settings_as_warnings() {
        let temp = TempDir::new().unwrap();
        let cwd = temp.path().join("repo");
        fs::create_dir_all(temp.path().join(".codex")).unwrap();
        fs::create_dir_all(cwd.join(".git")).unwrap();
        fs::write(temp.path().join(".codex/config.toml"), "invalid = [").unwrap();

        let audit = Audit::resolve(ContextRuntime::Codex, &cwd, &paths(temp.path()), None);
        assert!(!audit.warnings.is_empty());
        assert!(audit.warnings[0].contains("config.toml"));
    }

    #[test]
    fn cursor_reports_agents_rules_and_uncertain_claude_instructions() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("repo");
        let cwd = root.join("app");
        fs::create_dir_all(cwd.join(".cursor/rules")).unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir_all(root.join(".cursor/rules")).unwrap();
        fs::write(root.join("AGENTS.md"), "root agents").unwrap();
        fs::write(cwd.join("AGENTS.md"), "app agents").unwrap();
        fs::write(root.join("AGENTS.override.md"), "not Cursor").unwrap();
        fs::write(root.join("CLAUDE.md"), "Cursor CLI").unwrap();
        fs::write(
            root.join(".cursor/rules/always.mdc"),
            "---\nalwaysApply: true\n---\nalways\n",
        )
        .unwrap();
        fs::write(
            root.join(".cursor/rules/rust.mdc"),
            "---\nglobs: ['**/*.rs', 'Cargo.toml']\nalwaysApply: false\n---\nrust\n",
        )
        .unwrap();
        fs::write(
            cwd.join(".cursor/rules/testing.mdc"),
            "---\ndescription: Use for tests\nalwaysApply: false\n---\ntests\n",
        )
        .unwrap();
        fs::write(root.join(".cursor/rules/ignored.md"), "ignored").unwrap();

        let audit = Audit::resolve(ContextRuntime::Cursor, &cwd, &paths(temp.path()), None);

        assert!(audit.summary.contains("not inspectable"));
        assert!(audit.sources.iter().any(|source| {
            source.path == root.join("AGENTS.md") && matches!(source.state, SourceState::Startup)
        }));
        assert!(audit.sources.iter().any(|source| {
            source.path == cwd.join("AGENTS.md") && matches!(source.state, SourceState::Relevant(_))
        }));
        assert!(audit.sources.iter().any(|source| {
            source.path == root.join(".cursor/rules/always.mdc")
                && matches!(source.state, SourceState::Startup)
        }));
        assert!(audit.sources.iter().any(|source| {
            source.path == root.join(".cursor/rules/rust.mdc")
                && matches!(source.state, SourceState::Conditional(_))
        }));
        assert!(audit.sources.iter().any(|source| {
            source.path == cwd.join(".cursor/rules/testing.mdc")
                && matches!(source.state, SourceState::Relevant(_))
        }));
        assert!(audit.sources.iter().any(|source| {
            source.path == root.join("CLAUDE.md")
                && matches!(source.state, SourceState::Ambiguous(_))
        }));
        assert!(
            audit
                .sources
                .iter()
                .all(|source| source.path != root.join("AGENTS.override.md"))
        );
        assert!(
            audit
                .sources
                .iter()
                .all(|source| source.path != root.join(".cursor/rules/ignored.md"))
        );
    }

    #[test]
    fn claude_imports_preserve_relative_paths_and_ignore_code() {
        let parent = Path::new("/repo/docs");
        let home = Path::new("/home/test");
        let imports = claude_imports(
            "@./local.md @../shared.md `@inline.md`\n```md\n@fenced.md\n```\n~~~\n@also-fenced.md\n~~~\n",
            parent,
            home,
        );

        assert_eq!(
            imports,
            [parent.join("../shared.md"), parent.join("./local.md")]
        );
    }

    #[test]
    fn pi_uses_the_first_existing_candidate_even_when_empty() {
        let temp = TempDir::new().unwrap();
        let cwd = temp.path().join("repo");
        fs::create_dir_all(&cwd).unwrap();
        fs::write(cwd.join("AGENTS.override.md"), "").unwrap();
        fs::write(cwd.join("AGENTS.md"), "instructions").unwrap();

        let audit = Audit::resolve(ContextRuntime::Pi, &cwd, &paths(temp.path()), None);
        let override_source = audit
            .sources
            .iter()
            .find(|source| source.path == cwd.join("AGENTS.override.md"))
            .unwrap();
        let agents = audit
            .sources
            .iter()
            .find(|source| source.path == cwd.join("AGENTS.md"))
            .unwrap();
        assert!(matches!(override_source.state, SourceState::SelectedEmpty));
        assert!(matches!(agents.state, SourceState::Shadowed(_)));
        assert!(
            override_source
                .state
                .reason(&paths(temp.path()), &cwd)
                .is_some_and(|reason| reason.contains("contributes no instruction text"))
        );
    }

    #[test]
    fn parses_block_and_inline_rule_paths() {
        assert_eq!(
            rule_paths("---\npaths:\n  - 'src/**'\n  - \"tests/**\"\n---\n").unwrap(),
            ["src/**", "tests/**"]
        );
        assert_eq!(
            rule_paths("---\npaths: ['*.rs', 'Cargo.toml']\n---\n").unwrap(),
            ["*.rs", "Cargo.toml"]
        );
        for (key, runtime) in [
            ("paths", ContextRuntime::Claude),
            ("globs", ContextRuntime::Cursor),
        ] {
            for list in ["['src/**', 'tests/**']", "\n  - 'src/**'\n  - \"tests/**\""] {
                let content = format!("---\n{key}: {list}\n---\nbody");
                let state = match runtime {
                    ContextRuntime::Claude => claude_rule_state(&content, SourceState::Startup),
                    _ => cursor_rule_state(&content),
                };
                assert!(
                    matches!(state, SourceState::Conditional(reason) if reason.contains("src/**") && reason.contains("tests/**"))
                );
            }
            for metadata in [
                format!("{key}: ['src/**']"),
                "alwaysApply: true".into(),
                "body".into(),
            ] {
                let content = format!("---\n{metadata}\n");
                let state = match runtime {
                    ContextRuntime::Claude => claude_rule_state(&content, SourceState::Startup),
                    _ => cursor_rule_state(&content),
                };
                assert!(matches!(state, SourceState::Ambiguous(reason) if !reason.is_empty()));
            }
        }
    }

    #[test]
    fn omitted_leftover_global_target_is_not_managed() {
        let temp = TempDir::new().unwrap();
        let home = temp.path();
        let cwd = home.join("repo");
        fs::create_dir_all(&cwd).unwrap();
        let paths = Paths::for_home(home);
        fs::create_dir_all(paths.config.parent().unwrap().join("sections")).unwrap();
        fs::write(
            &paths.config,
            r#"
[[sections]]
id = "common"
name = "Common"
path = "sections/common.md"

[targets.pi]
path = "~/.pi/agent/AGENTS.md"
title = "Global Pi"

[targets.claude]
path = "~/.claude/CLAUDE.md"
title = "Global Claude"

[profiles.default]
pi = ["common"]
claude = ["common"]

[profiles.work]
claude = ["common"]
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
        deploy::apply(&global, "work", None, false, true).unwrap();
        let leftover = global.target_path("pi").unwrap();
        assert!(leftover.exists());

        let audit = Audit::resolve(ContextRuntime::Pi, &cwd, &paths, Some(&global));
        let source = audit
            .sources
            .iter()
            .find(|source| source.path == leftover)
            .unwrap();
        assert!(source.managed.is_none());

        let claude = Audit::resolve(ContextRuntime::Claude, &cwd, &paths, Some(&global));
        let claude_path = global.target_path("claude").unwrap();
        let claude_source = claude
            .sources
            .iter()
            .find(|source| source.path == claude_path)
            .unwrap();
        assert!(claude_source.managed.is_some());
    }
}
