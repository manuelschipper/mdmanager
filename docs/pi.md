# Pi context

Pi selects one persistent instruction file at each directory level. Its candidates include
`AGENTS.override.md`, `AGENTS.md`, and compatible uppercase or Claude filenames. Unlike Codex, an
empty winning override can intentionally contribute no instruction text.

## Managed files

- Global Pi instructions normally target `~/.pi/agent/AGENTS.md`. `PI_CODING_AGENT_DIR` changes the
  user directory Pi and Context inspect.
- Project Instructions use root `AGENTS.md` or `CLAUDE.md` according to Pi's selection.
- Local `AGENTS.override.md` replaces the regular candidate for Pi and also affects Codex.

Use Context after any agent-side change:

```sh
mdmanager context --runtime pi
```

Pi system-prompt files, extensions, skills, tools, prompt templates, and conversation state are
outside mdmanager's scope and are not displayed.
