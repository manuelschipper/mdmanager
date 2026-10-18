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

For the editor, mdmanager models complete `---` frontmatter headers and simple inline or block
lists, not general YAML. A leading `---` without a closing delimiter is reported as ambiguous;
close the header before relying on the reported loading behavior. For the CLI, mdmanager follows
the CLI's own header parser.

## Cursor editor

Context reports the `AGENTS.md` chain and `.cursor/rules/` directories from the nearest Git root
through the launch directory, or only the launch directory when no Git root exists. It does not
scan deeper descendants below the launch directory. Context reports root `CLAUDE.md` as uncertain,
noting that only Cursor CLI documents it.

## Cursor CLI

At startup the CLI reads every directory from the launch directory up to the filesystem root, not
only the project. In each one it loads `.cursor/rules/**/*.mdc`, then `CLAUDE.md`,
`CLAUDE.local.md`, and `AGENTS.md`; Context lists them in that order. Files above the project, such
as `~/AGENTS.md` or `~/.cursor/rules/`, therefore apply to every project below them. It also loads a
legacy `.cursorrules` file as an always-applied rule from the root Git reports, or from the launch
directory outside a repository.

- Markdown is decoded first: a UTF-8 byte-order mark is dropped, UTF-16 with a byte-order mark is
  decoded, and CRLF line endings become LF. Files detected as binary are skipped and reported as
  excluded. Other encoding detection is not modeled.
- Each decoded Markdown file is cut at 100 KiB; Context reports a longer file as partial.
- A Markdown file whose file identity or loaded text matches one already kept from the same or a
  higher directory is skipped, so a `CLAUDE.md` symlinked to `AGENTS.md` loads once.
- A rule that does not start with a `---` header followed by more text is skipped and reported as
  excluded. Only an unquoted `alwaysApply: true` makes a rule always apply.
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
