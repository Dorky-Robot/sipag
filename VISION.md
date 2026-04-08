# sipag — Product Vision

## One-liner

A board you can dispatch from. Tasks become work running in real terminals.

## What sipag is

sipag is the dispatcher in the Dorky Robot stack. It owns the project board —
tasks, statuses, roles — and ships work to long-running terminal sessions
managed by [katulong](https://github.com/Dorky-Robot/katulong).

Two commands for humans:

1. **`sipag dispatch <task_id>`** — Sends a task to its role's katulong
   session and moves it to `in-progress`.
2. **`sipag tui`** — Live kanban board across all configured projects.

Everything else (`sipag add`, `sipag move`, `sipag list`, `sipag projects`,
`sipag up`) is for managing the board from the command line.

Project-aware review agents and slash commands are scaffolded by
[hulma](https://github.com/Dorky-Robot/hulma), a separate tool extracted from
sipag in April 2026. The legacy v2/v3 Docker dispatch path was deleted in
April 2026 — sipag no longer launches containers itself.

## The philosophy

**The board is the contract.** A task is a small TOML file with a title and a
role. Roles are templates that say "when you dispatch a task with this role,
run this command in that katulong session." Everything else is bookkeeping.

**File-based state.** All board data lives under `~/.sipag/projects/<name>/`
as plain TOML. Git, sync tools, scripts, and other agents can read and write
the same files. There is no daemon, no database, no lock service.

**One job: route work to crews.** sipag does not run code, manage containers,
or talk to GitHub directly. It tells katulong what to do next, and katulong
runs the actual session. The two tools compose; either can be replaced.

**Finishing beats starting.** Statuses are intentionally ordered. Moving a
card forward should feel cheap; adding new ones should be deliberate. The TUI
makes it easy to see how much is in flight.

## sipag in the stack

```
kubo (think)  →  sipag (board)  →  katulong (sessions)  →  agents do the work
```

- **kubo** — chain-of-thought reasoning, breaks problems into steps
- **sipag** — board + dispatcher, turns backlog into in-flight work
- **katulong** — long-running terminal sessions for agents
- **hulma** — scaffolds review agents and slash commands into a project

Each tool is independent. sipag works fine on its own; its only runtime
dependency is a reachable katulong server (configured at
`~/.katulong/remote.json`).

## What sipag is not

- **Not a CI/CD pipeline.** sipag dispatches work; your existing CI still runs.
- **Not a code generator.** It launches sessions that use Claude Code (or
  whatever the role's command points at).
- **Not a sandboxer.** Isolation is katulong's job (or the agent's, or the
  worktree's). sipag does not run containers.
- **Not a chatbot.** sipag is plumbing — TOML files, an HTTP client, a TUI.
- **Not autonomous.** A human curates the board, reviews the output, and
  decides what to merge.
