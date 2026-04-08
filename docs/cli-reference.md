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

Open the interactive kanban TUI. Running `sipag` with no arguments does the
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

## sipag feature

Manage the dispatch feature store — raw ideas waiting to be refined into
actionable tickets. Features live under
`~/.sipag/projects/<project>/features/f-<uuid>.md` as markdown files with a
small frontmatter block.

### sipag feature add

Capture a raw idea. Prints the new feature id (`f-...`) on stdout.

```
sipag feature add <TEXT> [--project NAME] [--projects p1,p2,...]
```

| Flag | Description |
|------|-------------|
| `<TEXT>` | The raw idea text (becomes the body of the feature) |
| `-p`, `--project` | Project name (default: from config) |
| `--projects` | Comma-separated list of projects this feature should target |

### sipag feature list

List features in the dispatch store, optionally filtered by status.

```
sipag feature list [--project NAME] [--status raw|grouped|refined|needs-info|active]
```

### sipag feature show

Print a feature's frontmatter and body.

```
sipag feature show <FEATURE_ID> [--project NAME]
```

## sipag refine

Refine one or more raw features into actionable tickets. Spawns a `claude -p`
subprocess and streams its output, parsing the resulting bullets and writing
them back to the feature store. Failed refinements revert to `raw`.

```
sipag refine <FEATURE_ID>... [--project NAME]
```

| Flag | Description |
|------|-------------|
| `<FEATURE_ID>` | One or more feature ids (e.g. `f-abc123 f-def456`) |
| `-p`, `--project` | Project name (default: from config) |

Progress (one bullet per line) prints to stderr; the final ticket list goes
to stdout so you can pipe `sipag refine` into another command.

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
