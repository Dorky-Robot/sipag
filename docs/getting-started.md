# Getting started with sipag

sipag generates project-aware review agents, ships work through isolated Docker containers, and learns from failures. Re-run `sipag configure` as your project evolves — it analyzes what's there and updates your agents. You create the PR; workers do the work.

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

## 5. Configure a project

Configure review agents and commands for your project:

```bash
cd ~/Projects/my-app
sipag configure
```

This creates files in `.claude/`:

```
.claude/
├── agents/
│   ├── security-reviewer.md
│   ├── architecture-reviewer.md
│   ├── correctness-reviewer.md
│   ├── backlog-triager.md
│   └── issue-analyst.md
└── commands/
    ├── dispatch.md
    ├── review.md
    ├── triage.md
    └── ship-it.md
```

Re-run `sipag configure` as your project evolves — it reads existing files and updates them.

## 6. Create and dispatch work

### Create a PR

Create a branch and PR on GitHub. The PR body is the complete assignment for the worker — include what needs to happen, which issues it addresses, and any constraints:

```bash
git checkout -b sipag/fix-auth-middleware
git push -u origin sipag/fix-auth-middleware
gh pr create --title "Fix auth middleware timeout handling" --body "$(cat <<'EOF'
## Assignment

Fix the auth middleware to handle token refresh timeouts gracefully.

Closes #42
Closes #45

## Context

The auth middleware in `src/middleware/auth.rs` panics when the token refresh
endpoint takes longer than 5 seconds. Instead, it should fall back to the
cached token and log a warning.

## Constraints

- Do not change the token refresh endpoint itself
- Existing tests in `tests/auth_test.rs` must continue to pass
- Add a new test for the timeout fallback behavior
EOF
)"
```

### Dispatch a worker

```bash
sipag dispatch --repo owner/my-app --pr 47
```

This launches a Docker container that clones the repo, reads the PR body, invokes Claude Code to implement the changes, and pushes commits to the PR branch.

## 7. Monitor workers

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
heartbeat_interval=30
heartbeat_stale=90
```

| Key | Default | Description |
|-----|---------|-------------|
| `image` | `ghcr.io/dorky-robot/sipag-worker:latest` | Docker image for workers |
| `timeout` | `7200` | Worker timeout in seconds (2 hours) |
| `work_label` | `ready` | Issue label that marks tasks ready for dispatch |
| `max_open_prs` | `3` | Max active workers before dispatch is paused (0 = no limit) |
| `poll_interval` | `120` | Seconds between polling cycles |
| `heartbeat_interval` | `30` | Seconds between heartbeat writes |
| `heartbeat_stale` | `90` | Seconds before a heartbeat is considered stale |

Environment variables override config file values: `SIPAG_IMAGE`, `SIPAG_TIMEOUT`, `SIPAG_WORK_LABEL`, `SIPAG_MAX_OPEN_PRS`, `SIPAG_DIR`, `SIPAG_HEARTBEAT_INTERVAL`, `SIPAG_HEARTBEAT_STALE`.

## File layout

```
~/.sipag/
├── config          # Optional key=value config
├── workers/        # PR-keyed state JSON files + heartbeat files
├── events/         # Append-only lifecycle events
├── logs/           # Worker stdout/stderr
└── lessons/        # Per-repo learning from failures
```

## Quick reference

```bash
sipag configure                          # Configure agents + commands for .claude/
sipag dispatch --repo owner/repo --pr N  # Launch a Docker worker
sipag doctor                             # Check prerequisites
sipag tui                                # Interactive worker dashboard
sipag ps                                 # List workers
sipag logs <id>                          # View worker output
sipag kill <id>                          # Stop a worker
sipag version                            # Print version
```
