# Your First sipag Session

A walkthrough of a real sipag session from start to merged PR.

## Before you begin

Make sure you've:

- [Installed sipag and run `sipag setup`](../getting-started.md)
- Set `ANTHROPIC_API_KEY` and `GH_TOKEN`
- Have Docker running
- Have a GitHub repo with at least one issue

## Start a session

Open a new terminal and launch Claude Code:

```bash
claude
```

Inside the session, start sipag:

```
sipag start Dorky-Robot/myapp
```

Claude receives a snapshot of your GitHub board — open issues, PRs, labels, recent commits — and begins orienting itself.

## The opening conversation

Claude's first message typically looks something like:

> I've read the board for myapp. Here's what I see:
>
> **Open issues (4):**
> - #12: Add password reset flow (no label)
> - #13: Fix the flaky WebSocket test (approved)
> - #14: Upgrade dependencies (needs-spec)
> - #15: Add rate limiting (approved)
>
> **Open PRs (1):**
> - #11: Dark mode toggle (draft, opened by sipag worker)
>
> Two issues are approved (#13 and #15). I can start workers on those now. #12 looks ready but needs the approved label — want me to add it? #14 needs more detail before it's workable.
>
> What would you like to focus on?

From here, it's a natural conversation.

## Working through the board

### Approving work

If an issue is ready, tell Claude:

> "Go ahead and start workers on #13 and #15."

Claude labels them `in-progress`, starts `sipag work` in the background, and continues talking with you.

### Refining a vague issue

If Claude flags #14 as needing more detail:

> "What's unclear about the rate limiting issue?"

Claude might respond:

> "The issue says 'add rate limiting' but doesn't specify: which endpoints? What limits? What happens when the limit is hit — 429, or just slow the request? I can implement a reasonable default (100 req/min per IP, 429 on exceeded), or we can spec it out more precisely first."

You decide:

> "100 req/min per IP, 429 with a Retry-After header. Go ahead and approve it."

Claude creates a refined issue with that spec, labels it `approved`, and a worker picks it up.

### Checking on a vague issue you want to defer

> "Skip #14 for now. Let's wait until we have load test data."

Claude notes it and moves on.

## Watching workers in the TUI

Open a second terminal and run:

```bash
sipag
```

You'll see the TUI update in real-time as workers pick up issues:

```
┌─ sipag ──────────────────────────────────────────────────────┐
│                                                              │
│  ID  St  Pri  Repo   Title                            Age    │
│  ──  ──  ───  ─────  ─────────────────────────────  ──────── │
│  1   ⧖   M    myapp  Fix the flaky WebSocket test   2m       │
│  2   ⧖   M    myapp  Add rate limiting              1m       │
│                                                              │
│  · pending  ⧖ running  ✓ done  ✗ failed                     │
│                                                              │
│  2 tasks (0 pending, 2 running, 0 done, 0 failed)            │
├──────────────────────────────────────────────────────────────┤
│  r:refresh  Enter:detail  q:quit                             │
└──────────────────────────────────────────────────────────────┘
```

Press `Enter` on a running task to see its live log.

## When workers finish

Workers open PRs and update the GitHub issue. You can see PRs opening in real-time on GitHub, or watch the TUI flip tasks from `⧖ running` to `✓ done`.

If a lifecycle hook is configured, you'll get a notification:

```bash
# ~/.sipag/hooks/on-worker-completed
osascript -e "display notification \"PR #${SIPAG_PR_NUM} opened for #${SIPAG_ISSUE}\" with title \"sipag\""
```

## Reviewing PRs

Once PRs are stacked up, start a merge session:

```
sipag merge Dorky-Robot/myapp
```

Claude walks through each open PR:

> **PR #16: Fix the flaky WebSocket test**
>
> This PR identifies a race condition in the reconnect handler. The fix adds a short delay after connection state changes before asserting the state — not the most robust solution, but it eliminates the flakiness in CI.
>
> `src/websocket.rs` — 12 lines changed
>
> Tests: all passing. CI: green.
>
> Merge, request changes, or skip?

You respond naturally:

> "The delay feels like a bandage. Can you look at the actual race condition more carefully?"

Claude notes the feedback and can relay it or request changes on the PR for the worker to iterate.

> "Merge the rate limiting one though."

Claude merges #15.

## The full loop

After a session, you've:

1. Read and triaged your board in minutes, not hours
2. Dispatched multiple workers in parallel
3. Had vague issues clarified and specced
4. Reviewed and merged PRs conversationally

The next session picks up where this one left off — Claude reads the updated board and continues.

---

[Setting up a new repo →](new-repo-setup.md){ .md-button .md-button--primary }
[Running sipag in CI →](ci-integration.md){ .md-button }
