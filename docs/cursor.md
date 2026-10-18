# Cursor context

mdmanager models two Cursor runtimes:

- `cursor` is the Cursor editor and follows Cursor's documented behavior.
- `cursor-agent` is the Cursor CLI and follows behavior observed in `cursor-agent` 2026.10.16.
  Cursor does not document most of it, so a later release can change it.

```sh
mdmanager context --runtime cursor
mdmanager context --runtime cursor-agent
```

Both report Project Rules, the `.mdc` files under `.cursor/rules/`, as always-applied,
glob-scoped, intelligently selected, or manual. Cursor User and Team Rules live in Cursor settings
rather than documented Markdown files. Context notes that boundary but cannot inspect their
contents.

mdmanager models complete `---` frontmatter headers and simple inline or block lists, not general
YAML. A leading `---` without a closing delimiter is reported as ambiguous by the editor runtime;
close the header before relying on the reported loading behavior.

## Cursor editor

Context reports the `AGENTS.md` chain and `.cursor/rules/` directories from the nearest Git root
through the launch directory, or only the launch directory when no Git root exists. It does not
scan deeper descendants below the launch directory. Context reports root `CLAUDE.md` as uncertain,
noting that only Cursor CLI documents it.

## Cursor CLI

At startup the CLI reads every directory from the filesystem root to the launch directory, not
only the project. In each one it loads `AGENTS.md`, `CLAUDE.md`, `CLAUDE.local.md`, and
`.cursor/rules/**/*.mdc`. Files above the project, such as `~/AGENTS.md` or `~/.cursor/rules/`,
therefore apply to every project below them. It also loads a legacy `.cursorrules` file at the Git
root as an always-applied rule.

- Each Markdown file is cut at 100 KiB; Context reports a longer file as partial.
- A Markdown file whose content or file identity matches one already loaded from the same or a
  higher directory is skipped, so a `CLAUDE.md` symlinked to `AGENTS.md` loads once.
- A rule without a complete frontmatter header is skipped and reported as excluded.
- `AGENTS.md`, `CLAUDE.md`, and rules in subfolders below the launch directory attach after the
  agent reads files there. Context does not scan them.
- `.cursorignore` can hide instruction files from the CLI; Context does not model it.

## Managed files

- Project Instructions target root `AGENTS.md`, shared by Cursor, Codex, and Pi. The Cursor CLI
  also loads root `CLAUDE.md`.
- Local `CLAUDE.local.md` reaches the Cursor CLI as well as Claude.
- Cursor has no documented Global Markdown target. The CLI reads `~/AGENTS.md` for projects under
  your home directory, but other runtimes that read ancestor directories, such as Pi, read it too,
  so mdmanager does not offer it as a Global target.
- Cursor has no documented equivalent of mdmanager's Local `AGENTS.override.md` mechanism.

Cursor system instructions, User and Team Rule contents, skills, tools, and conversation state are
outside mdmanager's scope.
