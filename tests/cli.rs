use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

fn setup() -> TempDir {
    let temp = TempDir::new().unwrap();
    let config = temp.path().join(".mdmanager");
    fs::create_dir_all(config.join("sections")).unwrap();
    fs::write(
        config.join("mdmanager.toml"),
        include_str!("fixtures/home/.mdmanager/mdmanager.toml"),
    )
    .unwrap();
    for (name, content) in [
        (
            "common.md",
            include_str!("fixtures/home/.mdmanager/sections/common.md"),
        ),
        (
            "claude.md",
            include_str!("fixtures/home/.mdmanager/sections/claude.md"),
        ),
        (
            "web.md",
            include_str!("fixtures/home/.mdmanager/sections/web.md"),
        ),
    ] {
        fs::write(config.join("sections").join(name), content).unwrap();
    }
    temp
}

fn mdmanager(home: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mdmanager"))
        .args(arguments)
        .env("HOME", home)
        .output()
        .unwrap()
}

fn mdmanager_in(home: &Path, directory: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mdmanager"))
        .args(arguments)
        .env("HOME", home)
        .current_dir(directory)
        .output()
        .unwrap()
}

fn mdmanager_data_dir(home: &Path) -> std::path::PathBuf {
    home.join(".mdmanager")
}

fn add_personal_section(home: &Path, id: &str, name: &str, content: &str) {
    let root = mdmanager_data_dir(home);
    let path = format!("sections/{id}.md");
    let mut manifest = fs::read_to_string(root.join("mdmanager.toml")).unwrap();
    manifest.push_str(&format!(
        "\n[[sections]]\nid = \"{id}\"\nname = \"{name}\"\npath = \"{path}\"\n"
    ));
    fs::write(root.join("mdmanager.toml"), manifest).unwrap();
    fs::write(root.join(path), content).unwrap();
}

#[test]
fn bare_mdmanager_and_tui_are_the_same_command() {
    let home = TempDir::new().unwrap();
    let bare = mdmanager(home.path(), &[]);
    let explicit = mdmanager(home.path(), &["tui"]);
    assert!(!bare.status.success());
    assert_eq!(bare.status, explicit.status);
    assert_eq!(bare.stderr, explicit.stderr);
    assert!(
        String::from_utf8(bare.stderr)
            .unwrap()
            .contains("the TUI requires an interactive terminal")
    );

    for command in ["tui1", "tui2"] {
        let removed = mdmanager(home.path(), &[command]);
        assert!(!removed.status.success());
        assert!(
            String::from_utf8(removed.stderr)
                .unwrap()
                .contains("unrecognized subcommand")
        );
    }
}

#[test]
fn init_without_targets_creates_only_the_personal_library() {
    let home = TempDir::new().unwrap();
    let output = mdmanager(home.path(), &["init"]);
    assert!(output.status.success());
    let config = home.path().join(".mdmanager/mdmanager.toml");
    let common = home.path().join(".mdmanager/sections/common.md");
    assert!(config.exists());
    assert!(common.exists());
    let manifest = fs::read_to_string(&config).unwrap();
    assert!(!manifest.contains("[targets."));
    assert!(!manifest.contains("[profiles."));
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("no Global targets")
    );
    assert!(!home.path().join(".claude/CLAUDE.md").exists());
    assert!(!home.path().join(".codex/AGENTS.md").exists());
    assert!(!home.path().join(".pi/agent/AGENTS.md").exists());

    let status = mdmanager(home.path(), &["status"]);
    assert!(status.status.success());
    let status = String::from_utf8(status.stdout).unwrap();
    assert!(status.contains("Global targets: none configured"));
    assert!(status.contains("Personal Section library: ready"));

    let doctor = mdmanager(home.path(), &["doctor"]);
    assert!(doctor.status.success());
    let doctor = String::from_utf8(doctor.stdout).unwrap();
    assert!(doctor.contains("Global targets: none configured"));
    assert!(doctor.contains("No problems found."));

    let repeated = mdmanager(home.path(), &["init"]);
    assert!(!repeated.status.success());
    assert!(
        String::from_utf8(repeated.stderr)
            .unwrap()
            .contains("refusing to overwrite")
    );
}

#[test]
fn init_creates_exactly_the_selected_global_targets() {
    let home = TempDir::new().unwrap();
    let output = mdmanager(home.path(), &["init", "claude", "codex"]);
    assert!(output.status.success());
    let manifest = fs::read_to_string(home.path().join(".mdmanager/mdmanager.toml")).unwrap();

    assert!(manifest.contains("[targets.claude]"));
    assert!(manifest.contains("[targets.codex]"));
    assert!(!manifest.contains("[targets.pi]"));
    assert!(manifest.contains("[profiles.default]"));
    assert!(manifest.contains("claude = [\"common\"]"));
    assert!(manifest.contains("codex = [\"common\"]"));
    assert!(!home.path().join(".claude/CLAUDE.md").exists());
    assert!(!home.path().join(".codex/AGENTS.md").exists());
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("Configured Global targets: claude, codex.")
    );
}

