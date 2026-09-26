# Changelog

## mdmanager 0.2.0 — Sep 25, 2026

- **Claude reads AGENTS.md** — `mdmanager context --runtime claude` reports `AGENTS.md` and
  `.claude/AGENTS.md` the way Claude Code 2.1.277+ loads them: only when no `CLAUDE.md` or
  `CLAUDE.local.md` is on the launch path, or as `not selected` when one is. It follows the
  **Project instructions** setting and a disabled `agents-md` plugin, lists subfolder `AGENTS.md`
  files, and does not repeat an `AGENTS.md` that a `CLAUDE.md` imports.
- **Local Claude instructions warn about AGENTS.md** — `mdmanager local apply claude` warns when the
  new `CLAUDE.local.md` would stop Claude reading the repository's `AGENTS.md`.
- **Disable AGENTS.md for Claude** — `mdmanager local disable claude AGENTS.md` adds the file to
  `claudeMdExcludes` instead of rejecting it.
- **Claude settings scope** — Claude Context reads project `.claude/settings.json` only from the
  launch directory and `.claude/settings.local.json` from the repository (main checkout) root and
  then the launch directory, as Claude does, instead of from every ancestor. `claudeMdExcludes`
  patterns now also come from managed settings and match a symlinked file's resolved target.
- **Oversized Claude instructions** — instruction files over 4 MiB show as `excluded`, with their
  size, because Claude skips them.
- **HTML comments in Claude imports** — `@path` text inside block-level HTML comments is no longer
  listed as an import, matching Claude, which strips those comments.
- **Shared Local exclusions survive restores** — `mdmanager local restore` no longer removes a
  common `.git/info/exclude` rule that another linked worktree's mdmanager-owned output still
  relies on; ownership moves to that output. Previously a restore in one worktree could leave
  another worktree's private `AGENTS.override.md` unignored, so `git add .` could commit it.
- **Local status flags unignored outputs** — `mdmanager local status` warns, with the exact rule to
  add, when an owned Local output or disable exists but Git no longer ignores it.
- **Pi disable states its Codex effect** — the `local disable pi` plan says that the empty
  `AGENTS.override.md` also hides that directory's `AGENTS.md` from Codex, which selects the first
  existing file and then drops it when empty. mdmanager no longer probes `codex --version`.
- **Codex instruction discovery** — Codex Context matches Codex 0.156.1: the first existing project
  candidate in a directory wins even when empty (so an empty `AGENTS.override.md` hides
  `AGENTS.md`), while the global level still falls through empty files. Fallback names are
  validated and deduplicated, `project_root_markers = []` limits discovery to the launch
  directory, a zero `project_doc_max_bytes` loads no project instructions, and files after an
  exhausted budget are shown as not loaded.
- **Codex project trust** — projects marked `trust_level = "untrusted"` in the user Codex config
  show their project instructions as not loaded, as Codex skips them.
- **Pi worktrees and duplicates** — in a linked worktree nested inside its main checkout, Pi Context
  shows the main checkout's matching file as not loaded, as Pi does; a global file on the launch
  path is listed once.
- **Case-insensitive aliases** — one physical instruction file no longer shows phantom uppercase
  candidates such as `AGENTS.MD` on case-insensitive filesystems.
- **Serialized Global writes** — `mdmanager apply` and `doctor` repairs take an exclusive lock on
  `~/.mdmanager/state/state.lock`, so concurrent agents can no longer interleave target writes or
  lose ownership entries. A reviewed plan is checked again under the lock and refused if another
  apply changed the targets first.
- **TUI watches every Global target** — automatic reload now watches each configured Global target
  path, including custom locations, and Claude's managed-policy `CLAUDE.md`.
- **Project Sections stay inside `.mdmanager/`** — Project loading resolves Section paths through
  symlinks and rejects any Section, including one under a symlinked `sections/` directory, that
  resolves outside the repository's `.mdmanager/`, so committed compositions render the same bytes
  in every clone.
