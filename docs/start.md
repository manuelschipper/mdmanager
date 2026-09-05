# Start

mdmanager.ai splits work among a coding agent, the CLI, and the TUI:

- The coding agent interprets intent and edits Markdown or manifests.
- The CLI creates, adopts, validates, renders, applies, disables, and restores.
- The TUI browses files, explains Context, shows status and differences, and auto-reloads.

Run **mdmanager** in the repository you want to inspect. Opening the TUI writes nothing and does not
require Global configuration; choosing a theme later writes only `[ui].theme`. The TUI needs a
terminal, so coding agents should use the CLI instead.

## Agent contract

At ownership and deployment boundaries, confirm intent instead of choosing silently. Do not re-ask
anything the user already answered explicitly.

1. Confirm the repository and runtimes. Run **mdmanager context --runtime RUNTIME**, then identify
   existing files, mdmanager ownership, and relevant Git state. Seeing a file is not owning it.
2. Inspect the relevant files before editing.
3. If unclear, explain and confirm committed Project, private Local, or personal Global scope.
4. Ask whether to prepare and validate only, or also Apply after review. A source edit never updates
   its runtime target automatically.
5. Before introducing Project management, explain that `.mdmanager/project.toml`,
   `.mdmanager/sections/*.md`, and the generated root `AGENTS.md` or `CLAUDE.md` are intended for
   review and commit. Ask before adding that shared structure. Do not keep Project `.mdmanager/`
   untracked as private configuration; choose Local Instructions instead.
6. If an instruction file already exists but is external, ask whether to adopt it or leave it
   external. Adoption is an ownership decision, not merely file discovery.
7. Create or adopt the chosen source model, then edit its Markdown and composition.
8. Render, inspect the difference, and run **mdmanager project check** or **mdmanager status**. None
   of these applies anything.
9. Apply only with authorization. If step 4 did not settle it, summarize the target and difference
   and ask once. Non-interactive Apply and Project ownership writes require **--yes**; Local create
   and adopt take no **--yes**.
10. Re-run Context or status and `git status`. Report changed, commit-intended, and local-only files,
    including any `.git/info/exclude` rule or `.claude/settings.local.json` exclusion mdmanager
    changed.

Global targets that are changed on disk, not managed by mdmanager.ai, or symlinked are never replaced
silently. Interactive Apply shows the difference and a `y` authorizes replacement. Non-interactive
Apply stops with `retry with --force`; add **--yes --force** only after approval of that specific
replacement. Either path creates a numbered backup. Do not ask the user to reproduce CLI mechanics
you can safely perform yourself.

## Human workflow

Open **mdmanager**, ask your coding agent to read **mdmanager docs start**, and say whether you want
preparation only or preparation plus Apply. Inspect the reloaded result and request another revision
or authorize Apply. The TUI has no editing or Apply mode.

## Ownership boundary

mdmanager owns only the Project, Local, and configured Global Markdown families below. Context may
also report Claude and Cursor rules, nested files, imports, exclusions, and runtime selection as
audit-only. System prompts, memory, skills, hooks, commands, output styles, tools, and conversations
are outside its scope.

## Choose a scope

- **Project Instructions** are shared repository configuration. Their source manifest and Sections
  live under committed `.mdmanager/`; their generated root `AGENTS.md` and `CLAUDE.md` are also
  intended to be committed. Existing root files remain external until explicitly adopted.
- **Local Instructions** are private to one repository checkout and machine. Their compositions live
  under `~/.mdmanager/projects/`; `AGENTS.override.md` and `CLAUDE.local.md` stay ignored and
  untracked. On first Apply, mdmanager uses the repository's common `.git/info/exclude` only when no
  existing Git rule ignores the output. The Apply plan shows the exact rule and exclusion file before
  writing either one. This is independent of Project management and works across linked worktrees.
  `AGENTS.override.md` replaces—and therefore shadows—the regular `AGENTS.md` candidate for Pi and
  Codex. `CLAUDE.local.md` adds instructions after `CLAUDE.md` for Claude.
