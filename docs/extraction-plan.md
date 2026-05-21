# Extraction plan — sipag as lego blocks

> **Companion to [`modules.md`](modules.md) §9 (the extraction queue) and [`narrative.md`](narrative.md) (the product spine).**
>
> The queue tells you **what capabilities ship in what order**. This doc tells you **which crates come out and in what order** to support those capabilities. Same work, different cut.

The premise: we don't need a v2. We need to finish the extraction pattern that PR #535 (`katulong-client`) started. Most of what sipag-core owns is *already module-shaped* — it just hasn't been given a `Cargo.toml`. The mess that's left after extraction (mainly `sipag/src/serve/htmx.rs`) is in *one* place, not spread across the workspace.

This plan is incremental, reversible, and preserves history. It is not a rewrite.

---

## 0. The principle

**Extract; don't rewrite.** Each candidate module already exists as an internal module inside `sipag-core/` with the right shape: single responsibility, clear types, internal-only consumers. The extraction recipe is mechanical:

1. `cargo new --lib <crate>` in the workspace
2. `mv` the source files
3. Add the new crate as a dep in `sipag-core` (or wherever it's consumed)
4. Re-export at the old path for one cycle so consumers don't break (`pub use <new-crate> as <old-name>`)
5. Migrate consumers crate-by-crate; drop the re-export when zero callers remain

`katulong-client` was the proof. The `pub use katulong_client as katulong;` shim at `sipag-core/src/lib.rs:30` is the template — keep doing that.

**Out of scope for this doc:** the lens-worker design, the recording-API verbs, the dispatch-policy questions. Those live in `modules.md` §3 + §9. This doc is structural only.

---

## 1. Today

```
sipag/
├── katulong-client/      ✅ extracted (PR #535); + async HTTP client + body caps (PR #546)
├── ollama-bridge-client/ ✅ extracted (PR #549) — wire client for the bridge daemon
├── sipag-auth/           ✅ extracted (PR #554) — webauthn / passkeys / sessions
├── sipag-board/          ✅ extracted (PR #553) — OKR / Task / Role / Project / Observation
├── sipag-corpus/         ✅ extracted (PR #550) — vector store + Embedder trait
├── sipag-dispatch/       ✅ extracted (PR #547) — the dispatch action
├── sipag-lens/           ✅ extracted (PR #551) — Lens / LensWorker / 4 verbs / ModelChoice
├── sipag-mesh/           ✅ extracted (PR #552) — hosts.toml + multi-host topology
├── sipag-pubsub/         ✅ extracted (PR #545) — file-backed durable broker
├── sipag-core/
│   └── src/
│       ├── config.rs       ← stays (cross-cutting; tiny)
│       ├── feature.rs      ⛔ deprecated
│       ├── refine.rs       ⛔ deprecated
│       ├── gate.rs         ⛕ retiring (Phase 2 #9 — folds into lens-worker abstraction)
│       │                   (nudge.rs deleted in §9 #11; no successor needed)
│       ├── llm.rs          ⛕ retiring (callers migrate to `ollama-bridge-client`)
│       └── lib.rs          (thin re-exports of katulong-client / pubsub / mesh / board / auth)
├── sipag/             (binary: CLI + serve/)
│   └── src/serve/htmx.rs   ~1620 LOC — remaining mess is route plumbing + the gate (legacy nudge loop deleted in §9 #11)
└── tui/               (binary: ratatui board)
```

**All 8 planned extractions complete.** Eleven workspace crates plus two binaries. `sipag-core` is now a thin compatibility shim around the extracted crates plus a handful of retiring modules (gate + llm — scheduled to retire with the lens-worker scheduler landing, per modules.md §9 #9). `nudge` already retired in §9 #11 (deleted alongside `verify_and_heal_dispatch`).

External dependencies sipag relies on (not in this repo, but called out so the structure-only readers know what's outside the boundary):

- **[`dorky-robot/katulong`](https://github.com/Dorky-Robot/katulong)** — JS/Node terminal session server. sipag talks to it through `katulong-client`.
- **[`dorky-robot/ollama-bridge`](https://github.com/Dorky-Robot/ollama-bridge)** — Elixir queue+auth daemon in front of `ollama serve`. Solves the "three machines hit one GPU host concurrently and gemma4:31b unloads/reloads between requests" problem with a single-worker queue + sha256 dedup + bearer auth. sipag will talk to it through a thin `ollama-bridge-client` crate (Phase 2 #8 — see §3 below). **Sipag never talks to ollama directly** — same strict-layer-coupling discipline as `Claude → katulong → sipag` ([memory: `feedback-strict-layer-coupling`](../.claude/memory/feedback_strict_layer_coupling.md)).

## 2. Target

**Target reached.** Nine planned workspace crates + sipag-core (shim) + sipag (binary) + tui (binary). The target tree from earlier drafts is now the current state — see §1 above.

The remaining work isn't more extraction; it's the **post-extraction cleanup**: retire `sipag-core/src/gate.rs` + `llm.rs` (callers migrate to the lens-worker abstraction; `nudge.rs` already retired in §9 #11), let `sipag-core` shrink to just `config.rs` + the re-export shims, and then eventually consider dissolving sipag-core itself once the shims have aged out. That's all in modules.md §9 Phase 2 + Phase 3, not in this doc.

---

## 3. Per-crate extraction recipe

Each row: what it owns, why it earns crate status, what's in scope for v1 of the crate, what's deferred. Sequencing rationale is in §4.

### `sipag-pubsub` — file-backed broker ✅ done (PR #545)

- **Owns:** `Broker`, `Envelope`, JSONL append/tail per topic, in-process fan-out via `tokio::sync::broadcast`.
- **Source:** `sipag-pubsub/src/lib.rs` (was `sipag-core/src/pubsub.rs`, 441 LOC + 9 tests, moved via `git mv` so history follows).
- **Outcome:** template-prover. The katulong-client extraction (PR #535) gave us the pattern; this extraction confirmed it works for a second case. `sipag-core` re-exports at the old `sipag_core::pubsub::…` path for back-compat; the sipag binary's 5 import sites migrated to direct `sipag_pubsub::…`. The §3 recipe held with no surprises beyond needing `mkdir -p` before the new crate dir could be written into.

### `sipag-dispatch` — the dispatch action ✅ done (PR #547)

- **Owns:** the WS-attach + launch + paste + submit + processing-wait flow, plus optional worktree setup. One `dispatch(remote, &session, input, on_step)` function with typed `DispatchInput` and typed step-named errors.
- **Source:** `sipag-dispatch/src/lib.rs` (~500 LOC + 10 tests). What used to live in `sipag/src/serve/htmx.rs::dispatch_via_attach_client` (~240 LOC) and overlapping logic in `sipag/src/cli.rs::run_dispatch_task`.
- **Outcome:** the highest-payoff extraction. `htmx.rs` shrank by ~440 LOC. Two real behavior unifications shipped:
  - The web UI v2 dispatch now does worktree setup (previously skipped — `role.worktree = true` dispatches ran Claude in the main repo).
  - The CLI now uses WS-attach orchestration (TUI-ready wait, paste echo, processing wait) instead of fire-and-forget HTTP `/exec`.
  Loaders + prompt composition stayed outside the crate; the gate + `verify_and_heal_dispatch` stayed in `htmx.rs` (retire separately per §9 #11). Pre-created session is an input (not a side effect) so the gate can inspect before dispatching.
- **TUI not yet migrated:** `tui/src/board_app.rs` still uses sync HTTP `/exec` via `agent_command`. Follow-up PR.

### `ollama-bridge-client` — wire to the bridge daemon ✅ done (PR #549)

- **Owns:** Rust client for [`dorky-robot/ollama-bridge`](https://github.com/Dorky-Robot/ollama-bridge) — `POST /enqueue` + `GET /jobs/:hash` poll + the pass-through probe endpoints (`/api/tags`, `/api/show`, `/api/ps`) + a `submit_and_wait(endpoint, body, timeout)` convenience that hides the polling. Bearer auth on every call. See [memory: `reference-ollama-bridge`](../.claude/memory/reference_ollama_bridge.md) for the bridge's wire shape.
- **Source today:** **does not exist.** Net-new (parallels `katulong-client`).
- **Why extract:** the bridge is the actual integration point — sipag never talks to ollama directly. The wire client is the thin Rust shim. Same shape as `katulong-client`: workspace crate, sipag's own, separately testable.
- **In scope v1:** the four wire methods + the convenience wait helper. Bearer auth, body cap per #527 hygiene, typed errors. `~/.ollama-bridge/remote.json` reader for connection details (parallels `~/.katulong/remote.json`). Built-in handling for the bridge's 503-with-Retry-After queue-full response.
- **NOT in scope:** direct ollama calls — the bridge refuses them with 409 anyway. Model choice per call (model lives in the per-job body, not in the client constructor). `LlmClient` trait — the shape is fundamentally different from a synchronous chat trait; if a unifying abstraction emerges it lives in `sipag-lens`, not in this client.
- **Downstream knock-on:** `Profile` (Fast/Strong/CodeAware) per-lens still has a home in sipag — each lens-worker resolves its `ModelChoice` to a concrete model string and includes it in the enqueue body. `~/.sipag/models.toml` still makes sense as the friendly-name → concrete-model resolver. Only the wire client shape changes; the profile-resolution UX from modules.md §9 #3 stays.
- **Unblocks:** modules.md §9 #3 (lens-workers — they enqueue chat/generate jobs through this client), #8 (this extraction IS that item, reframed), corpus embedding (jobs at `/api/embed`).
- **Replaces in plan:** the previous "`ollama-client` with `LlmClient` trait + per-model construction" slot. That design was drafted before this session surfaced the bridge as the actual integration point. The dedup window (60s, sha256 of `{endpoint, body}`) is a free win for lens-workers that hit similar prompts.

### `sipag-corpus` — local vector store ✅ done (PR #550)

- **Owns:** append-only `CorpusItem` log, embeddings (via `ollama-bridge-client` → bridge → `/api/embed`), similarity search, `corpus.search` / `corpus.expand` MCP-shape tools.
- **Source today:** **does not exist.** This is net-new (modules.md §9 #3).
- **Why crate-first:** if we build it as a `sipag-core` module the lens-worker abstraction will compile-couple to corpus internals; cleaner to draw the boundary upfront.
- **In scope v1:** persistence (JSONL + a small index file), embed-on-write via `ollama-bridge-client::submit_and_wait("/api/embed", …)`, k-NN search, tag filtering, timestamp range filtering.
- **Deferred:** cross-corpus retrieval (sipag ↔ diwa per modules.md §10), corpus compaction, lens-citation graph.
- **Depends on:** `ollama-bridge-client` for embeddings.

### `sipag-lens` — lens-worker primitive ✅ done (PR #551)

- **Owns:** `Lens`, `LensWorker`, the four verbs (`observe` + `suggest_stance` + `ask_human` + `propose_task`), trigger policy, `ModelChoice` → concrete-model resolution.
- **Source today:** **does not exist.** This is net-new (modules.md §9 #3).
- **Why crate-first:** this is the abstraction that's supposed to unify the surviving gate prototype + future observers. (Earlier drafts named `nudge.rs` here as a sibling; that module retired in modules.md §9 #11 since the WS-attach path doesn't need post-dispatch keystroke retry.) If it lives inside `sipag-core` it will accidentally couple to board/auth/etc. The whole point of the abstraction is that it's substrate.
- **In scope v1:** runtime that takes a lens definition + bridge endpoint + corpus handle + trigger policy and produces corpus writes plus typed verb calls. Each worker resolves its `ModelChoice` (default / named / Fast / Strong / CodeAware) via `~/.sipag/models.toml` and includes the concrete model name in the bridge enqueue body. Bridge lens-worker as the first instance.
- **Deferred:** lens governance / sprawl ranking (modules.md §10), meta-cognitive guardrails (§10), ad-hoc lens expiry (§10).
- **Depends on:** `sipag-corpus`, `ollama-bridge-client`, `sipag-board` (for `KrStance` writes).

### `sipag-board` — OKR + Task domain ✅ done (PR #553)

- **Owns:** `Objective`, `KeyResult`, `Task`, `Role`, `Project`, `Observation`, TOML persistence under `~/.sipag/`.
- **Source today:** `sipag-core/src/board/` (well-organized — already 7 files with `mod.rs`).
- **Why extract:** large surface, well-factored, multiple consumers (tui, serve, dispatch, lens). Crate boundary forces the test pyramid to live here instead of being inherited from sipag-core.
- **In scope v1:** mechanical move. Keep all types and signatures.
- **Deferred:** the `Status` rename pass (modules.md §6 / §9 Phase 1 #2) lands separately as language-first work.
- **Depends on:** `sipag-pubsub` (it publishes change events on save).

### `sipag-mesh` — host topology ✅ done (PR #552)

- **Owns:** `Host`, `hosts.toml` reader, multi-host resolution.
- **Source today:** `sipag-core/src/hosts.rs` (117 LOC).
- **Why extract:** small but a different concern from board state. Lets `sipag-dispatch` depend on it without dragging the OKR domain along.
- **In scope v1:** mechanical move.
- **Deferred:** the §10 `hosts.rs` vs `mesh.json` reconciliation (one source of truth or two?).

### `sipag-auth` — identity ✅ done (PR #554)

- **Owns:** webauthn, passkeys, sessions, devices, tokens.
- **Source today:** `sipag-core/src/auth/` (multiple files, already crate-shaped internally).
- **Why extract:** modules.md §9 #15. Lowest urgency — works fine today, just big. Extract once it stops changing weekly.
- **Deferred:** could become `dorky-auth` (mesh-shared) instead of `sipag-auth` — see §10 crate-naming question.

---

## 4. Sequencing — why this order

The order matters because each extraction either (a) unblocks a downstream queue item in modules.md §9 or (b) provides a template for the next, harder extraction.

```
1. sipag-pubsub          ✅ done — PR #545 (template-prover)
2. sipag-dispatch        ✅ done — PR #547 (biggest htmx.rs cleanup; enabled modules.md §9 #11)
3. ollama-bridge-client  ✅ done — PR #549 (closed §9 #8, reframed)
4. sipag-corpus          ✅ done — PR #550 (storage half of Phase 1 #3)
5. sipag-lens            ✅ done — PR #551 (runtime half of Phase 1 #3)
6. sipag-board           ✅ done — PR #553 (mechanical move out of sipag-core)
7. sipag-mesh            ✅ done — PR #552 (mechanical move out of sipag-core)
8. sipag-auth            ✅ done — PR #554 (mechanical move; closes §9 #15)
```

**All eight planned extractions complete (2026-05-20).** The plan is done.

**Dependency direction.** Drawn as edges (`X → Y` means X depends on Y):

```
sipag-lens             → sipag-corpus, ollama-bridge-client, sipag-board
sipag-corpus           → ollama-bridge-client
sipag-dispatch         → katulong-client (done)
sipag-board            → sipag-pubsub
ollama-bridge-client   → (none — leaf)
sipag-pubsub           → (none — leaf, extracted)
sipag-mesh             → (none — leaf)
sipag-auth             → (none — leaf)
katulong-client        → (extracted; leaf)
```

No cycles. Every leaf is extractable in isolation. The binary depends on all of them.

**Interleaving with modules.md §9.** All landed:

| Extraction | §9 item closed/reframed |
|---|---|
| `sipag-pubsub` ✅ PR #545 | none — pure infrastructure win |
| `sipag-dispatch` ✅ PR #547 | **#12** (lift dispatch policy out of the wire crate). Unblocked #11 (`verify_and_heal_dispatch` deletion) and #13 (htmx.rs split). |
| `ollama-bridge-client` ✅ PR #549 | **#8** in the queue; reframed from "promote llm.rs" to "wrap the bridge daemon." |
| `sipag-corpus` + `sipag-lens` ✅ PRs #550 + #551 | **part of #3** — the storage + runtime halves of the lens-worker abstraction. |
| `sipag-board` ✅ PR #553 | enables clean #2 renames per-crate instead of one giant cross-cutting PR. |
| `sipag-auth` ✅ PR #554 | **#15** in the queue. |
| `sipag-mesh` ✅ PR #552 | feeds the §10 hosts/mesh reconciliation question. |

---

## 5. What we are NOT extracting

Each absence is deliberate. They're listed here because they're tempting candidates that don't earn their keep.

- **`sipag-cli`.** The CLI is a binary, not a library. It composes other crates; it isn't depended on by anything. Don't add a layer.
- **`sipag-serve`.** Same as above. The web UI is a binary surface (axum routes + maud views). It will shrink as `sipag-dispatch` + `sipag-lens` land. Splitting it from the CLI gains nothing.
- **`sipag-tui`.** Already its own binary crate (`tui/`). Stays as-is.
- **`sipag-config`.** `config.rs` is 50ish lines and resolves one env var. Keeping it in `sipag-core` is correct.
- **A `sipag-prelude` re-export crate.** No prelude. If a consumer needs three of our crates it imports three of our crates. The clarity of explicit imports beats the convenience.
- **`sipag-llm` as a separate crate.** No wrapper around `ollama-bridge-client`. The bridge is the integration point; the client is the wire shim. If a second backend ever exists (anthropic-bridge, openai-bridge), each gets its own wire-client crate the same way. The cross-cutting abstraction (lens-worker invokes any backend) lives in `sipag-lens`, not in a separate wrapper crate.

---

## 6. Risks + mitigations

- **Re-export shim churn.** Each extraction adds a `pub use new_crate as old_module;` line in `sipag-core/src/lib.rs`. Keep them for one full release cycle, then drop. The katulong-client shim has been in place since PR #535 (~ a month) and is fine.
- **Cargo build-time regression.** Splitting one crate into nine increases link work. Mitigation: most consumers depend on a small subset, not all nine. Measure after the third extraction; if `cargo build` regresses meaningfully, consolidate the leaf crates (`sipag-mesh` could fold into `sipag-board` if needed).
- **Test ergonomics.** Cross-crate test fixtures get harder. Mitigation: each new crate ships its own test helpers as a `#[cfg(test)]` module or a `test-support` sibling, not a shared `sipag-test-utils` crate (would re-create the coupling we're trying to break).
- **Extraction archaeology.** Easy-to-extract leaf modules (`sipag-mesh`, `sipag-auth`) won't move the needle on the actual mess (`htmx.rs`). Mitigation: keep `sipag-dispatch` near the front of the queue even though it's harder than `sipag-pubsub`.
- **Re-litigation.** Each extraction is a chance to relitigate every type signature inside the module. Don't. Extract first; refactor later, in the new crate's own follow-up PR. **One concern per PR.**

---

## 7. Definition of done — per extraction

A crate is "done" extracting when:

1. The new crate exists in the workspace and `cargo build --workspace` passes.
2. `sipag-core/src/lib.rs` re-exports the new crate at the old path (`pub use new_crate as old_name`), *if* there was an old path to preserve. (Net-new crates like `sipag-dispatch` skip this — no callers were depending on the old path because there wasn't one.)
3. At least one consumer (cli, serve, or tui) has switched to importing from the new crate directly.
4. The new crate has its tests (inline `#[cfg(test)] mod tests` or a `tests/` directory — either is acceptable) with at least the same coverage the source had before extraction. The sipag-pubsub extraction kept its inline tests; sipag-dispatch added new tests for its consolidated regex/types. Don't force a `tests/` directory if inline is idiomatic for the crate's size.
5. A diwa-indexable commit message tags the extraction with `[architecture]` (and `[extraction]` when applicable) and references the modules.md §9 item it serves.
6. README / module docs reference the new crate name.

A crate is "fully migrated" (re-export can be deleted) when zero `use sipag_core::<old_name>` references remain in the workspace. The sipag-pubsub shim was already orphaned at the time it landed — every consumer migrated to direct imports in the same PR. The katulong-client shim has been in place since #535 and is the canonical example of "keep one cycle, then drop."

---

## 8. Edit log

- **2026-05-18** — initial draft. Written after recognizing that `htmx.rs` (2169 LOC) holds most of sipag's accidental complexity, while `sipag-core` is already module-shaped enough to extract cleanly. Frames the work as completing the pattern PR #535 (`katulong-client`) started, not as a v2 rewrite.
- **2026-05-18** — **`sipag-pubsub` extracted** (PR #545). First extraction landed; the template-prover. `git mv sipag-core/src/pubsub.rs sipag-pubsub/src/lib.rs` preserved history. `sipag-core` re-exports at the old `sipag_core::pubsub::…` path; the `sipag` binary's five import sites (`serve/mod.rs`, `serve/state.rs`, `serve/ws.rs`, `serve/workers/expand.rs`, `serve/workers/mod.rs`) migrated to direct `sipag_pubsub::…` imports — satisfies §7 #3 ("at least one consumer migrated"). All 9 original tests came along intact. `make dev` clean. The §3 recipe held — no surprises beyond needing `mkdir -p` before `cargo new` would have worked.
- **2026-05-18** — **katulong-client async HTTP client + body caps** (PR #546) closed sipag #527. Not an extraction (extends the already-extracted `katulong-client`), but landed in the same series and is the prerequisite for the dispatch extraction below — without an async HTTP client in `katulong-client`, `sipag-dispatch` would have needed a `spawn_blocking` shim around the sync curl path.
- **2026-05-19** — **`sipag-dispatch` extracted** (PR #547). Implements modules.md §9 Phase 2 #12. Pulled the dispatch action (~240 LOC) out of `sipag/src/serve/htmx.rs::dispatch_via_attach_client` and the overlapping logic in `sipag/src/cli.rs::run_dispatch_task` into a single async `dispatch(remote, &session, input, on_step) -> Result<(), DispatchError>` function. `htmx.rs` shrank by ~440 LOC. Two real behavior unifications shipped: web UI v2 now does worktree setup (previously skipped), CLI now uses WS-attach orchestration (previously raw HTTP `/exec`). Loaders and prompt composition stayed outside the crate; the gate + `verify_and_heal_dispatch` stayed in `htmx.rs` per §9 #11. TUI dispatch still uses sync HTTP `/exec` — follow-up PR.
- **2026-05-19** — **`ollama-client` slot renamed to `ollama-bridge-client`** + entry rewritten. Earlier draft assumed sipag would talk to ollama directly with an `LlmClient` trait + per-model construction; that design was drafted before this session surfaced the existing [`dorky-robot/ollama-bridge`](https://github.com/Dorky-Robot/ollama-bridge) (Elixir queue+auth daemon) as the actual integration point. The bridge's wire shape is enqueue + poll, not synchronous chat. Updated §1 (external deps callout), §2 (crate list), §3 (full entry rewrite), §4 (sequencing), §5 (no sipag-llm wrapper); modules.md §9 #8 updated separately in this PR.
- **2026-05-20** — **all six remaining extractions landed in one autonomous push** (PRs #549-554). The user gave a "just keep going, don't ask between PRs, report when done" directive; the rest of the plan executed in sequence:
  - `ollama-bridge-client` (PR #549) — wire client for the bridge daemon. 29 behavioral tests covering wire contract (enqueue/poll/probes), polling contract (queue/run/done/error/timeout/dedup-cached/null-result-defensive), security (bearer auth on every method, body cap with sipag #527 invariant, token Debug-redaction, hash validation symmetric on send + receive).
  - `sipag-corpus` (PR #550) — local vector store. 17 behavioral tests covering persistence (add + reopen + malformed-line tolerance + source_refs round-trip), search (cosine ranking + top_k + tag/timestamp/generation filters + empty whitelist + dimension mismatch), embedder integration (BridgeEmbedder behind default cargo feature; FakeEmbedder for tests + error propagation).
  - `sipag-lens` (PR #551) — lens-worker primitive. 17 behavioral tests covering resolver (defaults match modules.md §10, named pass-through, models.toml override + missing-file fallback + malformed-file error), parser (clean JSON + chatty-prose extraction + no-JSON rejection + all four verbs round-trip), LensWorker (writes Observe to corpus + returns all actions, resolves Profile to concrete model on chat call, propagates backend failure, auto-adds lens-tag without duplication), wire-format round-trips for TriggerPolicy + LensSource.
  - `sipag-mesh` (PR #552) — host topology. Mechanical move of sipag-core/src/hosts.rs (117 LOC) via `git mv`. Inlined a small private `default_sipag_dir()` helper so the crate is a true leaf with no dep on sipag-core.
  - `sipag-board` (PR #553) — OKR + Task + Project + Observation domain. Mechanical move of sipag-core/src/board/ (7 files). One internal `crate::board::…` → `crate::…` fix.
  - `sipag-auth` (PR #554) — identity subsystem. Mechanical move of sipag-core/src/auth/ (9 files). Bulk `crate::auth::…` → `crate::…` fix (20+ references).
  - This docs PR closes the loop: §1 (current state) + §2 (target reached) + §3 (every entry ✅ done with PR link) + §4 (sequencing all done) updated.

**The plan is complete.** Future work — retire `sipag-core/src/{gate,llm}.rs` as the lens-worker scheduler lands (`nudge.rs` already retired in §9 #11), then dissolve `sipag-core` itself once its shims have aged out — lives in modules.md §9, not here.
