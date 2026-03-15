# sipag

<div align="center">

<img src="sipag.jpg" alt="sipag" width="300">

*Autonomous dev agents that evolve with your project.*

</div>

## What is sipag?

sipag generates project-aware review agents, ships work through isolated Docker containers, and learns from failures — all powered by Claude Code.

1. **`sipag configure`** — Analyzes your project and generates tailored review agents and commands for `.claude/`. Re-run as your project evolves — it reads what's there and updates it.
2. **`sipag dispatch`** — Launches an isolated Docker container that reads a PR description and implements it autonomously.
3. **`sipag tui`** — Live dashboard for all workers across the host.

## Quick start

1. Install sipag:

   ```bash
   brew tap Dorky-Robot/sipag
   brew install sipag
   ```

2. Configure review agents and commands for your project:

   ```bash
   cd ~/Projects/my-app
   sipag configure
   ```

3. Create a branch and PR on GitHub describing what needs to happen.

4. Dispatch a Docker worker to implement the PR:

   ```bash
   sipag dispatch https://github.com/owner/my-app/pull/42
   ```

5. Monitor workers:

   ```bash
   sipag tui
   ```

## How it works

```
sipag configure               Configure agents + commands for .claude/
          ↓
create branch + PR            Describe the work in the PR body
          ↓
sipag dispatch <PR_URL>       Launch a Docker worker
          ↓
Docker container              clone → read PR body → claude → push → done
          ↓
sipag tui / sipag ps          Monitor progress
          ↓
review + merge                You decide what ships
```

### sipag configure

Generates project-specific review agents and commands into `.claude/`. By default it launches Claude to analyze your project and write tailored agents. Use `--static` to install generic templates without Claude.

| Category | Files |
|----------|-------|
| Agents | `security-reviewer`, `architecture-reviewer`, `correctness-reviewer`, `root-cause-analyst`, `simplicity-advocate`, `backlog-triager`, `issue-analyst` |
| Commands | `dispatch`, `review`, `triage`, `ship-it`, `work`, `consult`, `release` |

Re-run `sipag configure` as your project evolves — it reads existing files and updates them.

### sipag dispatch

Launches an isolated Docker container that:

1. Clones the repo and checks out the PR branch
2. Reads the PR body as its complete assignment
3. Reads lessons from past failures for this repo
4. Runs `claude --dangerously-skip-permissions` to implement the work
5. Pushes commits to the PR branch
6. Writes state and lifecycle events to `~/.sipag/`

The container is the safety boundary. Workers have full autonomy inside it.

### sipag tui

Running `sipag` with no arguments (or `sipag tui`) opens the interactive terminal UI:

- Scrollable task list across all states (starting, working, finished, failed)
- Color-coded by status: yellow=starting, cyan=working, green=finished, red=failed
- Keyboard navigation: `↑`/`k` up, `↓`/`j` down, `Enter` detail view, `a` attach, `q` quit

## Installation

### Homebrew (macOS and Linux — recommended)

```bash
brew tap Dorky-Robot/sipag
brew install sipag
```

This installs the pre-built binary — no Rust toolchain required.

### One-line install (macOS and Linux)

```bash
curl -fsSL https://raw.githubusercontent.com/Dorky-Robot/sipag/main/scripts/install.sh | sh
```

Supports macOS (Intel and Apple Silicon) and Linux (x86\_64 and ARM64). Installs the binary to `/usr/local/bin/sipag`.

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

## Configuration

Create `~/.sipag/config` to override defaults (key=value format):

| Key | Default | Description |
|-----|---------|-------------|
| `image` | `ghcr.io/dorky-robot/sipag-worker:latest` | Docker image for workers |
| `timeout` | `7200` | Worker timeout in seconds (2 hours) |
| `work_label` | `ready` | Issue label that marks work ready for dispatch |
| `max_open_prs` | `3` | Max active workers before dispatch is paused |
| `poll_interval` | `120` | Seconds between polling cycles |
| `heartbeat_interval` | `30` | Seconds between heartbeat writes |
| `heartbeat_stale` | `90` | Seconds before a heartbeat is considered stale |

Environment variable overrides: `SIPAG_IMAGE`, `SIPAG_TIMEOUT`, `SIPAG_WORK_LABEL`, `SIPAG_MAX_OPEN_PRS`, `SIPAG_DIR`, `SIPAG_HEARTBEAT_INTERVAL`, `SIPAG_HEARTBEAT_STALE`.

## File layout

```
~/.sipag/
├── config          # Optional key=value config
├── workers/        # PR-keyed state JSON files + heartbeat files
├── events/         # Append-only lifecycle events
├── logs/           # Worker stdout/stderr
└── lessons/        # Per-repo learning from failures
```

## CLI reference

```
sipag configure [dir] [--static]        Configure agents and commands for .claude/
sipag dispatch <PR_URL>                 Launch a Docker worker for a PR
sipag ps [--all]                        List active and recent workers
sipag logs <id>                         Show logs for a worker (PR number or container name)
sipag kill <id>                         Kill a running worker
sipag tui                               Launch interactive TUI (same as no args)
sipag doctor                            Check system prerequisites
sipag version                           Print version
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

## Documentation

Full documentation at [sipag.dev](https://sipag.dev).
