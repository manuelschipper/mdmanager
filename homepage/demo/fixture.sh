#!/usr/bin/env bash
# Build the demo fixture: a Personal library with an applied Global Profile and a
# Project, "harbor", whose CLAUDE.md and AGENTS.md share one Project Section.
set -euo pipefail

mkdir -p "$HOME/.mdmanager/sections" "$HOME/.claude" "$HOME/.codex" "$HOME/.pi/agent"
cat > "$HOME/.mdmanager/mdmanager.toml" <<TOML
[ui]
theme = "${MDMANAGER_THEME:-gruvbox-dark}"

[[sections]]
id = "agent-common"
name = "Agent Common"
path = "sections/agent-common.md"

[[sections]]
id = "claude-common"
name = "Claude Common"
path = "sections/claude-common.md"

[[sections]]
id = "pi-common"
name = "Pi Common"
path = "sections/pi-common.md"

[[sections]]
id = "delegation"
name = "Delegation"
path = "sections/delegation.md"

[[sections]]
id = "delegation-hosted"
name = "Delegation Hosted"
path = "sections/delegation-hosted.md"

[[sections]]
id = "hosted-ops"
name = "Hosted Ops"
path = "sections/hosted-ops.md"

[targets.claude]
path = "~/.claude/CLAUDE.md"
title = "Global Claude"

[targets.codex]
path = "~/.codex/AGENTS.md"
title = "Global Codex"

[targets.pi]
path = "~/.pi/agent/AGENTS.md"
title = "Global Pi"

[profiles.laptop]
claude = ["agent-common", "claude-common", "delegation"]
codex = ["agent-common", "delegation"]
pi = ["agent-common", "pi-common", "delegation"]

[profiles.office]
claude = ["agent-common", "claude-common"]
codex = ["agent-common"]

[profiles.hosted]
claude = ["agent-common", "claude-common", "delegation-hosted", "hosted-ops"]
codex = ["agent-common", "delegation-hosted", "hosted-ops"]
pi = ["agent-common", "pi-common", "delegation-hosted", "hosted-ops"]
TOML
cat > "$HOME/.mdmanager/sections/agent-common.md" <<'MD'
## Working agreements

- Keep changes focused on the task.
- Match the style of the surrounding code.
- Explain tradeoffs before changing an API.
- Run the tests you touched before handing off.
MD
cat > "$HOME/.mdmanager/sections/claude-common.md" <<'MD'
## Claude

- Use plan mode before edits that touch more than one crate.
- Keep CLAUDE.md free of project secrets; use settings for those.
MD
cat > "$HOME/.mdmanager/sections/pi-common.md" <<'MD'
## Pi

- Prefer the built-in file tools over shell pipelines.
MD
cat > "$HOME/.mdmanager/sections/delegation.md" <<'MD'
## Delegation

- Delegate long tasks to a second agent in a worktree.
- Available runtimes: claude, codex, pi.
MD
cat > "$HOME/.mdmanager/sections/delegation-hosted.md" <<'MD'
## Delegation

- This machine runs unattended. Never wait for a human answer.
- Delegate only to codex and pi; claude is reserved for review.
MD
cat > "$HOME/.mdmanager/sections/hosted-ops.md" <<'MD'
## Hosted machine

- Deploys go through `ops deploy`; never restart services by hand.
- Logs live in /var/log/fleet; rotate before they reach 1 GB.
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
