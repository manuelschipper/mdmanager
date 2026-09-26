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
/// Runtime load-chain observation; managed ownership annotates sources without granting write authority.
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

    pub(crate) fn add_claude_scan(
        &mut self,
        mut scan: ClaudeScan,
        paths: &Paths,
        global: Option<&GlobalConfig>,
    ) {
        let existing = self
            .sources
            .iter()
            .map(|source| source.path.clone())
            .collect::<HashSet<_>>();
        scan.sources
            .retain(|source| !existing.contains(&source.path));
        self.sources.extend(scan.sources);
        // A startup CLAUDE.md can import a subfolder AGENTS.md the scan found.
        skip_agents_md_loaded_through_claude_md(&mut self.sources, paths);
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
    let instruction_files = claude_instruction_files(directory, paths);
    // By default a CLAUDE file on the launch path stops every AGENTS.md from loading.
    let agents_shadow = match instruction_files {
        ClaudeInstructionFiles::ClaudeMdOrAgentsMd => claude_md_on_path(directory, paths),
        _ => None,
    };

    push_loaded(
        &mut sources,
        &mut seen,
        &claude_managed_dir().join("CLAUDE.md"),
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
        if matches!(
            instruction_files,
            ClaudeInstructionFiles::ClaudeMdOrAgentsMd
                | ClaudeInstructionFiles::ClaudeMdAndAgentsMd
        ) {
            for agents in [
                ancestor.join("AGENTS.md"),
                ancestor.join(".claude/AGENTS.md"),
            ] {
                if regular_file(&agents) {
                    let state = agents_shadow
                        .clone()
                        .map_or(SourceState::Startup, SourceState::Shadowed);
                    push_with_state(
                        &mut sources,
                        &mut seen,
                        &agents,
                        &scope_for(&ancestor, directory),
                        state,
                        paths,
                        directory,
                    );
                }
            }
        }
        push_rules(
            &mut sources,
            &mut seen,
            &ancestor.join(".claude/rules"),
            "project rule",
            paths,
            directory,
        );
    }

    if instruction_files == ClaudeInstructionFiles::ManagedOnly {
        // Only managed instructions load at launch; path-scoped rules still load on demand.
        sources.retain(|source| {
            source.scope == "managed policy" || matches!(source.state, SourceState::Conditional(_))
        });
    }
    annotate_claude_imports(&mut sources, paths);
    skip_oversized_claude_files(&mut sources);
    apply_claude_exclusions(&mut sources, directory, paths);
    skip_agents_md_loaded_through_claude_md(&mut sources, paths);
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
    let instruction_files = claude_instruction_files(directory, paths);
    let agents_md_loads = match instruction_files {
        ClaudeInstructionFiles::ClaudeMdOrAgentsMd => claude_md_on_path(directory, paths).is_none(),
        ClaudeInstructionFiles::ClaudeMdAndAgentsMd => true,
        ClaudeInstructionFiles::ClaudeMd | ClaudeInstructionFiles::ManagedOnly => false,
    };
    let entries = WalkDir::new(directory)
        .follow_links(true)
        .min_depth(1)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|entry| {
            // Startup resolution owns the launch directory's .claude files.
            entry.file_name() != ".git"
                && entry.path() != directory.join(".claude")
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
        } else if agents_md_loads
            && name == "AGENTS.md"
            && let Some(parent) = path.parent().filter(|parent| *parent != directory)
            && !is_rule_path(path)
            && !path
                .components()
                .any(|component| component.as_os_str() == ".agents")
            && !(instruction_files == ClaudeInstructionFiles::ClaudeMdOrAgentsMd
                && has_claude_md(parent))
        {
            push_with_state(
                &mut sources,
                &mut seen,
                path,
                "nested instruction",
                SourceState::Nested(format!(
                    "Claude can load this after reading files under {}",
                    display_path(parent, paths, directory)
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
    skip_oversized_claude_files(&mut sources);
    apply_claude_exclusions(&mut sources, directory, paths);
    sort_sources(&mut sources);
    ClaudeScan {
        sources,
        unreadable,
    }
}

/// Claude skips an instruction file larger than 4 MiB instead of loading it.
const CLAUDE_MAX_INSTRUCTION_BYTES: u64 = 4 * 1024 * 1024;

fn skip_oversized_claude_files(sources: &mut [ContextSource]) {
    for source in sources {
        if matches!(
            source.state,
            SourceState::Shadowed(_) | SourceState::Unreadable(_)
        ) {
            continue;
        }
        let Ok(metadata) = fs::metadata(&source.path) else {
            continue;
        };
        if metadata.len() > CLAUDE_MAX_INSTRUCTION_BYTES {
            source.state = SourceState::Excluded(format!(
                "Claude skips instruction files over 4 MiB; this file is {} bytes",
                metadata.len()
            ));
        }
    }
}

fn apply_claude_exclusions(sources: &mut [ContextSource], directory: &Path, paths: &Paths) {
    // Arrays merge across every settings layer, managed settings included.
    let mut patterns = Vec::new();
    for settings_path in claude_settings_files(directory, paths) {
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
        // A pattern can name a symlinked file by its link path or its resolved target.
        let target = fs::canonicalize(&source.path).ok();
        if let Some((_, _, settings_path)) = patterns.iter().find(|(raw, pattern, _)| {
            pattern.matches_path(&source.path)
                || target
                    .as_ref()
                    .is_some_and(|target| pattern.matches_path(target))
                || (Path::new(raw).is_absolute()
                    && target.is_some()
                    && fs::canonicalize(raw).ok() == target)
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
        if !matches!(name, Some("CLAUDE.md" | "CLAUDE.local.md" | "AGENTS.md")) {
            continue;
        }
        let parent = source.path.parent().unwrap_or(Path::new("."));
        source.imports = claude_imports(&source.content, parent, &paths.home);
    }
}

/// Drops an AGENTS.md that a loading CLAUDE file already reads by symlink or import;
/// Claude reads it once, through that CLAUDE file. A subfolder CLAUDE file loads only on
/// demand, so it can hide only a subfolder AGENTS.md, never one that loads at startup.
fn skip_agents_md_loaded_through_claude_md(sources: &mut Vec<ContextSource>, paths: &Paths) {
    let loaded_by = |nested: bool| {
        let mut loaded = HashSet::new();
        for source in sources.iter().filter(|source| {
            is_claude_instruction_path(&source.path)
                && !is_rule_path(&source.path)
                && (source.group == SourceGroup::Nested) == nested
                && !matches!(
                    source.state,
                    SourceState::Excluded(_) | SourceState::Shadowed(_)
                )
        }) {
            loaded.extend(fs::canonicalize(&source.path).ok());
            // Claude expands imports up to four hops deep.
            let mut hop = source.imports.clone();
            for _ in 0..4 {
                let mut next = Vec::new();
                for import in hop {
                    if let Ok(canonical) = fs::canonicalize(&import)
                        && loaded.insert(canonical)
                    {
                        let content = fs::read_to_string(&import).unwrap_or_default();
                        let parent = import.parent().unwrap_or(Path::new("."));
                        next.extend(claude_imports(&content, parent, &paths.home));
                    }
                }
                hop = next;
            }
        }
        loaded
    };
    let startup = loaded_by(false);
    let nested = loaded_by(true);
    sources.retain(|source| {
        source.path.file_name() != Some("AGENTS.md".as_ref())
            || fs::canonicalize(&source.path).map_or(true, |path| {
                !(startup.contains(&path)
                    || source.group == SourceGroup::Nested && nested.contains(&path))
            })
    });
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

/// Content Claude scans for imports: code and block-level HTML comments are removed,
/// because Claude strips those comments before injecting the file.
fn markdown_without_code(content: &str) -> String {
    let mut output = String::with_capacity(content.len());
    let mut fence: Option<(char, usize)> = None;
    let mut in_comment = false;
    for line in content.lines() {
        let mut line = line;
        if fence.is_none() && (in_comment || line.trim_start().starts_with("<!--")) {
            let Some(end) = line.find("-->") else {
                in_comment = true;
                output.push('\n');
                continue;
            };
            in_comment = false;
            line = &line[end + 3..];
        }
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

/// Claude's **Project instructions** setting, which decides whether `AGENTS.md` loads.
/// The built-in `agents-md` plugin stores it; disabling that plugin reads `CLAUDE.md` only.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ClaudeInstructionFiles {
    /// Default: `AGENTS.md` loads only when no CLAUDE file is on the launch path.
    ClaudeMdOrAgentsMd,
    ClaudeMdAndAgentsMd,
    ClaudeMd,
    /// Only managed instructions at launch; subfolder files and path-scoped rules still load.
    ManagedOnly,
}

fn claude_instruction_files(directory: &Path, paths: &Paths) -> ClaudeInstructionFiles {
    let settings = claude_settings_files(directory, paths)
        .into_iter()
        .filter_map(|path| {
            let value =
                serde_json::from_str::<serde_json::Value>(&fs::read_to_string(&path).ok()?).ok()?;
            Some((path, value))
        })
        .collect::<Vec<_>>();
    let enabled = settings.iter().find_map(|(_, value)| {
        value
            .pointer("/enabledPlugins/agents-md@builtin")?
            .as_bool()
    });
    if enabled == Some(false) {
        return ClaudeInstructionFiles::ClaudeMd;
    }
    // Claude ignores pluginConfigs in project and local settings.
    let user = claude_config_dir(paths).join("settings.json");
    let configured = settings
        .iter()
        .filter(|(path, _)| path.starts_with(claude_managed_dir()) || *path == user)
        .find_map(|(_, value)| {
            value
                .pointer("/pluginConfigs/agents-md@builtin/options/instructionFiles")?
                .as_str()
        });
    match configured {
        Some("claude-md-and-agents-md") => ClaudeInstructionFiles::ClaudeMdAndAgentsMd,
        Some("claude-md") => ClaudeInstructionFiles::ClaudeMd,
        Some("managed-only") => ClaudeInstructionFiles::ManagedOnly,
        _ => ClaudeInstructionFiles::ClaudeMdOrAgentsMd,
    }
}

/// Claude settings files that apply to a session launched in `directory`, highest precedence
/// first: managed drop-ins and file, local, project, then user. Project settings come from the
/// launch directory only. In a Git repository the local file lives at the main checkout's root
/// and wins over one an older Claude left in the launch directory; outside Git, or when the
/// repository root is the home directory, it stays beside the project settings.
fn claude_settings_files(directory: &Path, paths: &Paths) -> Vec<PathBuf> {
    let managed = claude_managed_dir();
    let mut files = fs::read_dir(managed.join("managed-settings.d"))
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
                && !path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with('.'))
        })
        .collect::<Vec<_>>();
    // Claude merges drop-ins alphabetically, so later files win.
    files.sort();
    files.reverse();
    files.push(managed.join("managed-settings.json"));
    if let Ok(root) = crate::git_worktree::worktree_root(directory)
        && fs::canonicalize(&root).ok() != fs::canonicalize(&paths.home).ok()
        && let Ok(main_checkout) = crate::git_worktree::main_worktree(&root)
    {
        files.push(main_checkout.join(".claude/settings.local.json"));
    }
    files.push(directory.join(".claude/settings.local.json"));
    files.push(directory.join(".claude/settings.json"));
    files.push(claude_config_dir(paths).join("settings.json"));
    files.dedup();
    files
}

/// Files that stop Claude's default `AGENTS.md` loading at their directory level.
const CLAUDE_MD_FILES: [&str; 3] = ["CLAUDE.md", ".claude/CLAUDE.md", "CLAUDE.local.md"];

/// The nearest CLAUDE file on the launch path; the user and managed CLAUDE.md do not count.
fn claude_md_on_path(directory: &Path, paths: &Paths) -> Option<PathBuf> {
    let user = claude_config_dir(paths).join("CLAUDE.md");
    directory
        .ancestors()
        .flat_map(|ancestor| CLAUDE_MD_FILES.map(|name| ancestor.join(name)))
        .find(|path| *path != user && regular_file(path))
}

fn has_claude_md(directory: &Path) -> bool {
    CLAUDE_MD_FILES
        .iter()
        .any(|name| regular_file(&directory.join(name)))
}

/// `AGENTS.md` files Claude loads at startup in `directory` that a new `CLAUDE.local.md`
/// there would stop it reading under the default **Project instructions** setting.
pub(crate) fn agents_md_displaced_by_claude_local(directory: &Path, paths: &Paths) -> Vec<PathBuf> {
    if claude_instruction_files(directory, paths) != ClaudeInstructionFiles::ClaudeMdOrAgentsMd {
        return Vec::new();
    }
    resolve_claude(directory, paths)
        .sources
        .into_iter()
        .filter(|source| {
            source.path.file_name() == Some("AGENTS.md".as_ref())
                && source.state == SourceState::Startup
        })
        .map(|source| source.path)
        .collect()
}

/// Claude's system directory for organization-managed instructions and settings.
pub(crate) fn claude_managed_dir() -> &'static Path {
    #[cfg(target_os = "macos")]
    let directory = Path::new("/Library/Application Support/ClaudeCode");
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let directory = Path::new("/etc/claude-code");
    #[cfg(target_os = "windows")]
    let directory = Path::new(r"C:\Program Files\ClaudeCode");
    #[cfg(not(any(
        target_os = "macos",
        target_os = "linux",
        target_os = "android",
        target_os = "windows"
    )))]
    let directory = Path::new("/etc/claude-code");
    directory
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
    let untrusted = codex_untrusted_project(&settings, directory);
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
        let start = sources.len();
        let budget = remaining;
        push_priority_candidates(
            &mut sources,
            &candidates,
            &scope_for(&ancestor, directory),
            false,
            Some(&mut remaining),
            paths,
            directory,
        );
        if let Some(project) = &untrusted {
            remaining = budget;
            for source in &mut sources[start..] {
                source.state = SourceState::Excluded(format!(
                    "project {} is explicitly untrusted in {}",
                    project,
                    codex_home.join("config.toml").display()
                ));
            }
        }
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
    let shadowed = pi_shadowed_context_file(directory);
    for ancestor in ancestors_from_root(directory) {
        let start = sources.len();
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
        if let Some((main_file, worktree_file)) = &shadowed {
            for source in &mut sources[start..] {
                if matches!(
                    source.state,
                    SourceState::Startup | SourceState::SelectedEmpty
                ) && fs::canonicalize(&source.path).ok().as_ref() == Some(main_file)
                {
                    source.state = SourceState::Shadowed(worktree_file.clone());
                }
            }
        }
    }
    let mut seen = HashSet::new();
    sources.retain(|source| seen.insert(source.path.clone()));
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
    projects: std::collections::BTreeMap<String, CodexProjectSettings>,
}

