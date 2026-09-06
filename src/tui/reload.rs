use std::collections::hash_map::DefaultHasher;
use std::env;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

use walkdir::{DirEntry, WalkDir};

use crate::config::Paths;
use crate::context::{Audit, claude_config_dir, pi_agent_dir};

const AUTO_RELOAD_INTERVAL: Duration = Duration::from_secs(1);

/// Borrowed reload inputs from the session's accepted observations.
pub(super) struct ReloadInput<'a> {
    pub(super) audit: &'a Audit,
    pub(super) paths: &'a Paths,
    pub(super) watch_root: &'a Option<PathBuf>,
}

fn hash_tree(path: &Path, hasher: &mut DefaultHasher, project_tree: bool) {
    hash_dependency_tree(path, hasher, project_tree, false);
}

fn hash_dependency_tree(
    path: &Path,
    hasher: &mut DefaultHasher,
    project_tree: bool,
    rule_directory: bool,
) {
    if !path.exists() {
        path.hash(hasher);
        0_u8.hash(hasher);
        return;
    }
    let walker = WalkDir::new(path)
        .follow_links(rule_directory)
        .into_iter()
        .filter_entry(|entry| !ignored_watch_entry(entry));
    for entry in walker.flatten() {
        // Claude and Cursor rule directories share the resolver's linked traversal.
        if !rule_directory
            && (entry.path().ends_with(".claude/rules") || entry.path().ends_with(".cursor/rules"))
            && entry.path().is_dir()
        {
            hash_dependency_tree(entry.path(), hasher, false, true);
        }
        if entry.file_type().is_file() && project_tree && !relevant_project_file(entry.path()) {
            continue;
        }
        entry.path().hash(hasher);
        if entry.file_type().is_symlink() {
            fs::read_link(entry.path()).ok().hash(hasher);
        }
        let metadata = if entry.file_type().is_symlink() && entry.path().is_dir() {
            fs::symlink_metadata(entry.path())
        } else {
            fs::metadata(entry.path())
        };
        if let Ok(metadata) = metadata {
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

pub(super) fn current_watch_signature(input: &ReloadInput<'_>) -> u64 {
    let mut hasher = DefaultHasher::new();
    for path in [
        input.paths.config.clone(),
        input.paths.data_dir.clone(),
        input.paths.state_dir.clone(),
    ] {
        hash_tree(&path, &mut hasher, false);
    }
    let codex_home = if env::var_os("HOME").as_deref() == Some(input.paths.home.as_os_str()) {
        env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| input.paths.home.join(".codex"))
    } else {
        input.paths.home.join(".codex")
    };
    let claude_config = claude_config_dir(input.paths);
    let pi_agent = pi_agent_dir(input.paths);
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
        input.paths.home.join(".xi/AGENTS.md"),
    ] {
        hash_tree(&path, &mut hasher, false);
    }
    hash_dependency_tree(&claude_config.join("rules"), &mut hasher, false, true);
    if let Ok(main_checkout) = crate::git_worktree::worktree_root(&input.audit.directory)
        .and_then(|root| crate::git_worktree::main_worktree(&root))
    {
        hash_tree(
            &main_checkout.join(".claude/settings.local.json"),
            &mut hasher,
            false,
        );
    }
    if let Some(root) = &input.watch_root {
        hash_tree(root, &mut hasher, true);
    }
    hash_context_sources(input.audit, &mut hasher);
    hash_ancestor_candidates(
        &input.audit.directory,
        input.watch_root.as_deref(),
        &mut hasher,
    );
    hasher.finish()
}

/// Tracks the last accepted watch signature on the TUI session thread.
pub(super) struct ReloadWatcher {
    pub(super) signature: u64,
    last_watch: Instant,
}

impl ReloadWatcher {
    pub(super) fn new() -> Self {
        Self {
            signature: 0,
            last_watch: Instant::now(),
        }
    }

    /// Polling requests a refresh; the caller accepts a new signature after observing disk.
    pub(super) fn poll_reload(&mut self, input: &ReloadInput<'_>) -> bool {
        if self.last_watch.elapsed() < AUTO_RELOAD_INTERVAL {
            return false;
        }
        self.last_watch = Instant::now();
        current_watch_signature(input) != self.signature
    }

    pub(super) fn accept_reload(&mut self, signature: u64) {
        self.signature = signature;
        self.last_watch = Instant::now();
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::fixture;
    use super::super::*;
    use super::*;
    use std::fs;
    use std::process::Command;

    use tempfile::TempDir;

    #[test]
    fn watcher_ignores_runtime_session_churn() {
        for (transcript_path, instruction_path) in [
            (
                ".claude/projects/session/transcript.jsonl",
                ".claude/CLAUDE.md",
            ),
            (".xi/sessions/session/session.jsonl", ".xi/AGENTS.md"),
        ] {
            let (home, _repository, _paths, app) = fixture();
            let initial = current_watch_signature(&ReloadInput {
                audit: &app.inspection.context,
                paths: &app.paths,
                watch_root: &app.watch_root,
            });
            let transcript = home.path().join(transcript_path);
            fs::create_dir_all(transcript.parent().unwrap()).unwrap();
            fs::write(transcript, "session output\n").unwrap();
            assert_eq!(
                current_watch_signature(&ReloadInput {
                    audit: &app.inspection.context,
                    paths: &app.paths,
                    watch_root: &app.watch_root
                }),
                initial
            );

            let instruction = home.path().join(instruction_path);
            fs::write(instruction, "# Global instructions\n").unwrap();
            assert_ne!(
                current_watch_signature(&ReloadInput {
                    audit: &app.inspection.context,
                    paths: &app.paths,
                    watch_root: &app.watch_root
                }),
                initial
            );
        }
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
        let initial = current_watch_signature(&ReloadInput {
            audit: &app.inspection.context,
            paths: &app.paths,
            watch_root: &app.watch_root,
        });
        fs::write(ancestor, "# After\n").unwrap();
        assert_ne!(
            current_watch_signature(&ReloadInput {
                audit: &app.inspection.context,
                paths: &app.paths,
                watch_root: &app.watch_root
            }),
            initial
        );
    }

    #[test]
    fn watcher_discovers_new_cursor_rules() {
        use crate::context::SourceState;

        for git_repository in [false, true] {
            let home = TempDir::new().unwrap();
            let repository = TempDir::new().unwrap();
            if git_repository {
                assert!(
                    Command::new("git")
                        .args(["init", "-q"])
                        .current_dir(repository.path())
                        .status()
                        .unwrap()
                        .success()
                );
            }
            let rules = repository.path().join(".cursor/rules");
            fs::create_dir_all(&rules).unwrap();
            let candidates = vec![rules.join("new.mdc")];
            #[cfg(unix)]
            let (_external, candidates) = {
                use std::os::unix::fs::symlink;

                let mut candidates = candidates;
                let external = TempDir::new().unwrap();
                fs::create_dir_all(external.path().join("existing/deep")).unwrap();
                symlink(external.path(), rules.join("shared")).unwrap();
                symlink(external.path(), external.path().join("existing/cycle")).unwrap();
                candidates.push(rules.join("shared/added.mdc"));
                candidates.push(rules.join("shared/existing/deep/new.mdc"));
                (external, candidates)
            };
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
            let signature = |app: &App| {
                current_watch_signature(&ReloadInput {
                    audit: &app.inspection.context,
                    paths: &app.paths,
                    watch_root: &app.watch_root,
                })
            };
            let state = |app: &App, path: &Path| {
                app.context
                    .audit
                    .sources
                    .iter()
                    .find(|source| source.path == path)
                    .map(|source| source.state.clone())
            };
            for rule in candidates {
                for content in [
                    Some("---\nalwaysApply: true\n---\nNew rule\n"),
                    Some("---\nglobs: ['src/**']\n---\nChanged rule\n"),
                    None,
                ] {
                    let before = signature(&app);
                    let previous = state(&app, &rule);
                    if let Some(content) = content {
                        fs::write(&rule, content).unwrap();
                    } else {
                        fs::remove_file(&rule).unwrap();
                    }
                    assert_ne!(signature(&app), before, "undetected change: {rule:?}");
                    assert_eq!(state(&app, &rule), previous);
                    app.reload_from_disk();
                    match content {
                        Some(content) if content.contains("alwaysApply") => {
                            assert_eq!(state(&app, &rule), Some(SourceState::Startup));
                        }
                        Some(_) => assert!(matches!(
                            state(&app, &rule),
                            Some(SourceState::Conditional(_))
                        )),
                        None => assert_eq!(state(&app, &rule), None),
                    }
                    assert_eq!(app.watcher.signature, signature(&app));
                }
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn watcher_refreshes_linked_worktree_settings_and_external_claude_rules() {
        use crate::context::SourceState;
        use std::os::unix::fs::symlink;

        let home = TempDir::new().unwrap();
        let workspace = TempDir::new().unwrap();
        let main = workspace.path().join("main");
        let linked = workspace.path().join("linked");
        fs::create_dir(&main).unwrap();
        for arguments in [
            vec!["init", "-q"],
            vec![
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "--allow-empty",
                "-qm",
                "initial",
            ],
            vec!["worktree", "add", "-qb", "linked", linked.to_str().unwrap()],
        ] {
            assert!(
                Command::new("git")
                    .args(arguments)
                    .current_dir(&main)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let source = linked.join("CLAUDE.md");
        fs::write(&source, "shared").unwrap();
        fs::create_dir_all(main.join(".claude")).unwrap();
        fs::create_dir_all(linked.join(".claude/rules")).unwrap();
        let external = workspace.path().join("external");
        fs::create_dir_all(external.join("existing/deep")).unwrap();
        symlink(&external, linked.join(".claude/rules/shared")).unwrap();
        symlink(&external, external.join("existing/cycle")).unwrap();
        let unrelated = workspace.path().join("unrelated");
        fs::create_dir(&unrelated).unwrap();
        symlink(&unrelated, linked.join("ordinary-link")).unwrap();
        for ignored in [".git", "target", "node_modules"] {
            fs::create_dir_all(linked.join("controls").join(ignored).join("watch-control"))
                .unwrap();
        }

        let mut app = App::new_at(Paths::for_home(home.path()), None, linked.clone()).unwrap();
        let signature = |app: &App| {
            current_watch_signature(&ReloadInput {
                audit: &app.inspection.context,
                paths: &app.paths,
                watch_root: &app.watch_root,
            })
        };
        let state = |app: &App, path: &Path| {
            app.context
                .audit
                .sources
                .iter()
                .find(|source| source.path == path)
                .map(|source| source.state.clone())
        };
        assert_eq!(state(&app, &source), Some(SourceState::Startup));
        let initial = signature(&app);
        fs::write(unrelated.join("CLAUDE.md"), "unrelated").unwrap();
        for ignored in [".git", "target", "node_modules"] {
            fs::write(
                linked
                    .join("controls")
                    .join(ignored)
                    .join("watch-control/CLAUDE.md"),
                "ignored",
            )
            .unwrap();
        }
        assert_eq!(signature(&app), initial);
        let settings = main.join(".claude/settings.local.json");
        fs::write(
            &settings,
            serde_json::json!({"claudeMdExcludes": [source]}).to_string(),
        )
        .unwrap();
        assert_ne!(signature(&app), initial);
        assert_eq!(state(&app, &source), Some(SourceState::Startup));
        app.reload_from_disk();
        assert!(matches!(
            state(&app, &source),
            Some(SourceState::Excluded(_))
        ));
        assert_eq!(app.watcher.signature, signature(&app));

        let rule = linked.join(".claude/rules/shared/existing/deep/new.md");
        for content in [
            Some("new rule"),
            Some("---\npaths: ['src/**']\n---\nchanged rule"),
            None,
        ] {
            let before = signature(&app);
            let previous = state(&app, &rule);
            if let Some(content) = content {
                fs::write(&rule, content).unwrap();
            } else {
                fs::remove_file(&rule).unwrap();
            }
            assert_ne!(signature(&app), before);
            assert_eq!(state(&app, &rule), previous);
            app.reload_from_disk();
            match content {
                Some("new rule") => assert_eq!(state(&app, &rule), Some(SourceState::Startup)),
                Some(_) => assert!(matches!(
                    state(&app, &rule),
                    Some(SourceState::Conditional(_))
                )),
                None => assert_eq!(state(&app, &rule), None),
            }
            assert_eq!(app.watcher.signature, signature(&app));
        }
        let before = signature(&app);
        fs::remove_file(settings).unwrap();
        assert_ne!(signature(&app), before);
        app.reload_from_disk();
        assert_eq!(state(&app, &source), Some(SourceState::Startup));
    }

    #[test]
    fn non_git_watcher_tracks_candidates_without_scanning_the_directory_tree() {
        let home = TempDir::new().unwrap();
        let directory = home.path().join("workspace");
        fs::create_dir_all(directory.join("cache/nested")).unwrap();
        let app = App::new_at(Paths::for_home(home.path()), None, directory.clone()).unwrap();
        assert!(app.watch_root.is_none());
        let initial = current_watch_signature(&ReloadInput {
            audit: &app.inspection.context,
            paths: &app.paths,
            watch_root: &app.watch_root,
        });

        fs::write(directory.join("cache/nested/unrelated.txt"), "changed\n").unwrap();
        assert_eq!(
            current_watch_signature(&ReloadInput {
                audit: &app.inspection.context,
                paths: &app.paths,
                watch_root: &app.watch_root
            }),
            initial
        );

        fs::write(directory.join("AGENTS.md"), "# Instructions\n").unwrap();
        assert_ne!(
            current_watch_signature(&ReloadInput {
                audit: &app.inspection.context,
                paths: &app.paths,
                watch_root: &app.watch_root
            }),
            initial
        );
    }
}