- **`init` follows runtime directory overrides** — `mdmanager init` writes Global targets under
  `CLAUDE_CONFIG_DIR`, `CODEX_HOME` and `PI_CODING_AGENT_DIR` when set, matching what Context
  inspects, and stops without writing when an override is outside HOME.
- **Context reports incomplete scans** — unreadable subfolder Claude instruction files appear as
  warnings in `mdmanager context` text and JSON, and the "load when relevant" count excludes files
  that won't load.

## mdmanager 0.1.0 — Sep 6, 2026

- **Binary installation** — Install verified Linux and macOS binaries for x86_64 and
  ARM64 from `mdmanager.ai/install`, with archives and SHA-256 checksums on GitHub Releases.
- **Release checks** — Run formatting, Clippy, Rust tests, installer verification, and
  website builds on Linux and macOS before publishing native release binaries.
- **Pi path aliases** — Disable and restore Pi instructions reached through a symlinked
  parent directory, including macOS temporary paths, without rejecting them as outside the repository.

- **Cross-machine setup** — Document how to install a shared personal library on a fresh
  machine, select its Profile, and pull and apply later changes while keeping ownership state local.

- **Markdown documents** — Render headings, lists, emphasis, code, and links in TUI documents
  and previews. Press `m` to switch to raw source; the saved UI preference reloads from disk.
  Source line numbers, line jumps, and search remain aligned; differences always stay raw.

- **Bundled docs** — Remove private page-size limits so documentation growth cannot
  panic when reading a topic in a debug build.

- **Website** — Add the mdmanager.ai homepage, with the existing documentation and release changelog rendered as static pages.
- **Workflow demo** — Re-record the README and website demo from a scripted Docker session: it now starts at the splash, walks Home, Context, the Library and two Profiles, shows a coding agent editing a Section shared by Claude, Codex and Pi, and reviews and applies the Global difference. The site plays a matching light or dark video with the narration beside it as page text; the README GIF carries the narration below the terminal.
- **TUI metadata and diagnostics** — Source About panels account for wrapped paths and
  compact the displayed path when needed to keep usage metadata and Contents visible.
  Empty Context views retain configuration warnings and scan failures.
- **Document search navigation** — Search reveals matches within wrapped lines and
  advances past the final match before wrapping, even at the bottom of a document.

- **Collision-safe Global backups** — Backup reservations use create-only persistence so
  overlapping replacements retain every recovery point.
- **Project creation safety** — Reviewed creation rejects newly introduced target symlinks
  and preserves intervening Section replacements when a manifest commit fails.

- **Recovery after source removal** — Local status and restore no longer require the
  disabled source directory to exist. Shared Claude settings can be restored after
  removing their originating linked worktree, preserving unrelated settings edits.

- **Local recovery and concurrency** — Validate owned destinations and Claude settings
  file types before mutation, serialize linked-worktree writers with an advisory lock,
  and report partial-write recovery guidance without claiming multi-file atomicity.

- **Context refresh dependencies** — Refresh detects main-worktree Claude exclusion settings and additions, edits, and removals in linked Claude and Cursor rule directories, including deeply nested external rules.

- **Reviewed Project writes** — Apply retains the displayed review and refuses changed
  targets or source paths before writing, including when using `--yes`.
- **Local ownership safety** — Managed outputs reject resolving and dangling symlinks.
  Local writes validate recovery records first; restore prepares settings and owned
  exclusion edits before changing files, and repeated Apply retains exclusion ownership.
- **Consistent TUI inspection** — Pages, search, and difference controls use explicit inspection snapshots, keeping file observations stable between reloads and reporting current refresh errors. Differences follow content comparison independently of deployment status, and Context diagnostics no longer depend on display wording.

- **Claude worktree settings** — Context uses the same Git-reported settings worktree as Local disables, including separate Git directories and submodules, and no longer reads settings from Git metadata parents.
- **Malformed rule headers** — Claude and Cursor rules with unterminated frontmatter are reported as ambiguous with a reason instead of inferring loading from incomplete metadata.

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
