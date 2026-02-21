# CLI Reference

sipag has two command sets: **human-facing commands** you type directly, and **worker commands** that Claude invokes on your behalf.

## Human-facing commands

These are the commands you use as a human operator.

### `sipag start <owner/repo>`

Prime a Claude Code session with the current board state.

```bash
sipag start Dorky-Robot/myapp
```

Dumps open issues, PRs, labels, and recent activity to stdout. Run this inside a Claude Code session — Claude reads the board and begins working with you on priorities.

### `sipag merge <owner/repo>`

Start a merge review conversation.

```bash
sipag merge Dorky-Robot/myapp
```

Primes Claude with all open PRs in the repo. Claude walks through each one, summarizing changes so you can decide: merge, request changes, or skip.

### `sipag setup`

Run the interactive setup wizard.

```bash
sipag setup
```

Creates `~/.sipag/` directories, walks through API key configuration, and sets up lifecycle hooks directory.

### `sipag tui`

Launch the interactive terminal UI. Also the default when running `sipag` with no arguments.

```bash
sipag
sipag tui
```

Shows all tasks across queue, running, done, and failed states. Color-coded by status, keyboard-navigable.

**Keybindings:**

| Key | Action |
|---|---|
| `↑` / `k` | Navigate up |
| `↓` / `j` | Navigate down |
| `r` | Refresh |
| `Enter` | Show task detail |
| `q` / `Esc` | Quit |

### `sipag version`

Print the current sipag version.

```bash
sipag version
```

### `sipag help`

Show help text.

```bash
sipag help
sipag --help
```

---

## Worker commands (Claude's domain)

These commands are invoked by Claude inside a session, not by you directly. Documented here for reference.

### `sipag work <owner/repo>`

Poll GitHub for approved issues and spin up Docker workers.

```bash
sipag work Dorky-Robot/myapp
```

Runs a polling loop: checks for issues labeled `approved`, dispatches one Docker container per issue, and tracks state in `~/.sipag/`. Continues polling until killed.

**Options:**

| Flag | Purpose |
|---|---|
| `--label <label>` | Override the approved label (default: `approved`) |

**Environment overrides:**

| Variable | Purpose |
|---|---|
| `SIPAG_WORK_LABEL` | Label to watch for (default: `approved`) |
| `SIPAG_IMAGE` | Docker image to use |
| `SIPAG_TIMEOUT` | Per-container timeout in seconds |

### `sipag run`

Launch a Docker sandbox for a specific task.

```bash
sipag run --repo https://github.com/org/repo --issue 42 "Fix the auth middleware"
sipag run --repo https://github.com/org/repo -b "Add dark mode"   # background
```

**Options:**

| Flag | Purpose |
|---|---|
| `--repo <url>` | Repository URL (required) |
| `--issue <n>` | GitHub issue number to associate |
| `-b` | Run in background |

### `sipag ps`

List running and recent tasks with status.

```bash
sipag ps
```

Shows all tasks across queue, running, done, and failed directories with timestamps and status.

### `sipag logs <id>`

Print the log for a task.

```bash
sipag logs 20240101120000-fix-bug
```

The task ID is the filename (without `.md`) of the tracking file in `running/` or `done/` or `failed/`.

### `sipag kill <id>`

Kill a running container and move the task to `failed/`.

```bash
sipag kill 20240101120000-fix-bug
```

### `sipag status`

Show queue state across all directories.

```bash
sipag status
```

Counts tasks in each state: pending, running, done, failed.

### `sipag show <name>`

Print a task file and its log.

```bash
sipag show fix-bug
```

### `sipag retry <name>`

Move a failed task back to `queue/` for re-processing.

```bash
sipag retry fix-bug
```

---

## Queue commands (Claude's domain)

### `sipag add`

Add a task to the queue.

```bash
sipag add "Implement password reset" --repo myapp
sipag add "Fix the flaky test" --repo myapp --priority high
```

**Options:**

| Flag | Purpose |
|---|---|
| `--repo <name>` | Registered repo name |
| `--priority <level>` | Priority: `low`, `medium`, `high` |

Without `--repo`, appends to `./tasks.md` (legacy checklist mode).

### `sipag repo add <name> <url>`

Register a repo name → URL mapping.

```bash
sipag repo add myapp https://github.com/org/myapp
```

### `sipag repo list`

List registered repos.

```bash
sipag repo list
```

### `sipag init`

Create `~/.sipag/{queue,running,done,failed}` directories.

```bash
sipag init
```

---

## Legacy checklist commands

These operate on a `tasks.md` markdown checklist file and predate the Docker executor.

### `sipag next`

Run `claude` on the next pending checklist task.

```bash
sipag next         # run next task
sipag next -c      # mark current task complete without running
sipag next -n      # dry run — print task without executing
sipag next -f path # use a specific checklist file
```

### `sipag list`

Print all tasks with status from a checklist.

```bash
sipag list
sipag list -f path/to/tasks.md
```

---

[Configuration →](configuration.md){ .md-button .md-button--primary }
[How it works →](how-it-works.md){ .md-button }
