# Architecture: sipag and Claude Code

sipag is pure scaffolding. Claude Code is the intelligence. This document explains the boundary and the choreography pattern that ties them together.

## sipag = infrastructure

sipag handles everything that isn't thinking:

- **Issues**: Fetches open issues from GitHub, filters by label, structures them into prompts
- **Containers**: Spins up Docker containers with the right credentials, mounts, and environment
- **Lifecycle**: Tracks worker state via heartbeat files and state files
- **Back-pressure**: Pauses dispatch when too many PRs are open, resumes when they're merged
- **Prompt construction**: Assembles all context (issues, branch, PR template) into a structured prompt

sipag never decides *what* to do. It decides *when* to dispatch and *what context to provide*.

## Claude Code = intelligence

Claude Code runs inside the Docker container. It receives a structured prompt and decides:

- Which issues to address in this PR (and which to defer)
- What the root cause is across multiple issues
- How to implement the fix
- What tests to write
- How to structure the commit and PR description

The worker prompt (`lib/prompts/worker.md`) encodes a **procedure** — not just instructions, but a specific analysis framework:

1. **Three perspectives**: Architectural coherence, practical delivery, risk/dependency
2. **Synthesis**: Resolve conflicts between perspectives into one plan
3. **Implementation**: Execute the plan with Boy Scout Rule improvements
4. **Validation**: Run `make dev`, update PR description with structured references

This procedure ensures Claude doesn't just jump to the first issue and start coding. It forces systematic analysis before implementation.

## Choreography, not orchestration

sipag uses a **choreography** pattern: components communicate through files on disk, not RPC or function calls. `~/.sipag/` is the event bus.

**Writers don't know who reads. Readers don't know who writes.**

This decoupling means:
- Adding a Slack notifier = watch `events/`, zero producer changes
- Adding a tao integration = watch `events/` + `workers/`, zero producer changes
- Debugging = `ls -lt ~/.sipag/events/ | head`

### Heartbeat files

Workers write a `.heartbeat` file alongside their state file every 30 seconds:

```
~/.sipag/workers/
├── owner--repo--pr-42.json        # state (phase, repo, PR, etc.)
├── owner--repo--pr-42.heartbeat   # liveness signal (mtime-based)
```

The heartbeat file's **mtime** is the primary liveness signal. Contents are JSON for debugging:

```json
{"repo":"owner/repo","pr_num":42,"timestamp":"2026-02-23T10:30:00Z","pid":1}
```

Why a separate file (not updating the state JSON): two writers (heartbeat thread + main thread phase transitions) on the same atomic-write file creates a race. A separate `.heartbeat` file is lock-free and safe.

### Liveness detection

`scan_workers()` uses a three-tier approach:

1. **Heartbeat file** (fast path) — one `stat()` call per worker, no subprocess
2. **Grace period** — workers started less than 60s ago are assumed alive
3. **Docker ps** (fallback) — backward compat for old workers without heartbeat files

This replaces the previous approach of shelling out to `docker ps` for every non-terminal worker, which was orders of magnitude slower.

### Event files

Workers emit lifecycle events to `~/.sipag/events/` at phase transitions:

- `worker-started` — entered working phase
- `worker-finished` — completed successfully
- `worker-failed` — exited with error
- `worker-orphaned` — detected dead by the host (stale heartbeat or missing container)

Filenames are chronologically sortable: `{ISO8601}-{event_type}-{repo_slug}.md`.

External consumers (tao, Slack hooks, monitoring scripts) watch this directory. Adding a new consumer requires zero changes to sipag itself.

### State files as fallback

State files (`*.json`) remain the authoritative source of truth. Heartbeats and events are supplementary signals. If events are missed (e.g., filesystem watcher lag), the state files tell you exactly what happened.

## Why sipag doesn't shell out to `claude`

sipag runs inside a Claude Code session (`sipag work` is typically invoked by Claude Code itself). Shelling out to `claude --print` from within a Claude Code session causes nesting errors. More fundamentally, analysis belongs in the prompt — it's Claude Code's job, not sipag's.

The worker prompt carries the full analysis procedure. Claude Code follows it because the instructions are explicit, step-by-step, and structured with clear outputs expected at each stage.

## Sub-agents

The `.claude/agents/` directory contains specialized agents for the **host** Claude Code session (the one running `sipag work`):

| Agent | Purpose |
|-------|---------|
| `issue-analyst` | Pre-dispatch analysis: cluster issues, evaluate from 3 perspectives, recommend highest bang-for-buck PR |
| `backlog-triager` | Evaluate issues against VISION.md, recommend CLOSE/ADJUST/KEEP/MERGE |
| `architecture-reviewer` | Review PRs for crate boundary violations, config resolution order |
| `security-reviewer` | STRIDE threat modeling, OWASP checks, token handling |
| `correctness-reviewer` | Worker lifecycle edge cases, race conditions, state machine transitions |

These agents are **advisory** — they guide analysis but don't make changes. The host session invokes them via Claude Code's Task tool when it needs structured analysis.

## Data flow

```
GitHub Issues
     |
     v
sipag (Rust)              -- fetch issues, structure prompt, spin up container
     |
     v
Worker Prompt             -- carries the full analysis procedure
     |
     v
Claude Code (container)   -- analyzes issues, implements, tests, opens PR
     |                    -- writes heartbeats + events to ~/.sipag/
     v
GitHub PR                 -- ready for review
```

The Rust code's job is to produce the best possible prompt — all the right context, structured for Claude to follow the procedure naturally.
