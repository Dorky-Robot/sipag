# sipag

<div align="center">

<img src="sipag.jpg" alt="sipag" width="300">

*Spin up isolated Docker sandboxes. Make progress visible.*

</div>

## What is sipag?

sipag is a sandbox launcher for Claude Code. Claude Code is the orchestrator — it decides what to work on, in what order, and handles retries. sipag does one thing well: spinning up isolated Docker sandboxes and making progress visible.

```bash
# Launch a Docker sandbox for a task
sipag run --repo https://github.com/org/repo --issue 21 "Simplify sipag to sandbox launcher"

# Watch what's happening
sipag ps
sipag logs <task-id>

# If something goes wrong
sipag kill <task-id>

# Open the interactive TUI (no args)
sipag
```

## How it works

```
sipag run → Docker container → clone repo → claude -p → PR
```

1. Claude Code calls `sipag run` with a repo URL and task description
2. sipag generates a unique task ID, creates a tracking file in `running/`
3. Spins up a Docker container: clones the repo, injects credentials
4. Runs `claude --dangerously-skip-permissions` — Claude plans, codes, tests, commits, pushes, opens a PR
5. Records the result in `done/` or `failed/` with a log file

The container is the safety boundary. Claude has full autonomy inside it.

## Installation

### From source (Rust + Cargo required)

```bash
cargo install --path sipag
```

Or use the Makefile:

```bash
make install
```

This installs `sipag` to `~/.cargo/bin/sipag`.

### Build without installing

```bash
make build
# Binary at: target/release/sipag
```

## CLI

### Sandbox commands (primary interface for Claude Code)

```
sipag run --repo <url> [--issue <n>] [-b] "<task>"
                              Launch a Docker sandbox for a task
sipag ps                      List running and recent tasks with status
sipag logs <id>               Print the log for a task
sipag kill <id>               Kill a running container, move task to failed/
```

### Queue commands (batch processing)

```
sipag start                   Process queue/ serially (uses sipag run internally)
sipag add <title> --repo <name> [--priority <level>]
                              Add a task to queue/
sipag status                  Show queue state across all directories
sipag show <name>             Print task file and log
sipag retry <name>            Re-queue a failed task
sipag repo add <name> <url>   Register a repo name → URL mapping
sipag repo list               List registered repos
```

### Legacy checklist commands

```
sipag add "<title>"           Append task to ./tasks.md (no --repo)
sipag list [-f <file>]        List tasks from a markdown checklist file
sipag next [-c] [-n] [-f]     Run claude on the next pending checklist task
```

### Utility

```
sipag init                    Create ~/.sipag/{queue,running,done,failed}
sipag version                 Print version
sipag help                    Show help
sipag tui                     Launch interactive TUI (same as no args)
```

## TUI

Running `sipag` with no arguments (or `sipag tui`) opens the interactive terminal UI:

- Scrollable task list across all states (queue, running, done, failed)
- Color-coded by status: yellow=queued, cyan=running, green=done, red=failed
- Keyboard navigation: `↑`/`k` up, `↓`/`j` down, `r` refresh, `q`/`Esc` quit

## sipag run

```bash
sipag run --repo <url> [--issue <n>] [-b|--background] "<task description>"
```

- `--repo <url>` — repository URL to clone inside the container (required)
- `--issue <n>` — GitHub issue number to associate with this task (optional)
- `-b`, `--background` — run in background; sipag returns immediately (default: foreground)

On launch, sipag:

1. Auto-inits `~/.sipag/` if needed
2. Generates a task ID: `YYYYMMDDHHMMSS-slug`
3. Prints the task ID so you can follow up with `sipag logs` or `sipag kill`
4. Creates a tracking file in `running/` with repo, issue, started timestamp
5. Streams container output to a log file in `running/`
6. On completion, moves tracking file + log to `done/` or `failed/`

## File layout

```
~/.sipag/
  queue/                     # pending items (for sipag start)
    001-password-reset.md
  running/                   # currently executing (tracking files + logs)
    20240101120000-fix-bug.md
    20240101120000-fix-bug.log
  done/                      # completed
    20240101120000-fix-bug.md
    20240101120000-fix-bug.log
  failed/                    # needs attention
  repos.conf                 # registered repos (name → URL)
```

Tracking files use YAML frontmatter:

```yaml
---
repo: https://github.com/org/repo
issue: 21
started: 2024-01-01T12:00:00Z
container: sipag-20240101120000-fix-bug
ended: 2024-01-01T13:15:00Z
---
Simplify sipag to sandbox launcher
```

## Configuration

| Variable | Default | Purpose |
|---|---|---|
| `SIPAG_DIR` | `~/.sipag` | Data directory |
| `SIPAG_IMAGE` | `sipag-worker:latest` | Docker base image |
| `SIPAG_TIMEOUT` | `1800` | Per-container timeout (seconds) |
| `SIPAG_MODEL` | _(claude default)_ | Model override |
| `ANTHROPIC_API_KEY` | _(required)_ | Passed into container |
| `GH_TOKEN` | _(required)_ | Passed into container |

## Customizing behavior with CLAUDE.md

Repos can control how Claude behaves inside the sandbox by adding a `CLAUDE.md` file. Claude Code reads it automatically when it starts in the repo directory.

Common uses:

- **Coding conventions** — preferred style, naming rules, patterns to avoid
- **Test commands** — how to run tests for this repo (e.g. `make test`, `pytest`, `npm test`)
- **Architecture notes** — module layout, important constraints, areas to avoid touching
- **Commit message format** — conventional commits, ticket prefixes, etc.

**Where to put it:**

```
CLAUDE.md            # repo root (most common)
.claude/CLAUDE.md    # alternative location Claude also reads
```

sipag's executor prompt explicitly instructs Claude to read and follow `CLAUDE.md` before writing any code. No configuration is needed — just add the file to your repo.

## Project structure

```
sipag-core/    # Library: task parsing, repo registry, Docker executor
sipag/         # Binary: CLI (clap) + TUI (ratatui)
extras/        # safety-gate.sh: PreToolUse hook for Claude Code
```

## Development

```bash
# Requirements: Rust toolchain (rustup)
cargo build          # debug build
make build           # release build
make test            # cargo test
make lint            # cargo clippy -D warnings
make fmt             # cargo fmt
make dev             # lint + fmt-check + test
```

## Part of the dorky robot stack

```
kubo (think)  →  sipag (do)  →  GitHub PRs (review)
                    ↑
tao (decide)  ─────┘
```

- [kubo](https://github.com/Dorky-Robot/kubo) — chain-of-thought reasoning, breaks problems into steps
- [tao](https://github.com/Dorky-Robot/tao) — decision ledger, surfaces suspended actions
- **sipag** — autonomous executor, turns backlog into PRs

## Status

sipag v2 is in active development. See [VISION.md](VISION.md) for the full product vision.
