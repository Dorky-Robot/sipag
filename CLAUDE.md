# CLAUDE.md — sipag

This file primes Claude Code sessions working **on sipag itself**.

## Strategic frame

Per [`VISION.md`](VISION.md), sipag is **the OKR layer for an agentic fleet** — not a kanban tool, not a task tracker. Humans steer via **Objectives + Key Results + Standing + Ideas**; agents handle execution underneath. Tasks exist in the data model but live *below* the human surface as agent-managed scheduling units.

Two principles drive every architectural choice:

- **Strict layer coupling.** `Claude → katulong → sipag`. Sipag never reaches past katulong to talk to Claude directly. The bridge between Claude's free-form output and sipag's structured world is **gemma4 on sipag's side**. See memory `feedback-strict-layer-coupling`.
- **Empirical, not procedural.** Spike → observe → derive. Claude is the iterator; sipag observes and records. State machines were the wrong frame and were retired this session. See memory `project-sipag-work-model-experimentation`.

For current architecture, see [`docs/modules.md`](docs/modules.md). For capability-level state, see [`docs/feature-matrix.md`](docs/feature-matrix.md).

## Project overview

The two visible commands today:

1. **`sipag dispatch <task_id>`** — Sends a task from the board to its role's katulong session and moves it to `in-progress`.
2. **`sipag tui`** — Interactive board view across all configured projects.

