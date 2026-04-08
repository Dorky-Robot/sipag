# Your First sipag Session

This guide walks through a complete cycle: configure a project, create a PR, dispatch a worker, and review the result.

---

## Prerequisites

Before starting, make sure you have:

- Docker Desktop running
- GitHub CLI authenticated (`gh auth login`)
- Claude Code CLI installed (`npm install -g @anthropic-ai/claude-code`)
- sipag installed (`brew install sipag` or [other methods](../getting-started.md#1-install-sipag))
- A GitHub repo you have write access to

Run `sipag doctor` to verify everything is ready.

---

## 1. (Optional) Scaffold review agents with hulma

Project-aware review agents and slash commands are scaffolded by [hulma](https://github.com/Dorky-Robot/hulma), a separate tool. If you want them, install hulma and run:

```bash
cd ~/Projects/my-app
hulma configure
```

This is optional — sipag dispatch works without any `.claude/` setup. Skip ahead if you don't need agents.

---

## 2. Create a PR

Create a branch and PR on GitHub. The PR body is the complete assignment for the worker — be specific about what needs to happen:

```bash
git checkout -b sipag/add-input-validation
git push -u origin sipag/add-input-validation

gh pr create --title "Add input validation to user registration" --body "$(cat <<'EOF'
## Assignment

Add server-side input validation to the user registration endpoint.

Closes #15

## Context

The registration endpoint at `src/routes/auth.rs` currently accepts any input
without validation. We need to validate email format, password strength
(minimum 8 characters, at least one number), and username length (3-30 chars).

## Constraints

- Use the existing `validator` crate already in Cargo.toml
- Return 422 with structured error messages, not 400
- Existing tests in `tests/auth_test.rs` must continue to pass
- Add tests for each validation rule
EOF
)"
```

The more context you provide in the PR body, the better the worker performs. Include which files are involved, what patterns to follow, and what "done" looks like.

---

## 3. Dispatch a worker

```bash
sipag dispatch https://github.com/owner/my-app/pull/16
```

sipag runs preflight checks (Docker daemon, image, auth), verifies back-pressure limits, and launches a Docker container. You'll see output confirming the container started.

---

## 4. Monitor progress

Open a separate terminal and launch the TUI:

```bash
sipag
```

You'll see your worker in the task list. It moves through phases:

- **starting** (yellow) — container is booting, cloning the repo
- **working** (cyan) — Claude is implementing the changes
- **finished** (green) — work is done, commits pushed
- **failed** (red) — something went wrong

Press `Enter` on a worker to see its detail view with metadata and log output. Press `a` to attach to the container's shell if you want to watch Claude work in real time.

You can also check from the CLI:

```bash
sipag ps        # Quick status check
sipag logs 16   # View worker output
```

---

## 5. Review the result

When the worker finishes, go to GitHub and review the PR. The worker will have:

- Pushed commits to the PR branch
- Run review agents (security, architecture, correctness)
- Updated the PR description with what was actually done

Review the diff, check that tests pass in CI, and merge or close.

---

## What if it fails?

Check the logs:

```bash
sipag logs 16
```

Common failure reasons:

- **No commits pushed** — Claude ran but didn't produce any changes. The PR description may need more detail.
- **Auth failure** — Token expired or missing. Re-run `gh auth login` or refresh your Claude token.
- **Timeout** — Worker exceeded the 2-hour default. Consider increasing `timeout` in `~/.sipag/config`.
- **Test failures** — The existing test suite failed. Check if the repo needs environment setup the worker doesn't know about.

Failed workers record lessons automatically. The next worker for the same repo reads these lessons and avoids repeating the same mistake.

---

## Next steps

- [Setting up a new repo](new-repo-setup.md) — preparing a repo for sipag dispatch
- [Configuration](../configuration.md) — tuning timeouts, back-pressure limits, and Docker image
- [How it works](../how-it-works.md) — understanding the full worker lifecycle
