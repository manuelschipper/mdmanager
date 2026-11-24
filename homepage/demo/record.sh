#!/usr/bin/env bash
# Build the demo image, record the tape in gruvbox-dark and gruvbox-light, then cut each
# recording into one clip per scene and publish:
#   assets/demo/SCENE.mp4, SCENE-light.mp4   the website clips, captions beside them as page text
#   assets/demo/SCENE.webm, SCENE-light.webm the same clips in AV1, only where smaller at equal quality
#   assets/review.gif                        the README clip with its caption burned in below it
#   homepage/demo/scenes.json                scene titles and captions for the website, from the dark run
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
out=${1:-$here/out}
mkdir -p "$out" "$root/assets/demo"

# The scene driver starts about a second before VHS begins recording, and VHS records
# `length` seconds. It captures frames slower than real time, so its video runs a few
# percent shorter than the session; captions.py scales logged times to video times.
offset=1.0
length=54

docker build --network host -t mdmanager-demo -f "$here/Dockerfile" "$root"

dark='{"name":"gruvbox-dark","background":"#282828","foreground":"#ebdbb2","cursor":"#ebdbb2","selection":"#504945","black":"#282828","red":"#cc241d","green":"#98971a","yellow":"#d79921","blue":"#458588","magenta":"#b16286","cyan":"#689d6a","white":"#a89984","brightBlack":"#928374","brightRed":"#fb4934","brightGreen":"#b8bb26","brightYellow":"#fabd2f","brightBlue":"#83a598","brightMagenta":"#d3869b","brightCyan":"#8ec07c","brightWhite":"#ebdbb2"}'
light='{"name":"gruvbox-light","background":"#fbf1c7","foreground":"#3c3836","cursor":"#3c3836","selection":"#d5c4a1","black":"#fbf1c7","red":"#cc241d","green":"#98971a","yellow":"#d79921","blue":"#458588","magenta":"#b16286","cyan":"#689d6a","white":"#7c6f64","brightBlack":"#928374","brightRed":"#9d0006","brightGreen":"#79740e","brightYellow":"#b57614","brightBlue":"#076678","brightMagenta":"#8f3f71","brightCyan":"#427b58","brightWhite":"#3c3836"}'

in_image() { docker run --rm --network none -v "$out:/out" -w /out --entrypoint "$1" mdmanager-demo "${@:2}"; }

record() {
  local theme=$1 json=$2 suffix= duration
  [ "$theme" = gruvbox-light ] && suffix=-light
  sed -e "s|@THEME@|$theme|g" -e "s|@THEME_JSON@|$json|g" -e "s|@LENGTH@|$length|g" \
    "$here/demo.tape.in" > "$out/demo.$theme.tape"
  in_image vhs "/out/demo.$theme.tape"
  duration=$(in_image ffprobe -v error -show_entries format=duration -of csv=p=0 "/out/workflow-$theme.mp4")
  python3 "$here/captions.py" "$out/captions-$theme.tsv" "$offset" "$length" "$duration" '#ebdbb2' review \
    "$out/review.ass" "$out/cuts-$theme.tsv" "$out/scenes-$theme.json"
  while IFS=$'\t' read -r scene start end; do
    clip=$scene$suffix
    in_image ffmpeg -y -loglevel error -ss "$start" -to "$end" -i "/out/workflow-$theme.mp4" \
      -an -c:v libx264 -preset veryslow -crf 26 -pix_fmt yuv420p -movflags +faststart "/out/$clip.mp4"
    cp "$out/$clip.mp4" "$root/assets/demo/"
    # AV1 wins on some scenes and loses on others, so webm.sh publishes a WebM beside the
    # MP4 only where one is smaller at no lower SSIM.
    rm -f "$out/$clip.webm" "$root/assets/demo/$clip.webm"
    in_image bash /demo/webm.sh "workflow-$theme.mp4" "$start" "$end" "$clip"
    if [ -f "$out/$clip.webm" ]; then cp "$out/$clip.webm" "$root/assets/demo/"; fi
  done < "$out/cuts-$theme.tsv"
}
for theme in ${THEMES:-gruvbox-dark gruvbox-light}; do
  case $theme in gruvbox-dark) record "$theme" "$dark" ;; gruvbox-light) record "$theme" "$light" ;; esac
done

# The README stays dark, so only the dark run makes the GIF and the website's scene list.
if [ -f "$out/scenes-gruvbox-dark.json" ]; then
  cp "$out/scenes-gruvbox-dark.json" "$here/scenes.json"
  in_image ffmpeg -y -loglevel error -i /out/review.mp4 \
    -vf "pad=iw:ih+72:0:0:color=#32302b,subtitles=/out/review.ass,fps=12,split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=5" \
    /out/review.gif
  cp "$out/review.gif" "$root/assets/review.gif"
fi
