#!/usr/bin/env python3
"""Build mdmanager.ai into homepage/dist from the page fragment and repo docs."""

from html import escape
from pathlib import Path
import json
import re
import shutil
import tomllib

import markdown


HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
OUT = HERE / "dist"
SITE = "https://mdmanager.ai"
DESCRIPTION = (
    "Manage your CLAUDE.md and AGENTS.md across machines and runtimes. "
    "Reuse Markdown sections and choose a profile for each setup."
)

package = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]
fragment = (HERE / "fragment.html").read_text()
home_title = re.search(r"<title>(.*?)</title>", fragment)[1]
style = re.search(r"<style>.*?</style>", fragment, re.S)[0]
body = re.sub(r"<title>.*?</title>\s*<style>.*?</style>\s*", "", fragment, count=1, flags=re.S)

# Reuse the vector outlines of the splash mark, without the README background.
hero = (ROOT / "assets/hero.svg").read_text()
mark_paths = re.search(r"<g\b.*?</g>", hero, re.S)[0]
mark_paths = mark_paths.replace('fill="#d79921"', 'fill="currentColor"')
mark = (
    '<svg xmlns="http://www.w3.org/2000/svg" class="mark" viewBox="44 16 282 141" '
    f'role="img" aria-label="MD">{mark_paths}</svg>'
)

# Labels come from the documentation index, as they do in nah's site build.
docs_index = (ROOT / "docs/README.md").read_text()
topics = dict(
    (slug, label)
    for label, slug in re.findall(r"^- \[([^]]+)\]\(([a-z-]+)\.md\)", docs_index, re.M)
)
groups = [
    ("Getting started", ["start", "context", "concepts", "migrate"]),
    ("Runtimes", ["claude", "codex", "cursor", "pi"]),
    ("Reference", ["configuration", "tui", "cli"]),
]


def link(path, label, current):
    selected = ' aria-current="page"' if path == current else ""
    return f'<a href="{path}"{selected}>{escape(label)}</a>'


def sidebar(path):
    rows = [link("/docs/", "Index", path)]
    for title, slugs in groups:
        rows.append(f'<div class="group">{title}</div>')
        rows.extend(link(f"/docs/{slug}/", topics[slug], path) for slug in slugs)
    return '\n'.join(rows)


def page(title, description, path, current, content=None, noindex=False):
    nav = "\n      ".join([
        link("/", "[ overview ]", current),
        link("/docs/", "docs", current),
        link("/news/", "changelog", current),
        '<a href="https://github.com/manuelschipper/mdmanager">github ↗</a>',
    ])
    rendered = body
    if content is not None:
        rendered = re.sub(r"<main\b.*?</main>", lambda _: content, rendered, count=1, flags=re.S)
    for marker, value in {
        "{{NAV}}": nav,
        "{{MARK}}": mark,
        "{{VERSION}}": escape(package["version"]),
    }.items():
        rendered = rendered.replace(marker, value)
    robots = '<meta name="robots" content="noindex">' if noindex else ""
    title = escape(title)
    description = escape(description)
    return f'''<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>{title}</title>
<meta name="description" content="{description}">
<link rel="canonical" href="{SITE}{path}">
<meta property="og:type" content="website">
<meta property="og:url" content="{SITE}{path}">
<meta property="og:title" content="{title}">
<meta property="og:description" content="{description}">
<meta property="og:image" content="{SITE}/assets/context.png">
<meta name="twitter:card" content="summary_large_image">
<meta name="theme-color" content="#282828">
<link rel="icon" type="image/svg+xml" href="/assets/favicon.svg">
{robots}
<script>
try {{
  document.documentElement.classList.toggle('light', localStorage.getItem('mdmanager-theme') === 'light');
}} catch {{
  // Storage can be blocked by browser privacy settings.
}}
</script>
{style}
</head>
<body>
{rendered}
</body>
</html>
'''


def metadata(source):
    title = re.search(r"^# (.+)$", source, re.M)[1]
    paragraphs = source.split("\n\n")[1:]
    description = next(
        block for block in paragraphs
        if block.strip() and not block.startswith(("#", "-", "```", "|"))
    )
    description = re.sub(r"\[([^]]+)\]\([^)]*\)", r"\1", description)
    description = re.sub(r"[`*_]", "", description).replace("\n", " ")
    if len(description) > 160:
        description = description[:157].rsplit(" ", 1)[0] + "…"
    return title, description


def render_markdown(source):
    html = markdown.markdown(source, extensions=["fenced_code", "tables", "toc"])

    def rewrite(match):
        filename, anchor = match.groups()
        if filename == "../CHANGELOG.md":
            path = "/news/"
        elif filename == "README.md":
            path = "/docs/"
        else:
            path = f"/docs/{filename[:-3]}/"
        return f'href="{path}{anchor or ""}"'

    return re.sub(r'href="(\.\./CHANGELOG\.md|README\.md|[a-z-]+\.md)(#[^"]*)?"', rewrite, html)


def write_page(path, html):
    output = OUT / path.lstrip("/") / "index.html"
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(html)


