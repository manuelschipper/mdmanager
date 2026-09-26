# Claude context

Claude can load persistent instructions from organization policy, the user configuration directory,
ancestor and project `CLAUDE.md` or `.claude/CLAUDE.md`, `CLAUDE.local.md`, and `AGENTS.md`. The user
directory is `~/.claude` unless `CLAUDE_CONFIG_DIR` changes it. Descendant instruction files load when
Claude works in their subtree.

## AGENTS.md

Claude Code 2.1.277 and later read `AGENTS.md` and `.claude/AGENTS.md` directly. By default Claude
reads them only when no `CLAUDE.md`, `.claude/CLAUDE.md`, or `CLAUDE.local.md` exists in the launch
directory or above it; the user and managed `CLAUDE.md` and `.claude/rules/` do not count. Any one of
those files turns off every `AGENTS.md`: `mdmanager context` shows the launch-path files as
`not selected` and omits subfolder files. Otherwise a subfolder `AGENTS.md` loads on demand unless
that subfolder has its own `CLAUDE` file. An `AGENTS.md` that a loading `CLAUDE.md` imports or
symlinks to loads once, through that file. Claude never reads `AGENTS.override.md`,
`AGENTS.local.md`, or anything under `.agents/`.

The **Project instructions** setting changes this: `claude-md-and-agents-md` loads both,
`claude-md` loads `CLAUDE.md` only, and `managed-only` loads only the managed `CLAUDE.md` at launch.
mdmanager reads it from `pluginConfigs["agents-md@builtin"].options.instructionFiles` in user or
managed settings files, and treats `"agents-md@builtin": false` under `enabledPlugins` as
`claude-md` when it is the highest-precedence entry. It does not see MDM or server-managed policy, `--settings` files, or the Claude
Code version; on older versions, or in the first session after upgrading, Claude reads `CLAUDE.md`
files only.

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

The same steps disable a shared `AGENTS.md` that Claude reads directly. Claude matches
`claudeMdExcludes` patterns against absolute file paths. A bare relative name such
as `CLAUDE.md` does not match a project file.

The same setting can exclude an inherited `CLAUDE.md`. Remove only an exclusion the user asked the
agent to remove; broader globs may come from another settings layer.

## Local Instructions

`CLAUDE.local.md` adds to `CLAUDE.md`: Claude reads it after `CLAUDE.md` at that directory level.
It is also a `CLAUDE` file, so in a repository that relies on `AGENTS.md` it stops Claude reading
`AGENTS.md` under the default **Project instructions** setting. `mdmanager local apply claude` warns
before that happens; set **Project instructions** to `claude-md-and-agents-md` in `/config` to keep
both. Use
`mdmanager local create claude SECTION`, `mdmanager local adopt claude SECTION`,
`mdmanager local render claude`, and `mdmanager local apply claude --yes` to manage it. The Local
composition references Personal Sections from `~/.mdmanager/sections/`; do not include the same
content as `CLAUDE.md`, which would load those instructions twice.

Claude system-prompt injection, memory, skills, hooks, commands, agents, and output styles are
outside mdmanager's scope.