#[test]
fn init_rejects_cursor_as_a_global_target_with_project_guidance() {
    let home = TempDir::new().unwrap();
    let output = mdmanager(home.path(), &["init", "cursor"]);

    assert!(!output.status.success());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("Cursor has no Global Markdown target"));
    assert!(error.contains("mdmanager project adopt agents"));
    assert!(error.contains("mdmanager project create agents --from FILE"));
    assert!(error.contains("mdmanager context --runtime cursor"));
    assert!(!home.path().join(".mdmanager").exists());
}

#[test]
fn init_rejects_unknown_and_duplicate_global_targets_before_writing() {
    let unknown_home = TempDir::new().unwrap();
    let unknown = mdmanager(unknown_home.path(), &["init", "other"]);
    assert!(!unknown.status.success());
    assert!(
        String::from_utf8(unknown.stderr)
            .unwrap()
            .contains("expected claude, codex, or pi")
    );
    assert!(!unknown_home.path().join(".mdmanager").exists());

    let duplicate_home = TempDir::new().unwrap();
    let duplicate = mdmanager(duplicate_home.path(), &["init", "codex", "codex"]);
    assert!(!duplicate.status.success());
    assert!(
        String::from_utf8(duplicate.stderr)
            .unwrap()
            .contains("codex was given more than once")
    );
    assert!(!duplicate_home.path().join(".mdmanager").exists());
}

#[test]
fn doctor_describes_missing_configuration_without_calling_it_invalid() {
    let home = TempDir::new().unwrap();
    let output = mdmanager(home.path(), &["doctor"]);

    assert!(!output.status.success());
    let output = String::from_utf8(output.stdout).unwrap();
    assert!(output.contains("Global manifest: not configured"));
    assert!(output.contains("mdmanager init"));
    assert!(!output.contains("Global manifest: invalid"));
    assert!(!output.contains("repair the manifest"));
}

#[test]
fn non_interactive_apply_prints_the_plan_before_requiring_yes() {
    let home = setup();
    let output = mdmanager(home.path(), &["apply", "default"]);

    assert!(!output.status.success());
    let plan = String::from_utf8(output.stdout).unwrap();
    assert!(plan.contains("Profile: default"));
    assert!(plan.contains("not found"));
    assert!(plan.contains("+++ expected: default/claude"));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("non-interactive apply requires --yes")
    );
}

