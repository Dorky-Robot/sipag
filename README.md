# sipag

<div align="center">

<img src="sipag.jpg" alt="sipag" width="300">

*Conversational agile for Claude Code. You talk; workers ship PRs.*

</div>

## What is sipag?

sipag turns your Claude Code session into a product team. You open a conversation, type `sipag start <repo>`, and Claude reads your GitHub board — issues, PRs, labels — and starts working with you on priorities. When work is approved, Claude spins up Docker workers that build PRs autonomously. You make the calls; Claude and the workers do the work.

The two commands you type as a human:

- **`sipag start <repo>`** — begin a sipag session inside Claude Code
- **`sipag merge <repo>`** — begin a merge review conversation

Everything else (`sipag work`, `sipag run`, `sipag next`) is Claude's domain.

## Quick start

1. Install sipag and run the setup wizard:

   ```bash
   sipag setup
   ```

2. Start a Claude Code session and kick off sipag:

   ```bash
   claude
   ```

   Then inside Claude Code, type:

   ```
   sipag start Dorky-Robot/my-project
   ```

3. Have a conversation. Claude reads the board, asks product questions, and handles triaging issues, refining specs, spinning up workers, and reviewing PRs. You make the decisions, Claude does the work.

4. When PRs are ready to merge:

   ```
   sipag merge Dorky-Robot/my-project
   ```

   Claude walks through open PRs with you, you decide what ships.

## How it works

```
you type: sipag start <repo>
          ↓
Claude reads GitHub board (issues, PRs, labels)
          ↓
conversation: priorities, triage, refinement
          ↓
Claude creates/approves issues via gh
          ↓
Claude runs: sipag work <repo>  (in background)
          ↓
Docker workers → clone → claude --dangerously-skip-permissions → PR
          ↓
you type: sipag merge <repo>
          ↓
conversation: review, decide, merge
```

### What `sipag start` does

`sipag start <repo>` dumps the current GitHub board state to stdout — open issues, open PRs, labels, recent activity. This primes Claude Code with full context so it can immediately engage on product questions: what's approved, what needs refinement, what's blocked, what should ship next.

Claude then adapts the conversation to what the board needs. If there's a backlog of approved issues, it starts workers. If issues need specs, it asks you product questions and opens refined issues. If PRs are stacking up, it suggests a merge session.

### What workers do

When Claude decides work is ready, it runs `sipag work <repo>` in the background. Workers:

1. Poll GitHub for issues labeled `approved`
2. Spin up an isolated Docker container per issue
3. Clone the repo, inject credentials
4. Run `claude --dangerously-skip-permissions` — Claude plans, codes, tests, commits, pushes, opens a PR
5. Label the issue `in-progress`; on failure return it to `approved`

The container is the safety boundary. Workers have full autonomy inside it.

## Lifecycle hooks

sipag emits events at key worker milestones via hook scripts — the same pattern as git hooks. External tools subscribe by dropping executable scripts into `~/.sipag/hooks/`.

sipag itself has no notification logic. It emits events; you decide what to do with them.

```
sipag (orchestrate) → lifecycle hook → tao (email)
sipag (orchestrate) → lifecycle hook → slack-notify (Slack)
sipag (orchestrate) → lifecycle hook → osascript (desktop)
sipag (orchestrate) → lifecycle hook → your-custom-thing
```

### Hook scripts

Place executable files in `~/.sipag/hooks/` named after the event:

| Hook | When it fires |
|---|---|
| `on-worker-started` | Worker picked up an issue |
| `on-worker-completed` | Worker finished, PR opened |
| `on-worker-failed` | Worker exited non-zero |
| `on-pr-iteration-started` | Worker iterating on PR feedback |
| `on-pr-iteration-done` | PR iteration complete |

Hooks run asynchronously — they never block the worker. Missing or non-executable hooks are silently skipped.

### Event data

Passed as environment variables:

**on-worker-started**
```
SIPAG_EVENT=worker.started
SIPAG_REPO=Dorky-Robot/sipag
SIPAG_ISSUE=42
SIPAG_ISSUE_TITLE="Fix auth middleware"
SIPAG_TASK_ID=20260220-fix-auth-middleware
```

**on-worker-completed**
```
SIPAG_EVENT=worker.completed
SIPAG_REPO=Dorky-Robot/sipag
SIPAG_ISSUE=42
SIPAG_ISSUE_TITLE="Fix auth middleware"
SIPAG_PR_NUM=47
SIPAG_PR_URL=https://github.com/Dorky-Robot/sipag/pull/47
SIPAG_DURATION=503
SIPAG_TASK_ID=20260220-fix-auth-middleware
```

**on-worker-failed**
```
SIPAG_EVENT=worker.failed
SIPAG_REPO=Dorky-Robot/sipag
SIPAG_ISSUE=42
SIPAG_ISSUE_TITLE="Fix auth middleware"
SIPAG_EXIT_CODE=1
SIPAG_LOG_PATH=/tmp/sipag-backlog/issue-42.log
SIPAG_TASK_ID=20260220-fix-auth-middleware
```

### Example hooks

