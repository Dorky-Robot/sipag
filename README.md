# sipag

<div align="center">

<video src="sipag.mp4" width="600" controls></video>

*Queue up backlog items, go to sleep, wake up to pull requests.*

</div>

## What is sipag?

sipag is an autonomous dev agent. It takes items from your backlog, spins up an isolated Docker container, runs Claude Code with full autonomy, and opens a PR when it's done.

You manage the backlog. sipag does the work. You review PRs in the morning.

```bash
sipag add "Implement password reset flow" --repo salita
sipag add "Add rate limiting to API endpoints" --repo salita
sipag add "Fix the flaky WebSocket test" --repo salita

sipag start
# Go to bed. Wake up to PRs.
```

## How it works

```
backlog item → Docker container → clone repo → claude -p → PR
```

1. You add items to the backlog (via TUI, CLI, or just drop a `.md` file)
2. sipag picks the next item, spins up a Docker container
3. Clones the repo fresh, injects credentials
4. Runs `claude --dangerously-skip-permissions` — Claude plans, codes, tests, commits, pushes, and opens a PR
5. Records the result, tears down the container, picks up the next item

The container is the safety boundary. Claude has full autonomy inside it. No approval dialogs, no babysitting.

## The two halves

**TUI** — A terminal task manager (inspired by Taskwarrior, built with ratatui). Manage your backlog: add, edit, prioritize, filter. Sync from GitHub issues and email.

**Executor** — A serial worker loop. Picks up tasks, runs them in Docker, delivers PRs. Launched from the TUI or standalone.

Both operate on the same file-based storage:

```
~/.sipag/
  queue/                     # pending items
    001-password-reset.md
    002-rate-limiting.md
  running/                   # currently being worked
  done/                      # completed (with .log files)
  failed/                    # needs attention (with .log files)
  repos.conf                 # registered repos
```

Directories are statuses. Moving a file is a state transition. Any tool that can write a `.md` file can add work.

## Task file format

```markdown
---
repo: salita
priority: medium
---
Implement password reset flow

The user should receive an email with a one-time reset link.
Token expires after 1 hour. Use the existing email service.
```

## CLI

```
sipag                                            Launch TUI
sipag add <title> --repo <name> [--body <text>]  Add task to queue
sipag start                                       Process queue (serial)
sipag status                                      Show queue state
sipag show <name>                                 Print task + log
sipag retry <name>                                Re-queue a failed task
sipag sync <source>                               Pull from GitHub/email
sipag repo add <name> <url>                       Register a repo
sipag repo list                                   List repos
```

## Source adapters

Sources pull tasks from external systems into the queue.

- **GitHub Issues** — label issues with `sipag`, sync pulls them in
- **Email** — forward tasks to an inbox, sync pulls them in (inspired by [tao](https://github.com/Dorky-Robot/tao))
- **Manual** — drop a `.md` file in `queue/`, or use the TUI/CLI

## Configuration

| Variable | Default | Purpose |
|---|---|---|
| `SIPAG_DIR` | `~/.sipag` | Data directory |
| `SIPAG_IMAGE` | `sipag-worker:latest` | Docker base image |
| `SIPAG_TIMEOUT` | `1800` | Per-item timeout (seconds) |
| `SIPAG_MODEL` | _(claude default)_ | Model override |
| `ANTHROPIC_API_KEY` | _(required)_ | Passed into container |
| `GH_TOKEN` | _(required)_ | Passed into container |

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
brew install bats-core shellcheck shfmt
make dev     # lint + fmt-check + test
make test    # all tests
make lint    # shellcheck
```

## Status

sipag v2 is in active development. See [VISION.md](VISION.md) for the full product vision.
