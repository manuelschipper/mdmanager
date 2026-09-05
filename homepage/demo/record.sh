#!/usr/bin/env bash
# Build the demo image and record assets/workflow.gif (gruvbox-dark) and
# assets/workflow-light.gif (gruvbox-light) from the same tape.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
out=${1:-$here/out}
mkdir -p "$out"

docker build --network host -t mdmanager-demo -f "$here/Dockerfile" "$root"

dark='{"name":"gruvbox-dark","background":"#282828","foreground":"#ebdbb2","cursor":"#ebdbb2","selection":"#504945","black":"#282828","red":"#cc241d","green":"#98971a","yellow":"#d79921","blue":"#458588","magenta":"#b16286","cyan":"#689d6a","white":"#a89984","brightBlack":"#928374","brightRed":"#fb4934","brightGreen":"#b8bb26","brightYellow":"#fabd2f","brightBlue":"#83a598","brightMagenta":"#d3869b","brightCyan":"#8ec07c","brightWhite":"#ebdbb2"}'
light='{"name":"gruvbox-light","background":"#fbf1c7","foreground":"#3c3836","cursor":"#3c3836","selection":"#d5c4a1","black":"#fbf1c7","red":"#cc241d","green":"#98971a","yellow":"#d79921","blue":"#458588","magenta":"#b16286","cyan":"#689d6a","white":"#7c6f64","brightBlack":"#928374","brightRed":"#9d0006","brightGreen":"#79740e","brightYellow":"#b57614","brightBlue":"#076678","brightMagenta":"#8f3f71","brightCyan":"#427b58","brightWhite":"#3c3836"}'

record() {
  local theme=$1 json=$2
  sed -e "s|@THEME@|$theme|g" -e "s|@THEME_JSON@|$json|g" "$here/demo.tape.in" > "$out/demo.$theme.tape"
  docker run --rm --network none -v "$out:/out" -w /out mdmanager-demo "/out/demo.$theme.tape"
}
for theme in ${THEMES:-gruvbox-dark gruvbox-light}; do
  case $theme in gruvbox-dark) record "$theme" "$dark" ;; gruvbox-light) record "$theme" "$light" ;; esac
done
