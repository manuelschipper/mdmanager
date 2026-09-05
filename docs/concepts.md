# Concepts

mdmanager's awareness is broader than its ownership.

The coding agent and CLI own instruction and deployment changes. The TUI browses Context, Markdown,
composition status, and differences; it automatically reloads after disk changes and writes only its
theme preference.

## Context

Context resolves persistent Markdown instructions from a runtime and launch directory. It shows
which files load at startup, which may load when relevant, which candidates are not selected, and
which Claude sources are excluded. It describes files on disk, not a running conversation.

Claude and Cursor rules and `@` imports are visible but read-only. Runtime settings are read only
when needed to explain why Markdown is selected or excluded.

## Ownership scopes

- A **Personal Section** is reusable Markdown under `~/.mdmanager/sections/`. A **Profile** names the
  catalog Targets it deploys and an ordered Personal Section list for each.
- A **Project composition** is a committed, Profile-free Target and ordered Section list in
  `.mdmanager/project.toml`. It renders root `AGENTS.md` or `CLAUDE.md`.
- **Local Instructions** belong to one repository on one machine. Their composition references
  Personal Sections and renders `AGENTS.override.md` or `CLAUDE.local.md`.

Personal Sections may be used by Global Profiles and any number of private repository compositions.
Committed Project compositions remain self-contained: reusing a Personal Section there means
copying it into the repository as an independent Project Section.

An existing instruction file remains external until the user explicitly adopts it.

## Rendering and status

Rendering reads and validates Sections and writes nothing. Applying writes the target. These are CLI
operations normally performed by the coding agent; the TUI only displays their inputs and results.

Managed targets use one status vocabulary: `current`, `out of sync`, `changed on disk`, `existing ·
not managed by mdmanager.ai`, or `not found`. Project targets use the applicable subset. mdmanager
never overwrites Local Instructions that are changed on disk or not managed.

The TUI Library browses every configured Profile, Personal Section, and current-project Section,
including unused Sections. Deployment status describes only the active Profile's targets; with
none active, catalog targets are `not applied yet`. A Profile that is not active carries no
deployment obligation, so
the TUI compares its rendered compositions with the files on disk and says `matches current file`
or `differs from current file`. Browsing never activates or applies anything.
