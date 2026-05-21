# sipag modules — domain map

> *Companion to [`narrative.md`](narrative.md): that doc is the customer-facing product story; this is the architecture that implements it (DDD bounded contexts, lens-worker abstraction, phase queue).*

**Status:** working draft, iterating collaboratively.
**Frame:** [`narrative.md`](narrative.md) → product story. [`../VISION.md`](../VISION.md) → strategic principles. This doc → how the codebase carries them.

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

**Steering entries are also lens definitions** (set 2026-05-17 — see §3 reframe). Each Objective / KR / Standing item / Idea / named pattern is *both* a human-surface artifact AND the system prompt of a lens-worker in the Experimentation context. The text the human writes IS the worker's prompt; editing the entry changes the worker's behavior. This dual role is the load-bearing seam where strategic direction becomes configured agent perspective. See §6 ACL table for the formal translation.

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

**Responsibility.** Watch what's happening (katulong events, the project's KR context, prior corpus content), derive insights through configured **lenses**, write them back into a shared corpus, surface KR-relevant signals to the human's Steering surface. **Claude itself is the iterator** — sipag doesn't decide "what to try next." Sipag observes, derives, and records.

**Strict layer coupling (set 2026-05-17 — see `[[feedback-strict-layer-coupling]]`).** Sipag is two hops from Claude (`Claude → katulong → sipag`). Sipag never reaches past katulong. The bridge between Claude's free-form output and sipag's structured world is **gemma4 on sipag's side**, watching katulong's `claude/<uuid>` events. Sipag does NOT expose itself as an MCP server installed into projects' `.claude/` — that would couple Claude to sipag.

**Core abstraction — the lens-worker.** A lens-worker is a gemma4 invocation with:

