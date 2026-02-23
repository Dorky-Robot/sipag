# CLAUDE.md — sipag

This file primes Claude Code sessions working **on sipag itself**.

## Project overview

sipag is a sandbox launcher for Claude Code. It spins up isolated Docker containers, runs `claude --dangerously-skip-permissions` inside them, and implements GitHub PRs. sipag is pure infrastructure — containers, state files, lifecycle tracking. Claude Code is intelligence — analysis, implementation, review.

The three-phase flow:
1. **Analysis** (main Claude Code) — Cluster issues, craft a refined PR with architectural context
2. **Implementation** (Docker worker) — Read PR description as assignment, implement, test, push
3. **Review** (main Claude Code) — Review PR diff, merge or close, loop

## Architecture

### Rust workspace (3 crates)

```
sipag-core/src/
├── lib.rs              # pub mod: auth, config, docker, init, state, worker
├── auth.rs             # Token resolution (OAuth, API key, GH token)
├── config.rs           # WorkerConfig (5 fields), Credentials, default_sipag_dir()
├── docker.rs           # Preflight checks (daemon running, image available)
├── init.rs             # Create ~/.sipag/{workers,logs}
├── state.rs            # WorkerState, WorkerPhase, PR-keyed JSON state files
└── worker/
    ├── mod.rs           # pub use dispatch, github, lifecycle
    ├── dispatch.rs      # dispatch_worker() → Docker container
    ├── github.rs        # list_labeled_issues, count_open_sipag_prs, label_issues
    └── lifecycle.rs     # scan_workers, check_container_alive, cleanup_finished

sipag/src/
├── main.rs             # Entry point
└── cli.rs              # 7 commands: dispatch, ps, logs, kill, tui, doctor, version

tui/src/
├── main.rs             # Terminal setup, event loop, attach
├── app.rs              # App state, key handling, task refresh
├── task.rs             # Task struct (PR-keyed, built from WorkerState)
└── ui/                 # list.rs (table view), detail.rs (metadata + log)
```

### Container scripts

```
lib/container/worker.sh      # Worker entrypoint (embedded via include_str!)
lib/container/sipag-state.sh  # Atomic JSON state updates (copied into image)
lib/prompts/worker.md         # Worker disposition prompt (embedded via include_str!)
```

### Worker prompt

The PR description is the complete assignment. `worker.sh` reads it via `gh pr view`, appends the disposition from `worker.md`, and passes everything to `claude --dangerously-skip-permissions -p`.

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

Environment overrides: `SIPAG_IMAGE`, `SIPAG_TIMEOUT`, `SIPAG_WORK_LABEL`, `SIPAG_MAX_OPEN_PRS`, `SIPAG_DIR`.

## File layout (~/.sipag/)

```
workers/     # PR-keyed state JSON files
logs/        # Worker log files ({owner}--{repo}--pr-{N}.log)
config       # optional config file
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
