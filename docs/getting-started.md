# Getting started with sipag

sipag is a sandbox launcher for Claude Code. You point it at your local project directories, and it launches a Claude session that reads your GitHub boards, identifies the deepest problems in your codebases, crafts refined PRs, and spins up Docker workers to implement them. You make the decisions; Claude and the workers do the work.

## Prerequisites

| Tool | Why | Install |
|------|-----|---------|
| Docker Desktop | Runs worker containers | [docker.com](https://www.docker.com/products/docker-desktop/) |
| GitHub CLI (`gh`) | API access for PRs and issues | `brew install gh` |
| Claude Code CLI | AI on the host and inside containers | `npm install -g @anthropic-ai/claude-code` |

## 1. Install sipag

### Homebrew (macOS)

```bash
brew tap Dorky-Robot/tap
brew install sipag
```

### From source

Requires the [Rust toolchain](https://rustup.rs/).

```bash
git clone https://github.com/Dorky-Robot/sipag.git
cd sipag
make install
```

### Verify

```bash
sipag version
```

## 2. Authenticate

### GitHub

```bash
gh auth login
```

sipag uses `gh auth token` at runtime. Alternatively, export `GH_TOKEN` directly.

### Claude

**Option A: OAuth (recommended)**

1. Run `claude` and complete the OAuth flow in your browser
2. Copy the token that gets printed to your console
3. Save it to `~/.sipag/token`:

```bash
echo 'YOUR_OAUTH_TOKEN' > ~/.sipag/token
chmod 600 ~/.sipag/token
```

**Option B: API key**

```bash
export ANTHROPIC_API_KEY=sk-ant-...
```

## 3. Pull the worker Docker image

```bash
docker pull ghcr.io/dorky-robot/sipag-worker:latest
```

Or build locally:

```bash
docker build -t sipag-worker:local .
export SIPAG_IMAGE=sipag-worker:local
```

## 4. Check your setup

```bash
sipag doctor
```

Fix anything marked FAIL or MISSING before proceeding.

## 5. Start working

Point sipag at one or more local project directories:

```bash
# Single project (current directory)
sipag work .

# Single project (explicit path)
sipag work ~/Projects/my-app

# Multi-project session
sipag work ~/Projects/my-app ~/Projects/my-api ~/Projects/my-frontend
```

sipag reads the git remote from each directory to resolve the GitHub repo (e.g. `origin` → `Dorky-Robot/my-app`). No need to type `owner/repo` — if it's a git repo with a GitHub remote, sipag knows what it is.

This single command:

1. Resolves the GitHub repo for each directory from its git remotes
2. Fetches each repo's board state — open issues, open PRs, labels, recent activity
3. Launches an interactive Claude session with the sipag workflow and board state injected as system context
4. Claude kicks off a background poller per repo that watches for `ready`-labeled issues

You're now in a conversation with Claude. The pollers run as background tasks — you can check on them anytime, but they don't block your session. You can talk to Claude about priorities, triage issues, or just watch it work. You make the product decisions; Claude handles execution.

## 6. The disease identification and eradication cycle

Claude drives a continuous cycle inside your session, per repo:

### Codebase understanding

Before looking at any issues, Claude reads the local codebase to build a mental model of the project. Since `sipag work` points at local directories, Claude has direct access to the source code:

- Reads `CLAUDE.md` for project context, priorities, architecture notes, and test commands
- Explores the directory structure, key modules, and dependency graph
- Identifies patterns, boundaries, and conventions already in use

This happens first because disease clustering is meaningless without understanding the patient. When Claude later sees three issues about "config crashes," it already knows the config parser is 400 lines of ad-hoc string matching — so it can identify the structural disease instead of treating each crash as an isolated symptom.

### Parallel deep analysis

With the codebase understood, Claude spins up **four parallel analysis agents** that examine the codebase simultaneously from different angles:

1. **Security reviewer** — OWASP top 10, secrets in code, auth/authz gaps, input validation, dependency CVEs
2. **Architecture reviewer** — module boundaries, coupling, abstraction leaks, separation of concerns
3. **Code quality reviewer** — dead code, duplication, error handling patterns, missing abstractions
4. **Testing reviewer** — coverage gaps, missing edge cases, integration test needs

Each agent identifies **diseases, not symptoms**. Three issues about different error messages probably mean there's no unified error handling. Five issues about Docker configuration probably mean the config boundary is wrong.

After all agents return, Claude synthesizes the findings — deduplicates across reviewers, ranks by impact, and creates GitHub issues labeled `ready` for the top findings. These issues contain full architectural briefs: disease name, affected files, target design, and constraints.

### Implementation

Claude dispatches a Docker worker in the background. The container spins up, starts cold, reads the PR description as its complete assignment, and:

- Clones the repo, checks out the PR branch
- Implements the fix, runs tests, commits, pushes
- Updates the PR body and issue labels as it works
- Reports state back to the host via a JSON state file

The container is the safety boundary — Claude runs `--dangerously-skip-permissions` inside it without risking your machine.

### Review and merge

When a worker finishes successfully, Claude auto-merges the PR via squash merge. If a worker fails, Claude writes an event file to `~/.sipag/events/` and moves on — external systems can observe that directory for notifications. The issues return to the backlog for a different approach next cycle.

The cycle repeats continuously. The backlog changes, the codebase gets healthier, and the next analysis starts from a different place because the project is different.

In a multi-project session, Claude manages the cycle independently per repo — workers for different repos can run in parallel since they don't conflict.

## 7. Monitoring workers

### TUI

Open a separate terminal and run `sipag` with no arguments (or `sipag tui`) for the interactive dashboard:

```bash
sipag
```

The TUI shows all workers across all repos in a live table. From here you can:

| Key | Action |
|-----|--------|
| `j` / `↓` | Move down |
| `k` / `↑` | Move up (list view) / scroll up (detail view) |
| `Enter` | Open detail view (metadata + log) |
| `Esc` | Back to list |
| `a` | Attach to a running container's shell |
| `k` | Kill the selected worker |
| `K` | Kill all active workers |
| `x` / `Delete` | Dismiss a finished/failed worker |
| `Tab` | Toggle between active and archive views |
| `q` | Quit |

### CLI

You can also manage workers from the command line:

```bash
sipag ps          # List workers and their status
sipag logs 42     # View output for PR #42
sipag kill 42     # Stop a worker
```

Phases: `starting` → `working` → `finished` | `failed`

## Configuration

Create `~/.sipag/config` to override defaults:

```
image=ghcr.io/dorky-robot/sipag-worker:latest
timeout=7200
work_label=ready
max_open_prs=3
poll_interval=120
```

| Key | Default | Description |
|-----|---------|-------------|
| `image` | `ghcr.io/dorky-robot/sipag-worker:latest` | Docker image for workers |
| `timeout` | `7200` | Worker timeout in seconds (2 hours) |
| `work_label` | `ready` | Issue label that marks tasks ready for dispatch |
| `max_open_prs` | `3` | Max open sipag PRs before dispatch is paused (0 = no limit) |
| `poll_interval` | `120` | Seconds between polling cycles |

Environment variables override config file values: `SIPAG_IMAGE`, `SIPAG_TIMEOUT`, `SIPAG_WORK_LABEL`, `SIPAG_MAX_OPEN_PRS`, `SIPAG_POLL_INTERVAL`, `SIPAG_DIR`.

## File layout

```
~/.sipag/
├── config          # Optional key=value config
├── token           # Claude OAuth token (chmod 600)
├── workers/        # PR-keyed state JSON files
├── logs/           # Worker log files
└── events/         # Lifecycle event files (worker failures, escalations)
```

## Quick reference

```bash
sipag work .                              # Work on the current directory
sipag work ~/proj/a ~/proj/b              # Multi-project session
sipag doctor                              # Check prerequisites
sipag tui                                 # Interactive worker dashboard
sipag ps                                  # List workers
sipag logs <id>                           # View worker output
sipag kill <id>                           # Stop a worker
sipag version                             # Print version
```
