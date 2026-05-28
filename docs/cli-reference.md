# CLI Reference

> **Looking for `sipag configure`?** It moved to a separate tool: [hulma](https://github.com/Dorky-Robot/hulma). Run `hulma configure` to scaffold review agents and slash commands into a project's `.claude/` directory.

All commands operate on data under `~/.sipag/` (override with `SIPAG_DIR`).
Dispatching talks to a katulong server configured at `~/.katulong/remote.json`.

## sipag dispatch

Send a task from the board to its role's katulong session.

```
sipag dispatch <TASK_ID> [--project NAME] [--role NAME]
```

| Flag | Description |
|------|-------------|
| `<TASK_ID>` | Numeric task id (e.g. `42`) |
| `-p`, `--project` | Project name (default: from `config.toml` `default_project`) |
| `-r`, `--role` | Override the task's role |

What it does:

1. Loads the task TOML and the role template
2. Creates (or reuses) the katulong session for the role
3. Optionally creates a worktree for the task (`worktree = true` in the role)
4. Execs the role's command in the session
5. Moves the task to `in-progress`

## sipag up

Spin up the katulong sessions for every role configured in a project — useful
right after registering a new project.

```
sipag up [PROJECT]
```

## sipag tui

Open the interactive board TUI. Running `sipag` with no arguments does the
same thing.

```
sipag tui
```

Key bindings (from the board view):

- `↑`/`k`, `↓`/`j` — move within a column
- `←`/`h`, `→`/`l` — move between columns
- `a` — add a task
- `m` — move the selected task to the next status
- `Enter` — dispatch the selected task
- `q`, `Ctrl-C` — quit

## sipag add

Add a task to the board.

```
sipag add <TITLE> [--project NAME] [--role NAME] [--label TAG[,TAG...]]
```

| Flag | Description |
|------|-------------|
| `<TITLE>` | Task title (free text) |
| `-p`, `--project` | Project name |
| `-r`, `--role` | Role to dispatch with (default: `dev`) |
| `-l`, `--label` | Comma-separated labels |

## sipag list

List tasks on the board, optionally filtered by status.

```
sipag list [--project NAME] [--status STATUS]
```

## sipag move

Move a task to a new status (one of the statuses defined in `project.toml`).

```
sipag move <TASK_ID> <STATUS> [--project NAME]
```

## sipag projects

List every registered project, with the default project marked.

```
sipag projects
```

## sipag project add

Register a new project. The first project you add is set as the default.

```
sipag project add <NAME> --repo <OWNER/REPO>
```

## sipag feature / sipag refine — deprecated 2026-05-17

Earlier versions exposed `sipag feature add | list | show` and
`sipag refine <FEATURE_ID>` for capturing raw ideas and turning them
into actionable tickets via a background `claude -p` subprocess. That
kanban-shaped refinement pipeline was retired in favor of a new
**Experimentation** work model (spike → observe → record) currently
being designed. The CLI surface is gone; running `sipag feature` or
`sipag refine` now fails with clap's "unrecognized subcommand."

Pre-existing files at `~/.sipag/projects/<project>/features/f-*.md`
are left on disk (sipag does not auto-migrate or delete them) but are
no longer reachable from the CLI — read them directly with your
editor if you need to.

See [`docs/architecture.md`](architecture.md) for the current shape.
The deprecated source is preserved in
`sipag-core/src/{feature,refine}.rs` with deprecation banners.

## sipag sub

Subscribe to a katulong pub/sub topic and stream events.

```
sipag sub <TOPIC> [--from-seq N] [--json]
```

| Flag | Description |
|------|-------------|
| `<TOPIC>` | Pub/sub topic (e.g. `crew/katulong/dev/agent-done`) |
| `--from-seq` | Replay from a specific sequence number (default: `0`) |
| `--json` | Print events as JSON, one per line |

Useful for wiring sipag into shell scripts or other agents that react to
crew events.

## sipag version

Print the sipag version and the git SHA it was built from.

```
sipag version
```

`sipag --version` and `sipag -v` are equivalent.

## Environment variables

| Variable | Default | Description |
|----------|---------|-------------|
| `SIPAG_DIR` | `~/.sipag` | Root directory for board state |

The katulong server URL and API key are read from `~/.katulong/remote.json`,
not from environment variables.
