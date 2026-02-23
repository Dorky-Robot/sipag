# CLAUDE.md — sipag

This file primes Claude Code sessions working **on sipag itself**.

## Project overview

sipag is a sandbox launcher for Claude Code. It spins up isolated Docker containers, runs `claude --dangerously-skip-permissions` inside them, and implements GitHub PRs. sipag is pure infrastructure — containers, state files, lifecycle tracking. Claude Code is intelligence — analysis, implementation, review.

The three-phase flow:
1. **Analysis** (main Claude Code) — Cluster issues, craft a refined PR with architectural context
2. **Implementation** (Docker worker) — Read PR description as assignment, implement, test, push
3. **Review** (main Claude Code) — Review PR diff, merge or close, loop

## Architecture

### Rust workspace (4 crates)

```
sipag-core/src/
├── lib.rs              # pub mod: auth, config, docker, init, repo, state, worker
├── auth.rs             # Token resolution (OAuth, API key, GH token)
├── config.rs           # WorkerConfig (7 fields), Credentials, default_sipag_dir()
├── docker.rs           # Preflight checks (daemon running, image available)
├── init.rs             # Create ~/.sipag/{workers,logs}
├── repo.rs             # Git remote resolution (local dir → GitHub owner/repo)
├── state.rs            # WorkerState, WorkerPhase, PR-keyed JSON state files (atomic writes)
└── worker/
    ├── mod.rs           # pub use dispatch, github, lifecycle
    ├── dispatch.rs      # dispatch_worker() → Docker container
    ├── github.rs        # list_labeled_issues, count_open_sipag_prs, fetch_open_issues/prs
    └── lifecycle.rs     # scan_workers (heartbeat-based liveness), cleanup_finished

sipag/src/
├── main.rs             # Entry point
├── cli.rs              # 8 commands: work, dispatch, ps, logs, kill, tui, doctor, version
└── work.rs             # sipag work: resolve repos, fetch board state, exec claude

sipag-worker/src/
└── main.rs             # Container-side binary: clone, fetch PR, run Claude Code

tui/src/
├── main.rs             # Terminal setup, event loop, attach
├── app.rs              # App state, key handling, task refresh
├── task.rs             # Task struct (PR-keyed, built from WorkerState)
└── ui/                 # list.rs (table view), detail.rs (metadata + log)
```

### Prompts

```
lib/prompts/work.md           # sipag work system prompt (embedded via include_str!)
lib/prompts/worker.md         # Worker disposition prompt (embedded via include_str!)
```

The PR description is the complete assignment. `sipag-worker` reads it via `gh pr view`, appends the disposition from `worker.md`, and passes everything to `claude --dangerously-skip-permissions -p`. Both host and container use `sipag-core::state` for state I/O, ensuring field names are always consistent.

### State model

All state is PR-keyed JSON at `~/.sipag/workers/{owner}--{repo}--pr-{N}.json`:

```json
{
  "repo": "owner/repo",
  "pr_num": 42,
  "issues": [10, 11],
  "branch": "sipag/pr-42",
  "container_id": "abc123",
  "phase": "working",
  "heartbeat": "2026-01-15T10:30:00Z",
  "started": "2026-01-15T10:30:00Z"
}
```

Phases: `starting` → `working` → `finished` | `failed`

## Commands

```
sipag work [<dirs>...]        Start an interactive work session (main entry point)
sipag dispatch --repo <owner/repo> --pr <N>
                              Launch a Docker worker for a PR
sipag ps                      List active and recent workers
sipag logs <id>               Show logs for a worker (PR number or container name)
sipag kill <id>               Kill a running worker
sipag tui                     Launch interactive TUI (also: run sipag with no args)
sipag doctor                  Check system prerequisites
sipag version                 Print version
```

## Config

`~/.sipag/config` (optional, key=value):

| Key | Default | Description |
|-----|---------|-------------|
| `image` | `ghcr.io/dorky-robot/sipag-worker:latest` | Docker image |
| `timeout` | `7200` | Worker timeout in seconds |
| `work_label` | `ready` | Issue label gate |
| `max_open_prs` | `3` | Back-pressure limit |
| `poll_interval` | `120` | Seconds between polling cycles |
| `heartbeat_interval` | `30` | Seconds between heartbeat writes |
| `heartbeat_stale` | `90` | Seconds before a heartbeat is considered stale |

Environment overrides: `SIPAG_IMAGE`, `SIPAG_TIMEOUT`, `SIPAG_WORK_LABEL`, `SIPAG_MAX_OPEN_PRS`, `SIPAG_DIR`, `SIPAG_HEARTBEAT_INTERVAL`, `SIPAG_HEARTBEAT_STALE`.

## File layout (~/.sipag/)

```
workers/     # PR-keyed state JSON + heartbeat files
events/      # Append-only lifecycle events (the event bus)
logs/        # Worker stdout/stderr ({owner}--{repo}--pr-{N}.log)
lessons/     # Per-repo learning from failures ({owner}--{repo}.md)
config       # Optional config file
```

## Conventions

### Rust code

- `make dev` — full local validation: `cargo fmt` + `cargo clippy -D warnings` + `cargo test`
- `make build` — release build
- `make install` — `cargo install --path sipag`

### Quality gates — git hooks

Hooks are the sole quality gate. Code that gets pushed is already validated.

**Pre-commit** (~1 min): gitleaks secrets scan, typos spell check, cargo deny CVE
check, **cargo build --release**, cargo fmt, cargo clippy, shellcheck.

**Pre-push** (~2-3 min): cargo test --workspace (blocking), cargo machete (warning),
gitleaks final scan (blocking).

Install once after cloning:

```bash
make install-hooks
```

- **Never use `--no-verify`**. Fix the issue instead.
- Run `make dev` before opening or updating PRs.

### Docker image

Worker containers use `ghcr.io/dorky-robot/sipag-worker:latest`, published via GitHub Actions.

```bash
docker build -t sipag-worker:local .
SIPAG_IMAGE=sipag-worker:local sipag dispatch --repo <owner/repo> --pr <N>
```

## Running sipag work

**Never `unset CLAUDECODE`** to try to run `sipag work` from inside a Claude Code session. It causes nested session conflicts. Instead, ask the user to run it from a separate terminal, or have the main Claude Code session run it as a background Bash task:

```bash
SIPAG_IMAGE=sipag-worker:local sipag work ~/Projects/dorky_robot/katulong
```

`sipag work` launches its own Claude Code session — it cannot be nested inside another one.

## Working on sipag

### What changes most

- `sipag-core/src/worker/` — dispatch, lifecycle, GitHub operations
- `sipag-core/src/state.rs` — state file format and management
- `sipag/src/cli.rs` — CLI commands
- `tui/src/` — TUI views and task model

### PR-only workflow

The host machine is for **conversation and commands only**. All code changes happen through PRs built inside Docker workers.

1. Identify the need in conversation
2. Create or update a GitHub issue
3. Label the issue `ready`
4. Main Claude Code crafts a PR with architectural context
5. `sipag dispatch` launches a Docker worker
6. Review the PR, merge or close

### Part of the dorky robot stack

```
kubo (think)  →  sipag (do)  →  GitHub PRs (review)
                    ↑
tao (decide)  ─────┘
```

sipag is the execution layer. kubo handles chain-of-thought planning; tao surfaces suspended decisions.
