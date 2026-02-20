# sipag — Product Vision

## One-liner

Queue up backlog items, go to sleep, wake up to pull requests.

## Problem

Claude Code can already take a ticket from start to finish — plan, code, test, commit, push, open a PR. But you have to babysit it, approving tool use every few minutes. You're the bottleneck.

## Solution

sipag is two things:

1. **A TUI task manager** — inspired by Taskwarrior, where you manage your backlog: add tasks, prioritize, filter, pull in work from GitHub issues and email.
2. **An autonomous executor** — takes items from the backlog, spins up a Docker container, runs Claude Code with full autonomy, delivers PRs.

The TUI is where you think about what needs doing. The executor is what does it while you sleep.

## The two halves

```
┌─────────────────────────────┐     ┌──────────────────────────┐
│         TUI                 │     │       Executor           │
│                             │     │                          │
│  Manage your backlog:       │     │  Process the backlog:    │
│  - Add/edit/delete tasks    │     │  - Pick next task        │
│  - Prioritize & filter      │     │  - Spin up Docker        │
│  - Sync from GitHub issues  │     │  - Clone repo            │
│  - Sync from email          │     │  - Run claude -p         │
│  - Tag tasks for execution  │     │  - Record result         │
│                             │     │                          │
│  file-based storage ───────────────── queue/ directory       │
└─────────────────────────────┘     └──────────────────────────┘
```

Both halves operate on the same file-based storage. The TUI reads and writes task files. The executor moves them through `queue/ → running/ → done/|failed/`.

## TUI

### Inspired by Taskwarrior, built for sipag

A ratatui terminal app. Keyboard-driven, fast, no mouse needed.

### Views

**List view** (default):
```
┌─ sipag ──────────────────────────────────────────────────────┐
│                                                              │
│  ID  St  Pri  Repo     Title                          Age    │
│  ──  ──  ───  ───────  ─────────────────────────────  ────── │
│  1   ·   H    salita   Implement password reset flow  2d     │
│  2   ·   M    salita   Add rate limiting to endpoints  1d    │
│  3   ✗   M    salita   Fix the flaky WebSocket test   1d     │
│  4   ✓   L    kita     Add dark mode to dashboard     3h     │
│  5   ⧖   M    salita   Refactor date helpers          12m    │
│                                                              │
│  · pending  ⧖ running  ✓ done  ✗ failed                     │
│                                                              │
│  5 tasks (2 pending, 1 running, 1 done, 1 failed)            │
├──────────────────────────────────────────────────────────────┤
│  a:add  e:edit  d:delete  p:priority  r:retry  /:filter      │
│  Enter:detail  s:sync  x:execute  q:quit                     │
└──────────────────────────────────────────────────────────────┘
```

**Detail view** (Enter on a task):
```
┌─ sipag ── #3 ────────────────────────────────────────────────┐
│                                                              │
│  Fix the flaky WebSocket test                                │
│                                                              │
│  Repo:     salita                                            │
│  Status:   failed (exit 1)                                   │
│  Priority: medium                                            │
│  Source:   github #142                                        │
│  Added:    2d ago                                            │
│  Duration: 15m03s                                            │
│                                                              │
│  ── Description ──────────────────────────────────────────── │
│  The test_websocket_reconnect test fails intermittently.     │
│  It passes locally but fails in CI about 30% of the time.   │
│  Likely a race condition in the reconnect handler.           │
│                                                              │
│  ── Log (last 20 lines) ─────────────────────────────────── │
│  Error: assertion failed: connection.state == Connected      │
│  Expected: Connected                                         │
│  Actual: Connecting                                          │
│  ...                                                         │
│                                                              │
├──────────────────────────────────────────────────────────────┤
│  r:retry  e:edit  Esc:back                                   │
└──────────────────────────────────────────────────────────────┘
```

**Add view** (a):
```
┌─ sipag ── new task ──────────────────────────────────────────┐
│                                                              │
│  Title: █                                                    │
│  Repo:  [salita ▼]                                           │
│  Priority: [medium ▼]                                        │
│                                                              │
│  Description:                                                │
│  ┌──────────────────────────────────────────────────────────┐│
│  │                                                          ││
│  │                                                          ││
│  └──────────────────────────────────────────────────────────┘│
│                                                              │
├──────────────────────────────────────────────────────────────┤
│  Tab:next field  Enter:save  Esc:cancel                      │
└──────────────────────────────────────────────────────────────┘
```

