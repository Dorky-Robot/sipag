# CLI Reference

## sipag configure

Configure review agents and custom commands for a project's `.claude/` directory.

```
sipag configure [DIR] [--static]
```

| Argument | Default | Description |
|----------|---------|-------------|
| `DIR` | `.` (current directory) | Project directory to configure |
| `--static` | off | Install generic templates without launching Claude |

**Examples:**

```bash
sipag configure                    # Configure current project (launches Claude)
sipag configure ~/Projects/my-app  # Configure a specific project
sipag configure --static           # Install generic templates without Claude
```

**What gets installed:**

- `.claude/agents/` — 5 review agents (security, architecture, correctness, backlog, issue)
- `.claude/commands/` — 4 custom commands (dispatch, review, triage, ship-it)

Re-run `sipag configure` as your project evolves — it reads existing files and updates them.

---

## sipag dispatch

Launch a Docker worker for a specific PR.

```
sipag dispatch <PR_URL>
```

| Argument | Required | Description |
|----------|----------|-------------|
| `PR_URL` | yes | GitHub PR URL (e.g. `https://github.com/owner/repo/pull/42`) |

**Examples:**

```bash
sipag dispatch https://github.com/acme/my-app/pull/42
sipag dispatch https://github.com/Dorky-Robot/sipag/pull/123
```

**What it does:**

1. Runs preflight checks (gh auth, Docker daemon, Docker image)
2. Checks back-pressure (refuses if active workers >= `max_open_prs`)
3. Fetches the PR branch and body via `gh pr view`
4. Launches a Docker container that clones, implements, and pushes

**Environment overrides:**

- `SIPAG_IMAGE` — use a different Docker image
- `SIPAG_TIMEOUT` — override worker timeout

---

## sipag ps

List active and recent workers.

```
sipag ps [--all]
```

| Flag | Default | Description |
|------|---------|-------------|
| `--all` | off | Show all workers (not just active + recent) |

By default, shows active workers plus the 5 most recent terminal workers from the last 24 hours. Use `--all` to see everything.

**Example output:**

```
PR       REPO                           PHASE        AGE      CONTAINER
------------------------------------------------------------------------------
#42      acme/my-app                    working      15m      pr-42
#38      acme/my-app                    finished     2h       pr-38
#35      acme/my-app                    failed       5h       pr-35
         ↳ No commits pushed

3 active, 1 finished, 1 failed (3 total)
```

---

## sipag logs

Show logs for a worker.

```
sipag logs <ID>
```

| Argument | Description |
|----------|-------------|
| `ID` | PR number (e.g. `42` or `#42`) or Docker container name |

**Examples:**

```bash
sipag logs 42       # View logs for PR #42
sipag logs #42      # Same thing
```

Reads from the log file at `~/.sipag/logs/{owner}--{repo}--pr-{N}.log`. Falls back to `docker logs` if no log file exists.

---

## sipag kill

Kill a running worker.

```
sipag kill <ID>
```

| Argument | Description |
|----------|-------------|
| `ID` | PR number (e.g. `42` or `#42`) or Docker container name |

**Examples:**

```bash
sipag kill 42       # Kill worker for PR #42
```

Stops the Docker container and marks the worker state as failed with "Killed by user". If the worker already reached a terminal state (finished/failed), the state is preserved.

---

## sipag tui

Launch the interactive terminal UI. Also runs when `sipag` is invoked with no arguments.

```
sipag tui
sipag         # equivalent
```

Shows all workers across all repos in a live table with keyboard navigation:

| Key | Action |
|-----|--------|
| `j` / `↓` | Move down |
| `k` / `↑` | Move up / scroll up |
| `Enter` | Open detail view |
| `Esc` | Back to list |
| `a` | Attach to container shell |
| `k` | Kill selected worker |
| `K` | Kill all active workers |
| `x` / `Delete` | Dismiss finished/failed worker |
| `Tab` | Toggle active/archive views |
| `r` | Refresh |
| `q` | Quit |

---

## sipag doctor

Check system prerequisites.

```
sipag doctor
```

Checks:

- Docker daemon running
- Docker worker image available
- GitHub CLI authenticated
- `~/.sipag/` directory exists
- Config file validation (if present)

**Example output:**

```
sipag doctor
============

Docker daemon:  OK
Docker image:   OK (ghcr.io/dorky-robot/sipag-worker:latest)
GitHub CLI:     OK
sipag dir:      OK (/Users/you/.sipag)
```

---

## sipag version

Print version and git commit hash.

```
sipag version
```

**Example output:**

```
sipag 0.5.0 (a1b2c3d)
```
