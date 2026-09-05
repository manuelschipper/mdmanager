#[path = "context_view.rs"]
mod context_view;
#[path = "document.rs"]
mod document;
#[path = "inspection.rs"]
mod inspection;
#[path = "library_view.rs"]
mod library_view;
#[path = "managed_view.rs"]
mod managed_view;
#[path = "overview.rs"]
mod overview;
#[path = "reload.rs"]
mod reload;

use self::context_view::{
    ContextEntry, ContextViewInput, context_entries, render_context, render_source,
};
use self::document::{
    COMPACT_WIDTH, DocumentState, LinePrompt, SearchPrompt, bounded, bounded_around,
    handle_line_prompt_key, handle_search_key, info_panel_geometry, relative_message,
    render_document, render_info, render_line_prompt, render_search, truncate_start,
};
use self::inspection::{
    HomeItem, InspectionInput, InstructionInspection, ManagedRef, external_file_about,
    global_diagnostic_text, managed_workspace,
};
use self::library_view::{
    LibraryEntry, LibraryViewInput, library_entries, library_panel_area, personal_section_usage,
    profile_deploys, profile_panel_area, profile_target_reference, project_section_usage,
    render_global_profile, render_library,
};
use self::managed_view::{ManagedViewInput, render_diff, render_managed, render_managed_document};
use self::overview::{
    OverviewInput, home_canvas_height, home_canvas_width, home_header_line, home_item_key,
    home_target_reference, render_home,
};
use self::reload::{ReloadInput, ReloadWatcher, current_watch_signature};
use super::context_ui;
use super::{ACTIVE_THEME, activate_theme, active_theme, reset_theme};
use std::env;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};

use super::context_ui::ContextUi;
use crate::config::{self, GlobalConfig, Paths, target_display_name};
use crate::context::{Audit, ContextRuntime, ContextSource, SourceGroup, SourceState};
use crate::deploy;
use crate::local::{self, LocalRepository};
use crate::project::{self, Workspace};
use crate::theme::{self, Theme};

const RELOAD_MESSAGE_DURATION: Duration = Duration::from_secs(3);
const PAGE_LINES: u16 = 8;
const INTRO_DURATION: Duration = Duration::from_millis(1800);
const INTRO_WORDMARK: [&str; 5] = [
    "███╗   ███╗ ██████╗ ",
    "████╗ ████║ ██╔══██╗",
    "██╔████╔██║ ██║  ██║",
    "██║╚██╔╝██║ ██████╔╝",
    "╚═╝     ╚═╝ ╚═════╝ ",
];

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
    LibraryBrowser,
    GlobalProfile(String),
    Context,
    Source,
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
    document: DocumentState,
    context_preview: bool,
    help: bool,
    picker: Option<RuntimePicker>,
    theme_picker: Option<ThemePicker>,
    message: Option<(bool, String)>,
    message_expires_at: Option<Instant>,
    watcher: ReloadWatcher,
    // Set once by `run` at process start; cleared by expiry or the first key.
    // Nothing else ever sets it, so the intro cannot replay.
    intro_started: Option<Instant>,
}

