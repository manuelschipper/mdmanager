# Codex context

Codex loads one instruction candidate per directory from its configured project root to the launch
directory. The normal priority is `AGENTS.override.md`, then `AGENTS.md`, followed by configured
fallback filenames. Empty candidates are skipped. Project instructions are limited by Codex's
configured byte budget.

The Context summary's byte counter covers project instructions only. Global Codex instructions do
not consume this budget even though they appear in the same startup chain.

mdmanager reads Codex configuration only to explain root markers, fallback filenames, and the byte
budget. It does not manage Codex TOML or system/model instruction settings.

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
