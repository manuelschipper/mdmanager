use std::cell::Cell;
use std::collections::hash_map::DefaultHasher;
use std::env;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

use ratatui::Frame;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use walkdir::{DirEntry, WalkDir};

#[path = "inspection.rs"]
mod inspection;
use inspection::{InstructionInspection, InstructionStatus};

use super::app::{ContextScanStatus, ContextUi};
use crate::config::{self, GlobalConfig, Paths, target_display_name};
use crate::context::{
    Audit, ContextRuntime, ContextSource, SourceGroup, SourceState, claude_config_dir,
    display_path, pi_agent_dir,
};
use crate::deploy::{self, GlobalTargetStatus};
use crate::local::{self, LocalRepository};
use crate::project::{self, Workspace};
use crate::theme::{self, Theme};

thread_local! {
    static ACTIVE_THEME: Cell<Theme> = Cell::new(theme::default_theme());
}

fn activate_theme(global: &GlobalConfig) -> Result<(), String> {
    let selected = theme::resolve(global.theme())?;
    ACTIVE_THEME.set(selected);
    Ok(())
}

fn reset_theme() {
    ACTIVE_THEME.set(theme::default_theme());
}

fn active_theme() -> Theme {
    ACTIVE_THEME.get()
}
const COMPACT_WIDTH: u16 = 120;
const HOME_MIN_WIDTH: u16 = 80;
const READABLE_WIDTH: u16 = 100;
const AUTO_RELOAD_INTERVAL: Duration = Duration::from_secs(1);
const RELOAD_MESSAGE_DURATION: Duration = Duration::from_secs(3);
const PAGE_LINES: u16 = 8;
const LINE_HIGHLIGHT_DURATION: Duration = Duration::from_millis(1200);
const INTRO_DURATION: Duration = Duration::from_millis(1800);
const INTRO_WORDMARK: [&str; 5] = [
    "███╗   ███╗ ██████╗ ",
    "████╗ ████║ ██╔══██╗",
    "██╔████╔██║ ██║  ██║",
    "██║╚██╔╝██║ ██████╔╝",
    "╚═╝     ╚═╝ ╚═════╝ ",
];

#[derive(Clone, PartialEq)]
enum ManagedRef {
    Project(String),
    Local(local::ManagedTarget),
    // The browsed Profile is part of the reference so reloads and history cannot
    // silently switch it to another Profile.
    Global { profile: String, target: String },
}

#[derive(Clone, Copy, PartialEq)]
enum DiagnosticKind {
    General,
    Global,
}

#[derive(Clone)]
enum View {
    Home,
    Info {
        kind: DiagnosticKind,
        title: String,
        text: String,
    },
    File {
        title: String,
        path: PathBuf,
        about: String,
    },
    Managed(ManagedRef),
    ManagedDocument(ManagedRef),
    SectionDocument {
        title: String,
        path: PathBuf,
        about: String,
    },
    Diff(ManagedRef),
    GlobalLibrary,
    GlobalProfile(String),
    Context,
    Source,
}

#[derive(Clone)]
enum HomeItem {
    Context,
    ProjectInvalid,
    ProjectMissing(String),
    ProjectTarget(String),
    ProjectExternal { id: String, path: PathBuf },
    LocalDisable(local::DisableRuntime, local::DisableStatus),
    LocalInvalid(String),
    LocalMissing(local::ManagedTarget),
    LocalExternal(local::ManagedTarget, PathBuf),
    LocalTarget(local::ManagedTarget),
    GlobalInvalid,
    GlobalSetup,
    GlobalExternal(usize),
    GlobalTarget(String),
    Library,
}

#[derive(Clone, PartialEq)]
enum LibraryEntry {
    Profile(String),
    PersonalSection(String),
    ProjectSection(String),
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ContextEntry {
    Source(usize),
    Nested,
}

struct RuntimePicker {
    query: String,
    selected: usize,
}

struct ThemePicker {
    query: String,
    selected: usize,
    original: Theme,
}

struct SearchPrompt {
    query: String,
}

struct LinePrompt {
    input: String,
}

#[derive(Clone)]
struct ManagedSection {
    id: String,
    name: String,
    path: PathBuf,
    content: String,
}

#[derive(Clone)]
struct ManagedWorkspace {
    title: String,
    status: String,
    target: PathBuf,
    rendered: String,
    difference: Option<String>,
    instruction_status: InstructionStatus,
    sections: Vec<ManagedSection>,
}

#[derive(Clone)]
struct SymlinkInfo {
    target: PathBuf,
    resolution: SymlinkResolution,
}

#[derive(Clone)]
enum SymlinkResolution {
    Resolved(PathBuf),
    Missing,
    Unresolved,
}

struct ManagedDestination {
    scope: &'static str,
    target: String,
    status: Option<InstructionStatus>,
}

struct App {
    inspection: InstructionInspection,
    paths: Paths,
    global: Option<GlobalConfig>,
    global_error: Option<String>,
    global_active_profile: Option<String>,
    project: Option<Workspace>,
    project_error: Option<String>,
    project_root: PathBuf,
    watch_root: Option<PathBuf>,
    local_repository: Option<LocalRepository>,
    global_sources: Vec<(ContextRuntime, ContextSource)>,
    context: ContextUi,
    context_entry_index: usize,
    context_nested_expanded: bool,
    view: View,
    // Each entry keeps the parent view together with its list cursor so going
    // back restores the selection, not just the screen.
    history: Vec<(View, usize)>,
    home_index: usize,
    section_index: usize,
    scroll: u16,
    max_scroll: u16,
    document_width: u16,
    document_height: u16,
    context_preview: bool,
    help: bool,
    picker: Option<RuntimePicker>,
    theme_picker: Option<ThemePicker>,
    search: Option<SearchPrompt>,
    line_prompt: Option<LinePrompt>,
    highlighted_line: Option<(usize, Instant)>,
    message: Option<(bool, String)>,
    message_expires_at: Option<Instant>,
    watch_signature: u64,
    last_watch: Instant,
    // Set once by `run` at process start; cleared by expiry or the first key.
    // Nothing else ever sets it, so the intro cannot replay.
    intro_started: Option<Instant>,
}

impl App {
    #[cfg(test)]
    fn new_at(
        paths: Paths,
        global: Option<GlobalConfig>,
        directory: PathBuf,
    ) -> Result<Self, String> {
        Self::new_at_with_global_error(paths, global, None, directory)
    }

    fn new_at_with_global_error(
        paths: Paths,
        global: Option<GlobalConfig>,
        global_error: Option<String>,
        directory: PathBuf,
    ) -> Result<Self, String> {
        let theme_error = match global.as_ref() {
            Some(global) => activate_theme(global).err(),
            None if global_error.is_none() => {
                reset_theme();
                None
            }
            None => None,
        };
        let (project, project_error) = match Workspace::discover(&directory) {
            Ok(project) => (project, None),
            Err(error) => (None, Some(error)),
        };
        let watch_root = crate::git_worktree::worktree_root(&directory).ok();
        let project_root = project.as_ref().map_or_else(
            || watch_root.clone().unwrap_or_else(|| directory.clone()),
            |project| project.root.clone(),
        );
        let local_repository = LocalRepository::discover(&directory, &paths).ok();
        let (global_active_profile, global_error) = match active_global_profile(global.as_ref()) {
            Ok(profile) => (profile, global_error),
            Err(error) => (None, Some(error)),
        };
        let global_sources = discovered_global_sources(
            &paths,
            &directory,
            global.as_ref(),
            global_active_profile.as_deref(),
        );
        let mut context = ContextUi::new_at(paths.clone(), global.as_ref(), directory)?;
        context.start_scan();
        let view = global_error
            .as_ref()
            .map_or(View::Home, |error| View::Info {
                kind: DiagnosticKind::Global,
                title: if global.is_some() {
                    "Deployment state needs attention".into()
                } else {
                    "Global Instructions unavailable".into()
                },
                text: global_diagnostic_text(error),
            });
        let mut app = Self {
            inspection: InstructionInspection::new(context.audit.clone()),
            paths,
            global,
            global_error,
            global_active_profile,
            project,
            project_error,
            project_root,
            watch_root,
            local_repository,
            global_sources,
            context,
            context_entry_index: 0,
            context_nested_expanded: false,
            view,
            history: Vec::new(),
            home_index: 0,
            section_index: 0,
            scroll: 0,
            max_scroll: 0,
            document_width: 0,
            document_height: 0,
            context_preview: false,
            help: false,
            picker: None,
            theme_picker: None,
            search: None,
            line_prompt: None,
            highlighted_line: None,
            message: theme_error.map(|error| (false, format!("{error}; keeping current theme"))),
            message_expires_at: None,
            watch_signature: 0,
            last_watch: Instant::now(),
            intro_started: None,
        };
        app.refresh_inspection();
        app.watch_signature = app.current_watch_signature();
        Ok(app)
    }

    // Event handling owns refresh; render, search and sizing only read this snapshot.
    fn refresh_inspection(&mut self) {
        self.inspection = InstructionInspection::observe(self);
    }

    fn home_items(&self) -> Vec<HomeItem> {
        self.inspection.home_items.clone()
    }

