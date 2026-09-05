# Claude context

Claude can load persistent instructions from organization policy, the user configuration directory,
ancestor and project `CLAUDE.md` or `.claude/CLAUDE.md`, and `CLAUDE.local.md`. The user directory is
`~/.claude` unless `CLAUDE_CONFIG_DIR` changes it. Descendant instruction files load when Claude
works in their subtree.

Project and user `.claude/rules/**/*.md` files are also instructions. Rules without `paths` YAML
frontmatter load generally. Rules with `paths` load when Claude works with matching files. Symlinked
rule files and directories are followed. mdmanager shows rules but does not compose or rewrite them.

`CLAUDE.md` can reference another Markdown file with `@path/to/file.md`. mdmanager lists direct
Markdown imports beneath the parent source but does not own the imported file. Prefer mdmanager
Sections when mdmanager owns the target; preserve imports in external files unless the user asks to
change them.

mdmanager models complete `---` frontmatter headers and simple inline or block lists, not general
YAML. A leading `---` without a closing delimiter is reported as ambiguous; close the header
before relying on the reported loading behavior.

## Disable a rule for this repository

To disable a shared Claude rule only on this machine, the user's coding agent should:

1. Add the rule's exact absolute path to `claudeMdExcludes` in
   `.claude/settings.local.json`.
2. Preserve every existing setting and exclusion.
3. Do not rename, blank, or edit the shared rule file.
4. Run `mdmanager context --runtime claude` and verify that the rule is `excluded` and the reason
   names the settings file.

Claude matches `claudeMdExcludes` patterns against absolute file paths. A bare relative name such
as `CLAUDE.md` does not match a project file.

The same setting can exclude an inherited `CLAUDE.md`. Remove only an exclusion the user asked the
agent to remove; broader globs may come from another settings layer.

## Local Instructions

`CLAUDE.local.md` is additive: Claude reads it after `CLAUDE.md` at that directory level. Use
`mdmanager local create claude SECTION`, `mdmanager local adopt claude SECTION`,
`mdmanager local render claude`, and `mdmanager local apply claude --yes` to manage it. The Local
composition references Personal Sections from `~/.mdmanager/sections/`; do not include the same
content as `CLAUDE.md`, which would load those instructions twice.

Claude system-prompt injection, memory, skills, hooks, commands, agents, and output styles are
outside mdmanager's scope.