### Keybindings

| Key | Action |
|---|---|
| `j/k` | Navigate up/down |
| `a` | Add new task |
| `e` | Edit selected task |
| `d` | Delete selected task |
| `p` | Cycle priority (low → medium → high) |
| `r` | Retry failed task (move back to queue) |
| `x` | Start executor (process queue) |
| `s` | Sync from sources |
| `/` | Filter tasks |
| `Enter` | Show task detail |
| `1-4` | Filter by status (pending/running/done/failed) |
| `q` | Quit |

## Source adapters

Sources pull tasks from external systems into sipag. Adapters are Rust traits — same pattern as tao's resolution (filesystem first, then config, then remote).

### Adapter trait

```rust
trait SourceAdapter {
    fn name(&self) -> &str;
    fn sync(&self, queue_dir: &Path) -> Result<Vec<SyncedTask>>;
}
```

### Built-in adapters

#### GitHub Issues

Pull issues labeled `sipag` from registered repos.

```
sipag sync github
```

- Uses `gh` CLI or GitHub API
- Issue title → task title
- Issue body + comments → task description
- Repo from the issue's repository
- Tracks synced issue numbers to avoid duplicates
- On task completion, comments on the issue with PR link

#### Email (inspired by tao)

Pull tasks from an IMAP inbox. Same approach as tao's email-based human interactions.

```
sipag sync email
```

- Connects to configured IMAP inbox
- Subject line → task title
- Email body → task description
- Repo from subject tag like `[salita]` or a default
- Marks emails as read after syncing
- On completion, replies with PR link

#### Manual

No sync needed. The TUI's `a` key writes a `.md` file. Claude or any script can drop files directly into `queue/`.

### Sync in the TUI

Press `s` to open the sync panel:

```
┌─ sync ───────────────────────┐
│                              │
│  [g] GitHub issues   3 new   │
│  [e] Email           1 new   │
│  [a] Sync all                │
│                              │
│  Esc: cancel                 │
└──────────────────────────────┘
```

## File-based storage

The filesystem is the database. The TUI and executor both read/write the same directories.

```
~/.sipag/
  queue/                                # pending items (FIFO by filename)
    005-add-input-validation.md
    006-refactor-date-helpers.md
  running/                              # currently being worked (0 or 1 file)
    007-fix-n-plus-one.md
  done/                                 # completed successfully
    001-password-reset.md
    001-password-reset.log              # captured claude output
    002-rate-limiting.md
    002-rate-limiting.log
  failed/                               # needs attention
    003-fix-flaky-test.md
    003-fix-flaky-test.log              # captured claude output
  repos.conf                            # registered repos (name=url, one per line)
  sources.conf                          # source adapter config (IMAP creds, etc.)
  .synced                               # tracks which external items have been synced
```

### Task file format

```markdown
---
repo: salita
priority: medium
source: github#142
added: 2026-02-19T22:30:00Z
---
Implement password reset flow

The user should receive an email with a one-time reset link.
Token expires after 1 hour. Use the existing email service.
```

### Why files, not SQLite

- **Any tool can add tasks.** Claude, kubo, tao, a shell script — just write a `.md` file.
- **Human-readable.** `cat`, `vim`, `ls`. No query language.
- **Debuggable.** `ls running/` tells you what's happening right now.
- **TUI and executor are decoupled.** They share a directory, not a database connection.
- **Composable.** Unix tools just work on files.

## Executor

### Container as safety boundary

Docker replaces the approval dialog. Inside the container Claude has full autonomy. Outside, nothing is touched.

The container gets:
- Repo cloned fresh from remote
- `ANTHROPIC_API_KEY` and `GH_TOKEN` (env vars)
- Git identity configured
- `claude` CLI, `gh` CLI, `git`
- Network access

The container does NOT get:
- Host filesystem access
- Your SSH keys
- Unlimited resources

### What runs inside the container

```bash
git clone "$REPO_URL" /work
cd /work
git config user.name "sipag"
git config user.email "sipag@localhost"

claude --print --dangerously-skip-permissions -p "$PROMPT"
```

### Worker loop (serial)

The executor runs from the TUI (press `x`) or from CLI (`sipag start`). Serial — one task at a time.

