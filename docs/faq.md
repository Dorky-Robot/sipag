# FAQ

Common questions and troubleshooting.

## General

### What is sipag?

sipag is a sandbox launcher for Claude Code. You have a conversation about what to build; sipag spins up isolated Docker containers where Claude Code works autonomously — planning, coding, testing, committing, and opening pull requests.

### How is this different from just using Claude Code?

Claude Code normally requires you to approve tool use every few minutes. sipag removes that bottleneck by running Claude inside Docker containers with `--dangerously-skip-permissions`. The container is the safety boundary — Claude has full autonomy inside it, but nothing outside the container is touched.

You also get the conversation layer: `sipag start` primes Claude with your full GitHub board, turning a regular session into a product planning conversation where Claude acts as your engineering team.

### Do I need to watch the workers while they run?

No. Workers run fully autonomously and open PRs when done. You can watch the TUI if you want, set up lifecycle hooks for notifications, or come back hours later and run `sipag merge` to review what shipped.

### What tasks work well with sipag?

sipag workers do best with **well-defined, self-contained tasks**:

- Bug fixes with clear reproduction steps
- Adding new endpoints or UI components with specific requirements
- Writing tests for existing code
- Updating documentation
- Dependency upgrades
- Refactors with clear before/after

Tasks that need ongoing conversation or external judgment (user research, ambiguous product decisions, performance optimization without metrics) are better handled interactively.

### How long does a worker take?

Depends on the task. A small bug fix might take 5–10 minutes. A new feature might take 20–40 minutes. The default timeout is 30 minutes (`SIPAG_TIMEOUT=1800`).

### How much does it cost?

Workers call the Claude API. Cost depends on task size and the model used. A typical task uses 50–200K tokens. At current Sonnet pricing, that's roughly $0.15–$0.60 per task. Use `SIPAG_MODEL` to override the model if you want to use a cheaper or faster option.

---

## Installation

### What platforms are supported?

macOS (Intel and Apple Silicon) and Linux (x86_64 and ARM64). Windows is not supported.

### I installed sipag but `sipag start` isn't found

The bash helper commands (`sipag start`, `sipag merge`, `sipag work`, `sipag setup`) are separate from the Rust binary. If you installed via Homebrew, add the libexec path to your shell profile:

```bash
export PATH="$(brew --prefix sipag)/libexec/bin:$PATH"
```

If you installed via the one-line script, the bash commands are installed to `/usr/local/bin/`. Check that it's in your `PATH`:

```bash
which sipag start   # should show /usr/local/bin/sipag
```

### The one-line installer failed

Check that you have `curl` and `bash` installed, and that `/usr/local/bin` is writable:

```bash
ls -la /usr/local/bin/
```

