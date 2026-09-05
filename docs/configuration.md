# Configuration

mdmanager.ai separates human-edited configuration from generated ownership state and backups.

## Personal library and Global Profiles

Personal configuration lives under `~/.mdmanager/`. The root can be linked from dotfiles; its
generated `.gitignore` excludes machine-only `projects/`, `state/`, and `backups/`.

Create only the Global targets this machine should manage:

```sh
mdmanager init claude codex
```

Bare `mdmanager init` creates the Personal Section library without Global targets or Profiles.

```text
~/.mdmanager/
  mdmanager.toml
  sections/
  projects/
  state/
  backups/
```

```toml
[ui]
theme = "gruvbox-dark"

[[sections]]
id = "common"
name = "Common"
path = "sections/common.md"

[[sections]]
id = "claude"
name = "Claude"
path = "sections/claude.md"

[targets.codex]
path = "~/.codex/AGENTS.md"
title = "Global Codex"

[targets.claude]
path = "~/.claude/CLAUDE.md"
title = "Global Claude"

[profiles.default]
codex = ["common"]
claude = ["common", "claude"]
```

Section declaration order is the inspection order. Profile keys form the deploy set; each
Profile/Target array controls Section order. Profiles do not inherit. Rendering adds
`# TARGET_TITLE` before the selected Sections.

`theme` accepts `ayu-dark`, `ayu-light`, `catppuccin-frappe`, `catppuccin-latte`,
`catppuccin-macchiato`, `catppuccin-mocha`, `dracula`, `everforest-dark`, `github-dark`,
`github-light`, `gruvbox-dark`, `gruvbox-light`, `kanagawa-wave`, `material-ocean`, `monokai`,
`night-owl`, `nord`, `one-dark-pro`, `rose-pine`, `rose-pine-dawn`, `solarized-dark`,
`solarized-light`, `tokyo-night`, or `vitesse-dark`. The TUI reloads valid selections and keeps its
current palette when a name is invalid. Press **t** in the TUI to filter and preview the catalog;
Enter saves the selection to this file and Esc cancels it.

Global requirements:

- IDs use lowercase ASCII letters, digits, `-`, or `_`, and start with a letter.
- Section paths are relative and their UTF-8 files are non-empty.
- Target paths remain beneath home and end with `AGENTS.md` or `CLAUDE.md`.
- Section IDs and expanded target paths are unique.
- `[targets.*]` is the shared catalog. Each Profile names a non-empty subset of those Targets.
  Each named list is non-empty and duplicate-free. Unknown Target keys fail. An empty Profile
  fails.

Omitting a catalog Target from a Profile stops that Profile from deploying it and does not
delete an existing file. Removing `[targets.pi]` from the catalog drops Pi fleet-wide.

The TUI writes only `[ui].theme`. A coding agent edits compositions, validates them with `render`
and `status`, and applies them through the CLI.

A Global Target is a regular file owned by mdmanager after Apply. If multiple runtime paths should
read exactly the same document, keep only the canonical path in `targets` and let a coding agent
offer explicitly approved symlinks from the other paths. Those aliases are external: mdmanager
reports them but does not create or maintain them. A configured Target found as a symlink is instead
protected as an unmanaged or changed deployment and Apply replaces it only after replacement review.

## Committed project composition

An opted-in Git repository keeps this source beside its generated root files:

```text
.mdmanager/
  project.toml
  sections/
    common.md
    claude.md
AGENTS.md
CLAUDE.md
```

```toml
format = 1

[[sections]]
id = "common"
name = "Common"
path = "sections/common.md"

[[sections]]
id = "claude"
name = "Claude"
path = "sections/claude.md"

[targets.agents]
sections = ["common"]

[targets.claude]
sections = ["common", "claude"]
```

Project Section paths are relative to `.mdmanager/`. The `agents` and `claude` targets map to root
`AGENTS.md` and `CLAUDE.md`. Lists are non-empty and duplicate-free. One Section is preserved
byte-for-byte; multiple Sections are joined with one blank line. Symlinked targets are rejected.

## Local Instructions

Local Instructions reuse personal Section IDs. Their composition is keyed by the repository's common
Git directory under `~/.mdmanager/projects/`.

```text
~/.mdmanager/projects/<repository-id>/
  local.toml
```

```toml
format = 1

[targets.agents]
sections = ["common"]

[targets.claude]
sections = ["common", "claude"]
```

The `agents` composition renders `AGENTS.override.md`; `claude` renders `CLAUDE.local.md`.

The agent authors or selects a personal Section, then creates or adopts the Local composition. It can
edit each ordered list to add Sections. Missing referenced Sections make the composition invalid.

Do not hand-edit `~/.mdmanager/state/`. Global backups are in `~/.mdmanager/backups/`.
Invalid `overlays.toml` blocks writes; use your valid copy or recover manually.
Local has no backup or repair; `doctor` repairs Global only. Local writers in the same
repository (including linked worktrees) serialize through `overlays.lock` beside
`overlays.toml`. Waiting begins after review; the plan is checked again under the lock.
Read-only inspection creates no lock. The lock coordinates mdmanager writers, not
external editors, and does not make output, Git exclusion and state writes atomic.

After a Local I/O failure, inspect the output, common Git `info/exclude`, and
`overlays.toml` before retrying. Preserve copies of all three and stop other Local
writers before manual recovery:

- If only the exclusion was written and the output is unchanged, remove the I/O
  obstruction, review a fresh plan, and retry. The existing exclusion is now treated as
  user-owned and will remain after restore; remove that exact line manually only if it
  is no longer needed by any worktree.
- If the output was written but ownership was not saved, status reports an external
  managed output or no owned disable. Apply/disable refuses to claim it on retry.
  Preserve any subsequent user edits. Reconcile the output manually (remove only the
  known empty Pi suppression, or remove only the selected logical source string from
  Claude's `claudeMdExcludes`; retain all unrelated settings). For a managed output,
  move it aside for comparison before reviewing a fresh Apply. Do not adopt a partial
  output merely to bypass the refusal. A failed adoption can also leave `local.toml`
  without ownership; preserve that composition and move the unowned output aside
  before reviewing Apply.
- If restore replaced or removed the output but exclusion/state persistence failed,
  the recovery record remains. Status reports missing or modified suppression and
  ordinary restore refuses. Do not recreate an output from stale recovery content over
  user edits. Manually finish only the recorded suppression removal, retain exclusions
  still needed by other worktrees, and remove only that operation's ownership entry
  from `overlays.toml` after its output and exclusion effects are reconciled. Preserve
  every unrelated entry. This is the exceptional manual state recovery path; there is
  no automatic rollback or Local repair command.

Restore validates recorded destinations and source scope without requiring the source
file still to exist. Claude ownership is shared across the repository's worktrees;
Pi and managed outputs belong to their recorded worktree. Invalid destinations are
refused, never rewritten. Claude settings must be an ordinary file: resolving and
dangling symlinks are refused during review, Apply and restore.
Context runtime settings remain outside these manifests.

## Sharing a personal Section

A committed project never references `~/.mdmanager/`. Copy personal Markdown into
`<repo>/.mdmanager/sections/` and declare it in `project.toml`; the copy no longer synchronizes.
