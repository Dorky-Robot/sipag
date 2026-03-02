# FAQ

## What happens when a worker fails?

sipag marks the worker state as `failed`, records the exit code and error message, and emits a `worker-failed` lifecycle event. The failure reason is extracted from the worker log and appended to `~/.sipag/lessons/{owner}--{repo}.md`. The next worker dispatched for the same repo reads these lessons and includes them in its prompt, so it can avoid repeating the same mistake.

You can view the failure details with `sipag logs <PR_NUMBER>` or in the TUI detail view.

## Can I run multiple workers at once?

Yes. sipag supports concurrent workers up to the `max_open_prs` limit (default 3). Each worker runs in its own Docker container and operates independently. Dispatch additional workers by running `sipag dispatch` for different PRs.

Workers on different repos are fully independent. Workers on the same repo should target different PRs to avoid merge conflicts.

## How do I use a custom Docker image?

Build your image locally and point sipag at it:

```bash
docker build -t sipag-worker:local .
SIPAG_IMAGE=sipag-worker:local sipag dispatch https://github.com/owner/repo/pull/42
```

Or set it permanently in `~/.sipag/config`:

```
image=sipag-worker:local
```

## How do I debug a running worker?

Use the TUI to attach to a worker's container shell:

1. Run `sipag` (or `sipag tui`)
2. Select the worker with `j`/`k`
3. Press `a` to attach

You can also view live logs:

```bash
sipag logs <PR_NUMBER>
```

Or attach directly via Docker:

```bash
docker exec -it sipag-owner--repo-pr-42 bash
```

## What does the worker do inside the container?

The worker:

1. Clones the repo and checks out the PR branch
2. Reads the PR body as its complete assignment
3. Reads lessons from past failures for this repo
4. Runs Claude Code with `--dangerously-skip-permissions`
5. Claude resolves merge conflicts, addresses review feedback, implements the work, and pushes commits
6. A supervision loop monitors Claude, writes heartbeats, and checks PR state on GitHub
7. After Claude exits, the worker verifies commits were actually pushed

## What does `sipag configure` actually do?

By default, it launches Claude Code to analyze your project (directory listing, config files, README, existing CLAUDE.md) and generate tailored review agents and commands written specifically for your codebase. The agents reference your actual file paths, tech stack, and patterns.

With `--static`, it installs generic templates without running Claude — useful when you don't have Claude available or want a quick baseline.

Both modes write to `.claude/agents/` and `.claude/commands/` and install git hooks to `.husky/`.

## Do I need Docker?

Yes. Docker is required for `sipag dispatch`. The container is the safety boundary that allows Claude Code to run with full permissions without risking your host machine.

`sipag configure` and the TUI do not require Docker.

## What permissions does the GitHub token need?

The token needs write access to the target repository: read/write on contents (to push commits) and pull requests (to update PR state). If you use `gh auth login`, the default scopes are sufficient.

## Can workers merge PRs?

Yes. The worker prompt instructs Claude to run review agents and, if all reviews pass, merge the PR with `gh pr merge --squash --delete-branch`. You can control this behavior by editing the worker instructions in the PR body or by adjusting your project's `.claude/commands/` templates.

## How do lessons work?

Lessons are per-repo markdown files stored at `~/.sipag/lessons/{owner}--{repo}.md`. When a worker fails, sipag extracts the failure pattern from the log and appends it. Future workers for the same repo read the last 8KB of lessons (truncated at section boundaries to preserve recent entries) and include them in their prompt.

This creates a feedback loop where workers learn from past failures without human intervention.

## How do I reset lessons for a repo?

Delete or edit the lessons file directly:

```bash
rm ~/.sipag/lessons/owner--repo.md
```

## What's the difference between `sipag ps` and `sipag tui`?

`sipag ps` is a one-shot command that prints worker status to stdout — suitable for scripts and quick checks. `sipag tui` (or bare `sipag`) is an interactive terminal UI with live updates, detail views, and worker management (attach, kill, dismiss).

## Can I use sipag without the dorky robot stack?

Yes. sipag works standalone. kubo and tao are optional components that add planning and decision tracking, but sipag has no dependency on them.