    fn library_entries(&self) -> Vec<LibraryEntry> {
        let mut entries = self.global.as_ref().map_or_else(Vec::new, |global| {
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
        if let Some(project) = &self.project {
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

    // True when the view no longer refers to a Global Profile or Target that exists
    // after a reload; such views fall back through history instead of resolving
    // against a different Profile.
    fn global_view_is_stale(&self, view: &View) -> bool {
        let missing = |profile: &str, target: Option<&str>| {
            self.global.as_ref().is_none_or(|global| {
                !global.manifest.profiles.contains_key(profile)
                    || target.is_some_and(|target| !profile_deploys(global, profile, target))
            })
        };
        match view {
            View::GlobalLibrary => self.global.is_none() && self.project.is_none(),
            View::GlobalProfile(profile) => missing(profile, None),
            View::Managed(ManagedRef::Global { profile, target })
            | View::ManagedDocument(ManagedRef::Global { profile, target })
            | View::Diff(ManagedRef::Global { profile, target }) => missing(profile, Some(target)),
            _ => false,
        }
    }

    fn open(&mut self, view: View) {
        if matches!(view, View::Context) {
            self.context.start_scan();
        }
        self.history.push((self.view.clone(), self.section_index));
        self.view = view;
        if matches!(self.view, View::File { .. } | View::SectionDocument { .. }) {
            self.reload_observations(true);
        }
        self.scroll = 0;
        self.max_scroll = 0;
        self.section_index = 0;
        self.highlighted_line = None;
        self.clear_message();
    }

    fn back(&mut self) -> SessionAction {
        if matches!(self.view, View::Home) {
            return SessionAction::Quit;
        }
        let (view, section_index) = self.history.pop().unwrap_or((View::Home, 0));
        self.view = view;
        self.section_index = section_index;
        self.scroll = 0;
        self.max_scroll = 0;
        SessionAction::Continue
    }

    fn reload_from_disk(&mut self) {
        self.reload_observations(false);
    }

    // Accepted scans and document opens retain the scan only while watched inputs
    // are unchanged. A disk change invalidates both the audit and its receiver.
    fn reload_observations(&mut self, preserve_context_scan: bool) {
        let notify_reload = !preserve_context_scan;
        let preserve_context_scan =
            preserve_context_scan && self.current_watch_signature() == self.watch_signature;
        let previous_global_error = self.global_error.clone();
        let selected_home = self.home_items().get(self.home_index).map(home_item_key);
        let selected_context = self
            .context_entries()
            .get(self.context_entry_index)
            .and_then(|entry| match entry {
                ContextEntry::Source(index) => self
                    .inspection
                    .context
                    .sources
                    .get(*index)
                    .map(|source| source.path.clone()),
                ContextEntry::Nested => None,
            });
        let was_nested = matches!(
            self.context_entries().get(self.context_entry_index),
            Some(ContextEntry::Nested)
        );
        let selected_managed_section = match &self.view {
            View::Managed(reference) => {
                managed_workspace(self, reference)
                    .ok()
                    .and_then(|workspace| {
                        self.section_index
                            .checked_sub(1)
                            .and_then(|index| workspace.sections.get(index))
                            .map(|section| section.id.clone())
                    })
            }
            _ => None,
        };
        let selected_library = match &self.view {
            View::GlobalLibrary => self.library_entries().get(self.section_index).cloned(),
            _ => None,
        };
        let selected_profile_target = match &self.view {
            View::GlobalProfile(_) => self.global.as_ref().and_then(|global| {
                global
                    .target_names()
                    .nth(self.section_index)
                    .map(str::to_owned)
            }),
            _ => None,
        };
        match Workspace::discover(&self.inspection.context.directory) {
            Ok(project) => {
                self.project = project;
                self.project_error = None;
            }
            Err(error) => {
                self.project = None;
                self.project_error = Some(error);
            }
        }
        self.watch_root =
            crate::git_worktree::worktree_root(&self.inspection.context.directory).ok();
        self.project_root = self.project.as_ref().map_or_else(
            || {
                self.watch_root
                    .clone()
                    .unwrap_or_else(|| self.inspection.context.directory.clone())
            },
            |project| project.root.clone(),
        );
        let mut theme_error = None;
        if self.paths.config.exists() {
            match GlobalConfig::load(&self.paths) {
                Ok(global) => {
                    theme_error = activate_theme(&global).err();
                    self.global = Some(global);
                    self.global_error = None;
                }
                Err(error) => {
                    self.global = None;
                    self.global_error = Some(error);
                }
            }
        } else {
            reset_theme();
            self.global = None;
            self.global_error = None;
        }
        match active_global_profile(self.global.as_ref()) {
            Ok(profile) => self.global_active_profile = profile,
            Err(error) => {
                self.global_active_profile = None;
                self.global_error = Some(error);
            }
        }
        self.local_repository =
            LocalRepository::discover(&self.inspection.context.directory, &self.paths).ok();
        self.global_sources = discovered_global_sources(
            &self.paths,
            &self.inspection.context.directory,
            self.global.as_ref(),
            self.global_active_profile.as_deref(),
        );
        if !preserve_context_scan {
            if matches!(self.view, View::Context | View::Source) {
                self.context.reload_and_scan(self.global.as_ref());
            } else {
                self.context.reload(self.global.as_ref());
            }
        }
        self.refresh_inspection();
        self.context_entry_index = selected_context
            .and_then(|path| {
                self.context_entries().iter().position(|entry| {
                    matches!(
                        entry,
                        ContextEntry::Source(index)
                            if self.inspection.context.sources[*index].path == path
                    )
                })
            })
            .or_else(|| {
                was_nested
                    .then(|| {
                        self.context_entries()
                            .iter()
                            .position(|entry| *entry == ContextEntry::Nested)
                    })
                    .flatten()
            })
            .unwrap_or(0);
        if let Some(key) = selected_home {
            self.home_index = self
                .home_items()
                .iter()
                .position(|item| home_item_key(item) == key)
                .unwrap_or_else(|| {
                    self.home_index
                        .min(self.home_items().len().saturating_sub(1))
                });
        }
        while self.global_view_is_stale(&self.view.clone()) {
            let (view, section_index) = self.history.pop().unwrap_or((View::Home, 0));
            self.view = view;
            self.section_index = section_index;
            self.scroll = 0;
            self.max_scroll = 0;
        }
        if let View::Diff(reference) = self.view.clone()
            && managed_workspace(self, &reference)
                .is_ok_and(|workspace| workspace.difference.is_none())
        {
            let (view, section_index) = self.history.pop().unwrap_or((View::Home, 0));
            self.view = view;
            self.section_index = section_index;
            self.scroll = 0;
            self.max_scroll = 0;
        }
        // Re-find the selected Profile, Section, or target by identity so inserted
        // or reordered manifest entries do not move the selection to another item.
        match &self.view {
            View::Managed(reference) => {
                if let Some(id) = selected_managed_section
                    && let Ok(workspace) = managed_workspace(self, reference)
                {
                    self.section_index = workspace
                        .sections
                        .iter()
                        .position(|section| section.id == id)
                        .map_or(0, |index| index + 1);
                }
            }
            View::GlobalLibrary => {
                if let Some(entry) = &selected_library
                    && let Some(position) = self
                        .library_entries()
                        .iter()
                        .position(|candidate| candidate == entry)
                {
                    self.section_index = position;
                }
            }
            View::GlobalProfile(_) => {
                if let Some(target) = &selected_profile_target
                    && let Some(position) = self.global.as_ref().and_then(|global| {
                        global.target_names().position(|id| id == target.as_str())
                    })
                {
                    self.section_index = position;
                }
            }
            _ => {}
        }
        if self.global_error != previous_global_error {
            if let Some(error) = self.global_error.clone() {
                self.open(View::Info {
                    kind: DiagnosticKind::Global,
                    title: if self.global.is_some() {
                        "Deployment state needs attention".into()
                    } else {
                        "Global Instructions unavailable".into()
                    },
                    text: global_diagnostic_text(&error),
                });
            } else if matches!(
                &self.view,
                View::Info {
                    kind: DiagnosticKind::Global,
                    ..
                }
            ) {
                let (view, section_index) = self.history.pop().unwrap_or((View::Home, 0));
                self.view = view;
                self.section_index = section_index;
                self.scroll = 0;
                self.max_scroll = 0;
            }
        }
        if let Some(error) = theme_error {
            self.message = Some((false, format!("{error}; keeping current theme")));
            self.message_expires_at = None;
        } else if notify_reload {
            self.message = Some((true, "Updated from disk".into()));
            self.message_expires_at = Some(Instant::now() + RELOAD_MESSAGE_DURATION);
        }
        self.watch_signature = self.current_watch_signature();
        self.last_watch = Instant::now();
    }

    fn clear_message(&mut self) {
        self.message = None;
        self.message_expires_at = None;
    }

    fn set_message(&mut self, ok: bool, message: impl Into<String>) {
        self.message = Some((ok, message.into()));
        self.message_expires_at = None;
    }

    fn poll_message(&mut self) {
        if self
            .message_expires_at
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.clear_message();
        }
    }

    fn poll_intro(&mut self) {
        if self
            .intro_started
            .is_some_and(|started| started.elapsed() >= INTRO_DURATION)
        {
            self.intro_started = None;
        }
    }

    fn poll_reload(&mut self) {
        if self.last_watch.elapsed() < AUTO_RELOAD_INTERVAL {
            return;
        }
        self.last_watch = Instant::now();
        let signature = self.current_watch_signature();
        if signature != self.watch_signature {
            self.reload_from_disk();
        }
    }

    fn current_watch_signature(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        for path in [
            self.paths.config.clone(),
            self.paths.data_dir.clone(),
            self.paths.state_dir.clone(),
        ] {
            hash_tree(&path, &mut hasher, false);
        }
        let codex_home = if env::var_os("HOME").as_deref() == Some(self.paths.home.as_os_str()) {
            env::var_os("CODEX_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| self.paths.home.join(".codex"))
        } else {
            self.paths.home.join(".codex")
        };
        let claude_config = claude_config_dir(&self.paths);
        let pi_agent = pi_agent_dir(&self.paths);
        for path in [
            claude_config.join("CLAUDE.md"),
            claude_config.join("settings.json"),
            claude_config.join("settings.local.json"),
            codex_home.join("AGENTS.override.md"),
            codex_home.join("AGENTS.md"),
            codex_home.join("config.toml"),
            pi_agent.join("AGENTS.override.md"),
            pi_agent.join("AGENTS.md"),
            pi_agent.join("AGENTS.MD"),
            pi_agent.join("CLAUDE.md"),
            pi_agent.join("CLAUDE.MD"),
        ] {
            hash_tree(&path, &mut hasher, false);
        }
        hash_tree(&claude_config.join("rules"), &mut hasher, true);
        if let Some(root) = &self.watch_root {
            hash_tree(root, &mut hasher, true);
        }
        hash_context_sources(&self.inspection.context, &mut hasher);
        hash_ancestor_candidates(
            &self.inspection.context.directory,
            self.watch_root.as_deref(),
            &mut hasher,
        );
        hasher.finish()
    }

    fn context_entries(&self) -> Vec<ContextEntry> {
        let mut entries = self
            .inspection
            .context
            .sources
            .iter()
            .enumerate()
            .filter(|(_, source)| source.group != SourceGroup::Nested)
            .map(|(index, _)| ContextEntry::Source(index))
            .collect::<Vec<_>>();
        if self.inspection.context.runtime == ContextRuntime::Claude
            && self
                .inspection
                .context
                .sources
                .iter()
                .any(|source| source.group == SourceGroup::Nested)
        {
            let nested_at = entries.len();
            entries.insert(nested_at, ContextEntry::Nested);
            if self.context_nested_expanded {
                let nested = self
                    .inspection
                    .context
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

    fn move_context_selection(&mut self, down: bool) {
        let len = self.context_entries().len();
        self.context_entry_index = if len == 0 {
            0
        } else if down {
            (self.context_entry_index + 1).min(len - 1)
        } else {
            self.context_entry_index.saturating_sub(1)
        };
        self.scroll = 0;
    }

    fn open_context_entry(&mut self) {
        match self
            .context_entries()
            .get(self.context_entry_index)
            .copied()
        {
            Some(ContextEntry::Source(index)) => {
                self.context.source_index = index;
                self.open(View::Source);
            }
            Some(ContextEntry::Nested) => self.context_nested_expanded ^= true,
            None => {}
        }
    }

    fn poll_context_scan(&mut self) {
        if self.context.poll_scan(self.global.as_ref()) {
            self.reload_observations(true);
        }
    }

    fn open_picker(&mut self) {
        self.picker = Some(RuntimePicker {
            query: String::new(),
            selected: self.context.runtime_index,
        });
        self.clear_message();
    }

    fn open_theme_picker(&mut self) {
        let selected_name = self
            .global
            .as_ref()
            .map(|global| global.theme())
            .filter(|name| theme::resolve(name).is_ok())
            .unwrap_or(theme::DEFAULT_NAME);
        let selected = theme::names()
            .position(|name| name == selected_name)
            .unwrap_or(0);
        self.theme_picker = Some(ThemePicker {
            query: String::new(),
            selected,
            original: active_theme(),
        });
        self.clear_message();
    }

    fn select_runtime(&mut self, index: usize) {
        self.reload_observations(true);
        self.context.select_runtime(index, self.global.as_ref());
        self.refresh_inspection();
        self.context_entry_index = 0;
        self.context_nested_expanded = false;
        self.scroll = 0;
        if matches!(self.view, View::Source) {
            self.history.pop();
            self.view = View::Context;
        }
    }

    fn selected_source(&self) -> Option<&ContextSource> {
        self.inspection
            .context
            .sources
            .get(self.context.source_index)
    }

    fn searchable_text(&self) -> Option<String> {
        match &self.view {
            View::File { path, .. } | View::SectionDocument { path, .. } => {
                self.inspection.document(path).ok()
            }
            View::Source => self.selected_source().map(|source| source.content.clone()),
            View::ManagedDocument(reference) => managed_workspace(self, reference)
                .ok()
                .map(|view| view.rendered),
            View::Diff(reference) => managed_workspace(self, reference)
                .ok()
                .and_then(|view| view.difference),
            _ => None,
        }
    }

    fn numbered_text(&self) -> Option<String> {
        match &self.view {
            View::File { path, .. } | View::SectionDocument { path, .. } => {
                self.inspection.document(path).ok()
            }
            View::Source => self.selected_source().map(|source| source.content.clone()),
            View::ManagedDocument(reference) => managed_workspace(self, reference)
                .ok()
                .map(|view| view.rendered),
            _ => None,
        }
    }

    fn go_to_line(&mut self, input: String) {
        let Some(text) = self.numbered_text() else {
            return;
        };
        let lines = text.lines().collect::<Vec<_>>();
        if lines.is_empty() {
            self.set_message(false, "Document has no lines");
            return;
        }
        let Ok(line) = input.parse::<usize>() else {
            self.set_message(false, format!("Line must be between 1 and {}", lines.len()));
            return;
        };
        if line == 0 || line > lines.len() {
            self.set_message(false, format!("Line must be between 1 and {}", lines.len()));
            return;
        }
        let offset = wrapped_line_offset(&lines, line - 1, self.document_width.max(1));
        self.scroll = u16::try_from(offset)
            .unwrap_or(u16::MAX)
            .saturating_sub(self.document_height / 2)
            .min(self.max_scroll);
        self.highlighted_line = Some((line, Instant::now()));
        self.set_message(true, format!("Line {line}"));
    }

    fn apply_search(&mut self, query: String) {
        if query.is_empty() {
            return;
        }
        let Some(text) = self.searchable_text() else {
            return;
        };
        let needle = query.to_lowercase();
        let lines = text.lines().collect::<Vec<_>>();
        if lines.is_empty() {
            return;
        }
        let matches = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line.to_lowercase().contains(&needle))
            .map(|(index, _)| wrapped_line_offset(&lines, index, self.document_width.max(1)))
            .collect::<Vec<_>>();
        let found = matches
            .iter()
            .copied()
            .find(|offset| *offset > usize::from(self.scroll))
            .or_else(|| matches.first().copied());
        if let Some(offset) = found {
            self.scroll = u16::try_from(offset)
                .unwrap_or(u16::MAX)
                .min(self.max_scroll);
            self.set_message(true, format!("Found “{query}”"));
        } else {
            self.set_message(false, format!("No match for “{query}”"));
        }
    }
}

fn wrapped_line_offset(lines: &[&str], index: usize, width: u16) -> usize {
    if index == 0 {
        return 0;
    }
    let mut prefix = lines[..index].join("\n");
    prefix.push('\n');
    prefix.push('x');
    Paragraph::new(prefix)
        .wrap(Wrap { trim: false })
        .line_count(width)
        .saturating_sub(1)
}

fn home_item_key(item: &HomeItem) -> String {
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

fn hash_tree(path: &Path, hasher: &mut DefaultHasher, project_tree: bool) {
    if !path.exists() {
        path.hash(hasher);
        0_u8.hash(hasher);
        return;
    }
    let walker = WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| !ignored_watch_entry(entry));
    for entry in walker.flatten() {
        if entry.file_type().is_file() && project_tree && !relevant_project_file(entry.path()) {
            continue;
        }
        entry.path().hash(hasher);
        if entry.file_type().is_symlink() {
            fs::read_link(entry.path()).ok().hash(hasher);
        }
        if let Ok(metadata) = fs::metadata(entry.path()) {
            metadata.len().hash(hasher);
            metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_nanos())
                .hash(hasher);
        }
    }
}

fn hash_context_sources(audit: &Audit, hasher: &mut DefaultHasher) {
    for source in &audit.sources {
        hash_tree(&source.path, hasher, false);
        for import in &source.imports {
            hash_tree(import, hasher, false);
        }
    }
}

fn hash_ancestor_candidates(
    directory: &Path,
    watch_root: Option<&Path>,
    hasher: &mut DefaultHasher,
) {
    for ancestor in directory
        .ancestors()
        .filter(|ancestor| watch_root.is_none_or(|root| !ancestor.starts_with(root)))
    {
        for name in [
            "AGENTS.override.md",
            "AGENTS.md",
            "AGENTS.MD",
            "CLAUDE.md",
            "CLAUDE.MD",
            "CLAUDE.local.md",
            ".claude/CLAUDE.md",
            ".claude/settings.json",
            ".claude/settings.local.json",
        ] {
            hash_tree(&ancestor.join(name), hasher, false);
        }
        hash_tree(&ancestor.join(".claude/rules"), hasher, true);
        hash_tree(&ancestor.join(".cursor/rules"), hasher, true);
    }
}

fn ignored_watch_entry(entry: &DirEntry) -> bool {
    entry.depth() > 0
        && entry.file_type().is_dir()
        && matches!(
            entry.file_name().to_str(),
            Some(".git" | "target" | "node_modules")
        )
}

fn relevant_project_file(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    matches!(
        name,
        "AGENTS.md"
            | "AGENTS.MD"
            | "AGENTS.override.md"
            | "CLAUDE.md"
            | "CLAUDE.MD"
            | "CLAUDE.local.md"
            | "project.toml"
            | "local.toml"
            | "settings.json"
            | "settings.local.json"
    ) || path
        .components()
        .any(|component| component.as_os_str() == ".mdmanager")
        || (path.extension().and_then(|extension| extension.to_str()) == Some("md")
            && path
                .components()
                .zip(path.components().skip(1))
                .any(|(left, right)| left.as_os_str() == ".claude" && right.as_os_str() == "rules"))
        || (path.extension().and_then(|extension| extension.to_str()) == Some("mdc")
            && path
                .components()
                .zip(path.components().skip(1))
                .any(|(left, right)| left.as_os_str() == ".cursor" && right.as_os_str() == "rules"))
}

fn missing_project_text(id: &str) -> String {
    format!(
        "{} does not exist in this project.\n\nThe TUI does not edit instructions. Ask your coding agent to run 'mdmanager docs start', inspect the repository, and create or adopt the appropriate project instructions.",
        project::target_filename(id).expect("validated Project target")
    )
}

fn missing_local_text(target: local::ManagedTarget) -> String {
    let behavior = match target {
        local::ManagedTarget::Agents => {
            "When created, it becomes the selected project instruction file for Pi and Codex instead of AGENTS.md."
        }
        local::ManagedTarget::Claude => {
            "When created, Claude loads it after CLAUDE.md as additive project instructions."
        }
    };
    format!(
        "{} does not exist for this repository and machine.\n\n{behavior}\n\nThe TUI does not edit instructions. Ask your coding agent to run 'mdmanager docs start' and configure Local Instructions if you need them.",
        target.filename()
    )
}

fn no_active_profile_text() -> String {
    [
        "No Global Profile is active on this machine, so this target is not applied yet.",
        "Open the Library to browse every configured Profile and Section.",
        "The TUI does not edit instructions. Ask your coding agent to run 'mdmanager docs start' when a Profile should be validated and applied.",
    ]
    .join("\n\n")
}

fn global_setup_text() -> String {
    [
        "Global Instructions are not managed by mdmanager.ai on this machine.",
        "Existing global instruction files remain visible on Home and in Context.",
        "The TUI does not edit instructions. Ask your coding agent to run 'mdmanager docs start', inspect the current instructions, and configure the intended Global Profile.",
    ]
    .join("\n\n")
}

fn discovered_global_sources(
    paths: &Paths,
    directory: &Path,
    global: Option<&GlobalConfig>,
    active_profile: Option<&str>,
) -> Vec<(ContextRuntime, ContextSource)> {
    let configured = global.map_or_else(Vec::new, |global| {
        let ids: Vec<&str> = match active_profile {
            Some(profile) => global
                .profile_target_names(profile)
                .map(|ids| ids.collect())
                .unwrap_or_default(),
            None => global.target_names().collect(),
        };
        ids.into_iter()
            .filter_map(|id| global.target_path(id).ok())
            .collect()
    });
    ContextRuntime::ALL
        .into_iter()
        .filter_map(|runtime| {
            Audit::resolve(runtime, directory, paths, None)
                .sources
                .into_iter()
                .find(|source| {
                    source.scope == "user"
                        && source.group == SourceGroup::Startup
                        && !configured.contains(&source.path)
                        && matches!(
                            source.state,
                            SourceState::Startup
                                | SourceState::SelectedEmpty
                                | SourceState::Truncated { .. }
                        )
                })
                .map(|source| (runtime, source))
        })
        .collect()
}

fn active_global_profile(global: Option<&GlobalConfig>) -> Result<Option<String>, String> {
    global.map_or(Ok(None), deploy::active_profile)
}

fn personal_section_usage(app: &App, id: &str) -> String {
    let profiles = app.global.as_ref().map_or_else(Vec::new, |global| {
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
    let local_targets = app
        .inspection
        .personal_section_targets
        .get(id)
        .cloned()
        .unwrap_or_default();
    let mut usage = if profiles.is_empty() {
        String::new()
    } else if profiles.len() > 1
        && app
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

fn project_section_usage(project: &Workspace, id: &str) -> String {
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

// A Profile that is not active has no deployment obligation, so its targets are
// described by comparing the rendered composition with the file on disk instead
// of with deployment status vocabulary.
fn global_comparison(view: &deploy::TargetView) -> String {
    if view.status == GlobalTargetStatus::Missing {
        "target file not found".into()
    } else if view.difference.is_none() {
        "matches current file".into()
    } else {
        "differs from current file".into()
    }
}

fn profile_deploys(global: &GlobalConfig, profile: &str, target: &str) -> bool {
    global
        .profile_target_names(profile)
        .ok()
        .is_some_and(|ids| ids.collect::<Vec<_>>().contains(&target))
}

fn profile_target_reference(app: &App, profile: &str, index: usize) -> Option<ManagedRef> {
    app.global.as_ref().and_then(|global| {
        let target = global.target_names().nth(index)?;
        profile_deploys(global, profile, target).then(|| ManagedRef::Global {
            profile: profile.to_owned(),
            target: target.to_owned(),
        })
    })
}

fn home_target_reference(app: &App) -> Option<ManagedRef> {
    match app.home_items().get(app.home_index)? {
        HomeItem::ProjectTarget(id) => Some(ManagedRef::Project(id.clone())),
        HomeItem::LocalTarget(target) => Some(ManagedRef::Local(*target)),
        HomeItem::GlobalTarget(target) => {
            app.global_active_profile
                .as_ref()
                .map(|profile| ManagedRef::Global {
                    profile: profile.clone(),
                    target: target.clone(),
                })
        }
        _ => None,
    }
}

fn symlink_info(app: &App, path: &Path) -> Option<SymlinkInfo> {
    app.inspection
        .files
        .get(path)
        .and_then(|file| file.symlink.clone())
}

fn regular_file_resolves_to(app: &App, path: &Path, resolved: &Path) -> bool {
    app.inspection
        .files
        .get(path)
        .is_some_and(|file| file.regular && file.canonical.as_deref() == Some(resolved))
}

fn managed_destination(app: &App, resolved: &Path) -> Option<ManagedDestination> {
    if let Some(destination) = app.project.as_ref().and_then(|project| {
        project.target_names().find_map(|id| {
            let path = project.target_path(id).ok()?;
            regular_file_resolves_to(app, &path, resolved).then(|| ManagedDestination {
                scope: "Project",
                target: project::target_filename(id)
                    .expect("validated Project target")
                    .into(),
                status: managed_workspace(app, &ManagedRef::Project(id.to_owned()))
                    .ok()
                    .map(|view| view.instruction_status),
            })
        })
    }) {
        return Some(destination);
    }

    if let Some(destination) = app.local_repository.as_ref().and_then(|local_repository| {
        [local::ManagedTarget::Agents, local::ManagedTarget::Claude]
            .into_iter()
            .find_map(|target| {
                let path = local_repository.root.join(target.filename());
                (app.inspection.home_items.iter().any(
                    |item| matches!(item, HomeItem::LocalTarget(candidate) if *candidate == target),
                ) && regular_file_resolves_to(app, &path, resolved))
                .then(|| ManagedDestination {
                    scope: "Local",
                    target: target.filename().into(),
                    status: managed_workspace(app, &ManagedRef::Local(target))
                        .ok()
                        .map(|view| view.instruction_status),
                })
            })
    }) {
        return Some(destination);
    }

    let global = app.global.as_ref()?;
    let profile = app.global_active_profile.as_deref()?;
    global.profile_target_names(profile).ok()?.find_map(|id| {
        let path = global.target_path(id).ok()?;
        regular_file_resolves_to(app, &path, resolved).then(|| ManagedDestination {
            scope: "Global",
            target: path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(id)
                .into(),
            status: managed_workspace(
                app,
                &ManagedRef::Global {
                    profile: profile.to_owned(),
                    target: id.to_owned(),
                },
            )
            .ok()
            .map(|view| view.instruction_status),
        })
    })
}

fn symlink_target_status(app: &App, path: &Path, info: &SymlinkInfo) -> String {
    match &info.resolution {
        SymlinkResolution::Resolved(resolved) => {
            if let Some(destination) = managed_destination(app, resolved) {
                format!(
                    "{} {} · managed by mdmanager.ai · {}",
                    destination.scope,
                    destination.target,
                    destination
                        .status
                        .map_or("invalid", InstructionStatus::label)
                )
            } else {
                let project_root = app.inspection.canonical(&app.project_root);
                let source_is_project = path
                    .parent()
                    .and_then(|parent| app.inspection.canonical(parent))
                    .zip(project_root.as_ref())
                    .is_some_and(|(parent, root)| parent.starts_with(root));
                if source_is_project && project_root.is_some_and(|root| !resolved.starts_with(root))
                {
                    "outside the project · not managed by mdmanager.ai".into()
                } else {
                    "existing file · not managed by mdmanager.ai".into()
                }
            }
        }
        SymlinkResolution::Missing => "missing".into(),
        SymlinkResolution::Unresolved => "cannot resolve".into(),
    }
}

fn symlink_home_status(app: &App, info: &SymlinkInfo) -> String {
    let target = if info.target.is_absolute() {
        display_path(&info.target, &app.paths, &app.inspection.context.directory)
    } else {
        info.target.display().to_string()
    };
    match &info.resolution {
        SymlinkResolution::Resolved(resolved) => {
            if let Some(destination) = managed_destination(app, resolved) {
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

fn global_symlink_home_status(app: &App, id: &str, path: &Path, info: &SymlinkInfo) -> String {
    let target = if info.target.is_absolute() {
        display_path(&info.target, &app.paths, &app.inspection.context.directory)
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
    let destination = managed_destination(app, resolved).map_or_else(
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
    let comparison = app
        .global
        .as_ref()
        .zip(app.global_active_profile.as_deref())
        .and_then(|(global, profile)| global.render(profile, id).ok())
        .zip(app.inspection.document(path).ok())
        .map_or("no active Profile", |(expected, deployed)| {
            if expected == deployed {
                "matches Profile"
            } else {
                "differs from Profile"
            }
        });
    format!("symlink → {target} · {destination} · {comparison}")
}

fn external_file_about(app: &App, kind: &str, path: &Path) -> String {
    if let Err(error) = app.inspection.document(path) {
        return format!(
            "{kind}
{error}"
        );
    }
    let Some(info) = symlink_info(app, path) else {
        return format!(
            "{kind} · existing · not managed by mdmanager.ai\nPath: {}",
            path.display()
        );
    };
    format!(
        "{kind}\nPath: {}\nType: symlink\nPoints to: {}\nLink: not managed by mdmanager.ai\nTarget: {}",
        path.display(),
        info.target.display(),
        symlink_target_status(app, path, &info)
    )
}

fn managed_workspace(app: &App, reference: &ManagedRef) -> Result<ManagedWorkspace, String> {
    app.inspection
        .managed
        .iter()
        .find(|(candidate, _)| candidate == reference)
        .map(|(_, workspace)| workspace.clone())
        .unwrap_or_else(|| Err("Managed target is unavailable".into()))
}

pub(crate) fn run(
    paths: Paths,
    global: Option<GlobalConfig>,
    global_error: Option<String>,
) -> Result<(), String> {
    let directory = env::current_dir()
        .map_err(|error| format!("cannot determine launch directory: {error}"))?;
    let mut app = App::new_at_with_global_error(paths, global, global_error, directory)?;
    // The Claude subfolder scan is already running: App construction started it
    // before the terminal opens, so the intro overlaps that work.
    app.intro_started = Some(Instant::now());
    ratatui::run(|terminal| session(terminal, &mut app))
        .map_err(|error| format!("terminal interaction failed: {error}"))
}

fn session(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> std::io::Result<()> {
    loop {
        app.poll_intro();
        app.poll_context_scan();
        app.poll_reload();
        app.poll_message();
        terminal.draw(|frame| render(frame, app))?;
        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        if handle_key(app, key) == SessionAction::Quit {
            return Ok(());
        }
    }
}

#[derive(Eq, PartialEq)]
enum SessionAction {
    Continue,
    Quit,
}

fn handle_key(app: &mut App, key: KeyEvent) -> SessionAction {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return SessionAction::Quit;
    }
    if app.intro_started.is_some() {
        if key.code == KeyCode::Char('q') {
            return SessionAction::Quit;
        }
        app.intro_started = None;
        return SessionAction::Continue;
    }
    if app.help {
        if matches!(key.code, KeyCode::Esc | KeyCode::Char('?')) {
            app.help = false;
        } else if key.code == KeyCode::Char('q') {
            return SessionAction::Quit;
        }
        return SessionAction::Continue;
    }
    if app.theme_picker.is_some() {
        handle_theme_picker_key(app, key);
        return SessionAction::Continue;
    }
    if app.picker.is_some() {
        handle_picker_key(app, key);
        return SessionAction::Continue;
    }
    if app.search.is_some() {
        handle_search_key(app, key);
        return SessionAction::Continue;
    }
    if app.line_prompt.is_some() {
        handle_line_prompt_key(app, key);
        return SessionAction::Continue;
    }
    if key.modifiers.contains(KeyModifiers::SHIFT)
        && matches!(key.code, KeyCode::Up | KeyCode::Down)
    {
        app.scroll = if key.code == KeyCode::Down {
            app.scroll.saturating_add(PAGE_LINES).min(app.max_scroll)
        } else {
            app.scroll.saturating_sub(PAGE_LINES)
        };
        return SessionAction::Continue;
    }
    match key.code {
        KeyCode::Char('q') => SessionAction::Quit,
        KeyCode::Char('?') => {
            app.help = true;
            app.clear_message();
            SessionAction::Continue
        }
        KeyCode::Char('t') => {
            app.open_theme_picker();
            SessionAction::Continue
        }
        KeyCode::Esc => app.back(),
        KeyCode::Up => {
            match &app.view {
                View::Home => app.home_index = app.home_index.saturating_sub(1),
                View::Context => app.move_context_selection(false),
                View::Managed(_) | View::GlobalLibrary | View::GlobalProfile(_) => {
                    app.section_index = app.section_index.saturating_sub(1);
                    app.scroll = 0;
                }
                _ => app.scroll = app.scroll.saturating_sub(1),
            }
            SessionAction::Continue
        }
        KeyCode::Down => {
            match &app.view {
                View::Home => {
                    app.home_index =
                        (app.home_index + 1).min(app.home_items().len().saturating_sub(1));
                }
                View::Context => app.move_context_selection(true),
                View::Managed(reference) => {
                    let len = managed_workspace(app, reference)
                        .map_or(1, |workspace| workspace.sections.len() + 1);
                    app.section_index = (app.section_index + 1).min(len.saturating_sub(1));
                    app.scroll = 0;
                }
                View::GlobalLibrary => {
                    let len = app.library_entries().len();
                    app.section_index = (app.section_index + 1).min(len.saturating_sub(1));
                    app.scroll = 0;
                }
                View::GlobalProfile(_) => {
                    let len = app
                        .global
                        .as_ref()
                        .map_or(0, |global| global.target_names().count());
                    app.section_index = (app.section_index + 1).min(len.saturating_sub(1));
                    app.scroll = 0;
                }
                _ => app.scroll = app.scroll.saturating_add(1).min(app.max_scroll),
            }
            SessionAction::Continue
        }
        KeyCode::Left if matches!(app.view, View::Context | View::Source) => {
            let index = app.context.runtime_index.saturating_sub(1);
            app.select_runtime(index);
            SessionAction::Continue
        }
        KeyCode::Right if matches!(app.view, View::Context | View::Source) => {
            let index = (app.context.runtime_index + 1).min(ContextRuntime::ALL.len() - 1);
            app.select_runtime(index);
            SessionAction::Continue
        }
        KeyCode::Char('r') if matches!(app.view, View::Home | View::Context | View::Source) => {
            app.open_picker();
            SessionAction::Continue
        }
        KeyCode::Char('/') if app.searchable_text().is_some() => {
            app.search = Some(SearchPrompt {
                query: String::new(),
            });
            SessionAction::Continue
        }
        KeyCode::Char('g') if app.numbered_text().is_some() => {
            app.line_prompt = Some(LinePrompt {
                input: String::new(),
            });
            SessionAction::Continue
        }
        KeyCode::Char('d') => {
            let reference = match &app.view {
                View::Home => home_target_reference(app),
                View::Managed(reference) | View::ManagedDocument(reference) => {
                    Some(reference.clone())
                }
                View::GlobalProfile(profile) => {
                    profile_target_reference(app, profile, app.section_index)
                }
                _ => None,
            };
            if let Some(reference) = reference {
                match managed_workspace(app, &reference) {
                    Ok(workspace) if workspace.difference.is_some() => {
                        app.open(View::Diff(reference));
                    }
                    Ok(_) if matches!(app.view, View::Home) => {
                        app.set_message(true, "No difference.");
                    }
                    Err(error) if matches!(app.view, View::Home) => {
                        app.set_message(false, error);
                    }
                    _ => {}
                }
            } else if matches!(app.view, View::Diff(_)) {
                app.back();
            } else if matches!(app.view, View::Home) {
                app.set_message(false, "This item has no managed difference.");
            }
            SessionAction::Continue
        }
        KeyCode::Enter => {
            enter(app);
            SessionAction::Continue
        }
        _ => SessionAction::Continue,
    }
}

fn handle_picker_key(app: &mut App, key: KeyEvent) {
    let filtered = filtered_runtimes(app.picker.as_ref().unwrap());
    match key.code {
        KeyCode::Esc => app.picker = None,
        KeyCode::Backspace => {
            let picker = app.picker.as_mut().unwrap();
            picker.query.pop();
            picker.selected = filtered_runtimes(picker).first().copied().unwrap_or(0);
        }
        KeyCode::Up => {
            let picker = app.picker.as_mut().unwrap();
            if let Some(position) = filtered.iter().position(|index| *index == picker.selected) {
                picker.selected = filtered[position.saturating_sub(1)];
            }
        }
        KeyCode::Down => {
            let picker = app.picker.as_mut().unwrap();
            if let Some(position) = filtered.iter().position(|index| *index == picker.selected) {
                picker.selected = filtered[(position + 1).min(filtered.len().saturating_sub(1))];
            }
        }
        KeyCode::Enter => {
            if let Some(index) = filtered
                .iter()
                .find(|index| **index == app.picker.as_ref().unwrap().selected)
                .copied()
                .or_else(|| filtered.first().copied())
            {
                app.picker = None;
                app.select_runtime(index);
            }
        }
        KeyCode::Char(character)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            let picker = app.picker.as_mut().unwrap();
            picker.query.push(character);
            picker.selected = filtered_runtimes(picker).first().copied().unwrap_or(0);
        }
        _ => {}
    }
}

fn filtered_runtimes(picker: &RuntimePicker) -> Vec<usize> {
    let query = picker.query.to_lowercase();
    ContextRuntime::ALL
        .iter()
        .enumerate()
        .filter(|(_, runtime)| runtime.label().to_lowercase().contains(&query))
        .map(|(index, _)| index)
        .collect()
}

fn preview_theme_picker(app: &App) {
    let Some(index) = app.theme_picker.as_ref().map(|picker| picker.selected) else {
        return;
    };
    if let Some(selected) = theme::names()
        .nth(index)
        .and_then(|name| theme::resolve(name).ok())
    {
        ACTIVE_THEME.set(selected);
    }
}

fn handle_theme_picker_key(app: &mut App, key: KeyEvent) {
    let filtered = filtered_themes(app.theme_picker.as_ref().unwrap());
    match key.code {
        KeyCode::Esc => {
            let picker = app.theme_picker.take().unwrap();
            ACTIVE_THEME.set(picker.original);
        }
        KeyCode::Backspace => {
            let picker = app.theme_picker.as_mut().unwrap();
            picker.query.pop();
            if let Some(selected) = filtered_themes(picker).first().copied() {
                picker.selected = selected;
                preview_theme_picker(app);
            }
        }
        KeyCode::Up => {
            let picker = app.theme_picker.as_mut().unwrap();
            if let Some(position) = filtered.iter().position(|index| *index == picker.selected) {
                picker.selected = filtered[position.saturating_sub(1)];
            }
            preview_theme_picker(app);
        }
        KeyCode::Down => {
            let picker = app.theme_picker.as_mut().unwrap();
            if let Some(position) = filtered.iter().position(|index| *index == picker.selected) {
                picker.selected = filtered[(position + 1).min(filtered.len().saturating_sub(1))];
            }
            preview_theme_picker(app);
        }
        KeyCode::Enter => {
            if let Some(index) = filtered
                .iter()
                .find(|index| **index == app.theme_picker.as_ref().unwrap().selected)
                .copied()
                .or_else(|| filtered.first().copied())
                && let Some(name) = theme::names().nth(index)
            {
                let original = app.theme_picker.take().unwrap().original;
                match config::set_theme(&app.paths, name) {
                    Ok(()) => {
                        ACTIVE_THEME.set(theme::resolve(name).unwrap());
                        app.set_message(true, format!("Theme saved · {name}"));
                    }
                    Err(error) => {
                        ACTIVE_THEME.set(original);
                        app.set_message(false, format!("Could not save theme · {error}"));
                    }
                }
            }
        }
        KeyCode::Char(character)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            let picker = app.theme_picker.as_mut().unwrap();
            picker.query.push(character);
            if let Some(selected) = filtered_themes(picker).first().copied() {
                picker.selected = selected;
                preview_theme_picker(app);
            }
        }
        _ => {}
    }
}

fn filtered_themes(picker: &ThemePicker) -> Vec<usize> {
    let query = picker.query.to_lowercase();
    theme::names()
        .enumerate()
        .filter(|(_, name)| name.contains(&query))
        .map(|(index, _)| index)
        .collect()
}

fn handle_search_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => app.search = None,
        KeyCode::Backspace => {
            app.search.as_mut().unwrap().query.pop();
        }
        KeyCode::Enter => {
            let query = app.search.take().unwrap().query;
            app.apply_search(query);
        }
        KeyCode::Char(character)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            app.search.as_mut().unwrap().query.push(character);
        }
        _ => {}
    }
}

fn handle_line_prompt_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => app.line_prompt = None,
        KeyCode::Backspace => {
            app.line_prompt.as_mut().unwrap().input.pop();
        }
        KeyCode::Enter => {
            let input = app.line_prompt.take().unwrap().input;
            app.go_to_line(input);
        }
        KeyCode::Char(character) if character.is_ascii_digit() => {
            app.line_prompt.as_mut().unwrap().input.push(character);
        }
        _ => {}
    }
}

fn enter(app: &mut App) {
    match app.view.clone() {
        View::Home => enter_home(app),
        View::Context => app.open_context_entry(),
        View::Managed(reference) => {
            if app.section_index == 0 {
                app.open(View::ManagedDocument(reference));
            } else if let Ok(workspace) = managed_workspace(app, &reference)
                && let Some(section) = workspace.sections.get(app.section_index - 1)
            {
                app.open(View::SectionDocument {
                    title: section.name.clone(),
                    path: section.path.clone(),
                    about: format!(
                        "Section source · used by {}\nPath: {}",
                        workspace.title,
                        section.path.display()
                    ),
                });
            }
        }
        View::GlobalLibrary => match app.library_entries().get(app.section_index).cloned() {
            Some(LibraryEntry::Profile(profile)) => app.open(View::GlobalProfile(profile)),
            Some(LibraryEntry::PersonalSection(id)) => {
                let Some(global) = &app.global else { return };
                let (Some(section), Some(path)) = (global.section(&id), global.section_path(&id))
                else {
                    return;
                };
                let usage = personal_section_usage(app, &id);
                let usage = usage.strip_prefix("used by ").unwrap_or(&usage);
                app.open(View::SectionDocument {
                    title: section.name.clone(),
                    about: format!(
                        "Personal Section · {}\nPath: {}\nUsed by: {}",
                        section.name,
                        path.display(),
                        usage
                    ),
                    path,
                });
            }
            Some(LibraryEntry::ProjectSection(id)) => {
                let Some(project) = &app.project else { return };
                let (Some(section), Some(path)) = (project.section(&id), project.section_path(&id))
                else {
                    return;
                };
                app.open(View::SectionDocument {
                    title: section.name.clone(),
                    about: format!(
                        "Project Section · committed with this repository\nPath: {}\nUsed by: {}",
                        path.display(),
                        project_section_usage(project, &id)
                    ),
                    path,
                });
            }
            None => {}
        },
        View::GlobalProfile(profile) => {
            if let Some(reference) = profile_target_reference(app, &profile, app.section_index) {
                app.open(View::Managed(reference));
            }
        }
        _ => {}
    }
}

fn enter_home(app: &mut App) {
    let Some(item) = app.home_items().get(app.home_index).cloned() else {
        return;
    };
    match item {
        HomeItem::Context => app.open(View::Context),
        HomeItem::ProjectInvalid => app.open(View::Info {
            kind: DiagnosticKind::General,
            title: "Invalid project configuration".into(),
            text: app
                .project_error
                .clone()
                .unwrap_or_else(|| "Project configuration is invalid".into()),
        }),
        HomeItem::ProjectMissing(id) => app.open(View::Info {
            kind: DiagnosticKind::General,
            title: format!(
                "{} · not found",
                project::target_filename(&id).expect("validated Project target")
            ),
            text: missing_project_text(&id),
        }),
        HomeItem::ProjectTarget(id) => app.open(View::Managed(ManagedRef::Project(id))),
        HomeItem::ProjectExternal { id, path } => app.open(View::File {
            title: project::target_filename(&id)
                .expect("validated Project target")
                .into(),
            about: "Project file".into(),
            path,
        }),
        HomeItem::LocalDisable(runtime, status) => {
            let source = app
                .local_repository
                .as_ref()
                .and_then(|local_repository| {
                    local_repository.disabled_source(runtime).ok().flatten()
                })
                .map_or_else(|| "unknown".into(), |path| path.display().to_string());
            app.open(View::Info {
                kind: DiagnosticKind::General,
                title: format!("{} local exclusion", runtime.label()),
                text: format!(
                    "Status: {}\nSource: {source}\n\nThe TUI only reports this state. Ask your coding agent to run 'mdmanager docs start' before changing or restoring it.",
                    match status {
                        local::DisableStatus::Owned => "active",
                        local::DisableStatus::Modified => "changed; no longer safely owned",
                        local::DisableStatus::Missing => "owned file is missing",
                        local::DisableStatus::None => "not configured",
                    }
                ),
            });
        }
        HomeItem::LocalInvalid(error) => app.open(View::Info {
            kind: DiagnosticKind::General,
            title: "Invalid local configuration".into(),
            text: error,
        }),
        HomeItem::LocalMissing(target) => app.open(View::Info {
            kind: DiagnosticKind::General,
            title: format!("{} · not found", target.filename()),
            text: missing_local_text(target),
        }),
        HomeItem::LocalExternal(target, path) => app.open(View::File {
            title: target.filename().into(),
            about: "Local file".into(),
            path,
        }),
        HomeItem::LocalTarget(target) => app.open(View::Managed(ManagedRef::Local(target))),
        HomeItem::GlobalInvalid => app.open(View::Info {
            kind: DiagnosticKind::Global,
            title: if app.global.is_some() {
                "Deployment state needs attention".into()
            } else {
                "Global Instructions unavailable".into()
            },
            text: app
                .global_error
                .clone()
                .map(|error| global_diagnostic_text(&error))
                .unwrap_or_else(|| "Global configuration is invalid".into()),
        }),
        HomeItem::GlobalSetup => app.open(View::Info {
            kind: DiagnosticKind::General,
            title: "Global Instructions".into(),
            text: global_setup_text(),
        }),
        HomeItem::GlobalExternal(index) => {
            if let Some((runtime, source)) = app.global_sources.get(index) {
                app.open(View::File {
                    title: format!("{} · {}", runtime.label(), source.display),
                    path: source.path.clone(),
                    about: format!(
                        "Runtime: {}\n{}",
                        runtime.label(),
                        "Global instruction file"
                    ),
                });
            }
        }
        HomeItem::GlobalTarget(id) => {
            if let Some(profile) = app.global_active_profile.clone() {
                app.open(View::Managed(ManagedRef::Global {
                    profile,
                    target: id,
                }));
            } else {
                app.open(View::Info {
                    kind: DiagnosticKind::General,
                    title: format!("{} · not applied yet", target_display_name(&id)),
                    text: no_active_profile_text(),
                });
            }
        }
        HomeItem::Library => app.open(View::GlobalLibrary),
    }
}

fn render(frame: &mut Frame<'_>, app: &mut App) {
    frame.render_widget(
        Block::default().style(
            Style::new()
                .fg(active_theme().text)
                .bg(active_theme().background),
        ),
        frame.area(),
    );
    if frame.area().width < 54 || frame.area().height < 16 {
        frame.render_widget(
            Paragraph::new("mdmanager.ai\n\nTerminal too small.\nNeed 54x16.\n\nq quit")
                .style(
                    Style::new()
                        .fg(active_theme().text)
                        .bg(active_theme().background),
                )
                .block(
                    Block::default()
                        .title(" mdmanager.ai ")
                        .title_style(
                            Style::new()
                                .fg(active_theme().primary)
                                .add_modifier(Modifier::BOLD),
                        )
                        .border_style(Style::new().fg(active_theme().dim))
                        .borders(Borders::ALL),
                ),
            frame.area(),
        );
        return;
    }
    if let Some(phase) = app
        .intro_started
        .and_then(|started| intro_phase(started.elapsed()))
    {
        render_intro(frame, phase);
        return;
    }
    render_page(frame, app);
    if app.help {
        let screen = frame.area();
        frame.buffer_mut().set_style(
            screen,
            Style::new()
                .fg(active_theme().muted)
                .bg(active_theme().background)
                .add_modifier(Modifier::DIM),
        );
        render_help(frame, app, screen);
    }
}

// One brightness step of the startup intro: the wordmark color and, once the
// hold phase begins, the caption color. `None` means the intro is over.
fn intro_phase(elapsed: Duration) -> Option<(Color, Option<Color>)> {
    match elapsed.as_millis() {
        0..=299 => Some((active_theme().dim, None)),
        300..=599 => Some((active_theme().muted, None)),
        600..=1399 => Some((active_theme().primary, Some(active_theme().text))),
        1400..=1599 => Some((active_theme().muted, Some(active_theme().muted))),
        1600..=1799 => Some((active_theme().dim, Some(active_theme().dim))),
        _ => None,
    }
}

fn render_intro(frame: &mut Frame<'_>, (mark, caption): (Color, Option<Color>)) {
    let mut lines = INTRO_WORDMARK
        .map(|line| Line::styled(line, Style::new().fg(mark).add_modifier(Modifier::BOLD)))
        .to_vec();
    lines.push(Line::raw(""));
    // The caption line is reserved even while hidden so the mark never shifts.
    lines.push(match caption {
        Some(color) => Line::styled("mdmanager.ai", Style::new().fg(color)),
        None => Line::raw(""),
    });
    let area = frame.area();
    let height = u16::try_from(lines.len())
        .unwrap_or(u16::MAX)
        .min(area.height);
    let block = Rect::new(
        area.x,
        area.y + area.height.saturating_sub(height) / 2,
        area.width,
        height,
    );
    frame.render_widget(
        Paragraph::new(Text::from(lines)).alignment(Alignment::Center),
        block,
    );
}

fn render_page(frame: &mut Frame<'_>, app: &mut App) {
    if app.theme_picker.is_some() {
        render_theme_picker(frame, app, frame.area());
        return;
    }
    if app.picker.is_some() {
        render_picker(frame, app, frame.area());
        return;
    }
    if let View::Info { title, text, .. } = app.view.clone() {
        render_info(frame, app, frame.area(), &title, &text);
        return;
    }
    if matches!(app.view, View::GlobalLibrary) {
        render_global_library(frame, app, frame.area());
        return;
    }
    if let View::GlobalProfile(profile) = app.view.clone() {
        render_global_profile(frame, app, frame.area(), &profile);
        return;
    }
    let canvas = canvas_area(frame.area(), app);
    let header_height = header_height(app, canvas.width);
    let footer_height = if matches!(app.view, View::Home) { 2 } else { 3 };
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(header_height),
        Constraint::Min(8),
        Constraint::Length(footer_height),
    ])
    .areas(canvas);
    render_header(frame, app, header);
    match app.view.clone() {
        View::Home => render_home(frame, app, body),
        View::Info { .. } => unreachable!("Info views use the focus panel"),
        View::File { title, path, about } => {
            let about = external_file_about(app, &about, &path);
            let content = app.inspection.document(&path).unwrap_or_else(|error| error);
            render_document(frame, app, body, &title, &about, &content, true);
        }
        View::SectionDocument { title, path, about } => {
            let content = app.inspection.document(&path).unwrap_or_else(|error| error);
            render_document(frame, app, body, &title, &about, &content, true);
        }
        View::Managed(reference) => render_managed(frame, app, body, &reference),
        View::ManagedDocument(reference) => match managed_workspace(app, &reference) {
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
                    app,
                    body,
                    "Generated document",
                    &about,
                    &workspace.rendered,
                    true,
                );
            }
            Err(error) => {
                render_document(frame, app, body, "Managed target", "", &error, false);
            }
        },
        View::Diff(reference) => render_diff(frame, app, body, &reference),
        View::GlobalLibrary | View::GlobalProfile(_) => {
            unreachable!("Library views use focus panels")
        }
        View::Context => render_context(frame, app, body),
        View::Source => render_source(frame, app, body),
    }
    render_footer(frame, app, footer);
    if app.search.is_some() {
        render_search(frame, app);
    }
    if app.line_prompt.is_some() {
        render_line_prompt(frame, app);
    }
}

fn canvas_area(area: Rect, app: &App) -> Rect {
    let width = match app.view {
        View::Home => home_canvas_width(app, area.width),
        View::Info { .. } => area.width.min(COMPACT_WIDTH),
        _ => area.width,
    };
    let height = if matches!(app.view, View::Home) {
        home_canvas_height(app).min(area.height)
    } else {
        area.height
    };
    let x = area.x + area.width.saturating_sub(width) / 2;
    let free_height = area.height.saturating_sub(height);
    let y = area.y
        + if matches!(app.view, View::Home) {
            free_height / 2
        } else {
            0
        };
    Rect::new(x, y, width, height)
}

fn home_canvas_width(app: &App, available: u16) -> u16 {
    let item_width = app
        .home_items()
        .iter()
        .map(|item| {
            home_line(app, item)
                .width()
                .max(home_group_title(app, home_group(item)).width())
                + 4
        })
        .max()
        .unwrap_or(0);
    let header_width = home_header_line(app).map_or(0, |line| line.width() + 2);
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

fn home_canvas_height(app: &App) -> u16 {
    let items = app.home_items();
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

fn home_group(item: &HomeItem) -> &'static str {
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

fn home_group_title(app: &App, group: &str) -> String {
    match group {
        "CONTEXT" => format!("CONTEXT · {}", app.inspection.context.runtime.label()),
        "PROJECT" => format!(
            "PROJECT INSTRUCTIONS · {} · committed + shared",
            app.project_root
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("project")
        ),
        "LOCAL" => "LOCAL INSTRUCTIONS · this repository + this machine".into(),
        "GLOBAL" => match (&app.global, &app.global_active_profile) {
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

fn render_header(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let path = match &app.view {
        View::Home => app.project_root.display().to_string(),
        View::Info { title, .. } => title.clone(),
        View::File { title, .. } | View::SectionDocument { title, .. } => title.clone(),
        View::Managed(reference) | View::ManagedDocument(reference) | View::Diff(reference) => {
            managed_workspace(app, reference)
                .map_or_else(|_| "Managed target".into(), |workspace| workspace.title)
        }
        View::GlobalLibrary => "Profiles & Sections".into(),
        View::GlobalProfile(profile) => format!("Global Profile {profile}"),
        View::Context => format!("Context · {}", app.inspection.context.runtime.label()),
        View::Source => app.selected_source().map_or_else(
            || "Context · Source".into(),
            |source| source.display.clone(),
        ),
    };
    let block = Block::default()
        .title(" mdmanager.ai ")
        .title_style(
            Style::new()
                .fg(active_theme().primary)
                .add_modifier(Modifier::BOLD),
        )
        .border_style(Style::new().fg(active_theme().dim))
        .style(Style::new().bg(active_theme().surface))
        .borders(Borders::ALL);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let mut lines = vec![Line::styled(
        truncate_start(&path, usize::from(inner.width)),
        Style::new()
            .fg(active_theme().secondary)
            .add_modifier(Modifier::BOLD),
    )];
    if let Some(line) = home_header_line(app) {
        lines.push(line);
    }
    frame.render_widget(
        Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false }),
        inner,
    );
}

fn header_height(app: &App, width: u16) -> u16 {
    let Some(line) = home_header_line(app) else {
        return 3;
    };
    u16::try_from(
        Paragraph::new(line)
            .wrap(Wrap { trim: false })
            .line_count(width.saturating_sub(2).max(1)),
    )
    .unwrap_or(u16::MAX)
    .saturating_add(3)
}

fn home_header_line(app: &App) -> Option<Line<'static>> {
    if !matches!(app.view, View::Home) {
        return None;
    }
    if let Some((ok, text)) = &app.message {
        return Some(Line::styled(
            format!(
                "{} {}",
                if *ok { "●" } else { "!" },
                relative_message(app, text)
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

fn render_home(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    app.scroll = 0;
    app.max_scroll = 0;
    let items = app.home_items();
    app.home_index = app.home_index.min(items.len().saturating_sub(1));
    let mut rows = Vec::new();
    let mut selected_row = 0;
    let mut previous_group = "";
    for (index, item) in items.iter().enumerate() {
        let group = home_group(item);
        if group != previous_group {
            if !rows.is_empty() {
                rows.push(ListItem::new(""));
            }
            let title = home_group_title(app, group);
            rows.push(ListItem::new(Line::styled(
                title,
                Style::new()
                    .fg(active_theme().secondary)
                    .add_modifier(Modifier::BOLD),
            )));
            previous_group = group;
        }
        if index == app.home_index {
            selected_row = rows.len();
        }
        rows.push(ListItem::new(home_line(app, item)));
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

fn home_line(app: &App, item: &HomeItem) -> Line<'static> {
    match item {
        HomeItem::Context => {
            let startup = app
                .inspection
                .context
                .sources
                .iter()
                .filter(|source| loaded_at_startup(source))
                .count();
            let relevant = app
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
                app.context.scan_status,
                ContextScanStatus::NotStarted | ContextScanStatus::Scanning
            ) {
                spans.push(Span::styled(
                    app.context.scan_spinner().map_or_else(
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
            let status = app
                .project
                .as_ref()
                .and_then(|_| managed_workspace(app, &ManagedRef::Project(id.to_owned())).ok())
                .map_or("invalid", |view| view.instruction_status.label());
            home_status_line(
                project::target_filename(id).expect("validated Project target"),
                20,
                &format!("managed by mdmanager.ai · {status}"),
                managed_home_signal(app, &ManagedRef::Project(id.clone())),
            )
        }
        HomeItem::ProjectExternal { path, .. } => {
            let file = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("instruction file");
            let status = symlink_info(app, path).map_or_else(
                || "existing · not managed by mdmanager.ai".into(),
                |info| symlink_home_status(app, &info),
            );
            home_status_line(file, 20, &status, external_home_signal(app, path))
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
            let status = symlink_info(app, path).map_or_else(
                || "existing · not managed by mdmanager.ai".into(),
                |info| symlink_home_status(app, &info),
            );
            home_status_line(
                target.filename(),
                20,
                &status,
                external_home_signal(app, path),
            )
        }
        HomeItem::LocalTarget(target) => {
            let status = app
                .local_repository
                .as_ref()
                .and_then(|_| managed_workspace(app, &ManagedRef::Local(*target)).ok())
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
                managed_home_signal(app, &ManagedRef::Local(*target)),
            )
        }
        HomeItem::GlobalInvalid => Line::styled(
            if app.global.is_some() {
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
        HomeItem::GlobalExternal(index) => app.global_sources.get(*index).map_or_else(
            || Line::raw("  unknown global source"),
            |(runtime, source)| {
                let status = symlink_info(app, &source.path).map_or_else(
                    || "existing · not managed by mdmanager.ai".into(),
                    |info| symlink_home_status(app, &info),
                );
                Line::raw(format!(
                    "  {} {} {status}",
                    pad_right(runtime.label(), 8),
                    pad_right(&source.display, 28)
                ))
            },
        ),
        HomeItem::GlobalTarget(id) => {
            let status = app
                .global
                .as_ref()
                .and_then(|global| global.target_path(id).ok())
                .and_then(|path| {
                    symlink_info(app, &path)
                        .map(|info| global_symlink_home_status(app, id, &path, &info))
                })
                .unwrap_or_else(|| match (&app.global, &app.global_active_profile) {
                    (Some(_), Some(profile)) => managed_workspace(
                        app,
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
                if app
                    .global
                    .as_ref()
                    .and_then(|global| global.target_path(id).ok())
                    .is_some_and(|path| symlink_info(app, &path).is_some())
                {
                    HomeSignal::Invalid
                } else {
                    app.global_active_profile
                        .as_ref()
                        .map_or(HomeSignal::Missing, |profile| {
                            managed_home_signal(
                                app,
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
            let profiles = app
                .global
                .as_ref()
                .map_or(0, |global| global.profile_names().count());
            let personal = app
                .global
                .as_ref()
                .map_or(0, |global| global.manifest.sections.len());
            let project = app
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

#[derive(Clone, Copy)]
enum HomeSignal {
    Current,
    Changed,
    External,
    Missing,
    Invalid,
}

fn managed_home_signal(app: &App, reference: &ManagedRef) -> HomeSignal {
    managed_workspace(app, reference).map_or(HomeSignal::Invalid, |view| {
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

fn external_home_signal(app: &App, path: &Path) -> HomeSignal {
    if symlink_info(app, path).is_some() {
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

fn render_managed(frame: &mut Frame<'_>, app: &mut App, area: Rect, reference: &ManagedRef) {
    let workspace = match managed_workspace(app, reference) {
        Ok(workspace) => workspace,
        Err(error) => {
            let error = if matches!(reference, ManagedRef::Global { .. }) {
                global_diagnostic_text(&error)
            } else {
                error
            };
            render_document(
                frame,
                app,
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
    app.section_index = app.section_index.min(entries.saturating_sub(1));
    let link = symlink_info(app, &workspace.target);
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
                Span::raw(symlink_target_status(app, &workspace.target, info)),
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
        let mut state = ListState::default().with_selected(Some(app.section_index));
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
        let (title, content) = if app.section_index == 0 {
            (" Generated document ".into(), workspace.rendered.clone())
        } else {
            workspace.sections.get(app.section_index - 1).map_or_else(
                || (" Section ".into(), String::new()),
                |section| (format!(" {} ", section.name), section.content.clone()),
            )
        };
        render_plain_document(frame, app, preview_area, &title, &content, true, false);
    } else {
        app.max_scroll = 0;
        let mut state = ListState::default().with_selected(Some(app.section_index));
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

fn global_diagnostic_text(error: &str) -> String {
    if error.contains("mdmanager doctor") {
        error.to_owned()
    } else {
        format!("{error}\n\nAction: run `mdmanager doctor` for diagnosis and safe recovery.")
    }
}

fn render_global_library(frame: &mut Frame<'_>, app: &mut App, screen: Rect) {
    app.scroll = 0;
    app.max_scroll = 0;
    let entries = app.library_entries();
    if entries.is_empty() {
        render_info(
            frame,
            app,
            screen,
            "Library",
            "Global configuration is unavailable.",
        );
        return;
    }
    app.section_index = app.section_index.min(entries.len() - 1);
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
        if index == app.section_index {
            selected_row = rows.len();
        }
        let line = match entry {
            LibraryEntry::Profile(profile) => {
                let badge = if app.global_active_profile.as_deref() == Some(profile.as_str()) {
                    "active"
                } else {
                    "not active"
                };
                Line::raw(format!("  {} {badge}", pad_right(profile, 16)))
            }
            LibraryEntry::PersonalSection(id) => {
                let (name, path) = app
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
                    fit_column(&personal_section_usage(app, id), usage_width)
                ))
            }
            LibraryEntry::ProjectSection(id) => {
                let (name, path) = app
                    .project
                    .as_ref()
                    .and_then(|project| {
                        project
                            .section(id)
                            .map(|section| (section.name.clone(), section.path.clone()))
                    })
                    .unwrap_or_else(|| (id.clone(), String::new()));
                let usage = app.project.as_ref().map_or_else(
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

fn render_global_profile(frame: &mut Frame<'_>, app: &mut App, screen: Rect, profile: &str) {
    app.scroll = 0;
    app.max_scroll = 0;
    let Some((active, targets)) = app.global.as_ref().and_then(|global| {
        global.manifest.profiles.contains_key(profile).then(|| {
            let active = app.global_active_profile.as_deref() == Some(profile);
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
                        app,
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
            app,
            screen,
            "Global Profile",
            "The Global Profile is unavailable.",
        );
        return;
    };
    app.section_index = app.section_index.min(targets.len().saturating_sub(1));
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
    let mut state = ListState::default().with_selected(Some(app.section_index));
    frame.render_stateful_widget(
        List::new(rows)
            .highlight_style(active_theme().selected())
            .highlight_symbol("▸ "),
        list_area,
        &mut state,
    );
    let difference = if profile_target_reference(app, profile, app.section_index)
        .and_then(|reference| managed_workspace(app, &reference).ok())
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

fn render_context(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let scan_in_progress = app.inspection.context.runtime == ContextRuntime::Claude
        && matches!(
            app.context.scan_status,
            ContextScanStatus::NotStarted | ContextScanStatus::Scanning
        );
    if app.inspection.context.sources.is_empty() && !scan_in_progress {
        app.context_preview = false;
        app.max_scroll = 0;
        frame.render_widget(
            Paragraph::new("No persistent Markdown instructions found for this runtime.")
                .block(
                    Block::default()
                        .title(format!(
                            " {} · resolved load chain ",
                            app.inspection.context.runtime.label()
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
    let (list_area, detail_area) = if app.inspection.context.sources.is_empty() {
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
    app.context_preview = detail_area.is_some();
    let entries = app.context_entries();
    let selected = entries.get(app.context_entry_index).copied();
    let mut selected_row = None;
    let mut items = Vec::new();
    for warning in &app.inspection.context.warnings {
        items.push(ListItem::new(Line::styled(
            format!("  ⚠ {}", relative_message(app, warning)),
            Style::new().fg(active_theme().warning),
        )));
        items.push(ListItem::new(""));
    }
    for group in [
        SourceGroup::Startup,
        SourceGroup::PathFiltered,
        SourceGroup::Nested,
    ] {
        let source_indices = app
            .inspection
            .context
            .sources
            .iter()
            .enumerate()
            .filter(|(_, source)| source.group == group)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if group == SourceGroup::Nested && app.inspection.context.runtime != ContextRuntime::Claude
        {
            continue;
        }
        if source_indices.is_empty() && group == SourceGroup::PathFiltered {
            continue;
        }
        if group == SourceGroup::Nested {
            if source_indices.is_empty() {
                let before = items.len();
                push_scan_errors(&mut items, app);
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
            if !app.context_nested_expanded {
                push_scan_errors(&mut items, app);
                items.push(ListItem::new(""));
                continue;
            }
        } else {
            let count = if group == SourceGroup::Startup {
                source_indices
                    .iter()
                    .filter(|index| loaded_at_startup(&app.inspection.context.sources[**index]))
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
                && loaded_at_startup(&app.inspection.context.sources[index])
            {
                startup_position += 1;
                Some(startup_position)
            } else {
                None
            };
            items.push(context_source_item(
                &app.inspection.context.sources[index],
                position,
                usize::from(list_area.width.saturating_sub(4)),
            ));
        }
        if group == SourceGroup::Nested {
            push_scan_errors(&mut items, app);
        }
        items.push(ListItem::new(""));
    }
    let selected_source = match selected {
        Some(ContextEntry::Source(index)) => app.inspection.context.sources.get(index).cloned(),
        _ => None,
    };
    let mut state = ListState::default().with_selected(selected_row);
    frame.render_stateful_widget(
        List::new(items)
            .block(
                Block::default()
                    .title(
                        if app.inspection.context.runtime == ContextRuntime::Claude
                            && matches!(
                                app.context.scan_status,
                                ContextScanStatus::NotStarted | ContextScanStatus::Scanning
                            )
                        {
                            let scan = app.context.scan_spinner().map_or_else(
                                || "checking subfolders…".into(),
                                |spinner| format!("{spinner} checking subfolders…"),
                            );
                            Line::from(vec![
                                Span::styled(
                                    format!(" {} · ", app.inspection.context.runtime.label()),
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
                                app.inspection.context.runtime.label()
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
            render_source_panel(frame, app, detail_area, &source, false);
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
            app.max_scroll = 0;
        }
    } else {
        app.max_scroll = 0;
    }
}

fn loaded_at_startup(source: &ContextSource) -> bool {
    source.group == SourceGroup::Startup
        && matches!(
            source.state,
            SourceState::Startup | SourceState::Truncated { .. }
        )
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

fn source_list_status(state: &SourceState) -> Option<(&'static str, Style)> {
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

fn source_status(app: &App, source: &ContextSource) -> String {
    match &source.state {
        SourceState::Startup => source.scope.clone(),
        SourceState::Conditional(reason)
        | SourceState::Relevant(reason)
        | SourceState::Nested(reason) => reason.clone(),
        SourceState::Excluded(reason) => {
            format!("excluded · {}", relative_message(app, reason))
        }
        SourceState::Shadowed(path) => format!(
            "skipped · {} takes priority",
            display_path(path, &app.paths, &app.inspection.context.directory)
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

fn source_ownership(app: &App, source: &ContextSource) -> Option<String> {
    if let Some(managed) = &source.managed {
        return Some(format!("global · managed · {}", managed.target));
    }
    if let Some(ownership) = app.project.as_ref().and_then(|project| {
        project.target_names().find_map(|id| {
            (project.target_path(id).ok().as_deref() == Some(source.path.as_path())).then(|| {
                let status = managed_workspace(app, &ManagedRef::Project(id.to_owned()))
                    .map_or("invalid", |view| view.instruction_status.label());
                format!("project · managed · {status}")
            })
        })
    }) {
        return Some(ownership);
    }
    let local_repository = app.local_repository.as_ref()?;
    for target in [local::ManagedTarget::Agents, local::ManagedTarget::Claude] {
        if source.path == local_repository.root.join(target.filename())
            && app.inspection.home_items.iter().any(
                |item| matches!(item, HomeItem::LocalTarget(candidate) if *candidate == target),
            )
        {
            let status = managed_workspace(app, &ManagedRef::Local(target))
                .map_or("invalid", |view| view.instruction_status.label());
            return Some(format!("local · managed · {status}"));
        }
    }
    None
}

fn push_scan_errors(items: &mut Vec<ListItem<'static>>, app: &App) {
    match &app.context.scan_status {
        ContextScanStatus::Complete { unreadable } => {
            for path in unreadable {
                items.push(ListItem::new(Line::styled(
                    format!(
                        "  Could not read {}",
                        display_path(path, &app.paths, &app.inspection.context.directory)
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

fn render_source(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    if let Some(source) = app.selected_source().cloned() {
        render_source_panel(frame, app, area, &source, true);
    } else {
        render_document(frame, app, area, "Source", "", "No source selected.", false);
    }
}

fn render_source_panel(
    frame: &mut Frame<'_>,
    app: &mut App,
    area: Rect,
    source: &ContextSource,
    numbered: bool,
) {
    let reason = source
        .state
        .reason(&app.paths, &app.inspection.context.directory)
        .unwrap_or_else(|| "selected by the runtime".into());
    let link = symlink_info(app, &source.path);
    let ownership = source_ownership(app, source)
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
                Span::raw(symlink_target_status(app, &source.path, info)),
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
            Span::styled(source_status(app, source), source_style(&source.state)),
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
                    .map(|path| display_path(path, &app.paths, &app.inspection.context.directory))
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
        app,
        document_area,
        " Contents ",
        &source.content,
        true,
        numbered,
    );
}

fn render_document(
    frame: &mut Frame<'_>,
    app: &mut App,
    area: Rect,
    title: &str,
    about: &str,
    content: &str,
    numbered: bool,
) {
    if about.is_empty() {
        render_plain_document(
            frame,
            app,
            area,
            &format!(" {title} "),
            content,
            true,
            numbered,
        );
        return;
    }
    let lines = about.lines().count();
    let about_height = u16::try_from(lines)
        .unwrap_or(u16::MAX)
        .saturating_add(2)
        .min(area.height.saturating_sub(4));
    let [about_area, _, content_area] = Layout::vertical([
        Constraint::Length(about_height),
        Constraint::Length(1),
        Constraint::Min(3),
    ])
    .areas(area);
    frame.render_widget(
        Paragraph::new(about)
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
        app,
        content_area,
        " Contents ",
        content,
        true,
        numbered,
    );
}

fn render_plain_document(
    frame: &mut Frame<'_>,
    app: &mut App,
    area: Rect,
    title: &str,
    content: &str,
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
    let lines = content.lines().collect::<Vec<_>>();
    let number_width = lines.len().max(1).to_string().len().max(3);
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
    let highlighted = app
        .highlighted_line
        .and_then(|(line, started)| (started.elapsed() < LINE_HIGHLIGHT_DURATION).then_some(line));
    if app.highlighted_line.is_some() && highlighted.is_none() {
        app.highlighted_line = None;
    }
    let text = Text::from(
        lines
            .iter()
            .enumerate()
            .map(|(index, line)| {
                let line = Line::raw((*line).to_owned());
                if highlighted == Some(index + 1) {
                    line.style(
                        Style::new()
                            .fg(active_theme().background)
                            .bg(active_theme().warning),
                    )
                } else {
                    line
                }
            })
            .collect::<Vec<_>>(),
    );
    let paragraph = Paragraph::new(text).wrap(Wrap { trim: false });
    app.document_width = content_area.width.max(1);
    app.document_height = content_area.height;
    app.max_scroll = scroll_max(
        paragraph.line_count(content_area.width.max(1)),
        content_area.height,
    );
    app.scroll = app.scroll.min(app.max_scroll);
    frame.render_widget(block, area);
    if numbered {
        let mut gutter = Vec::new();
        for (index, line) in lines.iter().enumerate() {
            gutter.push(Line::styled(
                format!("{:>number_width$} │ ", index + 1),
                Style::new().fg(active_theme().muted),
            ));
            let wrapped = Paragraph::new((*line).to_owned())
                .wrap(Wrap { trim: false })
                .line_count(content_area.width.max(1))
                .max(1);
            gutter.extend((1..wrapped).map(|_| {
                Line::styled(
                    format!("{:number_width$} │ ", ""),
                    Style::new().fg(active_theme().muted),
                )
            }));
        }
        frame.render_widget(Paragraph::new(gutter).scroll((app.scroll, 0)), gutter_area);
    }
    frame.render_widget(paragraph.scroll((app.scroll, 0)), content_area);
}

fn render_diff(frame: &mut Frame<'_>, app: &mut App, area: Rect, reference: &ManagedRef) {
    match managed_workspace(app, reference) {
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
                app,
                diff_area,
                " Unified difference ",
                workspace.difference.as_deref().unwrap_or_default(),
                false,
                false,
            );
        }
        Err(error) => render_document(frame, app, area, "Difference", "", &error, false),
    }
}

fn render_footer(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let keys = match &app.view {
        View::Home => {
            let difference = if home_target_reference(app)
                .and_then(|reference| managed_workspace(app, &reference).ok())
                .is_some_and(|workspace| workspace.difference.is_some())
            {
                "   d difference"
            } else {
                ""
            };
            return render_footer_line(
                frame,
                app,
                area,
                &format!(
                    "↑↓ select   enter open{difference}   r runtime   t theme   ? help   q quit"
                ),
            );
        }
        View::Context => {
            let entries = app.context_entries();
            let selected = entries.get(app.context_entry_index);
            if selected.is_none() {
                return render_footer_line(frame, app, area, "←→/r runtime   esc back   ? help");
            }
            let action = if matches!(selected, Some(ContextEntry::Nested)) {
                if app.context_nested_expanded {
                    "hide"
                } else {
                    "show"
                }
            } else {
                "open"
            };
            let preview = if app.context_preview {
                "   ⇧↑/⇧↓ preview"
            } else {
                ""
            };
            return render_footer_line(
                frame,
                app,
                area,
                &format!("↑↓ source{preview}   enter {action}   ←→/r runtime   esc back   ? help"),
            );
        }
        View::Managed(reference) => {
            let preview = if area.width >= 100 {
                "   ⇧↑/⇧↓ preview"
            } else {
                ""
            };
            let difference =
                if managed_workspace(app, reference).is_ok_and(|view| view.difference.is_some()) {
                    "   d difference"
                } else {
                    ""
                };
            return render_footer_line(
                frame,
                app,
                area,
                &format!("↑↓ select{preview}   enter open{difference}   esc back"),
            );
        }
        View::ManagedDocument(reference) => {
            if managed_workspace(app, reference).is_ok_and(|view| view.difference.is_some()) {
                "↑↓ scroll   ⇧↑/⇧↓ page   g line   / search   d difference   esc back"
            } else {
                "↑↓ scroll   ⇧↑/⇧↓ page   g line   / search   esc back"
            }
        }
        View::Diff(_) => "↑↓ scroll   ⇧↑/⇧↓ page   / search   d close difference   esc back",
        View::GlobalLibrary => "↑↓ select   enter inspect   esc back",
        View::GlobalProfile(profile) => {
            let difference = if profile_target_reference(app, profile, app.section_index)
                .and_then(|reference| managed_workspace(app, &reference).ok())
                .is_some_and(|workspace| workspace.difference.is_some())
            {
                "   d difference"
            } else {
                ""
            };
            return render_footer_line(
                frame,
                app,
                area,
                &format!("↑↓ select   enter open composition{difference}   esc back"),
            );
        }
        View::Source => "↑↓ scroll   ⇧↑/⇧↓ page   g line   / search   r runtime   esc back",
        View::File { .. } | View::SectionDocument { .. } => {
            "↑↓ scroll   ⇧↑/⇧↓ page   g line   / search   esc back"
        }
        View::Info { .. } => "↑↓ scroll   ⇧↑/⇧↓ page   esc back",
    };
    render_footer_line(frame, app, area, keys);
}

fn render_footer_line(frame: &mut Frame<'_>, app: &App, area: Rect, keys: &str) {
    let home = matches!(app.view, View::Home);
    let message = if home {
        Line::default()
    } else {
        app.message
            .as_ref()
            .map_or_else(Line::default, |(ok, text)| {
                Line::from(format!(
                    "{} {}",
                    if *ok { "●" } else { "!" },
                    relative_message(app, text)
                ))
            })
    };
    let block = if home {
        Block::default()
    } else {
        Block::default().borders(Borders::TOP)
    };
    frame.render_widget(
        Paragraph::new(Text::from(vec![message, Line::from(keys.to_owned())]))
            .block(block)
            .alignment(Alignment::Center)
            .style(Style::new().fg(active_theme().muted)),
        area,
    );
}

fn render_help(frame: &mut Frame<'_>, app: &App, screen: Rect) {
    let workflow = "HOW CHANGES WORK\nThe TUI does not edit instructions or apply compositions. Ask your coding agent to read `mdmanager docs start` and make those changes through the CLI; this page reloads from disk.\n\nTHEMES\nPress t to preview themes. Enter saves the selected UI theme to mdmanager.toml.";
    let (context, text) = match &app.view {
        View::Home => (
            "Overview".into(),
            format!(
                "WHAT THIS PAGE SHOWS\nThe overview separates what a runtime loads from Project, Local, and Global instruction files. Library contains reusable Profiles and Sections.\n\nHOW MDMANAGER WORKS\nContext explains existing Markdown. Managed views compare composed Markdown with files on disk.\n\n{workflow}"
            ),
        ),
        View::Context => (
            format!("Context · {}", app.inspection.context.runtime.label()),
            format!(
                "WHAT THIS PAGE SHOWS\nContext predicts the persistent Markdown {} loads from this directory.\n\nHOW TO READ IT\nNumbered sources load at startup. Conditional sources load only for matching paths. Excluded or skipped candidates remain visible with their reason.\n\n{workflow}",
                app.inspection.context.runtime.label()
            ),
        ),
        View::Source => (
            "Context source".into(),
            format!(
                "WHAT THIS PAGE SHOWS\nThis is one Markdown source from the selected runtime's Context chain. About describes its path, load state, reason, and ownership; Contents is the file itself.\n\nOWNERSHIP\nObserved files affect Context but may remain outside mdmanager's control.\n\n{workflow}"
            ),
        ),
        View::GlobalLibrary => (
            "Library".into(),
            format!(
                "WHAT THIS PAGE SHOWS\nLibrary contains every Profile, Personal Section, and Section committed with this project, including unused Sections.\n\nHOW TO READ IT\nProfiles compose the catalog targets they name. Personal Sections live under ~/.mdmanager; This Project travels with the repository.\n\n{workflow}"
            ),
        ),
        View::GlobalProfile(profile) => (
            format!("Profile {profile}"),
            format!(
                "WHAT THIS PAGE SHOWS\nA Profile names the catalog targets it deploys. Omitted catalog targets are listed as not in this Profile. It is not filtered by Context Runtime.\n\nHOW TO READ IT\nEach row shows a target, filename, ordered Sections, and how its generated Markdown compares with the file on disk. An inactive Profile is only being inspected.\n\n{workflow}"
            ),
        ),
        View::Managed(_) => (
            "Composition".into(),
            format!(
                "WHAT THIS PAGE SHOWS\nThis target is generated from the ordered Sections on the left. The preview shows the selected Section or generated document.\n\nHOW TO READ IT\nStatus compares generated Markdown with the target on disk. A difference is available only when those contents differ.\n\n{workflow}"
            ),
        ),
        View::ManagedDocument(_) => (
            "Generated document".into(),
            format!(
                "WHAT THIS PAGE SHOWS\nThis is the complete Markdown generated by the target's ordered Sections. It is what mdmanager would write when the agent applies the composition.\n\n{workflow}"
            ),
        ),
        View::Diff(_) => (
            "Difference".into(),
            format!(
                "WHAT THIS PAGE SHOWS\nThis unified difference compares generated Markdown with the target currently on disk. Removed lines belong to the current file; added lines belong to the composition.\n\n{workflow}"
            ),
        ),
        View::SectionDocument { title, .. } => (
            format!("Section · {title}"),
            format!(
                "WHAT THIS PAGE SHOWS\nA Section is reusable source Markdown. About shows where it lives and which Profiles or project compositions use it.\n\n{workflow}"
            ),
        ),
        View::File { title, .. } => (
            title.clone(),
            format!(
                "WHAT THIS PAGE SHOWS\nThis is an instruction file a supported runtime can load. About explains its scope and whether mdmanager manages it; the numbered text is the file on disk.\n\n{workflow}"
            ),
        ),
        View::Info { title, .. } => (
            title.clone(),
            format!(
                "WHAT THIS PAGE SHOWS\nThis page explains the selected item and why it is unavailable or not managed.\n\n{workflow}"
            ),
        ),
    };
    let content_width = screen.width.min(88).saturating_sub(6).max(1);
    let height = u16::try_from(
        Paragraph::new(text.clone())
            .wrap(Wrap { trim: false })
            .line_count(content_width),
    )
    .unwrap_or(u16::MAX)
    .saturating_add(4)
    .min(30);
    let area = bounded_around(screen, help_anchor_area(app, screen), 88, height);
    let block = Block::default()
        .title(format!(" mdmanager.ai · Help · {context} "))
        .title_style(
            Style::new()
                .fg(active_theme().primary)
                .add_modifier(Modifier::BOLD),
        )
        .border_style(Style::new().fg(active_theme().primary))
        .style(
            Style::new()
                .fg(active_theme().text)
                .bg(active_theme().surface_raised)
                .remove_modifier(Modifier::DIM),
        )
        .borders(Borders::ALL);
    let inner = block.inner(area);
    let [content_area, controls_area] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).areas(inner);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    let text = Text::from(
        text.lines()
            .map(|line| match line {
                "WHAT THIS PAGE SHOWS"
                | "HOW TO READ IT"
                | "HOW MDMANAGER WORKS"
                | "HOW CHANGES WORK"
                | "OWNERSHIP" => Line::styled(
                    line.to_owned(),
                    Style::new()
                        .fg(active_theme().secondary)
                        .add_modifier(Modifier::BOLD),
                ),
                _ => Line::raw(line.to_owned()),
            })
            .collect::<Vec<_>>(),
    );
    frame.render_widget(
        Paragraph::new(text).wrap(Wrap { trim: false }),
        content_area,
    );
    frame.render_widget(
        Paragraph::new("Esc or ? close")
            .block(Block::default().borders(Borders::TOP))
            .alignment(Alignment::Center)
            .style(Style::new().fg(active_theme().muted)),
        controls_area,
    );
}

fn render_info(frame: &mut Frame<'_>, app: &mut App, screen: Rect, title: &str, text: &str) {
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
    app.max_scroll = wrapped_lines.saturating_sub(text_area.height);
    app.scroll = app.scroll.min(app.max_scroll);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .scroll((app.scroll, 0)),
        text_area,
    );
    let controls = if app.max_scroll > 0 {
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

fn render_picker(frame: &mut Frame<'_>, app: &App, screen: Rect) {
    let picker = app.picker.as_ref().unwrap();
    let filtered = filtered_runtimes(picker);
    let mut rows = Vec::new();
    let mut selected_row = None;
    for index in filtered {
        if index == picker.selected {
            selected_row = Some(rows.len());
        }
        let runtime = ContextRuntime::ALL[index];
        rows.push(ListItem::new(format!("  {}", runtime.label())));
    }
    if rows.is_empty() {
        rows.push(ListItem::new("  No matching runtime"));
    }
    let height = u16::try_from(rows.len().min(12))
        .unwrap_or(12)
        .saturating_add(8);
    let area = bounded(screen, 72, height);
    let block = Block::default()
        .title(" mdmanager.ai · Context Runtime ")
        .title_style(
            Style::new()
                .fg(active_theme().primary)
                .add_modifier(Modifier::BOLD),
        )
        .border_style(Style::new().fg(active_theme().primary))
        .style(Style::new().bg(active_theme().surface_raised))
        .borders(Borders::ALL);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [context_area, search_area, list_area, controls_area] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(2),
    ])
    .areas(inner);
    let context = app
        .inspection
        .context
        .directory
        .strip_prefix(&app.paths.home)
        .map_or_else(
            |_| app.inspection.context.directory.display().to_string(),
            |relative| format!("~/{}", relative.display()),
        );
    let context = truncate_start(
        &context,
        usize::from(context_area.width).saturating_sub("Context for ".len()),
    );
    frame.render_widget(
        Paragraph::new(format!("Context for {context}"))
            .style(Style::new().fg(active_theme().muted)),
        context_area,
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Search: ", Style::new().fg(active_theme().muted)),
            Span::raw(if picker.query.is_empty() {
                "_".into()
            } else {
                format!("{}_", picker.query)
            }),
        ])),
        search_area,
    );
    let mut state = ListState::default().with_selected(selected_row);
    frame.render_stateful_widget(
        List::new(rows)
            .highlight_style(active_theme().selected())
            .highlight_symbol("▸ "),
        list_area,
        &mut state,
    );
    frame.render_widget(
        Paragraph::new("type to filter · ↑↓ select · Enter choose · Esc cancel")
            .block(Block::default().borders(Borders::TOP))
            .alignment(Alignment::Center)
            .style(Style::new().fg(active_theme().muted)),
        controls_area,
    );
}

fn render_theme_picker(frame: &mut Frame<'_>, app: &App, screen: Rect) {
    let picker = app.theme_picker.as_ref().unwrap();
    let filtered = filtered_themes(picker);
    let configured = app
        .global
        .as_ref()
        .map_or(theme::DEFAULT_NAME, |global| global.theme());
    let mut rows = Vec::new();
    let mut selected_row = None;
    for index in filtered {
        if index == picker.selected {
            selected_row = Some(rows.len());
        }
        let name = theme::names().nth(index).unwrap_or("unknown");
        let badge = if configured == name {
            "  configured"
        } else {
            ""
        };
        rows.push(ListItem::new(format!("  {name}{badge}")));
    }
    if rows.is_empty() {
        rows.push(ListItem::new("  No matching theme"));
    }
    let height = u16::try_from(rows.len().min(16))
        .unwrap_or(16)
        .saturating_add(10)
        .min(30);
    let area = bounded(screen, 72, height);
    let block = Block::default()
        .title(" mdmanager.ai · Theme ")
        .title_style(
            Style::new()
                .fg(active_theme().primary)
                .add_modifier(Modifier::BOLD),
        )
        .border_style(Style::new().fg(active_theme().primary))
        .style(Style::new().bg(active_theme().surface_raised))
        .borders(Borders::ALL);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [
        intro_area,
        search_area,
        list_area,
        value_area,
        controls_area,
    ] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(2),
        Constraint::Length(2),
    ])
    .areas(inner);
    frame.render_widget(
        Paragraph::new("Live preview · Enter saves to mdmanager.toml")
            .style(Style::new().fg(active_theme().muted)),
        intro_area,
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Search: ", Style::new().fg(active_theme().muted)),
            Span::raw(if picker.query.is_empty() {
                "_".into()
            } else {
                format!("{}_", picker.query)
            }),
        ])),
        search_area,
    );
    let mut state = ListState::default().with_selected(selected_row);
    frame.render_stateful_widget(
        List::new(rows)
            .highlight_style(active_theme().selected())
            .highlight_symbol("▸ "),
        list_area,
        &mut state,
    );
    let selected = theme::names().nth(picker.selected).unwrap_or("");
    frame.render_widget(
        Paragraph::new(format!("[ui] theme = \"{selected}\""))
            .alignment(Alignment::Center)
            .style(Style::new().fg(active_theme().secondary)),
        value_area,
    );
    frame.render_widget(
        Paragraph::new("type to filter · ↑↓ preview · Enter use · Esc cancel")
            .block(Block::default().borders(Borders::TOP))
            .alignment(Alignment::Center)
            .style(Style::new().fg(active_theme().muted)),
        controls_area,
    );
}

fn render_search(frame: &mut Frame<'_>, app: &App) {
    let query = &app.search.as_ref().unwrap().query;
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

fn render_line_prompt(frame: &mut Frame<'_>, app: &App) {
    let input = &app.line_prompt.as_ref().unwrap().input;
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

fn scroll_max(line_count: usize, height: u16) -> u16 {
    u16::try_from(line_count)
        .unwrap_or(u16::MAX)
        .saturating_sub(height)
}

fn pad_right(value: &str, width: usize) -> String {
    let padding = width.saturating_sub(UnicodeWidthStr::width(value));
    format!("{value}{}", " ".repeat(padding))
}

fn fit_column(value: &str, width: usize) -> String {
    pad_right(&truncate_end(value, width), width)
}

fn truncate_start(value: &str, width: usize) -> String {
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

fn truncate_end(value: &str, width: usize) -> String {
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

fn relative_message(app: &App, message: &str) -> String {
    message.replace(
        &app.project_root.display().to_string(),
        app.project_root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("."),
    )
}

fn bounded(area: Rect, max_width: u16, max_height: u16) -> Rect {
    let [vertical] = Layout::vertical([Constraint::Length(area.height.min(max_height))])
        .flex(Flex::Center)
        .areas(area);
    let [centered] = Layout::horizontal([Constraint::Length(area.width.min(max_width))])
        .flex(Flex::Center)
        .areas(vertical);
    centered
}

fn bounded_around(screen: Rect, anchor: Rect, max_width: u16, max_height: u16) -> Rect {
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

fn library_panel_area(screen: Rect, row_count: usize) -> Rect {
    let height = u16::try_from(row_count)
        .unwrap_or(u16::MAX)
        .saturating_add(4)
        .clamp(8, 30)
        .min(screen.height.saturating_sub(4).max(8));
    bounded(screen, COMPACT_WIDTH, height)
}

fn profile_panel_area(screen: Rect, row_count: usize) -> Rect {
    let height = u16::try_from(row_count)
        .unwrap_or(u16::MAX)
        .saturating_add(6)
        .clamp(9, 30)
        .min(screen.height.saturating_sub(4).max(9));
    bounded(screen, COMPACT_WIDTH, height)
}

fn info_panel_geometry(screen: Rect, text: &str) -> (Rect, u16) {
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

fn help_anchor_area(app: &App, screen: Rect) -> Rect {
    match &app.view {
        View::Home => canvas_area(screen, app),
        View::Info { text, .. } => info_panel_geometry(screen, text).0,
        View::GlobalLibrary => {
            let entries = app.library_entries();
            let groups = usize::from(
                entries
                    .iter()
                    .any(|entry| matches!(entry, LibraryEntry::Profile(_))),
            ) + usize::from(
                entries
                    .iter()
                    .any(|entry| matches!(entry, LibraryEntry::PersonalSection(_))),
            ) + usize::from(
                entries
                    .iter()
                    .any(|entry| matches!(entry, LibraryEntry::ProjectSection(_))),
            );
            let rows = entries
                .len()
                .saturating_add(groups.saturating_mul(2).saturating_sub(1));
            library_panel_area(screen, rows)
        }
        View::GlobalProfile(_) => {
            let rows = app
                .global
                .as_ref()
                .map_or(0, |global| global.target_names().count());
            profile_panel_area(screen, rows)
        }
        _ => screen,
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use tempfile::TempDir;

    use super::*;

    fn fixture() -> (TempDir, TempDir, Paths, App) {
        let home = TempDir::new().unwrap();
        let repository = TempDir::new().unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .current_dir(repository.path())
                .status()
                .unwrap()
                .success()
        );
        fs::write(repository.path().join("AGENTS.md"), "# Existing\n").unwrap();
        let paths = Paths::for_home(home.path());
        let app = App::new_at(paths.clone(), None, repository.path().to_owned()).unwrap();
        (home, repository, paths, app)
    }

    fn draw(app: &mut App) -> String {
        draw_at(app, 100, 36)
    }

    fn draw_at(app: &mut App, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, app)).unwrap();
        terminal.backend().to_string()
    }

    fn finish_context_scan(app: &mut App) {
        let deadline = Instant::now() + Duration::from_secs(1);
        while app.context.is_scanning() && Instant::now() < deadline {
            app.poll_context_scan();
            std::thread::yield_now();
        }
        assert!(!app.context.is_scanning());
    }

    #[test]
    fn intro_phase_fades_in_holds_and_recedes() {
        assert_eq!(
            intro_phase(Duration::ZERO),
            Some((active_theme().dim, None))
        );
        assert_eq!(
            intro_phase(Duration::from_millis(400)),
            Some((active_theme().muted, None))
        );
        assert_eq!(
            intro_phase(Duration::from_millis(900)),
            Some((active_theme().primary, Some(active_theme().text)))
        );
        assert_eq!(
            intro_phase(Duration::from_millis(1700)),
            Some((active_theme().dim, Some(active_theme().dim)))
        );
        assert_eq!(intro_phase(INTRO_DURATION), None);
    }

    #[test]
    fn intro_covers_the_page_and_any_key_skips_without_acting() {
        let (_home, _repository, _paths, mut app) = fixture();
        app.intro_started = Some(Instant::now());
        let screen = draw(&mut app);
        assert!(screen.contains("███╗"));
        assert!(!screen.contains("PROJECT INSTRUCTIONS"));

        handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert!(app.intro_started.is_none());
        assert_eq!(app.home_index, 0);
        assert!(matches!(app.view, View::Home));
        assert!(!draw(&mut app).contains("███╗"));

        app.reload_from_disk();
        assert!(app.intro_started.is_none());
    }

    #[test]
    fn intro_expires_on_its_own_and_quit_keys_still_work() {
        let (_home, _repository, _paths, mut app) = fixture();
        app.intro_started = Some(Instant::now().checked_sub(INTRO_DURATION).unwrap());
        assert!(!draw(&mut app).contains("███╗"));
        app.poll_intro();
        assert!(app.intro_started.is_none());

        app.intro_started = Some(Instant::now());
        assert!(
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)
            ) == SessionAction::Quit
        );
        assert!(
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
            ) == SessionAction::Quit
        );
    }

    #[test]
    fn home_keeps_the_existing_groups_and_is_read_only() {
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
        assert!(!screen.contains("Enter create"));
        assert!(!screen.contains("Enter adopt"));
        assert!(!screen.contains("apply"));
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
    fn missing_local_details_explain_runtime_behavior() {
        assert!(missing_local_text(local::ManagedTarget::Agents).contains("instead of AGENTS.md"));
        assert!(
            missing_local_text(local::ManagedTarget::Claude)
                .contains("after CLAUDE.md as additive")
        );
    }

    #[test]
    fn runtime_picker_filters_and_selects() {
        let (_home, _repository, _paths, mut app) = fixture();
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE),
        );
        assert!(app.picker.is_some());
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE),
        );
        let picker = draw(&mut app);
        assert!(picker.contains("mdmanager.ai · Context Runtime"));
        assert!(picker.contains("Pi"));
        assert!(!picker.contains("Codex"));
        assert!(!picker.contains("PROJECT INSTRUCTIONS"));
        assert!(!picker.contains("GLOBAL INSTRUCTIONS"));
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.context.audit.runtime, ContextRuntime::Pi);
        assert!(app.picker.is_none());
        let settings = app.paths.home.join(".codex/config.toml");
        fs::create_dir_all(settings.parent().unwrap()).unwrap();
        fs::write(&settings, "invalid = [").unwrap();
        app.select_runtime(
            ContextRuntime::ALL
                .iter()
                .position(|runtime| *runtime == ContextRuntime::Codex)
                .unwrap(),
        );
        assert!(!app.inspection.context.warnings.is_empty());
        app.inspection.context.summary = "Summary without diagnostic text".into();
        app.inspection.context.warnings = vec!["diagnostic-data-marker".into()];
        app.open(View::Context);
        assert!(draw(&mut app).contains("diagnostic-data-marker"));
    }

    #[test]
    fn help_overlays_a_muted_page_with_contextual_content() {
        let (_home, _repository, _paths, mut app) = fixture();
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE),
        );
        let backend = TestBackend::new(160, 50);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let help = terminal.backend().to_string();
        assert!(help.contains("mdmanager.ai · Help · Overview"));
        assert!(help.contains("WHAT THIS PAGE SHOWS"));
        assert!(help.contains("HOW MDMANAGER WORKS"));
        assert!(!help.contains("NAVIGATION"));

        assert_eq!(
            terminal.backend().buffer().cell((0, 0)).unwrap().fg,
            active_theme().muted
        );
        let (row, line) = help
            .lines()
            .enumerate()
            .find(|(_, line)| line.contains("The TUI does not edit"))
            .unwrap();
        let column = UnicodeWidthStr::width(&line[..line.find("The TUI").unwrap()]);
        let body = terminal
            .backend()
            .buffer()
            .cell((u16::try_from(column).unwrap(), u16::try_from(row).unwrap()))
            .unwrap();
        assert_eq!(body.fg, active_theme().text);
        assert!(!body.modifier.contains(Modifier::DIM));

        handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(draw_at(&mut app, 160, 50).contains("PROJECT INSTRUCTIONS"));
    }

    #[test]
    fn help_anchor_tracks_the_page_and_stays_inside_every_terminal_size() {
        let (_home, _repository, _paths, app) = fixture();
        for (width, height) in [(54, 16), (80, 24), (160, 50), (240, 70)] {
            let screen = Rect::new(0, 0, width, height);
            let anchor = help_anchor_area(&app, screen);
            let modal = bounded_around(screen, anchor, 88, 18);

            assert!(modal.x >= screen.x);
            assert!(modal.y >= screen.y);
            assert!(modal.x + modal.width <= screen.x + screen.width);
            assert!(modal.y + modal.height <= screen.y + screen.height);
            assert!(
                modal
                    .x
                    .saturating_add(modal.width / 2)
                    .abs_diff(anchor.x.saturating_add(anchor.width / 2))
                    <= 1
            );
            assert!(
                modal
                    .y
                    .saturating_add(modal.height / 2)
                    .abs_diff(anchor.y.saturating_add(anchor.height / 2))
                    <= 1
            );
        }
    }

    #[test]
    fn profile_help_explains_the_cross_runtime_composition() {
        let (_home, _repository, _paths, mut app) = global_fixture();
        open_library(&mut app);
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE),
        );

        let help = draw_at(&mut app, 160, 50);
        assert!(help.contains("Help · Profile default"));
        assert!(help.contains("filtered"));
        assert!(help.contains("Context Runtime"));
        assert!(help.contains("An inactive Profile is only being inspected"));
    }

    #[test]
    fn short_info_replaces_the_screen_with_a_content_sized_panel() {
        let (_home, _repository, _paths, mut app) = fixture();
        app.open(View::Info {
            kind: DiagnosticKind::General,
            title: "Pi · not applied yet".into(),
            text: no_active_profile_text(),
        });

        let screen = draw_at(&mut app, 160, 50);
        let title = screen
            .lines()
            .find(|line| line.contains("mdmanager.ai · Pi · not applied yet"))
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
        assert!(right - left <= 88);
        assert!(screen.contains("Esc back"));
        assert!(!screen.contains("↑↓ scroll"));
        assert!(!screen.contains("PROJECT INSTRUCTIONS"));
        assert!(!screen.contains("GLOBAL INSTRUCTIONS"));
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
        assert!(app.max_scroll > 0);
        assert!(screen.contains("↑↓ scroll"));
        handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.scroll, 1);
    }

    #[test]
    fn context_opens_one_runtime_and_returns_home() {
        let (_home, _repository, _paths, mut app) = fixture();
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(app.view, View::Context));
        finish_context_scan(&mut app);
        let screen = draw(&mut app);
        assert!(screen.contains("resolved load chain"));
        handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(app.view, View::Home));
    }

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
        assert!(!app.context_entries().contains(&ContextEntry::Nested));
        assert!(draw(&mut app).contains("checking subfolders…"));
        finish_context_scan(&mut app);
        let screen = draw(&mut app);
        assert!(screen.contains("SUBFOLDER INSTRUCTIONS · 1 file"));
        app.context_entry_index = app
            .context_entries()
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
        assert!(stacked.contains("Claude can load this"));

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
    fn context_hides_an_empty_subfolder_group() {
        let (_home, _repository, _paths, mut app) = fixture();
        app.open(View::Context);
        finish_context_scan(&mut app);

        let screen = draw(&mut app);

        assert!(!screen.contains("SUBFOLDER INSTRUCTIONS"));
        assert!(!screen.contains("Enter show"));
    }

    #[test]
    fn malformed_local_manifest_is_one_invalid_home_item() {
        let (_home, _repository, _paths, mut app) = fixture();
        let local_repository = app.local_repository.as_ref().unwrap();
        fs::create_dir_all(&local_repository.data_dir).unwrap();
        fs::write(
            local_repository.data_dir.join("local.toml"),
            "format = 1\nordered = [\"local\"]\n",
        )
        .unwrap();
        app.reload_from_disk();

        let local = app
            .home_items()
            .into_iter()
            .filter(|item| home_group(item) == "LOCAL")
            .collect::<Vec<_>>();
        assert!(matches!(local.as_slice(), [HomeItem::LocalInvalid(_)]));
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

    #[test]
    fn managed_project_is_browsable_but_has_no_mutation_keys() {
        let (_home, repository, paths, _app) = fixture();
        project::adopt(repository.path(), "agents").unwrap();
        let mut app = App::new_at(paths, None, repository.path().to_owned()).unwrap();
        app.open(View::Managed(ManagedRef::Project("agents".into())));
        let screen = draw(&mut app);
        assert!(screen.contains("Generated document"));
        assert!(screen.contains("Composition"));
        assert!(!screen.contains("save"));
        assert!(!screen.contains("apply"));
        assert!(!screen.contains("toggle"));
    }

    #[test]
    fn symlink_destination_preserves_link_ownership_and_finds_managed_target() {
        use std::os::unix::fs::symlink;

        let (_home, repository, paths, _app) = fixture();
        project::adopt(repository.path(), "agents").unwrap();
        let link = repository.path().join("CLAUDE.md");
        symlink("AGENTS.md", &link).unwrap();
        let app = App::new_at(paths, None, repository.path().to_owned()).unwrap();

        let info = symlink_info(&app, &link).unwrap();
        assert_eq!(info.target, Path::new("AGENTS.md"));
        let SymlinkResolution::Resolved(resolved) = info.resolution else {
            panic!("managed target should resolve");
        };
        let destination = managed_destination(&app, &resolved).unwrap();
        assert_eq!(destination.scope, "Project");
        assert_eq!(destination.target, "AGENTS.md");
        assert!(
            destination
                .status
                .is_some_and(InstructionStatus::is_current)
        );
    }

    #[test]
    fn broken_symlink_is_not_mistaken_for_an_unmanaged_file() {
        use std::os::unix::fs::symlink;

        let (_home, repository, _paths, mut app) = fixture();
        let link = repository.path().join("CLAUDE.md");
        symlink("missing.md", &link).unwrap();
        app.refresh_inspection();

        let info = symlink_info(&app, &link).unwrap();
        assert_eq!(info.target, Path::new("missing.md"));
        assert!(matches!(info.resolution, SymlinkResolution::Missing));
    }

    #[test]
    fn managed_difference_is_full_width_and_read_only() {
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
        assert!(screen.contains("Ask your coding agent"));
        assert!(!screen.contains("confirm"));
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
        );
        assert!(matches!(app.view, View::Managed(_)));
    }

    #[test]
    fn home_opens_the_selected_managed_target_difference() {
        let (_home, repository, paths, _app) = fixture();
        let project = project::adopt(repository.path(), "agents").unwrap();
        fs::write(
            project.section_path("agents").unwrap(),
            "# Changed by agent\n",
        )
        .unwrap();
        let mut app = App::new_at(paths, None, repository.path().to_owned()).unwrap();
        app.home_index = app
            .home_items()
            .iter()
            .position(|item| matches!(item, HomeItem::ProjectTarget(id) if id == "agents"))
            .unwrap();

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
        );

        assert!(matches!(
            app.view,
            View::Diff(ManagedRef::Project(ref id)) if id == "agents"
        ));
    }

    #[test]
    fn reload_closes_a_difference_after_the_target_becomes_current() {
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

        let workspace = app.project.as_ref().unwrap();
        workspace
            .apply(&workspace.inspect("agents").unwrap())
            .unwrap();
        app.reload_from_disk();

        assert!(matches!(app.view, View::Managed(_)));
        assert!(
            app.message
                .as_ref()
                .unwrap()
                .1
                .contains("Updated from disk")
        );
    }

    #[test]
    fn instruction_snapshot_keeps_rendering_and_search_stable_until_reload() {
        let (_home, repository, paths, _app) = fixture();
        let project = project::adopt(repository.path(), "agents").unwrap();
        let section = project.section_path("agents").unwrap();
        let target = project.target_path("agents").unwrap();
        let reference = ManagedRef::Project("agents".into());
        let mut app = App::new_at(paths, None, repository.path().to_owned()).unwrap();
        finish_context_scan(&mut app);
        let views = [
            View::Home,
            View::Managed(reference.clone()),
            View::ManagedDocument(reference.clone()),
            View::SectionDocument {
                title: "Section".into(),
                path: section.clone(),
                about: String::new(),
            },
            View::File {
                title: "Target".into(),
                path: target.clone(),
                about: "Project file".into(),
            },
        ];
        let before = views
            .iter()
            .map(|view| {
                app.view = view.clone();
                (draw(&mut app), app.searchable_text())
            })
            .collect::<Vec<_>>();
        fs::write(&section, "# New snapshot content\n").unwrap();
        fs::remove_file(&target).unwrap();
        for (view, (screen, search)) in views.iter().zip(&before) {
            app.view = view.clone();
            assert_eq!(&draw(&mut app), screen);
            assert_eq!(&app.searchable_text(), search);
        }

        app.reload_from_disk();
        assert!(app.inspection.document(&target).is_err());
        app.view = View::ManagedDocument(reference.clone());
        assert_eq!(
            app.searchable_text().as_deref(),
            Some("# New snapshot content\n")
        );
        assert!(
            managed_workspace(&app, &reference)
                .unwrap()
                .difference
                .is_some()
        );

        fs::remove_file(&section).unwrap();
        app.reload_from_disk();
        assert!(managed_workspace(&app, &reference).is_err());
        assert!(app.searchable_text().is_none());
        assert!(!draw(&mut app).contains("# New snapshot content"));
        app.view = View::SectionDocument {
            title: "Section".into(),
            path: section.clone(),
            about: String::new(),
        };
        assert!(app.inspection.document(&section).is_err());
        assert!(app.searchable_text().is_none());
        assert!(!draw(&mut app).contains("# New snapshot content"));
    }

    #[test]
    fn equal_unmanaged_global_content_has_no_difference() {
        let (_home, _repository, _paths, mut app) = global_fixture();
        let global = app.global.as_ref().unwrap();
        let target = global.target_path("claude").unwrap();
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, global.render("default", "claude").unwrap()).unwrap();
        app.reload_from_disk();
        let reference = ManagedRef::Global {
            profile: "default".into(),
            target: "claude".into(),
        };
        let workspace = managed_workspace(&app, &reference).unwrap();
        assert!(
            workspace.instruction_status
                == InstructionStatus::Global(GlobalTargetStatus::Unmanaged)
        );
        assert!(workspace.difference.is_none());
        app.open(View::Managed(reference.clone()));
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
        );
        assert!(matches!(app.view, View::Managed(_)));

        fs::write(&target, "# Different\n").unwrap();
        app.reload_from_disk();
        // Changing presentation cannot change difference availability.
        for (_, workspace) in &mut app.inspection.managed {
            if let Ok(workspace) = workspace {
                workspace.status = "No changes.".into();
            }
        }
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
        );
        assert!(matches!(app.view, View::Diff(_)));
        fs::write(
            &target,
            app.global
                .as_ref()
                .unwrap()
                .render("default", "claude")
                .unwrap(),
        )
        .unwrap();
        app.reload_from_disk();
        assert!(matches!(app.view, View::Managed(_)));
    }

    #[test]
    fn reload_message_expires() {
        let (_home, _repository, _paths, mut app) = fixture();
        app.message = Some((true, "Updated from disk".into()));
        app.message_expires_at = Some(Instant::now());

        app.poll_message();

        assert!(app.message.is_none());
    }

    #[test]
    fn managed_document_diff_back_navigation_returns_to_home() {
        let (_home, repository, paths, _app) = fixture();
        let project = project::adopt(repository.path(), "agents").unwrap();
        fs::write(
            project.section_path("agents").unwrap(),
            "# Changed by agent\n",
        )
        .unwrap();
        let mut app = App::new_at(paths, None, repository.path().to_owned()).unwrap();
        app.open(View::Managed(ManagedRef::Project("agents".into())));

        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
        );
        assert!(matches!(app.view, View::Diff(_)));
        handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(app.view, View::ManagedDocument(_)));
        handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(app.view, View::Managed(_)));
        handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(app.view, View::Home));
    }

    #[test]
    fn reload_preserves_the_selected_context_path() {
        let (_home, repository, _paths, mut app) = fixture();
        app.select_runtime(
            ContextRuntime::ALL
                .iter()
                .position(|runtime| *runtime == ContextRuntime::Pi)
                .unwrap(),
        );
        app.open(View::Context);
        let selected = app.context.audit.sources[0].path.clone();
        app.context_entry_index = app
            .context_entries()
            .iter()
            .position(|entry| matches!(entry, ContextEntry::Source(0)))
            .unwrap();
        fs::write(repository.path().join("AGENTS.md"), "# Updated\n").unwrap();
        app.reload_from_disk();
        let entry = app.context_entries()[app.context_entry_index];
        assert!(matches!(
            entry,
            ContextEntry::Source(index) if app.context.audit.sources[index].path == selected
        ));
        assert!(
            app.message
                .as_ref()
                .unwrap()
                .1
                .contains("Updated from disk")
        );
    }

    #[test]
    fn watcher_ignores_runtime_session_churn() {
        let (home, _repository, _paths, app) = fixture();
        let initial = app.current_watch_signature();
        let transcript = home
            .path()
            .join(".claude/projects/session/transcript.jsonl");
        fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        fs::write(transcript, "session output\n").unwrap();
        assert_eq!(app.current_watch_signature(), initial);

        let instruction = home.path().join(".claude/CLAUDE.md");
        fs::write(instruction, "# Global instructions\n").unwrap();
        assert_ne!(app.current_watch_signature(), initial);
    }

    #[test]
    fn watcher_tracks_instruction_files_above_the_repository() {
        let home = TempDir::new().unwrap();
        let workspace = TempDir::new().unwrap();
        let repository = workspace.path().join("repo");
        let directory = repository.join("child");
        fs::create_dir_all(&directory).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .current_dir(&repository)
                .status()
                .unwrap()
                .success()
        );
        let ancestor = workspace.path().join("CLAUDE.md");
        fs::write(&ancestor, "# Before\n").unwrap();
        let app = App::new_at(Paths::for_home(home.path()), None, directory).unwrap();
        let initial = app.current_watch_signature();
        fs::write(ancestor, "# After\n").unwrap();
        assert_ne!(app.current_watch_signature(), initial);
    }

    #[test]
    fn watcher_discovers_new_cursor_rules() {
        let home = TempDir::new().unwrap();
        let repository = TempDir::new().unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .current_dir(repository.path())
                .status()
                .unwrap()
                .success()
        );
        fs::create_dir_all(repository.path().join(".cursor/rules")).unwrap();
        let mut app = App::new_at(
            Paths::for_home(home.path()),
            None,
            repository.path().to_owned(),
        )
        .unwrap();
        app.select_runtime(
            ContextRuntime::ALL
                .iter()
                .position(|runtime| *runtime == ContextRuntime::Cursor)
                .unwrap(),
        );
        let initial = app.current_watch_signature();
        let rule = repository.path().join(".cursor/rules/new.mdc");

        fs::write(&rule, "---\nalwaysApply: true\n---\nNew rule\n").unwrap();

        assert_ne!(app.current_watch_signature(), initial);
        app.reload_from_disk();
        assert!(
            app.context
                .audit
                .sources
                .iter()
                .any(|source| source.path == rule)
        );
    }

    #[test]
    fn non_git_watcher_tracks_candidates_without_scanning_the_directory_tree() {
        let home = TempDir::new().unwrap();
        let directory = home.path().join("workspace");
        fs::create_dir_all(directory.join("cache/nested")).unwrap();
        let app = App::new_at(Paths::for_home(home.path()), None, directory.clone()).unwrap();
        assert!(app.watch_root.is_none());
        let initial = app.current_watch_signature();

        fs::write(directory.join("cache/nested/unrelated.txt"), "changed\n").unwrap();
        assert_eq!(app.current_watch_signature(), initial);

        fs::write(directory.join("AGENTS.md"), "# Instructions\n").unwrap();
        assert_ne!(app.current_watch_signature(), initial);
    }

    #[test]
    fn stale_active_profile_is_shown_instead_of_blocking_the_tui() {
        let home = TempDir::new().unwrap();
        let repository = TempDir::new().unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .current_dir(repository.path())
                .status()
                .unwrap()
                .success()
        );
        let paths = Paths::for_home(home.path());
        fs::create_dir_all(paths.config.parent().unwrap().join("sections")).unwrap();
        fs::write(
            &paths.config,
            include_str!("../../tests/fixtures/home/.mdmanager/mdmanager.toml"),
        )
        .unwrap();
        for (name, contents) in [
            ("common.md", "# Common\n"),
            ("claude.md", "# Claude\n"),
            ("web.md", "# Web\n"),
        ] {
            fs::write(
                paths.config.parent().unwrap().join("sections").join(name),
                contents,
            )
            .unwrap();
        }
        fs::create_dir_all(&paths.state_dir).unwrap();
        fs::write(
            paths.state_dir.join("state.toml"),
            "active_profile = \"removed\"\n",
        )
        .unwrap();
        let global = GlobalConfig::load(&paths).unwrap();
        let mut app = App::new_at(paths, Some(global), repository.path().to_owned()).unwrap();

        assert!(app.global_error.as_ref().unwrap().contains("removed"));
        assert!(
            app.global_error
                .as_ref()
                .unwrap()
                .contains("mdmanager doctor")
        );
        let screen = draw(&mut app);
        assert!(screen.contains("Deployment state needs attention"));
        assert!(screen.contains("mdmanager doctor"));

        assert!(app.back() == SessionAction::Continue);
        let home = draw(&mut app);
        assert!(home.contains("Deployment state needs attention"));
        assert!(home.contains("Claude"));
        assert!(home.contains("Browse Profiles & Sections"));

        app.home_index = app
            .home_items()
            .iter()
            .position(|item| matches!(item, HomeItem::GlobalInvalid))
            .unwrap();
        enter_home(&mut app);
        if let View::Info { kind, title, .. } = &mut app.view {
            assert!(*kind == DiagnosticKind::Global);
            *title = "Renamed diagnostic".into();
        }
        fs::write(
            app.paths.state_dir.join("state.toml"),
            "active_profile = \"default\"\n",
        )
        .unwrap();
        app.reload_from_disk();
        assert!(app.global_error.is_none());
        assert!(matches!(app.view, View::Home));
        assert!(draw(&mut app).contains("PROFILE default (active)"));
    }

    #[test]
    fn global_diagnostics_point_to_doctor() {
        let parse_error = global_diagnostic_text("invalid manifest at line 2");
        assert!(parse_error.contains("invalid manifest at line 2"));
        assert!(parse_error.contains("run `mdmanager doctor`"));

        let directed = global_diagnostic_text("run `mdmanager doctor`");
        assert_eq!(directed.matches("mdmanager doctor").count(), 1);
    }

    #[test]
    fn document_search_moves_to_the_matching_line() {
        let (_home, repository, _paths, mut app) = fixture();
        fs::write(
            repository.path().join("AGENTS.md"),
            format!("{}needle\n", "line\n".repeat(40)),
        )
        .unwrap();
        app.open(View::File {
            title: "AGENTS.md".into(),
            path: repository.path().join("AGENTS.md"),
            about: "Project file".into(),
        });
        draw_at(&mut app, 80, 20);
        app.apply_search("needle".into());
        assert!(app.scroll > 0);
        assert!(app.message.as_ref().unwrap().1.contains("Found"));
    }

    #[test]
    fn document_search_accounts_for_wrapped_lines() {
        let (_home, repository, _paths, mut app) = fixture();
        fs::write(
            repository.path().join("AGENTS.md"),
            format!("{}\nneedle\n", "wide ".repeat(300)),
        )
        .unwrap();
        app.open(View::File {
            title: "AGENTS.md".into(),
            path: repository.path().join("AGENTS.md"),
            about: "Project file".into(),
        });
        draw_at(&mut app, 80, 20);
        app.apply_search("needle".into());
        assert!(app.scroll >= 5);
    }

    #[test]
    fn full_documents_are_left_anchored_with_dim_line_numbers() {
        let (_home, repository, _paths, mut app) = fixture();
        app.open(View::File {
            title: "AGENTS.md".into(),
            path: repository.path().join("AGENTS.md"),
            about: "Project file".into(),
        });
        let backend = TestBackend::new(180, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let screen = terminal.backend().to_string();
        let (row, line) = screen
            .lines()
            .enumerate()
            .find(|(_, line)| line.contains("# Existing"))
            .unwrap();
        assert!(line.contains("  1 │ # Existing"));
        let number = UnicodeWidthStr::width(&line[..line.find('1').unwrap()]);
        let content = UnicodeWidthStr::width(&line[..line.find("# Existing").unwrap()]);
        assert!(content < 12, "{line:?}");
        let buffer = terminal.backend().buffer();
        assert_eq!(
            buffer
                .cell((u16::try_from(number).unwrap(), u16::try_from(row).unwrap()))
                .unwrap()
                .fg,
            active_theme().muted
        );
        assert_ne!(
            buffer
                .cell((u16::try_from(content).unwrap(), u16::try_from(row).unwrap()))
                .unwrap()
                .fg,
            active_theme().muted
        );
    }

    #[test]
    fn go_to_line_centers_and_highlights_the_source_line() {
        let (_home, repository, _paths, mut app) = fixture();
        let content = (1..=100)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(repository.path().join("AGENTS.md"), content).unwrap();
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

        assert!(app.scroll > 0);
        assert!(matches!(app.highlighted_line, Some((80, _))));
        assert!(draw_at(&mut app, 80, 24).contains(" 80 │ line 80"));
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

    #[test]
    fn home_width_is_responsive_and_documents_use_the_screen() {
        let (_home, repository, _paths, mut app) = fixture();
        assert_eq!(home_canvas_width(&app, 60), 60);
        assert!(home_canvas_width(&app, 180) < COMPACT_WIDTH);
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
    fn shift_arrows_page_the_visible_document() {
        let (_home, repository, _paths, mut app) = fixture();
        fs::write(
            repository.path().join("AGENTS.md"),
            "long line\n".repeat(100),
        )
        .unwrap();
        app.open(View::File {
            title: "AGENTS.md".into(),
            path: repository.path().join("AGENTS.md"),
            about: "Project file".into(),
        });
        draw_at(&mut app, 80, 20);
        handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT));
        assert_eq!(app.scroll, PAGE_LINES);
    }

    // Two Profiles that differ on the claude target, plus a Section no Profile uses.
    const GLOBAL_MANIFEST: &str = r#"
[[sections]]
id = "common"
name = "Common"
path = "sections/common.md"

[[sections]]
id = "claude"
name = "Claude"
path = "sections/claude.md"

[[sections]]
id = "scratch"
name = "Scratch"
path = "sections/scratch.md"

[targets.claude]
path = "~/.claude/CLAUDE.md"
title = "Global Claude"

[targets.agents]
path = "~/.codex/AGENTS.md"
title = "Global Agents"

[profiles.default]
claude = ["common", "claude"]
agents = ["common"]

[profiles.work]
claude = ["common"]
agents = ["common"]
"#;

    fn global_fixture() -> (TempDir, TempDir, Paths, App) {
        let home = TempDir::new().unwrap();
        let repository = TempDir::new().unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .current_dir(repository.path())
                .status()
                .unwrap()
                .success()
        );
        let paths = Paths::for_home(home.path());
        fs::create_dir_all(paths.config.parent().unwrap().join("sections")).unwrap();
        fs::write(&paths.config, GLOBAL_MANIFEST).unwrap();
        for (name, contents) in [
            ("common.md", "# Common\n"),
            ("claude.md", "# Claude\n"),
            ("scratch.md", "# Scratch\n"),
        ] {
            fs::write(
                paths.config.parent().unwrap().join("sections").join(name),
                contents,
            )
            .unwrap();
        }
        let global = GlobalConfig::load(&paths).unwrap();
        let app = App::new_at(paths.clone(), Some(global), repository.path().to_owned()).unwrap();
        (home, repository, paths, app)
    }

    #[test]
    fn configured_global_profile_keeps_external_symlink_aliases_visible() {
        use std::os::unix::fs::symlink;

        let home = TempDir::new().unwrap();
        let repository = TempDir::new().unwrap();
        let paths = Paths::for_home(home.path());
        fs::create_dir_all(paths.config.parent().unwrap().join("sections")).unwrap();
        fs::create_dir_all(home.path().join(".codex")).unwrap();
        fs::create_dir_all(home.path().join(".claude")).unwrap();
        fs::write(
            &paths.config,
            r#"
[[sections]]
id = "common"
name = "Common"
path = "sections/common.md"

[targets.codex]
path = "~/.codex/AGENTS.md"
title = "Global Agents"

[profiles.default]
codex = ["common"]
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
        let alias = home.path().join(".claude/CLAUDE.md");
        symlink(home.path().join(".codex/AGENTS.md"), &alias).unwrap();

        let app = App::new_at(paths, Some(global), repository.path().to_owned()).unwrap();
        let (_, source) = app
            .global_sources
            .iter()
            .find(|(runtime, _)| *runtime == ContextRuntime::Claude)
            .unwrap();
        let info = symlink_info(&app, &source.path).unwrap();
        let SymlinkResolution::Resolved(resolved) = info.resolution else {
            panic!("global alias should resolve");
        };
        let destination = managed_destination(&app, &resolved).unwrap();
        assert_eq!(destination.scope, "Global");
        assert!(
            destination
                .status
                .is_some_and(InstructionStatus::is_current)
        );
        assert!(
            app.home_items()
                .iter()
                .any(|item| matches!(item, HomeItem::GlobalExternal(_)))
        );
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
        let info = symlink_info(&app, &claude).unwrap();

        let status = global_symlink_home_status(&app, "claude", &claude, &info);
        assert!(status.contains("target managed"));
        assert!(status.contains("differs from Profile"));
    }

    #[test]
    fn theme_reload_keeps_global_instructions_and_the_last_valid_palette() {
        let (_home, _repository, paths, mut app) = global_fixture();
        let source = fs::read_to_string(&paths.config).unwrap();
        fs::write(
            &paths.config,
            format!("[ui]\ntheme = \"github-light\"\n\n{source}"),
        )
        .unwrap();
        app.reload_from_disk();
        assert_eq!(active_theme().background, Color::Rgb(255, 255, 255));

        let source = fs::read_to_string(&paths.config)
            .unwrap()
            .replace("github-light", "unknown");
        fs::write(&paths.config, source).unwrap();
        app.reload_from_disk();

        assert!(app.global.is_some());
        assert_eq!(active_theme().background, Color::Rgb(255, 255, 255));
        assert!(
            app.message
                .as_ref()
                .is_some_and(|(ok, message)| !ok && message.contains("unknown theme"))
        );
        reset_theme();
    }

    #[test]
    fn theme_picker_previews_cancels_and_saves_the_choice() {
        let (_home, _repository, paths, mut app) = global_fixture();
        let source = fs::read_to_string(&paths.config).unwrap();
        let original = active_theme().background;
        let nord = theme::names().position(|name| name == "nord").unwrap();

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE),
        );
        app.theme_picker.as_mut().unwrap().selected = nord;
        preview_theme_picker(&app);
        assert_eq!(
            active_theme().background,
            theme::resolve("nord").unwrap().background
        );
        handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.theme_picker.is_none());
        assert_eq!(active_theme().background, original);

        app.open_theme_picker();
        app.theme_picker.as_mut().unwrap().selected = nord;
        preview_theme_picker(&app);
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let saved = fs::read_to_string(&paths.config).unwrap();
        assert_ne!(saved, source);
        assert!(saved.contains("[ui]\ntheme = \"nord\""));

        app.reload_from_disk();
        assert_eq!(
            active_theme().background,
            theme::resolve("nord").unwrap().background
        );
        reset_theme();
    }

    fn open_library(app: &mut App) {
        app.home_index = app
            .home_items()
            .iter()
            .position(|item| home_item_key(item) == "library")
            .unwrap();
        handle_key(app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(app.view, View::GlobalLibrary));
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

        app.section_index = app.library_entries().len() - 1;
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
                "library" => assert!(matches!(app.view, View::GlobalLibrary)),
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
        let selected = managed_workspace(&app, &reference).unwrap().sections[1]
            .path
            .clone();
        let source = fs::read_to_string(&paths.config).unwrap().replace(
            "claude = [\"common\", \"claude\", \"delegation-rules\"]",
            "claude = [\"claude\", \"common\", \"delegation-rules\"]",
        );
        fs::write(&paths.config, source).unwrap();
        app.reload_from_disk();
        assert_eq!(
            managed_workspace(&app, &reference).unwrap().sections[app.section_index - 1].path,
            selected
        );
    }

    #[test]
    fn reload_returns_to_the_library_when_the_browsed_profile_is_removed() {
        let (_home, _repository, paths, mut app) = global_fixture();
        open_library(&mut app);
        app.section_index = 1;
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(&app.view, View::GlobalProfile(profile) if profile == "work"));

        let manifest = GLOBAL_MANIFEST
            .split("[profiles.work]")
            .next()
            .unwrap()
            .to_owned();
        fs::write(&paths.config, manifest).unwrap();
        app.reload_from_disk();

        assert!(matches!(app.view, View::GlobalLibrary));
        assert!(draw(&mut app).contains("default"));
    }

    #[test]
    fn reload_keeps_the_browsed_profile_when_activation_changes() {
        let (_home, _repository, _paths, mut app) = global_fixture();
        deploy::apply(app.global.as_ref().unwrap(), "default", None, false, true).unwrap();
        app.reload_from_disk();
        open_library(&mut app);
        app.section_index = 1;
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(draw(&mut app).contains("Profile work · not active"));

        deploy::apply(app.global.as_ref().unwrap(), "work", None, false, true).unwrap();
        app.reload_from_disk();

        assert!(matches!(&app.view, View::GlobalProfile(profile) if profile == "work"));
        let screen = draw(&mut app);
        assert!(screen.contains("Profile work · active"));
        assert!(!screen.contains("matches current file"));
        assert!(!screen.contains("differs from current file"));
    }

    #[test]
    fn back_returns_to_the_same_library_row() {
        let (_home, _repository, _paths, mut app) = global_fixture();
        open_library(&mut app);
        app.section_index = 4;
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(app.view, View::SectionDocument { .. }));
        handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(app.view, View::GlobalLibrary));
        assert_eq!(app.section_index, 4);

        app.section_index = 1;
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(app.view, View::GlobalProfile(_)));
        handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(app.view, View::GlobalLibrary));
        assert_eq!(app.section_index, 1);
    }

    #[test]
    fn back_returns_to_the_same_profile_target_row() {
        let (_home, _repository, _paths, mut app) = global_fixture();
        open_library(&mut app);
        app.section_index = 1;
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(&app.view, View::GlobalProfile(profile) if profile == "work"));
        handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.section_index, 1);
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(
            &app.view,
            View::Managed(ManagedRef::Global { target, .. }) if target == "agents"
        ));
        handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(app.view, View::GlobalProfile(_)));
        assert_eq!(app.section_index, 1);
    }

    #[test]
    fn reload_retains_the_selected_identity_after_manifest_edits() {
        let (_home, _repository, paths, mut app) = global_fixture();
        open_library(&mut app);
        app.section_index = 4;
        assert!(matches!(
            app.library_entries().get(app.section_index),
            Some(LibraryEntry::PersonalSection(id)) if id == "scratch"
        ));

        // Insert a Section ahead of the selection.
        let inserted = GLOBAL_MANIFEST.replace(
            "[[sections]]\nid = \"common\"",
            "[[sections]]\nid = \"alpha\"\nname = \"Alpha\"\npath = \"sections/alpha.md\"\n\n[[sections]]\nid = \"common\"",
        );
        fs::write(
            paths.config.parent().unwrap().join("sections/alpha.md"),
            "# Alpha\n",
        )
        .unwrap();
        fs::write(&paths.config, &inserted).unwrap();
        app.reload_from_disk();
        assert!(matches!(app.view, View::GlobalLibrary));
        assert_eq!(app.section_index, 5);
        assert!(matches!(
            app.library_entries().get(app.section_index),
            Some(LibraryEntry::PersonalSection(id)) if id == "scratch"
        ));

        // Reorder targets while the profile inspector has the second one selected.
        app.section_index = 1;
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(&app.view, View::GlobalProfile(profile) if profile == "work"));
        handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.section_index, 1);
        let reordered = inserted.replace(
            "[targets.claude]\npath = \"~/.claude/CLAUDE.md\"\ntitle = \"Global Claude\"\n\n[targets.agents]\npath = \"~/.codex/AGENTS.md\"\ntitle = \"Global Agents\"",
            "[targets.agents]\npath = \"~/.codex/AGENTS.md\"\ntitle = \"Global Agents\"\n\n[targets.claude]\npath = \"~/.claude/CLAUDE.md\"\ntitle = \"Global Claude\"",
        );
        assert_ne!(reordered, inserted);
        fs::write(&paths.config, reordered).unwrap();
        app.reload_from_disk();
        assert!(matches!(&app.view, View::GlobalProfile(profile) if profile == "work"));
        assert_eq!(app.section_index, 0);
        assert_eq!(
            app.global
                .as_ref()
                .unwrap()
                .target_names()
                .nth(app.section_index),
            Some("agents")
        );
    }

    #[test]
    fn active_home_and_inspector_use_the_profile_subset() {
        let home = TempDir::new().unwrap();
        let repository = TempDir::new().unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .current_dir(repository.path())
                .status()
                .unwrap()
                .success()
        );
        let paths = Paths::for_home(home.path());
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
        let mut app =
            App::new_at(paths.clone(), Some(global), repository.path().to_owned()).unwrap();
        let keys: Vec<_> = app.home_items().iter().map(home_item_key).collect();
        assert!(keys.iter().any(|key| key == "global:pi"));
        assert!(keys.iter().any(|key| key == "global:claude"));

        deploy::apply(app.global.as_ref().unwrap(), "default", None, false, true).unwrap();
        deploy::apply(app.global.as_ref().unwrap(), "work", None, false, true).unwrap();
        app.reload_from_disk();
        let keys: Vec<_> = app.home_items().iter().map(home_item_key).collect();
        assert!(!keys.iter().any(|key| key == "global:pi"));
        assert!(keys.iter().any(|key| key == "global:claude"));
        let pi_path = app.global.as_ref().unwrap().target_path("pi").unwrap();
        assert!(pi_path.exists());
        assert!(
            app.global_sources
                .iter()
                .any(|(_, source)| source.path == pi_path)
        );

        open_library(&mut app);
        app.section_index = 1;
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(&app.view, View::GlobalProfile(profile) if profile == "work"));
        let pi = app
            .global
            .as_ref()
            .unwrap()
            .target_names()
            .position(|id| id == "pi")
            .unwrap();
        assert!(profile_target_reference(&app, "work", pi).is_none());
        app.section_index = pi;
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(&app.view, View::GlobalProfile(profile) if profile == "work"));
    }
}
