# Concepts

sipag turns a PR description into a pull request that implements it. You write the assignment, dispatch a worker, and review the result. sipag handles everything between "this needs to happen" and "here's a PR ready for review."

This document explains the ideas behind how sipag works, without getting into code or implementation details.

---

## You decide, workers execute

sipag draws a clear line between decisions and labor.

**Your side of the line:**

- What matters right now
- Which issues to prioritize
- What goes into the PR description (the assignment)
- Whether a PR is good enough to merge

**sipag's side of the line:**

- Installing review tooling into your project
- Launching workers in isolated containers
- Tracking worker state and liveness
- Recording what went wrong so the next attempt is better

This isn't autocomplete for code. It's an execution layer that takes direction and delivers pull requests.

---

## The container is the safety boundary

Workers run inside Docker containers with full Claude Code permissions. Claude can read files, write code, run tests, commit, and push — all without approval dialogs. This sounds dangerous, and it would be on your host machine.

But the container is disposable. It starts from a clean image, clones the repo fresh, works on a branch, and pushes to that branch. It can't touch your local files, can't access other repos, can't install packages on your machine. If something goes wrong, you close the PR and the damage is zero.

This isolation is what makes autonomous execution practical. The permission boundary isn't "Claude can do X but not Y." It's "Claude can do anything, but only inside this throwaway box."

---

## The PR is the contract

The PR description is the complete assignment for a worker. It contains:

- Which issues it addresses
- The architectural context (what modules are involved, what patterns to follow)
- The implementation approach
- Constraints (what not to change, what to be careful about)

The worker reads this description and executes it. It doesn't browse the backlog or pick its own issues. The person creating the PR has already done the analysis; the worker's job is to implement the plan.

This separation means the analysis quality directly determines the implementation quality. A vague PR description produces vague work. A precise architectural brief produces focused, well-scoped changes.

---

## Finishing beats starting

sipag enforces a work-in-progress limit. By default, at most 3 active workers can run at once. If you're at the limit, `sipag dispatch` refuses to start new work — it waits for existing workers to finish.

This is deliberate. Starting new work feels productive but increases cycle time. Finishing existing work reduces it. A codebase with 3 open PRs and 10 pending issues is healthier than one with 10 open PRs and 3 pending issues, because the 10-PR codebase has more unresolved merge conflicts, more stale branches, and more context for reviewers to hold in their heads.

The limit also creates natural checkpoints. When dispatch pauses because it's at capacity, that's your cue to review what's ready.

---

## Workers learn from each other

When a worker fails, sipag extracts the reason from the logs and records it as a lesson. The next worker for the same repo reads all previous lessons before starting work.

This creates a feedback loop:

1. Worker A fails because the test suite requires a specific env var
2. sipag records: "test suite requires DATABASE_URL to be set"
3. Worker B reads this lesson and sets the env var before running tests
4. Worker B succeeds

Lessons accumulate over time. A repo that has been through several dispatch cycles has a growing body of institutional knowledge — not in anyone's head, but in a file that every future worker reads automatically.

---

## The choreography pattern

sipag's components don't call each other directly. They communicate through files on disk.

- Workers write state files and heartbeats to `~/.sipag/workers/`
- Workers emit lifecycle events to `~/.sipag/events/`
- The TUI and CLI read these directories to display status

This means:

- **Adding a Slack notification** = write a script that watches `~/.sipag/events/`. Zero changes to sipag.
- **Adding a dashboard** = read `~/.sipag/workers/`. Zero changes to sipag.
- **Debugging** = `ls -lt ~/.sipag/events/ | head`. Every state change is a file you can read.

No message queues, no databases, no services to run. The filesystem is the event bus. Files are the API.

---

## Where sipag fits

sipag is the execution layer in the dorky robot stack:

```
kubo (think)  -->  sipag (do)  -->  GitHub PRs (review)
                      ^
tao (decide)  -------/
```

- **kubo** handles chain-of-thought planning and long-term reasoning
- **sipag** turns plans into pull requests through autonomous workers
- **tao** surfaces suspended decisions that need human input

Each tool is independent. sipag works fine on its own — you don't need kubo or tao. But together they form a system where thinking, doing, and deciding are separated and can happen at different cadences.

---

## What sipag is not

**Not a CI/CD pipeline.** sipag creates PRs. Your existing CI runs on those PRs. sipag doesn't replace your build system, test suite, or deployment pipeline.

**Not a code generator.** sipag doesn't template code or scaffold projects. It installs review tooling and launches workers that use Claude Code's full reasoning capabilities.

**Not a chatbot.** sipag is infrastructure — containers, state files, lifecycle tracking. Claude Code provides the intelligence.

**Not magic.** Workers can fail. PRs can be wrong. You're the final reviewer. sipag reduces the labor between "this needs to happen" and "here's a PR," but it doesn't remove your judgment from the process.
