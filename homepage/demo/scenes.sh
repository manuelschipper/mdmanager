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
  say "Home: Context, Project, Local and Global, plus your Library."
  sleep 3.0

  keys Enter; sleep 0.6
  say "Context shows every file Claude loads, in order, and why."
  sleep 2.4
  keys Down; sleep 1.2
  keys Right; sleep 0.5
  say "Press → for Codex. The first file in each chain is Global."
  sleep 3.0

  keys Escape; sleep 0.4
  for _ in 1 2 3 4 5 6 7 8; do keys Down; sleep 0.12; done
  keys Enter; sleep 0.5
  say "The Library: your Profiles and the Sections they share."
  sleep 3.4
  keys Enter; sleep 0.5
  say "Profile laptop: each runtime gets the Sections it needs."
  sleep 3.6
  keys Escape; sleep 0.3; keys Down; sleep 0.2; keys Down; sleep 0.3; keys Enter; sleep 0.5
  say "Profile hosted: same library, another machine, another mix."
  sleep 3.6
  keys Escape; sleep 0.3; keys Down; sleep 0.3; keys Enter; sleep 0.5
  say "Agent Common is used by every Profile. Edit it once."
  sleep 2.4
  keys m; sleep 0.4
  say "Documents render as Markdown. Press m for the raw source."
  sleep 2.6
  keys m; sleep 0.6

  tmux split-window -hb -l 42 -t demo:0 "bash /demo/agent.sh"
  tmux select-pane -t demo:0.right
  until [ -e "$flags/edited" ]; do sleep 0.2; done
  sleep 0.8
  say "Your agent edits the Section. The TUI reloads as it works."
  sleep 3.2

  keys Escape; sleep 0.3; keys Escape; sleep 0.5
  say "One edit: Claude, Codex and Pi are all out of sync."
  sleep 3.6
  keys Up; sleep 0.2; keys Up; sleep 0.2; keys Up; sleep 0.3; keys d; sleep 0.5
  say "Review the Global difference before anything is applied."
  sleep 3.2

  touch "$flags/approve"
  until [ -e "$flags/applied" ]; do sleep 0.2; done
  sleep 0.6
  tmux capture-pane -t demo:0.right -p | grep -q 'LIBRARY' || { keys Escape; sleep 0.4; }
  say "Authorize Apply. All three targets are current again."
  sleep 3.2
  say "Keep the library in dotfiles. Each machine applies its Profile."
  sleep 4
) &

exec tmux attach -t demo
