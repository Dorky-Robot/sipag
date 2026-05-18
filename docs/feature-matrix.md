# sipag feature matrix

**Status:** working draft, iterating collaboratively.
**Frame:** [`VISION.md`](../VISION.md) → product posture; [`docs/modules.md`](modules.md) → how we'll restructure to get there; **this doc** → what sipag actually offers (capability-level), where the gaps are, and what we're deliberately *not* building.

## How to use this doc

- One row per user-facing or system-level capability (not per file or module — that's `modules.md`'s job).
- Status reflects the *capability* state, not the underlying code's quality. A feature can be ✅ shipped while its implementing module is 🔴 messy.
- Group by [bounded context](modules.md#1-the-context-map) so the human/agent line stays visible.
- Edits welcome. When status changes, leave a one-liner in §10 edit log.

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

| Capability | Status | Where (code) | Notes |
|---|---|---|---|
| Create / view / close an **Objective** | 🟡 | `sipag-core/src/board/objective.rs` + `sipag/src/serve/board_view.rs` | Type exists; web UI exposes objectives. CLI ⏳ per UI-first priority. |
| Add / view / edit a **Key Result** under an Objective | 🟡 | `sipag-core/src/board/key_result.rs` + `sipag/src/serve/board.rs:237` | Type exists; web UI can add/edit. CLI ⏳. |
| Set **KrStance** (green / yellow / red / done) | 🟡 | `sipag-core/src/board/key_result.rs:19` (`KrStance` enum); `sipag/src/serve/board.rs:269` (HTTP toggle) | Stance is the scalar reward signal per VISION. Web UI works. CLI ⏳. |
| **Standing** — top-level surface for upkeep/firefights (work that doesn't ladder to an Objective) | 🟡 | `sipag-core/src/board/project.rs:17` (`ProjectKind::Standing`) | Exists as a project kind; web UI semantics still evolving. CLI ⏳. |
| **Idea box** — parking lot for unprocessed input | 🔴 | not modeled as a first-class type | VISION names this explicitly. No `Idea` aggregate. Captured ad-hoc as tasks today. **Web UI work** when this lands (Phase 1 #4 in modules.md). |
| **promote_idea → Experiment** (the ACL crossing into Experimentation) | 🔴 | n/a — depends on Idea + Experiment types | Per modules.md §6, this is the first-class cross-context ACL. Blocked on Phase 1 #3 + #4. |
| **Agent API surface** — read KRs, `report_stance` from agent | 🔴 | not implemented | VISION-planned: same `POST /tasks` and `PATCH /tasks/:id` endpoints exposed for agent loops. Phase 1 #5 + Phase 3 #14 in modules.md §9. |
| **KR-level summary across projects** (the "three objectives, six KRs, eleven tasks" view from VISION) | 🟡 | `sipag/src/serve/board_view.rs` | Exists in web UI. CLI ⏳. |
| **Accept / redirect agent-proposed work at the KR level** | 🔴 | no agent loop pushing proposals today | VISION's intended boundary. Comes alive when Agent API + Experimentation `iterate` policies land. |

---

## 2. Experimentation features (agent surface)

The spike → observe → record loop per [`project-sipag-work-model-experimentation`](../.claude/memory). **Reframed 2026-05-17**: there's no `Trial` aggregate, no `IteratePolicy` trait, no state machine. The event log is the source of truth. **Claude is the iterator**; sipag observes via gemma4 (watching katulong's published `claude/<uuid>` events) and records structured signals into KR-tagged pub/sub. Sipag never reaches past katulong to talk to Claude directly — gemma4 is the bridge. See `[[feedback-strict-layer-coupling]]`.

### Act — fire a spike

| Capability | Status | Where (code) | Notes |
|---|---|---|---|
| **Dispatch a task to a katulong session** | ✅ | `sipag/src/cli.rs::run_dispatch_task`, `sipag/src/serve/htmx.rs` (web path) | Sync CLI path works; web path is the 🔴 #527 surface (closes in Phase 2 #6). |
| **Auto-create worktree** for the dispatched task | ✅ | `katulong-client/src/http.rs:513` (`worktree_command`) | Wired via role's `worktree = true`. Helper currently in the wire crate; lifted to `act` sub-module in Phase 2 #12. |
| **Generate unique dispatch session name** (`sipag-d-<hex>`) | ✅ | `katulong-client/src/http.rs:494` | Per-dispatch tile so the auto-summarizer can rename without breaking back-pointers. |
| **Build agent launch command** (`cd <wt> && <role-cmd> -p '…'`) with shell-quote-escape | ✅ | `katulong-client/src/http.rs:533` | Single-quote escape verified by 3 unit tests. |
| **Spin up persistent role tiles** (`sipag up`) | ✅ | `sipag/src/cli.rs::run_up` | Project-level "warm the sessions" command. |
| ~~Trial lifecycle tracking~~ | 🚫 | n/a | Removed from roadmap 2026-05-17. State-machine framing was wrong (kanban-shaped despite the rename). The event log + recorded actions replace per-trial state tracking. See `[[feedback-strict-layer-coupling]]` and §3 reframe in modules.md. |

### Observe — collect the feedback signal

| Capability | Status | Where (code) | Notes |
|---|---|---|---|
| **Pre-dispatch state classification** ("is this pane ready?") | 🟡 | `sipag-core/src/gate.rs` | Works via gemma4 / ollama. Folds into `session_classifier` in Phase 2 #9. |
| **Post-dispatch periodic observation** | 🟡 | `sipag-core/src/nudge.rs` (mid-repositioning per its own module doc) | Today's polling loop. Replaced by event-driven SSE in Phase 2 #10 once Phase 2 #7 (SSE subscriber) lands. |
| **Event-driven observation** via katulong `claude/<uuid>` pub/sub | 🔴 | n/a — needs SSE subscriber | Phase 2 #7 + #10. Once upstream [katulong#715](https://github.com/Dorky-Robot/katulong/issues/715) / [#716](https://github.com/Dorky-Robot/katulong/issues/716) land, classification shrinks dramatically. |
| **Detect "session exited"** | 🔴 | not pushed by katulong today | Blocked on upstream [katulong#715](https://github.com/Dorky-Robot/katulong/issues/715). |
| **Detect "shell prompt returned"** (`hasChildProcesses` transitions) | 🔴 | not pushed by katulong today | Blocked on upstream [katulong#716](https://github.com/Dorky-Robot/katulong/issues/716). |
| **Detect "needs human"** (permission prompts) | 🟡 | implicit via gemma classification | Partial cover from `permission-request` events in `claude/<uuid>` topic. Will improve in Phase 2 #10. |
| **Detect "stuck"** (silence timeout) | 🔴 | not implemented | Subscriber-side derived signal (threshold is consumer policy); lands with Phase 2 #10. |
| **Typed `RecordedAction` value object** for what gemma4 derived | 🔴 | not implemented | Phase 1 #3 (reframed). Replaces the previous `Outcome` row — `Outcome` was the state-machine framing. `RecordedAction` is just "what gemma4 emitted + source-event-window reference," append-only. |
| **Cross-host observation** (subscribe to all mesh peers) | 🔴 | not implemented | Phase 2 #7 SSE subscriber needs N-stream fan-out. |
| **Recovery loop** (`verify_and_heal_dispatch` — LLM proposes keystrokes) | ⛔ | `sipag/src/serve/htmx.rs:1577` | Legacy; closes #528 by *deletion* (Phase 2 #11) when attach client owns keystrokes. |

### Record — what gemma4 derives from the event stream

(Was "iterate." Renamed 2026-05-17 — Claude is the iterator; sipag records.)

| Capability | Status | Where (code) | Notes |
|---|---|---|---|
| **Internal recording API** (`note_progress`, `flag_blocker`, `propose_task`, `suggest_stance`, `ask_human`) | 🔴 | not implemented | Phase 1 #3 (reframed). Sipag-internal Rust functions — **NOT exposed externally**; gemma4 dispatches into them. |
| **Gemma4 bridge dispatcher** (sliding window → structured JSON → recording-API call) | 🔴 | not implemented | Phase 1 #3 (reframed). The translation layer from Claude's free-form output to sipag's structured world. |
| **`RecordedAction` value object** | 🔴 | not implemented | What gemma4 emitted + the source-event-window reference. Append-only into pub/sub. |
| **Background workers** (expand, research, scheduler) | 🟡 | `sipag/src/serve/workers/{expand,research,scheduler}.rs` | Already running; likely consume the recording API rather than feed into it. Triage individually once the API exists. |
| **Categorize loop** (gemma sorts board items) | 🟡 | `sipag/src/serve/categorize.rs` | Works; same gemma-bridge shape as the recording bridge. Folds into `observe` per modules.md §8. |
| **Refine raw ideas → tickets** (the old kanban pipeline) | ⛔ | `sipag-core/src/{feature,refine}.rs` | Deprecated in PR #536 — replaced by the spike-observe-record model itself. |
| ~~`IteratePolicy` plug~~ | 🚫 | n/a | Removed from roadmap 2026-05-17. Claude is the iterator; sipag doesn't iterate. |
| ~~Conclude an experiment~~ | 🚫 | n/a | Removed — there's no per-experiment state to "conclude." When the human flips KR stance to `done`, the experiment is done. |

---

## 3. Topology features (platform — where things run)

The mesh of katulong instances and ollama hosts sipag talks to.

| Capability | Status | Where (code) | Notes |
|---|---|---|---|
| **Multi-host mesh** | 🟡 | `sipag-core/src/hosts.rs` + `~/.sipag/hosts.toml` | Works; overlap with `~/.katulong/mesh.json` is a §10 open question. |
| **Per-host API key** (server-side only, never sent to browser) | ✅ | `sipag-core/src/hosts.rs` | |
| **katulong WS attach** (full duplex, browser-equivalent) | ✅ | `katulong-client/src/attach.rs` | Typed errors, RAII close, redacted Debug — well-formed SDK. See modules.md §1 "wire clients." |
| **katulong HTTP** (sync, curl-shell-out) | 🟡 | `katulong-client/src/http.rs::KatulongClient` | CLI/TUI use this. Async sibling needed for web path (Phase 2 #6). |
| **katulong HTTP** (async, reqwest, body-cap) | 🔴 | not implemented | **Phase 2 #6 — closes sipag #527**. |
| **katulong SSE subscription** | 🔴 | only `sub_url()` URL builder exists | **Phase 2 #7**. Built against today's `claude/<uuid>`; gains `sessions/<id>/*` when upstream issues land. |
| **Cross-host federation of pub/sub** | 🔴 | each katulong is its own broker | Per upstream research (modules.md §3): sipag fans out N SSE subscriptions. |
| **Ollama HTTP client** | 🟡 | `sipag-core/src/llm.rs` (300 LOC) | Promote to `ollama-client` crate with typed responses (Phase 2 #8 — sets up #528 closure). |
| **Claude subprocess client** | 🟧 | inline in deprecated `sipag-core/src/refine.rs:181-516` | If Experimentation's `act` needs to spawn claude, lift the stream-json parser deliberately. |
| ~~Claude transcript proxy~~ | 🔴 | `sipag/src/serve/htmx.rs::observation_transcript_handler` + `katulong-client::http::claude_transcript_url` | **Demeter violation** — sipag parses Claude JSONL through katulong proxy. Retires when SSE subscriber (Phase 2 #7) lets sipag consume katulong's `claude/<uuid>` topic events instead. See `[[feedback-strict-layer-coupling]]`. |
| **Response-size cap** at wire boundary (DoS defense) | 🔴 | absent | The #527 vector. Lands at wire-client level in Phase 2 #6. |

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
| **TUI** (kanban board) | ✅ | `tui/src/board_app.rs` | Default with no args. Works for the dispatch-side workflow. |
| **Web UI** (`sipag serve`, port 7100) | ✅ | `sipag/src/serve/` (19 files; htmx + maud templates) | Where Steering lives today. |
| **Pub/sub subscribe** (`sipag sub <topic>`) | ✅ | `sipag/src/cli.rs::run_sub` | Subscribes to katulong topics for shell-level inspection. |
| **Dev-loop** (`SIPAG_DEV=1`, cargo-watch + tower-livereload) | ✅ | `serve/` startup | Iterates the web UI fast during sipag development. |
| **Agent API** (programmatic surface for agent loops) | 🔴 | not implemented | Phase 1 #5 + Phase 3 #14. The mechanism by which substrate absorbs more responsibility over time per VISION. |

---

## 6. Cross-cutting

| Concern | Status | Notes |
|---|---|---|
| **File-backed durable state** at `~/.sipag/` | ✅ | TOML for board, JSONL for pub/sub log, markdown+frontmatter for the deprecated feature store. |
| **Internal pub/sub broker** | ✅ | `sipag-core/src/pubsub.rs` — 16+ publish sites in `serve/`. Load-bearing. Future decision: keep in-process or route into katulong's broker (modules.md §10). |
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
