# Context

Context answers: "Which persistent Markdown instructions does this runtime get from this launch
directory?"

```sh
mdmanager context --runtime claude
mdmanager context --runtime codex
mdmanager context --runtime cursor
mdmanager context --runtime pi
mdmanager context --runtime xi
mdmanager context --runtime claude --json
```

It reports files that load at startup, sources that load when relevant, candidates skipped because
another filename won, empty or truncated sources, and Claude exclusions. It honors `CODEX_HOME`,
`CLAUDE_CONFIG_DIR`, and `PI_CODING_AGENT_DIR`. Claude subfolder scanning finds `CLAUDE.md`,
`CLAUDE.local.md`, and `.claude/rules/**/*.md` files without a fixed entry limit. It skips `.git` and
non-rule symlinked directories; Claude rule symlinks are followed.

## Managed versus observed

mdmanager can create, adopt, compose, render, and apply only these families:

- `AGENTS.md` and `CLAUDE.md`
- `AGENTS.override.md` and `CLAUDE.local.md`
- configured global `AGENTS.md`/`CLAUDE.md` targets

Context also observes Claude and Cursor rules, subfolder instructions, `@` imports, and exclusions
because they change effective Markdown context. These remain read-only. Ask the user's coding agent
to configure the runtime, then run Context again or watch the open TUI reload to verify the result.

## Deliberate exclusions

mdmanager does not inspect or manage system prompts, generated memory, skills, hooks, slash
commands, output styles, prompt arguments, tool/MCP output, explicit attachments, or conversation
state. A runtime configuration file is read only to explain selection of persistent instruction
Markdown.

See `mdmanager docs claude`, `mdmanager docs codex`, `mdmanager docs cursor`,
`mdmanager docs pi`, or `mdmanager docs xi` for runtime-specific loading and configuration recipes.