If you need a different install location, download the tarball from the [releases page](https://github.com/Dorky-Robot/sipag/releases) and extract manually.

---

## Workers

### Workers aren't picking up my issues

Check that:

1. The issue has the `approved` label (not `Approved` — it's case-sensitive)
2. Docker is running: `docker ps`
3. Your `GH_TOKEN` has repo access: `gh auth status`
4. The `approved` label exists in the repo: `gh label list --repo owner/repo`

### A worker failed — what do I do?

Check the log:

```bash
sipag logs <task-id>
```

Or look at the detail view in the TUI (press `Enter` on a failed task).

Failed tasks land in `~/.sipag/failed/`. Fix the underlying issue (clarify the GitHub issue, fix a broken test, update API keys), then retry:

```bash
sipag retry <task-name>
```

### The worker opened a PR but the code is wrong

Review the PR diff, leave comments requesting changes. Workers don't automatically iterate on PR feedback — you'd need to re-dispatch a worker with additional context in the issue.

Add more detail to the original issue (or a follow-up issue), approve it, and dispatch a new worker.

### Can I run multiple workers at the same time?

Yes. `sipag work` dispatches one container per approved issue and can run them in parallel. The `batch_size` config controls the maximum concurrent workers (default: unlimited). See [Configuration](configuration.md).

### The container times out before finishing

Increase `SIPAG_TIMEOUT`:

```bash
SIPAG_TIMEOUT=3600 sipag work owner/repo  # 1 hour
```

Or set it in `~/.sipag/config`:

```ini
timeout=3600
```

### Workers are using the wrong Docker image

Set `SIPAG_IMAGE` to override:

```bash
SIPAG_IMAGE=ghcr.io/dorky-robot/sipag-worker:v1.2.3 sipag work owner/repo
```

Or build locally:

```bash
docker build -t sipag-worker:local .
SIPAG_IMAGE=sipag-worker:local sipag work owner/repo
```

---

## GitHub integration

### Do workers need write access to the repo?

Yes. Workers clone the repo, push branches, and open PRs. Your `GH_TOKEN` needs `repo` scope (or `contents: write` + `pull-requests: write` for fine-grained tokens).

### Can workers access private repos?

Yes, as long as `GH_TOKEN` has access to the repo.

### Workers are hitting GitHub rate limits

The GitHub API rate limit is 5,000 requests/hour for authenticated requests. A single worker makes a few dozen API calls. If you're hitting rate limits, you're likely running many workers in parallel. Reduce `batch_size` or stagger dispatching.

### Can I use sipag with GitHub Enterprise?

sipag uses `gh` CLI under the hood, which supports GitHub Enterprise. Set `GH_HOST` in your environment:

```bash
export GH_HOST=github.example.com
```

---

## CLAUDE.md

### Do I need a CLAUDE.md?

No, but it helps a lot. Without it, Claude has to infer conventions from the code. With it, Claude works much more consistently with your project's patterns. Even a minimal CLAUDE.md with the test command is worth adding.

### Where should CLAUDE.md live?

Either location works:

```
CLAUDE.md            # repo root (most common)
.claude/CLAUDE.md    # alternative
```

Claude Code reads both.

### Can I have CLAUDE.md in subdirectories?

Yes — Claude Code reads `CLAUDE.md` files in parent directories up the tree. This lets you have repo-level conventions in the root and package-specific overrides in subdirectories.

---

## Lifecycle hooks

### How do I get notified when a worker finishes?

Create an executable script at `~/.sipag/hooks/on-worker-completed`:

**macOS desktop notification:**
```bash
#!/usr/bin/env bash
osascript -e "display notification \"PR #${SIPAG_PR_NUM} opened for #${SIPAG_ISSUE}\" with title \"sipag\""
```

**Slack (via webhook):**
```bash
#!/usr/bin/env bash
curl -s -X POST "$SLACK_WEBHOOK_URL" \
  -H 'Content-type: application/json' \
  --data "{\"text\": \"sipag: PR opened for #${SIPAG_ISSUE} in ${SIPAG_REPO}: ${SIPAG_PR_URL}\"}"
```

Make the script executable: `chmod +x ~/.sipag/hooks/on-worker-completed`

### My hook isn't firing

Check that:

1. The file is in `~/.sipag/hooks/` (not a subdirectory)
2. The file is executable: `ls -la ~/.sipag/hooks/`
3. The file name matches exactly: `on-worker-completed` (no extension)

Hooks run asynchronously and silently — errors don't bubble up to sipag output. Test your hook manually:

```bash
SIPAG_EVENT=worker.completed SIPAG_REPO=test/repo SIPAG_ISSUE=1 \
SIPAG_PR_NUM=2 SIPAG_PR_URL=https://github.com/test/repo/pull/2 \
~/.sipag/hooks/on-worker-completed
```

---

## Security

### Is it safe to run `--dangerously-skip-permissions`?

Inside the Docker container, yes. The container has no access to your host filesystem, SSH keys, or other credentials. Docker's isolation is the safety boundary. The flag removes approval dialogs inside the container — Claude can run arbitrary code there, but that code can only affect what's inside the container.

### What credentials does the worker container receive?

Only `ANTHROPIC_API_KEY` and `GH_TOKEN`. Nothing else from your environment is passed through. The container doesn't get your SSH keys, AWS credentials, or any other secrets.

### Can workers access other repos?

Only if `GH_TOKEN` has access to them. The token is scoped to whatever you set it to — typically just the repo being worked on.

---

[Getting Started →](getting-started.md){ .md-button .md-button--primary }
[Configuration →](configuration.md){ .md-button }
