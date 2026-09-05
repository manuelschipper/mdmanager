# mdmanager.ai documentation

mdmanager.ai manages the `CLAUDE.md` and `AGENTS.md` instruction-file families. Coding agents use
the CLI for instruction and deployment changes; the TUI browses and verifies the result. Its theme
picker writes only the UI preference. Context also explains runtime-native Markdown that affects
those instructions without taking ownership of it.

- [Start](start.md) — agent-assisted setup and safe workflow.
- [Context](context.md) — managed versus observed Markdown.
- [Claude](claude.md) — `CLAUDE.md`, rules, imports, and exclusions.
- [Codex](codex.md) — `AGENTS.md` discovery and overrides.
- [Cursor](cursor.md) — `AGENTS.md` and `.cursor/rules/*.mdc` discovery.
- [Pi](pi.md) — instruction-file selection.
- [Concepts](concepts.md) — scopes, rendering, and status.
- [Configuration](configuration.md) — Global, Project, and Local manifests plus UI preferences.
- [TUI](tui.md) — inspection screens, runtime and theme pickers, and keyboard controls.
- [CLI](cli.md) — the agent-facing command and write contract.
- [Migrate](migrate.md) — adopt existing instructions safely.

These pages ship inside the binary:

```sh
mdmanager docs
mdmanager docs start
mdmanager docs context
mdmanager docs claude
```
