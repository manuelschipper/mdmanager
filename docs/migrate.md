# Migrate existing instructions

This is an operational guide for converting existing agent instructions to mdmanager.ai without
losing meaning or silently replacing files.

## Safety boundary

- Treat current fragments, wrappers, imports, symlinks, installers, and deployed files as user-owned.
- Do not run `mdmanager apply`, use `--force`, delete old files, or change installer behavior without
  explicit approval.
- Preserve instruction meaning, ordering, runtime differences, and machine-specific profiles.
- Render and compare before writing a managed target.

## 1. Inventory unmanaged instructions

Find global Codex, Pi, and Claude files, Cursor rules, root project `AGENTS.md`/`CLAUDE.md`, symlink
destinations, imported Markdown, Local Instructions, and installer code. Record the effective ordered
content each runtime and machine currently receives. Distinguish canonical source fragments from
generated output.

Use `mdmanager` → Context in each important repository to see the predicted new-session chain. The
TUI does not edit instructions and reloads after agent-side changes. Context does not resolve custom import syntax
for you and does not observe running sessions.

## 2. Choose ownership

- Put reusable personal instructions in global Sections and give every Profile an explicit,
  non-empty composition for each catalog Target it deploys.
- Adopt a shared root `AGENTS.md` or `CLAUDE.md` into committed Project management only when the team
  should own `.mdmanager/` and generated root files together.
- Use Local Instructions for machine-specific `AGENTS.override.md` or `CLAUDE.local.md` content.

Show the proposed Section boundaries and ordering before creating files. Keep runtime-specific content
separate. Do not copy an injected global target title into a Global Section.

## 3. Create and verify sources

If no global source exists, `mdmanager init GLOBAL_TARGET...` creates only the selected Claude,
Codex, or Pi targets without deployment. Bare `mdmanager init` creates only the Personal Section
library. Edit `~/.mdmanager/mdmanager.toml` and its Sections, then run
`mdmanager render PROFILE TARGET` for every composition. Compare each result with the resolved old
output and explain every difference.

For a project file, `mdmanager project adopt agents|claude` verifies byte equality while creating the
committed source model. `project create TARGET --from FILE` starts a new target without applying it.
Review `.mdmanager/project.toml`, render each target, and run `mdmanager project check` after apply.

## 4. Apply and retire old paths

After explicit approval, apply the intended global Profile and confirm `mdmanager status` reports the
Profile targets current. Global targets changed on disk, not managed by mdmanager.ai, or symlinked require
replacement review and may create numbered backups. Project apply uses Git instead of machine
backups.

Update the dotfiles installer to install `mdmanager` and call `mdmanager apply PROFILE --yes`; do not
duplicate composition logic there. Add `--force` only when replacement of an existing Global target
has been explicitly approved. Remove old wrappers, links, generators, commands, and state only after
every required machine is verified.
