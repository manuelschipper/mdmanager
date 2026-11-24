#!/usr/bin/env python3
"""Turn the scene driver's log into clip cut points, website scenes, and the GIF caption."""
import json
import sys

log, offset, length, duration, colour, gif_scene, ass_path, cuts_path, json_path = sys.argv[1:]
offset, rate = float(offset), float(duration) / float(length)
# A clip ends this long before its logged cut, so the keys that follow never reach it
# even when the video time estimate is slightly late.
margin = 0.3
scenes = []
for line in open(log):
    seconds, event, *fields = line.rstrip("\n").split("\t")
    seconds = max(0.0, (float(seconds) - offset) * rate)
    if event == "scene":
        scene_id, title, text = fields
        scenes.append({"id": scene_id, "title": title, "text": text, "start": seconds})
    else:
        scenes[-1]["end"] = seconds - margin

with open(cuts_path, "w") as cuts:
    for scene in scenes:
        cuts.write(f"{scene['id']}\t{scene['start']:.2f}\t{scene['end']:.2f}\n")

# ASS colours are &HBBGGRR; the caption sits centred in the 72px band padded below the clip,
# shown for the whole clip.
gif = next(scene for scene in scenes if scene["id"] == gif_scene)
rr, gg, bb = colour[1:3], colour[3:5], colour[5:7]
with open(ass_path, "w") as ass:
    ass.write(
        "[Script Info]\nScriptType: v4.00+\nPlayResX: 1060\nPlayResY: 692\nWrapStyle: 0\n\n"
        "[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, OutlineColour, BackColour, "
        "Bold, Italic, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n"
        f"Style: Default,Space Mono,26,&H00{bb}{gg}{rr},&H00000000,&H00000000,0,0,1,0,0,2,20,20,23,1\n\n"
        "[Events]\nFormat: Layer, Start, End, Style, Text\n"
        f"Dialogue: 0,0:00:00.00,9:00:00.00,Default,{gif['text']}\n"
    )

json.dump([{key: scene[key] for key in ("id", "title", "text")} for scene in scenes],
          open(json_path, "w"), indent=1)
