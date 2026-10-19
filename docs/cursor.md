# Cursor context

The Cursor CLI (`cursor-agent`) and the Cursor editor's agent share one rule loader. mdmanager
models it from behavior observed in `cursor-agent` 2026.10.16 and the Cursor 3.25.9 editor.
Cursor documents little of it, so a later release can change it.

```sh
mdmanager context --runtime cursor
mdmanager context --runtime cursor-agent
```

`cursor` is the editor, launched on a workspace folder; `cursor-agent` is the CLI, launched in a
directory. Both report Project Rules, the `.mdc` files under `.cursor/rules/`, as always-applied,
glob-scoped, intelligently selected, or manual. Cursor User and Team Rules live in Cursor settings
rather than Markdown files. Context notes that boundary but cannot inspect their contents.

## What loads

At startup Cursor reads every directory from the launch directory up to the filesystem root, not
only the project. In each one it loads `.cursor/rules/**/*.mdc`, then `CLAUDE.md`,
`CLAUDE.local.md`, and `AGENTS.md`; Context lists them in that order. Files above the project, such
as `~/.cursor/rules/`, therefore apply to every project below them. It also loads a legacy
`.cursorrules` file as an always-applied rule from the root Git reports, or from the launch
directory outside a repository.

- Markdown and rules are decoded first: a UTF-8 byte-order mark is dropped, UTF-16 with a
  byte-order mark is decoded, and CRLF line endings become LF when at least 5% of line endings are
  CRLF (for UTF-16, within its first 8 KiB). Files detected as binary are skipped and reported as
  excluded. Other encoding detection is not modeled.
- Each decoded Markdown file is cut at 100 KiB; Context reports a longer file as partial.
- A rule that does not start with a `---` header followed by more text is skipped and reported as
  excluded. Only an unquoted `alwaysApply: true` makes a rule always apply.
- `AGENTS.md`, `CLAUDE.md`, and rules in subfolders below the launch directory were observed to
  attach only after the agent reads files there. Context does not scan them.
- `.cursorignore` can hide instruction files; Context does not model it.

The two runtimes differ in two ways:

- The CLI skips a Markdown file whose file identity or loaded text matches one already kept from
  the same or a higher directory, so a `CLAUDE.md` symlinked to `AGENTS.md` loads once. The editor
  loads both.
- The CLI always loads `CLAUDE.md` and `CLAUDE.local.md`. The editor loads them only when its
  third-party extensibility setting is on, which Context cannot read, so it reports them as
  uncertain.

## Managed files

- The Global target `cursor`, created by `mdmanager init cursor`, renders
  `~/.cursor/rules/mdmanager.mdc` with an `alwaysApply: true` header. The CLI and the editor load
  it for every project below your home directory. Other runtimes do not read `.cursor/rules`.
- Project Instructions target root `AGENTS.md`, shared by Cursor, Codex, and Pi. Cursor also loads
  root `CLAUDE.md`.
- Local `CLAUDE.local.md` reaches Cursor as well as Claude.
- Cursor has no equivalent of mdmanager's Local `AGENTS.override.md` mechanism.

Cursor system instructions, User and Team Rule contents, skills, tools, and conversation state are
outside mdmanager's scope.
