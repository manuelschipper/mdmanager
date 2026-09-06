#!/usr/bin/env bash
# The coding-agent pane. The transcript is scripted; the CLI commands are real.
set -u
cd "$HOME/projects/harbor"
flags=/tmp/demo
type_line() {
  local text=$1 delay=${2:-0.03}
  for ((i = 0; i < ${#text}; i++)); do
    printf '%s' "${text:i:1}"
    sleep "$delay"
  done
  printf '\n'
}
step() { printf '  \033[2m●\033[0m %s\n' "$1"; sleep "${2:-0.9}"; }

printf '\033[1;33m❯\033[0m '
sleep 0.6
type_line "Read mdmanager docs start. Add a working"
printf '  '
type_line "agreement for every runtime: say which"
printf '  '
type_line "files you changed when you hand off."
echo
sleep 0.8
step "Reading mdmanager docs start"
step "mdmanager status" 0.7
mdmanager status >/dev/null
step "Editing ~/.mdmanager/sections/agent-common.md" 0.5
echo "- Say which files you changed when you hand off." >> "$HOME/.mdmanager/sections/agent-common.md"
touch "$flags/edited"
sleep 0.9
step "mdmanager diff laptop claude" 0.7
mdmanager diff laptop claude >/dev/null || true
echo
echo "  Agent Common is in every Profile, so"
echo "  Global Claude, Codex and Pi are out of"
echo "  sync. Review them in mdmanager, then"
echo "  tell me to apply."
echo
until [ -e "$flags/approve" ]; do sleep 0.2; done
printf '\033[1;33m❯\033[0m '
sleep 0.4
type_line "Looks good, apply it."
echo
sleep 0.8
step "mdmanager apply laptop --yes" 0.4
mdmanager apply laptop --yes >/dev/null
touch "$flags/applied"
sleep 0.6
echo
echo "  Applied Profile laptop:"
echo "    ~/.claude/CLAUDE.md"
echo "    ~/.codex/AGENTS.md"
echo "    ~/.pi/agent/AGENTS.md"
echo
echo "  Commit sections/agent-common.md to your"
echo "  dotfiles. On the hosted machine, run:"
echo "    mdmanager apply hosted --yes"
sleep 60