- **Global Instructions** are personal machine configuration. `[targets.*]` is the shared catalog.
  Profiles compose Personal Sections into the catalog Targets they name. Cursor has no documented
  Global Markdown target.

Project and Local Instructions require a Git worktree. Global and Local compositions resolve
Personal Sections through `~/.mdmanager/mdmanager.toml`; **mdmanager init [GLOBAL_TARGET...]**
creates the library and exactly the requested Claude, Codex, or Pi targets without deploying them.
With no targets it creates only the library.

## Symlink aliases

When multiple runtime paths intentionally need identical Markdown, mdmanager may manage one regular
canonical Global or Project target and a coding agent may offer symlinks from the other paths. The
agent must inspect each existing path, explain that the alias remains external, and get approval
before creating or replacing a link. Apply and verify the canonical target first, then verify Context
for every aliased runtime.

Do not declare an alias path as another mdmanager Target. mdmanager does not create, repair, repoint,
or remove aliases; Home and Context report the link separately from its destination. Applying a
configured Global target that is currently a symlink requires replacement approval and replaces the
link with a regular generated file. Prefer separate Targets when any runtime needs different content.

## Editing is not applying

The normal sequence is:

~~~text
edit source → render → inspect difference → apply → verify
~~~

Editing changes the intended composition and makes status report **out of sync**; the runtime file is
untouched until Apply. Render and diff are read-only. Project or Local create prepares source state
and still needs Apply. Adoption copies the existing target into a Section, verifies that the render
is byte-identical, and leaves the deployed file untouched.

## Command recipes

Global Instructions:

~~~sh
mdmanager init claude codex
mdmanager render PROFILE TARGET
mdmanager diff PROFILE TARGET
mdmanager apply PROFILE --yes
mdmanager status
~~~

Committed Project Instructions use target `agents` or `claude`. Choose create for new authored
Markdown or adopt for an existing root file; create refuses when the root file exists and points to
adopt. Both initialize only the committed Project manifest, Sections, and requested root target
model.

~~~sh
mdmanager project create TARGET --from FILE --yes
mdmanager project adopt TARGET --yes
mdmanager project render TARGET
mdmanager project diff TARGET
mdmanager project apply [TARGET] --yes
mdmanager project check
~~~

Local Instructions read Personal Sections from `~/.mdmanager/mdmanager.toml`; if it does not exist,
bare **mdmanager init** creates the library without Global targets or deployment. Create starts a
private composition from a Section ID. Adopt requires that Section to render byte-for-byte as the
existing ignored Local file. If the matching output is not ignored, Adopt gives the exact common
`.git/info/exclude` rule to add and does not add it itself. Create and adopt write only
mdmanager-owned sources and take no **--yes**.

~~~sh
mdmanager local create TARGET SECTION
mdmanager local adopt TARGET SECTION
mdmanager local render TARGET
mdmanager local diff TARGET
mdmanager local apply TARGET --yes
mdmanager local disable RUNTIME [SOURCE] --yes
mdmanager local restore RUNTIME --yes
mdmanager local status
~~~

Local disable for Claude updates `.claude/settings.local.json`'s `claudeMdExcludes` list. Local
disable for Pi writes `AGENTS.override.md`. Both plans show a missing common `.git/info/exclude`
rule before confirmation and add only that rule; `--yes` still prints the plan. Restore removes the
Git rule only when mdmanager added it, so pre-existing ignore rules remain. Here, disable accepts
`claude` or `pi`; Codex has no verified local-disable mechanism.

To reuse a Personal Section in committed Project Instructions, copy it into
`.mdmanager/sections/` and declare it in `project.toml`. A committed project manifest cannot
reference `~/.mdmanager`; absolute and escaping paths are rejected.

Use **mdmanager COMMAND --help** for placeholders. Project recovery comes from Git. Status and
Project check exit non-zero when a target is out of sync; that is a report, not a crash. Read
**mdmanager docs migrate** before adopting an existing managed or generated setup. Useful final
checks:

~~~sh
mdmanager context --runtime claude
mdmanager context --runtime cursor
mdmanager status
mdmanager project check
mdmanager docs context
mdmanager docs claude
~~~
