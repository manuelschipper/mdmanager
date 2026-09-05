# mdmanager.ai design

## Product contract

mdmanager manages persistent **CLAUDE.md** and **AGENTS.md** instruction-file families. Its
awareness is broader than its ownership so Context can explain runtime-native Markdown without
taking control of it.

The interfaces have separate responsibilities:

- Coding agents interpret intent and edit Markdown or manifests.
- The CLI performs every create, adopt, validate, render, apply, disable, and restore operation.
- The TUI is a read-only browser and verifier that reloads after disk changes.

The TUI never stages composition changes and has no editing, save, confirmation, or Apply state.

### Managed

- Global configured **CLAUDE.md** and **AGENTS.md** targets.
- Project root **CLAUDE.md** and **AGENTS.md**.
- Local **CLAUDE.local.md** and **AGENTS.override.md**.

The CLI can create, adopt, compose, render, inspect, and apply these files.

### Storage and composition

All private configuration and state live under one CLI root, `~/.mdmanager/`. Personal Markdown
exists only in `~/.mdmanager/sections/`; Global Profiles and private per-repository compositions
reference those Section IDs directly. Repository composition metadata, ownership state, and backups
live under `projects/`, `state/`, and `backups/` in the same root and are excluded by its
`.gitignore`.

A shared project is self-contained: `<repo>/.mdmanager/project.toml` references Sections under
`<repo>/.mdmanager/sections/`, and both sources and rendered root targets are committed. Reusing a
Personal Section in a shared project is an explicit copy with no automatic synchronization.

Machine-local outputs never require Project setup to reserve paths in committed `.gitignore`.
Local Apply and disable operations first honor any existing Git ignore rule; otherwise their review
plans name one exact rule that they add to the repository's common `.git/info/exclude` after
confirmation. Linked worktrees share that exclusion file. Ownership state records only rules added
by mdmanager so restore leaves repository-provided and pre-existing local rules untouched.

### Observed read-only

- Claude **.claude/rules/**/*.md**, including path-scoped rules.
- Cursor **.cursor/rules/**/*.mdc**, including always, relevant, manual, and glob-scoped rules.
- Direct Markdown imports from **CLAUDE.md** and **CLAUDE.local.md**.
- Ancestor and nested instruction files.
- Runtime exclusions and candidate selection that affect these Markdown files.

Agents configure runtime-native behavior. Context reports the resulting state.

### Outside scope

System prompts, model instruction settings, generated memory, skills, hooks, commands, agents,
output styles, prompt templates, extensions, tools, MCP output, attachments, and conversations.

## Workflow

~~~text
User asks for a change
        │
        ▼
Coding agent reads mdmanager docs start
        │
        ├─ inspects Context and existing files
        ├─ edits the appropriate source scope
        ├─ validates and renders
        └─ applies when authorized
                  │
                  ▼
Read-only TUI reloads
        ├─ resolved load chain
        ├─ source Markdown
        ├─ ownership and status
        └─ meaningful differences
~~~

The user can ask the agent to stop before Apply. Otherwise an authorized configuration request may
include validating and applying the finished result.

## Information architecture

Home keeps five groups in this order:

1. Context
2. Project Instructions
3. Local Instructions
4. Global Instructions
5. Library

Context is first and initially selected because it works without configuration. Actual filenames
are used instead of labels such as “Agents profile.” The active Global Profile appears in the
Global heading. When no Profile is active the heading says **no active Profile**, target rows say
**not applied yet**, and no deployment status is shown. Library has one
**Browse Profiles & Sections** row that opens the scope-aware catalog.

~~~text
┌ mdmanager.ai · ~/repos/shared-app ─────────────────────────────────────┐
│  CONTEXT · Claude                                                      │
│>   3 sources at startup · 2 conditional                               │
│                                                                        │
│  PROJECT INSTRUCTIONS                                                  │
│    AGENTS.md             out of sync                                   │
│    CLAUDE.md             current                                       │
│                                                                        │
│  LOCAL INSTRUCTIONS · this repository + this machine                   │
│    AGENTS.override.md    current · replaces AGENTS.md for Pi/Codex      │
│    CLAUDE.local.md       not found                                     │
│                                                                        │
│  GLOBAL INSTRUCTIONS · PROFILE work (active)                           │
│    ~/.claude/CLAUDE.md   out of sync                                   │
│    ~/.codex/AGENTS.md    current                                       │
│    ~/.pi/agent/AGENTS.md current                                       │
│                                                                        │
│  LIBRARY                                                               │
│    Browse Profiles & Sections · 3 Profiles · 8 Sections                │
├────────────────────────────────────────────────────────────────────────┤
│ ↑↓ select   Enter open   r runtime   ? help   q quit                   │
└────────────────────────────────────────────────────────────────────────┘
~~~

Home is centered and width-limited. Documents, differences, Context, and managed-composition
inspectors use the available screen width.

The Home header always tells the user to ask their coding agent to read **mdmanager docs start**.
Transient status replaces that sentence briefly. The area below the Home box contains navigation
only.

Missing files open an explanation that directs the user to their coding agent. They never open a
creation form.

## Context Runtime picker

The selected Context runtime does not expand Home. Pressing **r** replaces the current content with
a centered, searchable picker. Typing filters, Up/Down selects, Enter chooses, and Esc cancels. This
interaction remains usable as more runtimes are added without visually mixing the picker with Home.

