use std::fs;
use std::io::{self, IsTerminal, Write};
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use crate::config::{GlobalConfig, Paths, target_display_name};
use crate::context::{Audit, ContextRuntime, SourceGroup};
use crate::deploy::{self, GlobalTargetStatus};
use crate::local;
use crate::project;
use crate::scaffold;

const TUI_HELP: &str = "Documentation:\n  Run `mdmanager docs` to list the bundled guides.\n  Run `mdmanager docs start` for the agent-driven workflow or `mdmanager docs migrate` for migration.\n\nTUI:\n  Run `mdmanager` or `mdmanager tui` to browse files, Context, status, and differences.\n  Up/Down           select or scroll one line\n  Shift+Up/Down     scroll the visible document by a page\n  Enter             inspect the selection\n  r                 search and choose a Context runtime\n  t                 search, preview, and save the UI theme\n  Left/Right        choose the previous/next Context runtime\n  /                 search an open document\n  g                 go to a source line\n  d                 toggle a meaningful managed-target difference\n  Esc               go back; from the overview, quit\n  ?                 contextual help\n  q or Ctrl+C       quit\n\nCoding agents edit instruction sources and manifests. The TUI only writes its theme preference; management and deployment operations use the CLI while it auto-reloads.";

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Manage and audit CLAUDE.md and AGENTS.md instructions"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Read bundled documentation.
    Docs {
        /// Exact documentation topic name.
        topic: Option<String>,
    },
    /// Create the Personal Section library and selected Global targets without deploying.
    #[command(
        long_about = "Create the Personal Section library and selected Global targets without deploying.\n\nAvailable Global targets: claude, codex, pi. Pass none to create only the Personal Section library. Cursor uses project AGENTS.md and has no Global target."
    )]
    Init {
        /// Global targets to configure: claude, codex, pi.
        #[arg(value_name = "GLOBAL_TARGET")]
        targets: Vec<String>,
    },
    /// Open the terminal interface (also the default with no command).
    #[command(after_long_help = TUI_HELP)]
    Tui,
    /// Show the Markdown instruction files a runtime would load.
    Context {
        /// Runtime to inspect: claude, codex, cursor, or pi.
        #[arg(long, default_value = "claude")]
        runtime: String,
        /// Emit stable machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Manage committed project instruction files.
    Project {
        #[command(subcommand)]
        command: ProjectCommand,
    },
    /// Manage machine-local instruction files and disables.
    Local {
        #[command(subcommand)]
        command: LocalCommand,
    },
    /// Validate configuration and report deployment status.
    Status,
    /// Diagnose Global problems and repair invalid generated deployment state.
    Doctor,
    /// Render one profile and target to stdout.
    Render {
        /// Profile ID from mdmanager.toml.
        profile: String,
        /// Target ID from mdmanager.toml.
        target: String,
    },
    /// Show the difference for one global profile and target.
    Diff {
        /// Profile ID from mdmanager.toml.
        profile: String,
        /// Target ID from mdmanager.toml.
        target: String,
    },
    /// Deploy a Profile to its targets.
    Apply {
        /// Profile ID; defaults to the active Profile.
        profile: Option<String>,
        /// Proceed without an interactive confirmation.
        #[arg(long, short = 'y')]
        yes: bool,
        /// Replace and back up targets changed on disk or not managed.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Clone, Debug, Subcommand)]
enum ProjectCommand {
    /// Create a managed root instruction file from authored Markdown.
    Create {
        /// Project target: agents or claude.
        target: String,
        /// Markdown file used for the initial project Section.
        #[arg(long, value_name = "FILE")]
        from: std::path::PathBuf,
        /// Proceed without an interactive confirmation.
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Adopt an existing root AGENTS.md or CLAUDE.md byte-for-byte.
    Adopt {
        /// Project target: agents or claude.
        target: String,
        /// Proceed without an interactive confirmation.
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Render a project target to stdout.
    Render {
        /// Project target: agents or claude.
        target: String,
    },
    /// Show the difference for a project target.
    Diff {
        /// Project target: agents or claude.
        target: String,
    },
    /// Apply one project target, or every declared target.
    Apply {
        /// Project target: agents or claude.
        target: Option<String>,
        /// Proceed without an interactive confirmation.
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Verify that committed project targets match their Sections.
    Check,
}

#[derive(Clone, Debug, Subcommand)]
enum LocalCommand {
    /// Report local disable and managed-override state.
    Status,
    /// Disable a project source for Claude or Pi.
    Disable {
        /// Runtime: claude or pi.
        runtime: String,
        /// Source path; defaults to the root runtime instruction file.
        source: Option<std::path::PathBuf>,
        /// Proceed without an interactive confirmation.
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Restore an mdmanager.ai-owned local disable.
    Restore {
        /// Runtime: claude or pi.
        runtime: String,
        /// Proceed without an interactive confirmation.
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Create managed Local Instructions from a personal Section.
    Create {
        /// Local target: agents or claude.
        target: String,
        /// Personal Section ID from ~/.mdmanager/mdmanager.toml.
        section: String,
    },
    /// Adopt an existing local file matching a personal Section byte-for-byte.
    Adopt {
        /// Local target: agents or claude.
        target: String,
        /// Personal Section ID from ~/.mdmanager/mdmanager.toml.
        section: String,
    },
    /// Render managed Local Instructions to stdout.
    Render {
        /// Local target: agents or claude.
        target: String,
    },
    /// Show the difference for managed Local Instructions.
    Diff {
        /// Local target: agents or claude.
        target: String,
    },
    /// Apply managed Local Instructions.
    Apply {
        /// Local target: agents or claude.
        target: String,
        /// Proceed without an interactive confirmation.
        #[arg(long, short = 'y')]
        yes: bool,
    },
}

pub fn run() -> ExitCode {
    match execute(Cli::parse()) {
        Ok(code) => code,
        Err(error) => {
            let _ = io::stdout().flush();
            eprintln!("mdmanager: {error}");
            ExitCode::FAILURE
        }
    }
}

fn execute(cli: Cli) -> Result<ExitCode, String> {
    if let Some(Command::Docs { topic }) = &cli.command {
        return show_docs(topic.as_deref());
    }
    let paths = Paths::discover()?;
    if let Some(Command::Context { runtime, json }) = &cli.command {
        return context_command(&paths, runtime, *json);
    }
    if let Some(Command::Project { command }) = &cli.command {
        return project_command(command.clone());
    }
    if let Some(Command::Local { command }) = &cli.command {
        return local_command(&paths, command.clone());
    }
    if matches!(&cli.command, None | Some(Command::Tui)) {
        if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            return Err("the TUI requires an interactive terminal".into());
        }
        let (global, global_error) = if paths.config.exists() {
            match GlobalConfig::load(&paths) {
                Ok(global) => (Some(global), None),
                Err(error) => (None, Some(error)),
            }
        } else {
            (None, None)
        };
        crate::tui::run_main(paths, global, global_error)?;
        return Ok(ExitCode::SUCCESS);
    }
    if let Some(Command::Init { targets }) = &cli.command {
        return init(&paths, targets);
    }
    if matches!(&cli.command, Some(Command::Doctor)) {
        return doctor(&paths);
    }
    let global = GlobalConfig::load(&paths)?;
    match cli.command {
        None => unreachable!("bare mdmanager is handled before loading global configuration"),
        Some(Command::Status) => status(&global),
        Some(Command::Doctor) => unreachable!("doctor is handled before loading the project"),
        Some(Command::Render { profile, target }) => render(&global, &profile, &target),
        Some(Command::Diff { profile, target }) => {
            let view = deploy::inspect_one(&global, &profile, &target)?;
            write_output(&mut io::stdout().lock(), &view.diff)
        }
        Some(Command::Apply {
            profile,
            yes,
            force,
        }) => apply(&global, profile.as_deref(), yes, force),
        Some(Command::Docs { .. }) => {
            unreachable!("docs is handled before discovering configuration")
        }
        Some(Command::Project { .. }) => {
            unreachable!("project commands are handled before global configuration")
        }
        Some(Command::Local { .. }) => {
            unreachable!("local commands are handled before global configuration")
        }
        Some(Command::Tui) => unreachable!("tui is handled before global configuration"),
        Some(Command::Context { .. }) => {
            unreachable!("context is handled before loading global configuration")
        }
        Some(Command::Init { .. }) => unreachable!("init is handled before loading the project"),
    }
}

fn context_command(paths: &Paths, runtime: &str, json: bool) -> Result<ExitCode, String> {
    let runtime = ContextRuntime::parse(runtime)?;
    let directory = std::env::current_dir()
        .map_err(|error| format!("cannot determine launch directory: {error}"))?;
    let global = paths
        .config
        .exists()
        .then(|| GlobalConfig::load(paths).ok())
        .flatten();
    let mut audit = Audit::resolve(runtime, &directory, paths, global.as_ref());
    if runtime == ContextRuntime::Claude {
        let seen = audit
            .sources
            .iter()
            .map(|source| source.path.clone())
            .collect();
        let scan = crate::context::scan_claude_descendants(&directory, paths, seen);
        audit.add_claude_scan(scan, global.as_ref());
    }
    if json {
        println!("{}", context_json(&audit, paths)?);
    } else {
        print_context(&audit, paths);
    }
    Ok(ExitCode::SUCCESS)
}

fn print_context(audit: &Audit, paths: &Paths) {
    let relevant = audit
        .sources
        .iter()
        .filter(|source| {
            matches!(
                source.group,
                SourceGroup::PathFiltered | SourceGroup::Nested
            )
        })
        .count();
    println!(
        "{} · {}\n{} · {relevant} load when relevant",
        audit.runtime.label(),
        audit.directory.display(),
        audit.summary,
    );
    for group in [
        SourceGroup::Startup,
        SourceGroup::PathFiltered,
        SourceGroup::Nested,
    ] {
        let sources = audit
            .sources
            .iter()
            .filter(|source| source.group == group)
            .collect::<Vec<_>>();
        if sources.is_empty() {
            continue;
        }
        println!("\n{}", group.label());
        for source in sources {
            println!(
                "  {:<14} {:<36} {}",
                source.state.label(),
                source.display,
                source.scope
            );
            if let Some(reason) = source.state.reason(paths, &audit.directory) {
                println!("                 {reason}");
            }
            for import in &source.imports {
                println!(
                    "                 imports {}",
                    crate::context::display_path(import, paths, &audit.directory)
                );
            }
        }
    }
}

fn context_json(audit: &Audit, paths: &Paths) -> Result<String, String> {
    let sources = audit
        .sources
        .iter()
        .map(|source| {
            serde_json::json!({
                "path": source.path,
                "display": source.display,
                "scope": source.scope,
                "group": match source.group {
                    SourceGroup::Startup => "startup",
                    SourceGroup::PathFiltered => "conditional",
                    SourceGroup::Nested => "nested",
                },
                "status": source.state.label(),
                "reason": source.state.reason(paths, &audit.directory),
                "imports": source.imports,
                "managed": source.managed.as_ref().map(|managed| serde_json::json!({
                    "target": managed.target,
                    "profile": managed.profile,
                    "status": managed.status,
                })),
            })
        })
        .collect::<Vec<_>>();
    serde_json::to_string_pretty(&serde_json::json!({
        "runtime": audit.runtime.id(),
        "directory": audit.directory,
        "summary": audit.summary,
        "sources": sources,
    }))
    .map_err(|error| format!("cannot serialize Context: {error}"))
}

fn local_command(paths: &Paths, command: LocalCommand) -> Result<ExitCode, String> {
    let directory = std::env::current_dir()
        .map_err(|error| format!("cannot determine launch directory: {error}"))?;
    let repository = local::LocalRepository::discover(&directory, paths)?;
    match command {
        LocalCommand::Status => {
            if let Some(error) = repository.local_manifest_error() {
                return Err(error);
            }
            println!("Repository: {}", repository.root.display());
            for runtime in [local::DisableRuntime::Claude, local::DisableRuntime::Pi] {
                let label = match runtime {
                    local::DisableRuntime::Claude => "Claude disable",
                    local::DisableRuntime::Pi => "Pi disable",
                };
                let status = repository.disable_status(runtime)?.label();
                println!("{label:<18} {status}");
            }
            for target in [local::ManagedTarget::Agents, local::ManagedTarget::Claude] {
                if repository.managed_exists(target) {
                    let view = repository.inspect_managed(target)?;
                    println!(
                        "{:<18} {:<10} {}",
                        target.filename(),
                        view.status.label(),
                        view.path.display()
                    );
                    for id in &view.ordered {
                        println!(
                            "  Section {id}: {}",
                            repository.managed_section_path_for(target, id)?.display()
                        );
                    }
                } else {
                    println!("{:<18} none", target.filename());
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        LocalCommand::Disable {
            runtime,
            source,
            yes,
        } => {
            let runtime = local::DisableRuntime::parse(&runtime)?;
            let source = source.unwrap_or_else(|| match runtime {
                local::DisableRuntime::Claude => repository.root.join("CLAUDE.md"),
                local::DisableRuntime::Pi => {
                    let agents = repository.root.join("AGENTS.md");
                    if agents.exists() {
                        agents
                    } else {
                        repository.root.join("CLAUDE.md")
                    }
                }
            });
            let plan = repository.disable_plan(runtime, &source)?;
            println!("{}", plan.text);
            if !confirm_or_require_yes("Create this local disable?", yes)? {
                println!("Cancelled.");
                return Ok(ExitCode::FAILURE);
            }
            repository.apply_disable(&plan)?;
            println!("Disabled {} locally", source.display());
            Ok(ExitCode::SUCCESS)
        }
        LocalCommand::Restore { runtime, yes } => {
            let runtime = local::DisableRuntime::parse(&runtime)?;
            match repository.disable_status(runtime)? {
                local::DisableStatus::Owned => {}
                local::DisableStatus::None => {
                    return Err(format!("no mdmanager.ai-owned {} disable", runtime.key()));
                }
                local::DisableStatus::Modified => {
                    return Err(format!(
                        "{} local disable changed after mdmanager.ai wrote it; resolve the modified file first",
                        runtime.key()
                    ));
                }
                local::DisableStatus::Missing => {
                    return Err(format!(
                        "{} local disable file is missing; resolve the ownership record first",
                        runtime.key()
                    ));
                }
            }
            println!(
                "Restore the mdmanager.ai-owned {} local disable. Unowned changes are preserved.",
                runtime.label()
            );
            if !confirm_or_require_yes("Restore this local disable?", yes)? {
                println!("Cancelled.");
                return Ok(ExitCode::FAILURE);
            }
            repository.restore_disable(runtime)?;
            println!("Restored local disable");
            Ok(ExitCode::SUCCESS)
        }
        LocalCommand::Create { target, section } => {
            let target = local::ManagedTarget::parse(&target)?;
            repository.create_managed(target, &section)?;
            println!(
                "Created machine-local {} composition from personal Section {}\nSource: {}\nRun `mdmanager local apply {}` to deploy it.",
                target.filename(),
                section,
                repository
                    .managed_section_path_for(target, &section)?
                    .display(),
                target.id()
            );
            Ok(ExitCode::SUCCESS)
        }
        LocalCommand::Adopt { target, section } => {
            let target = local::ManagedTarget::parse(&target)?;
            repository.adopt_managed(target, &section)?;
            println!(
                "Adopted {} using personal Section {}",
                target.filename(),
                section
            );
            Ok(ExitCode::SUCCESS)
        }
        LocalCommand::Render { target } => {
            let target = local::ManagedTarget::parse(&target)?;
            write_output(
                &mut io::stdout().lock(),
                &repository.render_managed(target)?,
            )
        }
        LocalCommand::Diff { target } => {
            let target = local::ManagedTarget::parse(&target)?;
            let view = repository.inspect_managed(target)?;
            write_output(&mut io::stdout().lock(), &view.diff)
        }
        LocalCommand::Apply { target, yes } => {
            let target = local::ManagedTarget::parse(&target)?;
            let plan = repository.managed_plan(target)?;
            let view = &plan.view;
            println!(
                "{:<18} {:<10} {}",
                target.filename(),
                view.status.label(),
                view.path.display()
            );
            if matches!(
                view.status,
                local::LocalTargetStatus::External | local::LocalTargetStatus::Modified
            ) {
                let section = view
                    .ordered
                    .first()
                    .ok_or_else(|| "Local Instructions contain no Sections".to_owned())?;
                let section = repository.managed_section_path_for(target, section)?;
                return Err(format!(
                    "{} is {}; refusing to overwrite it. Review it against {} and move intended edits into that Section, or restore the deployed file to the rendered content",
                    view.path.display(),
                    view.status.label(),
                    section.display()
                ));
            }
            if view.status == local::LocalTargetStatus::Current && plan.exclusion_write().is_none()
            {
                println!("Managed local instructions are current.");
                return Ok(ExitCode::SUCCESS);
            }
            println!("{}", view.diff);
            if let Some((pattern, path)) = plan.exclusion_write() {
                println!("Git exclusion\n  {pattern} in {}", path.display());
            }
            println!(
                "{}",
                match target {
                    local::ManagedTarget::Agents => {
                        "AGENTS.override.md replaces AGENTS.md for Pi and Codex."
                    }
                    local::ManagedTarget::Claude => {
                        "CLAUDE.local.md adds instructions after CLAUDE.md for Claude."
                    }
                }
            );
            if !confirm_or_require_yes("Apply these managed local instructions?", yes)? {
                println!("Cancelled.");
                return Ok(ExitCode::FAILURE);
            }
            let previous = repository.apply_managed(&plan)?;
            println!("Applied {} (was {})", target.filename(), previous.label());
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn project_command(command: ProjectCommand) -> Result<ExitCode, String> {
    let directory = std::env::current_dir()
        .map_err(|error| format!("cannot determine launch directory: {error}"))?;
    match command {
        ProjectCommand::Create { target, from, yes } => {
            let root = project::worktree_root(&directory)?;
            let content = fs::read_to_string(&from)
                .map_err(|error| format!("cannot read {}: {error}", from.display()))?;
            let plan = project::create_plan(&root, &target, &content)?;
            println!("Project root: {}", root.display());
            println!("{}", plan.manifest_diff());
            println!(
                "Create Section: {} ({} bytes)",
                plan.section_path.display(),
                plan.content.len()
            );
            println!("The root target is applied separately after creation.");
            if !confirm_or_require_yes("Create this managed project target?", yes)? {
                println!("Cancelled.");
                return Ok(ExitCode::FAILURE);
            }
            let workspace = plan.apply()?;
            println!("Created {}", workspace.manifest_path.display());
            println!("Created {}", plan.section_path.display());
            println!("Run `mdmanager project apply {target}` to create the root file.");
            Ok(ExitCode::SUCCESS)
        }
        ProjectCommand::Adopt { target, yes } => {
            let root = project::worktree_root(&directory)?;
            let path = root.join(match target.as_str() {
                "agents" => "AGENTS.md",
                "claude" => "CLAUDE.md",
                _ => {
                    return Err(format!(
                        "unknown project target {target}; expected agents or claude"
                    ));
                }
            });
            if project::Workspace::discover(&root)?
                .is_some_and(|workspace| workspace.manifest.targets.contains_key(&target))
            {
                return Err(format!("project already manages {target}"));
            }
            println!("Project root: {}", root.display());
            println!("Adopt: {}", path.display());
            println!("The existing target will not be rewritten.");
            if !confirm_or_require_yes("Adopt this file into project management?", yes)? {
                println!("Cancelled.");
                return Ok(ExitCode::FAILURE);
            }
            let workspace = project::adopt(&root, &target)?;
            println!("Adopted {}", workspace.target_path(&target)?.display());
            Ok(ExitCode::SUCCESS)
        }
        ProjectCommand::Render { target } => {
            let workspace = load_project(&directory)?;
            write_output(&mut io::stdout().lock(), &workspace.render(&target)?)
        }
        ProjectCommand::Diff { target } => {
            let workspace = load_project(&directory)?;
            let view = workspace.inspect(&target)?;
            write_output(&mut io::stdout().lock(), &view.diff)
        }
        ProjectCommand::Apply { target, yes } => {
            let workspace = load_project(&directory)?;
            let targets = target.map_or_else(
                || {
                    workspace
                        .target_names()
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                },
                |target| vec![target],
            );
            if targets.is_empty() {
                return Err("project manifest declares no targets".into());
            }
            let mut changed = false;
            for target in &targets {
                let view = workspace.inspect(target)?;
                println!(
                    "{:<12} {:<12} {}",
                    target_display_name(target),
                    view.status.label(),
                    view.path.display()
                );
                if view.status != project::ProjectTargetStatus::Current {
                    changed = true;
                    println!("{}", view.diff);
                }
            }
            if !changed {
                println!("All project targets are current.");
                return Ok(ExitCode::SUCCESS);
            }
            println!("Git is your recovery; mdmanager.ai will not create project backups.");
            if !confirm_or_require_yes("Apply these project targets?", yes)? {
                println!("Cancelled.");
                return Ok(ExitCode::FAILURE);
            }
            for target in &targets {
                let previous = workspace.apply(target)?;
                if previous != project::ProjectTargetStatus::Current {
                    println!(
                        "{}: applied (was {})",
                        target_display_name(target),
                        previous.label()
                    );
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        ProjectCommand::Check => {
            let workspace = load_project(&directory)?;
            let views = workspace.inspect_all()?;
            if views.is_empty() {
                println!("No project targets declared.");
                return Ok(ExitCode::SUCCESS);
            }
            for view in &views {
                println!(
                    "{:<12} {:<12} {}",
                    target_display_name(&view.id),
                    view.status.label(),
                    view.path.display()
                );
            }
            if views
                .iter()
                .all(|view| view.status == project::ProjectTargetStatus::Current)
            {
                Ok(ExitCode::SUCCESS)
            } else {
                Ok(ExitCode::FAILURE)
            }
        }
    }
}

fn load_project(directory: &std::path::Path) -> Result<project::Workspace, String> {
    project::Workspace::discover(directory)?.ok_or_else(|| {
        "no .mdmanager/project.toml found in the launch directory or its ancestors".to_owned()
    })
}

fn confirm_or_require_yes(prompt: &str, yes: bool) -> Result<bool, String> {
    if yes {
        return Ok(true);
    }
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err("non-interactive write requires --yes".into());
    }
    print!("{prompt} [y/N] ");
    io::stdout()
        .flush()
        .map_err(|error| format!("cannot write prompt: {error}"))?;
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .map_err(|error| format!("cannot read confirmation: {error}"))?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes" | "YES"))
}

fn show_docs(topic: Option<&str>) -> Result<ExitCode, String> {
    let output = crate::docs::render(topic)?;
    write_output(&mut io::stdout().lock(), &output)
}

fn init(paths: &Paths, targets: &[String]) -> Result<ExitCode, String> {
    let global = scaffold::create(paths, targets)?;
    println!("Created {}", paths.config.display());
    println!("Created {}", scaffold::common_path(paths)?.display());
    let targets = global.target_names().collect::<Vec<_>>();
    if targets.is_empty() {
        println!("Created the Personal Section library with no Global targets.");
    } else {
        println!("Configured Global targets: {}.", targets.join(", "));
        println!("Edit the Common section, then render and apply the default Profile.");
    }
    Ok(ExitCode::SUCCESS)
}

fn render(global: &GlobalConfig, profile: &str, target: &str) -> Result<ExitCode, String> {
    let output = global.render(profile, target)?;
    write_output(&mut io::stdout().lock(), &output)
}

fn write_output(writer: &mut impl Write, output: &str) -> Result<ExitCode, String> {
    match writer.write_all(output.as_bytes()) {
        Ok(()) => Ok(ExitCode::SUCCESS),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(ExitCode::SUCCESS),
        Err(error) => Err(format!("cannot write output: {error}")),
    }
}

fn status(global: &GlobalConfig) -> Result<ExitCode, String> {
    if global.target_names().next().is_none() {
        println!("Manifest: valid");
        println!("Global targets: none configured");
        println!("Personal Section library: ready");
        return Ok(ExitCode::SUCCESS);
    }
    let profile = deploy::active_profile(global)?
        .ok_or_else(|| "no active profile; run `mdmanager apply PROFILE`".to_owned())?;
    let views = deploy::inspect(global, &profile)?;
    println!("Profile: {profile}");
    println!("Manifest: valid");
    for view in &views {
        println!(
            "{:<12} {:<10} {}",
            target_display_name(&view.id),
            view.status.label(),
            view.path.display()
        );
    }
    if views
        .iter()
        .all(|view| view.status == GlobalTargetStatus::Current)
    {
        Ok(ExitCode::SUCCESS)
    } else {
        println!("\nAction: run `mdmanager apply` to review and repair drift.");
        Ok(ExitCode::FAILURE)
    }
}

fn doctor(paths: &Paths) -> Result<ExitCode, String> {
    if !paths.config.exists() {
        println!("Global manifest: not configured");
        println!("Expected: {}", paths.config.display());
        println!("\nAction: run `mdmanager init GLOBAL_TARGET...` or `mdmanager docs start`.");
        return Ok(ExitCode::FAILURE);
    }
    let global = match GlobalConfig::load(paths) {
        Ok(global) => global,
        Err(error) => {
            println!("Global manifest: invalid\nProblem: {error}");
            println!("\nAction: run `mdmanager docs start` and repair the manifest.");
            return Ok(ExitCode::FAILURE);
        }
    };
    println!("Global manifest: valid ({})", paths.config.display());

    if global.target_names().next().is_none() {
        println!("Global targets: none configured");
        println!("Personal Section library: ready");
        println!("No problems found.");
        return Ok(ExitCode::SUCCESS);
    }

    let profile = match deploy::recorded_active_profile(&global) {
        Ok(Some(profile)) if global.manifest.profiles.contains_key(&profile) => profile,
        Ok(Some(profile)) => {
            println!("Deployment state: invalid");
            println!("Problem: active Profile `{profile}` no longer exists in the manifest.");
            print_profile_repair(&global);
            return Ok(ExitCode::FAILURE);
        }
        Ok(None) => {
            println!("Deployment state: no active Profile");
            print_profile_repair(&global);
            return Ok(ExitCode::FAILURE);
        }
        Err(error) => {
            println!("Deployment state: invalid\nProblem: {error}");
            let mut matching = deploy::matching_profiles_on_disk(&global)?;
            if matching.len() == 1 {
                let profile = matching.remove(0);
                deploy::repair_state(&global, &profile)?;
                println!("Repair: rebuilt generated state for detected Profile {profile}.");
                profile
            } else {
                deploy::clear_state(&global)?;
                println!(
                    "Matching Profiles: {}",
                    if matching.is_empty() {
                        "none".to_owned()
                    } else {
                        matching.join(", ")
                    }
                );
                println!("Repair: cleared invalid generated ownership state.");
                println!(
                    "\nAction: choose the intended Profile, then run `mdmanager apply PROFILE` to review and activate it."
                );
                return Ok(ExitCode::FAILURE);
            }
        }
    };

    println!("Active Profile: {profile}");
    let views = match deploy::inspect(&global, &profile) {
        Ok(views) => views,
        Err(error) => {
            println!("Deployment state: invalid\nProblem: {error}");
            println!(
                "\nAction: resolve the reported target path, then run `mdmanager doctor` again."
            );
            return Ok(ExitCode::FAILURE);
        }
    };
    for view in &views {
        println!(
            "{:<12} {:<10} {}",
            target_display_name(&view.id),
            view.status.label(),
            view.path.display()
        );
    }
    if views
        .iter()
        .all(|view| view.status == GlobalTargetStatus::Current)
    {
        println!("No problems found.");
        Ok(ExitCode::SUCCESS)
    } else {
        println!("\nAction: run `mdmanager apply` to review and repair deployment drift.");
        Ok(ExitCode::FAILURE)
    }
}

fn print_profile_repair(global: &GlobalConfig) {
    println!(
        "Available Profiles: {}",
        global.profile_names().collect::<Vec<_>>().join(", ")
    );
    let matching = global
        .profile_names()
        .filter(|profile| {
            deploy::inspect(global, profile).is_ok_and(|views| {
                views
                    .iter()
                    .all(|view| view.status == GlobalTargetStatus::Current)
            })
        })
        .collect::<Vec<_>>();
    if matching.len() == 1 {
        println!("Detected Profile: {} (all target files match)", matching[0]);
        println!(
            "\nAction: run `mdmanager apply {}` to repair deployment state without changing target files.",
            matching[0]
        );
    } else {
        println!(
            "\nAction: choose the intended Profile, then run `mdmanager apply PROFILE` to review and activate it."
        );
    }
}

fn apply(
    global: &GlobalConfig,
    requested_profile: Option<&str>,
    yes: bool,
    force: bool,
) -> Result<ExitCode, String> {
    let active_profile = if requested_profile.is_some() {
        deploy::recorded_active_profile(global)?
    } else {
        deploy::active_profile(global)?
    };
    let profile = match requested_profile {
        Some(profile) => profile.to_owned(),
        None => active_profile
            .clone()
            .ok_or_else(|| "no active profile; pass one to `mdmanager apply PROFILE`".to_owned())?,
    };
    let views = deploy::inspect(global, &profile)?;
    let has_changes = views
        .iter()
        .any(|view| view.status != GlobalTargetStatus::Current);
    let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();

    println!("Profile: {profile}");
    for view in &views {
        println!(
            "{:<12} {:<10} {}",
            target_display_name(&view.id),
            view.status.label(),
            view.path.display()
        );
        if view.status != GlobalTargetStatus::Current {
            println!("{}", view.diff);
        }
    }

    if has_changes && !yes && !interactive {
        return Err("non-interactive apply requires --yes".into());
    }

    if !has_changes && active_profile.as_deref() == Some(&profile) {
        println!("All targets are current.");
        return Ok(ExitCode::SUCCESS);
    }

    let mut confirmed_interactively = false;
    if has_changes && !yes {
        print!("Apply these changes? [y/N] ");
        io::stdout()
            .flush()
            .map_err(|error| format!("cannot write prompt: {error}"))?;
        let mut answer = String::new();
        io::stdin()
            .read_line(&mut answer)
            .map_err(|error| format!("cannot read confirmation: {error}"))?;
        if !matches!(answer.trim(), "y" | "Y" | "yes" | "YES") {
            println!("Cancelled.");
            return Ok(ExitCode::FAILURE);
        }
        confirmed_interactively = true;
    }

    let reports = if confirmed_interactively {
        deploy::apply_reviewed(global, &profile, &views, true)?
    } else {
        deploy::apply(global, &profile, None, force, true)?
    };
    let mut applied = 0;
    for report in reports {
        if report.previous == GlobalTargetStatus::Current {
            continue;
        }
        applied += 1;
        println!(
            "{}: applied (was {})",
            target_display_name(&report.id),
            report.previous.label()
        );
        if let Some(backup) = report.backup {
            println!("  backup: {}", backup.display());
        }
    }
    if applied == 0 {
        println!("All targets are current.");
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_closed_render_pipe_is_successful() {
        struct ClosedPipe;

        impl Write for ClosedPipe {
            fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
                Err(io::Error::from(io::ErrorKind::BrokenPipe))
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        assert_eq!(
            write_output(&mut ClosedPipe, "output").unwrap(),
            ExitCode::SUCCESS
        );
    }
}
