#!/usr/bin/env bash
# Build the demo fixture: a Personal library with an applied Global Profile and a
# Project, "harbor", whose CLAUDE.md and AGENTS.md share one Project Section.
set -euo pipefail

mkdir -p "$HOME/.mdmanager/sections" "$HOME/.claude" "$HOME/.codex"
cat > "$HOME/.mdmanager/mdmanager.toml" <<TOML
[ui]
theme = "${MDMANAGER_THEME:-gruvbox-dark}"

[[sections]]
id = "conventions"
name = "Coding conventions"
path = "sections/conventions.md"

[[sections]]
id = "claude"
name = "Claude"
path = "sections/claude.md"

[targets.claude]
path = "~/.claude/CLAUDE.md"
title = "Global Claude"

[targets.codex]
path = "~/.codex/AGENTS.md"
title = "Global Codex"

[profiles.laptop]
claude = ["conventions", "claude"]
codex = ["conventions"]
TOML
cat > "$HOME/.mdmanager/sections/conventions.md" <<'MD'
## Coding conventions

- Keep changes focused on the task.
- Match the style of the surrounding code.
- Explain tradeoffs before changing an API.
MD
cat > "$HOME/.mdmanager/sections/claude.md" <<'MD'
## Claude

- Use plan mode before edits that touch more than one crate.
MD
mdmanager apply laptop --yes >/dev/null

repo="$HOME/projects/harbor"
mkdir -p "$repo/.mdmanager/sections" "$repo/.claude/rules" "$repo/src"
cd "$repo"
git init -q -b main
cat > .mdmanager/project.toml <<'TOML'
format = 1

[[sections]]
id = "common"
name = "Project conventions"
path = "sections/common.md"

[[sections]]
id = "claude"
name = "Claude workflow"
path = "sections/claude.md"

[[sections]]
id = "codex"
name = "Codex workflow"
path = "sections/codex.md"

[targets.claude]
sections = ["common", "claude"]

[targets.agents]
sections = ["common", "codex"]
TOML
cat > .mdmanager/sections/common.md <<'MD'
# Harbor

Harbor is a Rust service for background jobs.

## Project conventions

- Keep HTTP handlers in src/api/.
- Put job processing in src/worker/.
- Run cargo test before handing off a change.
MD
cat > .mdmanager/sections/claude.md <<'MD'
## Claude workflow

- Check .claude/rules/ for path-specific guidance.
MD
cat > .mdmanager/sections/codex.md <<'MD'
## Codex workflow

- Report the commands you ran at the end of a task.
MD
cat > .claude/rules/api.md <<'MD'
---
paths: ["src/api/**"]
---
Every handler returns a typed error; never unwrap in request paths.
MD
mdmanager project apply --yes >/dev/null
git add -A
git -c user.name=demo -c user.email=demo@example.com commit -q -m "Add Harbor instructions"
