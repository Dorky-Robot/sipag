# CLAUDE.md — sipag

This file primes Claude Code sessions working **on sipag itself**.
For guidance on adding CLAUDE.md to repos that sipag manages, see [Customizing behavior with CLAUDE.md](#customizing-behavior-with-claudemd) in README.md.

## Project overview

sipag is a sandbox launcher for Claude Code. It spins up isolated Docker containers, runs `claude --dangerously-skip-permissions` inside them, and turns GitHub issues into pull requests. Claude is the orchestrator — it decides what to do and how; sipag provides the infrastructure (containers, credentials, lifecycle tracking) and makes progress visible.

Architecture has two layers:

- **Rust CLI** (`sipag-cli/`, `sipag-core/`, `tui/`) — the primary binary, handles sandbox management, task queue, and TUI
- **Bash scripts** (`bin/sipag`, `lib/`) — the worker polling loop and checklist helpers (the part that runs *inside* containers or in CI-style batch flows)

## Architecture

### Rust (primary CLI and core library)

```
sipag-core/    # Library: task parsing, repo registry, Docker executor
sipag-cli/     # Binary: CLI (clap) + dispatches to sipag-core
tui/           # Binary: ratatui TUI, exec'd by `sipag tui`
```

Key modules in `sipag-core/`:
- `executor` — Docker container lifecycle (run, ps, kill, logs, status, retry)
- `task` — task file format (YAML frontmatter + description), slugify, queue filenames
- `repo` — `~/.sipag/repos.conf` registry (name → URL)
- `config` — environment-based config (`SIPAG_DIR`, `SIPAG_IMAGE`, etc.)
- `init` — creates `~/.sipag/{queue,running,done,failed}`

### Bash scripts

```
bin/sipag          # Bash CLI entry point: work, next, list, add commands
lib/run.sh         # claude invocation helper (respects SIPAG_* env vars)
lib/task.sh        # Markdown checklist parser: parse_next, mark_done, list, add
lib/worker.sh      # Docker worker polling loop for `sipag work`
lib/notify.sh      # Notification helpers
```

### Prompt injected into each worker container

`lib/worker.sh:worker_run_issue()` builds the prompt that every container receives. It includes:
- The GitHub issue title and body
- Standard instructions: branch, implement, test, commit, draft PR, mark ready

## Commands

### Rust CLI (installed binary)

```
sipag run --repo <url> [--issue <n>] [-b] "<task>"
                              Launch a Docker sandbox for a task
sipag ps                      List running and recent tasks with status
sipag logs <id>               Print the log for a task
sipag kill <id>               Kill a running container, move task to failed/
sipag status                  Show queue state across all directories
sipag show <name>             Print task file and log
sipag retry <name>            Re-queue a failed task
sipag add "<title>" [--repo <name>] [--priority <level>]
                              Add a task to queue/ or to tasks.md
sipag repo add <name> <url>   Register a repo name → URL mapping
sipag repo list               List registered repos
sipag init                    Create ~/.sipag/{queue,running,done,failed}
sipag start                   Process queue/ serially using Docker
sipag tui                     Launch interactive TUI (also: run sipag with no args)
sipag version                 Print version
```

### Bash CLI (bin/sipag — used inside containers and for issue polling)

```
sipag work <owner/repo>      Poll GitHub for approved issues, spin up Docker workers
sipag next [-c] [-n] [-f]    Run next task from a markdown checklist
sipag list [-f path]          Print all tasks with status
sipag add "task" [-f path]    Append task to checklist
```

## Conventions

### Rust code

- `make dev` — full local validation: `cargo fmt` + `cargo clippy -D warnings` + `cargo test`
- `make build` — release build
- `make lint` — clippy with `-D warnings`
- `make fmt` — format in place
- `make fmt-check` — CI-safe format check (no writes)
- `make install` — `cargo install --path sipag`

### Bash scripts

- All scripts use `set -euo pipefail`
- shellcheck-clean (run `shellcheck bin/sipag lib/*.sh` before committing)
- Tests live in `test/unit/` and `test/integration/` (BATS-style)

### Worker label gate

Workers only pick up issues labeled **`approved`** (configurable via `SIPAG_WORK_LABEL` env var or `work_label=` in `~/.sipag/config`). Issues in flight get the `in-progress` label; on failure they return to `approved`.

Priority labels: P0–P3 (convention, not enforced by sipag).

### File layout (~/.sipag/)

```
queue/       # pending tasks (YAML frontmatter + description)
running/     # active containers (tracking file + .log)
done/        # completed
failed/      # needs attention — use `sipag retry` to re-queue
repos.conf   # name → URL registry
config       # optional: batch_size, image, timeout, poll_interval, work_label
seen         # worker dedup list (issue numbers already dispatched)
token        # Claude OAuth token for worker containers
```

## PR-only workflow

The host machine running sipag is for **conversation and commands only**. All code
changes must happen through PRs built inside Docker workers. This is a hard rule.

### What the host does
- Runs `sipag start` to prime a Claude Code session
- Runs `sipag work <repo>` to dispatch Docker workers
- Manages issues and PRs via `gh` commands
- Reviews and merges PRs via `gh pr review` / `gh pr merge`
- That's it

### What the host must NEVER do
- Edit files locally (`nano`, `vim`, `sed`, `Edit`/`Write` tools, etc.)
- Commit or push to main directly (`git commit`, `git push`)
- Apply patches or run `git apply`

### How changes get made
1. Identify the need in conversation
2. Create or update a GitHub issue describing the change
3. Label the issue `approved`
4. `sipag work` dispatches a Docker worker that implements and opens a PR
5. Review the PR via `gh pr diff` / `gh pr review`
6. Merge via `gh pr merge`

This applies to all repos sipag manages, including sipag itself.

## Working on sipag

### Use sipag to manage its own backlog

sipag manages its own development — Claude Code sessions for sipag issues are dispatched via `sipag work Dorky-Robot/sipag`. So when working on sipag: label an issue `approved` and the worker will pick it up.

For interactive sessions: open a terminal, run `sipag`, and use the TUI to inspect queue state.

### What changes most

- `lib/worker.sh` — the worker loop and per-issue prompt construction
- `sipag-core/src/executor.rs` — Docker executor logic
- `sipag-core/src/task.rs` — task file format

### Docker image

Worker containers use `ghcr.io/dorky-robot/sipag-worker:latest`, published automatically to GHCR via the `Publish Worker Image` GitHub Actions workflow on every release and on Dockerfile/lib changes to main.

To test with a locally built image:

```bash
docker build -t sipag-worker:local .
SIPAG_IMAGE=sipag-worker:local sipag work <owner/repo>
```

### Running tests

```bash
make dev       # fmt + lint + test (recommended before pushing)
make test      # cargo test only
```

The pre-push hook runs the full test suite automatically (installed via `make install-hooks`).

### Quality gates — git hooks

Hooks are the sole quality gate. Code that gets pushed is already validated.

**Pre-commit** (fast, ~15s): gitleaks secrets scan, typos spell check, cargo deny CVE
check, cargo fmt, cargo clippy, shellcheck.

**Pre-push** (~2-3 min): cargo test --workspace (blocking), cargo machete (warning),
gitleaks final scan (blocking).

Install once after cloning:

```bash
make install-hooks
```

#### Rules

- **Never use `--no-verify`**. Fix the issue instead.
- Run `make dev` (fmt + clippy + test) before opening or updating PRs.
- Tests must pass before push.

### Part of the dorky robot stack

```
kubo (think)  →  sipag (do)  →  GitHub PRs (review)
                    ↑
tao (decide)  ─────┘
```

sipag is the execution layer. kubo handles chain-of-thought planning; tao surfaces suspended decisions.