#[derive(Deserialize)]
struct RawCodexSettings {
    project_root_markers: Option<Vec<String>>,
    project_doc_fallback_filenames: Option<Vec<String>>,
    project_doc_max_bytes: Option<usize>,
    #[serde(default)]
    projects: std::collections::BTreeMap<String, CodexProjectSettings>,
}

#[derive(Debug, Deserialize)]
struct CodexProjectSettings {
    trust_level: Option<String>,
}

fn codex_settings(home: &Path) -> CodexSettings {
    let mut settings = CodexSettings {
        root_markers: vec![".git".into()],
        fallback_names: Vec::new(),
        max_bytes: 32 * 1024,
        warning: None,
        projects: Default::default(),
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
    if let Some(markers) = value.project_root_markers {
        settings.root_markers = markers;
    }
    if let Some(names) = value.project_doc_fallback_filenames {
        let mut seen = HashSet::from(["AGENTS.override.md".to_owned(), "AGENTS.md".to_owned()]);
        settings.fallback_names = names
            .into_iter()
            .filter(|name| {
                if name.is_empty()
                    || matches!(name.as_str(), "." | "..")
                    || name.contains(['/', '\0'])
                    || cfg!(windows) && name.contains(['\\', ':'])
                {
                    return false;
                }
                seen.insert(name.clone())
            })
            .collect();
    }
    if let Some(value) = value.project_doc_max_bytes {
        settings.max_bytes = value;
    }
    settings.projects = value.projects;
    settings
}

fn codex_untrusted_project(settings: &CodexSettings, directory: &Path) -> Option<String> {
    let normalize = |path: &Path| {
        let key = path.to_string_lossy().into_owned();
        if cfg!(windows) {
            key.to_ascii_lowercase()
        } else {
            key
        }
    };
    for path in std::iter::once(directory.to_owned()).chain(codex_trust_root(directory)) {
        for key in fs::canonicalize(&path)
            .ok()
            .iter()
            .chain(std::iter::once(&path))
            .map(|path| normalize(path))
        {
            let matched = settings.projects.get_key_value(&key).or_else(|| {
                settings
                    .projects
                    .iter()
                    .find(|(candidate, _)| normalize(Path::new(candidate)) == key)
            });
            if let Some((key, project)) = matched {
                return (project.trust_level.as_deref() == Some("untrusted")).then(|| key.clone());
            }
        }
    }
    None
}

// Codex checks the launch directory first, then a verified main-checkout trust key.
fn codex_trust_root(directory: &Path) -> Option<PathBuf> {
    let root = directory.ancestors().find(|ancestor| {
        let git = ancestor.join(".git");
        git.exists() && (!git.is_dir() || git.join("HEAD").exists())
    })?;
    let dot_git = root.join(".git");
    if dot_git.is_dir() {
        return Some(root.to_owned());
    }
    let git_dir = codex_gitdir_target(&dot_git)?;
    let metadata = fs::symlink_metadata(&git_dir).ok()?;
    if !metadata.is_dir() {
        return None;
    }
    let canonical = fs::canonicalize(&git_dir).ok()?;
    let worktrees = canonical.parent()?;
    if worktrees.file_name()? != "worktrees" {
        return None;
    }
    let common = worktrees.parent()?;
    let registered = canonical.join(codex_git_metadata(&canonical.join("gitdir"))?.trim());
    if registered.file_name()? != ".git"
        || fs::canonicalize(registered.parent()?).ok()? != fs::canonicalize(root).ok()?
        || fs::canonicalize(
            canonical.join(codex_git_metadata(&canonical.join("commondir"))?.trim()),
        )
        .ok()?
            != common
    {
        return None;
    }
    let main = git_dir.parent()?.parent()?.parent()?;
    let main_dot_git = main.join(".git");
    let main_git_dir = if main_dot_git.is_dir() {
        main_dot_git
    } else {
        codex_gitdir_target(&main_dot_git)?
    };
    (fs::canonicalize(main_git_dir).ok()? == common).then(|| main.to_owned())
}

fn codex_git_metadata(path: &Path) -> Option<String> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > 64 * 1024 {
        return None;
    }
    let text = fs::read_to_string(path).ok()?;
    (text.len() <= 64 * 1024 && !text.trim().is_empty()).then_some(text)
}

