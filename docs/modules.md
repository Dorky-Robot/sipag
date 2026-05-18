# sipag modules — domain map

**Status:** working draft, iterating collaboratively.
**Frame:** [`VISION.md`](../VISION.md) → product posture. This doc → how the codebase carries that posture. DDD bounded contexts as the organizing principle.

## Reading guide

- ✅ already extracted as its own crate / well-formed module
- 🟡 lives in code today, needs polish or relocation
- 🟧 mixed in with other things, candidate for extraction
- 🔴 scattered or duplicated, structural debt
- ⛔ deprecated — see `[[feedback-deprecate-with-rationale]]` (don't delete, preserve as "we tried this")
- ❓ open question — answer in a follow-up edit

---

## 0. Frame

sipag's strategic posture (from VISION.md):

- **Steering is human, execution is agent.** The human surface is Objectives + KRs + Standing + Ideas. Tasks live *below* the human surface.
- **The agent loop is empirical, not procedural.** Spike → observe → record (was "iterate" — see §3 reframe; Claude is the iterator, sipag records). OKRs provide the reward signal; the substrate provides gradient ascent. *"It's the coding version of reinforcement learning... the Thomas Edison of experiment, observe, tweak, iterate"* (memory: `project-sipag-work-model-experimentation`).
- **The fix happens at the right layer.** When the right home for a signal is in a tool we own (katulong, kubo), extend that tool — don't add sipag-side workarounds (memory: `feedback-fix-at-right-layer`).

These three principles drive every contextual choice below.

---

## 1. The context map

Four bounded contexts. Each owns its language; translation happens at named anti-corruption layers (§6).

> **See also:** [`docs/feature-matrix.md`](feature-matrix.md) tracks per-capability state (✅/🟡/🟧/🔴/⏳/🚫) organized by the same bounded contexts. modules.md is the architectural design; feature-matrix.md is the running scorecard for what's shipped, what's gapped, and what's deliberately out of scope.

| Context | Side of the line | What it owns | Posture |
|---|---|---|---|
| **Steering** | human | direction + reward signal | strategic, qualitative, durable |
| **Experimentation** | agent | the spike-observe-record loop | tactical, empirical, fast |
| **Topology** | platform | where things run | infrastructure, mostly stable |
| **Identity** | platform | who can do what | infrastructure, mostly stable |

The human/agent boundary cuts between Steering and Experimentation. The platform contexts support both.

```
                         ┌─────────────────────────────────────┐
                  human  │   Steering: Objectives, KRs, …      │
                         └─────────────┬───────────────────────┘
                                       │ promote_idea / report_stance
                                       │ (ACL)
                         ┌─────────────▼───────────────────────┐
                  agent  │   Experimentation:                  │
                         │     spike → observe → record        │
                         └─────────────┬───────────────────────┘
                                       │ (ACLs to platform)
              ┌────────────────────────┴─────────────────────────┐
              ▼                                                  ▼
         ┌──────────────────────┐                ┌─────────────────────────┐
plat ←   │  Topology            │                │   Identity              │
         │  mesh, hosts, wire   │                │   auth, devices,        │
         │  (katulong, ollama)  │                │   passkeys, tokens      │
         └──────────────────────┘                └─────────────────────────┘
```

---

## 2. Steering context (human surface)

**Responsibility.** Capture what we're optimizing for, capture whether it's working. Everything else is below the line.

**Ubiquitous language.**

- **Nouns**: `Objective`, `KeyResult`, `KrStance` (green / yellow / red / done), `Standing`, `Idea`, `Project` (namespace)
- **Verbs**: `set_objective`, `add_key_result`, `update_stance`, `close_objective`, `capture_idea`, `promote_idea`, `report_done`

**Aggregates.**

- `Objective` (root) — owns its KRs, has a lifecycle (open / closed)
- `KeyResult` — child of Objective; stance is the scalar feedback for the experimentation loop below
- `Standing` (root) — upkeep / firefight work that doesn't ladder to an objective
- `Idea` (root, short-lived) — unprocessed human input; `promote_idea` is the ACL that starts an Experiment

**Where it lives today.**

| Piece | Path | Status |
|---|---|---|
| Objective + KrRef | `sipag-core/src/board/objective.rs` | 🟡 |
| KeyResult + KrStance | `sipag-core/src/board/key_result.rs` | 🟡 |
| Project + Status + ProjectKind::Standing | `sipag-core/src/board/project.rs` | 🟧 — mixes Steering (Standing) with Experimentation (Status column metadata, Role) |
| Idea | ❌ not modeled as a type | per VISION, "Idea box" exists — needs first-class aggregate |
| Agent API surface (read KRs, push stance) | ❌ not implemented | **planned per VISION**; published-language layer for agents |
| Human web UI | `sipag/src/serve/board.rs` (602), `serve/board_view.rs` (2276 🔴), parts of `serve/htmx.rs` | needs decomposition (§8) |
| TUI | `tui/src/board_app.rs` | 🟡 |

**What's missing.**

- `Idea` aggregate (and `promote_idea` ACL into Experimentation)
- Agent API surface — read-only KR + objective endpoints, plus `report_stance` from the agent side

---

## 3. Experimentation context (the agent loop)

**Responsibility.** Run the spike-observe-record loop. Fire something (or let the human fire it), watch what happens via katulong's published event stream, and record signals worth surfacing back up to the Steering layer. **Claude itself is the iterator** — sipag doesn't decide "what to try next." Sipag observes and records.

**Strict layer coupling (added 2026-05-17 — see `[[feedback-strict-layer-coupling]]`).** Sipag is two hops from Claude (`Claude → katulong → sipag`). Sipag never reaches past katulong to talk to Claude directly. The bridge between Claude's free-form output and sipag's structured world is **gemma4 on sipag's side**, watching katulong's `claude/<uuid>` events. Sipag does NOT expose itself as an MCP server installed into projects' `.claude/` — that would couple Claude to sipag.

**Ubiquitous language.**

- **Nouns**: `Experiment` (thin grouping), `Hypothesis`, `Spike`, `Action` (the structured thing gemma4 emits), the recording-API verbs themselves
- **Verbs (the recording API gemma4 invokes via structured JSON)**:
  - `note_progress(kr_id, observation)` — agent appears to have made progress toward this KR
  - `flag_blocker(kr_id, reason)` — agent is stuck on something specific
  - `propose_task(kr_id, title, hypothesis?)` — derived from agent context: a task worth tracking
  - `suggest_stance(kr_id, stance, reason)` — gemma's view of where this KR is; human still confirms
  - `ask_human(question)` — surface a question in the human UI sidebar
- **NOT in the language**:
  - kanban verbs (`refine_features`, `group_features`, `move_task`) — deprecated since PR #536
  - state-machine verbs (`transition_to`, `complete_trial`, `set_workflow_status`) — there's no state machine; the event log + recorded signals are the source of truth
  - MCP tools exposed to Claude — Demeter violation

**Aggregates.**

- `Experiment` (root, **thin first-class on disk**) — a `Hypothesis` + `KrRef` + a tag for filtering the event log. NO state field. More like a saved view / folder than a state-machine aggregate. Optional — could be derived entirely from the log if even this proves overkill.
- `RecordedAction` (value object) — what gemma4 emitted (e.g., `{kind: "note_progress", kr_id: ..., text: ...}`) plus the event-window it derived from. Append-only into pub/sub.
- **NO** `Trial` aggregate — was state-machine-shaped; the event log + recorded actions replace it.
- **NO** `IteratePolicy` trait — Claude is the iterator; sipag doesn't iterate.
- **NO** `WorkflowStatus` enum (the per-trial state version) — same reason. `KrStance` (Steering-side, human-confirmed) is the only stance signal sipag tracks.

**Sub-responsibilities.**

- **act** — kick off whatever the human or agent is going to do. Today: katulong session create + agent command exec + worktree setup. Doesn't change in this reframe; lives wherever dispatch lives.
- **observe** — gemma4 watches katulong's `claude/<uuid>` event stream (and the new `sessions/<id>/*` topics when [katulong#715](https://github.com/Dorky-Robot/katulong/issues/715) / [#716](https://github.com/Dorky-Robot/katulong/issues/716) land). Maintains a sliding window of recent events per session. **On threshold-crossing events** (specific event kinds: `permission-request`, `reply` containing terminal-phrase patterns, silence > N seconds, etc.) prompts gemma4 with the window + project KR context + the structured-output schema for the recording API. Dispatches gemma4's emitted action to the recording function. *(Threshold-based not periodic — periodic ticks at scale spend a gemma call per session per minute, even when nothing's happening. Open question §10: exact threshold list.)*
- **record** — the internal recording API itself (Rust functions, sipag-internal — NOT exposed externally). Persists each action into pub/sub (`observations/<kr_id>` topic, say) so UI can render reactively.

**Failure-mode policy (sub-responsibility-level invariants).**

These are the places the implementer will invent a policy on day one if the doc doesn't commit to one. Each invariant is testable (see Testability invariants below):

- **Malformed gemma4 JSON / schema violation** — drop the action, increment a per-session `bridge_parse_failures` counter, log at WARN. Don't retry-with-reflection in the first cut (cost; complexity — see §10 for the deferred option). Threshold for escalation: **5 consecutive parse failures or 10 parse failures in a 50-event window** routes to the operator channel (NOT `ask_human` — see operator-vs-steering separation below). The window keeps moving regardless; gemma4 sees fresh context on the next threshold trigger.
- **Recording-API call fails** (validated JSON but e.g., `kr_id` doesn't exist) — same: drop, counter (`bridge_dispatch_rejections`), log. Reject the action; don't auto-create the missing entity. Same threshold rule applies; same operator-channel routing.
- **Sliding window dedup** — each `RecordedAction` is dedup'd by `(kind, kr_id, payload-content-hash)` over a per-session 60-second window. Source-event seq range is recorded on each action for debugging (so you can see "this `note_progress` was derived from events seq 4520-4544") but is **not** the dedup key. Reason: gemma4 can legitimately propose the same action from slightly-shifted windows of the same underlying signal; content-hash catches that case correctly. Cross-window same-action firing is therefore one action, not two. **Schema invariant for the implementer**: the recording-API's structured-output schema must not include window-positional fields (timestamps, seq numbers, "as-of-N-seconds-ago" phrasings) in the hashed payload — those go in the debug-only source-event-window field on `RecordedAction`. Otherwise hash diverges across windows for the same logical observation and dedup silently fails open.
- **`ask_human` fan-in semantics** — dedupe by `(session_id, kr_id, kind, normalized-question-text-hash)`. Default window per kind: **5 min for `permission-style` questions, 1 hour for `progress-check` questions, 24 hours for `is-this-KR-still-alive` questions.** See §10 — these defaults are starting points; per-kind tuning is consumer policy and will likely move to a config.
- **Operator-vs-steering signal separation** — the bridge has TWO escalation channels:
  - **Operator channel** (new — sidebar item with `kind: operator-alert`) for "sipag is misfiring" signals: parse-failure threshold, dispatch-rejection threshold, gemma-unreachable status. Audience is the human-as-sipag-operator. **This channel renders inside Experimentation's own UI surface — there is no Steering ACL crossing** (the operator-channel never escalates into the human's KR-stance world). That's why §6 ACL table doesn't carry a row for it: the channel is Experimentation-internal.
  - **`ask_human` channel** (sidebar item with `kind: steering-question`) for genuine "the work needs your input" signals from the bridge's derivation. Audience is the human-as-steerer. **This one DOES cross into Steering** — it appears in the KR sidebar where the human is already looking for "is this KR working." Catalogued in §6 ACL table.
  Conflating these into one sidebar makes both noisier than they need to be (and conflates VISION's two questions with a third).
- **Gemma4 unreachable** — bridge marks status as `Degraded` (NOT silent), increments a `bridge_unavailability_seconds` counter, and surfaces a sticky **operator-channel banner** ("auto-observer offline — last seen N min ago"). The sliding-window event stream is **bounded at 200 events per session** (configurable); during an outage longer than what 200 events worth of activity covers, **older events are dropped on the floor** — gemma4 sees only the most recent 200 when it comes back, not the full backlog. This is a deliberate trade-off (bounded memory) and the doc commits to it explicitly so the operator-channel banner can also surface "auto-observer caught up; N events dropped during outage" when ollama returns. Do NOT fall back to LLM-free derivation (separate path with separate failure modes).

**Trigger-blind-spot mitigation.** Threshold-crossing-only triggers (`permission-request`, `reply` containing terminal-phrase patterns, silence > N seconds) miss the "steady low-signal session" case — a session emitting continuous `reply` events that never trip a pattern looks healthy but generates no observations. Mitigation: **second-tier trigger** — if a session has had ≥20 events in the last 5 minutes AND no `RecordedAction` from the bridge in the same window, fire one gemma4 call regardless of pattern matching. Cheap (caps at one extra call per 5 min per active session) but defends against the silent-success blind spot. **Schema invariant**: the recording-API structured-output schema must include an explicit `no_action_warranted` response variant. The second-tier call exists to *ask* gemma whether there's a signal; it must not pressure gemma to emit a placeholder action just to satisfy being called. A quiet-but-productive session should yield zero `RecordedAction`s, not a low-signal one.

**Testability invariants** (Phase 1 #3 must preserve):

- Bridge dispatcher is a pipeline of pure functions + one async LLM call: `(window: &[KatulongEvent], ctx: &KrContext) → render_prompt → llm_client.call → parse_action → dispatch_to_recording_api`. The pure halves (`render_prompt`, `parse_action`) are unit-testable without a live ollama, matching the `gate.rs::render_user_prompt` + `gate.rs::parse_decision` template that's already proven out (see `sipag-core/src/gate.rs:123, 153` and the test module at `:272+` for the live example).
- Bridge takes a **`dyn LlmClient`** (the trait emerging from the Phase 2 #8 typed ollama client work). Tests inject a mock that returns canned structured-JSON responses; production wires the real ollama-backed client. This seam is reusable across the bridge AND the existing `gate.rs` / `nudge.rs` flows when they fold into `observe` (Phase 2 #9) — locking it in here means Phase 2 #8 ships a consumer that already speaks the trait.
- Sliding window is **passed as a plain `&[KatulongEvent]` slice**, not a `dyn SlidingWindow` trait. The window's *maintenance* (push, expire, content-hash dedup state) is the testable thing; the *snapshot at gemma-call time* is plain data. No trait needed for the snapshot half.
- Recording-API functions take an injectable `dyn PubSubSink` (or equivalent) so tests construct a mock sink and assert what gets appended.
- **Operator-channel and `ask_human` channel are concrete typed wrappers around a shared `dyn PubSubSink` injection** — i.e., one sink injection per bridge instance, with two newtype-style wrappers (`OperatorSink(&dyn PubSubSink)` and `SteeringSink(&dyn PubSubSink)`) that constrain what topic each can publish to. Tests inject one mock sink and assert *which wrapper* published *which payload* — cleaner than two separate `dyn` injections that would have to be wired identically.

**Where it lives today.**

| Piece | Path | Status |
|---|---|---|
| Task | `sipag-core/src/board/task.rs` | 🟡 — stays as a board-level concept (a task on the board, dispatched to a session). NOT renamed to `Trial`; no `Trial` aggregate exists in the new shape (state machines were the wrong frame — see §3 reframe 2026-05-17). |
| Observation aggregate | `sipag-core/src/board/observation.rs` | 🟧 — close cousin of the new `RecordedAction`; either renames or composes |
| Claude transcript proxy | `sipag/src/serve/htmx.rs::observation_transcript_handler` + `katulong-client::http::claude_transcript_url` | 🔴 **Demeter violation** — sipag parsing Claude-shaped JSONL through a katulong proxy. Retires when the SSE subscriber lets sipag consume katulong's `claude/<uuid>` topic events instead. See `[[feedback-strict-layer-coupling]]`. |
| Role (agent command template) | `sipag-core/src/board/role.rs` | 🟡 — Experimentation infrastructure |
| Pre-dispatch classifier ("gate") | `sipag-core/src/gate.rs` (346 LOC) | 🟧 — fold into `observe` |
| Post-dispatch observer | `sipag-core/src/nudge.rs` (417 LOC, mid-repositioning) | 🟧 — fold into `observe` |
| Recovery loop (`verify_and_heal_dispatch`) | `sipag/src/serve/htmx.rs:1577` | 🔴 — **delete** when attach client owns keystrokes (per existing dispatch-implementation-plan §7) |
| LLM / gemma client | `sipag-core/src/llm.rs` (300 LOC) | 🟡 — Experimentation's observe-side infrastructure; **the gemma4 bridge runs through here** (see `[[feedback-strict-layer-coupling]]`) |
| Dispatch mechanics (CLI + TUI paths) | `sipag/src/cli.rs`, `tui/src/board_app.rs:353` (uses sync `KatulongClient::from_remote_json()` + calls `katulong::{session_name, worktree_command, agent_command}` inline — the exact dispatch-policy helpers §9 #9 lifts) | 🟧 — the `act` sub-module; **TUI is the third dedup site** alongside CLI and serve |
| Dispatch mechanics (web path) | `sipag/src/serve/htmx.rs` + URL builders | 🔴 — same act surface duplicated, plus the #527 unbounded-body vector |
| Refinement pipeline | `sipag-core/src/feature.rs` (847), `sipag-core/src/refine.rs` (1367) | ⛔ **deprecated** — kanban-shaped; replaced by Experimentation. Don't delete; preserve as "we tried this" per `[[feedback-deprecate-with-rationale]]`. Strip wiring, add deprecation note pointing at this doc. |
| Categorize loop | `sipag/src/serve/categorize.rs` | 🟧 — fold into `observe` (same gemma-bridge shape: window of events → structured action) |
| Background workers (expand, research, scheduler) | `sipag/src/serve/workers/{expand,research,scheduler}.rs` (plus `mod.rs`) | ❓ — they look like background actors that consume the recording API rather than feed into it. Triage individually once the recording API exists. |

**What's missing.**

- `Experiment` thin aggregate (KR ref + hypothesis + filter tag for the event log). Optional — may even be derived rather than persisted on disk.
- `RecordedAction` value object — what gemma4 emitted (kind + payload + source-event-window reference); append-only into pub/sub.
- **Internal recording API** (`note_progress`, `flag_blocker`, `propose_task`, `suggest_stance`, `ask_human`) — sipag-internal Rust functions, **NOT exposed externally** (per `[[feedback-strict-layer-coupling]]`). Dispatched to from gemma4's structured JSON output.
- **Gemma4 bridge dispatcher** — sliding-window prompter + structured-output parser + recording-API dispatcher. The bridge between Claude's free-form output and sipag's structured world.
- **Event-driven observer** that subscribes to katulong's `claude/<uuid>` topic and feeds the gemma4 bridge.
- **NOT needed anymore** (struck from previous plan): `Trial` aggregate, `IteratePolicy` trait, `WorkflowStatus` enum, `Outcome` as state-machine-payload type. The event log + recorded actions replace state-machine state. Claude is the iterator; sipag doesn't iterate.

---

## 4. Topology context (platform — where things run)

**Responsibility.** Model the mesh of machines / katulong instances / ollama hosts. Own the wire protocols sipag talks. Federate when needed.

**Ubiquitous language.**

- **Nouns**: `Host`, `Peer`, `MeshTopology`, `KatulongInstance`, `OllamaHost`, `TmuxSession`, `WireResponse` (the typed shape katulong returns from an HTTP/SSE call — distinct from the retired Experimentation `Outcome`; this is the post-deserialization-and-validation envelope that flows into Experimentation's gemma4 bridge as input)
- **Verbs**: `register_host`, `resolve_peer`, `dispatch_to_host`, `subscribe`, `attach`, `exec`

**Where it lives today.**

| Piece | Path | Status |
|---|---|---|
| Mesh / host config | `sipag-core/src/hosts.rs` (117), `~/.katulong/mesh.json` | 🟡 — overlapping responsibilities between sipag's `hosts.toml` and katulong's `mesh.json` (❓ from §9 carried over) |
| katulong WS attach + types | `katulong-client/src/attach.rs`, `protocol.rs` | ✅ |
| katulong HTTP (sync curl) | `katulong-client/src/http.rs` `KatulongClient` | 🟡 — used by CLI/TUI act path; works fine sync |
| katulong HTTP (async) for serve | ❌ not in client; sipag uses reqwest + URL builders | 🔴 — **#527 root cause**; queue item #1 |
| katulong SSE subscriber | ❌ not in client (only `sub_url()` builder exists) | 🔴 — queue item #2 |
| Ollama HTTP client | `sipag-core/src/llm.rs` | 🟡 — could promote to `ollama-client` crate (queue item #3) |
| Claude subprocess client | inline in `sipag-core/src/refine.rs` (the stream-json parser at lines ~181–516 is the reusable core: `Command::new("claude").args(["-p", "--output-format", "stream-json", ...])` + `BufReader::lines()` pumping `tool_use` events) | ⛔ (because refine.rs is deprecated); if Experimentation's `act` sub-module needs to spawn claude, lift that range deliberately rather than rehabilitate the module in place |
| Dispatch-policy helpers (worktree, agent command, session naming) | `katulong-client/src/http.rs:477-548` | 🟧 — sipag concepts in a wire crate; lift to Experimentation's `act` sub-module |
| Filed katulong upstream gaps | [Dorky-Robot/katulong#715](https://github.com/Dorky-Robot/katulong/issues/715), [#716](https://github.com/Dorky-Robot/katulong/issues/716) | ⏳ awaiting upstream |

---

## 5. Identity context (platform — auth)

**Responsibility.** Who is allowed to drive sipag? Devices, passkeys, setup tokens, browser sessions.

**Ubiquitous language.**

- **Nouns**: `User`, `Device`, `Passkey`, `SetupToken`, `AuthSession`, `AccessMethod`, `CredentialLockout`
- **Verbs**: `register_device`, `pair_via_setup_token`, `login`, `logout`, `revoke`

**Where it lives today.**

| Piece | Path | Status |
|---|---|---|
| Auth state + store + setup tokens + webauthn + sessions | `sipag-core/src/auth/` (~2045 LOC across 9 files: mod, credential, error, random, session, setup_token, state, store, webauthn) | 🟡 self-contained, mature |
| Serve-side handlers + middleware | `sipag/src/serve/{auth, auth_middleware, cookie, devices, login, tokens, access}.rs` (7 files) | 🟡 already decomposed reasonably |

**Extraction.** Candidate for `sipag-auth` (or `dorky-auth`) own crate. Lowest urgency — works fine, no open bugs.

---

## 6. Anti-corruption layers (translation between contexts)

DDD principle: when two contexts have different words for the same shape, the boundary translates. Today the boundaries are implicit (and leaky). Make them named.

### Naming disambiguation

| Today | Context | Rename to | Status |
|---|---|---|---|
| `Session` (in `katulong-client::http`) | Topology | `TmuxSession` | ✅ done 2026-05-17 (PR #537). `KatulongSession` rejected — the type literally models a tmux session, and leaving `KatulongSession` unclaimed makes room for the existing `sipag/src/serve/observers.rs::KatulongSession` (richer post-deserialization shape with `meta.*` fields the wire type drops) to formalize as a named sipag-side ACL when Phase 1 #3 lands. |
| `Session` (in `sipag-core::auth::session`) | Identity | `AuthSession` | 🟡 deferred — call sites import bare `Session` via `sipag_core::auth::{...}` (re-exported from `auth/mod.rs`), but no in-file collision with another `Session` today since auth and katulong-client types are never co-imported. Defer until a second `Session` lands in the same file or the auth crate is touched for other reasons. |
| Claude UUID (in `claude/<uuid>` topics) | Experimentation (observe input) | `ClaudeSession` | greenfield — no type today, just a `String` UUID. Use this name when a type appears. |
| `sipag-d-<hex>` dispatch session | Experimentation | `DispatchSession` | greenfield — no type today, just a name pattern from `generate_dispatch_session_name`. (Previously suggested folding into `Trial`; `Trial` was removed in the 2026-05-17 reframe — `DispatchSession` stands alone if it ever gets a type.) |
| `Status` (in `Project`, column name) | Steering | `ColumnName` or `BoardStatus` | 🟡 **deferred** — see §10 domain-vs-schema-noun question |
| `TaskStatus` | Experimentation / Board | TBD | 🟡 deferred until §10 domain-vs-schema-noun resolved. `WorkflowStatus` was suggested when `Trial` was on the roadmap; with `Trial` removed (2026-05-17 reframe), `TaskStatus` either stays as-is on `Task` or gets a new name in the recording-API/event-log shape. Decide alongside §10. |
| `SessionStatus` (alive / has-child) | Topology | `TmuxSessionStatus` | ✅ done 2026-05-17 |
| `KrStance` | Steering | already unambiguous ✓ | ✅ |

### Named ACLs

| From | To | What gets translated |
|---|---|---|
| Steering: `Idea` | Experimentation: `Experiment` (thin grouping) | `promote_idea` — the only way Steering crosses into Experimentation. May reduce to "tag the idea with a KR ref and watch the event log" if `Experiment` aggregate proves unnecessary. |
| Experimentation: `suggest_stance(...)` action | Steering: candidate `KrStance` update | The gemma4 bridge's `suggest_stance` recording API → surfaced in human UI as a proposed stance; **human confirms**. Sipag never auto-updates stance. |
| Topology: `TmuxSession` | Experimentation: dispatch reference (held by `Task` today) | dispatch returns a reference; Experimentation never holds the raw type. |
| Topology: katulong pub/sub events | Experimentation: gemma4 bridge input | **Two-stage ACL**: katulong `claude/<uuid>` events → sliding window in observe → gemma4 prompt → structured JSON action → recording API. Sipag never parses Claude-shaped data; only katulong-shaped events. See `[[feedback-strict-layer-coupling]]`. |
| Gemma4 structured output | Recording API call | The bridge's parse-and-dispatch step — gemma4 returns `{action: "note_progress", ...}`; sipag validates the schema and dispatches to the matching internal Rust function. |
| Topology: ollama response | Experimentation: gemma4 bridge | typed parsing inside `llm-client`; only validated types leave. This is the layer where the bridge's prompt-and-parse work happens. |

---

## 7. Cross-cutting

| Concern | Today | Direction |
|---|---|---|
| Error strategy | mix of `anyhow` + `thiserror` | typed errors at every crate boundary; `anyhow` inside sipag-side glue |
| Tracing | `tracing` everywhere | OK |
| Response-size caps | absent (the #527 vector) | install at wire-client level (Topology), not at call site |
| Trust-boundary validation | ad-hoc | every wire client returns typed values; validation lives in the parser, not the caller |

---

## 8. Migration map — current code → target context

| File / module today | Goes to | Notes |
|---|---|---|
| `sipag-core/src/board/objective.rs`, `key_result.rs` | Steering | stay |
| `sipag-core/src/board/project.rs` | split: Steering (Project as namespace, Standing) + Experimentation (Status, Role) | currently one file |
| `sipag-core/src/board/task.rs` | Board (stays) | Stays as `Task` — no `Trial` rename in the new shape (state machines were the wrong frame; see §3 reframe 2026-05-17). |
| `sipag-core/src/board/role.rs` | Experimentation (`act` sub-module's command template) | |
| `sipag-core/src/board/observation.rs` | Experimentation (`RecordedAction` companion or composed-with) | Close cousin of the new `RecordedAction` value object — either renames or composes. |
| `sipag-core/src/feature.rs` | ⛔ deprecate | preserve as "we tried this"; strip wiring |
| `sipag-core/src/refine.rs` | ⛔ deprecate | same |
| `sipag-core/src/gate.rs` | Experimentation `observe` | merge with nudge into one classifier |
| `sipag-core/src/nudge.rs` | Experimentation `observe` | merge with gate |
| `sipag-core/src/llm.rs` | Topology (as `ollama-client`); consumed by Experimentation `observe` | promote to crate |
| `sipag-core/src/pubsub.rs` | **stays for now** — load-bearing (16+ publish sites in `sipag/src/serve/`). Future decision: keep in-process broker, or route sipag's own topics into katulong's broker per `[[feedback-fix-at-right-layer]]`. See §10 + §9 #16. |
| `sipag-core/src/hosts.rs` | Topology | reconcile with katulong mesh.json |
| `sipag-core/src/config.rs` | Cross-cutting | stay |
| `sipag-core/src/auth/` | Identity | extract to `sipag-auth` (low priority) |
| `katulong-client/src/{attach, protocol, http (WS+SSE), serve}.rs` | Topology | stay |
| `katulong-client/src/http.rs` dispatch helpers | Experimentation `act` | lift out of wire crate |
| `sipag/src/serve/htmx.rs` (2169 🔴) | split by feature after wire clients + classifier extract | dispatch UI / observation UI / transcript proxy / recovery (the last gets deleted with attach-client wiring) |
| `sipag/src/serve/board_view.rs` (2276 🔴) | Steering's web surface | needs decomposition |
| `sipag/src/serve/katulong_proxy.rs` | dies | when async katulong HTTP client lands |
| `sipag/src/serve/workers/{expand,research,scheduler}.rs` (plus `mod.rs`) | ❓ Experimentation `observe` / `record` consumers, or their own thing? — `expand` / `research` look like background actors that consume the recording API; `scheduler` looks more cross-cutting. Triage individually. |
| `sipag/src/serve/categorize.rs` (199 LOC) | Experimentation `observe` | gemma-driven classification of board items — same trust boundary as the rest of `observe`. |

---

## 9. Extraction queue — three phases (structural language → bug-fix-driven → cleanup)

**Sequencing principle.** When a refactor combines vocabulary changes with behavior changes, the structural language lands *first*. Every behavior PR that ships in the old vocabulary entrenches it and inflates the eventual rename. Language-first means subsequent bug-fix and extraction PRs naturally write the new vocabulary, so the codebase migrates organically instead of needing a Big Bang rewrite.

The trade-off: Phase 1 PRs don't close open bugs. They earn their keep by making Phase 2 PRs smaller and self-consistent.

**Accepted risk:** Phase 2 bug fixes #6 (closes sipag #527) and #11 (closes sipag #528 by deletion) close *live security-adjacent vectors* (unbounded response body, unsafe LLM→PTY recovery loop). Strict serialization of Phase 1 before Phase 2 leaves both open longer than necessary. **Interleaving is permissible** once Phase 1 #1 + #2 (this PR + the mechanical naming pass) land — at that point the Topology context's wire vocabulary is settled enough that #6's new async HTTP client can ship in the right language without waiting for the rest of Phase 1. Items #3-#5 (Experiment aggregate, Idea ACL, Agent API types) are mostly Steering/Experimentation work and don't block Topology PRs.

### Phase 1 — structural language (front-loaded; no bugs closed yet)

1. **Deprecate `feature.rs` + `refine.rs`.** Experimentation. Strip wiring, add deprecation notes per `[[feedback-deprecate-with-rationale]]`. Stops the old kanban language from competing with the new. Lowest coupling, ships first.
2. **Naming disambiguation pass.** Cross-cutting. Mechanical rename per the §6 table — splitting overloaded `Session` and `Status` across contexts. Sized as multiple focused PRs (one per context) to keep diffs reviewable. **Partially landed 2026-05-17**: katulong-client's `Session` → `TmuxSession` and `SessionStatus` → `TmuxSessionStatus` done; auth's `Session` → `AuthSession` deferred (no in-file collision today; revisit when auth is touched — see §6 row for the full rationale); all `Status` renames deferred until §10 domain-vs-schema-noun question resolves; `ClaudeSession` / `DispatchSession` are greenfield names for types that don't exist yet.
3. **Recording API + gemma4 bridge dispatcher** (replaces the original Phase 1 #3 — see 2026-05-17 reframe in §3). Experimentation. Introduce: (a) the internal recording API as Rust functions (`note_progress`, `flag_blocker`, `propose_task`, `suggest_stance`, `ask_human`) — sipag-internal, **NOT exposed as MCP server or external endpoint** per `[[feedback-strict-layer-coupling]]`; (b) the `RecordedAction` value object, append-only into pub/sub; (c) the gemma4 dispatcher that takes a sliding window of katulong events, prompts gemma with the recording-API schema, parses the structured JSON response, and dispatches to the matching function. Optional thin `Experiment` aggregate (KR ref + hypothesis + log-filter tag) — start without it; add only if persistence buys something. **NO** `Trial`, `IteratePolicy`, `WorkflowStatus`, or `Outcome` types — those were state-machine framings the reframe retired.
4. **`Idea` aggregate + `promote_idea` ACL.** Steering ↔ Experimentation. First instance of a named cross-context translation; sets the pattern for future ACLs.
5. **Agent API published-language types.** Steering. Define the typed shapes for read-only KR/objective views and `report_stance` commands — *types only*, no endpoint wiring yet. Establishes the protocol so Phase 2 work can write toward it.

### Phase 2 — bug-fix-driven (now using the new vocabulary)

6. **katulong-client async HTTP client + body cap.** Topology. **Closes sipag #527** structurally. Shrinks htmx.rs and kills katulong_proxy.rs. ~200 LOC + 9-site migration. Speaks new Topology language (`TmuxSession`, `TmuxSessionStatus`).
7. **katulong-client SSE subscriber.** Topology. Third wire surface alongside WS attach + HTTP. Built against today's `claude/<uuid>` topic; gains `sessions/<id>/*` for free when katulong#715/#716 land. Feeds the gemma4 bridge (Phase 1 #3) — emits structured `KatulongEvent`s that the bridge's sliding window consumes. **This is the canonical replacement for the existing Demeter-violating Claude-transcript-proxy path** (`katulong-client::http::claude_transcript_url` + `serve/htmx.rs::observation_transcript_handler`) — once #7 lands, that path retires per `[[feedback-strict-layer-coupling]]`.
8. **Promote `llm.rs` to `ollama-client` with typed response shapes + `LlmClient` trait.** Topology. Sets up #528 closure by giving validated typed outputs a home. Sized for consumption by Experimentation's `observe`. **The crate exports an `LlmClient` trait** that the gemma4 bridge (Phase 1 #3) takes via `dyn LlmClient` injection — tests inject a canned-response mock, production wires the real ollama-backed impl. The same trait is reused by `gate.rs` and `nudge.rs` when they fold into `observe` (Phase 2 #9), so the seam ships with three consumers from day one.
9. **Unify gate + nudge into one observe/bridge module.** Experimentation. Depends on #3 (recording API + gemma4 dispatcher) + #8 (typed ollama client). Today they're parallel gemma-prompt-then-react paths; they should share the bridge plumbing.
10. **Event-driven observer.** Experimentation. Depends on #7 + #9. Shrinks dramatically when katulong#715/#716 land.
11. **Recovery deletion** (`verify_and_heal_dispatch`). Experimentation. **Closes sipag #528** by removing the unsafe seam. Happens when attach client wires in per dispatch-implementation-plan §7. *Existing planned work.*
12. **Lift dispatch policy** (session naming, worktree, agent command) out of katulong-client into Experimentation's `act` sub-module. Topology → Experimentation. Removes sipag concepts from wire crate; deduplicates CLI/TUI/serve.
13. **Split `htmx.rs` + `board_view.rs`.** Cross-cutting. After #6 + #11, what's left is route handlers + view helpers; split by feature (Steering vs Experimentation surfaces).

### Phase 3 — cleanup + extractions (no urgency)

14. **Agent API endpoint wiring.** Steering. Type-driven; types landed in Phase 1 (#5). The published-language types should drive the route shapes naturally.
15. **Promote `auth/` to its own crate** (`sipag-auth` or `dorky-auth`). Identity. Lowest urgency — works fine today, just big.
16. **Decide `pubsub.rs` future** (not its fate — load-bearing today). Topology. sipag's broker has 16+ publish sites internally; see §10. The decision is whether to keep an in-process broker or route sipag's own topics into katulong's broker per `[[feedback-fix-at-right-layer]]`. Defer until queue items #2 + #7 prove out the katulong-consumer side; the consolidate-vs-keep call is much easier with both ends working.

---

## 10. Open questions

- **§2 Agent API surface**: what's the auth model? Same passkey + cookie session as the human, or a separate service-account / bearer-token flow?
- **§3 bridge retry-with-reflection**: currently deferred ("cost; complexity"). When/whether to add reflection ("gemma, your last response didn't parse — here's the schema, try again") depends on production parse-failure rates. Open until we have telemetry.
- **§3 `ask_human` per-kind dedup windows**: defaults are 5min/1hr/24hr for `permission-style` / `progress-check` / `is-this-KR-still-alive`. These are starting points; should likely move to a config file once usage patterns are visible.
- **§3 trigger-blind-spot heuristic**: the "second-tier trigger" (20 events in 5 min + no RecordedAction → fire a gemma call) is a starting threshold. Need to validate it doesn't false-positive on legitimately quiet productive work.
- **§3 disk-backed outage buffer (future option)**: v1 commits to a 200-event in-memory sliding window per session, with explicit "older events dropped on the floor during gemma outage" behavior. A disk-backed slower buffer (replay-after-recovery) is a reasonable future option but adds: serialization format, on-disk schema, replay-ordering invariants, recovery dedup against actions emitted during the outage. Not v1. Re-litigate only if the operator-channel "N events dropped during outage" banner starts firing in production with N values that matter.
- ~~**§3 IteratePolicy plug shape**: trait with a `decide(experiment, outcome) -> NextAction` method, or richer (multi-step planning)?~~ **Resolved 2026-05-17:** `IteratePolicy` removed entirely in the §3 reframe. Claude is the iterator; sipag doesn't iterate.
- **§3 workers/**: relationship to `observe` / `record`. Are `expand`/`research`/`scheduler` background actors that consume the recording API, or a separate kind of work?
- **§4 hosts.rs vs mesh.json**: overlapping topology configs. One source of truth or two?
- **§4 pubsub.rs** — sipag's broker is **load-bearing**, not a deprecation candidate. Round-1 review claimed sipag was consumer-only based on a faulty grep; the actual state (verified 2026-05-17) is that `sipag/src/serve/` has 16+ `.publish(` call sites — `workers/{expand,research,scheduler,mod}.rs`, `htmx.rs` (6), `observers.rs`, `board_view.rs`, `ws.rs`. Topics include `workers/activity`, `observations/activity`, plus per-item `discourse_topic()` channels. The broker serves sipag's own intra-process consumers (UI updates, worker coordination). The real architectural question is whether sipag's internal topics should be **published into katulong's broker** instead of sipag running its own — per `[[feedback-fix-at-right-layer]]`. That would unify the pub/sub seam at the katulong layer but requires sipag's UI subscribers to round-trip through the network. Worth weighing, but not until queue items #2 + #7 land — first prove out the katulong consumer path so the consolidate-vs-keep call has both ends working.
- **§4 Topology context scope**: today §4 bundles mesh/host config + wire protocols + protocol shapes. These are different shapes ("Topology" feels like infra-config; "Wire" feels like client libraries). Should **Wire** be a sibling context to **Topology**, with `katulong-client` / `ollama-client` living under Wire and `hosts.rs` / mesh.json under Topology? Affects where the response-size cap conceptually lives (§7).
- **§6 ColumnName + TaskStatus + TmuxSessionStatus**: are these *domain nouns* or *wire/presentation schema nouns*? `ColumnName` smells like a UI presentation concern that might never need to appear in `sipag-core`; `TmuxSessionStatus` might be a wire response shape, not a domain type. Worth distinguishing "domain language per context" from "schema language per wire/UI seam" so we don't pollute domain modules with concerns that only matter at boundaries. (Previously this question also mentioned `WorkflowStatus` — that name was tied to the removed `Trial` aggregate; `TaskStatus` stays on `Task` for now.)
- **§6 type prefixes vs module paths**: for the types that DO exist (e.g., `RecordedAction`, `KrStance`), prefer module paths (`experimentation::RecordedAction` vs `steering::KrStance`) or short prefixes? More idiomatic Rust to use module paths; less visible at call sites.
- **Crate naming**: `sipag-experimentation` (project-coupled) or `dorky-experimentation` (mesh-shared)? Same question for auth, pubsub.

---

## 11. Edit log

- 2026-05-17 — initial draft (organized by code mechanics: wire clients / domain libs / intelligence).
- 2026-05-17 — reframed §3 from "dispatch intelligence layer" to event-driven session classifier + observer. Filed upstream katulong issues [#715](https://github.com/Dorky-Robot/katulong/issues/715) + [#716](https://github.com/Dorky-Robot/katulong/issues/716).
- 2026-05-17 — full rewrite around DDD bounded contexts (Steering / Experimentation / Topology / Identity). Folded former "Refinement" / "Execution" / "Sensing" into Experimentation per the RL/Edison work model (see memory: `project-sipag-work-model-experimentation`). Established naming disambiguation for overloaded `Session` / `Status`. Marked `feature.rs` + `refine.rs` as ⛔ deprecated (preserve, don't delete, per `feedback-deprecate-with-rationale`).
- 2026-05-17 — re-sequenced §9 into three phases (structural language first, then bug-fix-driven, then cleanup). Rationale: every PR that ships in the old vocabulary entrenches it. Front-loading language work means subsequent PRs migrate the codebase organically.
- 2026-05-17 — review-fix round on PR #536 — fixed URL typo (keglong → katulong), normalized memory name to `feedback-deprecate-with-rationale`, corrected LOC counts (feature/refine/auth), enumerated `serve/workers/` files, added `serve/categorize.rs` row, added Claude-subprocess breadcrumb to §4, closed pubsub open question with grep finding, opened topology-split and domain-vs-schema-noun questions in §10, acknowledged phase-ordering trade-off in §9 (interleaving permissible after #1 + #2 land), explicit TUI-as-third-dedup-site note.
- 2026-05-17 — review-fix round 2 on PR #536 — reverted the **factually wrong** pubsub resolution (sipag's broker has 16+ internal publish sites — it's load-bearing, NOT a deprecation candidate); reframed §10 + §9 #16 around the correct architectural question (in-process broker vs routing into katulong's broker). LOC drift round-2 (feature.rs 837→847, refine.rs 1364→1367 — the round-1 banner additions pushed them up again). Normalized the two memory references that still used the unprefixed form (`[[memory: deprecate-with-rationale]]` in the §0 legend and `[[fix-at-right-layer]]` in pubsub paragraphs).
- 2026-05-17 — Phase 1 #2 (partial): renamed `katulong_client::Session` → `TmuxSession` and `katulong_client::SessionStatus` → `TmuxSessionStatus` (plus the one external consumer in `sipag/src/serve/katulong_proxy.rs`). `KatulongSession` rejected in favor of `TmuxSession` — the type literally models a tmux session and `katulong_client::` already namespaces it. Deferred: auth's `Session` (no in-file collision today; revisit on touch), all `Status` renames (waiting on §10 domain-vs-schema-noun), `ClaudeSession` / `DispatchSession` (greenfield names for types not yet introduced). §6 table updated with per-row status; §9 #2 noted as partially landed.
- 2026-05-17 — review-fix round 1 on PR #537 — corrected the deferral rationale for auth's `Session` rename (call sites import bare `Session` via re-export, NOT the module-path-qualified form the prior wording claimed); annotated `sipag/src/serve/observers.rs::KatulongSession` as the informal sipag-side ACL distinct from the new `katulong_client::TmuxSession`, with a candidate-for-naming reference to Phase 1 #3 (originally `Outcome`; the §3 reframe later that day retired `Outcome` — the equivalent reference today is `RecordedAction`).
- 2026-05-17 — review-fix round 2 on PR #537 — round-1 rationale fix landed in §6 but the parallel sentence in §9 #2 still parroted the old "module path already disambiguates" wording. Updated §9 #2 to point at §6 for the full rationale.
- 2026-05-17 — **major §3 reframe**: Experimentation aggregates collapsed. State-machine framing (`Trial`, `IteratePolicy`, `WorkflowStatus`, `Outcome`-as-state-payload) retired entirely — the new shape is event log + gemma4 bridge + internal recording API + recorded actions. Driven by user's MCP-shaped framing: "more vibey and loose, like MCP." Followed by user's Demeter catch on a proposed sipag-as-MCP-server design — corrected to gemma4-as-bridge (sipag never reaches past katulong to Claude; gemma4 watches katulong events and dispatches structured JSON actions to internal recording API). Saved as memory `feedback-strict-layer-coupling`. Updates touched: §3 (responsibilities, language, aggregates, sub-responsibilities, what-lives-where, what's-missing), §6 ACL table (new gemma4-bridge two-stage ACL row, removed Trial/Outcome rows), §8 migration map (Task stays as Task, observation.rs reframed), §9 Phase 1 #3 rewritten + #7 cross-referenced + #9 dependency reframed, §10 IteratePolicy question resolved (no longer needed). Existing `claude_transcript_url` + `observation_transcript_handler` reframed as a 🔴 Demeter violation that retires when SSE subscriber lands.
- 2026-05-17 — review-fix round 1 on PR #538 — stale-vocab sweep (`iterate` → `record` reached §0/§1 prose + ASCII diagram + §8 workers row + §10 question); §4 Topology nouns renamed `WireOutcome` → `WireResponse` with clarifying parenthetical (the former collided with retired Experimentation `Outcome`); §3 sub-responsibilities expanded with **failure-mode policy** (malformed gemma JSON, recording-API call failures, sliding-window overlap dedup, `ask_human` fan-in, gemma unreachable) and **testability invariants** (pipeline of pure functions + injectable PubSubSink — matches `gate.rs::parse_decision`/`render_user_prompt` template); switched the observation trigger from "periodic" to "threshold-crossing events" with cost rationale; added §1 forward link to feature-matrix.md; cli-reference.md tombstone phrasing updated `iterate` → `record`; edit-log entry for PR #537 round-1 updated to note `Outcome` → `RecordedAction` rename.
- 2026-05-17 — review-fix round 2 on PR #538 — the round-1 §3 failure-mode policy created its own engineering issues that round 2 caught (1 HIGH, 4 MEDIUM, 2 LOW). All addressed: **gemma-unreachable** no longer silent — explicit `Degraded` status, `bridge_unavailability_seconds` counter, sticky operator-channel banner, sliding-window bounded at 200 events (older events dropped during outage, called out explicitly); **dedup key** corrected from source-event seq range to `(kind, kr_id, payload-content-hash)`; **parse/dispatch failure thresholds** committed (5 consecutive or 10-in-50-window); **`ask_human` per-kind windows** specified (5min permission / 1hr progress-check / 24hr KR-alive) with config-file deferral noted in §10; **operator-vs-steering channel separation** introduced — bridge confusion is operator-channel, KR-relevant signals are steering-channel; **trigger blind-spot mitigation** added (second-tier trigger: 20 events in 5min + no RecordedAction → one gemma call); **`dyn LlmClient` injection** locked in for testability and Phase 2 #8 inheritance; **sliding window as `&[KatulongEvent]` value** (not trait) for the snapshot. §10 grew three new open questions (retry-with-reflection, per-kind window config, blind-spot heuristic validation).
- 2026-05-17 — review-fix round 3 on PR #538 — engineering-precision pass: §3 dedup-key gained a **schema invariant** ("don't include window-positional fields in the hashed payload — otherwise dedup silently fails open"); trigger-blind-spot mitigation gained a **schema invariant** ("must include `no_action_warranted` response variant so quiet-but-productive sessions yield zero `RecordedAction`s, not placeholders"); operator-channel placement clarified — it renders inside Experimentation's own UI surface (no Steering ACL crossing, hence no §6 row); §3 sink-injection ambiguity disambiguated as "**concrete typed wrappers around a shared `dyn PubSubSink` injection**" (one sink, two newtype wrappers); §9 Phase 2 #8 updated to commit to exporting the `LlmClient` trait that §3's testability invariant references; §10 grew a fourth entry — "disk-backed outage buffer (future option), not v1."
