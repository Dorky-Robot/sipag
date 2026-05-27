# sipag feature matrix

> *Companion to [`narrative.md`](narrative.md): that doc is the customer-facing product story; this is the per-capability scorecard tracking that story's delivery.*

**Status:** working draft, iterating collaboratively.
**Frame:** [`narrative.md`](narrative.md) → product story. [`../VISION.md`](../VISION.md) → strategic principles. [`modules.md`](modules.md) → architecture. **This doc** → what sipag actually offers (capability-level), where the gaps are, and what we're deliberately *not* building.

## How to use this doc

- One row per **user-facing or system-level capability** (not per file or module — that's `modules.md`'s job). A useful litmus test: if the row could be deleted without losing a user-observable capability, it belongs in modules.md. Module-level concerns leaking into this doc is the canonical doc-rot vector — keep the rubric tight.
- Status reflects the *capability* state, not the underlying code's quality. A feature can be ✅ shipped while its implementing module is 🔴 messy.
- Group by [bounded context](modules.md#1-the-context-map) so the human/agent line stays visible.
- Edits welcome. When status changes, leave a one-liner in §10 edit log.
- Cross-cutting: tombstones for retired modules.md concepts (e.g., `Trial`, `IteratePolicy`) stay here only when their *absence* affects user-visible capability — to mark "this was promised, withdrawn, here's the replacement." Pure module-shape decisions belong in modules.md.

## Surface priority (set 2026-05-17)

**Web UI first** (`sipag serve`, port 7100 — htmx + maud). CLI subcommands for Steering capabilities (Objective / KR / Stance / Idea) are **deliberately deferred** until the web UI is fully working end-to-end. CLI rows in this matrix that note "no CLI surface" are status ⏳, not 🔴 — they're parked, not missing. See memory `feedback-sipag-ui-first`.

The TUI (`sipag tui`) is a separate dispatch-side surface that pre-existed this priority; don't break it, don't necessarily extend it. Existing CLI commands (`dispatch`, `up`, `tui`, `add`, `list`, `move`, `projects`, `project`, `sub`, `serve`, `version`) stay supported.

## Status legend

| Symbol | Meaning |
|---|---|
| ✅ | shipped and working as intended |
| 🟡 | implemented but with known gaps or rough edges |
| 🟧 | partially implemented or scattered across the codebase |
| 🔴 | needed but missing or broken |
| ⏳ | planned, not yet started |
| ❓ | open question — capability needs design before implementation |
| ⛔ | explicitly deprecated; see [`feedback-deprecate-with-rationale`](../.claude/memory) |
| 🚫 | deliberately NOT in scope per VISION (see §9) |

---

## 1. Steering features (human surface)

The human's two questions per VISION: "what are we optimizing for" and "is it working." Everything else here is in service of those.

**Dual role (set 2026-05-17):** every Steering entry (Objective / KR / Standing / Idea / named pattern) is *also* a lens definition — its text serves as the system prompt of a corresponding `LensWorker` in Experimentation. Adding a KR spawns a lens-worker; editing the text changes the worker's behavior; retiring the entry retires the worker. The Steering UI doubles as the lens registry; no separate "lens configuration" surface needed. See `docs/modules.md` §3 and §6.

| Capability | Status | Where (code) | Notes |
|---|---|---|---|
| Create / view / close an **Objective** | 🟡 | `sipag-board/src/objective.rs` + `sipag/src/serve/board_view.rs` | Type exists; web UI exposes objectives. CLI ⏳ per UI-first priority. |
| Add / view / edit a **Key Result** under an Objective | 🟡 | `sipag-board/src/key_result.rs` + `sipag/src/serve/board.rs:237` | Type exists; web UI can add/edit. CLI ⏳. |
| Set **KrStance** (green / yellow / red / done) | 🟡 | `sipag-board/src/key_result.rs:19` (`KrStance` enum); `sipag/src/serve/board.rs:269` (HTTP toggle) | Stance is the scalar reward signal per VISION. Web UI works. CLI ⏳. |
| **Standing** — top-level surface for upkeep/firefights (work that doesn't ladder to an Objective) | 🟡 | `sipag-board/src/project.rs:17` (`ProjectKind::Standing`) | Exists as a project kind; web UI semantics still evolving. CLI ⏳. |
| **Idea box** — parking lot for unprocessed input | 🔴 | not modeled as a first-class type | VISION names this explicitly. No `Idea` aggregate. Captured ad-hoc as tasks today. **Web UI work** when this lands (Phase 1 #4 in modules.md). |
| **promote_idea → Experiment** (the ACL crossing into Experimentation) | 🔴 | n/a — depends on Idea + Experiment types | Per modules.md §6, this is the first-class cross-context ACL. Blocked on Phase 1 #3 + #4. |
| **Agent API surface** — read KRs, `report_stance` from agent | 🔴 | not implemented | VISION-planned: same `POST /tasks` and `PATCH /tasks/:id` endpoints exposed for agent loops. Phase 1 #5 + Phase 3 #14 in modules.md §9. |
| **KR-level summary across projects** (the "three objectives, six KRs, eleven tasks" view from VISION) | 🟡 | `sipag/src/serve/board_view.rs` | Exists in web UI. CLI ⏳. |
| **Accept / redirect agent-proposed work at the KR level** | 🔴 | no agent loop pushing proposals today | VISION's intended boundary. Comes alive when the Agent API + the gemma4 bridge's recording API (`suggest_stance`, `propose_task`) start emitting proposals the human surface can accept/redirect. |

---

## 2. Experimentation features (agent surface)

The spike → observe → derive loop per [`project-sipag-work-model-experimentation`](../.claude/memory). **Reframed 2026-05-17 (third pass — lens-worker abstraction)**: no state machine, no per-classification recording verbs. Instead: a local vector **corpus** + a registry of **lens-workers** (each Steering entry is a lens; plus project-meta and ad-hoc lenses) + four verbs (`observe` workhorse + 3 structural). The gemma bridge is just the first lens-worker. Claude is the iterator; sipag observes, derives, and surfaces. Sipag never reaches past katulong — gemma4 is the bridge. See `[[feedback-strict-layer-coupling]]`.

### Act — fire a spike

| Capability | Status | Where (code) | Notes |
|---|---|---|---|
| **Dispatch a task to a katulong session** | ✅ | `sipag-dispatch/src/lib.rs::dispatch` (callers: `sipag/src/cli.rs::run_dispatch_task` and `sipag/src/serve/htmx.rs::run_sipag_dispatch`) | One unified async function for both CLI and web paths. Closed Phase 2 #12 in PR #547. Body cap on every HTTP leg via `katulong_client::KatulongAsyncClient` (closed Phase 2 #6 / sipag #527 in PR #546). |
| **Auto-create worktree** for the dispatched task | ✅ | `sipag-dispatch/src/lib.rs::WorktreeSpec` (uses `katulong-client::worktree_command` helper) | Wired via role's `worktree = true`. Web UI v2 path used to skip this — fixed by Phase 2 #12 (PR #547). |
| **Generate unique dispatch session name** (`sipag-d-<hex>`) | ✅ | `katulong-client/src/http.rs:494` | Per-dispatch tile so the auto-summarizer can rename without breaking back-pointers. |
| **Build agent launch command** (`cd <wt> && <role-cmd> -p '…'`) with shell-quote-escape | ✅ | `katulong-client/src/http.rs:533` | Single-quote escape verified by 3 unit tests. Still used by the TUI; CLI + web paths went through `sipag-dispatch` instead and paste the prompt over WS attach. |
| **Spin up persistent role tiles** (`sipag up`) | ✅ | `sipag/src/cli.rs::run_up` | Project-level "warm the sessions" command. |
| ~~Trial lifecycle tracking~~ | 🚫 | n/a | Removed from roadmap 2026-05-17. State-machine framing was wrong (kanban-shaped despite the rename). The event log + recorded actions replace per-trial state tracking. See `[[feedback-strict-layer-coupling]]` and §3 reframe in modules.md. |

### Observe — collect the feedback signal

| Capability | Status | Where (code) | Notes |
|---|---|---|---|
| **Pre-dispatch state classification** ("is this pane ready?") | ✅ | `sipag/src/dispatch_gate.rs` (was `sipag-core/src/gate.rs` until §9 #9) | Folded into the lens-worker abstraction in §9 #9 — now calls gemma through `sipag_lens::ChatBackend` (bridge-backed). One-shot calling convention preserved; the fold is about the wire. |
| ~~Post-dispatch periodic observation~~ | ✅ | ~~`sipag-core/src/nudge.rs`~~ | Deleted in §9 #11 alongside `verify_and_heal_dispatch` (the only consumer). Event-driven observation via SSE remains the Phase 2 #10 target; the polling loop is gone. |
| **Event-driven observation** via katulong `claude/<uuid>` pub/sub | ✅ | `sipag/src/serve/bridge_worker.rs` + `katulong-client/src/sse.rs` | Bridge worker subscribes to `claude/<uuid>` SSE events, fires gemma on threshold, writes observations to corpus. Session discovery via observer poll (PR #565). Gains `sessions/<id>/*` topics for free when katulong#715/#716 land. |
| **Detect "session exited"** | 🔴 | not pushed by katulong today | Blocked on upstream [katulong#715](https://github.com/Dorky-Robot/katulong/issues/715). |
| **Detect "shell prompt returned"** (`hasChildProcesses` transitions) | 🔴 | not pushed by katulong today | Blocked on upstream [katulong#716](https://github.com/Dorky-Robot/katulong/issues/716). |
| **Detect "needs human"** (permission prompts) | 🟡 | implicit via gemma classification | Partial cover from `permission-request` events in `claude/<uuid>` topic. Will improve in Phase 2 #10. |
| **Detect "stuck"** (silence timeout) | 🔴 | not implemented | Subscriber-side derived signal (threshold is consumer policy); lands with Phase 2 #10. |
| ~~Typed `RecordedAction` value object~~ | ✅ | `sipag_lens::StructuralAction` (PR #551) | Reframed in 2026-05-17 §3 third pass: `RecordedAction` collapsed into `StructuralAction` (`Observe` + `SuggestStance` + `AskHuman` + `ProposeTask`). What gemma derives is typed + append-only into the corpus via `LensWorker::run_with_tools`. |
| **Cross-host observation** (subscribe to all mesh peers) | 🔴 | not implemented | Phase 2 #7 SSE subscriber needs N-stream fan-out. |
| ~~Recovery loop~~ (`verify_and_heal_dispatch` — LLM proposes keystrokes) | ✅ | ~~`sipag/src/serve/htmx.rs::verify_and_heal_dispatch`~~ | **Deleted in §9 #11** (closes sipag #528 by deletion). No more LLM-emitted bytes reach a PTY by design. The WS-attach path (`sipag_dispatch::dispatch` via `KatulongAttachClient::wait_for`) is now the only path. `SIPAG_DISPATCH_V2` env var, `build_launch_cmd` HTTP `/exec` pre-send, and `sipag-core/src/nudge.rs` all retired in the same PR. |

### Derive — what gemma4 lens-workers derive from the corpus

(Was "iterate" → "record" → now "derive." Claude is the iterator; sipag observes and derives. The full lens-worker abstraction sits here.)

| Capability | Status | Where (code) | Notes |
|---|---|---|---|
| **Corpus** (local vector DB, append-only forever, tagged + timestamped, sibling to diwa) | ✅ | `sipag-corpus` crate (PR #550) | Append-only JSONL at `~/.sipag/corpus/items.jsonl`. Cosine-similarity k-NN search with tag + timestamp + generation filters. Embedded via the ollama bridge through `BridgeEmbedder` (default model `nomic-embed-text`). |
| **`LensWorker` runtime** (lens text + `ModelChoice` + trigger policy + query strategy + write protocol) | ✅ | `sipag-lens` crate (PR #551) | `LensWorker::run` (one-shot) and `LensWorker::run_with_tools` (multi-turn with corpus tool calls). Composes prompt → calls `ChatBackend` → parses structured JSON → writes Observes to the corpus → returns the four structural verbs to the caller. |
| **Per-lens model selection** (`ModelChoice` + `Profile` + `~/.sipag/models.toml`) | ✅ | `sipag-lens::ModelResolver` (PR #551) | Decouples lens definitions from concrete model names. `Profile::{Fast, Strong, CodeAware}` resolved per-environment via `~/.sipag/models.toml`. Built-in defaults: Fast=gemma4:latest, Strong=gemma4:31b, CodeAware=qwen2.5-coder:7b. Cloud-only models excluded by default per strict-layer-coupling. |
| **Lens scheduler** (walks `~/.sipag/lenses/*.toml`, fires each on its `TriggerPolicy::Schedule` cadence) | ✅ | `sipag/src/serve/lens_scheduler.rs` (PR #558) | Gated by `sipag serve --lens-scheduler`. Schedule trigger supported; `Threshold` + `ModelDecide` surface in `skipped_unsupported` telemetry with a boot-time warn (operator sees "this lens won't fire until §9 #7 / §10 land"). 30s tick interval; serial fires per tick; corpus mutex held across tick (single-consumer v1, documented). |
| **Bridge lens-worker** (reactive on katulong `claude/<uuid>` events; sliding window of 200) | ✅ | `sipag/src/serve/bridge_worker.rs` (PR #564) + observer wiring (PR #565) | Subscribes to `claude/<uuid>` SSE topics, maintains 200-event `SessionWindow` per session, fires gemma via `LensWorker::run_with_tools` on threshold (every 10 events). Session discovery: observer poll → `BridgeHandle::watch`. Enable: `sipag serve --bridge-worker --workers`. Structural verbs logged at warn (UI dispatch deferred). Shared corpus with scheduler (`Arc<Mutex<Corpus>>` opened once at startup). |
| **`corpus.search` + `corpus.expand` tools** (sipag-internal MCP-shape) | ✅ | `sipag-lens::execute_corpus_search` / `execute_corpus_expand` + `LensWorker::run_with_tools` multi-turn loop (PR #556) | Gemma calls them mid-prompt via a JSON envelope (`{"tool_call": {"name": "corpus.search", ...}}`). Embedded into the lens's system prompt, parsed out of the model's reply, executed against the corpus, results fed back to the model as a `tool` message. NOT exposed to Claude (sipag is never an MCP server installed into project `.claude/`). |
| **Four verbs**: `observe(text, tags?)` + `suggest_stance` + `ask_human` + `propose_task` | ✅ | `sipag-lens::StructuralAction` (PR #551) | Free-form `observe` is the workhorse; three structural verbs for typed UI affordances. The lens scheduler currently dispatches Observes to the corpus but **logs the three structural verbs at warn-level rather than dispatching them to UI surfaces** — that wiring is the next slice of Phase 1 #3. |
| **Project-meta lenses** (pattern-spotter, meta-cognitive, strategic-cross-cutting) | 🟡 | mechanism supported; no built-ins shipped | The scheduler will load any `~/.sipag/lenses/<name>.toml` with `[source] kind = "project_meta"`. No bundled project-meta lenses yet — operators write their own. See `extras/lens.toml.example`. |
| **Ad-hoc / hypothesis lenses** (UI-created, short-lived, promote-or-expire) | 🔴 | not implemented | Phase 1 #3 + web UI for "Lenses" panel. Operator can hand-create them by dropping TOML files; no UI for create/edit/expire yet. |
| **Background workers** (expand, research, scheduler) | 🟡 | `sipag/src/serve/workers/{expand,research,scheduler}.rs` | Already running; likely become **lens-worker scheduler infrastructure**. Triage individually once the abstraction lands. |
| ~~Pre-dispatch classifier (gate)~~ | ✅ | `sipag/src/dispatch_gate.rs` (was `sipag-core/src/gate.rs` until §9 #9) | Folded into the lens-worker abstraction in §9 #9 — calls gemma via `sipag_lens::ChatBackend`. One-shot calling convention kept (output shape doesn't fit `StructuralAction`); the fold is about the wire. |
| ~~Post-dispatch observer (nudge)~~ | ✅ | ~~`sipag-core/src/nudge.rs`~~ | Deleted in §9 #11. The "post-dispatch progress" lens lives in the lens-worker abstraction (Phase 1 #3) instead. |
| **Categorize loop** (gemma sorts board items) | 🟡 | `sipag/src/serve/categorize.rs` (199 LOC) | **Early lens-worker prototype** — folds in as a "board-item categorization" lens. |
| **Refine raw ideas → tickets** (the old kanban pipeline) | ⛔ | `sipag-core/src/{feature,refine}.rs` | Deprecated in PR #536 — replaced by the spike-observe-derive model itself. |
| ~~`IteratePolicy` plug~~ | 🚫 | n/a | Removed 2026-05-17. Claude is the iterator. |
| ~~Conclude an experiment~~ | 🚫 | n/a | Removed — no per-experiment state. KR stance flipping to `done` is the signal. |
| ~~Per-classification recording verbs~~ (`note_progress`, `flag_blocker`, `record_decision`, ...) | 🚫 | n/a | Removed 2026-05-17 (lens-worker reframe). Collapsed into free-form `observe(text, tags?)`. Classification happens at *query time* via tag filters + semantic search, not at write time. |

---

## 3. Topology features (platform — where things run)

The mesh of katulong instances and ollama hosts sipag talks to.

| Capability | Status | Where (code) | Notes |
|---|---|---|---|
| **Multi-host mesh** | 🟡 | `sipag-core/src/hosts.rs` + `~/.sipag/hosts.toml` | Works; overlap with `~/.katulong/mesh.json` is a §10 open question. |
| **Per-host API key** (server-side only, never sent to browser) | ✅ | `sipag-core/src/hosts.rs` | |
| **katulong WS attach** (full duplex, browser-equivalent) | ✅ | `katulong-client/src/attach.rs` | Typed errors, RAII close, redacted Debug — well-formed SDK. See modules.md §1 "wire clients." |
| **katulong HTTP** (sync, curl-shell-out) | 🟡 | `katulong-client/src/http.rs::KatulongClient` | CLI/TUI use this. Async sibling needed for web path (Phase 2 #6). |
| **katulong HTTP** (async, reqwest, body-cap) | ✅ | `katulong-client/src/async_http.rs::KatulongAsyncClient` | Landed in PR #546. Per-call streaming body cap (`DEFAULT_BODY_CAP=1MiB`, `TRANSCRIPT_BODY_CAP=10MiB`) aborts before the full body buffers. Used by sipag's serve layer + `sipag-dispatch`. |
| **katulong SSE subscription** | ✅ | `katulong-client/src/sse.rs` (PR #562) | Third wire surface: `subscribe(http, base, api_key, topic, from_seq)` → `KatulongEventStream`. Hand-rolled SSE parser; bounded line/event caps; `terminated` flag on cap-trip; 14 tests. Consumed by the bridge lens-worker (PR #564). |
| **Cross-host federation of pub/sub** | 🔴 | each katulong is its own broker | Per upstream research (modules.md §3): sipag fans out N SSE subscriptions. |
| **LLM access via ollama-bridge** (queue+auth daemon) | ✅ | `ollama-bridge-client` crate (PR #549) + `sipag/src/bridge.rs` | `OllamaBridgeClient` wraps the [`dorky-robot/ollama-bridge`](https://github.com/Dorky-Robot/ollama-bridge) Elixir queue+auth daemon. `BridgeChatBackend` implements `sipag_lens::ChatBackend`; `BridgeEmbedder` implements `sipag_corpus::Embedder`. Shared via `BridgeWiring` in `AppState`. `sipag-core/src/llm.rs` stays for legacy callers (`categorize.rs`, `workers::research`, `workers::expand`). |
| **Claude subprocess client** | 🟧 | inline in deprecated `sipag-core/src/refine.rs:181-516` | If Experimentation's `act` needs to spawn claude, lift the stream-json parser deliberately. |
| ~~Claude transcript proxy~~ | 🟡 | `sipag/src/serve/htmx.rs::observation_transcript_handler` + `katulong-client::http::claude_transcript_url` | **Demeter violation** — sipag parses Claude JSONL through katulong proxy. The bridge lens-worker (PR #564) now consumes `claude/<uuid>` SSE events directly, replacing this path's function. Deletion is cleanup — the transcript handler is still wired but superseded. See `[[feedback-strict-layer-coupling]]`. |
| **Response-size cap** at wire boundary (DoS defense) | ✅ | `katulong-client/src/async_http.rs::bytes_capped` | Closed sipag #527 in PR #546. Streaming cap aborts before the full body is buffered; upper memory bound is `cap + max_chunk_size`. |

---

## 4. Identity features (platform — auth)

Self-contained passkey-based auth subsystem. Heavier today than the rest of sipag-core; promotion to its own crate is low-urgency (Phase 3 #15).

| Capability | Status | Where (code) | Notes |
|---|---|---|---|
| **Passkey registration** (WebAuthn) | ✅ | `sipag-core/src/auth/webauthn.rs` | |
| **Passkey login** | ✅ | `sipag-core/src/auth/webauthn.rs` + `serve/login.rs` | |
| **First-device-localhost bootstrap** | ✅ | `serve/auth_middleware.rs` | Per modules.md §5: katulong-shaped first-device flow. |
| **Pair-by-setup-token** (out-of-band device pairing) | ✅ | `sipag-core/src/auth/setup_token.rs` + `serve/devices.rs` | |
| **Device management UI** (list, revoke) | 🟡 | `serve/devices.rs` (96 LOC) | Works; sparse. |
| **Browser session cookies** | ✅ | `serve/cookie.rs` + `auth/session.rs` | |
| **Credential lockout** (brute-force defense) | ✅ | `sipag-core/src/auth/state.rs` (via `credential.rs`) | |
| **API access tokens** (for agent API, distinct from browser session) | ❓ | not implemented | Agent API surface needs an auth model — same passkey + cookie session as human, or separate service-account/bearer flow? Open question in modules.md §10. |

---

## 5. User-facing surfaces

How a human (or agent) actually drives sipag.

| Surface | Status | Where (code) | What's there |
|---|---|---|---|
| **CLI** (`sipag …`) | ✅ | `sipag/src/cli.rs` | `dispatch`, `up`, `tui`, `add`, `list`, `move`, `projects`, `project`, `sub`, `serve`, `version`. **No Steering subcommands (`objective` / `kr` / `idea`) by design** — UI-first priority; CLI surface for Steering is parked until the web UI is solid. |
| **TUI** (interactive board) | ✅ | `tui/src/board_app.rs` | Default with no args. Works for the dispatch-side workflow. |
| **Web UI** (`sipag serve`, port 7100) | ✅ | `sipag/src/serve/` (19 files; htmx + maud templates) | Where Steering lives today. |
| **Pub/sub subscribe** (`sipag sub <topic>`) | ✅ | `sipag/src/cli.rs::run_sub` | Subscribes to katulong topics for shell-level inspection. |
| **Dev-loop** (`SIPAG_DEV=1`, cargo-watch + tower-livereload) | ✅ | `serve/` startup | Iterates the web UI fast during sipag development. |
| **Agent API** (programmatic surface for agent loops) | 🔴 | not implemented | Phase 1 #5 + Phase 3 #14. The mechanism by which substrate absorbs more responsibility over time per VISION. |

---

## 6. Cross-cutting

| Concern | Status | Notes |
|---|---|---|
| **File-backed durable state** at `~/.sipag/` | ✅ | TOML for board, JSONL for pub/sub log, markdown+frontmatter for the deprecated feature store. |
| **Internal pub/sub broker** | ✅ | `sipag-pubsub` workspace crate (extracted from `sipag-core/src/pubsub.rs` in PR #545) — 16+ publish sites in `serve/`. Load-bearing. Future decision: keep in-process or route into katulong's broker (modules.md §10). |
| **Tracing / logging** | ✅ | `tracing` crate; not in scope to change. |
| **Error type strategy** | 🟡 | Mix of typed (`thiserror` in attach + auth) and `anyhow` (sipag-side glue). Standardize "typed at boundaries" per modules.md §7. |
| **Pre-commit + pre-push hooks** | ✅ | gitleaks, typos, cargo deny, cargo build --release, fmt, clippy, shellcheck (pre-commit); cargo test --workspace, cargo machete (pre-push). |
| **Bounded-context naming** | 🟧 | Partly disambiguated (PR #537 splits `Session` → `TmuxSession` + the existing `AuthSession`). `Status` splits still deferred per §10 domain-vs-schema-noun question. |

---

## 7. Quality / DX

| Feature | Status | Notes |
|---|---|---|
| **Workspace tests** (`cargo test --workspace`) | ✅ | Runs as pre-push hook. |
| **Multi-agent code review** (security + architecture + correctness + test adequacy) | ✅ | `/ship-it` skill; recently used on PRs #535, #536, #537. |
| **Playwright UI tests** (notebook + katulong-client serve) | ✅ | `katulong-client/tests-web/` |
| **Integration tests against real katulong** | ✅ | `katulong-client/tests/` (skip when `KATULONG_REPO` unset) |
| **Diwa indexing** (post-commit/post-merge hooks) | ✅ | Searchable corpus for "we tried this and moved away" trails per [`feedback-deprecate-with-rationale`](../.claude/memory). |

---

## 8. External dependencies on the dorky-robot stack

Capabilities that require upstream work in another tool we own.

| Capability | Blocked on | Status |
|---|---|---|
| Push-based "session exited" detection | [katulong#715](https://github.com/Dorky-Robot/katulong/issues/715) (`sessions/<id>/lifecycle` topic) | ⏳ filed 2026-05-17 |
| Push-based "shell prompt returned" detection | [katulong#716](https://github.com/Dorky-Robot/katulong/issues/716) (`sessions/<id>/child` topic) | ⏳ filed 2026-05-17 |
| Scaffold review agents into a project's `.claude/` | [hulma](https://github.com/Dorky-Robot/hulma) | ✅ exists (separate tool) |
| Chain-of-thought planning | [kubo](https://github.com/Dorky-Robot/kubo) | ✅ exists |

---

## 9. Deliberately NOT in scope (per VISION)

Each absence is a feature. Don't accidentally build these.

| 🚫 Not building | Why |
|---|---|
| Kanban funnel (swimlanes, lanes, drag-cards-between-columns workflow) | Tasks live under their KR, not in columns. The funnel is "task management theatre." |
| Sprints, velocity, story points | Cadence is irrelevant. An objective is open until it isn't. |
| Standups, retros, planning rituals | These are human-coordinating-with-human ceremonies. sipag is human-coordinating-with-agents. |
| Assignees | Ownership lives at the KR level. Whichever agent picks a task up runs it. |
| "Blocked" / "Ready for review" lanes | Kanban artifacts. If a KR is at risk, its stance turns yellow or red. That's the only signal. |
| Portfolio / program / roadmap layer above objectives | Above objectives there is nothing. Any new layer goes *up* (decide what we're not optimizing for), not sideways into more management. |
| Refinement pipeline (raw → grouped → refined → ticket) | Replaced by Experimentation (spike → observe → record). The kanban-shaped refinement was deprecated in PR #536. |
| Sipag exposing MCP server in projects' `.claude/` for Claude to call directly | Demeter violation — couples Claude to sipag's existence and tool schemas. The bridge between Claude's free-form output and sipag's structured world is **gemma4 on sipag's side**, watching katulong's published events. MCP semantics still useful, but between sipag and gemma4 internally — not at the project boundary. See `[[feedback-strict-layer-coupling]]`. |
| Sipag reaching past katulong to parse Claude transcripts directly | Same Demeter violation, in reverse. The existing `claude_transcript_url` path is a legacy backdoor that retires when SSE subscriber lands. New code goes through katulong's published event topics, never raw Claude JSONL. |
| State-machine tracking of "trials" (per-dispatch lifecycle aggregates) | Removed 2026-05-17 — was kanban-shaped despite the rename. Event log + recorded actions replace it. Claude is the iterator; sipag observes and records. |

---

## 10. Edit log

- 2026-05-17 — initial draft.
- 2026-05-17 — added UI-first surface priority. CLI rows that previously flagged "CLI surface absent" as a gap re-framed as ⏳ deferred. See memory `feedback-sipag-ui-first`.
- 2026-05-17 — §2 Experimentation reframed. State-machine vocabulary (`Trial`, `IteratePolicy`, `Conclude an experiment`) struck through and marked 🚫 (removed from roadmap). Added "Record" sub-section (was "Iterate") with internal recording API, gemma4 bridge dispatcher, and `RecordedAction` rows. §3 Topology gained an explicit Demeter-violation row for the existing Claude-transcript-proxy reach. §9 NOT-in-scope grew three rows: sipag-as-MCP-server-to-Claude (Demeter), sipag-parsing-Claude-transcripts-directly (Demeter, in reverse), state-machine trial tracking (kanban-shaped). See memories `feedback-strict-layer-coupling`, `project-sipag-work-model-experimentation`.
- 2026-05-17 — review-fix round 1 on PR #538 — corrected stale "Experimentation `iterate` policies" reference in §1 KR-acceptance row to point at the gemma4-bridge recording API; fixed `worktree_command` line number (513→523); tightened "How to use" rubric with explicit doc-rot anti-pattern + carve-out for retired-concept tombstones. (Companion: modules.md §1 now forward-links here.)
- 2026-05-17 — **§1 + §2 reframed for the lens-worker abstraction** (third pass on Experimentation; see modules.md §3 reframe). §1 Steering gained a "dual role" note — every Steering entry IS also a lens definition. §2 Experimentation's "Record" sub-section renamed to "Derive" (Claude is the iterator; sipag derives). Rows: corpus, LensWorker runtime, bridge as first worker, corpus-search tools, four verbs (observe + 3 structural), project-meta lenses, ad-hoc lenses, plus reframed gate/nudge/categorize as early lens-worker prototypes. Added 🚫 row for the per-classification recording verbs (collapsed into free-form `observe`).
- 2026-05-18 — added **per-lens model selection** row to §2 Derive. Driven by the practical observation (running the live serve with gemma4:31b for everything) that high-frequency lens-workers want a fast model and derivation workers can afford a strong one. The `LensWorker` runtime row updated to include `ModelChoice` as part of the spec. See modules.md §3 + §9 #3 + #8 for full details.
- 2026-05-18 — §3 Topology rows for body-cap + async HTTP flipped to ✅ (PR #546 closed sipag #527). §6 internal pub/sub broker row updated to reflect the `sipag-pubsub` extraction (PR #545).
- 2026-05-19 — §2 Experimentation `Act` rows updated for the `sipag-dispatch` extraction (PR #547 — Phase 2 #12). Dispatch action now lives in its own workspace crate; web UI + CLI both call the same function. Worktree setup parity restored on the web UI path. §3 Topology row for ollama renamed from "Ollama HTTP client" to "LLM access via ollama-bridge" — the integration point is the [`dorky-robot/ollama-bridge`](https://github.com/Dorky-Robot/ollama-bridge) Elixir daemon (queue+auth), not direct ollama HTTP. See `[[reference-ollama-bridge]]`.
- 2026-05-22 — **lens-worker substrate flipped to ✅.** Five §2 Derive rows updated from 🔴 to ✅ to reflect what landed across PRs #549 (`ollama-bridge-client`), #550 (`sipag-corpus`), #551 (`sipag-lens` runtime + ModelChoice + four verbs), #556 (`corpus.search`/`corpus.expand` MCP-shape tool wrappers + multi-turn loop), #558 (lens scheduler in `sipag/src/serve/lens_scheduler.rs` behind `--lens-scheduler` flag). New row added for the lens scheduler itself. §2 Observe rows flipped to ✅ for pre-dispatch state classification + pre-dispatch classifier prototype (gate folded into the lens-worker abstraction per §9 #9, now talks gemma via `sipag_lens::ChatBackend` — PR #559). Bridge lens-worker stays 🔴 (Phase 1 #3 remainder; needs §9 Phase 2 #7 SSE subscriber or a polling fallback). Project-meta lenses move 🔴 → 🟡 (mechanism supported by the scheduler + lens TOML schema; no built-in lenses bundled). Recovery loop and post-dispatch observer (nudge) rows flipped to ✅ deleted in §9 #11 (closes sipag #528 by deletion). Structural-verb dispatch to UI surfaces (suggest_stance / ask_human / propose_task) is the next slice — scheduler logs them at warn-level today but doesn't wire them into the UI.