Beyond that, sipag is becoming an **OKR + Experimentation surface**: humans write Objectives and KRs; lens-workers (gemma-driven, configured by each Steering entry's text) derive insights into a local vector corpus; KR sidebars surface what's been observed. Phase 1 #3 in `docs/modules.md` §9 is the next major work item.

Project-aware review agents and slash commands are scaffolded by [hulma](https://github.com/Dorky-Robot/hulma), a separate tool extracted from sipag in April 2026. The legacy v2/v3 Docker dispatch path was deleted in April 2026 — sipag no longer launches containers itself.

## Architecture

### Rust workspace (4 crates)

```
sipag-core/src/                # Library — domain logic + auth + LLM client + pub/sub
├── lib.rs
├── auth/                      # webauthn, passkeys, sessions, devices, tokens
├── board/                     # Objective, KeyResult, Task, Role, Project, Observation
├── config.rs                  # default_sipag_dir() — resolves SIPAG_DIR / ~/.sipag
├── feature.rs                 # ⛔ deprecated (kanban refinement pipeline; PR #536)
├── refine.rs                  # ⛔ deprecated (companion to feature.rs)
├── gate.rs                    # early lens-worker prototype (pre-dispatch classifier)
├── nudge.rs                   # early lens-worker prototype (post-dispatch observer)
├── hosts.rs                   # multi-host mesh config
│                              # (no katulong.rs file — `lib.rs:30` does
│                              # `pub use katulong_client as katulong;` as
│                              # a back-compat shim for the extracted crate)
├── llm.rs                     # ollama HTTP client — gemma4 lives here
└── pubsub.rs                  # file-backed durable broker (load-bearing; sipag-internal)

katulong-client/src/           # Wire client — extracted into its own crate via PR #535
├── http.rs                    # KatulongClient: REST surface
├── attach.rs                  # KatulongAttachClient: WS attach (full duplex)
├── protocol.rs                # Inbound/Outbound wire types
├── serve.rs                   # `katulong-client serve` notebook UI
└── lib.rs

sipag/src/                     # Binary — CLI + web server
├── main.rs
├── cli.rs                     # CLI subcommands: dispatch, up, tui, add, list, move,
│                              # projects, project, sub, serve, version
│                              # (feature/refine were removed in PR #536)
└── serve/                     # axum + maud htmx-based web UI

tui/src/                       # Binary — interactive board (ratatui)
├── main.rs
├── board_app.rs
└── ui/board.rs
```

### Bounded contexts (DDD frame, per modules.md)

| Context | Side | What it owns |
|---|---|---|
| **Steering** | human | Objectives + KRs + Standing + Ideas. Each entry is *also* a lens-worker prompt (dual role). |
| **Experimentation** | agent | Spike-observe-derive loop. Lens-workers + corpus + structural verbs (`observe` + `suggest_stance` + `ask_human` + `propose_task`). |
| **Topology** | platform | Mesh / wire — katulong-client, ollama HTTP, hosts.toml. |
| **Identity** | platform | Auth — webauthn, passkeys, devices, sessions. |

### State model

Everything sipag knows lives under `~/.sipag/` as TOML/JSONL:

```
~/.sipag/
├── config.toml                        # default_project, etc.
├── hosts.toml                         # multi-host mesh registration
├── pubsub/                            # file-backed durable broker
│   └── <topic>/log.jsonl
├── objectives/                        # top-level Objectives (project-agnostic)
│   └── <id>/
│       ├── objective.toml             # Objective metadata
│       └── key-results/<NNN>.toml     # KRs attached to this Objective
└── projects/
    └── <project>/
        ├── project.toml               # name, repo, statuses, ProjectKind
        ├── key-results/<NNN>.toml     # KRs scoped to this project
        ├── tasks/<id>.toml            # Task (board-level work unit)
        └── roles/<role>.toml          # Role template (command + worktree)
```

Two parallel KR locations today (project-scoped vs Objective-scoped) — both are load-bearing per `sipag-core/src/board/key_result.rs:77` (project) and `:149` (objective).

A future addition (Phase 1 #3 in modules.md §9): a **local vector corpus** sibling to the TOML state, holding observations and derived insights tagged + timestamped + embedded via ollama.

## Commands

```
sipag dispatch <TASK_ID>    Dispatch a task to its role's katulong session
sipag up [project]          Spin up sessions for every role in the project
sipag tui                   Launch the interactive board (default when run with no args)
sipag add <title>           Add a task to the board
sipag list                  List tasks on the board
sipag move <id> <status>    Move a task to a new status
sipag projects              List all projects
sipag project add <name>    Register a project
sipag sub <topic>           Subscribe to a katulong pub/sub topic
sipag serve                 Run the web UI (default port 7100)
sipag version               Print version
```

**Web UI is the primary surface** for Steering capabilities (Objectives, KRs, etc.). CLI subcommands for Steering are deliberately deferred until the web UI is fully working — see memory `feedback-sipag-ui-first`.

## Dependencies on katulong

Sipag dispatches by talking to a katulong server over HTTP and WS. Connection details live at `~/.katulong/remote.json`:

```json
{ "url": "https://katulong.example", "apiKey": "..." }
```

The **`katulong-client` crate** is the only place that knows the wire format — keep all katulong I/O funneled through it. `sipag-core::katulong` is a thin re-export for back-compat.

Two upstream issues filed (Phase 1 #3 depends on the topics they add):

- [Dorky-Robot/katulong#715](https://github.com/Dorky-Robot/katulong/issues/715) — `sessions/<id>/lifecycle` topic
- [Dorky-Robot/katulong#716](https://github.com/Dorky-Robot/katulong/issues/716) — `sessions/<id>/child` topic

## Env vars

- `SIPAG_DIR` — overrides `~/.sipag` for board state.
- `SIPAG_DEV=1` — enables tower-livereload + filesystem watcher in `sipag serve`.
- `SIPAG_DISPATCH_V2=1` — routes `sipag serve` dispatches through the
  `KatulongAttachClient` (WS attach + explicit `wait_for` handshake) instead
  of the legacy `verify_and_heal_dispatch` keystroke loop. **Default OFF**
  — the legacy path is still the default until Phase 2 #11 in
  `docs/modules.md` §9 deletes `verify_and_heal_dispatch` outright. See
  `sipag/src/serve/htmx.rs::dispatch_v2_enabled` for the truthy-value
  semantics. Set this to `1` in production to ride the v2 attach path.

## Conventions

### Rust code

- `make dev` — full local validation: `cargo fmt` + `cargo clippy -D warnings` + `cargo test`
- `make build` — release build
- `make install` — `cargo install --path sipag`

### Quality gates — git hooks

Hooks are the sole quality gate. Code that gets pushed is already validated.

**Pre-commit** (~1 min): gitleaks secrets scan, typos spell check, cargo deny CVE check, **cargo build --release**, cargo fmt, cargo clippy, shellcheck.

**Pre-push** (~2-3 min): cargo test --workspace (blocking), cargo machete (warning), gitleaks final scan (blocking).

Install once after cloning:

```bash
make install-hooks
```

- **Never use `--no-verify`**. Fix the issue instead.
- Run `make dev` before opening or updating PRs.

## Working on sipag

### What changes most

- `sipag-core/src/board/` — Objective / KeyResult / Task / Role / Project schema
- `sipag-core/src/llm.rs` — gemma4 / ollama client (will export `LlmClient` trait per Phase 2 #8)
- `sipag-core/src/{gate,nudge}.rs` — early lens-worker prototypes; fold into the lens-worker abstraction in Phase 1 #3
- `sipag/src/serve/` — the web UI (htmx + maud), where Steering lives today
- `tui/src/board_app.rs` — interactive board
- `katulong-client/src/` — wire client; touch when adding HTTP/WS/SSE consumers

### Part of the dorky robot stack

```
kubo (think)  →  sipag (OKR + dispatch)  →  katulong (sessions)  →  agents
```

- **kubo** — chain-of-thought reasoning, decomposes problems
- **sipag** — the OKR layer; humans steer here, agents work below
- **katulong** — long-running terminal sessions for agents
- **hulma** — scaffolds review agents and slash commands into a project

Each tool composes; any can be replaced. Sipag's only runtime dependency is a reachable katulong server.

## Pointers for new sessions

- **Vision**: [`VISION.md`](VISION.md) — the strategic anchor; don't reframe operational changes into it.
- **Architecture**: [`docs/modules.md`](docs/modules.md) — DDD bounded contexts, lens-worker abstraction, phase queue.
- **Capability state**: [`docs/feature-matrix.md`](docs/feature-matrix.md) — per-capability ✅/🟡/🟧/🔴/⏳/🚫 with code locations.
- **Memories** (accumulated context across sessions): live under `~/.claude/projects/<encoded-cwd>/memory/`. **Naming convention**: in-repo references use hyphens (`feedback-strict-layer-coupling`); on-disk filenames use underscores (`feedback_strict_layer_coupling.md`). Most load-bearing today:
  - `feedback-strict-layer-coupling` — sipag never reaches past katulong; gemma is the bridge
  - `feedback-fix-at-right-layer` — when sipag would need a workaround, extend the upstream tool instead
  - `feedback-deprecate-with-rationale` — abandon modules by deprecating + recording why
  - `feedback-structural-language-before-behavior` — vocabulary changes ship before behavior changes
  - `feedback-sipag-ui-first` — web UI is the primary surface; CLI deferred
  - `project-sipag-work-model-experimentation` — spike-observe-derive, not refine-execute-verify
