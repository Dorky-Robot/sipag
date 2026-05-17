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

- **Steering is human, execution is agent.** The human surface is Objectives + KRs + Standing + Ideas. Tasks/trials live *below* the human surface.
- **The agent loop is empirical, not procedural.** Spike → observe → iterate. OKRs provide the reward signal; the substrate provides gradient ascent. *"It's the coding version of reinforcement learning... the Thomas Edison of experiment, observe, tweak, iterate"* (memory: `project-sipag-work-model-experimentation`).
- **The fix happens at the right layer.** When the right home for a signal is in a tool we own (katulong, kubo), extend that tool — don't add sipag-side workarounds (memory: `feedback-fix-at-right-layer`).

These three principles drive every contextual choice below.

---

## 1. The context map

Four bounded contexts. Each owns its language; translation happens at named anti-corruption layers (§6).

| Context | Side of the line | What it owns | Posture |
|---|---|---|---|
| **Steering** | human | direction + reward signal | strategic, qualitative, durable |
| **Experimentation** | agent | the spike-observe-iterate loop | tactical, empirical, fast |
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
                         │     spike → observe → iterate       │
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

**Responsibility.** Run the spike-observe-iterate loop. Pick something to try, fire it, collect feedback, decide the next move, push KR-level stance back to Steering when warranted.

**Ubiquitous language.**

- **Nouns**: `Experiment`, `Hypothesis`, `Trial`, `Spike`, `Outcome`, `IteratePolicy`, `WorkflowStatus` (per-trial state, distinct from `KrStance`)
- **Verbs**: `start_experiment(hypothesis, kr_ref) -> Experiment`, `spike(experiment, intent) -> Trial`, `observe(trial) -> Outcome`, `iterate(experiment, outcome) -> NextAction`, `conclude(experiment, stance)`
- **NOT in the language** (deprecated kanban verbs): `refine_features`, `group_features`, `move_task`

**Aggregates.**

- `Experiment` (root, **first-class on disk**) — has a `Hypothesis`, targets a `KrRef`, has a lifecycle (active / paused / concluded), holds running notes / strategy
- `Trial` (root, child of Experiment) — one iteration; holds the spike action, the resulting `Outcome`, and the `WorkflowStatus`
- `Outcome` (value object) — typed reaction to feedback signals (logs, rescues, test failures, lints, HTTP codes, pane behavior, classifier verdicts)
- `IteratePolicy` (trait) — pluggable per-experiment: Claude-in-the-loop, deterministic retry, gemma classifier, rule-based, etc.

**Sub-responsibilities.**