```bash
while true; do
    next=$(ls queue/*.md 2>/dev/null | head -1)
    [[ -z "$next" ]] && break

    mv "$next" running/
    name=$(basename "$next" .md)

    repo=$(parse_repo "running/${name}.md")
    prompt=$(parse_prompt "running/${name}.md")
    url=$(repo_url "$repo")

    docker run --rm \
        -e ANTHROPIC_API_KEY \
        -e GH_TOKEN \
        sipag-worker "$url" "$prompt" \
        > "running/${name}.log" 2>&1

    if [[ $? -eq 0 ]]; then
        mv "running/${name}.md" "done/${name}.md"
        mv "running/${name}.log" "done/${name}.log"
    else
        mv "running/${name}.md" "failed/${name}.md"
        mv "running/${name}.log" "failed/${name}.log"
    fi
done
```

The TUI watches the filesystem and updates the view in real-time as files move between directories.

## CLI

The TUI is the primary interface, but everything is also available as CLI commands:

```
sipag                                               Launch TUI
sipag add <title> --repo <name> [--body <text>]     Write task file to queue/
sipag start                                          Process queue (foreground, serial)
sipag status                                         List items by status
sipag show <name>                                    Print task file + log
sipag retry <name>                                   Move from failed/ back to queue/
sipag sync <source>                                  Pull tasks from external source
sipag repo add <name> <url>                          Register a repo
sipag repo list                                      List registered repos
```

## Prompt template

What sipag passes to Claude inside the container:

```
You are working on the repository at /work.

Your task:
<title + body from the .md file>

Instructions:
- Create a new branch with a descriptive name
- Before writing any code, open a draft pull request with:
    - The PR title matching the task title
    - A body containing:
        > 🤖 This PR is being worked on by sipag. Commits will appear as work progresses.
        Task: <title>
        Issue: #<number>  (include only if the task references a GitHub issue number)
- Implement the changes
- Commit and push after each logical unit of work so GitHub reflects progress in real-time
- Run any existing tests and make sure they pass
- Update the PR body with a summary of what you changed and why
- When all work is complete, mark the pull request as ready for review
```

## Configuration

Env vars for credentials, config files for everything else.

| Variable | Default | Purpose |
|---|---|---|
| `SIPAG_DIR` | `~/.sipag` | Data directory |
| `SIPAG_IMAGE` | `sipag-worker:latest` | Docker base image |
| `SIPAG_TIMEOUT` | `1800` | Per-item timeout (seconds) |
| `SIPAG_MODEL` | _(claude default)_ | Model override |
| `ANTHROPIC_API_KEY` | _(required)_ | Passed into container |
| `GH_TOKEN` | _(required)_ | Passed into container for push + PR |

## Technology

**TUI**: Rust + ratatui. Fits the stack (tao is Rust, salita is Rust). Watches `~/.sipag/` directories for changes, renders the task list.

**Executor**: Bash script. `mv`, `ls`, `docker run`, while loop. Called from the TUI or standalone via `sipag start`.

**Source adapters**: Rust (compiled into the TUI binary). GitHub adapter uses `gh` CLI. Email adapter uses IMAP (like tao).

## Relationship to the stack

```
┌───────────────────────────────────────────────────────┐
│                    sipag TUI                          │
│                                                       │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐            │
│  │ GitHub   │  │  Email   │  │  Manual  │  adapters   │
│  │ adapter  │  │ adapter  │  │ (+ kubo, │            │
│  │          │  │ (à la    │  │  tao,    │            │
│  │          │  │   tao)   │  │  claude) │            │
│  └────┬─────┘  └────┬─────┘  └────┬─────┘            │
│       └──────────────┴─────────────┘                  │
│                      │ .md files                      │
│                      ▼                                │
│              ~/.sipag/queue/                           │
│                      │                                │
│                      ▼                                │
│              executor (bash)                          │
│              docker → claude → PR                     │
└───────────────────────────────────────────────────────┘
```

## Non-goals (MVP)

- Parallel execution
- Multi-repo items
- Auto-retry
- Notifications
- Cost tracking
- Anything Claude Code already does

## Success criteria

1. `sipag` launches a TUI where you can manage your backlog
2. `s` syncs GitHub issues into the task list
3. `x` starts the executor — tasks get worked serially
4. Wake up to PRs on GitHub
5. Failed tasks show full logs in the detail view
6. Any tool can add work by dropping a `.md` file in `queue/`
