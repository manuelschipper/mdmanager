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
type_line "Read mdmanager docs start. Add a project"
printf '  '
type_line "convention: add a regression test when"
printf '  '
type_line "fixing a bug. Prepare it for my review."
echo
sleep 0.8
step "Reading mdmanager docs start"
step "mdmanager context --runtime claude" 0.7
mdmanager context --runtime claude >/dev/null
step "Editing .mdmanager/sections/common.md" 0.5
echo "- Add a regression test when fixing a bug." >> .mdmanager/sections/common.md
touch "$flags/edited"
sleep 0.9
step "mdmanager project check" 0.7
mdmanager project check >/dev/null
echo
echo "  common.md feeds both CLAUDE.md and"
echo "  AGENTS.md, so both are now out of sync."
echo "  Review them in mdmanager, then tell me"
echo "  to apply."
echo
until [ -e "$flags/approve" ]; do sleep 0.2; done
printf '\033[1;33m❯\033[0m '
sleep 0.4
type_line "Looks good, apply it."
echo
sleep 0.8
step "mdmanager project apply --yes" 0.4
mdmanager project apply --yes >/dev/null
touch "$flags/applied"
sleep 0.6
echo
echo "  Applied. CLAUDE.md and AGENTS.md are"
echo "  current. Files to commit:"
echo "    .mdmanager/sections/common.md"
echo "    CLAUDE.md"
echo "    AGENTS.md"
sleep 60