- a **lens definition** (free-form text — the worker's system prompt);
- a **trigger policy** (schedule + threshold + model-decide, see below);
- a **query strategy** (what to pull from the corpus when running);
- a **write protocol** (every insight tagged with at minimum `lens=<entry-name>`, plus context tags like `kr_ref`, `session`, `valence`, `confidence`).

**Every Steering entry IS a lens.** Each Objective / KR / Standing item / Idea / named pattern is *both* a human-surface artifact AND the system prompt of a lens-worker. Add a KR → a worker spawns. Edit the Objective text → the worker's behavior changes immediately. Retire the entry → the worker retires per `[[feedback-deprecate-with-rationale]]`. The Steering surface IS the lens registry; there's no parallel configuration. Plus **project-level meta-lenses** that don't hang off a specific Steering entry (pattern-spotter, meta-cognitive, strategic-cross-cutting) and **ad-hoc/hypothesis lenses** (short-lived; promote-or-expire).

**The gemma bridge is the first lens-worker.** Its lens text: "observe what's happening in this active session and surface anything KR-relevant." Its trigger: new katulong event (most reactive worker). Its input: sliding window of `claude/<uuid>` events + retrieved relevant prior corpus content. Its output: `observe(...)` writes to the corpus plus (rarely) typed structural-verb calls. Other lens-workers are **siblings of the bridge** — same plumbing, different triggers (scheduled / threshold / model-decide instead of event-driven) and different inputs (corpus query instead of event stream).

**Ubiquitous language.**

- **Nouns**: `Lens`, `LensWorker`, `Corpus`, `CorpusItem`, `Tag`, `Embedding`, `Hypothesis`
- **Verbs** (the bridge AND every lens-worker invoke these via structured JSON):
  - `observe(text, tags?, source_refs?)` — **the workhorse**: gemma writes free-form prose, sipag embeds + appends to the corpus, tagged with at minimum the worker's `lens=<entry-name>`
  - `suggest_stance(kr_id, stance, reason)` — typed action; surfaces as proposed `KrStance` update in KR sidebar; **human confirms**, sipag never auto-updates stance
  - `ask_human(question, kr_ref?, kind)` — typed; surfaces as `steering-question` sidebar item; deduped per kind
  - `propose_task(kr_id, title, hypothesis?)` — typed; creates a `Task` on the board
- **NOT in the language**:
  - kanban verbs (`refine_features`, `group_features`, `move_task`) — deprecated since PR #536
  - state-machine verbs (`transition_to`, `complete_trial`, `set_workflow_status`) — no state machine; the event log + corpus are the source of truth
  - per-classification recording-API verbs (`note_progress`, `flag_blocker`, `record_decision`, etc.) — collapsed into free-form `observe(...)`; classification happens at *query time* via tag filters + semantic search, not at write time
  - MCP tools exposed to Claude — Demeter violation

**Aggregates / value objects.**

- `CorpusItem` (value object): `content: String, embedding: Vec<f32>, tags: Vec<String>, timestamp, source_refs: Vec<ItemId>, generation: u8` (0 = raw observation; N = derived from generation N-1 items). **Append-only, forever.** Tags + timestamps + semantic search are how it's sliced.
- `Lens` (root): `name, prompt_text, source: SteeringEntry(ref) | ProjectMeta | AdHoc, model: ModelChoice, trigger: TriggerPolicy, last_ran, retired: bool`. Comes into existence when a Steering entry is added, when a project-meta-lens is registered, or when a human creates an ad-hoc lens via the web UI.
- `ModelChoice` (value object): `Default | Named(String) | Profile(Profile)` — per-lens model selection. `Default` falls back to `OLLAMA_MODEL` env var. `Named` pins to a specific model. `Profile` decouples the lens definition from concrete model names; the implementation maps profiles to models per environment via `~/.sipag/models.toml`.
- `Profile` (enum): `Fast | Strong | CodeAware`. Semantic tier rather than specific model. The same lens definition runs against different concrete models in dev / prod / a cheaper machine — implementation picks the model from the profile-to-model mapping at lens-worker spawn time.
  - **Fast** — bridge-tier workers (the gemma bridge, per-KR lens-workers). High frequency, threshold-driven, must feel real-time (~5-7s/call budget). Maps to small fast local models (e.g. `gemma4:latest`).
  - **Strong** — derivation-tier workers (strategic, meta-cognitive, pattern-spotter). Runs daily / on threshold; quality > latency (~20-30s/call OK). Maps to large local models (e.g. `gemma4:31b`).
  - **CodeAware** — workers that read diffs / source / commits. Maps to code-tuned models (e.g. `qwen2.5-coder:7b` or `qwen3-coder:30b`).
- `LensWorker` (runtime concept): consumes a `Lens` + the corpus + (for the bridge worker) the live event stream → produces `CorpusItem`s + occasional typed structural-verb calls.
- **NO** `Trial`, `Experiment`, `IteratePolicy`, `WorkflowStatus`, or `RecordedAction`-as-state-payload. Those were state-machine framings; the corpus + tags + lens registry replace them. The thin `Experiment` aggregate from the previous reframe also goes — a KR (already a Steering entry, already a lens) does the grouping job.

**Sub-responsibilities.**

- **act** — kick off whatever the human or agent is going to do. Today: katulong session create + agent command exec + worktree setup. Unchanged in this reframe.
- **observe** — the bridge lens-worker watches katulong events, prompts gemma with sliding window + KR context + retrieved relevant prior corpus content, writes free-form `observe(...)` and (rarely) typed structural-verb calls. Reactive trigger; runs whenever new events arrive on its threshold-crossing rule.
- **derive** — non-bridge lens-workers (one per Steering entry + project-meta lenses + ad-hoc) run on the **hybrid trigger** below. Each queries the corpus through its lens, prompts gemma to derive insights, writes them back tagged. Higher-order insights compound: a generation-2 insight derives from generation-1 + raw observations; a meta-cognitive lens can derive generation-3 insights from prior derivations. Provenance via `source_refs` makes derivation chains reconstructable.
- **surface** — KR detail view renders insights tagged with that KR's ref, ranked by recency × semantic similarity to the KR's text. A separate **Lenses** panel in the web UI shows the project-meta + ad-hoc lenses and lets humans add / edit / retire them.

**Trigger model (hybrid).**

```
schedule:     run at minimum cadence (e.g., daily floor for strategic lens, weekly for meta-cognitive)
threshold:    run when N new corpus items tagged with my lens-relevant tags have appeared
model_decide: each worker can introspect — "is there enough novel content since I last ran
              that another run would be productive?" — and re-trigger itself
```

v1 ships schedule + threshold. Model-decide is the second-tier elaboration — wire when the basic loop is observable in production.

**Failure-mode policy (sub-responsibility-level invariants).**

These are the places the implementer will invent a policy on day one if the doc doesn't commit to one. Each invariant is testable (see Testability invariants below):

- **Malformed gemma4 JSON / schema violation** — drop the action, increment a per-session `bridge_parse_failures` counter, log at WARN. Don't retry-with-reflection in the first cut (cost; complexity — see §10 for the deferred option). Threshold for escalation: **5 consecutive parse failures or 10 parse failures in a 50-event window** routes to the operator channel (NOT `ask_human` — see operator-vs-steering separation below). The window keeps moving regardless; gemma4 sees fresh context on the next threshold trigger.
- **Recording-API call fails** (validated JSON but e.g., `kr_id` doesn't exist) — same: drop, counter (`bridge_dispatch_rejections`), log. Reject the action; don't auto-create the missing entity. Same threshold rule applies; same operator-channel routing.
- **Dedup logic (split by call type)**:
  - **Free-form `observe(...)` writes** are dedup'd **per-worker** by semantic-similarity threshold (default: cosine ≥ 0.92 against the worker's own last 20 outputs). Above threshold → drop. **Do NOT dedup across workers** — different lenses observing the same fact yield distinct framings; both are valuable, both stay. That's the whole point of the lens-worker abstraction.
  - **Typed structural-verb writes** (`suggest_stance`, `ask_human`, `propose_task`) dedup'd by `(kind, kr_id, payload-content-hash)` over a 60-second window. Source-event seq range is recorded on each call for debugging but is **not** the dedup key. **Schema invariant for the implementer**: the structural-verb output schemas must not include window-positional fields (timestamps, seq numbers, "as-of-N-seconds-ago" phrasings) in the hashed payload — those go in the debug-only source-event-window field on the call record. Otherwise hash diverges across windows for the same logical action and dedup silently fails open.
- **`ask_human` fan-in semantics** — dedupe by `(session_id, kr_id, kind, normalized-question-text-hash)`. Default window per kind: **5 min for `permission-style` questions, 1 hour for `progress-check` questions, 24 hours for `is-this-KR-still-alive` questions.** See §10 — these defaults are starting points; per-kind tuning is consumer policy and will likely move to a config.
- **Operator-vs-steering signal separation** — the bridge has TWO escalation channels:
  - **Operator channel** (new — sidebar item with `kind: operator-alert`) for "sipag is misfiring" signals: parse-failure threshold, dispatch-rejection threshold, gemma-unreachable status. Audience is the human-as-sipag-operator. **This channel renders inside Experimentation's own UI surface — there is no Steering ACL crossing** (the operator-channel never escalates into the human's KR-stance world). That's why §6 ACL table doesn't carry a row for it: the channel is Experimentation-internal.
  - **`ask_human` channel** (sidebar item with `kind: steering-question`) for genuine "the work needs your input" signals from the bridge's derivation. Audience is the human-as-steerer. **This one DOES cross into Steering** — it appears in the KR sidebar where the human is already looking for "is this KR working." Catalogued in §6 ACL table.
  Conflating these into one sidebar makes both noisier than they need to be (and conflates VISION's two questions with a third).
- **Gemma4 unreachable** — bridge marks status as `Degraded` (NOT silent), increments a `bridge_unavailability_seconds` counter, and surfaces a sticky **operator-channel banner** ("auto-observer offline — last seen N min ago"). The sliding-window event stream is **bounded at 200 events per session** (configurable); during an outage longer than what 200 events worth of activity covers, **older events are dropped on the floor** — gemma4 sees only the most recent 200 when it comes back, not the full backlog. This is a deliberate trade-off (bounded memory) and the doc commits to it explicitly so the operator-channel banner can also surface "auto-observer caught up; N events dropped during outage" when ollama returns. Do NOT fall back to LLM-free derivation (separate path with separate failure modes).

**Trigger-blind-spot mitigation (bridge worker).** Threshold-crossing-only triggers (`permission-request`, `reply` containing terminal-phrase patterns, silence > N seconds) miss the "steady low-signal session" case — a session emitting continuous `reply` events that never trip a pattern looks healthy but generates no observations. Mitigation: **second-tier trigger** — if a session has had ≥20 events in the last 5 minutes AND no corpus write from the bridge in the same window, fire one gemma4 call regardless of pattern matching. Cheap (caps at one extra call per 5 min per active session) but defends against the silent-success blind spot. **Schema invariant**: the bridge's structured-output schema must include an explicit `no_action_warranted` response variant. The second-tier call exists to *ask* gemma whether there's a signal; it must not pressure gemma to emit a placeholder observation just to satisfy being called. A quiet-but-productive session should yield zero corpus writes, not a low-signal one.

**Testability invariants** (Phase 1 #3 must preserve):

- Bridge dispatcher is a pipeline of pure functions + one async LLM call: `(window: &[KatulongEvent], ctx: &KrContext) → render_prompt → llm_client.call → parse_action → dispatch_to_recording_api`. The pure halves (`render_prompt`, `parse_action`) are unit-testable without a live ollama, matching the `gate.rs::render_user_prompt` + `gate.rs::parse_decision` template that's already proven out (see `sipag-core/src/gate.rs:123, 153` and the test module at `:272+` for the live example).
- Bridge (and every lens-worker) takes a **`dyn LlmClient`** (the trait emerging from the Phase 2 #8 typed ollama client work). Tests inject a mock that returns canned structured-JSON responses; production wires the real ollama-backed client **with the lens's `ModelChoice` resolved to a concrete model name at construction time**. This seam is reusable across the bridge AND the surviving `gate.rs` flow when it folds into `observe` (Phase 2 #9) — locking it in here means Phase 2 #8 ships a consumer that already speaks the trait. (`nudge.rs` was the obvious sibling here when this section was written; it retired in §9 #11 since the WS-attach path doesn't need post-dispatch keystroke retry.) **Each lens-worker gets its own `LlmClient` instance configured for its model** rather than a single global one — that's how per-lens model choice composes through the test seam.
- Sliding window is **passed as a plain `&[KatulongEvent]` slice**, not a `dyn SlidingWindow` trait. The window's *maintenance* (push, expire, content-hash dedup state) is the testable thing; the *snapshot at gemma-call time* is plain data. No trait needed for the snapshot half.
- Structural-verb functions take an injectable `dyn PubSubSink` (or equivalent) so tests construct a mock sink and assert what gets appended.
- **Operator-channel and `ask_human` channel are concrete typed wrappers around a shared `dyn PubSubSink` injection** — i.e., one sink injection per bridge instance, with two newtype-style wrappers (`OperatorSink(&dyn PubSubSink)` and `SteeringSink(&dyn PubSubSink)`) that constrain what topic each can publish to. Tests inject one mock sink and assert *which wrapper* published *which payload* — cleaner than two separate `dyn` injections that would have to be wired identically.
- **Lens workers (the non-bridge ones) take `dyn CorpusReader` + `dyn CorpusWriter`** so tests construct an in-memory mock corpus and assert what got read and written. The bridge worker takes the same writer trait plus the structural-verb sinks above.
- **Trigger policies are pure functions** over `(LensState, CorpusDelta) -> ShouldRun` so test cases pin trigger semantics without time-based flakiness.
- **`Lens` resolution** — given a Steering entry ref, the lens's `prompt_text` is read directly from the entry. Tests inject a `dyn LensRegistry` that maps refs to text; production wires the real Steering store.

**Where it lives today.**

| Piece | Path | Status |
|---|---|---|
| Task | `sipag-core/src/board/task.rs` | 🟡 — stays as a board-level concept (a task on the board, dispatched to a session). NOT renamed to `Trial`. The `propose_task` structural verb writes into this. |
| Observation aggregate | `sipag-core/src/board/observation.rs` | 🟧 — close cousin of `CorpusItem`; either renames or composes once the corpus lands |
| Claude transcript proxy | `sipag/src/serve/htmx.rs::observation_transcript_handler` + `katulong-client::http::claude_transcript_url` | 🔴 **Demeter violation** — sipag parsing Claude-shaped JSONL through a katulong proxy. Retires when the SSE subscriber lets sipag consume katulong's `claude/<uuid>` topic events instead. See `[[feedback-strict-layer-coupling]]`. |
| Role (agent command template) | `sipag-core/src/board/role.rs` | 🟡 — Experimentation `act` infrastructure |
| Pre-dispatch classifier ("gate") | `sipag-core/src/gate.rs` (346 LOC) | 🟧 **early lens-worker prototype** — gemma reads pane + project statuses, derives a classification. Folds into the lens-worker abstraction (Phase 1 #3): becomes a worker whose lens text says "given current pane state, classify against the project's dispatchable statuses." |
| ~~Post-dispatch observer~~ | ~~`sipag-core/src/nudge.rs` (417 LOC)~~ | ✅ **deleted** in §9 #11 (the only consumer was the recovery loop, which retired in the same PR). The "post-dispatch progress" lens lives in the lens-worker abstraction (Phase 1 #3) instead. |
| Categorize loop | `sipag/src/serve/categorize.rs` (199 LOC) | 🟧 **early lens-worker prototype** — gemma reads board items + KRs, categorizes. Folds in as a worker with a "board-item categorization" lens. |
| ~~Recovery loop (`verify_and_heal_dispatch`)~~ | ~~`sipag/src/serve/htmx.rs`~~ | ✅ **deleted** in §9 #11 (closes sipag #528 by deletion). The WS-attach path now owns keystrokes; no more LLM-emitted bytes reach the PTY by design. |
| LLM / gemma client | `sipag-core/src/llm.rs` (300 LOC) | 🟡 — Experimentation infrastructure; will export the `LlmClient` trait that lens-workers (including the bridge) inject (Phase 2 #8). |
| Dispatch mechanics (CLI + TUI paths) | `sipag/src/cli.rs`, `tui/src/board_app.rs:353` (uses sync `KatulongClient::from_remote_json()` + calls `katulong::{session_name, worktree_command, agent_command}` inline — the exact dispatch-policy helpers §9 #9 lifts) | 🟧 — the `act` sub-module; **TUI is the third dedup site** alongside CLI and serve |
| Dispatch mechanics (web path) | `sipag/src/serve/htmx.rs` + URL builders | 🔴 — same act surface duplicated, plus the #527 unbounded-body vector |
| Refinement pipeline | `sipag-core/src/feature.rs` (847), `sipag-core/src/refine.rs` (1367) | ⛔ **deprecated** — kanban-shaped; replaced by Experimentation. Don't delete; preserve as "we tried this" per `[[feedback-deprecate-with-rationale]]`. |
| Background workers (expand, research, scheduler) | `sipag/src/serve/workers/{expand,research,scheduler}.rs` (plus `mod.rs`) | ❓ likely become **scheduler infrastructure for lens-workers** — `scheduler` runs the trigger machinery; `expand` / `research` may themselves be lens-workers or may be consumers of the structural verbs. Triage individually once the lens-worker abstraction lands. |

**What's missing.**

- **Corpus** — local vector DB (sipag-internal, embedded via ollama, append-only forever, sliced by tags + timestamps + semantic search). New infrastructure; sipag does not have this today. Sibling to diwa, not absorbed by it (per §10 decision 2026-05-17).
- **Lens registry** — mapping from `Lens` (Steering entry ref / ProjectMeta name / AdHoc id) → live `LensWorker`. Bootstrap: every existing Steering entry on project load. UI: web panel for project-meta + ad-hoc lenses.
- **LensWorker runtime** — schedules + triggers + executes a lens-worker invocation: query corpus, prompt gemma, parse structured JSON, dispatch to `observe(...)` / structural verbs.
- **Bridge worker** (the first lens-worker) — subscribes to katulong's `claude/<uuid>` topic, maintains sliding window, fires on threshold-crossing events.
- ~~**Corpus search tool** (`corpus.search(query, top_k, filter_tags?, time_window?)` + `corpus.expand(item_id)`) — sipag-internal MCP-shape that gemma can call mid-prompt for multi-step retrieval.~~ ✅ landed in `sipag-lens` (PR #556 — `execute_corpus_search` / `execute_corpus_expand` + `LensWorker::run_with_tools` multi-turn loop).
- **Structural verb implementations** (`observe`, `suggest_stance`, `ask_human`, `propose_task`) — Rust functions, sipag-internal, with the dedup invariants from the failure-mode policy section.
- **NOT needed anymore** (struck from previous plans): `Trial` aggregate, `IteratePolicy` trait, `WorkflowStatus` enum, `Outcome`-as-state-payload, the per-classification recording-API verbs (`note_progress`, `flag_blocker`, etc.). The corpus + tags + lens registry replace state-machine state; free-form `observe(...)` replaces categorical recording verbs.

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
| **Model profile config** | ⏳ `~/.sipag/models.toml` (new — Phase 1 #3) | 🔴 not implemented. Maps `Profile` (Fast / Strong / CodeAware) → concrete model name. Lens-workers read it at spawn time when resolving `ModelChoice`. Allows the same lens definitions to ride different models per environment. |
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
| Steering: `Objective` / `KR` / `Standing` / `Idea` / named pattern (text) | Experimentation: `Lens.prompt_text` for a `LensWorker` | **Steering entries ARE lenses.** The entry's text is read directly as the worker's system prompt. Add a Steering entry → spawn a lens-worker. Edit the text → worker behavior changes. Retire the entry → worker retires per `[[feedback-deprecate-with-rationale]]`. This is the load-bearing seam where the human's strategic direction becomes the agent's configured perspective. |
| Steering: `Idea` | Experimentation: ad-hoc `Lens` (then promotable to Objective/KR) | `promote_idea` — variant on the above. An Idea becomes an ad-hoc lens initially (short-lived, promote-or-expire); if it proves load-bearing, the human promotes it to an Objective or KR, at which point its lens-worker reclassifies as Steering-derived. |
| Experimentation: lens-worker `observe(...)` writes | Steering: KR detail view (semantic feed) | The KR sidebar renders `CorpusItem`s tagged with the KR's ref, ranked by recency × semantic similarity. This is how derived insights flow back up to the human surface without crossing through a typed verb. |
| Experimentation: `suggest_stance(...)` action | Steering: candidate `KrStance` update | A lens-worker's `suggest_stance` call → surfaces in KR sidebar as proposed stance; **human confirms**. Sipag never auto-updates stance. |
| Experimentation: `ask_human(...)` action | Steering: KR sidebar `steering-question` item | A lens-worker's `ask_human` call → surfaces in the KR sidebar (deduped per the per-kind windows in §3 failure-mode policy). Distinct from the `operator-alert` channel, which renders inside Experimentation's own UI and never crosses into Steering. |
| Experimentation: `propose_task(...)` action | Steering: new `Task` on the board | A lens-worker's `propose_task` call → creates a Task on the project board. Human can edit/dismiss. |
| Topology: `TmuxSession` | Experimentation: dispatch reference (held by `Task` today) | dispatch returns a reference; Experimentation never holds the raw type. |
| Topology: katulong pub/sub events | Experimentation: bridge lens-worker input | **Two-stage ACL**: katulong `claude/<uuid>` events → sliding window in the bridge worker → gemma4 prompt → structured JSON → `observe(...)` write into corpus AND optional structural-verb calls. Sipag never parses Claude-shaped data; only katulong-shaped events. See `[[feedback-strict-layer-coupling]]`. |
| Gemma4 structured output | `observe(...)` / structural-verb call | Each lens-worker's parse-and-dispatch step — gemma returns `{call: "observe", text: "...", tags: [...]}` or a structural-verb shape; sipag validates the schema and dispatches. |
| Topology: ollama response | Experimentation: lens-worker prompt loop | typed parsing inside `llm-client`; only validated types leave. This is the layer where every lens-worker's prompt-and-parse work happens. |
| Corpus (Experimentation-internal) | Lens-workers via `corpus.search` / `corpus.expand` | Sipag-internal MCP-shape tools that lens-workers (including the bridge) call mid-prompt for multi-step retrieval — gemma can pull relevant prior `CorpusItem`s into its current context. |

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
| `sipag-core/src/gate.rs` | Experimentation `observe` | fold into the lens-worker abstraction (§9 #3 / §9 #9) — `nudge.rs` was deleted in §9 #11, so there's no longer anything to merge with |
| ~~`sipag-core/src/nudge.rs`~~ | ✅ deleted in §9 #11 | the only consumer (`verify_and_heal_dispatch`) retired in the same PR |
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

> See [`extraction-plan.md`](extraction-plan.md) for the **crate-shaped view** of this same work — which lego blocks come out, in what order, and what each one owns. This section keeps the capability sequencing; that doc keeps the module boundaries.

**Sequencing principle.** When a refactor combines vocabulary changes with behavior changes, the structural language lands *first*. Every behavior PR that ships in the old vocabulary entrenches it and inflates the eventual rename. Language-first means subsequent bug-fix and extraction PRs naturally write the new vocabulary, so the codebase migrates organically instead of needing a Big Bang rewrite.

The trade-off: Phase 1 PRs don't close open bugs. They earn their keep by making Phase 2 PRs smaller and self-consistent.

**Accepted risk:** Phase 2 bug fixes #6 (closes sipag #527) and #11 (closes sipag #528 by deletion) close *live security-adjacent vectors* (unbounded response body, unsafe LLM→PTY recovery loop). Strict serialization of Phase 1 before Phase 2 leaves both open longer than necessary. **Interleaving is permissible** once Phase 1 #1 + #2 (this PR + the mechanical naming pass) land — at that point the Topology context's wire vocabulary is settled enough that #6's new async HTTP client can ship in the right language without waiting for the rest of Phase 1. Items #3-#5 (Experiment aggregate, Idea ACL, Agent API types) are mostly Steering/Experimentation work and don't block Topology PRs.

### Phase 1 — structural language (front-loaded; no bugs closed yet)

1. **Deprecate `feature.rs` + `refine.rs`.** Experimentation. Strip wiring, add deprecation notes per `[[feedback-deprecate-with-rationale]]`. Stops the old kanban language from competing with the new. Lowest coupling, ships first.
2. **Naming disambiguation pass.** Cross-cutting. Mechanical rename per the §6 table — splitting overloaded `Session` and `Status` across contexts. Sized as multiple focused PRs (one per context) to keep diffs reviewable. **Partially landed 2026-05-17**: katulong-client's `Session` → `TmuxSession` and `SessionStatus` → `TmuxSessionStatus` done; auth's `Session` → `AuthSession` deferred (no in-file collision today; revisit when auth is touched — see §6 row for the full rationale); all `Status` renames deferred until §10 domain-vs-schema-noun question resolves; `ClaudeSession` / `DispatchSession` are greenfield names for types that don't exist yet.
3. **Lens-worker abstraction + Corpus + corpus-search tools + bridge as first worker** (third rewrite — see 2026-05-17 reframes in §3). Experimentation. The biggest single piece of work; the foundation everything observation-related sits on. **⚙️ PARTIALLY LANDED (substrate + tool wrappers + scheduler)**: the *substrate* shipped 2026-05-20 — `sipag-corpus` (PR #550, storage + Embedder trait + BridgeEmbedder), `sipag-lens` (PR #551, Lens + ModelChoice + four verbs + LensWorker runtime), `ollama-bridge-client` (PR #549, queue-aware wire client); the **`corpus.search` / `corpus.expand` MCP-shape tool wrappers** plus `LensWorker::run_with_tools` multi-turn loop landed in PR #556 (gemma can call the two tools mid-prompt, and observations written through the tool path are embedded so subsequent searches find them); the **scheduler loop** landed in `sipag/src/serve/lens_scheduler.rs` (Schedule trigger supported; Threshold + ModelDecide surface in `skipped_unsupported` telemetry until the corresponding event sources land). Wire into `sipag serve` with `--lens-scheduler`; lens registry lives at `~/.sipag/lenses/*.toml`. What's still pending: the **bridge lens-worker concrete instance** (the first reactive trigger on katulong `claude/<uuid>` events) — needs §9 Phase 2 #7 (SSE subscriber) or a polling fallback. Introduce:
   - **Corpus** — local vector DB (sipag-internal, embedded via ollama, append-only forever, tagged + timestamped, sibling to diwa not absorbed by it).
   - **`Lens` + `LensWorker` abstraction** — runtime that takes a lens definition (text), a `ModelChoice` (default / named / profile-tier — see §3), a trigger policy (schedule + threshold + model-decide), a query strategy, and a write protocol; produces `CorpusItem` writes + occasional typed structural-verb calls. Each worker is wired with its own `LlmClient` configured for the resolved model — bridge tier gets `Fast`, derivation tier gets `Strong`, code-aware lenses get `CodeAware`.
   - **`~/.sipag/models.toml`** — small config mapping `Profile` → concrete model name per machine. Lets the same lens definitions run on a workstation, a CI box, and a low-spec laptop without rewriting lenses.
   - **Lens registry** — every Steering entry (Objective / KR / Standing / Idea / named pattern) becomes a lens; plus project-meta lenses (pattern-spotter, meta-cognitive, strategic-cross-cutting); plus ad-hoc lenses with promote-or-expire policy.
   - **Bridge lens-worker** (the first lens-worker; reactive trigger on katulong `claude/<uuid>` events) — sliding window of 200 events per session, threshold-crossing-only triggers plus second-tier blind-spot mitigation, schema-invariant payloads.
   - **`corpus.search` + `corpus.expand` tools** — sipag-internal MCP-shape, called by gemma mid-prompt for multi-step retrieval.
   - **Four verbs** — `observe(text, tags?, source_refs?)` (the workhorse, free-form, embedded into the corpus) + `suggest_stance` / `ask_human` / `propose_task` (three typed structural verbs that drive UI affordances).
   - **NOT** the per-classification recording verbs (`note_progress`, `flag_blocker`, etc.) — those collapsed into free-form `observe(...)`. **NOT** `Trial` / `IteratePolicy` / `WorkflowStatus` / `Outcome`-as-state-payload — state-machine framings the reframes retired.
4. **`Idea` aggregate + `promote_idea` ACL.** Steering ↔ Experimentation. First instance of a named cross-context translation; sets the pattern for future ACLs.
5. **Agent API published-language types.** Steering. Define the typed shapes for read-only KR/objective views and `report_stance` commands — *types only*, no endpoint wiring yet. Establishes the protocol so Phase 2 work can write toward it.

### Phase 2 — bug-fix-driven (now using the new vocabulary)

6. **✅ DONE (PR #546) — katulong-client async HTTP client + body cap.** Topology. Closed sipag #527 structurally. Shrank htmx.rs and killed katulong_proxy.rs. Speaks new Topology language (`TmuxSession`, `TmuxSessionStatus`). Body cap is per-call streaming with `DEFAULT_BODY_CAP=1MiB` / `TRANSCRIPT_BODY_CAP=10MiB`; aborts before the full body buffers, so a misbehaving katulong can no longer OOM the sipag process.
7. **katulong-client SSE subscriber.** Topology. Third wire surface alongside WS attach + HTTP. Built against today's `claude/<uuid>` topic; gains `sessions/<id>/*` for free when katulong#715/#716 land. Feeds the gemma4 bridge (Phase 1 #3) — emits structured `KatulongEvent`s that the bridge's sliding window consumes. **This is the canonical replacement for the existing Demeter-violating Claude-transcript-proxy path** (`katulong-client::http::claude_transcript_url` + `serve/htmx.rs::observation_transcript_handler`) — once #7 lands, that path retires per `[[feedback-strict-layer-coupling]]`.
8. **✅ DONE (PR #549) — Add `ollama-bridge-client` workspace crate.** Topology. Rust wire client for [`dorky-robot/ollama-bridge`](https://github.com/Dorky-Robot/ollama-bridge) (Elixir queue+auth daemon that solves the GPU concurrency problem by serializing requests through a single-worker queue with sha256-based 60s dedup and bearer auth). Wire shape: `POST /enqueue → {hash, status}` + `GET /jobs/:hash` poll + the pass-through probe endpoints (`/api/tags`/`/api/show`/`/api/ps`). Crate exposes a `submit_and_wait(endpoint, body, timeout)` convenience that hides the polling, plus separate raw enqueue/poll for callers (e.g. the lens-worker fleet) that want to track job ids. **Reframed 2026-05-19** — earlier draft assumed sipag would talk to ollama directly with an `LlmClient` trait + per-model construction; that design was wrong. The bridge is the actual integration point, the wire shape is async-jobs-not-sync-chat, and model choice lives in the per-job body (no constructor-level model). `~/.sipag/models.toml` profile→concrete-model resolution stays in sipag — lens-workers do the lookup and include the resolved string in the enqueue body. `sipag-core/src/llm.rs` retires (callers migrate to the new client). See [`docs/extraction-plan.md`](extraction-plan.md) §3 for the full crate scope; see memory `[[reference-ollama-bridge]]` for the bridge's wire details.
9. **Fold gate into the lens-worker abstraction.** Experimentation. Depends on #3 (lens-worker substrate). Today the gate is a parallel gemma-prompt-then-react path; it should be re-expressed as a lens-worker whose lens text says "given current pane state, classify against the project's dispatchable statuses." (`nudge.rs` previously sat alongside as the post-dispatch sibling; it retired in §9 #11 since the WS-attach path doesn't need post-dispatch keystroke retry.)
10. **Event-driven observer.** Experimentation. Depends on #7 + #9. Shrinks dramatically when katulong#715/#716 land.
11. **✅ DONE — Recovery deletion** (`verify_and_heal_dispatch`). Experimentation. **Closed sipag #528** by removing the unsafe seam entirely — no more LLM-emitted bytes reach a PTY by design. The legacy keystroke-driving nudge loop in `sipag/src/serve/htmx.rs::verify_and_heal_dispatch`, the `SIPAG_DISPATCH_V2` env var that gated it, the dispatch-time `build_launch_cmd` HTTP `/exec` pre-send, and `sipag-core/src/nudge.rs` (the only-consumer companion) were all deleted; the WS-attach path (`sipag_dispatch::dispatch` via `KatulongAttachClient::wait_for`) is now the only path. See also memory `feedback-strict-layer-coupling`.
12. **✅ DONE (PR #547) — Lift dispatch policy** (session naming, worktree, agent command) out of katulong-client into Experimentation's `act` sub-module. Topology → Experimentation. Shipped as the `sipag-dispatch` workspace crate. Removes sipag concepts from the wire crate; deduplicates CLI/serve (TUI still uses sync HTTP `/exec`, follow-up). Web UI dispatches now do worktree setup (previously skipped); CLI dispatches now use WS-attach orchestration (previously raw HTTP `/exec`). See [`docs/extraction-plan.md`](extraction-plan.md) §3 for the crate's full scope.
13. **Split `htmx.rs` + `board_view.rs`.** Cross-cutting. After #6 + #11 + #12, what's left is route handlers + view helpers; split by feature (Steering vs Experimentation surfaces). With #6 + #12 landed, htmx.rs is already ~440 LOC smaller — the remaining pressure is the gate (retires in #11/#9 series) and the dispatch handler shell.

### Phase 3 — cleanup + extractions (no urgency)

14. **Agent API endpoint wiring.** Steering. Type-driven; types landed in Phase 1 (#5). The published-language types should drive the route shapes naturally.
15. **✅ DONE (PR #554) — Promote `auth/` to its own crate.** Shipped as `sipag-auth`. Mechanical extraction of 9 files via `git mv`; 48 tests preserved + passing. `sipag-core` re-exports at the old `sipag_core::auth::…` path for back-compat.
16. **Decide `pubsub.rs` future** (not its fate — load-bearing today). Topology. sipag's broker has 16+ publish sites internally; see §10. The decision is whether to keep an in-process broker or route sipag's own topics into katulong's broker per `[[feedback-fix-at-right-layer]]`. Defer until queue items #2 + #7 prove out the katulong-consumer side; the consolidate-vs-keep call is much easier with both ends working. (Note: pubsub.rs itself was extracted into `sipag-pubsub` in PR #545 — that's an extraction, not a fate decision. This item is still about the in-process vs route-through-katulong question.)

---

## 10. Open questions

- **§2 Agent API surface**: what's the auth model? Same passkey + cookie session as the human, or a separate service-account / bearer-token flow?
- **§3 bridge retry-with-reflection**: currently deferred ("cost; complexity"). When/whether to add reflection ("gemma, your last response didn't parse — here's the schema, try again") depends on production parse-failure rates. Open until we have telemetry.
- **§3 `ask_human` per-kind dedup windows**: defaults are 5min/1hr/24hr for `permission-style` / `progress-check` / `is-this-KR-still-alive`. These are starting points; should likely move to a config file once usage patterns are visible.
- **§3 trigger-blind-spot heuristic**: the "second-tier trigger" (20 events in 5 min + no RecordedAction → fire a gemma call) is a starting threshold. Need to validate it doesn't false-positive on legitimately quiet productive work.
- **§3 disk-backed outage buffer (future option)**: v1 commits to a 200-event in-memory sliding window per session, with explicit "older events dropped on the floor during gemma outage" behavior. A disk-backed slower buffer (replay-after-recovery) is a reasonable future option but adds: serialization format, on-disk schema, replay-ordering invariants, recovery dedup against actions emitted during the outage. Not v1. Re-litigate only if the operator-channel "N events dropped during outage" banner starts firing in production with N values that matter.
- **§3 `Profile` → concrete model mapping** (Fast / Strong / CodeAware → e.g. gemma4:latest / gemma4:31b / qwen3-coder:30b): what's the *default* mapping shipped with sipag, and what's the override path? Probably `~/.sipag/models.toml` exists optionally; absent file → built-in defaults. Open: what are the right built-in defaults given the dorky-robot stack typically runs locally? Tentative: `Fast=gemma4:latest, Strong=gemma4:31b, CodeAware=qwen2.5-coder:7b`. Decide once profile usage patterns are visible in practice.
- **§3 cross-corpus retrieval (sipag corpus ↔ diwa)**: lens-workers querying *both* sipag's observation corpus AND diwa's source-tree index gives much richer context for technical-lens derivations. Mechanism: lens-workers declare which corpora they want read access to; `corpus.search` could fan out across declared corpora. v1: every lens-worker has read access to both. Re-evaluate if cost/latency become real.
- **§3 lens governance / sprawl**: lens-workers proliferate over time (one per Steering entry + project-meta + ad-hoc). Some lenses will produce noise; some will compound. Worth: ranking lenses by "did insights derived through this lens get cited / built upon downstream?" Low-cited lenses get suggested for retirement. Need a UI affordance once the corpus has enough volume to test the ranking.
- **§3 meta-cognitive lens guardrails**: compounding derivation can build elaborate towers on shaky ground. The meta-cognitive lens should specifically watch for "deep derivation chains with thin source-observation support" and flag them. Open: what exactly is "thin" — generation-depth threshold, source-count, source-confidence?
- **§3 ad-hoc lens expiry**: ad-hoc lenses with no Steering promotion and no downstream citation auto-expire after N days. What N? Probably 30. Source preserved per `[[feedback-deprecate-with-rationale]]`.
- **§3 lens worker triggers — model-decide formalization**: v1 ships schedule + threshold. The third (model-decide self-retrigger) needs a concrete shape. Probably: each worker introspects "how much new content has my lens not yet processed, and is any of it semantically novel relative to my prior outputs?" Threshold on novelty score → re-trigger.
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
- 2026-05-17 — review-fix round 4 on PR #538 — symmetric fix: round-3 added the operator-channel explanation for why §6 doesn't carry that row, but missed adding the matching `ask_human` row that §3 line 329 explicitly claims is "Catalogued in §6 ACL table." Added the row (Experimentation `ask_human(...)` → Steering KR sidebar `steering-question` item) with cross-reference to the per-kind dedup windows in §3's failure-mode policy.
- 2026-05-18 — **per-lens model selection** added to the lens-worker abstraction. Driven by user's observation that different lens-workers want different models — the bridge tier (high-frequency, threshold-driven) wants a fast small model; the derivation tier (strategic, meta-cognitive) wants a stronger slower one; code-aware lenses want a code-tuned model. Discovered while flipping the running serve from `llama3.1:8b` (code default) to `gemma4:31b` and noticing the 2-3× latency hit was unacceptable for what would become the bridge worker. Changes: §3 `Lens` aggregate grew a `model: ModelChoice` field; new `ModelChoice` value object (`Default | Named | Profile`); new `Profile` enum (`Fast | Strong | CodeAware`) decoupling lens definitions from concrete model names. §3 testability invariants: each lens-worker gets its own `LlmClient` instance configured for its model (not a global one). §8 migration map: new `~/.sipag/models.toml` config (Phase 1 #3, 🔴 not yet) maps Profile → concrete model name per environment. §9 Phase 1 #3 expanded to include the ModelChoice runtime + models.toml reader; Phase 2 #8 (`ollama-client`) constructor takes a model name (per-lens construction). §10 grew an open question on built-in default Profile mappings (tentative: Fast=gemma4:latest, Strong=gemma4:31b, CodeAware=qwen2.5-coder:7b). Cloud-only models (e.g. `gemma4:31b-cloud`) deliberately not in the default profile mapping — conflicts with the "your private code never has to leave your machine" promise in narrative.md.
- 2026-05-17 — **major §3 reframe (third pass): lens-worker abstraction**. Driven by user's conversational insight: "each object in our OKR or a KR can be thought of as a lens." The per-classification recording-API verbs (`note_progress`, `flag_blocker`, etc. — kept after the prior reframe) collapse into one free-form `observe(text, tags?, source_refs?)` workhorse plus three structural verbs (`suggest_stance`, `ask_human`, `propose_task`). New core abstraction: **`LensWorker`** — a gemma4 invocation with a lens definition (text), trigger policy (hybrid: schedule + threshold + model-decide), query strategy, write protocol. **Every Steering entry IS a lens** — its text serves directly as the worker's system prompt; Steering UI doubles as lens registry. **The gemma bridge is just the first lens-worker** (reactive trigger on katulong events). Other workers (one per Steering entry + project-meta lenses + ad-hoc) are siblings. New infrastructure: local vector **`Corpus`** (sipag-internal, sibling to diwa not absorbed), append-only forever, tagged + timestamped + semantic-search; **`corpus.search`/`corpus.expand` tools** (sipag-internal MCP-shape) for multi-step retrieval; **lens registry**; **trigger machinery**. §3 rewritten: responsibility, language, aggregates (`Lens`, `LensWorker`, `CorpusItem`), sub-responsibilities (act / observe / **derive** — third one is new), dedup logic (split: per-worker semantic for free-form; per-payload-hash for structural verbs; **no cross-worker dedup** — different lenses on same fact are intentionally distinct). §6 ACL table grew: Steering-entry-text → lens-worker prompt row (the new load-bearing seam); corpus-search tool row; `propose_task` row explicit. §8 migration map: gate.rs / nudge.rs / categorize.rs reframed as **early lens-worker prototypes**; workers/{expand,research,scheduler} likely become lens-worker scheduler infrastructure. §9 Phase 1 #3 rewritten yet again — was "recording API + bridge"; now "lens-worker abstraction + Corpus + corpus-search tools + bridge as first worker." §10 grew five new open questions (cross-corpus retrieval, lens governance/sprawl, meta-cognitive guardrails, ad-hoc lens expiry, model-decide trigger formalization).
- 2026-05-18 — Phase 2 #6 landed (PR #546): `KatulongAsyncClient` added to `katulong-client` with per-call streaming body cap (`DEFAULT_BODY_CAP=1MiB`, `TRANSCRIPT_BODY_CAP=10MiB`). Closed sipag #527. `katulong_proxy.rs` deleted (~243 LOC). `sanitize_upstream_body` moved to a new `serve/upstream.rs` since it's a sipag-side concern (governs what sipag puts into its own outbound responses). Round-2 review caught three additional raw `GET /sessions` call sites the PR had missed (`fetch_sessions_full`, `scan_host`, nudge-loop exec) plus a 409-fallback `validate_id()` regression — all fixed in the round-2 commit before merge. Round-2 also added a `BadSessionId` arm to `observation_transcript_handler` so the trust-boundary violation logs loudly.
- 2026-05-18 — Phase 2 extraction structural prereq landed (PR #545): `sipag-pubsub` extracted from `sipag-core/src/pubsub.rs` into its own workspace crate. Not on the §9 queue (it's a clean structural refactor with no behavior change), but lands the extraction template for the harder #12 below. See [`docs/extraction-plan.md`](extraction-plan.md) §3 / §4 / §8 for the broader extraction sequencing.
- 2026-05-19 — **Phase 2 #12 landed (PR #547): `sipag-dispatch` workspace crate.** Pulled the dispatch action (~240 LOC) out of `sipag/src/serve/htmx.rs::dispatch_via_attach_client` and the overlapping logic in `sipag/src/cli.rs::run_dispatch_task` into one async `dispatch(remote, &session, input, on_step) -> Result<(), DispatchError>` function. `htmx.rs` shrank by ~440 LOC. Web UI v2 dispatches now do worktree setup (previously skipped — a real feature gap closed by the extraction); CLI dispatches now use WS-attach orchestration (TUI-ready wait, paste echo, processing wait — previously raw HTTP `/exec`). The gate and `verify_and_heal_dispatch` stayed (retire separately per #11). TUI dispatch still uses sync HTTP `/exec` — follow-up.
- 2026-05-19 — **#8 reframed: `ollama-client` → `ollama-bridge-client`.** Earlier draft assumed sipag would talk to ollama directly with an `LlmClient` trait + per-model construction. That design was drafted before this session surfaced [`dorky-robot/ollama-bridge`](https://github.com/Dorky-Robot/ollama-bridge) as the actual integration point. The bridge's wire shape is enqueue-job + poll, not synchronous chat; model choice lives in the per-job body, not in a client constructor; bearer auth and built-in sha256 dedup are non-negotiable. `~/.sipag/models.toml` profile-resolution stays in sipag — lens-workers resolve their `ModelChoice` (Fast/Strong/CodeAware) to a concrete model string and include it in the enqueue body. `sipag-core/src/llm.rs` retires entirely; callers migrate to the new wire client.
- 2026-05-20 — **Six extraction PRs landed back-to-back (autonomous push).** Per `docs/extraction-plan.md` §4 sequencing: `ollama-bridge-client` (PR #549, §9 #8), `sipag-corpus` (PR #550, substrate for #3), `sipag-lens` (PR #551, runtime for #3), `sipag-mesh` (PR #552, mechanical), `sipag-board` (PR #553, mechanical), `sipag-auth` (PR #554, §9 #15). §9 #3 marked partially-landed: the **substrate** (corpus + lens + bridge-client) shipped; the **scheduler loop** + **bridge-as-first-worker concrete instance** + **`corpus.search`/`corpus.expand` MCP tool wrappers** remain. §9 #8 + #15 marked DONE. §9 #16 clarified ("pubsub.rs itself was extracted in PR #545; this item is still about the in-process-vs-route-through-katulong decision, not the extraction"). All extractions are mechanical moves OR thin new crates with behavioral test suites focused on feature requirements (per user direction); none broke any existing tests. Outcome: `sipag-core` is now a thin re-export shim around 8 extracted crates + a handful of retiring modules (gate/nudge/llm scheduled to retire with the lens-worker scheduler landing).
- 2026-05-20 — **§9 Phase 2 #11 landed: `verify_and_heal_dispatch` deleted (closes sipag #528 by deletion).** The legacy keystroke-driving nudge loop in `sipag/src/serve/htmx.rs` is gone. Also retired in the same PR: the `SIPAG_DISPATCH_V2` env var + `dispatch_v2_enabled` (no more two-path branching — the WS-attach path is the only path), `build_launch_cmd` + the HTTP `/exec` pre-send (the attach owns the launch keystroke via `attach.input("<role-cmd>\r")`), `persist_task_state` + `park_task_with_reason` (helpers only the nudge loop called), and `sipag-core/src/nudge.rs` itself (417 LOC, only consumer was the recovery loop). No more LLM-emitted bytes reach the PTY by design (memory `feedback-strict-layer-coupling`). Foundation shipped earlier: v2 attach series (PRs #532-#535) and the `sipag-dispatch` extraction (PR #547). Docs updated: CLAUDE.md (env vars section), modules.md §6 (Post-dispatch observer + Recovery loop rows), §8 (migration map row for `nudge.rs`), §9 #11 (marked DONE), feature-matrix.md (three rows). dispatch.md got a strengthened snapshot-stale header noting the two-path framing is now historical; a full doc rewrite is deferred.
- 2026-05-21 — **§9 Phase 1 #3 scheduler landed.** `sipag/src/serve/lens_scheduler.rs` walks `~/.sipag/lenses/*.toml`, registers non-retired `Lens` entries, and fires each on its `TriggerPolicy::Schedule { interval }` cadence via `LensWorker::run_with_tools`. New `sipag serve --lens-scheduler` flag wires the loop end-to-end: loads `~/.ollama-bridge/remote.json`, opens `~/.sipag/corpus/`, constructs `BridgeChatBackend` + `BridgeEmbedder`, spawns a 30s-interval tick. Off by default. `TriggerPolicy::Threshold` and `::ModelDecide` are surfaced as `skipped_unsupported` telemetry rather than silently dropped — those wait on event sources (Threshold needs §9 Phase 2 #7 SSE subscriber; ModelDecide is the §10 open question on model-decide formalization). Tests use `tokio::time::pause()` + `advance()` against the scheduler's `tokio::time::Instant` clock; 13 feature-requirement tests cover fire-on-first-tick, interval respect, refire-after-elapse, retired-lens filtering, error backoff (errored lens advances `last_fired` so it doesn't busy-loop), and the lens loader's tolerance to malformed TOML. What's still pending in #3: the **bridge lens-worker concrete instance** — the first reactive trigger on katulong `claude/<uuid>` events. Needs §9 Phase 2 #7 (SSE subscriber) or a polling fallback.
