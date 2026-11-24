#!/usr/bin/env bash
# Runs in the demo image after record.sh cuts CLIP.mp4 from START-END of RECORDING. Encode
# the same span as AV1 and keep CLIP.webm only if it is smaller than the MP4 with no lower
# SSIM. SVT-AV1 and libaom each win on different scenes, so both are swept and the smallest
# qualifying file is kept.
set -euo pipefail
recording=$1 start=$2 end=$3 clip=$4

# SSIM of FILE against the source frames both encoders saw. The input seek leaves the
# source's first timestamp slightly above zero, so both timelines restart at zero to pair
# frame N of the clip with frame N of the source.
ssim() {
  ffmpeg -nostdin -i "$1" -ss "$start" -to "$end" -i "$recording" \
    -lavfi '[0:v]setpts=PTS-STARTPTS[a];[1:v]setpts=PTS-STARTPTS[b];[a][b]ssim' -f null - 2>&1 |
    sed -n 's/.*All:\([0-9.]*\).*/\1/p'
}

mp4_size=$(stat -c %s "$clip.mp4")
mp4_ssim=$(ssim "$clip.mp4")
best_size=$mp4_size
result="none smaller than the MP4 ($mp4_size bytes) at SSIM $mp4_ssim or above"
rm -f "$clip.webm"
for encoder in "libsvtav1 -preset 4" "libaom-av1 -cpu-used 6 -row-mt 1 -b:v 0"; do
  # CRFs run from the smallest file up, so an encoder's first qualifying CRF is its smallest.
  for crf in $(seq 63 -1 0); do
    # shellcheck disable=SC2086 # the encoder string carries its own options
    ffmpeg -nostdin -y -loglevel error -ss "$start" -to "$end" -i "$recording" \
      -an -c:v $encoder -crf "$crf" -pix_fmt yuv420p "$clip.candidate.webm" 2>/dev/null
    size=$(stat -c %s "$clip.candidate.webm")
    # Lower CRFs only make larger files, so none of them can beat the best so far either.
    [ "$size" -lt "$best_size" ] || break
    webm_ssim=$(ssim "$clip.candidate.webm")
    if awk -v a="$webm_ssim" -v b="$mp4_ssim" 'BEGIN { exit !(a >= b) }'; then
      mv "$clip.candidate.webm" "$clip.webm"
      best_size=$size
      result="${encoder%% *} CRF $crf, $size bytes, SSIM $webm_ssim; MP4 $mp4_size bytes, SSIM $mp4_ssim"
      break
    fi
  done
done
rm -f "$clip.candidate.webm"
echo "$clip.webm: $result"