- **act** — fire the spike. Currently the dispatch path: katulong session create, agent command exec, worktree setup.
- **observe** — collect the feedback signal. Currently: pane scrollback fetch + gemma classification. Future: katulong pub/sub subscriber driving typed `Outcome`s directly (most signals don't need an LLM call — see katulong issues [#715](https://github.com/Dorky-Robot/katulong/issues/715) / [#716](https://github.com/Dorky-Robot/katulong/issues/716)).
- **iterate** — decide what to try next given the latest outcome. This is where the `IteratePolicy` plugs in.

**Where it lives today.**

| Piece | Path | Status |
|---|---|---|
| Task + TaskStatus + WorkflowStatus | `sipag-core/src/board/task.rs` | 🟧 — `Task` is the current name for what should become `Trial`; `TaskStatus` is workflow state |
| Role (agent command template) | `sipag-core/src/board/role.rs` | 🟡 — Experimentation infrastructure |
| Pre-dispatch classifier ("gate") | `sipag-core/src/gate.rs` (346 LOC) | 🟧 — fold into `observe` |
| Post-dispatch observer | `sipag-core/src/nudge.rs` (417 LOC, mid-repositioning) | 🟧 — fold into `observe` |
| Recovery loop (`verify_and_heal_dispatch`) | `sipag/src/serve/htmx.rs:1577` | 🔴 — **delete** when attach client owns keystrokes (per existing dispatch-implementation-plan §7) |
| Observation aggregate | `sipag-core/src/board/observation.rs` | 🟧 — belongs to Experimentation, not the board |
| LLM / gemma client | `sipag-core/src/llm.rs` (300 LOC) | 🟡 — Experimentation's observe-side infrastructure |
| Dispatch mechanics (CLI + TUI paths) | `sipag/src/cli.rs`, `tui/src/board_app.rs:353` (uses sync `KatulongClient::from_remote_json()` + calls `katulong::{session_name, worktree_command, agent_command}` inline — the exact dispatch-policy helpers §9 #9 lifts) | 🟧 — the `act` sub-module; **TUI is the third dedup site** alongside CLI and serve |
| Dispatch mechanics (web path) | `sipag/src/serve/htmx.rs` + URL builders | 🔴 — same act surface duplicated, plus the #527 unbounded-body vector |
| Refinement pipeline | `sipag-core/src/feature.rs` (847), `sipag-core/src/refine.rs` (1367) | ⛔ **deprecated** — kanban-shaped; replaced by Experimentation. Don't delete; preserve as "we tried this" per `[[feedback-deprecate-with-rationale]]`. Strip wiring, add deprecation note pointing at this doc. |
| Categorize loop | `sipag/src/serve/categorize.rs` | 🟧 — fold into `observe` / `iterate` |
| Background workers (expand, research, scheduler) | `sipag/src/serve/workers/` | ❓ — what's the seam against `observe` / `iterate`? Same code path, different trigger? |

**What's missing.**

- `Experiment` aggregate (first-class on disk; per-experiment hypothesis + strategy + lifecycle)
- `Trial` rename / repositioning of `Task`
- `Outcome` typed value object (currently outcomes are implicit / scattered)
- `IteratePolicy` trait + at least one default implementation
- Event-driven observer (consumes katulong pub/sub)

---

## 4. Topology context (platform — where things run)

**Responsibility.** Model the mesh of machines / katulong instances / ollama hosts. Own the wire protocols sipag talks. Federate when needed.

**Ubiquitous language.**

- **Nouns**: `Host`, `Peer`, `MeshTopology`, `KatulongInstance`, `OllamaHost`, `TmuxSession`, `WireOutcome`
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
| `sipag-d-<hex>` dispatch session | Experimentation | `DispatchSession` (or fold into `Trial`) | greenfield — no type today, just a name pattern from `generate_dispatch_session_name`. Decision deferred to `Trial` introduction (Phase 1 #3). |
| `Status` (in `Project`, column name) | Steering | `ColumnName` or `BoardStatus` | 🟡 **deferred** — see §10 domain-vs-schema-noun question |
| `TaskStatus` | Experimentation | `WorkflowStatus` (per-trial) | 🟡 deferred until §10 resolved (likely also affected by Phase 1 #3 `Trial` rename) |
| `SessionStatus` (alive / has-child) | Topology | `TmuxSessionStatus` | ✅ done 2026-05-17 |
| `KrStance` | Steering | already unambiguous ✓ | ✅ |

### Named ACLs

| From | To | What gets translated |
|---|---|---|
| Steering: `Idea` | Experimentation: `Experiment` | `promote_idea` — the only way Steering crosses into Experimentation |
| Experimentation: `Outcome` (terminal) | Steering: `KrStance` | `report_stance` — agent's only push back up |
| Topology: `TmuxSession` | Experimentation: `Trial.session_ref` | dispatch returns a reference; Experimentation never holds the raw type |
| Topology: katulong pub/sub events | Experimentation: `Outcome` | `KatulongEvent → Outcome` mapper in observe sub-module |
| Topology: ollama response | Experimentation: `Classification` | typed parsing inside `llm-client`; only validated types leave |

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
| `sipag-core/src/board/task.rs` | Experimentation as `Trial` | rename, repurpose |
| `sipag-core/src/board/role.rs` | Experimentation (`act` sub-module's command template) | |
| `sipag-core/src/board/observation.rs` | Experimentation (`Outcome` raw material) | |
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
| `sipag/src/serve/workers/{expand,research,scheduler}.rs` (plus `mod.rs`) | ❓ Experimentation `iterate` or its own thing? — `expand` / `research` look like agent-driven iteration; `scheduler` looks more cross-cutting. Triage individually. |
| `sipag/src/serve/categorize.rs` (199 LOC) | Experimentation `observe` | gemma-driven classification of board items — same trust boundary as the rest of `observe`. |

---

## 9. Extraction queue — three phases (structural language → bug-fix-driven → cleanup)

**Sequencing principle.** When a refactor combines vocabulary changes with behavior changes, the structural language lands *first*. Every behavior PR that ships in the old vocabulary entrenches it and inflates the eventual rename. Language-first means subsequent bug-fix and extraction PRs naturally write the new vocabulary, so the codebase migrates organically instead of needing a Big Bang rewrite.

The trade-off: Phase 1 PRs don't close open bugs. They earn their keep by making Phase 2 PRs smaller and self-consistent.

**Accepted risk:** Phase 2 bug fixes #6 (closes sipag #527) and #11 (closes sipag #528 by deletion) close *live security-adjacent vectors* (unbounded response body, unsafe LLM→PTY recovery loop). Strict serialization of Phase 1 before Phase 2 leaves both open longer than necessary. **Interleaving is permissible** once Phase 1 #1 + #2 (this PR + the mechanical naming pass) land — at that point the Topology context's wire vocabulary is settled enough that #6's new async HTTP client can ship in the right language without waiting for the rest of Phase 1. Items #3-#5 (Experiment aggregate, Idea ACL, Agent API types) are mostly Steering/Experimentation work and don't block Topology PRs.

### Phase 1 — structural language (front-loaded; no bugs closed yet)

1. **Deprecate `feature.rs` + `refine.rs`.** Experimentation. Strip wiring, add deprecation notes per `[[feedback-deprecate-with-rationale]]`. Stops the old kanban language from competing with the new. Lowest coupling, ships first.
2. **Naming disambiguation pass.** Cross-cutting. Mechanical rename per the §6 table — splitting overloaded `Session` and `Status` across contexts. Sized as multiple focused PRs (one per context) to keep diffs reviewable. **Partially landed 2026-05-17**: katulong-client's `Session` → `TmuxSession` and `SessionStatus` → `TmuxSessionStatus` done; auth's `Session` → `AuthSession` deferred (no in-file collision today; revisit when auth is touched — see §6 row for the full rationale); all `Status` renames deferred until §10 domain-vs-schema-noun question resolves; `ClaudeSession` / `DispatchSession` are greenfield names for types that don't exist yet.
3. **`Experiment` + `Trial` + `Outcome` + `IteratePolicy` as first-class types.** Experimentation. Additive — introduce alongside `Task`, alias `Task = Trial` for transition. Includes moving `board/observation.rs` into Experimentation. First-class on-disk format for `Experiment` per [[project-sipag-work-model-experimentation]].
4. **`Idea` aggregate + `promote_idea` ACL.** Steering ↔ Experimentation. First instance of a named cross-context translation; sets the pattern for future ACLs.
5. **Agent API published-language types.** Steering. Define the typed shapes for read-only KR/objective views and `report_stance` commands — *types only*, no endpoint wiring yet. Establishes the protocol so Phase 2 work can write toward it.

### Phase 2 — bug-fix-driven (now using the new vocabulary)

6. **katulong-client async HTTP client + body cap.** Topology. **Closes sipag #527** structurally. Shrinks htmx.rs and kills katulong_proxy.rs. ~200 LOC + 9-site migration. Speaks new Topology language (`TmuxSession`, `TmuxSessionStatus`).
7. **katulong-client SSE subscriber.** Topology. Third wire surface alongside WS attach + HTTP. Built against today's `claude/<uuid>` topic; gains `sessions/<id>/*` for free when katulong#715/#716 land. Emits `Outcome`s in Experimentation's language at the ACL.
8. **Promote `llm.rs` to `ollama-client` with typed response shapes.** Topology. Sets up #528 closure by giving validated typed outputs a home. Sized for consumption by Experimentation's `observe`.
9. **Unify gate + nudge into `session_classifier`** (Experimentation's `observe` core). Depends on #3 (the `Outcome` type) + #8.
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
- **§3 IteratePolicy plug shape**: trait with a `decide(experiment, outcome) -> NextAction` method, or richer (multi-step planning)?
- **§3 workers/**: relationship to `observe` / `iterate`. Are `expand`/`research`/`scheduler` background-triggered iteration policies, or a separate kind of work?
- **§4 hosts.rs vs mesh.json**: overlapping topology configs. One source of truth or two?
- **§4 pubsub.rs** — sipag's broker is **load-bearing**, not a deprecation candidate. Round-1 review claimed sipag was consumer-only based on a faulty grep; the actual state (verified 2026-05-17) is that `sipag/src/serve/` has 16+ `.publish(` call sites — `workers/{expand,research,scheduler,mod}.rs`, `htmx.rs` (6), `observers.rs`, `board_view.rs`, `ws.rs`. Topics include `workers/activity`, `observations/activity`, plus per-item `discourse_topic()` channels. The broker serves sipag's own intra-process consumers (UI updates, worker coordination). The real architectural question is whether sipag's internal topics should be **published into katulong's broker** instead of sipag running its own — per `[[feedback-fix-at-right-layer]]`. That would unify the pub/sub seam at the katulong layer but requires sipag's UI subscribers to round-trip through the network. Worth weighing, but not until queue items #2 + #7 land — first prove out the katulong consumer path so the consolidate-vs-keep call has both ends working.
- **§4 Topology context scope**: today §4 bundles mesh/host config + wire protocols + protocol shapes. These are different shapes ("Topology" feels like infra-config; "Wire" feels like client libraries). Should **Wire** be a sibling context to **Topology**, with `katulong-client` / `ollama-client` living under Wire and `hosts.rs` / mesh.json under Topology? Affects where the response-size cap conceptually lives (§7).
- **§6 ColumnName + WorkflowStatus + TmuxSessionStatus**: are these *domain nouns* or *wire/presentation schema nouns*? `ColumnName` smells like a UI presentation concern that might never need to appear in `sipag-core`; `TmuxSessionStatus` might be a wire response shape, not a domain type. Worth distinguishing "domain language per context" from "schema language per wire/UI seam" so we don't pollute domain modules with concerns that only matter at boundaries.
- **§6 type prefixes vs module paths**: prefer `Trial`, `Outcome`, `Stance` reached via module paths (`experimentation::Trial` vs `steering::KrStance`)? More idiomatic Rust; less visible at call sites.
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
- 2026-05-17 — review-fix round 1 on PR #537 — corrected the deferral rationale for auth's `Session` rename (call sites import bare `Session` via re-export, NOT the module-path-qualified form the prior wording claimed); annotated `sipag/src/serve/observers.rs::KatulongSession` as the informal sipag-side ACL distinct from the new `katulong_client::TmuxSession`, with a candidate-for-naming reference to Phase 1 #3 (`Outcome`).
- 2026-05-17 — review-fix round 2 on PR #537 — round-1 rationale fix landed in §6 but the parallel sentence in §9 #2 still parroted the old "module path already disambiguates" wording. Updated §9 #2 to point at §6 for the full rationale.
