# Dogfood findings

Claude dogfooded mdmanager.ai 0.1.0 end to end on 2026-08-18. The run used the installed CLI,
embedded documentation, isolated Global and Project configurations, and the live TUI at 30×8,
54×16, 60×16, 80×50, 100×30, 110×30, and 160×45. User configuration and managed targets were
not changed.

Fix these in order. Mark an item complete only after its focused tests and the full suite pass.

## Bugs

- [x] **1. Codex byte count has misleading scope.** Context can say `0/32768 instruction bytes`
  while a Global `AGENTS.md` is loaded because the counter covers only project instructions.
  Fixed by labeling the value `project instruction bytes` in Context and documenting its scope.
- [x] **2. `d` silently does nothing on Home.** Top-level help presents `d` as a TUI shortcut, but
  selecting an out-of-sync target on Home and pressing it gives no response. Either make the
  action work there or scope its documentation and provide feedback. Home now opens the selected
  managed target's difference and gives feedback when no difference is available.
- [x] **3. Narrow rows truncate without an ellipsis.** Profile compositions can end mid-word near
  100 columns, while Library usage can collapse to an unhelpful `used by` near 60 columns. Preserve
  meaning with responsive fields or explicit ellipses. Library and Profile columns now resize and
  ellipsize independently.
- [x] **4. Section usage repeats its label.** Section About can render `Used by: used by default,
  vps`. The value should not repeat `used by` after the label. The duplicated prefix is removed.

## Rough edges

- [x] **5. `mdmanager init` target selection.** The old starter accidentally omitted Claude and
  later hard-coded all three runtimes. `mdmanager init GLOBAL_TARGET...` now creates exactly the
  requested Global targets without deploying any target; bare init creates only the Personal
  Section library.
- [x] **6. Global backups are single-slot.** This finding did not reproduce: backup creation already
  chooses `target.bak`, then `target.1.bak`, and so on without replacement. A regression test now
  verifies that repeated forced applies preserve both recovery points.
- [x] **7. Unknown-value errors are inconsistent.** Invalid runtimes list valid choices, while
  invalid Profiles, targets, and Local targets generally do not. These errors now list the valid
  configured or fixed choices.
- [x] **8. `local adopt` exposes plumbing before the useful verdict.** It requires a manual
  `.git/info/exclude` change before reporting whether existing content matches the composition.
  Adoption now reports a content mismatch before checking the Git exclusion.
- [x] **9. The CLI has no dedicated read-only difference command.** An agent must run `apply`
  without `--yes` to inspect a difference, even though the TUI exposes it directly. Global,
  Project, and Local scopes now each provide a read-only `diff` command.
- [x] **10. Empty Context is mostly blank space.** An unconfigured non-repository directory shows
  `LOADS AT STARTUP · 0` above an empty box instead of a plain-language empty state. It now explains
  that no persistent Markdown instructions were found and hides unavailable source actions.
- [x] **11. Status vocabulary changes across surfaces.** Global uses `modified` or `stale`, Project
  uses `out of sync`, and an inactive Profile uses `differs from current file`. Deployment surfaces
  now share `current`, `out of sync`, `changed on disk`, `existing · not managed by mdmanager.ai`,
  and `not found`; inactive Profiles retain comparison language because they have no deployment
  obligation.
- [x] **12. Context preview requires more than 100 columns.** A tall 80-column terminal shows a
  sparse source chain without using its available height to expose the selected source. Narrow,
  tall terminals now stack the source chain above its preview.

## What held up well

- Precise validation errors and the `--yes`/`--force` write-safety ladder.
- First-use Local Apply and disable reviews expose the exact machine-local Git exclusion before
  confirmation without changing committed repository policy.
- Context explanations for exclusions, candidate priority, rules, imports, and subfolders.
- Automatic reload, including closing an open difference after Apply.
- Live degradation and recovery when a managed manifest becomes invalid.
- The read-only TUI and agent-driven CLI contract.

## Coverage not exercised

- Interactive TTY Apply confirmation.
- Symlinked-target rejection.
- `CODEX_HOME` and `CLAUDE_CONFIG_DIR` overrides.
