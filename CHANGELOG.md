# Changelog

## Unreleased

- **Reviewed Project writes** — Apply retains the displayed review and refuses changed
  targets or source paths before writing, including when using `--yes`.
- **Local ownership safety** — Managed outputs reject resolving and dangling symlinks.
  Local writes validate recovery records first; restore prepares settings and owned
  exclusion edits before changing files, and repeated Apply retains exclusion ownership.
- **Consistent TUI inspection** — Pages, search, and difference controls use explicit inspection snapshots, keeping file observations stable between reloads and reporting current refresh errors. Differences follow content comparison independently of deployment status, and Context diagnostics no longer depend on display wording.

- **Claude worktree settings** — Context uses the same Git-reported settings worktree as Local disables, including separate Git directories and submodules, and no longer reads settings from Git metadata parents.
- **Malformed rule headers** — Claude and Cursor rules with unterminated frontmatter are reported as ambiguous with a reason instead of inferring loading from incomplete metadata.

## mdmanager 0.1.0 — Sep 5, 2026

- **Runtime Context** — Inspect persistent Markdown instructions for Claude Code,
  Codex, Cursor, and Pi. Browse global and project sources with loading order,
  overrides, conditional rules, imports, exclusions, and truncation explained.
- **Reusable Sections and Global Profiles** — Compose shared Markdown into
  runtime-specific instruction files. Named Profiles let workstations and hosted
  agent machines use different global setups from a personal Section library.
- **Project and Local instructions** — Keep shared project compositions committed
  with the repository, and private repository instructions in ignored local files.
  Local compositions can reuse personal Sections across repositories.
- **Agent workflow and TUI review** — Point a coding agent to `mdmanager docs start`
  for instruction edits and CLI Apply. Use the TUI to browse sources and review
  differences; it reloads disk changes while preserving selection and scroll position.
- **Adoption and deployment checks** — Adopt existing instruction files while
  preserving their deployed contents. Render and diff before Apply, and check for
  source changes awaiting deployment or managed targets edited directly on disk.
- **Bundled guides and diagnostics** — Read setup, configuration, migration, and
  runtime guides through `mdmanager docs`. Use `mdmanager doctor` to diagnose Global
  configuration problems and repair invalid generated state.
