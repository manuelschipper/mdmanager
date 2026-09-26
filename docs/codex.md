# Codex context

Codex loads one instruction candidate per directory from its configured project root to the launch
directory. The normal priority is `AGENTS.override.md`, then `AGENTS.md`, followed by configured
fallback filenames. The first existing regular file wins, even when it is empty or contains only
whitespace: an empty project override prevents the regular `AGENTS.md` from loading. At the global
level, Codex instead skips empty or whitespace-only candidates and tries the next one. Duplicate
fallback names and entries that are not filenames are ignored.

Project instructions share a byte budget (32 KiB by default). Codex truncates a selected file to
the remaining budget before checking for whitespace-only content, and stops loading project files
when the budget is exhausted. Setting `project_doc_max_bytes = 0` disables project instructions.
An explicitly empty `project_root_markers = []` limits discovery to the launch directory.

The Context summary's byte counter covers project instructions only. Global Codex instructions do
not consume this budget even though they appear in the same startup chain.

mdmanager reads `$CODEX_HOME/config.toml` (normally `~/.codex/config.toml`) to explain root markers,
fallback filenames, the byte budget, and explicit project trust. An `untrusted` project loads no
project instructions; global instructions remain eligible. Trust lookup checks the launch directory
before the Git repository's main checkout, trying canonical paths before path aliases. A matching
launch-directory entry takes precedence even when it has no trust level.

These rules follow Codex CLI 0.156.1. Context does not merge Codex's system, project `.codex`, profile,
or command-line configuration layers, so overrides in those layers can change the actual load chain.
It does not manage Codex TOML or system/model instruction settings.

## Managed files

- Global Codex instructions normally target `~/.codex/AGENTS.md`.
- Project Instructions target root `AGENTS.md`.
- Local Instructions target `AGENTS.override.md`, which replaces the regular candidate for Codex
  and Pi in that directory.

After an agent changes Codex discovery settings, run:

```sh
mdmanager context --runtime codex
```

Model/system instructions, skills, tool configuration, and conversation state are outside scope.
