# Pi context

Pi selects one persistent instruction file at each directory level. Its candidates include
`AGENTS.override.md`, `AGENTS.md`, `AGENTS.MD`, `CLAUDE.md`, and `CLAUDE.MD`, in that order. An
empty winning override contributes no instruction text and blocks the other candidates in that
directory, as it does for Codex project instructions.

Pi loads the global file first, then ancestor files from the filesystem root through the launch
directory, deduplicating paths already loaded globally. When a linked worktree is nested under its
main checkout, its selected root file suppresses the main checkout's file with the same basename.
Context shows that main file as not selected and names the worktree file. Different filenames,
sibling worktrees, bare repository layouts, and submodules keep normal ancestor inheritance.

Context follows the resource loader in [earendil-works/pi](https://github.com/earendil-works/pi),
the current Pi repository. Candidate aliases on case-insensitive filesystems appear only once.

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
