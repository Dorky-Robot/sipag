# sipag — Product Vision

## One-liner

Queue up backlog items, go to sleep, wake up to pull requests.

## What sipag is

sipag is a slow, relentless gardener for codebases. It generates project-aware review agents, ships work through isolated Docker containers, and learns from failures — all powered by Claude Code.

Three commands for humans:

1. **`sipag configure`** — Analyzes your project and generates tailored review agents and commands for `.claude/`. Re-run as your project evolves.
2. **`sipag dispatch`** — Launches an isolated Docker container that reads a PR description and implements it autonomously.
3. **`sipag tui`** — Live dashboard for all workers across the host.

Everything else (`sipag ps`, `sipag logs`, `sipag kill`, `sipag doctor`) is for managing workers from the command line.

## The philosophy

**Find the disease, not the symptoms.** Three issues about different error messages probably mean there's no unified error handling. sipag's job — through its analysis agents and the Claude Code session crafting PRs — is to see these patterns and fix the root cause.

**The container is the safety boundary.** Docker replaces the approval dialog. Inside the container, Claude has full autonomy. Outside, nothing is touched. The permission boundary isn't "Claude can do X but not Y." It's "Claude can do anything, but only inside this throwaway box."

**The PR is the contract.** The PR description is the complete assignment for a worker. The person creating the PR has done the analysis; the worker's job is to implement the plan. Analysis quality determines implementation quality.

**Finishing beats starting.** sipag enforces a work-in-progress limit (default 3 active workers). Starting new work feels productive but increases cycle time. Finishing existing work reduces it.

**Workers learn from each other.** When a worker fails, sipag records the reason. The next worker for the same repo reads all previous lessons before starting. This creates a feedback loop where workers improve without human intervention.

**File-based choreography.** Components communicate through files on disk, not RPC or function calls. `~/.sipag/` is the event bus. Writers don't know who reads. Readers don't know who writes. Adding a Slack notifier or monitoring dashboard requires zero changes to sipag.

## sipag is infrastructure, Claude Code is intelligence

This is the core architectural boundary.

**sipag** (the Rust binary) handles everything that isn't thinking: containers, state tracking, heartbeats, back-pressure, lifecycle events.

**Claude Code** (on the host) handles the intelligence: disease analysis, decision-making, PR crafting, review.

**Claude Code** (in Docker) handles implementation: writing code, running tests, committing, pushing.

sipag never decides *what* to do. It decides *when* to dispatch and *what context to provide*.

## Part of the dorky robot stack

```
kubo (think)  →  sipag (do)  →  GitHub PRs (review)
                    ↑
tao (decide)  ─────┘
```

- **kubo** — chain-of-thought reasoning, breaks problems into steps
- **tao** — decision ledger, surfaces suspended actions
- **sipag** — autonomous executor, turns backlog into PRs

Each tool is independent. sipag works fine on its own.

## What sipag is not

- **Not a CI/CD pipeline.** sipag creates PRs. Your existing CI runs on those PRs.
- **Not a code generator.** It installs review tooling and launches workers that use Claude Code's full reasoning capabilities.
- **Not a chatbot.** sipag is infrastructure — containers, state files, lifecycle tracking.
- **Not autonomous.** A human starts it, reviews the PRs, and decides what to merge.
- **Not fast.** It's relentless. One careful cycle at a time, every cycle leaving the project measurably better.
