# How It Works

sipag generates project-aware review agents, ships work through isolated Docker containers, and learns from failures. This document walks through every stage.

---

## The big picture

```
┌──────────────────────────────────────────────────────┐
│  sipag configure                                      │
│  Configures agents and commands                       │
│  for your project's .claude/ directory                │
└──────────────────────────┬───────────────────────────┘
                           │
                           v
┌──────────────────────────────────────────────────────┐
│  You create a branch + PR on GitHub                   │
│  PR body = the complete assignment for the worker     │
└──────────────────────────┬───────────────────────────┘
                           │
                           v
┌──────────────────────────────────────────────────────┐
│  sipag dispatch <PR_URL>                              │
│                                                       │
│  Preflight: gh auth, Docker daemon, image available   │
│  Back-pressure: count active workers vs max_open_prs  │
│  PR lookup: fetch branch name and body                │
│  Launch: docker run with mounts + credentials         │
└──────────────────────────┬───────────────────────────┘
                           │
                           v
┌──────────────────────────────────────────────────────┐
│  Docker container (sipag-worker)                      │
│                                                       │
│  clone repo → checkout branch → read PR body          │
│  read lessons from past failures                      │
│  invoke Claude Code (--dangerously-skip-permissions)  │
│  push commits → write state + events                  │
└──────────────────────────┬───────────────────────────┘
                           │
                           v
┌──────────────────────────────────────────────────────┐
│  sipag tui / sipag ps                                 │
│  Monitor worker progress, view logs, kill workers     │
└──────────────────────────────────────────────────────┘
```

---

## sipag configure

`sipag configure` generates project-specific agents and commands for your project's `.claude/` directory:

| Category | Files | Purpose |
|----------|-------|---------|
| Agents | `security-reviewer.md`, `architecture-reviewer.md`, `correctness-reviewer.md`, `backlog-triager.md`, `issue-analyst.md` | Specialized review agents usable via Claude Code's Task tool |
| Commands | `dispatch.md`, `review.md`, `triage.md`, `ship-it.md` | Custom slash commands for Claude Code |

By default, it launches Claude to analyze your project and write tailored agents. Use `--static` to install generic templates without Claude. Re-run as your project evolves.

---

## sipag dispatch

`sipag dispatch <PR_URL>` is a one-shot command that launches a Docker worker for a specific PR. Here's the full sequence:

### Preflight checks

1. **gh auth** — Verify GitHub CLI is authenticated
2. **Docker daemon** — Verify Docker is running
3. **Docker image** — Verify the worker image is available locally

If any check fails, dispatch exits immediately with an error.

### Back-pressure check

Count active (non-terminal) workers. If the count equals or exceeds `max_open_prs`, dispatch refuses to start. This is fail-closed: capacity must be verified before new work starts.

### PR lookup

Fetch the PR's branch name and body via `gh pr view`. Extract issue numbers from `Closes/Fixes/Resolves #N` patterns in the body.

### Container launch

Create a Docker container with:
- The repo URL, PR number, and branch as environment variables
- Credentials (`GH_TOKEN`, `CLAUDE_CODE_OAUTH_TOKEN` or `ANTHROPIC_API_KEY`) as environment variables
- Three host directory mounts (state, lessons, events)
- A timeout wrapper (`timeout` on Linux, `gtimeout` on macOS)

---

## Worker execution

When the container starts, the `sipag-worker` binary runs through a structured lifecycle:

### Setup

```
┌──────────────────────────────────────────────────────────┐
│  Container startup                                        │
│                                                           │
│  1. Read env: REPO, PR_NUM, BRANCH, STATE_FILE           │
│  2. Write git credential file (/tmp/.git-credentials)     │
│     - mode 0600, token never in process args              │
│  3. git clone https://github.com/{repo}.git /work         │
│  4. git fetch origin {branch} && git checkout {branch}    │
│  5. Sanity check: >= 5 tracked files (catch bad branches) │
│  6. Read PR body via gh pr view (the assignment)          │
│  7. Read lessons from /sipag-lessons/{repo}.md            │
│  8. Update state: phase = Working                         │
│  9. Emit worker-started event                             │
└──────────────────────────────────────────────────────────┘
```

The token is written to a credential file with `0600` permissions, not passed as a git clone argument. This keeps it out of `ps aux` and `/proc/PID/cmdline`.

### Claude Code invocation

The worker builds a prompt from three parts:

1. **PR body** — The complete assignment. Contains the issue references, architectural context, implementation approach, and constraints.
2. **Lessons** — Failures from previous workers for this repo (last 8KB, truncated at section boundaries).
3. **Worker disposition** — Procedural instructions from `worker.md`: push to branch only, resolve merge conflicts first, address review feedback before new work, run tests.

Claude Code runs with `--dangerously-skip-permissions` from the `/work` directory. The container is the safety boundary.

### Supervision loop

While Claude runs, the worker supervises it with a 10-second tick loop:

```
every 10 seconds:
  ├─ check if Claude exited naturally (try_wait)
  ├─ write heartbeat (every heartbeat_interval, default 30s)
  └─ check PR state on GitHub (every 5 minutes)
      ├─ PR still open → continue
      ├─ PR merged → start 120s grace period, then kill
      └─ PR closed → start 120s grace period, then kill
```