impl App {
    fn reload_input(&self) -> ReloadInput<'_> {
        ReloadInput {
            audit: &self.inspection.context,
            paths: &self.paths,
            watch_root: &self.watch_root,
        }
    }

    fn overview_input(&self) -> OverviewInput<'_> {
        OverviewInput {
            global: &self.global,
            global_active_profile: &self.global_active_profile,
            global_sources: &self.global_sources,
            inspection: &self.inspection,
            is_home: matches!(self.view, View::Home),
            local_repository: &self.local_repository,
            message: &self.message,
            paths: &self.paths,
            project: &self.project,
            project_root: &self.project_root,
            scan_spinner: self.context.scan_spinner(),
            scan_status: &self.context.scan_status,
        }
    }

    fn library_view_input(&self) -> LibraryViewInput<'_> {
        LibraryViewInput {
            global: &self.global,
            global_active_profile: &self.global_active_profile,
            managed: &self.inspection.managed,
            personal_section_targets: &self.inspection.personal_section_targets,
            project: &self.project,
        }
    }

    fn apply_search(&mut self, query: String) {
        if let Some(text) = self.searchable_text()
            && let Some((ok, message)) = self.document.apply_search(&text, query)
        {
            self.set_message(ok, message);
        }
    }

    fn go_to_line(&mut self, input: String) {
        if let Some(text) = self.numbered_text() {
            let (ok, message) = self.document.go_to_line(&text, input);
            self.set_message(ok, message);
        }
    }

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
            document: DocumentState::default(),
            context_preview: false,
            help: false,
            picker: None,
            theme_picker: None,
            message: theme_error.map(|error| (false, format!("{error}; keeping current theme"))),
            message_expires_at: None,
            watcher: ReloadWatcher::new(),
            intro_started: None,
        };
        app.refresh_inspection();
        app.watcher.signature = current_watch_signature(&app.reload_input());
        Ok(app)
    }

    // Event handling owns refresh; render, search and sizing only read this snapshot.
    fn refresh_inspection(&mut self) {
        self.inspection = InstructionInspection::observe(&InspectionInput {
            context: &self.context.audit,
            global: &self.global,
            global_active_profile: &self.global_active_profile,
            global_error: &self.global_error,
            global_sources: &self.global_sources,
            local_repository: &self.local_repository,
            project: &self.project,
            project_error: &self.project_error,
            project_root: &self.project_root,
            document_paths: std::iter::once(&self.view)
                .chain(self.history.iter().map(|(view, _)| view))
                .filter_map(|view| match view {
                    View::File { path, .. } | View::SectionDocument { path, .. } => {
                        Some(path.as_path())
                    }
                    _ => None,
                })
                .collect(),
        });
    }

    fn home_items(&self) -> Vec<HomeItem> {
        self.inspection.home_items.clone()
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
            View::LibraryBrowser => self.global.is_none() && self.project.is_none(),
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
        self.document.scroll = 0;
        self.document.max_scroll = 0;
        self.section_index = 0;
        self.document.highlighted_line = None;
        self.clear_message();
    }

    fn back(&mut self) -> SessionAction {
        if matches!(self.view, View::Home) {
            return SessionAction::Quit;
        }
        let (view, section_index) = self.history.pop().unwrap_or((View::Home, 0));
        self.view = view;
        self.section_index = section_index;
        self.document.scroll = 0;
        self.document.max_scroll = 0;
        SessionAction::Continue
    }

    fn reload_from_disk(&mut self) {
        self.reload_observations(false);
    }

    // Accepted scans and document opens retain the scan only while watched inputs
    // are unchanged. A disk change invalidates both the audit and its receiver.
    fn reload_observations(&mut self, preserve_context_scan: bool) {
        let notify_reload = !preserve_context_scan;
        let preserve_context_scan = preserve_context_scan
            && current_watch_signature(&self.reload_input()) == self.watcher.signature;
        let previous_global_error = self.global_error.clone();
        let selected_home = self.home_items().get(self.home_index).map(home_item_key);
        let selected_context =
            context_entries(&self.inspection.context, self.context_nested_expanded)
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
            context_entries(&self.inspection.context, self.context_nested_expanded)
                .get(self.context_entry_index),
            Some(ContextEntry::Nested)
        );
        let selected_managed_section = match &self.view {
            View::Managed(reference) => managed_workspace(&self.inspection.managed, reference)
                .ok()
                .and_then(|workspace| {
                    self.section_index
                        .checked_sub(1)
                        .and_then(|index| workspace.sections.get(index))
                        .map(|section| section.id.clone())
                }),
            _ => None,
        };
        let selected_library = match &self.view {
            View::LibraryBrowser => library_entries(&self.library_view_input())
                .get(self.section_index)
                .cloned(),
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
                context_entries(&self.inspection.context, self.context_nested_expanded)
                    .iter()
                    .position(|entry| {
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
                        context_entries(&self.inspection.context, self.context_nested_expanded)
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
            self.document.scroll = 0;
            self.document.max_scroll = 0;
        }
        if let View::Diff(reference) = self.view.clone()
            && managed_workspace(&self.inspection.managed, &reference)
                .is_ok_and(|workspace| workspace.difference.is_none())
        {
            let (view, section_index) = self.history.pop().unwrap_or((View::Home, 0));
            self.view = view;
            self.section_index = section_index;
            self.document.scroll = 0;
            self.document.max_scroll = 0;
        }
        // Re-find the selected Profile, Section, or target by identity so inserted
        // or reordered manifest entries do not move the selection to another item.
        match &self.view {
            View::Managed(reference) => {
                if let Some(id) = selected_managed_section
                    && let Ok(workspace) = managed_workspace(&self.inspection.managed, reference)
                {
                    self.section_index = workspace
                        .sections
                        .iter()
                        .position(|section| section.id == id)
                        .map_or(0, |index| index + 1);
                }
            }
            View::LibraryBrowser => {
                if let Some(entry) = &selected_library
                    && let Some(position) = library_entries(&self.library_view_input())
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
                self.document.scroll = 0;
                self.document.max_scroll = 0;
            }
        }
        if let Some(error) = theme_error {
            self.message = Some((false, format!("{error}; keeping current theme")));
            self.message_expires_at = None;
        } else if notify_reload {
            self.message = Some((true, "Updated from disk".into()));
            self.message_expires_at = Some(Instant::now() + RELOAD_MESSAGE_DURATION);
        }
        self.watcher
            .accept_reload(current_watch_signature(&self.reload_input()));
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
        let input = ReloadInput {
            audit: &self.inspection.context,
            paths: &self.paths,
            watch_root: &self.watch_root,
        };
        if self.watcher.poll_reload(&input) {
            self.reload_from_disk();
        }
    }

    fn move_context_selection(&mut self, down: bool) {
        let len = context_entries(&self.inspection.context, self.context_nested_expanded).len();
        self.context_entry_index = if len == 0 {
            0
        } else if down {
            (self.context_entry_index + 1).min(len - 1)
        } else {
            self.context_entry_index.saturating_sub(1)
        };
        self.document.scroll = 0;
    }

    fn open_context_entry(&mut self) {
        match context_entries(&self.inspection.context, self.context_nested_expanded)
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
        self.document.scroll = 0;
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
            View::ManagedDocument(reference) => {
                managed_workspace(&self.inspection.managed, reference)
                    .ok()
                    .map(|view| view.rendered)
            }
            View::Diff(reference) => managed_workspace(&self.inspection.managed, reference)
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
            View::ManagedDocument(reference) => {
                managed_workspace(&self.inspection.managed, reference)
                    .ok()
                    .map(|view| view.rendered)
            }
            _ => None,
        }
    }
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
    if app.document.search.is_some() {
        if let Some(query) = handle_search_key(&mut app.document, key) {
            app.apply_search(query);
        }
        return SessionAction::Continue;
    }
    if app.document.line_prompt.is_some() {
        if let Some(input) = handle_line_prompt_key(&mut app.document, key) {
            app.go_to_line(input);
        }
        return SessionAction::Continue;
    }
    if key.modifiers.contains(KeyModifiers::SHIFT)
        && matches!(key.code, KeyCode::Up | KeyCode::Down)
    {
        app.document.scroll = if key.code == KeyCode::Down {
            app.document
                .scroll
                .saturating_add(PAGE_LINES)
                .min(app.document.max_scroll)
        } else {
            app.document.scroll.saturating_sub(PAGE_LINES)
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
                View::Managed(_) | View::LibraryBrowser | View::GlobalProfile(_) => {
                    app.section_index = app.section_index.saturating_sub(1);
                    app.document.scroll = 0;
                }
                _ => app.document.scroll = app.document.scroll.saturating_sub(1),
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
                    let len = managed_workspace(&app.inspection.managed, reference)
                        .map_or(1, |workspace| workspace.sections.len() + 1);
                    app.section_index = (app.section_index + 1).min(len.saturating_sub(1));
                    app.document.scroll = 0;
                }
                View::LibraryBrowser => {
                    let len = library_entries(&app.library_view_input()).len();
                    app.section_index = (app.section_index + 1).min(len.saturating_sub(1));
                    app.document.scroll = 0;
                }
                View::GlobalProfile(_) => {
                    let len = app
                        .global
                        .as_ref()
                        .map_or(0, |global| global.target_names().count());
                    app.section_index = (app.section_index + 1).min(len.saturating_sub(1));
                    app.document.scroll = 0;
                }
                _ => {
                    app.document.scroll = app
                        .document
                        .scroll
                        .saturating_add(1)
                        .min(app.document.max_scroll)
                }
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
            app.document.search = Some(SearchPrompt {
                query: String::new(),
            });
            SessionAction::Continue
        }
        KeyCode::Char('g') if app.numbered_text().is_some() => {
            app.document.line_prompt = Some(LinePrompt {
                input: String::new(),
            });
            SessionAction::Continue
        }
        KeyCode::Char('d') => {
            let reference = match &app.view {
                View::Home => home_target_reference(&app.overview_input(), &app.home_index),
                View::Managed(reference) | View::ManagedDocument(reference) => {
                    Some(reference.clone())
                }
                View::GlobalProfile(profile) => {
                    profile_target_reference(&app.library_view_input(), profile, app.section_index)
                }
                _ => None,
            };
            if let Some(reference) = reference {
                match managed_workspace(&app.inspection.managed, &reference) {
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

fn enter(app: &mut App) {
    match app.view.clone() {
        View::Home => enter_home(app),
        View::Context => app.open_context_entry(),
        View::Managed(reference) => {
            if app.section_index == 0 {
                app.open(View::ManagedDocument(reference));
            } else if let Ok(workspace) = managed_workspace(&app.inspection.managed, &reference)
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
        View::LibraryBrowser => match library_entries(&app.library_view_input())
            .get(app.section_index)
            .cloned()
        {
            Some(LibraryEntry::Profile(profile)) => app.open(View::GlobalProfile(profile)),
            Some(LibraryEntry::PersonalSection(id)) => {
                let Some(global) = &app.global else { return };
                let (Some(section), Some(path)) = (global.section(&id), global.section_path(&id))
                else {
                    return;
                };
                let usage = personal_section_usage(&app.library_view_input(), &id);
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
            if let Some(reference) =
                profile_target_reference(&app.library_view_input(), &profile, app.section_index)
            {
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
        HomeItem::Library => app.open(View::LibraryBrowser),
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
        render_info(frame, &mut app.document, frame.area(), &title, &text);
        return;
    }
    if matches!(app.view, View::LibraryBrowser) {
        render_library(
            frame,
            &LibraryViewInput {
                global: &app.global,
                global_active_profile: &app.global_active_profile,
                managed: &app.inspection.managed,
                personal_section_targets: &app.inspection.personal_section_targets,
                project: &app.project,
            },
            &mut app.document,
            &mut app.section_index,
            frame.area(),
        );
        return;
    }
    if let View::GlobalProfile(profile) = app.view.clone() {
        render_global_profile(
            frame,
            &LibraryViewInput {
                global: &app.global,
                global_active_profile: &app.global_active_profile,
                managed: &app.inspection.managed,
                personal_section_targets: &app.inspection.personal_section_targets,
                project: &app.project,
            },
            &mut app.document,
            &mut app.section_index,
            frame.area(),
            &profile,
        );
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
        View::Home => render_home(
            frame,
            &OverviewInput {
                global: &app.global,
                global_active_profile: &app.global_active_profile,
                global_sources: &app.global_sources,
                inspection: &app.inspection,
                is_home: matches!(app.view, View::Home),
                local_repository: &app.local_repository,
                message: &app.message,
                paths: &app.paths,
                project: &app.project,
                project_root: &app.project_root,
                scan_spinner: app.context.scan_spinner(),
                scan_status: &app.context.scan_status,
            },
            &mut app.document,
            &mut app.home_index,
            body,
        ),
        View::Info { .. } => unreachable!("Info views use the focus panel"),
        View::File { title, path, about } => {
            let about = external_file_about(
                app.global
                    .as_ref()
                    .zip(app.global_active_profile.as_deref()),
                &app.inspection,
                &app.local_repository,
                &app.project,
                &app.project_root,
                &about,
                &path,
            );
            let content = app.inspection.document(&path).unwrap_or_else(|error| error);
            render_document(
                frame,
                &mut app.document,
                body,
                &title,
                &about,
                &content,
                true,
            );
        }
        View::SectionDocument { title, path, about } => {
            let content = app.inspection.document(&path).unwrap_or_else(|error| error);
            render_document(
                frame,
                &mut app.document,
                body,
                &title,
                &about,
                &content,
                true,
            );
        }
        View::Managed(reference) => render_managed(
            frame,
            &ManagedViewInput {
                global: &app.global,
                global_active_profile: &app.global_active_profile,
                inspection: &app.inspection,
                local_repository: &app.local_repository,
                project: &app.project,
                project_root: &app.project_root,
            },
            &mut app.document,
            &mut app.section_index,
            body,
            &reference,
        ),
        View::ManagedDocument(reference) => {
            render_managed_document(frame, &app.inspection, &mut app.document, body, &reference)
        }
        View::Diff(reference) => render_diff(
            frame,
            &ManagedViewInput {
                global: &app.global,
                global_active_profile: &app.global_active_profile,
                inspection: &app.inspection,
                local_repository: &app.local_repository,
                project: &app.project,
                project_root: &app.project_root,
            },
            &mut app.document,
            body,
            &reference,
        ),
        View::LibraryBrowser | View::GlobalProfile(_) => {
            unreachable!("Library views use focus panels")
        }
        View::Context => render_context(
            frame,
            &ContextViewInput {
                context_entry_index: app.context_entry_index,
                context_nested_expanded: app.context_nested_expanded,
                global: &app.global,
                global_active_profile: &app.global_active_profile,
                inspection: &app.inspection,
                local_repository: &app.local_repository,
                paths: &app.paths,
                project: &app.project,
                project_root: &app.project_root,
                scan_spinner: app.context.scan_spinner(),
                scan_status: &app.context.scan_status,
                source_index: app.context.source_index,
            },
            &mut app.document,
            &mut app.context_preview,
            body,
        ),
        View::Source => render_source(
            frame,
            &ContextViewInput {
                context_entry_index: app.context_entry_index,
                context_nested_expanded: app.context_nested_expanded,
                global: &app.global,
                global_active_profile: &app.global_active_profile,
                inspection: &app.inspection,
                local_repository: &app.local_repository,
                paths: &app.paths,
                project: &app.project,
                project_root: &app.project_root,
                scan_spinner: app.context.scan_spinner(),
                scan_status: &app.context.scan_status,
                source_index: app.context.source_index,
            },
            &mut app.document,
            body,
        ),
    }
    render_footer(frame, app, footer);
    if app.document.search.is_some() {
        render_search(frame, &app.document);
    }
    if app.document.line_prompt.is_some() {
        render_line_prompt(frame, &app.document);
    }
}

fn canvas_area(area: Rect, app: &App) -> Rect {
    let width = match app.view {
        View::Home => home_canvas_width(&app.overview_input(), area.width),
        View::Info { .. } => area.width.min(COMPACT_WIDTH),
        _ => area.width,
    };
    let height = if matches!(app.view, View::Home) {
        home_canvas_height(&app.overview_input()).min(area.height)
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

fn render_header(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let path = match &app.view {
        View::Home => app.project_root.display().to_string(),
        View::Info { title, .. } => title.clone(),
        View::File { title, .. } | View::SectionDocument { title, .. } => title.clone(),
        View::Managed(reference) | View::ManagedDocument(reference) | View::Diff(reference) => {
            managed_workspace(&app.inspection.managed, reference)
                .map_or_else(|_| "Managed target".into(), |workspace| workspace.title)
        }
        View::LibraryBrowser => "Profiles & Sections".into(),
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
    if let Some(line) = home_header_line(&app.overview_input()) {
        lines.push(line);
    }
    frame.render_widget(
        Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false }),
        inner,
    );
}

fn header_height(app: &App, width: u16) -> u16 {
    let Some(line) = home_header_line(&app.overview_input()) else {
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

fn render_footer(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let keys = match &app.view {
        View::Home => {
            let difference = if home_target_reference(&app.overview_input(), &app.home_index)
                .and_then(|reference| managed_workspace(&app.inspection.managed, &reference).ok())
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
            let entries = context_entries(&app.inspection.context, app.context_nested_expanded);
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
            let difference = if managed_workspace(&app.inspection.managed, reference)
                .is_ok_and(|view| view.difference.is_some())
            {
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
            if managed_workspace(&app.inspection.managed, reference)
                .is_ok_and(|view| view.difference.is_some())
            {
                "↑↓ scroll   ⇧↑/⇧↓ page   g line   / search   d difference   esc back"
            } else {
                "↑↓ scroll   ⇧↑/⇧↓ page   g line   / search   esc back"
            }
        }
        View::Diff(_) => "↑↓ scroll   ⇧↑/⇧↓ page   / search   d close difference   esc back",
        View::LibraryBrowser => "↑↓ select   enter inspect   esc back",
        View::GlobalProfile(profile) => {
            let difference =
                if profile_target_reference(&app.library_view_input(), profile, app.section_index)
                    .and_then(|reference| {
                        managed_workspace(&app.inspection.managed, &reference).ok()
                    })
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
                    relative_message(&app.project_root, text)
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
        View::LibraryBrowser => (
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

fn help_anchor_area(app: &App, screen: Rect) -> Rect {
    match &app.view {
        View::Home => canvas_area(screen, app),
        View::Info { text, .. } => info_panel_geometry(screen, text).0,
        View::LibraryBrowser => {
            let entries = library_entries(&app.library_view_input());
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
    use std::fs;
    use unicode_width::UnicodeWidthStr;

    use super::overview::home_group;

    use std::process::Command;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use tempfile::TempDir;

    use super::*;

    pub(super) fn fixture() -> (TempDir, TempDir, Paths, App) {
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

    pub(super) fn draw(app: &mut App) -> String {
        draw_at(app, 100, 36)
    }

    pub(super) fn draw_at(app: &mut App, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, app)).unwrap();
        terminal.backend().to_string()
    }

    pub(super) fn finish_context_scan(app: &mut App) {
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
            managed_workspace(&app.inspection.managed, &reference)
                .unwrap()
                .difference
                .is_some()
        );

        fs::remove_file(&section).unwrap();
        app.reload_from_disk();
        assert!(managed_workspace(&app.inspection.managed, &reference).is_err());
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
        app.context_entry_index =
            context_entries(&app.inspection.context, app.context_nested_expanded)
                .iter()
                .position(|entry| matches!(entry, ContextEntry::Source(0)))
                .unwrap();
        fs::write(repository.path().join("AGENTS.md"), "# Updated\n").unwrap();
        app.reload_from_disk();
        let entry = context_entries(&app.inspection.context, app.context_nested_expanded)
            [app.context_entry_index];
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

    pub(super) fn global_fixture() -> (TempDir, TempDir, Paths, App) {
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

    pub(super) fn open_library(app: &mut App) {
        app.home_index = app
            .home_items()
            .iter()
            .position(|item| home_item_key(item) == "library")
            .unwrap();
        handle_key(app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(app.view, View::LibraryBrowser));
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

        assert!(matches!(app.view, View::LibraryBrowser));
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
        assert!(matches!(app.view, View::LibraryBrowser));
        assert_eq!(app.section_index, 4);

        app.section_index = 1;
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(app.view, View::GlobalProfile(_)));
        handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(app.view, View::LibraryBrowser));
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
            library_entries(&app.library_view_input()).get(app.section_index),
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
        assert!(matches!(app.view, View::LibraryBrowser));
        assert_eq!(app.section_index, 5);
        assert!(matches!(
            library_entries(&app.library_view_input()).get(app.section_index),
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
        assert!(profile_target_reference(&app.library_view_input(), "work", pi).is_none());
        app.section_index = pi;
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(&app.view, View::GlobalProfile(profile) if profile == "work"));
    }
}
