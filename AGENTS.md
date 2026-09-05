# Agent instructions

## Keep the changelog current

Update [CHANGELOG.md](CHANGELOG.md) in the same change whenever you make a material
user-visible change: runtime discovery, instruction composition or deployment,
CLI or TUI behavior, or a documented workflow or limitation.

- Collect changes under `## Unreleased` at the top, with the newest release first.
  When cutting a release, rename that heading to
  `## mdmanager X.Y.Z — Mon D, YYYY`, using the version in `Cargo.toml`.
- Write one bullet per change: a **bold label** naming it, followed by a plain
  description of what changed and why a user would care. Use a technical voice.
- Internal refactors, trivial copy edits, and cosmetic cleanup need no entry
  unless they change observable behavior or performance.
- Keep entries for shipped releases unchanged.
