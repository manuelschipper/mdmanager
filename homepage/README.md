# mdmanager.ai homepage

A static site, organized like nah's `homepage/`. Edit `fragment.html` for the
layout, styles, and homepage copy. `build.py` renders the repository's `docs/*.md`
at `/docs/` and `CHANGELOG.md` at `/news/`; edit those sources to change their
content. Unreleased changelog entries stay out of the public site.

The build copies the bundled Space Mono fonts from `assets/` and reuses the
repository's hero artwork and workflow recording. It reads the version and Rust
requirement from `Cargo.toml`.

## Build and preview

From the repository root, with Python 3.11 or newer:

```sh
python3 -m venv homepage/.venv
homepage/.venv/bin/pip install -r homepage/requirements.txt
homepage/.venv/bin/python homepage/build.py
python3 -m http.server 8090 --bind 127.0.0.1 --directory homepage/dist
```

Open <http://127.0.0.1:8090>. Serve the generated `homepage/dist/` directory at the
root of `mdmanager.ai`, with directory indexes and `404.html` as the error page.
Rebuild after changing docs, the changelog, or site sources. Generated output is
ignored by Git. The site needs no application server; only the theme toggle and
copy button use JavaScript. Clipboard access requires HTTPS or localhost.

## Re-record the demo

`demo/` records the workflow demo from a clean Docker installation with VHS. The
fixture is a fictional Personal library with three machine Profiles over Claude, Codex
and Pi, and a `harbor` Project as Context backdrop. `scenes.sh` drives the TUI in
tmux and logs when each caption starts; `agent.sh` plays the coding-agent pane with a
scripted transcript around real CLI commands.

```sh
homepage/demo/record.sh
cp homepage/demo/out/workflow-gruvbox-dark.mp4 assets/workflow.mp4
cp homepage/demo/out/workflow-gruvbox-light.mp4 assets/workflow-light.mp4
cp homepage/demo/out/workflow-gruvbox-dark.gif assets/workflow.gif
```

Both themes come from the same tape. The site plays the video matching its theme
with the captions beside it as page text, from `demo/captions.json`, which
`record.sh` refreshes. The README GIF carries the same captions burned in below the
terminal and stays dark like the rest of the README. Building the image compiles mdmanager from the checkout, so the recording
always matches the source it ships with.
