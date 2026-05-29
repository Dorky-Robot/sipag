# sipag — architecture

> The single technical reference for sipag. Companion docs: [`narrative.md`](narrative.md) (product story) and [`../VISION.md`](../VISION.md) (strategic principles).

---

## Contents

- [System context](#system-context) — where sipag sits in the stack
- [Runtime topology](#runtime-topology) — what runs and what talks to what
- [Crate map](#crate-map) — workspace layout and dependency direction
- [Data flow](#data-flow) — dispatch, observation, and lens-worker paths
- [Event topology](#event-topology) — the buses, what each is for, why sugo is the universal mesh bus
- [Key abstractions](#key-abstractions) — the types that carry the system
- [State model](#state-model) — what lives on disk
- [Invariants](#invariants) — the rules that must hold
- [Trust boundaries](#trust-boundaries) — auth model and authorization scope
- [Phase queue](#phase-queue) — what's shipped, what's open, what's blocked

---

## System context

sipag is the OKR layer for an agentic dev fleet. It sits between the human (who writes Objectives and Key Results) and the agent sessions (which run inside katulong). A local LLM (gemma, routed through an ollama bridge) watches what the agents do and surfaces observations.

```
human (browser / CLI)
  │
  ▼
sipag  ◄──── gemma (via ollama-bridge) ◄──── ollama (local GPU)
  │
  ▼
katulong (session manager)
  │
  ▼
Claude / agents (running in tmux PTYs)
```

**Strict layer coupling.** sipag never reaches past katulong to talk to Claude directly. The bridge between Claude's free-form output and sipag's structured world is gemma on sipag's side. See `[[feedback-strict-layer-coupling]]`.

---

## Runtime topology

`sipag serve` is a single process. Everything runs on the tokio async runtime as spawned tasks.

```
sipag serve --workers --lens-scheduler --bridge-worker
  │
  ├── HTTP server (axum)           ← browser + API clients
  │     ├── auth middleware        ← passkeys + session cookies
  │     ├── HTMX board handlers   ← server-rendered UI
  │     ├── WebSocket endpoint     ← live updates to browser
  │     └── dispatch handler       ← creates katulong sessions
  │
  ├── observer poll (15s)          ← GET /sessions on each host
  │     └── feeds claude/<uuid> topics to bridge worker
  │
  ├── worker scheduler             ← label-driven dispatch (research/expand)
  │
  ├── lens scheduler (30s tick)    ← fires Schedule-triggered lenses
  │     └── holds shared corpus mutex during gemma calls
  │
  └── bridge worker coordinator    ← manages per-session SSE subscribers
        ├── session-A subscriber   ← SSE → sliding window → gemma fire
        ├── session-B subscriber
        └── ...
```

**Shared state:** the lens scheduler and bridge worker share a single `Arc<Mutex<Corpus>>` opened once at startup. The corpus mutex is held during gemma round-trips (v1 limitation; documented, not yet addressed). All other shared state flows through `AppState` (cloned per-request by axum).

---

## Crate map

10 workspace crates. The binary (`sipag`) depends on all of them; the TUI depends only on `sipag-core`.

```
sipag (binary — CLI + web server)
├── sipag-core         re-export shim for backward compat
│   ├── sipag-board    Objective / KR / Task / Role / Project / Observation
│   ├── sipag-auth     WebAuthn passkeys, sessions, devices
│   ├── sipag-mesh     multi-host mesh config (hosts.toml)
│   ├── sipag-pubsub   in-process broker (slated for deletion — see Event topology)
│   └── katulong-client  HTTP + WS + SSE wire client
├── sipag-dispatch     the dispatch action (attach + paste + wait)
├── sipag-lens         LensWorker runtime + ChatBackend trait + 4 verbs
│   ├── sipag-corpus   local vector DB (JSONL + cosine search)
│   └── ollama-bridge-client  queue-aware wire client for ollama-bridge
└── ollama-bridge-client

sipag-tui (binary — interactive board)
└── sipag-core
```

**Dependency direction:** library crates never depend on the binary. `sipag-core` is a thin re-export layer — new code should depend on the leaf crates directly. `sipag-core/src/llm.rs` is legacy (3 callers remain); all new LLM calls go through `sipag-lens::ChatBackend`.

### Extraction history

The workspace started as a monolithic `sipag-core`. Eight crates were extracted in series over April–May 2026 to make the system composable (each is separately replaceable). All extractions are complete. The principle: every concept that has a non-sipag consumer eventually moves to its own crate.

| Crate | Extracted as | What moved |
|---|---|---|
| `sipag-pubsub` | PR #545 | the file-backed broker |
| `sipag-dispatch` | PR #547 | the WS-attach dispatch action |
| `ollama-bridge-client` | PR #549 | the ollama-bridge wire client |
| `sipag-corpus` | PR #550 | the local vector store |
| `sipag-lens` | PR #551 | LensWorker + ChatBackend trait |
| `sipag-mesh` | PR #552 | host topology config |
| `sipag-board` | PR #553 | OKR + Task domain |
| `sipag-auth` | PR #554 | WebAuthn + sessions |

`sipag-core` survives as a re-export shim for back-compat. New code should depend on the leaf crates directly.

---

## Data flow

### Dispatch path

```
user clicks "Dispatch" in web UI
  → dispatch_task_handler (htmx.rs)
    → resolve host, load Task + Role, build prompt
    → POST /sessions on katulong → fresh tmux pane (sipag-d-<hex>)
    → dispatch gate: gemma classifies pane via ChatBackend
        ├── Dispatch  → continue
        └── Parked    → render toast, return
    → move Task to in-progress
    → sipag_dispatch::dispatch
        ├── WS attach to the session
        ├── paste the prompt
        ├── wait for Claude TUI ready signal
        └── wait for processing complete signal
    → observer discovers session on next poll (15s)
      → reads meta.claude.uuid from /sessions response
      → bridge_handle.watch("claude/<uuid>")
```

### Observation path (bridge worker)

```
bridge coordinator receives topic
  → spawns per-session SSE subscriber
    → katulong_client::sse::subscribe(topic, from_seq)
    → events accumulate in SessionWindow (200-event VecDeque)
    → every 10 events: fire gemma via LensWorker::run_with_tools
      → gemma reads the window as a user prompt
      → gemma can call corpus.search / corpus.expand mid-prompt
      → gemma produces: observe / suggest_stance / ask_human / propose_task
      → observations written to corpus (embedded via ollama bridge)
      → structural verbs logged at warn (UI dispatch is a follow-up)
    → on stream error: reconnect from last_seq + 1
```

### Scheduled lens path

```
lens scheduler tick (every 30s)
  → for each Lens in ~/.sipag/lenses/*.toml with Schedule trigger:
    → if interval elapsed since last fire:
      → LensWorker::run_with_tools(lens.prompt_text, corpus, embedder)
      → same gemma → corpus → structural verb flow as bridge
```

---

## Event topology

**Sugo is the universal bus for all external mesh I/O.** This includes both typed coordination events (`incident@v1`, `dispatch.outcome@v1`) AND LLM jobs (`llm-chat@v1`, `llm-embed@v1`). Everything that crosses a process or machine boundary on the mesh goes through sugo. Multiple "event-bus-shaped" things exist today because of historical staging, not architectural intent — they collapse into sugo as the migration proceeds.

> **Scope note.** Sugo's intended scope extends beyond the dorky_robot stack — it's also the unification layer for the humOS stack and related mesh tools (kapwa, manggagamot, tao). What this doc says about sugo's wire shape, profiles, and roadmap reflects **sipag's use of sugo**, not sugo's whole design. When sipag's needs would push design pressure onto sugo that only sipag cares about, defer to sugo's own README as authoritative.

### What we have today (transitional)

Five surfaces. The end state is **one external bus (sugo)** plus one tightly-scoped local primitive (`tokio::broadcast` for the browser WS loop). Today:

```
1. katulong SSE broker      [external, we CONSUME]
   - In-process pubsub inside katulong (not sugo)
   - Carries: claude/<uuid> session events, sessions/<id>/* (when #715 lands)
   - WHY it stays: katulong must work without sugo deployed (the
     stack's compose-and-replace property). Local subscribers
     (katulong's own web UI, sipag-on-the-same-machine) use it directly.
     Cross-machine reach happens via a katulong → sugo mirror.

2. ollama-bridge             [external, we CONSUME — RETIRING]
   - Elixir daemon, hand-rolled queue + auth + storage for LLM jobs
   - Sugo was BOOTSTRAPPED from ollama-bridge's primitives
   - WHY it exists today: sugo Phase 1 has the right wire shape (enqueue
     + poll) but no LLM-execution worker. ollama-bridge is the
     execution side wrapped in its own daemon process.
   - WHY it retires: identical wire shape duplicated in two processes;
     the right cut is "sugo carries the LLM job; an ollama-worker
     subscribes and executes." See "ollama-bridge → sugo absorption" below.

3. sugo                      [external, we PRODUCE — target universal bus]
   - Elixir daemon, generalized event bus
   - Wire: POST /enqueue {type, payload} + GET /jobs/:hash poll
   - Phase 2 (pending): GET /events?type=&since= subscriber long-poll
   - Carries everything that crosses a process/machine boundary:
     mesh events AND LLM jobs (once ollama-worker lands)

4. sipag-pubsub              [in-process, we OWN — RETIRING]
   - Rust crate, file-backed JSONL + tokio::broadcast
   - Carries: sipag's own events (observations/*, tasks/*, workers/*)
   - WHY it exists today: built before sugo did
   - WHY it retires: mesh-level kinds (observations/*, dispatch.outcome,
     worker.complete) move to sugo; UI-loop kinds (tasks/* updates) use
     tokio::broadcast directly. The broker abstraction has no remaining
     role between those two extremes.

5. Browser WebSocket fanout  [in-process transport, stays]
   - sipag's WS endpoint forwards UI-loop events to the browser
   - After sipag-pubsub retirement: backed by tokio::broadcast directly
   - WHY it stays: htmx live updates need µs-latency in-process fanout;
     routing through sugo would add a network hop to every keypress
```

### The principles

Two rules hold the shape together:

**1. Each tool in the Dorky Robot stack must work standalone.** kubo, katulong, sipag, hulma are each separately useful and replaceable. Making any of them require another at runtime breaks that property. This is the **only** reason katulong keeps its own broker (its web UI can't depend on sugo being deployed). Same reason sipag doesn't *require* sugo even after the migration — sipag's UI-loop events stay in-process.

**2. The best part is no part.** Where we own the code and the equivalent functionality exists in a shared service, prefer the shared service. Where two external services have the same wire shape (sugo + ollama-bridge), collapse them. We don't own three event buses; we don't run two services with identical primitives.

### Target shape

```
┌─────────────────────────────────────────────────────────────────┐
│ EXTERNAL MESH                                                    │
│                                                                  │
│         ┌─────────────────────────────────────────────┐         │
│         │              SUGO                            │         │
│         │      the universal mesh bus                  │         │
│         │                                              │         │
│         │  POST /enqueue {type, payload}               │         │
│         │  GET  /jobs/:hash         (poll for result)  │         │
│         │  GET  /events?type=&since=  (subscribe)      │         │
│         │                                              │         │
│         │  carries:                                    │         │
│         │   - mesh events (incident@v1,                │         │
│         │     dispatch.outcome@v1, observation@v1, …)  │         │
│         │   - LLM jobs    (llm-chat@v1, llm-embed@v1)  │         │
│         └────────────────┬────────────────────────────┘         │
│                          │                                       │
│        ┌─────────────────┼──────────────────┐                   │
│        │                 │                  │                    │
│        ▼                 ▼                  ▼                    │
│   sipag               ollama-worker      other workers          │
│   publishes +         (subscribes to     (kapwa, manggagamot,   │
│   subscribes          llm-*; executes    tao, …)                │
│                       against ollama)                            │
│                                                                  │
│                                                                  │
│   katulong SSE broker  ──►  katulong → sugo mirror  ──►  sugo   │
│   (local consumers           (small daemon,                      │
│    keep direct access)        operator-configured)               │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────┐
│ INSIDE sipag serve                                               │
│                                                                  │
│   katulong-client::sse  ──► bridge worker  ──► corpus           │
│                                                                  │
│   sugo-client (NEW, replaces ollama-bridge-client)              │
│                       ──► publish mesh events                   │
│                       ──► publish LLM jobs (chat, embed)        │
│                       ──► subscribe for replies + mesh events   │
│                                                                  │
│   tokio::broadcast (direct, no abstraction)                     │
│                       ──► UI-loop events  ──► browser WS        │
│                          (tasks/* label.changed, done.toggled)  │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

### Why this split, by access pattern

| Pattern | Latency | Reach | Durability | Choice |
|---|---|---|---|---|
| UI-loop events (browser WS) | µs | one process | none needed | `tokio::broadcast` directly |
| Mesh events (observations, dispatch outcomes) | ~ms ok | cross-machine | needs persistence | **sugo** |
| LLM jobs (chat, embed) | seconds | wherever the GPU is | dedup matters | **sugo** (via ollama-worker) |
| Session events (claude/uuid) | tight, mostly local | local-first, cross-machine via mirror | needs replay | katulong SSE; cross-machine via sugo mirror |

### The cuts

- **Delete `sipag-pubsub` as a broker abstraction.** Mesh events publish to sugo. UI events use `tokio::sync::broadcast` directly. No broker, no per-topic seq tracking, no JSONL persistence we wrote ourselves.
- **Absorb ollama-bridge into sugo.** Sugo carries `llm-chat@v1` / `llm-embed@v1` jobs; a new `ollama-worker` daemon subscribes and executes against ollama. The single-worker GPU concurrency control moves into the worker (same code, different deployment shape). One Elixir daemon (sugo) replaces two (sugo + ollama-bridge).
- **Build `sugo-client` Rust crate.** Replaces `ollama-bridge-client`. Hides the publish + poll pattern behind `submit_and_wait<T>(type, payload) -> T` for sync-feeling callers; exposes `subscribe(type) -> Stream` for long-poll consumers.
- **Don't touch katulong's broker.** It exists because katulong must work standalone. Local subscribers keep direct access; cross-machine reach via the `katulong → sugo` mirror.

### ollama-bridge → sugo absorption (the bigger cut)

This is the load-bearing simplification. Today's two-service shape:

```
sipag ──► ollama-bridge-client ──► ollama-bridge daemon ──► ollama
                                   (queue + auth + dedup +
                                    storage + janitor)
```

Becomes:

```
sipag ──► sugo-client ──► sugo daemon ──► ollama-worker ──► ollama
                          (same primitives,    (subscribes to
                           generalized over     llm-*@v1,
                           single-worker queue
                                                lives here)
```

What changes:
- One daemon process instead of two
- One bearer-token surface instead of two (`~/.config/sugo/config.json` replaces `~/.ollama-bridge/remote.json`)
- One Rust client crate instead of two (`sugo-client` replaces `ollama-bridge-client`)
- LLM jobs become observable on the mesh (other tools can subscribe to `llm-chat@v1` to watch what's hitting the GPU)
- **New LLM types (thinking, vision, audio) can be added without changes to sugo, sipag, or any other publisher** — see polymorphic shape below

What stays the same:
- The wire shape callers see (POST enqueue + poll for hash) is identical
- The single-worker GPU concurrency control still exists — just moves from ollama-bridge daemon to ollama-worker daemon
- Dedup, persistence, auth — sugo already has these (inherited from ollama-bridge's primitives)

Cost: one extra in-process hop (sugo daemon → ollama-worker subscriber → ollama), and the worker subscriber needs leasing semantics so only one worker picks up each job. Both are small.

### Polymorphic LLM events — flexible to new model kinds

Sugo doesn't need to know what an LLM is. The protocol separates three concerns:

```
type                    = MODALITY (input/output shape)
payload.profile         = PUBLISHER'S INTENT (what kind of model)
worker decides          = ACTUAL MODEL CHOICE (operator config)
```

**Event types are modality-keyed:**

| Event type | Modality | Why a separate type |
|---|---|---|
| `llm-chat@v1` | text in, text out | Standard chat shape |
| `llm-embed@v1` | text in, vector out | Output schema is different (vector, not text) |
| `llm-vision@v1` | image+text in, text out | Input schema is different (image bytes) |
| `llm-audio@v1` | audio in, text out | Different input/output shape again |
| `llm-tool-use@v1` | text in, structured tool calls out | If we want a separate type for this pattern |

**Profile is a payload field, freely extensible:**

```json
POST /enqueue
{
  "type": "llm-chat@v1",
  "payload": {
    "profile": "thinking",
    "messages": [...],
    "max_tokens": 4096,
    "model_hint": "gemma3:thinking-13b"
  }
}
```

Profile is a free-form string (`fast`, `strong`, `thinking`, `code-aware`, `long-context`, or anything else operators want to introduce). It's a declaration of intent; there's no central registry.

**Workers self-select by subscribing + filtering:**

```
ollama-worker (default deployment)
  subscribes: llm-chat@v1, llm-embed@v1
  serves profiles: ["fast", "strong", "code-aware"]
  → competes for events matching its profile set; ignores others

thinking-worker (new, deployed when needed)
  subscribes: llm-chat@v1
  serves profiles: ["thinking"]
  → only takes thinking-profile events

vision-worker (entirely separate type)
  subscribes: llm-vision@v1
  serves profiles: ["vision-fast", "vision-strong"]
```

**The flexibility properties:**

| Change | What it takes |
|---|---|
| Add a new model in an existing profile | Operator changes ollama-worker config to point profile at new model name |
| Add a new profile (e.g. `thinking`) | Deploy a new worker that serves it. No sipag change. No sugo change. |
| Add a new modality (e.g. `llm-vision@v1`) | New event type + new worker. Publishers that don't know vision aren't affected. |
| Route a profile to a specific GPU host | Deploy that profile's worker only on that host. Other workers don't compete. |

**Sipag-side implication:**

The Rust `Profile` enum (currently `Fast | Strong | CodeAware`) becomes a `String` — backed by `~/.sipag/models.toml` for the operator-known set, but free-form at the wire. Lens definitions still declare `model: Profile("thinking")` (the existing API stays); the resolver passes the string through to sugo unchanged. The model name is decided by the worker, not by sipag.

This also gives an honest failure mode: if a publisher requests `profile: "thinking"` and no worker handles it, `submit_and_wait` returns "no handler for profile thinking" instead of silently running whatever ollama happens to have configured. Sugo doesn't enforce this directly (it doesn't know what profiles exist); it surfaces in the publisher's timeout or an explicit "no subscriber" reply from sugo if it gains that signal.

**What sugo needs (beyond Phase 2):**

| Capability | Status | Why |
|---|---|---|
| Subscribe by type | Phase 2, pending | Workers need to long-poll for events of a given type |
| Leasing / lock | Phase 3 (likely) | Multiple workers may subscribe to the same type; only one should process each event |
| Result correlation | Convention only | `payload.in_reply_to: <original-hash>` — no protocol change needed |
| Profile awareness | **Never** | Sugo stays a dumb pipe; profile is opaque payload to it |

### Envelope shapes converge

| Surface | seq | timestamp | type | per-type data |
|---|---|---|---|---|
| katulong SSE wire (`KatulongEvent`) | `seq` | `timestamp` | `event` | `extra` (flattened) |
| sipag-pubsub `Envelope` (retiring) | `seq` | `ts` | `kind` | `payload` (nested) |
| ollama-bridge (retiring) | hash | (none) | (none — single-purpose) | `body` |
| **sugo (target — universal)** | hash + per-type since | RFC3339 | `type` | `payload` |

Once the migration completes, all external sipag I/O uses sugo's `{type, payload}` shape. The katulong SSE wire stays distinct because katulong owns it.

### Migration sequence

```
1. sugo Phase 2 ships (GET /events?type=&since= subscriber long-poll)
2. sugo-client Rust crate
   - submit_and_wait<T>(type, payload) -> Result<T>      [sync-feel]
   - subscribe(type) -> Stream<Event>                    [long-poll]
3. ollama-worker daemon
   - subscribes to llm-chat@v1, llm-embed@v1
   - single-worker queue (moves from ollama-bridge)
   - executes against ollama, publishes llm-chat-result@v1
4. Migrate BridgeChatBackend + BridgeEmbedder
   - replace ollama-bridge-client calls with sugo-client
   - same submit_and_wait shape, different transport
5. Retire ollama-bridge daemon
6. Migrate one mesh kind end-to-end as proof  →  dispatch.outcome
7. Migrate remaining mesh kinds  →  observations/*, kr.assigned, kr.rejected, kr.proposed
8. Replace remaining sipag-pubsub uses with tokio::broadcast (UI loop only)
9. Delete sipag-pubsub crate
10. (Operator-side) deploy katulong → sugo mirror for cross-machine session events
```

Steps 1–5 retire ollama-bridge. Steps 6–9 retire sipag-pubsub. Step 10 closes the cross-machine session-event story.

---

## Key abstractions

| Abstraction | Crate | What it does |
|---|---|---|
| `LensWorker` | sipag-lens | Takes a `Lens` definition + `ChatBackend` + `ModelResolver`. Runs a one-shot or multi-turn (tool-calling) gemma invocation. Writes `Observe` actions to the corpus; returns all `StructuralAction`s to the caller. |
| `ChatBackend` | sipag-lens (trait) | Decouples the LLM call from the wire. Production impl: `BridgeChatBackend` (routes through ollama-bridge). Test impl: `CannedBackend` (returns fixed JSON). |
| `Corpus` | sipag-corpus | Append-only local vector DB. JSONL on disk at `~/.sipag/corpus/items.jsonl`. In-memory search via cosine similarity. Embedded via `BridgeEmbedder` (default model: `nomic-embed-text`). |
| `KatulongEventStream` | katulong-client | Async `Stream<Item = Result<KatulongEvent, SseError>>`. Hand-rolled SSE parser with bounded line/event caps. Callers drive reconnection by tracking the highest seen `seq`. |
| `BridgeWiring` | sipag/bridge.rs | Bundles `OllamaBridgeClient` + `BridgeChatBackend` + `BridgeEmbedder`. Constructed once at startup from `~/.ollama-bridge/remote.json`; shared via `AppState`. |
| `BridgeHandle` | sipag/serve/bridge_worker.rs | Channel handle for feeding session topics to the bridge worker coordinator. The observer calls `handle.watch(topic)` when it discovers a `claude/<uuid>`. |
| `StructuralAction` | sipag-lens | Four-verb enum gemma produces: `Observe`, `SuggestStance`, `AskHuman`, `ProposeTask`. Observe writes to the corpus; the other three log at warn pending UI dispatch. |

---

## State model

Everything sipag knows lives under `~/.sipag/` (overridable via `SIPAG_DIR`):

```
~/.sipag/
├── config.toml                        # default_project, etc.
├── hosts.toml                         # multi-host mesh registration
├── models.toml                        # optional: Profile → concrete model
├── auth.json                          # WebAuthn credentials + sessions
├── corpus/items.jsonl                 # local vector store (lens-worker observations)
├── lenses/<name>.toml                 # lens registry (scheduler walks these)
├── pubsub/<topic>/log.jsonl           # file-backed durable broker (slated for deletion)
├── observations/<host>--<session>.toml  # observer-discovered katulong sessions
├── objectives/<id>/
│   ├── objective.toml
│   └── key-results/<NNN>.toml
└── projects/<project>/
    ├── project.toml
    ├── key-results/<NNN>.toml
    ├── tasks/<id>.toml
    └── roles/<role>.toml
```

External config:
```
~/.katulong/remote.json       # { url, apiKey } — katulong server
~/.ollama-bridge/remote.json  # { url, bearer } — ollama bridge
~/.config/sugo/config.json    # { token } — sugo bus (when sugo-client lands)
```

---

## Invariants

These are the rules that must hold across all changes. Violating them is a bug, not a trade-off.

1. **Strict layer coupling.** `Claude → katulong → sipag`. sipag never reaches past katulong to talk to Claude directly. gemma is the bridge on sipag's side. (The transcript proxy that violated this was deleted in PR #567.)

2. **Each Dorky Robot tool works standalone.** kubo, katulong, sipag, hulma must each be useful without the others. This is why katulong has its own broker — its web UI can't require sugo. Same reason sipag can be used without sugo today (in-process broker carries the load).

3. **Fix at the right layer.** When sipag would need a workaround for something katulong should provide, extend katulong instead. See `[[feedback-fix-at-right-layer]]`.

4. **Deprecate with rationale.** Abandon modules by marking them deprecated with a record of why, not by silently deleting. See `[[feedback-deprecate-with-rationale]]`.

5. **Web UI is the primary surface.** CLI subcommands for Steering capabilities are deliberately deferred until the web UI is fully working. See `[[feedback-sipag-ui-first]]`.

6. **Empirical, not procedural.** Spike → observe → derive. No state machines. Claude is the iterator; sipag observes and records. See `[[project-sipag-work-model-experimentation]]`.

---

## Trust boundaries

```
                 ┌─────────────────────────────────┐
  browser ──────►│  sipag serve                     │
  (passkey +     │  ┌───────────────────────────┐   │
   session       │  │ auth middleware            │   │
   cookie)       │  │ localhost bypass OR        │   │
                 │  │ session cookie validation  │   │
                 │  └───────────────────────────┘   │
                 │                                   │
                 │  ──► katulong (bearer auth)       │
                 │  ──► ollama-bridge (bearer auth)  │
                 │  ──► sugo (bearer auth, target)   │
                 └─────────────────────────────────┘
```

- **Browser → sipag:** WebAuthn passkeys. First device on localhost gets automatic access; subsequent devices pair via setup token. Session cookies (opaque token, server-validated, sliding TTL).
- **sipag → katulong:** Bearer token from `~/.katulong/remote.json`. Per-host API key in `hosts.toml`. Session IDs validated against an ASCII alphanumeric allowlist before URL interpolation (defense against injection from a compromised katulong).
- **sipag → ollama-bridge:** Bearer token from `~/.ollama-bridge/remote.json`. All LLM I/O (chat + embed) goes through this single point.
- **sipag → sugo** (target): Bearer token from `~/.config/sugo/config.json`. Same wire shape as ollama-bridge (POST /enqueue + GET /jobs/:hash + Phase 2 long-poll).
- **Event payloads from katulong:** Treated as trusted input (operator-controlled service). The bridge worker interpolates event content into gemma prompts — prompt injection from crafted event payloads is a known v1 limitation (structural verbs are log-only, mitigating the blast radius).

---

## Phase queue

Tracking item, not architecture. Snapshots the current state of in-progress work.

### Closed

| # | Item | PR(s) |
|---|---|---|
| 1 | Deprecate `feature.rs` + `refine.rs` | #536 |
| 3 | Lens-worker abstraction + corpus + scheduler + bridge worker | #549–#551, #556, #558, #562, #564, #565 |
| 6 | katulong-client async HTTP + body cap | #546 |
| 7 | SSE subscriber + bridge consumer + proxy retirement | #562, #564, #565, #567 |
| 8 | `ollama-bridge-client` crate | #549 |
| 9 | Gate folded into lens-worker abstraction | #559 |
| 11 | Recovery loop deletion (`verify_and_heal_dispatch`) | #557 |
| 12 | `sipag-dispatch` crate extraction | #547 |
| 15 | `sipag-auth` crate extraction | #554 |

### Open

| # | Item | Dependencies |
|---|---|---|
| 4 | `Idea` aggregate + `promote_idea` ACL | — |
| 5 | Agent API published-language types | — |
| 10 | Event-driven observer | shrinks when katulong#715/#716 land |
| 13 | Split `htmx.rs` + `board_view.rs` | — |
| 14 | Agent API endpoint wiring | #5 |
| — | `sugo-client` Rust crate | sugo Phase 2 |
| — | `ollama-worker` daemon (sugo subscriber, replaces ollama-bridge) | sugo Phase 2 + 3 (leasing) |
| — | Migrate `BridgeChatBackend`/`BridgeEmbedder` to sugo | ollama-worker deployed |
| — | Open `Profile` enum to free-form String | sugo migration |
| — | Retire ollama-bridge daemon | migration complete |
| — | Migrate mesh-level events to sugo | sugo Phase 2 |
| — | Delete `sipag-pubsub` after migration | mesh migration complete |
| — | `katulong → sugo` mirror service | sugo Phase 2 |
| — | Structural-verb dispatch to UI | none, but no consumer yet |
| — | **Lens health metrics** | none — cheapest experimentation-gap plug |

**Lens health metrics** (no §, no PR yet — scope sketch):

The scheduler today tracks `fired` and `errors`. Add three counters per lens:

```
fires:                  total times the lens fired
fires_with_action:      fires that produced at least one structural verb
fires_with_no_action:   fires where gemma picked no_action_warranted
verbs_produced:         total structural verbs emitted
verbs_acted_on:         verbs the human acted on (needs UI affordance — couples
                        with the structural-verb-dispatch follow-up)
```

Derived rates:

```
action_rate     = fires_with_action / fires
no_action_rate  = fires_with_no_action / fires
citation_rate   = verbs_acted_on / verbs_produced
```

Surface in a `/lens-health` page. Flag thresholds:

| Signal | Likely meaning |
|---|---|
| `action_rate > 0.8` over 50+ fires | Lens is finding pattern in noise (sycophantic / too eager) |
| `action_rate < 0.05` over 100+ fires | Lens is dead weight |
| `no_action_rate < 0.1` over 50+ fires | Lens never says "nothing to note" — biased framing |
| `citation_rate < 0.1` over 50+ verbs | Human ignores this lens — retire it |

Why this plug, ahead of other experimentation-gap plugs: counters are free; the rates are interpretable without ML expertise; and the human-side feedback loop is currently invisible (operators have no way today to notice a lens is misbehaving). Doesn't fix confirmation bias, but it does surface the symptoms of confirmation bias so the operator can decide. See the §10 "lens governance / sprawl" question — this item is the operational primitive that question needs.

### Blocked / deferred

| # | Item | Why |
|---|---|---|
| 2 | Naming pass remainder | blocked on the domain-vs-schema-noun design question |
| 16 | Decide `pubsub.rs` future | superseded by the sugo migration plan above |

### Open questions

These are decisions to make as design pressure surfaces, not items to schedule:

- **Profile → concrete model mapping defaults.** What ships with sipag for `Fast`/`Strong`/`CodeAware`? Tentative: `gemma4:latest` / `gemma4:31b` / `qwen2.5-coder:7b`. Decide once profile usage patterns are visible.
- **Cross-corpus retrieval (sipag corpus ↔ diwa).** Should lens-workers fan out `corpus.search` across both? v1: both, declared per-lens.
- **Lens governance / sprawl.** Lens-workers proliferate (one per Steering entry + project-meta + ad-hoc). Rank by downstream citation; surface low-cited for retirement when the corpus has enough volume.
- **Meta-cognitive lens guardrails.** Compounding derivation chains can build towers on thin source. Flag deep chains with low source-confidence.
- **Ad-hoc lens expiry.** Auto-expire after N days (probably 30) if no Steering promotion and no downstream citation.
- **Trigger-blind-spot heuristic validation.** Current threshold (20 events in 5 min + no action → fire) is a starting guess. Validate against real traffic.
- **`ask_human` per-kind dedup windows.** Defaults: 5min / 1hr / 24hr for permission / progress-check / KR-alive. Move to config when usage patterns are visible.
- **Bridge retry-with-reflection.** When (or whether) to add "your last response didn't parse — try again." Depends on production parse-failure rates.
- **Disk-backed outage buffer.** v1 drops events older than the 200-event in-memory window during gemma outage. Re-evaluate if the operator-channel banner starts firing with N values that matter.