fn codex_gitdir_target(path: &Path) -> Option<PathBuf> {
    let text = codex_git_metadata(path)?;
    let target = text.trim().strip_prefix("gitdir:")?.trim();
    if target.is_empty() {
        return None;
    }
    // Normalize relative metadata paths without resolving aliases used as trust keys.
    let mut resolved = PathBuf::new();
    for component in path.parent()?.join(target).components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                resolved.pop();
            }
            component => resolved.push(component),
        }
    }
    Some(resolved)
}

fn pi_shadowed_context_file(directory: &Path) -> Option<(PathBuf, PathBuf)> {
    let root = directory
        .ancestors()
        .find(|ancestor| ancestor.join(".git").exists())?;
    let dot_git = root.join(".git");
    let common = if dot_git.is_dir() {
        dot_git
    } else {
        let text = fs::read_to_string(dot_git).ok()?;
        let git_dir = root.join(text.trim().strip_prefix("gitdir: ")?.trim());
        if !git_dir.join("HEAD").exists() {
            return None;
        }
        let commondir = git_dir.join("commondir");
        if commondir.exists() {
            git_dir.join(fs::read_to_string(commondir).ok()?.trim())
        } else {
            git_dir
        }
    };
    let common = fs::canonicalize(common).ok()?;
    let root = fs::canonicalize(root).ok()?;
    let main = common.parent()?;
    if root == main
        || !root.starts_with(main)
        || fs::canonicalize(main.join(".git")).ok()? != common
    {
        return None;
    }
    let selected = PI_INSTRUCTION_CANDIDATES
        .iter()
        .map(|name| root.join(name))
        .find(|path| regular_file(path) && fs::read(path).is_ok())?;
    Some((main.join(selected.file_name()?), selected))
}

