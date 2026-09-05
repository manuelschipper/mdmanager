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
