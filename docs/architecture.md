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
- [Product experience](#product-experience) — coworker model, two-channel UI, single-AI-many-hats, the chat companion, genesis
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

## Product experience

> Design direction, not all shipped yet. This section captures the experience model the technical surfaces above are being built toward. Where items below aren't yet built, the phase queue tracks the work.

### The mental model: an AI coworker

The AI in sipag is modeled as **a tireless but courteous coworker**. Not "an assistant" (too servile, too eager to please). Not "an agent" (too anthropomorphized, too autonomous). A coworker who happens to be available 24/7, can read every observation, never gets tired, and is courteous enough not to drown you in everything they noticed.

This frame drives concrete design constraints across every surface:

| Coworker would | Sipag's AI must |
|---|---|
| Walk over when something's on fire | Surface destructive-action signals via blocking modal / OS notification |
| Slack DM for permission asks | Notification badge + card on the relevant KR |
| Sticky-note on monitor for non-urgent | Pinned card in the KR sidebar |
| Mention things in 1:1 / standup | Surface in chat on next open / morning catch-up summary |
| Keep observations to themselves if they don't matter | Write to corpus only; no notification |
| Not message you at 3am unless building's on fire | Respect presence; queue non-urgent until you're back |
| Not repeat themselves | Dedup against per-channel history, not just corpus |
| Not gush or agree reflexively | System-prompt-level anti-sycophancy norms |
| Acknowledge what they don't know | Bounded to what's in the corpus + OKR state |
| Say "noted" when corrected and move on | Corrections write counter-observations; no apology theater |

The "tireless" part is the AI's superpower: it can read every observation across every session in real-time without getting tired. The "courteous" part is the discipline that keeps that superpower from becoming the thing it warns against. A real human coworker is courteous-but-overloaded — they miss things because they're human. The AI coworker is courteous-AND-tireless — it can be quiet because it actually noticed everything and decided most of it didn't matter. That combination is the value.

### Two-channel split: targeted vs conversational

The AI talks to the human through two channels with different jobs:

| Channel | Job | Who initiates | Examples |
|---|---|---|---|
| **Targeted UI cards** | "Look at this specific thing right now" | AI → human (interruption-class) | `ask_human` permission requests, `suggest_stance` change, `propose_task`, blocking modals on destructive actions |
| **Conversational chat** | "Let's think through this together" | Human → AI (mostly) | Strategy work, OKR refinement, ambient questions, exploratory thinking |

The two channels don't compete because they answer different questions. The targeted channel is "the AI tells you something pointed." The conversational channel is "you and the AI think together."

**The chat never speaks first** (except for explicit morning catch-up summaries when you open it). Proactive AI-to-human signals always go through targeted cards, not the chat. The chat is sacred conversational space; if the AI has something unsolicited to say, it goes on the agenda for next time the chat is opened, not as a notification.

### Single AI, different hats

There is one AI in sipag. The bridge worker, the lens scheduler, the categorize loop, the strategy chat — these are not separate AIs. They are the same AI wearing different hats for different focuses.

The unified identity is enforced at three layers:

1. **Shared coworker preamble.** Every gemma invocation across every subsystem starts with the same system-prompt preamble (the coworker contract). Tone, voice, and etiquette are constant.
2. **Shared memory via corpus.** The bridge worker writes observations; the chat AI reads them via `corpus.search`. Both ground themselves in the same body of context.
3. **First-person framing across hats.** When the chat AI references something a bridge-worker fire wrote earlier, it speaks in first person ("I noticed that ralph picked refresh tokens") — not "the bridge worker observed X." The technical fact that fires are separate gemma invocations is implementation detail that does not leak into the conversation.

**Embrace the unified-self fiction.** The AI may be a thousand fires across the day, but to the human it is one continuous coworker. This is the more honest UX choice — the alternative (constant epistemic disclaimers about which fire produced which observation) would feel like a chatbot managing your trust, which is itself a trust-erosion pattern.

The AI has no proper name; it just speaks. It does not introduce itself. It does not sign messages. It is sipag's voice — the way Copilot is GitHub's voice, not a separate character.

### Urgency dimension on structural verbs

Today's `StructuralAction` enum (`Observe`, `SuggestStance`, `AskHuman`, `ProposeTask`) needs a fifth field: **urgency**. Without it, every output goes to the same channel and the courteous-coworker policy can't be implemented.

```
Urgency::Page         → blocking modal + OS notification + email
                       (destructive action, irreversible, building on fire)
Urgency::Notify       → notification badge + pinned card
                       (synchronous question, permission, time-sensitive)
Urgency::CardPinned   → sticky note on the relevant KR; no notification
                       (suggested stance change, propose_task)
Urgency::NextOpen     → chat agenda item, surfaces when you open the chat
                       (strategic question, pattern observation worth a 1:1)
Urgency::CorpusOnly   → written to corpus, no surfacing
                       (background context, ambient fact, no action needed)
```

The lens-worker prompt asks gemma to classify urgency as part of producing the verb. The notification router (a new component, `sipag/src/serve/notify.rs`) consumes verb-plus-urgency and decides the actual channel based on presence and dedup state.

This is the single biggest structural change behind the entire coworker experience. Today everything logs at warn; tomorrow the right thing surfaces at the right time through the right channel.

### Presence as a new primitive

The notification router needs to know: is the human at their desk right now?

```
Presence::AtDesk      = sipag tab focused, activity within last 5 min
Presence::Recent      = active within last 30 min but not currently focused
Presence::Away        = no activity beyond 30 min
Presence::Offline     = no presence signal for hours
```

Implemented as a small in-process signal: browser heartbeat to a `/presence` endpoint when the tab is focused; falls through to `Away` after idle. Routing decisions condition on this:

- `Urgency::Notify` while `Presence::Away` → falls back to email / phone push (only if the operator opts in)
- `Urgency::NextOpen` while `Presence::AtDesk` → still waits for explicit chat engagement; doesn't proactively chime in
- `Urgency::Page` always interrupts regardless of presence (this is the building-on-fire case)

Presence also conditions chat behavior: a courteous coworker who sees you've been heads-down for an hour doesn't lead with "btw I noticed 47 things" when you finally open the chat — they pick the most important and let you ask for the rest.

### The conversational chat (clippy companion)

A persistent right-side panel, always available, idle by default, speaks only when spoken to (or when explicitly opened, optionally with a brief "here's the agenda" if items wait).

Structurally it is just another `LensWorker`-shaped invocation, but with three new properties:

1. **One persistent thread.** `~/.sipag/strategy/thread.jsonl`. One thread per operator, continuous across days. The AI sees yesterday's conversation when you come back today.
2. **Focus / selection anchor.** As the user clicks items in the OKR panel, the chat's context shifts: "now looking at: auth Objective." That focus is injected as context into the next message. The user thinks of it as "pointing at" something the AI now has vision into.
3. **Inline mutation diff cards.** AI's replies can include `<mutation>...</mutation>` blocks (`refine_objective`, `propose_kr`, `propose_split`, etc.). UI renders each as an accept/edit/reject card inline. Accept applies the mutation through the same write paths as any operator-driven OKR edit.

The chat also has full access to corpus tools (`corpus.search`, `corpus.expand`) so the AI can ground every reply in observed work, not vibes. When the user asks "what's been happening on auth?" the AI does a corpus.search filtered by that Objective and summarizes the result honestly.

**The chat writes to the corpus by default.** Each human/AI exchange becomes a corpus item tagged `kind=strategy_chat, thread=...`. This is what makes the bridge worker's future fires able to ground in articulated human intent (e.g., the bridge worker sees "the human decided JWT yesterday" via corpus.search). A "private mode" toggle exists for sensitive runs that should stay out of the corpus.

### Genesis: how the experience starts

Most product onboarding fails because it asks the user to restructure existing work before getting value. Sipag's genesis is the opposite: **sipag joins what the user is already doing**.

Day 0 — install + 2-line `hosts.toml` + start serve. Board is mostly empty BUT a `misc` project shows the user's existing live katulong sessions, each with auto-summary and an "open in katulong" link. **The user did nothing — the sessions just appeared.** This is the first AHA.

Hour 1 — categorize gemma proposes: "this session looks like it's about auth. File under an 'auth' Objective?" User clicks [yes]. The Objective is born. The strategy chat opens on the right, says: "I created auth for you. Here are three candidate KRs based on what I've been seeing — or, if you want a different strategic frame entirely, tell me." User and AI converge on KRs via the chat. **Sipag asked a question the user should have asked themselves.** This is the second AHA.

Hours later — the bridge worker has been firing against the new KRs. A pinned `ask_human` card appears on one KR: "the agents picked JWT — is that the intended direction?" User clicks the answer; the answer writes back to the corpus; next bridge fire is aware. **The system noticed something the user wouldn't have noticed unless they'd been watching the session.** This is the third AHA.

After that — the loop is established. Targeted cards appear when something needs the user. The chat is there when the user wants to think. The corpus accumulates. KRs evolve through conversation. The user's time-per-day drops; the surface area covered doesn't.

### First-run: the wizard that isn't a wizard

**There is no separate wizard UI.** The chat is the chat. On day 1 it does a contextual welcome based on what sipag finds in the operator's environment; on day 30 it picks up where you left off. Same surface, different content.

The principle: **detection before conversation.** Before the AI sends its first message, sipag has already checked:

- `~/.katulong/remote.json` (is katulong wired?)
- `~/.ollama-bridge/remote.json` (is the bridge wired?)
- Observer poll state (are there live sessions to surface?)
- Board state (empty = first run; populated = returning user)

The AI never asks a question it could have answered from filesystem state. The first message is **conditional on what was found**.

#### Branch A — everything wired, live sessions exist

The common case for an operator who already has the dorky_robot stack running. Sipag detects existing sessions and offers to bootstrap the OKR structure from them.

```
AI: hey. caught you up on what i can see:

    katulong: connected (3 sessions live)
    ollama-bridge: connected
    board: empty (this looks like first run)

    your live sessions are:
      • ralph-auth-rewrite — 4 hours in, looks like real work
      • brian-flake-investigation — 2 hours, mostly debugging
      • ralph-docs-typo — idle 45 min

    a) i can look at what these sessions are about and propose
       Objectives — your sipag board grows from what's already
       happening
    b) you tell me what you're working on and we shape the
       Objectives together
    c) just use sipag as a session dashboard for now — i'll
       stay quiet unless you ping me

    or just say what you want.
```

If the user picks (a), the AI fires the categorize loop on the existing sessions, proposes Objective candidates with KRs, and the strategy chat handles refinement from there. This is the genesis story's first two AHAs collapsed into a single conversational turn.

#### Branch B — everything wired, no sessions

```
AI: hey. quick scan: katulong + ollama-bridge connected, board
    empty, no sessions running. so you're either setting up for
    later, or want to start something now. which?
```

#### Branch C — bridge missing

The chat panel shows a placeholder; the AI is genuinely dormant. The operator-facing prompt is small and clear:

```
[chat panel placeholder:]
AI dormant — wire ~/.ollama-bridge/remote.json to wake me.
You can still use sipag without me for dispatch + the board.
```

#### Branch D — katulong missing, bridge wired

The AI is awake but blind. It asks one question:

```
AI: hey. i'm awake (bridge is wired) but i can't see katulong.
    without it i can't see sessions, dispatch tasks, or really
    do most of what i'm here for. paste me the URL + bearer
    (saves to ~/.katulong/remote.json) or tell me where to find it.
```

### Principles that fall out of the first-run shape

A few design rules apply across all branches and onward into normal use:

**1. No step flow.** There's no "step 3 of 7" anywhere. The AI's job in any state is one well-judged message that handles the operator's actual situation.

**2. Detect, don't ask.** If sipag can know something from the filesystem or a quick HTTP probe, it knows it before saying anything. The wizard never asks "do you have katulong?" — it already checked.

**3. Volunteer context only when there's a gap.** First-run = huge gap (AI catches you up). Returning user = small gap (AI just acknowledges presence). Returning after 3-week vacation = medium gap (AI does a brief "here's what happened" recap). The rule: speak in proportion to what the user couldn't already know.

**4. Chunk when there's volume.** If the operator has 30 live sessions instead of 3, the AI groups into clusters rather than dumping 30 cards. "i see five rough clusters — want me to dig into any?" is better than 30 lines of session names.

**5. Mesh-aware but single-primary by default.** `~/.katulong/remote.json` is the primary connection; `hosts.toml` adds richness for multi-host deployments. The AI surfaces "you've got katulong on N hosts" but doesn't make the operator pick during first run.

**6. AI-is-real from minute one.** Because we assume the bridge is wired (per the [Scope note](#event-topology) — the bootstrap problem is the only deferred case), there is no templated phase. The first message the operator reads is gemma actually thinking. Honest product position.

### Single-user as N=1 (preparing for team mode without building it)

The team-vs-solo question is a fork in the architecture that, if deferred without preparation, becomes an expensive refactor later. The wizard's "are you solo or team?" branch makes the decision explicit — but for v1 the team arm is **graceful degradation**: "team mode is coming; for now sipag is single-user."

Three small commitments now keep the door open without the team-mode work:

- **KR / Objective ownership is a field** (defaults to "the operator"), not implicit.
- **Chat threads have an owner** (defaults to "the operator"), not singleton.
- **Notifications have a recipient** (defaults to "the operator"), not implicit.

Each is a small data-shape change. Single-user becomes the N=1 case of multi-user. When team mode lands, these fields gain real values; nothing about the v1 surface changes for solo operators.

### The bad-day flow

Trust in sipag is decided by what happens when an agent does something wrong. An operator will not run agents unattended unless they trust the recovery path. Get it wrong and nothing else matters.

**Sipag's bad-day flow is the *last* line of defense, not the first.** Prevention happens upstream at layers sipag does not own. Sipag's job in the bad day is what happens after those upstream layers were either insufficient or the operator explicitly chose to skip them (e.g., `claude --dangerously-skip-permissions` on a role).

#### The defense hierarchy

Per `[[feedback-fix-at-right-layer]]`: when a downstream tool already provides a guard, sipag should not reimplement it. Safety defenses for agent work live in seven layers, only three of which are sipag's:

| Layer | Owner | What it does |
|---|---|---|
| 1. Claude's judgment | Anthropic (the model) | Decides what to do; internal caution about destructive operations |
| 2. Claude Code's permission system | Anthropic (the agent) | Asks operator before bash, file writes, etc. — **the actual gate** |
| 3. Per-role permission policy | Operator (via role TOML) | `claude` vs `claude --dangerously-skip-permissions`; operator's choice per role |
| **4. Dispatch prompt template** | **Sipag** (`build_dispatch_prompt`) | Careful-behavior meta-instruction prepended at dispatch time |
| **5. Strategy lens proposal bias** | **Sipag** (strategy lens prompt) | Anti-destructiveness norm in the coworker preamble; KR proposals surface destructive operations in their text |
| **6. Bridge worker observation** | **Sipag** | Watches; classifies destructiveness; surfaces via urgency-routed cards |
| 7. Page modal + recovery | Sipag | This section — what happens when the upstream layers were insufficient |

The implication for what sipag should not build:

- **Sipag does not reimplement Claude Code's permission system.** That gate exists upstream (layer 2). Sipag duplicating it would be a second permission UI fighting Claude's, a latency hit on every command, and layer confusion about who's the source of truth.
- **Sipag does not auto-deny dangerous Claude actions in flight.** Layer 2 (Claude Code) is the gate. Sipag observes (layer 6) and recovers (layer 7); it does not intervene on the wire between Claude and the PTY.
- **Sipag does not silently restrict what the operator can do.** If the operator chose `--dangerously-skip-permissions` for a role, that's the operator's choice. Sipag may warn at dispatch time but does not refuse.

What sipag does provide at its three layers:

- **Layer 4 (dispatch prompt)**: `build_dispatch_prompt` includes careful-behavior framing. Not "don't do dangerous things" (Claude has its own judgment) but "be conservative; prefer reversible changes; ask permission with explicit framing of what would be lost."
- **Layer 5 (strategy lens bias)**: the shared coworker preamble includes anti-destructiveness norms. When the strategy lens proposes KRs, it prefers reversible-shaped ones. When a destructive operation is genuinely necessary, the KR text surfaces it explicitly — not buried in the falsifier, named in the proposal itself.
- **Layer 6 (bridge worker classification)**: gemma in bridge-worker mode classifies the destructiveness of observed actions; the resulting `ask_human` / Page urgency is conditioned on that classification + operator presence.

The bad-day section below covers layer 7. The rest of this section assumes layers 1–6 either failed, were skipped, or didn't catch the case.

#### The failure modes

| Mode | What happens | Detection | Urgency |
|---|---|---|---|
| **Pre-destructive** | Agent requests permission for something destructive (`rm -rf /`, force-push to main, drop table) | Bridge worker on `permission-request` event; gemma classifies destructiveness | `Notify` if operator at desk; **`Page` if operator away** |
| **Post-destructive** | Damage is done — wrong file deleted, force-pushed, etc. | Bridge worker on the event sequence that ended in destruction | `Page` always |
| **Stuck / lost** | Agent flailing — same kinds of actions repeatedly, no progress | Pattern lens: 10+ permission requests with same prefix; agent-done events not following | `Notify` |
| **Cross-session conflict** | Two agents stepping on each other (writing same files, contradictory decisions) | Periodic "conflict lens" correlates across session windows | `Notify` |
| **Off-track drift** | Agent making progress but in the wrong direction; KR lens producing "no relevant activity" while session is busy | Lens-worker action-rate going to zero against active session | `NextOpen` |
| **Failure cascade** | Test broke → build broke → agent panicked → tried 5 wrong fixes | Bridge worker on sequence of error events followed by destructive recovery attempts | `Page` if cascade is destructive; `Notify` if just spinning |

The two `Page`-level cases (post-destructive, destructive cascade) are the building-on-fire moments. The others are coworker-pulls-you-aside cases. The product policy in all of them is the same:

#### What sipag does in a Page event

The modal/notification carries more than "an agent did a thing." It carries the **context sipag has that the operator doesn't have in the moment**:

```
┌────────────────────────────────────────────────────────────────┐
│ 🔥  Destructive action — ralph-auth-rewrite                     │
│                                                                 │
│  what happened                                                  │
│    rm -rf /Users/felix/Projects/auth-worktree                  │
│    14 seconds ago                                              │
│                                                                 │
│  what sipag knows                                              │
│    • worktree was for branch feat/auth-jwt                     │
│    • last commit pushed to origin: abc123 (6 min ago)          │
│    • 3 uncommitted files at deletion                           │
│    • 2 of those 3 were already committed and pushed            │
│                                                                 │
│  what can be recovered                                          │
│    ✓  branch state (re-create from origin/feat/auth-jwt)       │
│    ✗  one uncommitted file (NEW src/jwt_test.rs)               │
│        — likely lost unless session scrollback has it           │
│                                                                 │
│  actions                                                        │
│    [ recreate worktree    ]  ← runs `git worktree add` locally │
│    [ view session scroll  ]  ← jump to katulong WS attach     │
│    [ kill ralph's session ]  ← stop further work; needs your   │
│                                 explicit click                  │
│    [ acknowledge          ]  ← I'll handle it manually        │
│                                                                 │
│  saved to corpus as fire_record id=4129                        │
└────────────────────────────────────────────────────────────────┘
```

The actions are operator-authorized — the click is the consent. None of them happen without it.

#### What sipag explicitly does NOT do

This is the trust foundation. Sipag's restraint is what makes it safe to run agents unattended:

- **Never auto-kills a session.** The agent might be doing real work the operator can see in scrollback. Killing is always an operator click.
- **Never auto-reverts a commit.** Might destroy intentional work. Revert is always an operator click.
- **Never auto-pauses an agent.** Might break a critical flow. Pause is always an operator click.
- **Never sends input that wasn't operator-typed.** Per `[[feedback-strict-layer-coupling]]` — Claude generates the agent's input, sipag never does. The Page modal's "kill session" action calls katulong's session-kill API; it does not type into the PTY.
- **Never escalates urgency without operator consent.** "I keep noticing X" doesn't become a Page event just because sipag thinks it's important. Page is reserved for the failure modes the operator pre-agreed are page-worthy (destructive + irreversible).

Sipag's superpower in a bad day is **context** — it knows the session history, the git state, the corpus, what was running, what was decided. Its restraint is to never act on that context without the operator's word.

#### Detection: who flags what

The bridge worker is the spine of detection. It's already watching `claude/<uuid>` events; it adds destructiveness classification to the existing pipeline:

- On every `permission-request` event, gemma (in the bridge worker fire) classifies the requested action: `safe`, `risky`, `destructive`. The classification result is a tag on the resulting `ask_human` verb. Urgency on the verb gets bumped to `Page` if `destructive` AND `Presence::Away`.
- On every `tool-use` event that completed a destructive command, the bridge worker records the action with its before/after context (git state, file existence, etc.) for use in the Page modal.
- A **cross-session conflict lens** runs periodically (probably hourly) — fires gemma against multiple session windows looking for write-overlap and contradictory decisions. Surfaces as `Notify`.

#### Recovery actions sipag owns

The Page modal's action buttons aren't free-form prompts; they map to a small set of operator-authorized operations sipag can perform:

| Action | What sipag does | Trust note |
|---|---|---|
| Recreate worktree | Local `git worktree add <path> <branch>` against the source repo | Operator-authorized; restores state, doesn't modify history |
| View session scrollback | Opens katulong WS attach in a new tab, scrolled to the destructive event | Read-only; just routes the operator |
| Kill session | Calls katulong's `session-kill` API for the session id | Operator-authorized; uses katulong's API surface, not direct PTY manipulation |
| Acknowledge | Marks the event in the corpus as "operator handled"; nothing else | No-op from sipag's side; defaults to manual recovery |
| Revert commit | Local `git revert <sha>` against the source repo (if branch is HEAD) | Operator-authorized; only available when revert is mechanical |

Notably absent: any action that modifies the agent's behavior mid-flight. Sipag pauses, kills, or watches — it does not steer.

#### Post-mortem to corpus

Every Page event writes a `fire_record` to the corpus tagged `kind=bad_day, severity=page, session=<id>`. This includes:

- The triggering event sequence
- What sipag knew at modal-render time
- Which recovery action the operator chose (if any)
- Whether recovery succeeded (operator can mark this; defaults to "unknown")

Future bridge worker fires that see this corpus item via `corpus.search` get to factor it in: "the last time ralph's session did X, the operator killed it." Becomes context, not just history.

#### The off-fire policy when operator is away

`Page` events always interrupt regardless of presence. The interrupt path:

1. Browser modal if sipag is currently focused (`Presence::AtDesk`)
2. OS notification if sipag tab is open but not focused (`Presence::Recent`)
3. Email + (if configured) phone push if operator hasn't been seen in 30+ min (`Presence::Away`/`Offline`)
4. **Default-deny timeout for pre-destructive cases**: if a destructive permission request is pending for > 30s with no operator response and the operator is `Away`, sipag denies on their behalf. The agent can ask again; the operator can reverse the policy with a setting.

The default-deny is the one place where sipag does take an action without explicit consent. It's the conservative choice — preventing damage is reversible (agent retries); allowing damage isn't (operator gets a destroyed worktree). Operators who want a different default can flip it in settings.

### Anti-sycophancy via coworker norms

The biggest risk in the strategy-chat shape is that conversational LLMs default to agreement. Without intervention, the AI would tell the user whatever they seem to want to hear, which is the failure mode every operator-facing AI product eventually hits.

The coworker frame solves this structurally. A coworker who agreed with everything you said would be useless to you — that's not "supportive collaboration," that's a yes-man, and you'd stop trusting them within a week.

So the shared preamble says explicitly:

> You're not here to make me feel good. You're here to help me think. If I'm wrong, say so. If I'm being lazy, say so. If a KR I just wrote sounds like ass-covering, name it. Be polite about it but be honest. Sycophancy is rude — it wastes my time by pretending to be helpful when it isn't.

This is reinforced by structural mechanisms:

- **Falsifier-as-gate on proposed mutations.** Every proposed KR mutation includes `falsified_by:`. The accept button is disabled until the user reads and either agrees to or edits the falsifier.
- **Always-propose-alternatives.** Every batch of proposed KRs includes at least one that points at a different strategic frame, not just refinements of the current one.
- **Lens health metrics** (already on queue): track action rate, no-action rate, and citation rate per lens/hat. If the strategy chat's recommendations are always accepted, that's a flag — either the AI is too cautious or the human is too compliant.
- **Bounded knowledge.** The AI knows what's in the corpus and OKR state. It doesn't pretend to know more. "I don't have that in my context" is the correct answer to many questions, not a failure.

### What this requires that doesn't exist yet

Pulled into the phase queue from this section:

- Shared coworker preamble (system-prompt-level convention; affects every gemma invocation)
- Urgency dimension on `StructuralAction`
- Notification router (`sipag/src/serve/notify.rs`)
- Presence primitive (`/presence` endpoint + state)
- Strategy chat MVP (persistent thread + selection anchor + mutation diff cards)
- First-run detection routine (startup check of remote configs + observer state + board state)
- Conditional first-message generation (chat AI fed the situation summary; picks the right branch)
- Categorize-into-Objectives as a chat-callable action (wires `categorize.rs` to the chat surface)
- Many-session chunking heuristics for branch A
- Single-user-as-N=1 data shapes (owner fields on KRs, Objectives, chat threads, notifications)
- Destructiveness classification on `permission-request` events (bridge worker extension)
- Cross-session conflict lens
- Bad-day Page modal UI + the operator-authorized recovery action set
- `fire_record` corpus item kind (for post-mortem write-back)
- Default-deny-on-Away policy for destructive permission requests
- Anti-destructiveness norm in the shared coworker preamble (layer 5 prevention)
- Careful-behavior preamble in `build_dispatch_prompt` (layer 4 prevention)
- KR proposals must surface destructive operations in the text (schema convention for `propose_kr`)
- Structural-verb dispatch to UI (already on queue; covers targeted-card rendering)
- Lens health metrics (already on queue, PR #571)

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
| — | Structural-verb dispatch to UI (targeted-card rendering) | none, but no consumer yet |
| — | **Lens health metrics** | none — cheapest experimentation-gap plug |
| — | **Shared coworker preamble** | none — system-prompt-level convention |
| — | **Urgency dimension on `StructuralAction`** | none |
| — | **Notification router** (`sipag/src/serve/notify.rs`) | urgency dimension + presence primitive |
| — | **Presence primitive** (`/presence` endpoint + state) | none |
| — | **Strategy chat MVP** (persistent thread + selection anchor + mutation diff cards) | shared preamble + structural-verb UI |
| — | **First-run detection routine** (startup probe of remote configs + observer state + board state) | none |
| — | **Conditional first-message** in the chat (situation summary fed to gemma; correct branch picked) | first-run detection + strategy chat MVP |
| — | **Categorize-into-Objectives as a chat action** | strategy chat MVP |
| — | **Many-session chunking** (cluster N sessions into ~5 groups for first-run branch A) | none |
| — | **Single-user-as-N=1 data shapes** (owner fields on KRs / Objectives / threads / notifications) | none — preparation for team mode without building it |
| — | **Bad-day Page modal** (UI surface + operator-authorized recovery action set) | urgency dimension + notification router |
| — | **Destructiveness classification** on `permission-request` events (bridge worker extension) | structural-verb dispatch to UI |
| — | **Cross-session conflict lens** | lens scheduler (shipped) |
| — | **`fire_record` corpus item kind** (post-mortem write-back from Page events) | corpus (shipped) |
| — | **Default-deny-on-Away policy** for destructive permission requests | destructiveness classification + presence primitive |
| — | **Anti-destructiveness norm in coworker preamble** (layer 5 — strategy lens biases away from destructive-shaped KRs) | shared coworker preamble |
| — | **Careful-behavior preamble in `build_dispatch_prompt`** (layer 4 — meta-instruction prepended at dispatch time) | none |
| — | **KR proposal schema: surface destructive operations in the text** (layer 5 — `propose_kr` mutation must name destructive scope in KR body, not hide in falsifier) | strategy chat MVP |

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