# A clean output directory prevents deleted documentation from staying public.
if OUT.exists():
    shutil.rmtree(OUT)
shutil.copytree(HERE / "assets", OUT / "assets")
shutil.copyfile(HERE / "install.sh", OUT / "install")
(OUT / "_headers").write_text("/install\n  Content-Type: text/plain; charset=utf-8\n  Cache-Control: no-cache\n  X-Content-Type-Options: nosniff\n")
for filename in ["workflow.mp4", "workflow-light.mp4", "context.png"]:
    shutil.copyfile(ROOT / "assets" / filename, OUT / "assets" / filename)
(OUT / "assets/favicon.svg").write_text(
    mark.replace('class="mark"', 'color="#d79921"').replace('44 16 282 141', '34 -64 302 302')
)

write_page("/", page(home_title, DESCRIPTION, "/", "/"))
paths = ["/"]
for source_path in sorted((ROOT / "docs").glob("*.md")):
    source = source_path.read_text()
    slug = source_path.stem
    path = "/docs/" if slug == "README" else f"/docs/{slug}/"
    title, description = metadata(source)
    side = sidebar(path)
    command = "mdmanager docs" if slug == "README" else f"mdmanager docs {slug}"
    content = f'''<main id="content" class="docs">
    <aside><nav aria-label="Documentation">{side}</nav></aside>
    <section>
      <details class="mobile-topics"><summary>Documentation · {escape(title)}</summary>
        <nav aria-label="Documentation">{link('/docs/', 'Index', path)}
        {''.join(link(f'/docs/{slug}/', label, path) for slug, label in topics.items())}</nav>
      </details>
      <div class="document">
        <div class="doc-path">docs/{source_path.name} · {command}</div>
        <article class="article">{render_markdown(source)}</article>
      </div>
    </section>
  </main>'''
    write_page(path, page(f"{title} · mdmanager.ai", description, path, "/docs/", content))
    paths.append(path)

changelog = (ROOT / "CHANGELOG.md").read_text()
changelog = re.sub(r"^## Unreleased\n.*?(?=^## |\Z)", "", changelog, flags=re.M | re.S)
content = f'''<main id="content" class="release"><div class="document">
  <div class="doc-path">CHANGELOG.md</div>
  <article class="article">{render_markdown(changelog)}</article>
</div></main>'''
write_page("/news/", page("Changelog · mdmanager.ai", "mdmanager releases and changes.", "/news/", "/news/", content))
paths.append("/news/")

DEMO_ALT = (
    "Open mdmanager, browse Context, the Library and two Profiles, watch a coding agent edit a "
    "Section shared by Claude, Codex and Pi, review the Global difference, then apply the Profile"
)
captions = json.loads((HERE / "demo/captions.json").read_text())
content = f'''<main id="content" class="demo-panel">
  <a class="back" href="/">← Overview</a>
  <h1 class="caption"><span class="counter"></span><span class="text">The workflow in one minute</span></h1>
  <video class="demo-video" autoplay muted loop playsinline controls
    data-dark="/assets/workflow.mp4" data-light="/assets/workflow-light.mp4"
    aria-label="{escape(DEMO_ALT)}"></video>
</main>
<script>
const video = document.querySelector('.demo-video');
const steps = {json.dumps(captions)};
const counter = document.querySelector('.caption .counter');
const text = document.querySelector('.caption .text');
function source() {{
  const wanted = video.dataset[document.documentElement.classList.contains('light') ? 'light' : 'dark'];
  if (video.getAttribute('src') === wanted) return;
  const at = video.currentTime;
  video.setAttribute('src', wanted);
  video.currentTime = at;
  video.play().catch(() => {{}});
}}
source();
window.addEventListener('themechange', source);
video.addEventListener('timeupdate', () => {{
  let active = -1;
  steps.forEach((step, index) => {{ if (video.currentTime >= step.start) active = index; }});
  if (active < 0) return;
  counter.textContent = `${{active + 1}}/${{steps.length}}`;
  text.textContent = steps[active].text;
}});
</script>
'''
write_page("/demo/", page("Demo · mdmanager.ai", "Browse Context and Profiles, review a shared Section change across runtimes, and apply it with mdmanager.", "/demo/", "", content))
paths.append("/demo/")

(OUT / "404.html").write_text(page(
    "Page not found · mdmanager.ai", "This page could not be found.", "/404.html", "",
    '<main id="content" class="document article"><h1>Page not found</h1><p><a href="/">← Overview</a></p></main>',
    noindex=True,
))
(OUT / "robots.txt").write_text(f"User-agent: *\nAllow: /\n\nSitemap: {SITE}/sitemap.xml\n")
(OUT / "sitemap.xml").write_text(
    '<?xml version="1.0" encoding="UTF-8"?>\n'
    '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n'
    + "".join(f"<url><loc>{SITE}{path}</loc></url>\n" for path in paths)
    + "</urlset>\n"
)
print(f"Built {len(paths)} pages, 404.html, and assets in {OUT}")
