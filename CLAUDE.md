# CLAUDE.md — sipag

This file primes Claude Code sessions working **on sipag itself**.

## Project overview

sipag is a board-driven work dispatcher for Claude Code crews. It owns the
project board (tasks, statuses, roles) and ships work to running terminal
sessions managed by [katulong](https://github.com/Dorky-Robot/katulong).

1. **`sipag dispatch <task_id>`** — Sends a task from the board to the role's
   katulong session and moves it to `in-progress`.
2. **`sipag tui`** — Live kanban board across all configured projects.

Project-aware review agents and slash commands are scaffolded by
[hulma](https://github.com/Dorky-Robot/hulma), a separate tool that was
extracted from sipag in April 2026. The legacy v2/v3 Docker dispatch path was
deleted in April 2026 — sipag no longer launches containers itself.

## Architecture

### Rust workspace (3 crates)

```
sipag-core/src/
├── lib.rs              # pub mod: board, config, katulong
├── config.rs           # default_sipag_dir() — resolves SIPAG_DIR / ~/.sipag
├── katulong.rs         # HTTP client for katulong crew API + command builders
└── board/
    ├── mod.rs           # BoardConfig, project listing, add/move helpers
    ├── project.rs       # Project struct + project.toml
    ├── task.rs          # Task struct + task TOML files
    └── role.rs          # Role struct + role.toml templates

sipag/src/
├── main.rs             # Entry point
└── cli.rs              # CLI subcommands: dispatch, up, tui, add, list, move,
                        # projects, project, sub, version

tui/src/
├── main.rs             # Terminal setup, event loop
├── board_app.rs        # BoardApp state (columns, selection, key handling)
└── ui/board.rs         # Kanban column rendering
```

### State model

Everything sipag knows lives under `~/.sipag/` as TOML:

```
~/.sipag/
├── config.toml                        # default_project, etc.
└── projects/
    └── <project>/
        ├── project.toml               # name, repo, statuses
        ├── tasks/
        │   └── <id>.toml              # id, title, status, role, labels
        └── roles/
            └── <role>.toml            # name, command, worktree
```

A task is just a small TOML file. A role is a template that says "when you
dispatch a task tagged with this role, run this command in that katulong
session."

## Commands

```
sipag dispatch <TASK_ID>    Dispatch a task to its role's katulong session
sipag up [project]          Spin up sessions for every role in the project
sipag tui                   Launch the kanban TUI (default when run with no args)
sipag add <title>           Add a task to the board
sipag list                  List tasks on the board
sipag move <id> <status>    Move a task to a new status
sipag projects              List all projects
sipag project add <name>    Register a project
sipag sub <topic>           Subscribe to a katulong pub/sub topic
sipag version               Print version
```

## Dependencies on katulong

sipag dispatches by talking to a katulong server over HTTP. The connection
details live at `~/.katulong/remote.json`:

```json
{ "url": "https://katulong.example", "apiKey": "..." }
```

`sipag-core/src/katulong.rs` is the only place that knows the wire format —
keep API calls funneled through it.

## Conventions

### Rust code

- `make dev` — full local validation: `cargo fmt` + `cargo clippy -D warnings` + `cargo test`
- `make build` — release build
- `make install` — `cargo install --path sipag`

### Quality gates — git hooks

Hooks are the sole quality gate. Code that gets pushed is already validated.

**Pre-commit** (~1 min): gitleaks secrets scan, typos spell check, cargo deny CVE
check, **cargo build --release**, cargo fmt, cargo clippy, shellcheck.

**Pre-push** (~2-3 min): cargo test --workspace (blocking), cargo machete (warning),
gitleaks final scan (blocking).

Install once after cloning:

```bash
make install-hooks
```

- **Never use `--no-verify`**. Fix the issue instead.
- Run `make dev` before opening or updating PRs.

## Working on sipag

### What changes most

- `sipag-core/src/board/` — task/project/role schema and helpers
- `sipag-core/src/katulong.rs` — katulong client + command builders
- `sipag/src/cli.rs` — CLI surface
- `tui/src/board_app.rs`, `tui/src/ui/board.rs` — kanban view

### Part of the dorky robot stack

```
kubo (think)  →  sipag (board)  →  katulong (sessions)  →  agents do the work
```

sipag is the dispatcher. kubo handles chain-of-thought planning; katulong
hosts the long-running terminal sessions where agents run.
