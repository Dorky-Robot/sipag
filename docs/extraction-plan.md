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
├── katulong-client/   ✅ extracted (PR #535)
├── sipag-core/
│   └── src/
│       ├── auth/           ← extract candidate (Identity context)
│       ├── board/          ← extract candidate (Steering domain)
│       ├── config.rs       ← stays (cross-cutting; tiny)
│       ├── feature.rs      ⛔ deprecated — delete after Phase 2 closes #527
│       ├── refine.rs       ⛔ deprecated — same
│       ├── gate.rs         ⛕ retiring (Phase 2 #9 + #11)
│       ├── nudge.rs        ⛕ retiring (Phase 2 #9 + #11)
│       ├── hosts.rs        ← extract candidate (Topology config)
│       ├── llm.rs          ← extract candidate (Topology wire — becomes ollama-client per Phase 2 #8)
│       ├── pubsub.rs       ← extract candidate (Topology wire)
│       └── (re-export of katulong-client)
├── sipag/             (binary: CLI + serve/)
│   └── src/serve/htmx.rs   2169 LOC — accidental complexity lives here
└── tui/               (binary: ratatui board)
```

Three crates and a binary. One bloated handler file. Multiple deprecated modules preserved for the diwa trail.

## 2. Target

```
sipag/
├── katulong-client/      ✅ wire to katulong (HTTP + WS attach + SSE soon)
├── ollama-client/        🆕 wire to ollama; exports LlmClient trait (Phase 2 #8)
├── sipag-board/          🆕 OKR + Task + Role + Project domain + TOML
├── sipag-pubsub/         🆕 file-backed durable broker
├── sipag-mesh/           🆕 hosts.toml + multi-host topology
├── sipag-auth/           🆕 webauthn / passkeys / sessions (Phase 3 #15)
├── sipag-corpus/         🆕 local vector DB for observations (Phase 1 #3)
├── sipag-lens/           🆕 Lens + LensWorker + verbs (Phase 1 #3)
├── sipag-dispatch/       🆕 the pure dispatch action (act sub-module, Phase 2 #12)
├── sipag-core/           shrinks to: config + thin re-exports + cross-cutting types
├── sipag/                shrinks to: CLI + serve/ HTTP handlers + view helpers
└── tui/                  unchanged shape; switches imports to the new crates
```

Nine crates plus binaries. Each crate has one responsibility, an internal-only API surface, and its own test suite. The binary becomes a composition layer.

---

## 3. Per-crate extraction recipe

Each row: what it owns, why it earns crate status, what's in scope for v1 of the crate, what's deferred. Sequencing rationale is in §4.

### `sipag-pubsub` — file-backed broker

- **Owns:** `Topic`, `Subscriber`, `Publisher`, JSONL append/tail, in-process fan-out.
- **Source today:** `sipag-core/src/pubsub.rs` (441 LOC), self-contained.
- **Why extract:** load-bearing per `modules.md` §10; 16+ internal publish sites already use it as a stable API. Extracting it makes that stability explicit. Lowest-risk extraction — the module has no outbound dependencies except `serde` and `tokio`.
- **In scope v1:** as-is. No behavior changes.
- **Deferred:** the §10 "should sipag's topics route through katulong's broker?" decision. That's a topology call, not an extraction call.
- **Unblocks:** nothing directly, but proves the extraction template a second time before the harder ones.

### `sipag-dispatch` — the pure dispatch action

- **Owns:** the six-line dispatch action (`create_session` → `attach` → wait for TUI → input prompt → press Enter), plus role loading, worktree setup, and prompt composition.
- **Source today:** scattered across `sipag/src/serve/htmx.rs` (`dispatch_task_handler` + the v2 path) and `sipag/src/cli.rs` (`run_dispatch_task`). Duplicated logic between the two paths.
- **Why extract:** highest-payoff extraction. Pulls the action out of htmx.rs, lets the gate + `verify_and_heal_dispatch` retire on the same series of PRs (modules.md §9 #11), deduplicates CLI vs serve, and makes the action independently testable. This is the lego block that snaps cleanly between board state and the wire client.
- **In scope v1:** `Dispatch::action(task, role, host) -> Result<DispatchHandle>` with the v2 (WS attach + `wait_for`) flow. The legacy `verify_and_heal_dispatch` path stays in htmx.rs until #11 deletes it; the new crate ships v2-only.
- **Deferred:** observation/healing belongs to `sipag-lens` (Phase 1 #3), not here.
- **Unblocks:** modules.md §9 #11 (recovery deletion), #12 (lift dispatch policy out of katulong-client), and the htmx.rs split (#13).

### `ollama-client` — Topology wire

- **Owns:** typed request/response shapes for ollama HTTP, model selection, `LlmClient` trait.
- **Source today:** `sipag-core/src/llm.rs` (300 LOC), reasonably self-contained.
- **Why extract:** modules.md §9 #8 already calls for this. The trait is the seam lens-workers and dispatch sit on. Mocking `LlmClient` in tests requires the trait to live somewhere the test crate can depend on without pulling all of sipag-core.
- **In scope v1:** `LlmClient` trait, `OllamaClient::new(host, model_name)`, typed response shapes (no more raw JSON munging at call sites).
- **Deferred:** `~/.sipag/models.toml` reader (lives in `sipag-lens` since profile→model resolution is a lens concern), prompt template library (lives with callers).
- **Unblocks:** modules.md §9 #3, #9, #10.

### `sipag-corpus` — local vector store

- **Owns:** append-only `CorpusItem` log, ollama-backed embeddings, similarity search, `corpus.search` / `corpus.expand` MCP-shape tools.
- **Source today:** **does not exist.** This is net-new (modules.md §9 #3).
- **Why crate-first:** if we build it as a `sipag-core` module the lens-worker abstraction will compile-couple to corpus internals; cleaner to draw the boundary upfront.
- **In scope v1:** persistence (JSONL + a small index file), embed-on-write via `ollama-client`, k-NN search, tag filtering, timestamp range filtering.
- **Deferred:** cross-corpus retrieval (sipag ↔ diwa per modules.md §10), corpus compaction, lens-citation graph.
- **Depends on:** `ollama-client` for embeddings.

### `sipag-lens` — lens-worker primitive

- **Owns:** `Lens`, `LensWorker`, the four verbs (`observe` + `suggest_stance` + `ask_human` + `propose_task`), trigger policy, `ModelChoice` → concrete-model resolution.
- **Source today:** **does not exist.** This is net-new (modules.md §9 #3).
- **Why crate-first:** this is the abstraction that's supposed to unify gate + nudge + future observers. If it lives inside `sipag-core` it will accidentally couple to board/auth/etc. The whole point of the abstraction is that it's substrate.
- **In scope v1:** runtime that takes a lens definition + `LlmClient` + corpus handle + trigger policy and produces corpus writes plus typed verb calls. Bridge lens-worker as the first instance.
- **Deferred:** lens governance / sprawl ranking (modules.md §10), meta-cognitive guardrails (§10), ad-hoc lens expiry (§10).
- **Depends on:** `sipag-corpus`, `ollama-client`, `sipag-board` (for `KrStance` writes).

### `sipag-board` — OKR + Task domain

- **Owns:** `Objective`, `KeyResult`, `Task`, `Role`, `Project`, `Observation`, TOML persistence under `~/.sipag/`.
- **Source today:** `sipag-core/src/board/` (well-organized — already 7 files with `mod.rs`).
- **Why extract:** large surface, well-factored, multiple consumers (tui, serve, dispatch, lens). Crate boundary forces the test pyramid to live here instead of being inherited from sipag-core.
- **In scope v1:** mechanical move. Keep all types and signatures.
- **Deferred:** the `Status` rename pass (modules.md §6 / §9 Phase 1 #2) lands separately as language-first work.
- **Depends on:** `sipag-pubsub` (it publishes change events on save).

### `sipag-mesh` — host topology

- **Owns:** `Host`, `hosts.toml` reader, multi-host resolution.
- **Source today:** `sipag-core/src/hosts.rs` (117 LOC).
- **Why extract:** small but a different concern from board state. Lets `sipag-dispatch` depend on it without dragging the OKR domain along.
- **In scope v1:** mechanical move.
- **Deferred:** the §10 `hosts.rs` vs `mesh.json` reconciliation (one source of truth or two?).

### `sipag-auth` — identity

- **Owns:** webauthn, passkeys, sessions, devices, tokens.
- **Source today:** `sipag-core/src/auth/` (multiple files, already crate-shaped internally).
- **Why extract:** modules.md §9 #15. Lowest urgency — works fine today, just big. Extract once it stops changing weekly.
- **Deferred:** could become `dorky-auth` (mesh-shared) instead of `sipag-auth` — see §10 crate-naming question.

---

## 4. Sequencing — why this order

The order matters because each extraction either (a) unblocks a downstream queue item in modules.md §9 or (b) provides a template for the next, harder extraction.

```
1. sipag-pubsub         template + lowest risk
2. sipag-dispatch       biggest htmx.rs cleanup; enables modules.md §9 #11
3. ollama-client        required by §9 #8; required by sipag-lens
4. sipag-corpus         required by sipag-lens
5. sipag-lens           the Phase 1 #3 lego block; unlocks the product capability
6. sipag-board          mechanical; safe to do anytime after the renames in §9 Phase 1 #2 land
7. sipag-mesh           mechanical; anytime
8. sipag-auth           lowest urgency; defer until auth surface stabilizes
```

**Dependency direction.** Drawn as edges (`X → Y` means X depends on Y):

```
sipag-lens       → sipag-corpus, ollama-client, sipag-board
sipag-corpus     → ollama-client
sipag-dispatch   → katulong-client, sipag-board, sipag-mesh, ollama-client (for prompt model only)
sipag-board      → sipag-pubsub
ollama-client    → (none — leaf)
sipag-pubsub     → (none — leaf)
sipag-mesh       → (none — leaf)
sipag-auth       → (none — leaf)
katulong-client  → (already extracted; leaf)
```

No cycles. Every leaf is extractable in isolation. The binary depends on all of them.

**Interleaving with modules.md §9.** Roughly:

| Extraction | Enables / pairs with §9 item |
|---|---|
| `sipag-pubsub` | none — pure infrastructure win |
| `sipag-dispatch` | #11 (delete `verify_and_heal_dispatch`), #12 (lift dispatch policy), #13 (split htmx.rs) |
| `ollama-client` | **= #8** in the queue; this IS that item, framed as extraction |
| `sipag-corpus` + `sipag-lens` | **= part of #3** in the queue (the foundation half) |
| `sipag-board` | enables clean #2 renames per-crate instead of one giant cross-cutting PR |
| `sipag-auth` | **= #15** in the queue |
| `sipag-mesh` | folds into the §10 hosts/mesh reconciliation question |

So this plan is not adding work to the queue — it's giving the queue items a structural shape so each one ships as a clean crate-sized PR instead of a sprawling cross-cutting one.

---

## 5. What we are NOT extracting

Each absence is deliberate. They're listed here because they're tempting candidates that don't earn their keep.

- **`sipag-cli`.** The CLI is a binary, not a library. It composes other crates; it isn't depended on by anything. Don't add a layer.
- **`sipag-serve`.** Same as above. The web UI is a binary surface (axum routes + maud views). It will shrink as `sipag-dispatch` + `sipag-lens` land. Splitting it from the CLI gains nothing.
- **`sipag-tui`.** Already its own binary crate (`tui/`). Stays as-is.
- **`sipag-config`.** `config.rs` is 50ish lines and resolves one env var. Keeping it in `sipag-core` is correct.
- **A `sipag-prelude` re-export crate.** No prelude. If a consumer needs three of our crates it imports three of our crates. The clarity of explicit imports beats the convenience.
- **`sipag-llm` as a separate crate from `ollama-client`.** The trait *is* the abstraction; a wrapper crate would be a fake layer. If a second backend ever exists (anthropic, openai), each gets its own client crate implementing the same trait.

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
2. `sipag-core/src/lib.rs` re-exports the new crate at the old path (`pub use new_crate as old_name`).
3. At least one consumer (cli, serve, or tui) has switched to importing from the new crate directly.
4. The new crate has its own `tests/` directory with at least the same coverage the module had inside sipag-core.
5. A diwa-indexable commit message tags the extraction with `[architecture]` and links the modules.md §9 item it serves.
6. README / module docs reference the new crate name.

A crate is "fully migrated" (re-export can be deleted) when zero `use sipag_core::<old_name>` references remain in the workspace.

---

## 8. Edit log

- **2026-05-18** — initial draft. Written after recognizing that `htmx.rs` (2169 LOC) holds most of sipag's accidental complexity, while `sipag-core` is already module-shaped enough to extract cleanly. Frames the work as completing the pattern PR #535 (`katulong-client`) started, not as a v2 rewrite.
- **2026-05-18** — **`sipag-pubsub` extracted.** First extraction landed; the template-prover. `git mv sipag-core/src/pubsub.rs sipag-pubsub/src/lib.rs` preserved history. `sipag-core` re-exports at the old `sipag_core::pubsub::…` path; the `sipag` binary's five import sites (`serve/mod.rs`, `serve/state.rs`, `serve/ws.rs`, `serve/workers/expand.rs`, `serve/workers/mod.rs`) migrated to direct `sipag_pubsub::…` imports — satisfies §7 #3 ("at least one consumer migrated"). All 9 original tests came along intact. `make dev` clean. The §3 recipe held — no surprises beyond needing `mkdir -p` before `cargo new` would have worked. **Next up:** `sipag-dispatch` per §4 ranking.