fn priority_same_file(left: &Path, right: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        match (fs::metadata(left), fs::metadata(right)) {
            (Ok(left), Ok(right)) => left.dev() == right.dev() && left.ino() == right.ino(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        match (fs::canonicalize(left), fs::canonicalize(right)) {
            (Ok(left), Ok(right)) => left == right,
            _ => false,
        }
    }
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
    let mut existing: Vec<PathBuf> = Vec::new();
    for path in candidates.iter().filter(|path| regular_file(path)) {
        if !existing.iter().any(|other| priority_same_file(path, other)) {
            existing.push(path.clone());
        }
    }
    let winner = existing
        .iter()
        .find(|path| {
            if skip_empty {
                fs::read(path).is_ok_and(|bytes| !String::from_utf8_lossy(&bytes).trim().is_empty())
            } else if remaining.is_none() {
                // Pi tries the next candidate when reading a file fails.
                fs::read(path).is_ok()
            } else {
                true
            }
        })
        .cloned();
    for path in &existing {
        let bytes = fs::read(path);
        let state = match bytes {
            Err(error) => SourceState::Unreadable(error.to_string()),
            Ok(bytes) if winner.as_ref() == Some(path) => {
                if let Some(remaining) = remaining.as_deref_mut() {
                    if *remaining == 0 {
                        SourceState::Excluded("project instruction byte budget exhausted".into())
                    } else {
                        let included = bytes.len().min(*remaining);
                        if String::from_utf8_lossy(&bytes[..included])
                            .trim()
                            .is_empty()
                        {
                            SourceState::SelectedEmpty
                        } else {
                            *remaining -= included;
                            if included < bytes.len() {
                                SourceState::Truncated {
                                    included,
                                    total: bytes.len(),
                                }
                            } else {
                                SourceState::Startup
                            }
                        }
                    }
                } else if bytes.is_empty() {
                    SourceState::SelectedEmpty
                } else {
                    SourceState::Startup
                }
            }
            Ok(bytes) if skip_empty && String::from_utf8_lossy(&bytes).trim().is_empty() => {
                SourceState::Empty
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
        // Claude loads a file of exactly 4 MiB and skips a larger one.
        let limit = CLAUDE_MAX_INSTRUCTION_BYTES as usize;
        fs::write(root.join("CLAUDE.md"), "r".repeat(limit + 1)).unwrap();
        fs::write(cwd.join("CLAUDE.md"), "a".repeat(limit)).unwrap();
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
        assert!(audit.sources.iter().any(|source| {
            source.path == root.join("CLAUDE.md")
                && matches!(source.state, SourceState::Excluded(_))
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
        audit.add_claude_scan(scan, &paths(temp.path()), None);
        assert!(audit.sources.iter().any(|source| {
            source.path == cwd.join("tests/CLAUDE.md") && source.group == SourceGroup::Nested
        }));
        assert!(audit.sources.iter().any(|source| {
            source.path == cwd.join(".claude/rules/testing.md")
                && source.group == SourceGroup::PathFiltered
        }));
    }

    #[test]
    fn claude_reads_agents_md_only_without_claude_md_on_the_launch_path() {
        let temp = TempDir::new().unwrap();
        let home = paths(temp.path());
        let root = temp.path().join("repo");
        let cwd = root.join("app");
        fs::create_dir_all(cwd.join("lib")).unwrap();
        fs::create_dir_all(cwd.join("docs")).unwrap();
        fs::create_dir_all(cwd.join("sub")).unwrap();
        fs::create_dir_all(temp.path().join(".claude")).unwrap();
        fs::write(temp.path().join(".claude/CLAUDE.md"), "user").unwrap();
        // A subfolder CLAUDE.md loads on demand, so its import cannot hide a startup AGENTS.md.
        fs::write(cwd.join("sub/CLAUDE.md"), "@../AGENTS.md").unwrap();
        fs::write(root.join("AGENTS.md"), "root agents").unwrap();
        fs::write(cwd.join("AGENTS.md"), "app agents").unwrap();
        fs::write(cwd.join("lib/AGENTS.md"), "lib agents").unwrap();
        fs::write(cwd.join("docs/AGENTS.md"), "docs agents").unwrap();
        fs::write(cwd.join("docs/CLAUDE.md"), "docs claude").unwrap();
        let state = |audit: &Audit, path: &Path| {
            audit
                .sources
                .iter()
                .find(|source| source.path == path)
                .map(|source| source.state.clone())
        };

        let mut audit = Audit::resolve(ContextRuntime::Claude, &cwd, &home, None);
        let seen = audit
            .sources
            .iter()
            .map(|source| source.path.clone())
            .collect();
        audit.add_claude_scan(scan_claude_descendants(&cwd, &home, seen), &home, None);
        assert_eq!(
            state(&audit, &root.join("AGENTS.md")),
            Some(SourceState::Startup)
        );
        assert_eq!(
            state(&audit, &cwd.join("AGENTS.md")),
            Some(SourceState::Startup)
        );
        assert!(matches!(
            state(&audit, &cwd.join("lib/AGENTS.md")),
            Some(SourceState::Nested(_))
        ));
        assert_eq!(state(&audit, &cwd.join("docs/AGENTS.md")), None);
        assert_eq!(
            agents_md_displaced_by_claude_local(&cwd, &home),
            [root.join("AGENTS.md"), cwd.join("AGENTS.md")]
        );

        // Any CLAUDE file on the launch path turns AGENTS.md off, including descendants.
        fs::write(root.join("CLAUDE.local.md"), "local").unwrap();
        let mut audit = Audit::resolve(ContextRuntime::Claude, &cwd, &home, None);
        let seen = audit
            .sources
            .iter()
            .map(|source| source.path.clone())
            .collect();
        audit.add_claude_scan(scan_claude_descendants(&cwd, &home, seen), &home, None);
        assert_eq!(
            state(&audit, &cwd.join("AGENTS.md")),
            Some(SourceState::Shadowed(root.join("CLAUDE.local.md")))
        );
        assert_eq!(state(&audit, &cwd.join("lib/AGENTS.md")), None);
        assert!(agents_md_displaced_by_claude_local(&cwd, &home).is_empty());

        // An AGENTS.md that a CLAUDE.md imports is read once, through the import.
        fs::write(cwd.join("CLAUDE.md"), "@AGENTS.md\n").unwrap();
        let audit = Audit::resolve(ContextRuntime::Claude, &cwd, &home, None);
        assert_eq!(state(&audit, &cwd.join("AGENTS.md")), None);
        assert!(audit.sources.iter().any(|source| {
            source.path == cwd.join("CLAUDE.md") && source.imports == [cwd.join("AGENTS.md")]
        }));
    }

    #[test]
    fn claude_project_instructions_setting_selects_agents_md() {
        let temp = TempDir::new().unwrap();
        let home = paths(temp.path());
        let cwd = temp.path().join("repo");
        fs::create_dir_all(cwd.join(".claude/rules")).unwrap();
        fs::create_dir_all(cwd.join("child")).unwrap();
        fs::create_dir_all(cwd.join(".agents")).unwrap();
        fs::create_dir_all(temp.path().join(".claude")).unwrap();
        fs::write(cwd.join("CLAUDE.md"), "@bridge.md").unwrap();
        fs::write(cwd.join("bridge.md"), "@child/AGENTS.md").unwrap();
        fs::write(cwd.join("AGENTS.md"), "agents").unwrap();
        fs::write(cwd.join("child/AGENTS.md"), "child agents").unwrap();
        fs::write(cwd.join(".agents/AGENTS.md"), "not Claude").unwrap();
        fs::write(cwd.join(".claude/rules/always.md"), "always").unwrap();
        fs::write(
            cwd.join(".claude/rules/tests.md"),
            "---\npaths:\n  - 'tests/**'\n---\ntests\n",
        )
        .unwrap();
        let loaded = |home: &Paths| {
            let mut audit = Audit::resolve(ContextRuntime::Claude, &cwd, home, None);
            let seen = audit
                .sources
                .iter()
                .map(|source| source.path.clone())
                .collect();
            audit.add_claude_scan(scan_claude_descendants(&cwd, home, seen), home, None);
            audit
                .sources
                .into_iter()
                .filter(|source| {
                    matches!(
                        source.state,
                        SourceState::Startup | SourceState::Conditional(_) | SourceState::Nested(_)
                    )
                })
                .map(|source| source.path)
                .collect::<Vec<_>>()
        };
        let set = |file: &Path, value: &str| fs::write(file, value).unwrap();
        let user_settings = temp.path().join(".claude/settings.json");
        let instruction_files = |value: &str| {
            format!(
                "{{\"pluginConfigs\":{{\"agents-md@builtin\":{{\"options\":{{\"instructionFiles\":\"{value}\"}}}}}}}}"
            )
        };

        set(
            &user_settings,
            &instruction_files("claude-md-and-agents-md"),
        );
        assert!(loaded(&home).contains(&cwd.join("CLAUDE.md")));
        assert!(loaded(&home).contains(&cwd.join("AGENTS.md")));
        // The two-hop import already loads child/AGENTS.md; Claude never reads .agents/.
        assert!(!loaded(&home).contains(&cwd.join("child/AGENTS.md")));
        assert!(!loaded(&home).contains(&cwd.join(".agents/AGENTS.md")));
        // An excluded importer loads nothing, so the imported file loads on its own.
        set(
            &user_settings,
            &format!(
                "{{\"claudeMdExcludes\":[{}],{}",
                serde_json::to_string(&cwd.join("CLAUDE.md")).unwrap(),
                &instruction_files("claude-md-and-agents-md")[1..]
            ),
        );
        assert!(loaded(&home).contains(&cwd.join("child/AGENTS.md")));

        // Claude ignores pluginConfigs in project settings.
        set(&user_settings, "{}");
        set(
            &cwd.join(".claude/settings.json"),
            &instruction_files("claude-md-and-agents-md"),
        );
        assert!(!loaded(&home).contains(&cwd.join("AGENTS.md")));

        set(&user_settings, &instruction_files("managed-only"));
        assert_eq!(loaded(&home), [cwd.join(".claude/rules/tests.md")]);

        // Disabling the built-in plugin reads CLAUDE.md files only.
        fs::remove_file(cwd.join("CLAUDE.md")).unwrap();
        set(&user_settings, "{}");
        assert!(loaded(&home).contains(&cwd.join("AGENTS.md")));
        set(
            &cwd.join(".claude/settings.local.json"),
            "{\"enabledPlugins\":{\"agents-md@builtin\":false}}",
        );
        assert!(!loaded(&home).contains(&cwd.join("AGENTS.md")));
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
        // An exclusion glob can name a linked rule by its resolved target.
        fs::write(
            cwd.join(".claude/settings.json"),
            format!(
                "{{\"claudeMdExcludes\":[{}]}}",
                serde_json::to_string(&fs::canonicalize(&shared).unwrap().join("*.md")).unwrap()
            ),
        )
        .unwrap();

        let audit = Audit::resolve(ContextRuntime::Claude, &cwd, &paths(temp.path()), None);
        let state = |path: &Path| {
            audit
                .sources
                .iter()
                .find(|source| source.path == path)
                .map(|source| source.state.clone())
        };
        assert_eq!(
            state(&cwd.join(".claude/rules/file.md")),
            Some(SourceState::Startup)
        );
        assert!(matches!(
            state(&cwd.join(".claude/rules/shared/directory.md")),
            Some(SourceState::Excluded(_))
        ));
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

        // Project settings apply only from the launch directory; local settings from the root.
        let sub = cwd.join("sub");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("CLAUDE.md"), "sub").unwrap();
        fs::write(
            cwd.join(".claude/settings.json"),
            format!(
                "{{\"claudeMdExcludes\":[{}]}}",
                serde_json::to_string(&sub.join("CLAUDE.md")).unwrap()
            ),
        )
        .unwrap();

        for directory in [&cwd, &sub] {
            let audit =
                Audit::resolve(ContextRuntime::Claude, directory, &paths(temp.path()), None);
            let source = audit
                .sources
                .iter()
                .find(|source| source.path == instruction)
                .unwrap();
            assert!(matches!(source.state, SourceState::Excluded(_)));
            assert!(
                source
                    .state
                    .reason(&paths(temp.path()), directory)
                    .unwrap()
                    .contains(".claude/settings.local.json")
            );
        }
        let audit = Audit::resolve(ContextRuntime::Claude, &sub, &paths(temp.path()), None);
        assert!(audit.sources.iter().any(|source| {
            source.path == sub.join("CLAUDE.md") && source.state == SourceState::Startup
        }));
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
    fn codex_selects_empty_project_overrides_and_applies_the_project_budget() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("repo");
        let cwd = root.join("app/deeper");
        let home = temp.path().join(".codex");
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(home.join("config.toml"), "project_doc_max_bytes = 5\n").unwrap();
        fs::write(root.join("AGENTS.md"), "123").unwrap();
        fs::write(root.join("app/AGENTS.md"), "456789").unwrap();
        fs::write(cwd.join("AGENTS.md"), "later").unwrap();
        fs::write(home.join("AGENTS.md"), "user instructions").unwrap();

        for empty in ["", " \n\t"] {
            fs::write(root.join("AGENTS.override.md"), empty).unwrap();
            fs::write(home.join("AGENTS.override.md"), empty).unwrap();
            let audit = Audit::resolve(ContextRuntime::Codex, &cwd, &paths(temp.path()), None);
            let state = |path: PathBuf| {
                &audit
                    .sources
                    .iter()
                    .find(|source| source.path == path)
                    .unwrap()
                    .state
            };
            assert_eq!(state(home.join("AGENTS.md")), &SourceState::Startup);
            assert_eq!(state(home.join("AGENTS.override.md")), &SourceState::Empty);
            assert_eq!(
                state(root.join("AGENTS.override.md")),
                &SourceState::SelectedEmpty
            );
            assert_eq!(
                state(root.join("AGENTS.md")),
                &SourceState::Shadowed(root.join("AGENTS.override.md"))
            );
            assert_eq!(
                state(root.join("app/AGENTS.md")),
                &SourceState::Truncated {
                    included: 5,
                    total: 6
                }
            );
            assert!(matches!(
                state(cwd.join("AGENTS.md")),
                SourceState::Excluded(_)
            ));
            assert!(audit.summary.contains("2 load at startup"));
            assert!(audit.summary.contains("5/5"));
        }

        // A truncated whitespace-only prefix does not consume the project budget.
        fs::write(root.join("AGENTS.override.md"), "     ignored suffix").unwrap();
        let audit = Audit::resolve(ContextRuntime::Codex, &cwd, &paths(temp.path()), None);
        assert!(
            audit
                .sources
                .iter()
                .any(|source| source.path == root.join("AGENTS.override.md")
                    && source.state == SourceState::SelectedEmpty)
        );
        assert!(
            audit
                .sources
                .iter()
                .any(|source| source.path == root.join("app/AGENTS.md")
                    && source.state
                        == SourceState::Truncated {
                            included: 5,
                            total: 6
                        })
        );
    }

    #[test]
    fn codex_honors_empty_markers_zero_budget_and_unique_fallback_filenames() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("repo");
        let cwd = root.join("app");
        let home = temp.path().join(".codex");
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join("AGENTS.md"), "abc").unwrap();
        fs::write(cwd.join("TEAM.md"), "def").unwrap();
        fs::write(home.join("AGENTS.md"), "global").unwrap();
        let fallbacks = r#"project_doc_fallback_filenames = ["AGENTS.md", "AGENTS.md", "", ".", "..", "../outside.md", "TEAM.md", "TEAM.md"]"#;
        for (config, root_present, expected_bytes) in [
            (
                format!("{fallbacks}\nproject_doc_max_bytes = 6"),
                true,
                "6/6",
            ),
            (
                format!("{fallbacks}\nproject_root_markers = []"),
                false,
                "3/32768",
            ),
            (
                format!("{fallbacks}\nproject_doc_max_bytes = 0"),
                true,
                "0/0",
            ),
        ] {
            fs::write(home.join("config.toml"), config).unwrap();
            let audit = Audit::resolve(ContextRuntime::Codex, &cwd, &paths(temp.path()), None);
            assert!(audit.warnings.is_empty());
            assert_eq!(
                audit
                    .sources
                    .iter()
                    .filter(|source| source.path == root.join("AGENTS.md"))
                    .count(),
                usize::from(root_present)
            );
            assert_eq!(
                audit
                    .sources
                    .iter()
                    .filter(|source| source.path == cwd.join("TEAM.md"))
                    .count(),
                1
            );
            assert!(audit.summary.contains(expected_bytes));
            for source in &audit.sources {
                if expected_bytes == "0/0" && source.scope != "user" {
                    assert!(matches!(source.state, SourceState::Excluded(_)));
                } else {
                    assert_eq!(source.state, SourceState::Startup);
                }
            }
        }
        // An invalid fallback must never escape the directory to load another file.
        fs::write(root.join("outside.md"), "outside").unwrap();
        fs::write(home.join("config.toml"), "project_root_markers = []\nproject_doc_fallback_filenames = ['../outside.md', '', '.', '..']").unwrap();
        let audit = Audit::resolve(ContextRuntime::Codex, &cwd, &paths(temp.path()), None);
        assert!(audit.sources.iter().all(|source| source.scope == "user"));
    }

    #[test]
    fn codex_trust_matches_launch_directory_before_main_worktree() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("repo");
        let cwd = root.join("app");
        let home = temp.path().join(".codex");
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(&home).unwrap();
        crate::git_worktree::git(&root, &["init", "-q"]).unwrap();
        crate::git_worktree::git(
            &root,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "--allow-empty",
                "-qm",
                "initial",
            ],
        )
        .unwrap();
        fs::write(root.join("AGENTS.md"), "repo").unwrap();
        fs::write(cwd.join("AGENTS.md"), "app").unwrap();
        fs::write(home.join("AGENTS.md"), "global").unwrap();
        let worktree = temp.path().join("linked");
        crate::git_worktree::git(
            &root,
            &["worktree", "add", "--detach", worktree.to_str().unwrap()],
        )
        .unwrap();
        fs::write(worktree.join("AGENTS.md"), "worktree").unwrap();
        let root_key = toml::Value::String(fs::canonicalize(&root).unwrap().display().to_string());
        let cwd_key = toml::Value::String(fs::canonicalize(&cwd).unwrap().display().to_string());
        for launch in [&cwd, &worktree] {
            fs::write(
                home.join("config.toml"),
                format!("[projects.{root_key}]\ntrust_level = 'untrusted'\n"),
            )
            .unwrap();
            let audit = Audit::resolve(ContextRuntime::Codex, launch, &paths(temp.path()), None);
            assert!(audit.sources.iter().any(|source| source.scope != "user"));
            for source in &audit.sources {
                if source.scope == "user" {
                    assert_eq!(source.state, SourceState::Startup);
                } else {
                    assert!(
                        matches!(&source.state, SourceState::Excluded(reason) if reason.contains("untrusted") && reason.contains("config.toml"))
                    );
                }
            }
            assert!(audit.summary.contains("0/32768"));
        }
        for launch_config in ["trust_level = 'trusted'", ""] {
            fs::write(home.join("config.toml"), format!("[projects.{root_key}]\ntrust_level = 'untrusted'\n[projects.{cwd_key}]\n{launch_config}")).unwrap();
            let audit = Audit::resolve(ContextRuntime::Codex, &cwd, &paths(temp.path()), None);
            assert!(
                audit
                    .sources
                    .iter()
                    .all(|source| source.state == SourceState::Startup)
            );
        }
        #[cfg(unix)]
        {
            let alias = temp.path().join("alias");
            std::os::unix::fs::symlink(&cwd, &alias).unwrap();
            let alias_key = toml::Value::String(alias.display().to_string());
            for canonical_entry in ["", "trust_level = 'trusted'"] {
                let config = if canonical_entry.is_empty() {
                    format!("[projects.{alias_key}]\ntrust_level = 'untrusted'")
                } else {
                    format!(
                        "[projects.{alias_key}]\ntrust_level = 'untrusted'\n[projects.{cwd_key}]\n{canonical_entry}"
                    )
                };
                fs::write(home.join("config.toml"), config).unwrap();
                let audit =
                    Audit::resolve(ContextRuntime::Codex, &alias, &paths(temp.path()), None);
                let source = audit
                    .sources
                    .iter()
                    .find(|source| source.path == alias.join("AGENTS.md"))
                    .unwrap();
                assert_eq!(
                    matches!(source.state, SourceState::Excluded(_)),
                    canonical_entry.is_empty()
                );
            }
        }
        // Arbitrary parent entries do not inherit outside a Git repository.
        let outside = temp.path().join("outside/nested");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("AGENTS.md"), "outside").unwrap();
        let parent_key = toml::Value::String(outside.parent().unwrap().display().to_string());
        fs::write(
            home.join("config.toml"),
            format!("[projects.{parent_key}]\ntrust_level = 'untrusted'"),
        )
        .unwrap();
        let audit = Audit::resolve(ContextRuntime::Codex, &outside, &paths(temp.path()), None);
        assert!(
            audit
                .sources
                .iter()
                .all(|source| source.state == SourceState::Startup)
        );
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
            "@./local.md @../shared.md `@inline.md`\n```md\n@fenced.md\n```\n~~~\n@also-fenced.md\n~~~\n<!-- @hidden.md -->\n<!--\n@multiline-hidden.md\n-->\n",
            parent,
            home,
        );

        assert_eq!(
            imports,
            [parent.join("../shared.md"), parent.join("./local.md")]
        );
    }

    #[test]
    fn pi_deduplicates_global_paths_and_physical_candidate_aliases() {
        let temp = TempDir::new().unwrap();
        let global = temp.path().join(".pi/agent");
        fs::create_dir_all(&global).unwrap();
        for basename in ["AGENTS", "CLAUDE"] {
            let cwd = global.join(basename);
            fs::create_dir_all(&cwd).unwrap();
            let normal = cwd.join(format!("{basename}.md"));
            let uppercase = cwd.join(format!("{basename}.MD"));
            fs::write(&normal, "instructions").unwrap();
            // Hardlinks exercise identical file identity even on case-sensitive CI hosts.
            if !uppercase.exists() {
                fs::hard_link(&normal, &uppercase).unwrap();
            }
            let audit = Audit::resolve(ContextRuntime::Pi, &cwd, &paths(temp.path()), None);
            assert_eq!(
                audit
                    .sources
                    .iter()
                    .filter(|source| source.path.parent() == Some(cwd.as_path()))
                    .count(),
                1
            );
        }
        fs::write(global.join("AGENTS.md"), "global").unwrap();
        let audit = Audit::resolve(ContextRuntime::Pi, &global, &paths(temp.path()), None);
        let global_sources = audit
            .sources
            .iter()
            .filter(|source| source.path == global.join("AGENTS.md"))
            .collect::<Vec<_>>();
        assert_eq!(global_sources.len(), 1);
        assert_eq!(global_sources[0].scope, "user");
        assert_eq!(global_sources[0].state, SourceState::Startup);

        let cwd = temp.path().join("distinct");
        fs::create_dir_all(&cwd).unwrap();
        fs::write(cwd.join("AGENTS.md"), "first").unwrap();
        let uppercase = cwd.join("AGENTS.MD");
        if !uppercase.exists() {
            fs::write(&uppercase, "second").unwrap();
            let audit = Audit::resolve(ContextRuntime::Pi, &cwd, &paths(temp.path()), None);
            assert!(audit.sources.iter().any(|source| source.path == uppercase
                && source.state == SourceState::Shadowed(cwd.join("AGENTS.md"))));
        }
    }

    #[test]
    fn pi_shadows_only_matching_main_checkout_files_for_nested_worktrees() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("repo");
        fs::create_dir_all(&root).unwrap();
        crate::git_worktree::git(&root, &["init", "-q"]).unwrap();
        fs::write(root.join("AGENTS.md"), "main").unwrap();
        crate::git_worktree::git(&root, &["add", "-f", "AGENTS.md"]).unwrap();
        crate::git_worktree::git(
            &root,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-qm",
                "initial",
            ],
        )
        .unwrap();
        let nested = root.join(".worktrees/feat");
        let sibling = temp.path().join("sibling");
        for worktree in [&nested, &sibling] {
            crate::git_worktree::git(
                &root,
                &["worktree", "add", "--detach", worktree.to_str().unwrap()],
            )
            .unwrap();
            let cwd = worktree.join("subdir");
            fs::create_dir_all(&cwd).unwrap();
            let audit = Audit::resolve(ContextRuntime::Pi, &cwd, &paths(temp.path()), None);
            assert!(
                audit
                    .sources
                    .iter()
                    .any(|source| source.path == worktree.join("AGENTS.md")
                        && source.state == SourceState::Startup)
            );
            let main = audit
                .sources
                .iter()
                .find(|source| source.path == root.join("AGENTS.md"));
            if worktree == &nested {
                let canonical_selected = fs::canonicalize(worktree.join("AGENTS.md")).unwrap();
                assert_eq!(
                    main.unwrap().state,
                    SourceState::Shadowed(canonical_selected)
                );
            } else {
                assert!(main.is_none());
            }
        }
        fs::write(nested.join("AGENTS.override.md"), "").unwrap();
        let audit = Audit::resolve(ContextRuntime::Pi, &nested, &paths(temp.path()), None);
        assert!(
            audit
                .sources
                .iter()
                .any(|source| source.path == root.join("AGENTS.md")
                    && source.state == SourceState::Startup)
        );
        fs::write(root.join("AGENTS.override.md"), "main override").unwrap();
        let audit = Audit::resolve(ContextRuntime::Pi, &nested, &paths(temp.path()), None);
        assert!(
            audit
                .sources
                .iter()
                .any(|source| source.path == root.join("AGENTS.override.md")
                    && source.state
                        == SourceState::Shadowed(
                            fs::canonicalize(nested.join("AGENTS.override.md")).unwrap()
                        ))
        );

        // A bare repository's containing directory is not a main checkout.
        let bare_parent = temp.path().join("bare-layout");
        fs::create_dir_all(&bare_parent).unwrap();
        fs::write(bare_parent.join("AGENTS.md"), "ancestor").unwrap();
        let bare = bare_parent.join(".bare");
        crate::git_worktree::git(
            temp.path(),
            &[
                "clone",
                "--bare",
                root.to_str().unwrap(),
                bare.to_str().unwrap(),
            ],
        )
        .unwrap();
        let checkout = bare_parent.join("main");
        crate::git_worktree::git(
            &bare,
            &["worktree", "add", "--detach", checkout.to_str().unwrap()],
        )
        .unwrap();
        let audit = Audit::resolve(ContextRuntime::Pi, &checkout, &paths(temp.path()), None);
        for file in [bare_parent.join("AGENTS.md"), checkout.join("AGENTS.md")] {
            assert!(
                audit
                    .sources
                    .iter()
                    .any(|source| source.path == file && source.state == SourceState::Startup)
            );
        }

        let host = temp.path().join("host");
        fs::create_dir_all(&host).unwrap();
        crate::git_worktree::git(&host, &["init", "-q"]).unwrap();
        crate::git_worktree::git(
            &host,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                root.to_str().unwrap(),
                "sub",
            ],
        )
        .unwrap();
        fs::write(host.join("AGENTS.md"), "host").unwrap();
        let submodule = host.join("sub");
        let audit = Audit::resolve(ContextRuntime::Pi, &submodule, &paths(temp.path()), None);
        for file in [host.join("AGENTS.md"), submodule.join("AGENTS.md")] {
            assert!(
                audit
                    .sources
                    .iter()
                    .any(|source| source.path == file && source.state == SourceState::Startup)
            );
        }
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
