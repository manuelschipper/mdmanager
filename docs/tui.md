# TUI

Run **mdmanager** or **mdmanager tui** to open the terminal interface. It never changes instruction
content or applies compositions. Ask a coding agent to read **mdmanager docs start** and use the CLI
while the TUI reloads. The theme picker changes only the UI theme setting.
The startup wordmark lasts 1.8 seconds; any key skips it. The `[ui] theme` in
`~/.mdmanager/mdmanager.toml` also reloads; an invalid name leaves the current palette unchanged.

## Home

Home keeps the centered overview:

1. Context
2. Project Instructions
3. Local Instructions
4. Global Instructions
5. Library

The Library group contains one **Browse Profiles & Sections** row. With no active Profile, the
Global heading says **no active Profile** and target rows say **not applied yet**.

An invalid Global manifest or deployment state opens a diagnostic inside the TUI instead of closing
the app. Esc returns Home, where valid targets and the Library remain browsable. The diagnostic
points the coding agent to **mdmanager doctor**. When the agent repairs the problem, automatic reload
closes the diagnostic.

The Home header always points coding agents at `mdmanager docs start`. Transient status replaces
that sentence briefly.

## Navigation

~~~text
Up/Down            select, or scroll one line
Shift+Up/Down      scroll a document by a page
Enter              inspect the selection
Esc                go back; from Home, quit
r                  choose the Context runtime
t                  search, preview, and save themes
Left/Right         previous/next Context runtime
/                  search an open document
g                  go to a source line
d                  toggle a managed-target difference
?                  contextual help
q or Ctrl+C        quit
~~~

Help overlays and mutes the current page, follows its position, and explains it; Esc returns.

## Theme picker

Press **t** from any page. Typing filters the catalog and Up/Down previews each theme immediately.
Enter saves the selection as `[ui] theme` in `~/.mdmanager/mdmanager.toml`; Esc restores the previous
palette without changing the file.

## Context Runtime picker

Press **r** on Home, in Context, or in an open source. Typing filters, Up/Down selects, Enter
chooses, Esc cancels. It stays usable as the runtime list grows and replaces the screen while
active.

## Context

Context resolves one runtime at a time. Wide terminals show the chain beside a non-focusable
preview; narrower ones stack it below when height permits, or show only the chain. Up/Down
selects, Shift+Up/Down scrolls the preview, and Enter opens the source full-screen.

Only startup sources are numbered to show load order. Conditional rules say when they load.
Excluded, skipped, empty, truncated, and unreadable candidates stay visible with plain-language
reasons. Claude subfolder files form an expandable **Subfolder Instructions** group; empty groups
are hidden.

Source metadata sits in a separate **About** border above **Contents**, so it cannot be mistaken
for Markdown. A symlink row shows where it points; About reports the link's ownership separately
from whether its resolved target is managed.

## Documents

Full Markdown documents are left-aligned in a readable-width column. Dim line numbers and dividers
stay separate; wrapped continuations leave the gutter blank. Press **g** and a source line number
to center and briefly highlight it. Embedded previews remain unnumbered.

## Profiles & Sections

The Library opens every configured Profile, Personal Section, and current-project Section,
including unused Sections. Its compact panel groups them as **Profiles**, **Personal Sections**,
and **This Project**, with active badges, paths, and usage such as **not currently used**. A Profile
panel lists every catalog target; omitted rows are not in this Profile. Section
Markdown remains full-width. **d** opens a composition's difference. Browsing never activates or
applies anything; if a browsed Profile disappears, reload returns here.

## Managed compositions and differences

Opening a managed Project, Local, or Global target shows its generated document and ordered
Section sources, read-only; the agent edits sources and manifests through the CLI. Enter opens the
generated document or a Section.

When the generated composition differs from the target on disk, **d** opens one full-width unified
difference; there is no Apply action. When an agent applies the target, automatic reload closes an
open difference.

## Automatic reload

The TUI checks instruction Markdown, managed manifests and Sections, Claude and Cursor rules, and runtime
settings that affect discovery. After a disk change it recomputes status and Context, preserving
the runtime, selected source, open view, and scroll position. Ordinary changes show a quiet
**Updated from disk** message for three seconds without moving focus; a new configuration or
deployment error opens its diagnostic. Recursive discovery reload is limited to Git worktrees.
Outside Git, known files still reload, but newly created nested instruction files appear after a
restart.
