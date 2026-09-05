# Cursor context

Context reports the `AGENTS.md` chain from the nearest Git root through the launch directory, or
only the launch directory when no Git root exists. It does not scan deeper descendants below the
launch directory.

Cursor Project Rules are `.mdc` files under `.cursor/rules/`. Context reports always-applied,
glob-scoped, intelligently selected, and manual rules from each rule directory between the project
root and launch directory. It does not manage these files.

Cursor User and Team Rules live in Cursor settings rather than documented Markdown files. Context
notes that boundary but cannot inspect their contents. Context reports root `CLAUDE.md` as
uncertain, noting that only Cursor CLI documents it.

## Managed files

- Project Instructions target root `AGENTS.md`, shared by Cursor, Codex, and Pi.
- Cursor has no documented Global Markdown target.
- Cursor has no documented equivalent of mdmanager's Local `AGENTS.override.md` mechanism.

Use Context after changing Cursor instructions or rules:

```sh
mdmanager context --runtime cursor
```

Cursor system instructions, User and Team Rule contents, skills, tools, and conversation state are
outside mdmanager's scope.
