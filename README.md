# sipag

<div align="center">

<img src="sipag.jpg" alt="sipag" width="300">

*Spin up isolated Docker sandboxes. Make progress visible.*

</div>

## What is sipag?

sipag is a tool for **Claude Code**, not for humans to run directly. You talk to Claude; Claude runs sipag on your behalf.

Claude Code is the orchestrator — it decides what to work on, in what order, and handles retries. sipag does one thing well: spinning up isolated Docker sandboxes and making progress visible.

The human experience looks like this:

1. Open a terminal and run `claude`
2. Say "let's work on my project"
3. Claude runs `sipag start <repo>` to prime itself with the backlog
4. You have a product and architecture conversation
5. Claude creates issues, triages, refines, approves — all via `gh`
6. Claude kicks off workers in the background
7. You keep talking while Docker containers build your features
8. Claude reviews PRs, merges what's ready

**You never touch a single sipag command yourself.**

## Quick start

1. Install sipag and run the setup wizard:
   ```bash
   sipag setup
   ```

2. Start a Claude Code session:
   ```bash
   claude
   ```

3. Tell Claude to prime itself with your project:
   ```
   > sipag start Dorky-Robot/my-project
   ```

4. Have a conversation. Claude handles the rest — triaging issues,
   refining specs, spinning up workers, reviewing PRs, merging.

## How it works

```
claude → sipag run → Docker container → clone repo → claude -p → PR
```

1. Claude Code calls `sipag run` with a repo URL and task description
2. sipag generates a unique task ID, creates a tracking file in `running/`
3. Spins up a Docker container: clones the repo, injects credentials
4. Runs `claude --dangerously-skip-permissions` inside — Claude plans, codes, tests, commits, pushes, opens a PR
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

## TUI

Running `sipag` with no arguments (or `sipag tui`) opens the interactive terminal UI. This is useful for watching what Claude is doing:

- Scrollable task list across all states (queue, running, done, failed)
- Color-coded by status: yellow=queued, cyan=running, green=done, red=failed
- Keyboard navigation: `↑`/`k` up, `↓`/`j` down, `r` refresh, `q`/`Esc` quit

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

### What to include in your CLAUDE.md

A good `CLAUDE.md` for a sipag-managed repo answers four questions for Claude before it reads a single line of code:

| Section | What to write |
|---|---|
| **Project** | One paragraph: what the repo does and who uses it |
| **Priorities** | What matters right now — stability, a specific feature area, a migration in progress |
| **Architecture** | Tech stack, key modules, patterns Claude must follow or avoid |
| **Testing** | Exact commands to run tests; what "passing" looks like |

Keep it short. Claude reads CLAUDE.md before writing code, so dense prose slows it down. Bullet points and short paragraphs work best.

### Example CLAUDE.md for a sipag-managed repo

```markdown
## Project
dorky_robot is a personal mesh network for self-hosted services.
Rust/Axum backend, HTMX frontend, SQLite database. Passkey auth, no passwords.

## Priorities
Stability > features. Hardening auth before adding new mesh capabilities.
Do not change the passkey flow without a green test suite first.

## Architecture
- src/auth/     — passkey registration and assertion
- src/api/      — Axum route handlers (thin — business logic lives in src/domain/)
- src/domain/   — pure Rust, no async, no framework dependencies
- migrations/   — SQLite migrations via sqlx (never edit existing migrations)

## Testing
cargo test                   # unit + integration
npx playwright test          # E2E (requires running dev server)
make ci                      # full suite used in CI

All tests must pass before opening a PR. If a test is flaky, note it in the PR body.
```

### Labels and conventions

sipag workers only pick up issues labeled **`approved`**. Add any project-specific label conventions to CLAUDE.md so Claude can apply them correctly when opening PRs:

```markdown
## Labels
- `approved` — ready for a sipag worker to implement
- `needs-spec` — issue needs more detail before approval
- `blocked` — waiting on external dependency
```

## Commands (for Claude)

These commands are intended to be run by Claude Code, not typed by humans directly. They're documented here so you know what Claude is doing on your behalf.

### Sandbox commands (primary interface)

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

### sipag run in detail

```bash
sipag run --repo <url> [--issue <n>] [-b|--background] "<task description>"
```

- `--repo <url>` — repository URL to clone inside the container (required)
- `--issue <n>` — GitHub issue number to associate with this task (optional)
- `-b`, `--background` — run in background; sipag returns immediately (default: foreground)

On launch, sipag:

1. Auto-inits `~/.sipag/` if needed
2. Generates a task ID: `YYYYMMDDHHMMSS-slug`
3. Prints the task ID so Claude can follow up with `sipag logs` or `sipag kill`
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