**Desktop notification (macOS)**
```bash
#!/usr/bin/env bash
# ~/.sipag/hooks/on-worker-completed
osascript -e "display notification \"PR opened for #${SIPAG_ISSUE}\" with title \"sipag\""
```

**Log to file**
```bash
#!/usr/bin/env bash
# ~/.sipag/hooks/on-worker-completed
echo "$(date) ${SIPAG_EVENT} ${SIPAG_REPO}#${SIPAG_ISSUE} ${SIPAG_PR_URL}" >> ~/.sipag/events.log
```

**Email via tao**
```bash
#!/usr/bin/env bash
# ~/.sipag/hooks/on-worker-completed
echo "PR ${SIPAG_PR_URL} opened for #${SIPAG_ISSUE} in ${SIPAG_REPO}" \
    | tao notify developer felix@example.com --detach
```

`sipag setup` creates `~/.sipag/hooks/` for you.

## Installation

### Homebrew (macOS and Linux — recommended)

```bash
brew tap Dorky-Robot/sipag
brew install sipag
```

This installs the pre-built binary — no Rust toolchain required.

To also use the bash helper commands (`sipag start`, `sipag work`, `sipag merge`, `sipag setup`), add to your shell profile:

```bash
export PATH="$(brew --prefix sipag)/libexec/bin:$PATH"
```

### One-line install (macOS and Linux)

```bash
curl -fsSL https://raw.githubusercontent.com/Dorky-Robot/sipag/main/scripts/install.sh | bash
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

## TUI

Running `sipag` with no arguments (or `sipag tui`) opens the interactive terminal UI to observe worker activity:

- Scrollable task list across all states (queue, running, done, failed)
- Color-coded by status: yellow=queued, cyan=running, green=done, red=failed
- Keyboard navigation: `↑`/`k` up, `↓`/`j` down, `r` refresh, `q`/`Esc` quit

## File layout

```
~/.sipag/
  queue/                     # pending items
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

### Global configuration

| Variable | Default | Purpose |
|---|---|---|
| `SIPAG_DIR` | `~/.sipag` | Data directory |
| `SIPAG_IMAGE` | `ghcr.io/dorky-robot/sipag-worker:latest` | Docker base image |
| `SIPAG_TIMEOUT` | `1800` | Per-container timeout (seconds) |
| `SIPAG_MODEL` | _(claude default)_ | Model override |
| `ANTHROPIC_API_KEY` | _(required)_ | Passed into container |
| `GH_TOKEN` | _(required)_ | Passed into container |

Global defaults can also be set in `~/.sipag/config` (key=value format):

```
batch_size=4
image=ghcr.io/dorky-robot/sipag-worker:latest
timeout=1800
poll_interval=120
work_label=approved
in_progress_label=in-progress
```

### Per-repo configuration (`.sipag.toml`)

Repos can override worker settings by placing a `.sipag.toml` file in their root:

```toml
[worker]
image = "ghcr.io/org/custom-worker:latest"  # custom Docker image for this repo
timeout = 3600                               # seconds (default: 1800)
model = "claude-sonnet-4-6"                  # model override for this repo
batch_size = 2                               # parallel workers for this repo

[labels]
work = "ready"          # label that triggers workers (default: "approved")
in_progress = "wip"     # label applied when worker picks up issue (default: "in-progress")

[prompts]
# Additional instructions appended to every worker prompt for this repo
extra = """
Always run the full test suite before opening a PR.
Use conventional commits.
"""
```

**Resolution order** (most specific wins):

1. `.sipag.toml` in repo root — per-repo, highest priority
2. `~/.sipag/config` — global defaults
3. `SIPAG_*` environment variables
4. Built-in defaults — lowest priority

Workers fetch `.sipag.toml` from the GitHub API before spinning up containers. The file is optional — if absent, global and environment settings apply unchanged.

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

---

## Appendix: Full CLI reference

Most of these commands are invoked by Claude, not the human directly.

### Human-facing

```
sipag start <owner/repo>      Prime a Claude Code session with board state
sipag merge <owner/repo>      Start a merge review conversation
sipag setup                   Run the interactive setup wizard
sipag tui                     Launch interactive TUI (same as no args)
sipag version                 Print version
sipag help                    Show help
```

### Worker commands (Claude's domain)

```
sipag work <owner/repo>       Poll GitHub for approved issues, spin up Docker workers
sipag run --repo <url> [--issue <n>] [-b] "<task>"
                              Launch a Docker sandbox for a task
sipag ps                      List running and recent tasks with status
sipag logs <id>               Print the log for a task
sipag kill <id>               Kill a running container, move task to failed/
```

### Queue commands (Claude's domain)

```
sipag add <title> --repo <name> [--priority <level>]
                              Add a task to queue/
sipag status                  Show queue state across all directories
sipag show <name>             Print task file and log
sipag retry <name>            Re-queue a failed task
sipag repo add <name> <url>   Register a repo name → URL mapping
sipag repo list               List registered repos
sipag init                    Create ~/.sipag/{queue,running,done,failed}
```

### Legacy checklist commands

```
sipag next [-c] [-n] [-f]     Run claude on the next pending checklist task
sipag list [-f <file>]        List tasks from a markdown checklist file
sipag add "<title>"           Append task to ./tasks.md (no --repo)
```