Context Runtime selection, Library, and Profile inspection use focus screens: the normal page
disappears and one content-sized panel contains the information and controls. Library and Profile
panels grow with their rows up to a scrolling height cap. Profiles list every catalog target;
omitted ones are not in this Profile. Context Runtime never filters them. Section documents
remain full-width.

Help is contextual instead: it opens as a modal centered on the current page's occupied canvas and
clamped to the terminal. The whole page beneath it is muted, including its selection, while the
modal uses the normal cyan focus color. Help explains what the visible page means, how to read it,
and the agent-driven change workflow; it does not duplicate the navigation reference already
present in the footer.

## Context

Context resolves one runtime at a time. On wide terminals it pairs the source chain with a
non-focusable preview. Metadata and Markdown have separate borders.

~~~text
┌ mdmanager.ai · Context · Claude ────────────────────────────────────────┐
│ LOADS AT STARTUP                     │ ABOUT                            │
│  1 ~/.claude/CLAUDE.md               │ Path       ./CLAUDE.local.md     │
│> 2 ./CLAUDE.local.md                 │ Status     project               │
│  · ./CLAUDE.md                       │ Why        selected by runtime   │
│      excluded by settings.json       │ Ownership  local · managed       │
│                                      ├──────────────────────────────────┤
│ LOADS WHEN RELEVANT                  │ CONTENTS                         │
│  · .claude/rules/tests.md            │ # Personal instructions         │
│      when files match tests/**       │ Prefer focused tests.            │
├──────────────────────────────────────┴──────────────────────────────────┤
│ ↑↓ source   Shift+↑/↓ preview   Enter open   ←→/r runtime   Esc back   │
└────────────────────────────────────────────────────────────────────────┘
~~~

Only sources that actually load at startup are numbered. Conditional, excluded, skipped, empty, and
nested sources have no sequence number. Reasons use plain language and name the setting or winning
candidate where known.

Below 100 columns, Context shows the chain only and Enter opens the source full-screen. Claude
descendant scanning has no entry cap. It skips **.git** and non-rule symlinked directories; symlinked
Claude rule files and directories are followed. Cursor Context resolves `AGENTS.md` plus
`.cursor/rules/**/*.mdc` from the nearest Git root through the launch directory; it does not scan
descendants below the launch directory.

## Documents

Full Markdown documents use a left-anchored readable-width column with dim line numbers and
dividers. Wrapped continuations have a blank number gutter. **g** accepts a source line number,
centers it in the viewport, and briefly highlights it. Embedded previews remain unnumbered.

## Profiles & Sections

The Library row opens a read-only browser grouped into **Profiles**, **Personal Sections**, and
**This Project**. It includes every declared Section, including unused Personal and Project
Sections. Profiles carry an active badge; Sections show origin, path, and concise usage such as
**not currently used**. Enter opens a Profile inspector that lists each Target composition, or a
Section's Markdown with origin and **Used by** lines in About.

A Profile inspector's title names the selected Profile and whether it is active. Enter reuses the
managed-composition inspector and **d** the unified difference. Targets of a Profile that is not
active are described by comparison — **matches current file** or **differs from current file** —
never with deployment vocabulary such as `current` or `out of sync`, which belongs to the active
Profile alone.

Every Global view carries its Profile in the view reference, so automatic reload can never
silently resolve it against a different Profile. If the browsed Profile disappears from the
manifest, the view falls back through history to the browser.

## Managed compositions and differences

Opening a managed target shows:

- target path and status;
- the generated document;
- the ordered Section sources;
- a preview of the selected document.

This is inspection only. **d** toggles a full-width unified difference when the generated
composition differs from the target on disk. Identical content never appears in duplicate panes.

## Automatic reload

The TUI checks:

- instruction Markdown and candidate files;
- managed manifests and Sections;
- Claude and Cursor rules;
- runtime settings that affect discovery or exclusion;
- mdmanager ownership state.

After a change it rebuilds status and Context while preserving the runtime, selected source path,
open view, and scroll position. It reports **Updated from disk** without opening a modal or moving
focus.

Recursive discovery reload is limited to Git worktrees. Outside Git, known instruction candidates
remain watched, but newly created nested instruction files require a restart.

## Keys

~~~text
Up/Down            select or scroll one line
Shift+Up/Down      scroll the visible document by a page
Enter              inspect
Esc                back; quit only from Home
r                  search and choose a Context runtime
Left/Right         previous/next Context runtime
/                  search an open document
g                  go to a source line
d                  toggle a meaningful managed-target difference
?                  contextual help
q or Ctrl+C        quit
~~~

## CLI

The TUI is **mdmanager** and **mdmanager tui**. Context is independently scriptable:

~~~sh
mdmanager context --runtime claude
mdmanager context --runtime cursor
mdmanager context --runtime codex --json
~~~

Project commands manage **agents** and **claude**. Local commands manage **agents**
(**AGENTS.override.md**) and **claude** (**CLAUDE.local.md**). Global commands retain Profile and
Target arguments. The coding agent uses these CLI commands for every mutation.

## Safety

- Existing files remain external until explicit CLI adoption.
- Adoption is byte-identical.
- Project and Local symlinked targets stay read-only. Global symlinks require explicit replacement
  review and are backed up before replacement.
- Project recovery is Git; Global replacement backups are retained.
- Local ownership hashes prevent overwriting modified or external files.
- Local write reviews include any first-use Git exclusion and are revalidated before the output or
  exclusion is changed.
- Runtime settings are not silently edited to manage observed Claude or Cursor rules or imports.
