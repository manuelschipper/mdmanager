# Persistent Markdown context

This document records the runtime behavior mdmanager models. Product ownership remains limited to
the `CLAUDE.md` and `AGENTS.md` families.

## Claude

Observed instruction sources:

- organization-managed `CLAUDE.md`
- user `CLAUDE.md` and rules under `~/.claude`, or `CLAUDE_CONFIG_DIR` when set
- ancestor and project `CLAUDE.md` or `.claude/CLAUDE.md`
- `CLAUDE.local.md`
- user and project `.claude/rules/**/*.md`
- nested instruction files loaded when Claude works in their subtree

Rules without `paths` frontmatter load generally. Rules with `paths` load when matching files become
relevant. Symlinked rule files and directories are followed. `claudeMdExcludes` can exclude ordinary
Claude instruction files and rule paths; Context reads applicable settings to report the exclusion
and its source.

`CLAUDE.md` and `CLAUDE.local.md` may directly import Markdown with `@path.md`. Context displays the
resolved direct imports beneath their parent. mdmanager does not adopt, compose, or rewrite them.

## Codex

Codex selects one instruction candidate per directory from its configured project root through the
launch directory. `AGENTS.override.md` precedes `AGENTS.md`, followed by configured fallback names.
Empty candidates are skipped and the configured project-document byte limit applies across loaded
project instructions.

Context reads root markers, fallback names, and the byte budget from Codex configuration only to
explain Markdown selection. It does not manage that configuration.

## Pi

Pi selects one candidate at every directory level. Supported candidates include
`AGENTS.override.md`, `AGENTS.md`, compatible uppercase filenames, and Claude filenames. An empty
winning override may intentionally contribute no text. Its user directory is `~/.pi/agent`, or
`PI_CODING_AGENT_DIR` when set.

Pi system prompts and other prompt machinery are outside mdmanager's discovery and management
scope.

## Cursor

Cursor uses `AGENTS.md` plus Project Rules under `.cursor/rules/`. Rules can load always, for
matching globs, when Cursor decides they are relevant, or when selected manually. Context uses the
nearest Git root as the project root and reports the effective root-to-launch chain; it does not
scan descendants below the launch directory.

Cursor User and Team Rules live in Cursor settings and are not inspectable as Markdown files.
Context reports root `CLAUDE.md` as uncertain, noting that only Cursor CLI documents it. Cursor has
no documented Global Markdown or Local override target.

## Ownership

```text
Global   configured CLAUDE.md / AGENTS.md Targets
Project  root CLAUDE.md / AGENTS.md
Local    CLAUDE.local.md / AGENTS.override.md
```

Claude and Cursor rules, imports, exclusions, ancestor files, and nested files remain observations.
Only the supported Global, Project, and Local instruction-file families can be adopted. Agents make
runtime-side configuration changes; `mdmanager context` verifies their effect.