#[test]
fn root_help_is_concise_and_tui_help_has_the_keymap() {
    let output = Command::new(env!("CARGO_BIN_EXE_mdmanager"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("Create the Personal Section library and selected Global targets"));
    assert!(
        help.contains("Diagnose Global problems and repair invalid generated deployment state")
    );
    assert!(!help.contains("Shift+Up/Down"));

    let output = Command::new(env!("CARGO_BIN_EXE_mdmanager"))
        .args(["init", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("Usage: mdmanager init [GLOBAL_TARGET]..."));
    assert!(help.contains("Available Global targets: claude, codex, pi"));
    assert!(help.contains("Cursor uses project AGENTS.md"));

    let output = Command::new(env!("CARGO_BIN_EXE_mdmanager"))
        .args(["tui", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("Run `mdmanager` or `mdmanager tui`"));
    assert!(help.contains("mdmanager docs migrate"));
    assert!(help.contains("Shift+Up/Down     scroll the visible document by a page"));
    assert!(help.contains("r                 search and choose a Context runtime"));
    assert!(help.contains("Esc               go back; from the overview, quit"));
}

#[test]
fn bundled_docs_work_without_configuration() {
    let home = TempDir::new().unwrap();
    let index = mdmanager(home.path(), &["docs"]);
    assert!(index.status.success());
    let index = String::from_utf8(index.stdout).unwrap();
    assert!(index.contains("start"));
    assert!(index.contains("configuration"));
    assert!(index.contains("cursor"));
    assert!(index.contains("migrate"));

    let cursor = mdmanager(home.path(), &["docs", "cursor"]);
    assert!(cursor.status.success());
    let cursor = String::from_utf8(cursor.stdout).unwrap();
    assert!(cursor.starts_with("# Cursor context"));
    assert!(cursor.contains("mdmanager context --runtime cursor"));

    let migrate = mdmanager(home.path(), &["docs", "migrate"]);
    assert!(migrate.status.success());
    let migrate = String::from_utf8(migrate.stdout).unwrap();
    assert!(migrate.starts_with("# Migrate existing instructions"));
    assert!(migrate.contains("Do not run `mdmanager apply`"));
    assert!(migrate.contains("mdmanager render PROFILE TARGET"));

    let start = mdmanager(home.path(), &["docs", "start"]);
    assert!(start.status.success());
    let start = String::from_utf8(start.stdout).unwrap();
    assert!(start.contains("mdmanager docs migrate"));
    assert!(start.contains("mdmanager context --runtime claude"));
    assert!(start.contains("Project and Local Instructions require a Git worktree"));
    assert!(start.contains("mdmanager project create TARGET --from FILE --yes"));
    assert!(start.contains("mdmanager local create TARGET SECTION"));
    assert!(start.contains("mdmanager apply PROFILE --yes"));

    let tui = mdmanager(home.path(), &["docs", "tui"]);
    assert!(tui.status.success());
    let tui = String::from_utf8(tui.stdout).unwrap();
    assert!(tui.contains("terminal interface"));
    assert!(tui.contains("Up/Down            select, or scroll one line"));
    assert!(tui.contains("choose the Context runtime"));
    assert!(
        tui.contains("The TUI has no editing or Apply mode") || tui.contains("no Apply action")
    );
    assert!(tui.contains("Profiles & Sections"));
    assert!(tui.contains("not currently used"));

    let missing = mdmanager(home.path(), &["docs", "missing"]);
    assert!(!missing.status.success());
    assert!(
        String::from_utf8(missing.stderr)
            .unwrap()
            .contains("run `mdmanager docs`")
    );
}

#[test]
fn context_reports_rules_imports_exclusions_and_json() {
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
    fs::create_dir_all(repository.path().join(".claude/rules")).unwrap();
    fs::create_dir_all(repository.path().join("docs")).unwrap();
    fs::create_dir_all(repository.path().join(".pi")).unwrap();
    fs::write(
        repository.path().join("CLAUDE.md"),
        "# Project\n\nSee @./docs/testing.md but not `@docs/ignored.md`.\n",
    )
    .unwrap();
    fs::write(repository.path().join("docs/testing.md"), "# Testing\n").unwrap();
    let rule = repository.path().join(".claude/rules/tests.md");
    fs::write(&rule, "---\npaths: ['tests/**']\n---\n# Test rules\n").unwrap();
    fs::write(
        repository.path().join(".claude/settings.local.json"),
        format!(
            "{{\"claudeMdExcludes\":[{}]}}",
            serde_json::to_string(&rule.display().to_string()).unwrap()
        ),
    )
    .unwrap();
    fs::write(repository.path().join(".pi/SYSTEM.md"), "# Out of scope\n").unwrap();

    fs::write(
        repository.path().join(".claude/rules/incomplete.md"),
        "---\nalwaysApply: true\npaths: ['tests/**']\n",
    )
    .unwrap();

    let output = mdmanager_in(
        home.path(),
        repository.path(),
        &["context", "--runtime", "claude"],
    );
    assert!(output.status.success());
    let output = String::from_utf8(output.stdout).unwrap();
    assert!(output.contains("CLAUDE.md"));
    assert!(output.contains(".claude/rules/tests.md"));
    assert!(output.contains("excluded"));
    assert!(output.contains("imports ./docs/testing.md"));
    assert!(!output.contains("ignored.md"));

    let json = mdmanager_in(
        home.path(),
        repository.path(),
        &["context", "--runtime", "claude", "--json"],
    );
    assert!(json.status.success());
    let json: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert!(json["sources"].as_array().unwrap().iter().any(|source| {
        source["display"] == "./.claude/rules/incomplete.md"
            && source["status"] == "uncertain"
            && source["reason"]
                .as_str()
                .is_some_and(|reason| !reason.is_empty())
    }));
    assert_eq!(json["runtime"], "claude");
    assert!(json["summary"].as_str().is_some());
    assert!(json["sources"].as_array().is_some_and(|sources| {
        sources
            .iter()
            .any(|source| source["display"] == "./.claude/rules/tests.md")
    }));

    let pi = mdmanager_in(
        home.path(),
        repository.path(),
        &["context", "--runtime", "pi"],
    );
    assert!(pi.status.success());
    assert!(!String::from_utf8(pi.stdout).unwrap().contains("SYSTEM.md"));
}

#[test]
fn cursor_context_reports_agents_and_cursor_rules() {
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
    fs::write(
        repository.path().join("AGENTS.md"),
        "# Cursor instructions\n",
    )
    .unwrap();
    fs::write(
        repository.path().join(".cursor/rules/rust.mdc"),
        "---\nglobs: ['**/*.rs']\nalwaysApply: false\n---\nRust rule\n",
    )
    .unwrap();

    fs::write(
        repository.path().join(".cursor/rules/incomplete.mdc"),
        "---\nalwaysApply: true\npaths: ['tests/**']\n",
    )
    .unwrap();

    let output = mdmanager_in(
        home.path(),
        repository.path(),
        &["context", "--runtime", "cursor", "--json"],
    );

    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(json["sources"].as_array().unwrap().iter().any(|source| {
        source["display"] == "./.cursor/rules/incomplete.mdc"
            && source["status"] == "uncertain"
            && source["reason"]
                .as_str()
                .is_some_and(|reason| !reason.is_empty())
    }));
    assert_eq!(json["runtime"], "cursor");
    assert!(
        json["summary"]
            .as_str()
            .is_some_and(|summary| summary.contains("User and Team Rules"))
    );
    assert!(json["sources"].as_array().is_some_and(|sources| {
        sources
            .iter()
            .any(|source| source["display"] == "./AGENTS.md")
            && sources
                .iter()
                .any(|source| source["display"] == "./.cursor/rules/rust.mdc")
    }));
}

#[test]
fn context_remains_available_when_the_global_manifest_is_invalid() {
    let home = setup();
    fs::write(home.path().join(".mdmanager/mdmanager.toml"), "[").unwrap();

    let output = mdmanager_in(home.path(), home.path(), &["context", "--runtime", "codex"]);

    assert!(output.status.success());
    assert!(String::from_utf8(output.stdout).unwrap().contains("Codex"));
}

#[test]
fn context_honors_claude_and_pi_user_directories() {
    let home = TempDir::new().unwrap();
    let repository = TempDir::new().unwrap();
    let claude_dir = home.path().join("claude-config");
    let pi_dir = home.path().join("pi-config");
    fs::create_dir_all(&claude_dir).unwrap();
    fs::create_dir_all(&pi_dir).unwrap();
    fs::write(claude_dir.join("CLAUDE.md"), "custom Claude").unwrap();
    fs::write(pi_dir.join("AGENTS.md"), "custom Pi").unwrap();

    let claude = Command::new(env!("CARGO_BIN_EXE_mdmanager"))
        .args(["context", "--runtime", "claude", "--json"])
        .env("HOME", home.path())
        .env("CLAUDE_CONFIG_DIR", &claude_dir)
        .current_dir(repository.path())
        .output()
        .unwrap();
    assert!(claude.status.success());
    let claude: serde_json::Value = serde_json::from_slice(&claude.stdout).unwrap();
    assert!(claude["sources"].as_array().is_some_and(|sources| {
        sources.iter().any(|source| {
            source["path"].as_str() == Some(claude_dir.join("CLAUDE.md").to_str().unwrap())
        })
    }));

    let pi = Command::new(env!("CARGO_BIN_EXE_mdmanager"))
        .args(["context", "--runtime", "pi", "--json"])
        .env("HOME", home.path())
        .env("PI_CODING_AGENT_DIR", &pi_dir)
        .current_dir(repository.path())
        .output()
        .unwrap();
    assert!(pi.status.success());
    let pi: serde_json::Value = serde_json::from_slice(&pi.stdout).unwrap();
    assert!(pi["sources"].as_array().is_some_and(|sources| {
        sources.iter().any(|source| {
            source["path"].as_str() == Some(pi_dir.join("AGENTS.md").to_str().unwrap())
        })
    }));
}

#[test]
fn codex_context_reports_invalid_configuration_in_human_and_json_output() {
    let home = TempDir::new().unwrap();
    let repository = TempDir::new().unwrap();
    fs::create_dir_all(home.path().join(".codex")).unwrap();
    fs::write(home.path().join(".codex/config.toml"), "invalid = [").unwrap();

    let human = mdmanager_in(
        home.path(),
        repository.path(),
        &["context", "--runtime", "codex"],
    );
    assert!(human.status.success());
    assert!(
        String::from_utf8(human.stdout)
            .unwrap()
            .contains("Warning: invalid")
    );

    let json = mdmanager_in(
        home.path(),
        repository.path(),
        &["context", "--runtime", "codex", "--json"],
    );
    assert!(json.status.success());
    let json: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert!(json.get("warnings").is_none());
    assert!(
        json["summary"]
            .as_str()
            .is_some_and(|summary| summary.contains("Warning: invalid"))
    );
}

#[test]
fn render_apply_status_and_modified_recovery() {
    let home = setup();
    let rendered = mdmanager(home.path(), &["render", "default", "claude"]);
    assert!(rendered.status.success());
    assert_eq!(
        String::from_utf8(rendered.stdout).unwrap(),
        "# Global Claude\n\n### Focused Changes\n\nTouch only what the task requires.\n\n### Claude\n\nUse ordinary chat text for questions.\n"
    );

    let initial = mdmanager(home.path(), &["status"]);
    assert!(!initial.status.success());
    assert!(
        String::from_utf8(initial.stderr)
            .unwrap()
            .contains("no active profile")
    );

    let needs_yes = mdmanager(home.path(), &["apply", "default"]);
    assert!(!needs_yes.status.success());
    assert!(
        String::from_utf8(needs_yes.stdout)
            .unwrap()
            .contains("Profile: default")
    );
    assert!(
        String::from_utf8(needs_yes.stderr)
            .unwrap()
            .contains("requires --yes")
    );

    let applied = mdmanager(home.path(), &["apply", "default", "--yes"]);
    assert!(
        applied.status.success(),
        "{}",
        String::from_utf8_lossy(&applied.stderr)
    );
    let current = mdmanager(home.path(), &["status"]);
    assert!(current.status.success());
    assert!(
        String::from_utf8(current.stdout)
            .unwrap()
            .contains("current")
    );
    let no_op = mdmanager(home.path(), &["apply", "--yes"]);
    assert!(no_op.status.success());
    let no_op = String::from_utf8(no_op.stdout).unwrap();
    assert_eq!(no_op.matches("Agents").count(), 1);
    assert_eq!(no_op.matches("Claude").count(), 1);
    assert!(no_op.contains("All targets are current."));

    fs::write(
        home.path().join(".mdmanager/sections/common.md"),
        "### Common\n\nChanged source.\n",
    )
    .unwrap();
    let difference = mdmanager(home.path(), &["diff", "default", "claude"]);
    assert!(difference.status.success());
    let difference = String::from_utf8(difference.stdout).unwrap();
    assert!(difference.contains("---"));
    assert!(difference.contains("+++"));
    let out_of_sync = mdmanager(home.path(), &["status"]);
    assert!(!out_of_sync.status.success());
    let out_of_sync = String::from_utf8(out_of_sync.stdout).unwrap();
    assert!(out_of_sync.contains("out of sync"));
    assert!(out_of_sync.contains("run `mdmanager apply`"));
    assert!(mdmanager(home.path(), &["apply", "--yes"]).status.success());

    let target = home.path().join(".claude/CLAUDE.md");
    fs::write(&target, "manual edit\n").unwrap();
    let refused = mdmanager(home.path(), &["apply", "--yes"]);
    assert!(!refused.status.success());
    assert!(
        String::from_utf8(refused.stderr)
            .unwrap()
            .contains("--force")
    );
    let forced = mdmanager(home.path(), &["apply", "--yes", "--force"]);
    assert!(forced.status.success());
    assert_eq!(
        fs::read_to_string(mdmanager_data_dir(home.path()).join("backups/claude.bak")).unwrap(),
        "manual edit\n"
    );
    assert!(mdmanager(home.path(), &["status"]).status.success());
}

#[test]
fn doctor_reports_a_removed_active_profile_and_the_repair_command() {
    let home = setup();
    let applied = mdmanager(home.path(), &["apply", "default", "--yes"]);
    assert!(applied.status.success());

    let manifest_path = home.path().join(".mdmanager/mdmanager.toml");
    let manifest = fs::read_to_string(&manifest_path).unwrap();
    fs::write(
        &manifest_path,
        manifest.replace("[profiles.default]", "[profiles.repos]"),
    )
    .unwrap();

    let output = mdmanager(home.path(), &["doctor"]);
    assert!(!output.status.success());
    assert!(output.stderr.is_empty());
    let output = String::from_utf8(output.stdout).unwrap();
    assert!(output.contains("Global manifest: valid"));
    assert!(output.contains("active Profile `default` no longer exists"));
    assert!(output.contains("Available Profiles: repos, work"));
    assert!(output.contains("Detected Profile: repos (all target files match)"));
    assert!(output.contains("mdmanager apply repos"));

    let status = mdmanager(home.path(), &["status"]);
    assert!(!status.status.success());
    assert!(
        String::from_utf8(status.stderr)
            .unwrap()
            .contains("run `mdmanager doctor`")
    );

    let repaired = mdmanager(home.path(), &["apply", "repos"]);
    assert!(repaired.status.success());
    let healthy = mdmanager(home.path(), &["doctor"]);
    assert!(healthy.status.success());
    assert!(
        String::from_utf8(healthy.stdout)
            .unwrap()
            .contains("No problems found.")
    );
}

#[test]
fn doctor_repairs_invalid_generated_state_without_changing_targets() {
    let home = setup();
    assert!(
        mdmanager(home.path(), &["apply", "default", "--yes"])
            .status
            .success()
    );
    let targets = [
        home.path().join(".claude/CLAUDE.md"),
        home.path().join(".codex/AGENTS.md"),
    ];
    let before = targets
        .iter()
        .map(|path| fs::read(path).unwrap())
        .collect::<Vec<_>>();
    let state = home.path().join(".mdmanager/state/state.toml");
    fs::write(&state, "active_profile = [").unwrap();

    let repaired = mdmanager(home.path(), &["doctor"]);
    assert!(repaired.status.success());
    let output = String::from_utf8(repaired.stdout).unwrap();
    assert!(output.contains("Deployment state: invalid"));
    assert!(output.contains("Repair: rebuilt generated state for detected Profile default"));
    assert!(output.contains("No problems found"));
    assert!(mdmanager(home.path(), &["doctor"]).status.success());
    for (path, expected) in targets.iter().zip(before) {
        assert_eq!(fs::read(path).unwrap(), expected);
    }
}

#[test]
fn doctor_clears_invalid_state_when_no_profile_matches() {
    let home = setup();
    assert!(
        mdmanager(home.path(), &["apply", "default", "--yes"])
            .status
            .success()
    );
    fs::write(home.path().join(".codex/AGENTS.md"), "manual\n").unwrap();
    fs::write(
        home.path().join(".mdmanager/state/state.toml"),
        "active_profile = [",
    )
    .unwrap();

    let repaired = mdmanager(home.path(), &["doctor"]);
    assert!(!repaired.status.success());
    let output = String::from_utf8(repaired.stdout).unwrap();
    assert!(output.contains("Matching Profiles: none"));
    assert!(output.contains("Repair: cleared invalid generated ownership state"));
    assert!(output.contains("mdmanager apply PROFILE"));

    let review = mdmanager(home.path(), &["apply", "default"]);
    assert!(!review.status.success());
    assert!(
        String::from_utf8(review.stdout)
            .unwrap()
            .contains("existing · not managed by mdmanager.ai")
    );
    assert_eq!(
        fs::read_to_string(home.path().join(".codex/AGENTS.md")).unwrap(),
        "manual\n"
    );
}

#[test]
fn doctor_gives_a_safe_action_for_a_directory_target() {
    let home = setup();
    assert!(
        mdmanager(home.path(), &["apply", "default", "--yes"])
            .status
            .success()
    );
    let target = home.path().join(".codex/AGENTS.md");
    fs::remove_file(&target).unwrap();
    fs::create_dir(&target).unwrap();

    let output = mdmanager(home.path(), &["doctor"]);
    assert!(!output.status.success());
    let output = String::from_utf8(output.stdout).unwrap();
    assert!(output.contains("inspect it and move it out of the target path"));
    assert!(output.contains("Action: resolve the reported target path"));
    assert!(!output.contains("remove it before applying"));
}

#[test]
fn project_adopt_render_check_and_apply() {
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
    let original = "# Existing\n\nKeep these bytes.\n";
    fs::write(repository.path().join("AGENTS.md"), original).unwrap();
    fs::write(
        repository.path().join("CLAUDE.local.md"),
        "tracked local file\n",
    )
    .unwrap();
    assert!(
        Command::new("git")
            .args(["add", "CLAUDE.local.md"])
            .current_dir(repository.path())
            .status()
            .unwrap()
            .success()
    );

    let adopted = mdmanager_in(
        home.path(),
        repository.path(),
        &["project", "adopt", "agents", "--yes"],
    );
    assert!(
        adopted.status.success(),
        "{}",
        String::from_utf8_lossy(&adopted.stderr)
    );
    assert_eq!(
        fs::read_to_string(repository.path().join("AGENTS.md")).unwrap(),
        original
    );
    assert_eq!(
        fs::read_to_string(repository.path().join(".mdmanager/sections/agents.md")).unwrap(),
        original
    );
    assert!(!repository.path().join(".gitignore").exists());

    let rendered = mdmanager_in(
        home.path(),
        repository.path(),
        &["project", "render", "agents"],
    );
    assert!(rendered.status.success());
    assert_eq!(String::from_utf8(rendered.stdout).unwrap(), original);
    assert!(
        mdmanager_in(home.path(), repository.path(), &["project", "check"])
            .status
            .success()
    );

    fs::write(
        repository.path().join(".mdmanager/sections/agents.md"),
        "# Changed\n",
    )
    .unwrap();
    let difference = mdmanager_in(
        home.path(),
        repository.path(),
        &["project", "diff", "agents"],
    );
    assert!(difference.status.success());
    let difference = String::from_utf8(difference.stdout).unwrap();
    assert!(difference.contains("---"));
    assert!(difference.contains("+++"));
    assert!(
        !mdmanager_in(home.path(), repository.path(), &["project", "check"])
            .status
            .success()
    );
    assert!(
        mdmanager_in(
            home.path(),
            repository.path(),
            &["project", "apply", "agents", "--yes"],
        )
        .status
        .success()
    );
    assert_eq!(
        fs::read_to_string(repository.path().join("AGENTS.md")).unwrap(),
        "# Changed\n"
    );
}

#[test]
fn project_check_explains_an_empty_manifest() {
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
    fs::create_dir_all(repository.path().join(".mdmanager")).unwrap();
    fs::write(
        repository.path().join(".mdmanager/project.toml"),
        "format = 1\n",
    )
    .unwrap();

    let output = mdmanager_in(home.path(), repository.path(), &["project", "check"]);

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        "No project targets declared."
    );
}

#[test]
fn local_commands_use_a_plain_non_git_error() {
    let home = TempDir::new().unwrap();
    let directory = TempDir::new().unwrap();

    let output = mdmanager_in(home.path(), directory.path(), &["local", "status"]);

    assert!(!output.status.success());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("local management requires a Git worktree"));
    assert!(!error.contains("git rev-parse"));
    assert!(!error.contains("fatal:"));
}

#[test]
fn project_create_authors_management_then_applies_the_root_file() {
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
    let draft = repository.path().join("draft.md");
    fs::write(&draft, "# New project instructions\n").unwrap();

    let created = mdmanager_in(
        home.path(),
        repository.path(),
        &[
            "project",
            "create",
            "agents",
            "--from",
            draft.to_str().unwrap(),
            "--yes",
        ],
    );
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    assert!(!repository.path().join("AGENTS.md").exists());
    assert_eq!(
        fs::read_to_string(repository.path().join(".mdmanager/sections/agents.md")).unwrap(),
        "# New project instructions\n"
    );
    assert!(!repository.path().join(".gitignore").exists());

    let applied = mdmanager_in(
        home.path(),
        repository.path(),
        &["project", "apply", "agents", "--yes"],
    );
    assert!(applied.status.success());
    assert_eq!(
        fs::read_to_string(repository.path().join("AGENTS.md")).unwrap(),
        "# New project instructions\n"
    );
}

#[test]
fn project_create_allows_tracked_optional_local_override_paths() {
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
    fs::write(repository.path().join("AGENTS.override.md"), "external\n").unwrap();
    fs::write(repository.path().join(".gitignore"), "/target\n").unwrap();
    assert!(
        Command::new("git")
            .args(["add", "AGENTS.override.md"])
            .current_dir(repository.path())
            .status()
            .unwrap()
            .success()
    );
    let draft = repository.path().join("draft.md");
    fs::write(&draft, "# Project instructions\n").unwrap();

    let output = mdmanager_in(
        home.path(),
        repository.path(),
        &[
            "project",
            "create",
            "agents",
            "--from",
            draft.to_str().unwrap(),
            "--yes",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(repository.path().join(".mdmanager/project.toml").is_file());
    assert_eq!(
        fs::read_to_string(repository.path().join("AGENTS.override.md")).unwrap(),
        "external\n"
    );
    assert_eq!(
        fs::read_to_string(repository.path().join(".gitignore")).unwrap(),
        "/target\n"
    );
}

#[test]
fn managed_local_override_is_machine_owned_and_hash_guarded() {
    let home = setup();
    let repository = TempDir::new().unwrap();
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(repository.path())
            .status()
            .unwrap()
            .success()
    );
    fs::write(repository.path().join("AGENTS.md"), "# Shared\n").unwrap();
    add_personal_section(home.path(), "local", "Local instructions", "# Shared\n");
    assert!(
        mdmanager_in(
            home.path(),
            repository.path(),
            &["project", "adopt", "agents", "--yes"],
        )
        .status
        .success()
    );
    let exclude = repository.path().join(".git/info/exclude");
    let exclude_before = fs::read_to_string(&exclude).unwrap();

    let created = mdmanager_in(
        home.path(),
        repository.path(),
        &["local", "create", "agents", "local"],
    );
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    assert!(!repository.path().join("AGENTS.override.md").exists());
    assert_eq!(fs::read_to_string(&exclude).unwrap(), exclude_before);
    let difference = mdmanager_in(home.path(), repository.path(), &["local", "diff", "agents"]);
    assert!(difference.status.success());
    let difference = String::from_utf8(difference.stdout).unwrap();
    assert!(difference.contains("---"));
    assert!(difference.contains("+++"));
    let data_projects = mdmanager_data_dir(home.path()).join("projects");
    assert_eq!(fs::read_dir(&data_projects).unwrap().count(), 1);
    let status = mdmanager_in(home.path(), repository.path(), &["local", "status"]);
    let status = String::from_utf8(status.stdout).unwrap();
    assert!(status.contains("Section local:"));
    assert!(status.contains("sections/local.md"));
    assert!(!status.contains("Identity:"));

    let repeated = mdmanager_in(
        home.path(),
        repository.path(),
        &["local", "create", "agents", "local"],
    );
    assert!(!repeated.status.success());
    assert!(
        String::from_utf8(repeated.stderr)
            .unwrap()
            .contains("managed agents instructions already exist")
    );

    let applied = mdmanager_in(
        home.path(),
        repository.path(),
        &["local", "apply", "agents", "--yes"],
    );
    assert!(
        applied.status.success(),
        "{}",
        String::from_utf8_lossy(&applied.stderr)
    );
    let applied_output = String::from_utf8(applied.stdout).unwrap();
    let exclusion_plan = format!("  /AGENTS.override.md in {}", exclude.display());
    assert_eq!(
        applied_output
            .lines()
            .filter(|line| *line == exclusion_plan)
            .count(),
        1
    );
    assert_eq!(
        fs::read_to_string(repository.path().join("AGENTS.override.md")).unwrap(),
        "# Shared\n"
    );
    assert!(
        Command::new("git")
            .args(["check-ignore", "-q", "AGENTS.override.md"])
            .current_dir(repository.path())
            .status()
            .unwrap()
            .success()
    );

    fs::write(repository.path().join("AGENTS.override.md"), "manual\n").unwrap();
    let refused = mdmanager_in(
        home.path(),
        repository.path(),
        &["local", "apply", "agents", "--yes"],
    );
    assert!(!refused.status.success());
    let error = String::from_utf8(refused.stderr).unwrap();
    assert!(error.contains("refusing to overwrite"));
    assert!(error.contains("sections/local.md"));
    assert!(error.contains("move intended edits into that Section"));
}

#[test]
fn managed_claude_local_instructions_are_additive() {
    let home = setup();
    let repository = TempDir::new().unwrap();
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(repository.path())
            .status()
            .unwrap()
            .success()
    );
    fs::write(repository.path().join("CLAUDE.md"), "# Shared Claude\n").unwrap();
    add_personal_section(
        home.path(),
        "claude-local",
        "Local Claude instructions",
        "# Local Claude instructions\n",
    );

    let created = mdmanager_in(
        home.path(),
        repository.path(),
        &["local", "create", "claude", "claude-local"],
    );
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    assert!(
        String::from_utf8(created.stdout)
            .unwrap()
            .contains("personal Section claude-local")
    );
    assert!(!repository.path().join("CLAUDE.local.md").exists());

    let applied = mdmanager_in(
        home.path(),
        repository.path(),
        &["local", "apply", "claude", "--yes"],
    );
    assert!(
        applied.status.success(),
        "{}",
        String::from_utf8_lossy(&applied.stderr)
    );
    assert_eq!(
        fs::read_to_string(repository.path().join("CLAUDE.local.md")).unwrap(),
        "# Local Claude instructions\n"
    );
    assert!(!repository.path().join("AGENTS.override.md").exists());

    let context = mdmanager_in(
        home.path(),
        repository.path(),
        &["context", "--runtime", "claude"],
    );
    assert!(context.status.success());
    let context = String::from_utf8(context.stdout).unwrap();
    assert!(context.contains("CLAUDE.md"));
    assert!(context.contains("CLAUDE.local.md"));
}

#[test]
fn managed_local_apply_preserves_a_preexisting_ignore_rule() {
    let home = setup();
    let repository = TempDir::new().unwrap();
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(repository.path())
            .status()
            .unwrap()
            .success()
    );
    fs::write(
        repository.path().join(".gitignore"),
        "/AGENTS.override.md\n",
    )
    .unwrap();
    add_personal_section(home.path(), "local", "Local instructions", "# Local\n");
    assert!(
        mdmanager_in(
            home.path(),
            repository.path(),
            &["local", "create", "agents", "local"],
        )
        .status
        .success()
    );
    let exclude = repository.path().join(".git/info/exclude");
    let before = fs::read_to_string(&exclude).unwrap();

    let applied = mdmanager_in(
        home.path(),
        repository.path(),
        &["local", "apply", "agents", "--yes"],
    );

    assert!(applied.status.success());
    assert_eq!(fs::read_to_string(exclude).unwrap(), before);
    assert_eq!(
        fs::read_to_string(repository.path().join(".gitignore")).unwrap(),
        "/AGENTS.override.md\n"
    );
}

