#!/usr/bin/env bash
# Build the demo image, record the tape in gruvbox-dark and gruvbox-light, then
# derive the published files from each recording:
#   workflow-THEME.mp4   the website video, captions live beside it as page text
#   workflow-THEME.gif   the README recording with captions burned in below it
#   captions.json        caption start times for the website, from the dark run
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
out=${1:-$here/out}
mkdir -p "$out"

# The scene driver starts about a second before VHS begins recording.
offset=0.9

docker build --network host -t mdmanager-demo -f "$here/Dockerfile" "$root"

dark='{"name":"gruvbox-dark","background":"#282828","foreground":"#ebdbb2","cursor":"#ebdbb2","selection":"#504945","black":"#282828","red":"#cc241d","green":"#98971a","yellow":"#d79921","blue":"#458588","magenta":"#b16286","cyan":"#689d6a","white":"#a89984","brightBlack":"#928374","brightRed":"#fb4934","brightGreen":"#b8bb26","brightYellow":"#fabd2f","brightBlue":"#83a598","brightMagenta":"#d3869b","brightCyan":"#8ec07c","brightWhite":"#ebdbb2"}'
light='{"name":"gruvbox-light","background":"#fbf1c7","foreground":"#3c3836","cursor":"#3c3836","selection":"#d5c4a1","black":"#fbf1c7","red":"#cc241d","green":"#98971a","yellow":"#d79921","blue":"#458588","magenta":"#b16286","cyan":"#689d6a","white":"#7c6f64","brightBlack":"#928374","brightRed":"#9d0006","brightGreen":"#79740e","brightYellow":"#b57614","brightBlue":"#076678","brightMagenta":"#8f3f71","brightCyan":"#427b58","brightWhite":"#3c3836"}'

in_image() { docker run --rm --network none -v "$out:/out" -w /out --entrypoint "$1" mdmanager-demo "${@:2}"; }

record() {
  local theme=$1 json=$2 band text
  case $theme in
    gruvbox-light) band='#f2e5bc'; text='#3c3836' ;;
    *)             band='#32302b'; text='#ebdbb2' ;;
  esac
  sed -e "s|@THEME@|$theme|g" -e "s|@THEME_JSON@|$json|g" "$here/demo.tape.in" > "$out/demo.$theme.tape"
  in_image vhs "/out/demo.$theme.tape"
  python3 "$here/captions.py" "$out/captions-$theme.tsv" "$offset" "$text" "$out/captions-$theme.ass" "$out/captions-$theme.json"
  in_image ffmpeg -y -loglevel error -i "/out/workflow-$theme.mp4" \
    -vf "pad=iw:ih+72:0:0:color=$band,subtitles=/out/captions-$theme.ass,fps=12,split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=5" \
    "/out/workflow-$theme.gif"
}
for theme in ${THEMES:-gruvbox-dark gruvbox-light}; do
  case $theme in gruvbox-dark) record "$theme" "$dark" ;; gruvbox-light) record "$theme" "$light" ;; esac
done
[ -f "$out/captions-gruvbox-dark.json" ] && cp "$out/captions-gruvbox-dark.json" "$here/captions.json"
