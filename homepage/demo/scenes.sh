#!/usr/bin/env bash
# Drives the recorded tmux session: the TUI on the right, the agent on the left.
set -u
export LANG=C.UTF-8
flags=/tmp/demo
rm -rf "$flags"; mkdir -p "$flags"
cd "$HOME/projects/harbor"

case "${MDMANAGER_THEME:-gruvbox-dark}" in
  gruvbox-light) bg='#fbf1c7'; border='#bdae93' ;;
  *)             bg='#282828'; border='#665c54' ;;
esac
captions=${CAPTIONS:-$flags/captions.tsv}
: > "$captions"
started=$(date +%s.%N)

tmux -u new-session -d -s demo -x "${COLUMNS:-120}" -y "${LINES:-32}" \
  -e MDMANAGER_THEME="${MDMANAGER_THEME:-gruvbox-dark}" \
  "bash --noprofile --rcfile /demo/bashrc"
tmux set -g status off
tmux set -g pane-border-style "fg=$border"
tmux set -g pane-active-border-style "fg=$border"
tmux set -g window-style "bg=$bg"
tmux set -g escape-time 0

# Captions are rendered later, outside the terminal: burned under the README GIF
# and shown as page text beside the website video. Log when each one starts.
say() { printf '%s\t%s\n' "$(echo "$(date +%s.%N) - $started" | bc)" "$1" >> "$captions"; }
keys() { tmux send-keys -t demo:0.right "$@"; }
type_cmd() {
  local text=$1
  for ((i = 0; i < ${#text}; i++)); do
    tmux send-keys -t demo:0.right -l "${text:i:1}"
    sleep 0.08
  done
}

(
  sleep 1.2
  say "Open mdmanager inside a repository."
  type_cmd "mdmanager"; sleep 0.4; keys Enter
  sleep 3.2                                  # splash
  say "Home lists Context, Project, Local, Global and the Library."
  sleep 3.0

  keys Enter; sleep 0.6
  say "Context shows every file Claude loads, in order, and why."
  sleep 2.4
  keys Down; sleep 1.2
  keys Down; sleep 0.4
  say "Rules that load only for matching paths are called out."
  sleep 2.8

  keys Right; sleep 0.5
  say "Press → to compare runtimes. Codex loads a different chain."
  sleep 3.2

  keys Escape; sleep 0.5; keys Down; sleep 0.25; keys Down; sleep 0.4; keys Enter; sleep 0.6
  say "CLAUDE.md is built from Sections; one is shared with AGENTS.md."
  sleep 3.0
  keys Down; sleep 0.3; keys Enter; sleep 0.6
  say "Ask your coding agent. It edits the Section through the CLI."
  sleep 1.0
  tmux split-window -hb -l 42 -t demo:0 "bash /demo/agent.sh"
  tmux select-pane -t demo:0.right
  until [ -e "$flags/edited" ]; do sleep 0.2; done
  sleep 0.8
  say "The TUI reloads from disk while the agent works."
  sleep 3.6

  keys Escape; sleep 0.5; keys d; sleep 0.5
  say "Review the difference. Nothing has been applied yet."
  sleep 3.4
  keys Escape; sleep 0.3; keys Escape; sleep 0.4; keys Up; sleep 0.3; keys Enter; sleep 0.4; keys d; sleep 0.5
  say "AGENTS.md gets the same line: both targets share the Section."
  sleep 3.6

  touch "$flags/approve"
  until [ -e "$flags/applied" ]; do sleep 0.2; done
  sleep 0.5
  say "Authorize Apply. The difference closes on its own."
  sleep 3.0
  keys Escape; sleep 0.5
  say "Every target is current. mdmanager.ai"
  sleep 4
) &

exec tmux attach -t demo