#[test]
fn pi_disable_adds_and_restores_its_exclusion() {
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
    fs::write(repository.path().join("AGENTS.md"), "# Shared\n").unwrap();
    let exclude = repository.path().join(".git/info/exclude");

    let disabled = mdmanager_in(
        home.path(),
        repository.path(),
        &["local", "disable", "pi", "--yes"],
    );

    assert!(
        disabled.status.success(),
        "{}",
        String::from_utf8_lossy(&disabled.stderr)
    );
    let plan = String::from_utf8(disabled.stdout).unwrap();
    let exclusion_plan = format!("  /AGENTS.override.md in {}", exclude.display());
    assert_eq!(
        plan.lines().filter(|line| *line == exclusion_plan).count(),
        1
    );
    assert!(
        fs::read_to_string(&exclude)
            .unwrap()
            .lines()
            .any(|line| line == "/AGENTS.override.md")
    );

    assert!(
        mdmanager_in(
            home.path(),
            repository.path(),
            &["local", "restore", "pi", "--yes"],
        )
        .status
        .success()
    );
    assert!(!repository.path().join("AGENTS.override.md").exists());
    assert!(
        !fs::read_to_string(exclude)
            .unwrap()
            .lines()
            .any(|line| line == "/AGENTS.override.md")
    );
}

