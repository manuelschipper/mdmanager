#!/usr/bin/env python3
"""Turn the scene driver's caption log into ASS subtitles and website JSON."""
import json
import sys

log, offset, colour, ass_path, json_path = sys.argv[1:]
offset = float(offset)
rows = []
for line in open(log):
    seconds, text = line.rstrip("\n").split("\t", 1)
    rows.append((max(0.0, float(seconds) - offset), text))
end_of_video = rows[-1][0] + 3600


def stamp(seconds):
    return f"{int(seconds // 3600)}:{int(seconds % 3600 // 60):02d}:{seconds % 60:05.2f}"


# ASS colours are &HBBGGRR; the caption sits centred in the 72px band padded below the video.
rr, gg, bb = colour[1:3], colour[3:5], colour[5:7]
with open(ass_path, "w") as ass:
    ass.write(
        "[Script Info]\nScriptType: v4.00+\nPlayResX: 1060\nPlayResY: 692\nWrapStyle: 0\n\n"
        "[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, OutlineColour, BackColour, "
        "Bold, Italic, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n"
        f"Style: Default,Space Mono,26,&H00{bb}{gg}{rr},&H00000000,&H00000000,0,0,1,0,0,2,20,20,23,1\n\n"
        "[Events]\nFormat: Layer, Start, End, Style, Text\n"
    )
    for (start, text), (end, _) in zip(rows, rows[1:] + [(end_of_video, "")]):
        ass.write(f"Dialogue: 0,{stamp(start)},{stamp(end)},Default,{text}\n")

json.dump([{"start": round(start, 1), "text": text} for start, text in rows], open(json_path, "w"), indent=1)
