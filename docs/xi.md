# Xi context

Xi loads `~/.xi/AGENTS.md` first, then `AGENTS.md` at each directory from the nearest ancestor
containing a `.git` entry down to the launch directory. A `.git` file counts, so linked worktrees
and submodules use their own directory boundary. Outside Git, the chain starts at the filesystem
root. Ancestors above that boundary and descendants below the launch directory do not load.

Xi reloads these files before each user turn. It skips unreadable files and files larger than
64 KiB entirely; the limit applies to each file, not the combined instructions. Context keeps
oversized sources visible as excluded and empty files visible without a load sequence number.

## Setup and inspection

On a new mdmanager installation, create the Global Xi target without deploying:

```sh
mdmanager init xi
```

For an existing configuration, add a `xi` catalog Target with path `~/.xi/AGENTS.md`, then include
it in the desired Profile. Existing custom `xi` Targets already use this contract. Compose and
apply through the usual Global commands; Project Instructions use target `agents` (`AGENTS.md`).

```sh
mdmanager context --runtime xi
mdmanager context --runtime xi --json
```

Xi is also available in the TUI runtime picker. Global and Project instruction edits reload in
mdmanager and take effect in Xi on its next user turn.

## Runtime boundaries

Context reports Xi's default loading behavior. It does not inspect a running Xi process or its
`--no-global-agents-md` and `--no-project-agents-md` flags, which skip the respective sources for
that run. Check those flags when comparing Context with a live session.

Xi does not select `AGENTS.override.md`, `CLAUDE.md`, or `CLAUDE.local.md`. mdmanager's Local
compositions and `local disable` do not disable or override Xi instructions. Xi's provider
selection does not change these instruction-loading rules; config, hooks, skills, and session
state remain outside this audit.
