# How It Works

sipag has two layers: a **conversation layer** where you and Claude plan work together, and an **execution layer** where Docker workers implement that work autonomously.

## The full flow

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

## The conversation layer

### What `sipag start` does

`sipag start <repo>` dumps the current GitHub board state to stdout — open issues, open PRs, labels, recent activity. This primes Claude Code with full context so it can immediately engage on product questions.

Claude then adapts the conversation to what the board needs:

- **Approved backlog?** Claude starts workers.
- **Vague issues?** Claude asks you product questions and opens refined issues.
- **Stacked PRs?** Claude suggests a merge session.

You don't direct Claude through menus. The conversation is natural — Claude reads the situation and acts.

### What `sipag merge` does

`sipag merge <repo>` is a focused code review conversation. Claude pulls up each open PR, summarizes what changed, and lets you decide: merge, request changes, or skip. You can ask questions, drill into diffs, or just say "ship it."

## The execution layer

### Worker lifecycle

When Claude decides work is ready (issues labeled `approved`), it runs `sipag work <repo>`. Workers operate in a polling loop:

```
poll GitHub for issues labeled "approved"
          ↓
pick up issue, add "in-progress" label
          ↓
spin up Docker container
          ↓
clone repo, inject credentials
          ↓
run claude --dangerously-skip-permissions
          ↓
Claude: plan → branch → code → test → commit → push → PR
          ↓
on success: PR opened, issue updated
on failure: remove "in-progress", return issue to "approved"
```

Multiple issues run in parallel — one container per issue.

### The container as safety boundary

Docker replaces the approval dialog. Inside the container, Claude has full autonomy — no permission prompts, no interruptions. Outside, nothing on the host machine is touched.

The container gets:

| Provided | Not provided |
|---|---|
| Fresh repo clone | Host filesystem |
| `ANTHROPIC_API_KEY` | Your SSH keys |
| `GH_TOKEN` | Unlimited resources |
| `claude` CLI, `gh` CLI, `git` | Access to other repos |
| Network access | Docker socket |

The worker image is `ghcr.io/dorky-robot/sipag-worker:latest`, published automatically on each release.

### What Claude does inside the container

Claude receives a prompt that includes:

- The GitHub issue title and body
- Your repo's `CLAUDE.md` (if present) — coding conventions, test commands, architecture notes
- Instructions: create a branch, open a draft PR first, commit incrementally, push after each commit, run tests, mark PR ready when done

Claude then executes this plan fully autonomously. If it hits a wall (build fails, tests fail, unclear requirement), it documents the problem in the PR body and marks the issue as failed.

### Prompt template

```
You are working on the repository at /work.

Your task:
<issue title + body>

Instructions:
- Create a new branch with a descriptive name
- Before writing any code, open a draft pull request
- Commit after each logical unit of work
- Push after each commit so GitHub reflects progress in real time
- Run any existing tests and make sure they pass
- When all work is complete, update the PR body with a summary
- Mark the pull request as ready for review
```

## File-based state

sipag uses the filesystem as its database. The TUI and executor both read/write the same directories under `~/.sipag/`:

```
~/.sipag/
  queue/      # pending tasks  (YAML frontmatter + description)
  running/    # active tasks   (tracking file + .log)
  done/       # completed tasks
  failed/     # tasks needing attention
```

Task files move through these directories as they're processed:

```
queue/ → running/ → done/
                 └→ failed/
```

Any tool can add work by dropping a `.md` file into `queue/`. The format is simple:

```yaml
---
repo: https://github.com/org/repo
issue: 42
started: 2024-01-01T12:00:00Z
container: sipag-20240101120000-fix-bug
---
Task description here
```

## Lifecycle hooks

sipag emits events at key milestones via hook scripts — the same pattern as git hooks. Drop executable scripts into `~/.sipag/hooks/` named after the event.

| Hook | When it fires |
|---|---|
| `on-worker-started` | Worker picked up an issue |
| `on-worker-completed` | Worker finished, PR opened |
| `on-worker-failed` | Worker exited non-zero |
| `on-pr-iteration-started` | Worker iterating on PR feedback |
| `on-pr-iteration-done` | PR iteration complete |

Hooks receive event data as environment variables. For example, `on-worker-completed` receives:

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

sipag has no notification logic built in. It emits events; you decide what to do:

```bash
#!/usr/bin/env bash
# ~/.sipag/hooks/on-worker-completed — desktop notification on macOS
osascript -e "display notification \"PR opened for #${SIPAG_ISSUE}\" with title \"sipag\""
```

## The TUI

Running `sipag` with no arguments opens the interactive terminal UI:

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

Color coding: yellow = queued, cyan = running, green = done, red = failed.

Keyboard navigation: `↑`/`k` up, `↓`/`j` down, `r` refresh, `q`/`Esc` quit.

---

[Configuration reference →](configuration.md){ .md-button .md-button--primary }
[CLI reference →](cli-reference.md){ .md-button }