#[test]
fn claude_disable_preserves_source_and_restores_owned_settings() {
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
    let source = "# Shared Claude\n";
    fs::write(repository.path().join("CLAUDE.md"), source).unwrap();
    assert!(
        mdmanager_in(
            home.path(),
            repository.path(),
            &["project", "adopt", "claude", "--yes"],
        )
        .status
        .success()
    );

    let disabled = mdmanager_in(
        home.path(),
        repository.path(),
        &["local", "disable", "claude", "--yes"],
    );
    assert!(
        disabled.status.success(),
        "{}",
        String::from_utf8_lossy(&disabled.stderr)
    );
    let disabled_output = String::from_utf8(disabled.stdout).unwrap();
    let exclude = repository.path().join(".git/info/exclude");
    let exclusion_plan = format!("  /.claude/settings.local.json in {}", exclude.display());
    assert_eq!(
        disabled_output
            .lines()
            .filter(|line| *line == exclusion_plan)
            .count(),
        1
    );
    assert_eq!(
        fs::read_to_string(repository.path().join("CLAUDE.md")).unwrap(),
        source
    );
    let settings =
        fs::read_to_string(repository.path().join(".claude/settings.local.json")).unwrap();
    assert!(settings.contains("claudeMdExcludes"));
    assert!(settings.contains(&repository.path().join("CLAUDE.md").display().to_string()));

    let restored = mdmanager_in(
        home.path(),
        repository.path(),
        &["local", "restore", "claude", "--yes"],
    );
    assert!(restored.status.success());
    assert!(
        !repository
            .path()
            .join(".claude/settings.local.json")
            .exists()
    );
    assert_eq!(
        fs::read_to_string(repository.path().join("CLAUDE.md")).unwrap(),
        source
    );
    assert!(
        !fs::read_to_string(exclude)
            .unwrap()
            .lines()
            .any(|line| line == "/.claude/settings.local.json")
    );
}

#[cfg(unix)]
#[test]
fn claude_disable_refuses_a_claude_symlink_to_agents() {
    use std::os::unix::fs::symlink;

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
    fs::write(repository.path().join("AGENTS.md"), "# Shared\n").unwrap();
    symlink("AGENTS.md", repository.path().join("CLAUDE.md")).unwrap();

    let disabled = mdmanager_in(
        home.path(),
        repository.path(),
        &["local", "disable", "claude", "--yes"],
    );

    assert!(!disabled.status.success());
    let error = String::from_utf8(disabled.stderr).unwrap();
    assert!(error.contains("CLAUDE.md"));
    assert!(error.contains("symlinked source"));
    assert_eq!(
        fs::read_to_string(repository.path().join("AGENTS.md")).unwrap(),
        "# Shared\n"
    );
    assert!(
        !repository
            .path()
            .join(".claude/settings.local.json")
            .exists()
    );
}
