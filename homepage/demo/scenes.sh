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
# The recording terminal does not advertise 24-bit colour; without this tmux rounds every
# theme colour to the nearest 256-colour palette entry and tints the light theme peach.
tmux set -as terminal-features ',*:RGB'
tmux set -g pane-border-style "fg=$border"
tmux set -g pane-active-border-style "fg=$border"
tmux set -g window-style "bg=$bg"
tmux set -g escape-time 0

# Each scene becomes one website clip and carries one caption: `scene` logs where it
# starts, on a settled frame, and `cut` where it ends, before the keys that lead on.
# record.sh cuts the clips there; the README GIF burns one scene's caption in below it.
now() { echo "$(date +%s.%N) - $started" | bc; }
scene() { printf '%s\tscene\t%s\t%s\t%s\n' "$(now)" "$1" "$2" "$3" >> "$captions"; }
cut() { printf '%s\tcut\n' "$(now)" >> "$captions"; }
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
  type_cmd "mdmanager"; sleep 0.4; keys Enter
  sleep 3.4                                  # splash

  scene context "Context" "Every file your agent loads, in order, and why."
  sleep 2.2
  keys Enter; sleep 3.0
  keys Down; sleep 2.4
  cut

  keys Escape; sleep 0.4
  for _ in 1 2 3 4 5 6 7 8 9; do keys Down; sleep 0.12; done
  keys Enter; sleep 0.6
  scene library "Library and Profiles" "One library of Sections, a different mix for each machine."
  sleep 2.4
  keys Enter; sleep 3.0
  keys Escape; sleep 0.3; keys Down; sleep 0.2; keys Down; sleep 0.3; keys Enter; sleep 3.0
  cut

  keys Escape; sleep 0.3; keys Down; sleep 0.3; keys Enter; sleep 1.0
  # Start once the agent pane is open, so the loop never jumps between layouts.
  tmux split-window -hb -l 42 -t demo:0 "bash /demo/agent.sh"
  tmux select-pane -t demo:0.right
  sleep 0.4
  scene reload "Live reload" "Your agent edits a shared Section; the TUI reloads as it works."
  until [ -e "$flags/edited" ]; do sleep 0.2; done
  sleep 2.6
  cut

  keys Escape; sleep 0.3; keys Escape; sleep 0.6
  scene review "Review the difference" "One edit leaves Claude, Codex, Cursor and Pi out of sync."
  sleep 2.6
  keys Up; sleep 0.2; keys Up; sleep 0.3; keys d; sleep 3.4
  cut

  scene apply "Apply" "Authorize Apply and every Global target is current again."
  touch "$flags/approve"
  until [ -e "$flags/applied" ]; do sleep 0.2; done
  sleep 0.6
  tmux capture-pane -t demo:0.right -p | grep -q 'LIBRARY' || { keys Escape; sleep 0.4; }
  sleep 3.4
  cut
) &

exec tmux attach -t demo
