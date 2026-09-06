# CLI

```text
mdmanager                                  Open the TUI
mdmanager tui                              Open the same TUI explicitly
mdmanager context [--runtime RUNTIME]      Explain persistent Markdown context
mdmanager context --runtime RUNTIME --json Emit machine-readable Context
mdmanager docs [TOPIC]                     Read bundled documentation
mdmanager init [GLOBAL_TARGET...]          Create the Personal library and selected Global targets
mdmanager doctor                           Diagnose Global problems and repair invalid state
mdmanager status                           Report global deployment drift
mdmanager render PROFILE TARGET            Print a global document
mdmanager diff PROFILE TARGET              Show a global target difference
mdmanager apply [PROFILE] [--yes] [--force] Apply a Profile's targets

mdmanager project create TARGET --from FILE [--yes]
mdmanager project adopt TARGET [--yes]
mdmanager project render TARGET
mdmanager project diff TARGET
mdmanager project apply [TARGET] [--yes]
mdmanager project check
mdmanager local status
mdmanager local create TARGET SECTION
mdmanager local adopt TARGET SECTION
mdmanager local render TARGET
mdmanager local diff TARGET
mdmanager local apply TARGET [--yes]
mdmanager local disable RUNTIME [SOURCE] [--yes]
mdmanager local restore RUNTIME [--yes]
```

`context`, `docs`, the TUI, and Project commands need no personal configuration. Context accepts
`claude` (default), `codex`, `cursor`, `pi`, or `xi`; JSON reports the same resolution details.

`init` accepts Global targets `claude`, `codex`, `pi`, and `xi` and creates exactly those targets without
deploying them. `apply PROFILE` writes that Profile's targets, not every catalog Target. With no
arguments `init` creates only the Personal Section library. Cursor uses project
`AGENTS.md` and has no Global target, so `init cursor` stops with directed project and Context
guidance.

Project targets are `agents` (`AGENTS.md`) and `claude` (`CLAUDE.md`). Local targets render
`AGENTS.override.md` and `CLAUDE.local.md`. `SECTION` names a Personal Section; Local compositions
reference it without copying its Markdown.

Non-interactive target writes require `--yes`. `init`, `local create`, and `local adopt` create only
owned sources. Local Apply never overwrites changed or unowned files. Resolve Local symlinks,
even dangling ones, before writes. Replacing changed, unowned, or symlinked Global targets requires `--force` and creates a numbered backup.

Project Apply rechecks reviewed targets before writing. On changes, review again. Recover via Git.

`doctor` repairs only invalid generated state and never changes instruction targets. It restores a
uniquely matching Profile; otherwise it clears invalid ownership so Apply remains protected.

The CLI is the mutation interface and never creates symlinks. Agents should read
`mdmanager docs start`; the TUI never invokes write commands.

Use `mdmanager <command> --help` for exact arguments and `mdmanager docs context` for the boundary
between managed and observed context.