The grace period lets Claude finish any in-progress commit or push before being terminated.

### Post-run verification

When Claude exits with code 0, the worker verifies that commits were actually pushed:

1. Compare HEAD before and after Claude ran
2. If HEAD didn't change: mark as failed (no commits)
3. If HEAD changed: fetch remote, check for unpushed commits
4. If unpushed commits exist: mark as failed
5. If PR was merged by Claude itself: skip verification (merge is proof)

This catches the case where Claude reports success but didn't actually push anything.

### Cleanup

The worker writes the final state (finished or failed with exit code), emits the appropriate lifecycle event, and removes the heartbeat file.

---

## Liveness detection

`scan_workers()` needs to know which workers are alive. It uses a three-tier approach:

| Tier | Method | Speed | When |
|------|--------|-------|------|
| 1 | Heartbeat file mtime | One `stat()` call | Primary — most workers |
| 2 | Grace period (started < 60s ago) | State file timestamp | New workers that haven't written a heartbeat yet |
| 3 | `docker ps` | Shell out | Fallback for workers without heartbeat files |

If a heartbeat is older than `heartbeat_stale` seconds (default 90s) and the worker is past the grace period, the worker is considered dead. It gets marked as failed and a `worker-orphaned` event is emitted.

---

## State management

All state is PR-keyed JSON files at `~/.sipag/workers/{owner}--{repo}--pr-{N}.json`:

```json
{
  "repo": "owner/repo",
  "pr_num": 42,
  "issues": [10, 11],
  "branch": "sipag/pr-42",
  "container_id": "sipag-owner--repo-pr-42",
  "phase": "working",
  "heartbeat": "2026-01-15T10:30:00Z",
  "started": "2026-01-15T10:30:00Z",
  "ended": null,
  "exit_code": null,
  "error": null
}
```

**Phase transitions**: `starting` -> `working` -> `finished` | `failed`

**Atomic writes**: State is written to a temp file in the same directory, then renamed. POSIX rename on the same filesystem is atomic, so readers never see partial writes.

**Heartbeat files**: A separate `.heartbeat` file sits alongside the state file. Its mtime is the primary liveness signal. A separate file avoids a race between the heartbeat thread and the main thread both writing to the same state JSON.

---

## Lifecycle events

Workers emit events to `~/.sipag/events/` as markdown files with chronologically sortable names:

```
~/.sipag/events/
├── 2026-02-24T10:30:00Z-worker-started-acme--my-app-1234567890.md
├── 2026-02-24T12:15:00Z-worker-finished-acme--my-app-1234567891.md
└── 2026-02-24T12:20:00Z-worker-failed-acme--api-1234567892.md
```

Event types: `worker-started`, `worker-finished`, `worker-failed`, `worker-orphaned`.

These files are append-only and never modified. External systems (Slack hooks, monitoring scripts, the [tao](https://github.com/Dorky-Robot/tao) decision tracker) can watch this directory and react. Adding a new consumer requires zero changes to sipag.

---

## Learning from failure

When a worker fails, sipag:

1. Reads the log file (`~/.sipag/logs/{owner}--{repo}--pr-{N}.log`)
2. Extracts the failure reason (pattern matching for known errors: auth failures, OOM, no commits pushed, API errors)
3. Appends a lesson to `~/.sipag/lessons/{owner}--{repo}.md`

The next worker for the same repo reads these lessons at startup and includes them in its prompt. This creates a feedback loop where workers learn from past mistakes without human intervention.

Lessons are capped at 8KB per repo. When the file grows beyond that, older lessons are truncated from the front at section boundaries so recent lessons are always preserved.

---

## Back-pressure

sipag enforces a work-in-progress limit: at most `max_open_prs` (default 3) active workers can run at once.

The policy is **fail closed**: if the count can't be verified, dispatch refuses to start. No new work begins when the system can't verify its capacity.

The reasoning follows flow theory: finishing work-in-progress is always more valuable than starting new work. A smaller batch size means faster cycle time and less context switching.

---

## Docker container details

The worker container (built from the project Dockerfile) contains:

- Ubuntu 24.04 base
- Node 22 + Claude Code CLI
- GitHub CLI (`gh`)
- Git, curl, build-essential
- `sipag-worker` binary at `/usr/local/bin/sipag-worker`
- Non-root user `sipag` (Claude refuses `--dangerously-skip-permissions` as root)

The host mounts three directories into the container:

| Host path | Container path | Mode | Purpose |
|-----------|---------------|------|---------|
| `~/.sipag/workers/` | `/sipag-state` | read-write | State files and heartbeats |
| `~/.sipag/lessons/` | `/sipag-lessons` | read-only | Cross-worker learning |
| `~/.sipag/events/` | `/sipag-events` | read-write | Lifecycle event emission |

Credentials are passed as environment variables (never as Docker build args or command-line arguments):

- `GH_TOKEN` — GitHub access
- `CLAUDE_CODE_OAUTH_TOKEN` — Claude OAuth (if available)
- `ANTHROPIC_API_KEY` — Claude API key (fallback)

The container is wrapped in a timeout command (`timeout` on Linux, `gtimeout` on macOS) set to `timeout` seconds (default 7200 = 2 hours).
