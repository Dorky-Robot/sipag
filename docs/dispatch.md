# How sipag dispatch works

> *Companion to [`narrative.md`](narrative.md): that doc is the product story; this one is the mechanics — what happens between you clicking "Dispatch" in the web UI and an agent running in a katulong session. Read [`modules.md`](modules.md) §3 + §4 for the architectural context (Experimentation and Topology contexts).*

This is the **current implementation** (May 2026). Many concepts here (the gate, the legacy nudge loop) are being refactored into the lens-worker abstraction per [`modules.md`](modules.md) §9 Phase 1 #3 and Phase 2 #11.

---

## One-line summary

Dispatch turns a `Task` on the sipag board into a **katulong session running an agent with a task-shaped prompt**, fanning the work through one of two backend paths gated by the `SIPAG_DISPATCH_V2` env var.

---

## The end-to-end flow

```
┌─────────────────────────────────────────────────────────────────────┐
│  Browser (http://localhost:7100)                                    │
│  user clicks Dispatch button on a task tile                         │
└─────────────────────────┬───────────────────────────────────────────┘
                          │ POST /htmx/projects/:name/tasks/:id/dispatch
                          ▼
┌─────────────────────────────────────────────────────────────────────┐
│  dispatch_task_handler         sipag/src/serve/htmx.rs:529          │
│                                                                     │
│  1. Resolve host (from form, or first host in hosts.toml)           │
│  2. Load Task TOML  + Role TOML                                     │
│  3. Build task prompt (Objective + KR + task + diwa hint)           │
│  4. Read SIPAG_DISPATCH_V2 → use_v2: bool                           │
└─────────────────────────┬───────────────────────────────────────────┘
                          ▼
┌─────────────────────────────────────────────────────────────────────┐
│  HTTP POST /sessions   →   katulong (per host config)               │
│  creates a fresh tmux pane named "sipag-d-<hex>"                    │
│  returns { id, name }; sipag persists the id on the Task            │
└─────────────────────────┬───────────────────────────────────────────┘
                          ▼
┌─────────────────────────────────────────────────────────────────────┐
│  Dispatch Gate          sipag-core/src/gate.rs                      │
│  GET /sessions/by-id/:id/output  → pane scrollback                  │
│  POST {ollama}/api/chat → gemma4 classifies pane against statuses   │
│                                                                     │
│  ┌─────────────┐         ┌───────────────────────────────────────┐  │
│  │ Dispatch    │         │ Parked                                │  │
│  │ (status has │         │ (status has dispatchable=false        │  │
│  │  dispatchable= │      │  OR gemma errored OR couldn't parse)  │  │
│  │  true)      │         │                                       │  │
│  └──────┬──────┘         │ render board + oob_toast              │  │
│         │                │ ("task #N not dispatched — <status>") │  │
│         │                │ return immediately — handler exits    │  │
│         │                └───────────────────────────────────────┘  │
└─────────┼───────────────────────────────────────────────────────────┘
          ▼
┌─────────────────────────────────────────────────────────────────────┐
│  Task.status = "in-progress"                                        │
│  Clear stale reason/human_action notes                              │
└─────────────────────────┬───────────────────────────────────────────┘
                          ▼
              ┌───────────┴───────────┐
              │                       │
       use_v2 = true            use_v2 = false
              │                       │
              ▼                       ▼
   ┌──────────────────────┐  ┌─────────────────────────────────────┐
   │ V2 path              │  │ Legacy path                         │
   │ (rust client + WS    │  │ (HTTP exec + gemma4 nudge loop)     │
   │  attach, PR #532-#535│  │ scheduled for deletion              │
   │  Phase 2 #11)        │  │ — see modules.md §9 Phase 2 #11     │
   └──────────────────────┘  └─────────────────────────────────────┘
```

---

## Setup phase: `dispatch_task_handler`

`sipag/src/serve/htmx.rs:529`

```
┌──────────────────────────────────────────────────────────┐
│  Inputs                                                  │
│  ─ AxumPath((project_name, id))                          │
│  ─ State(state): AppState  (hosts, http client, etc.)    │
│  ─ Form(body):  DispatchForm { host: Option<String> }    │
└────────────────────┬─────────────────────────────────────┘
                     ▼
       ┌─────────────────────────────────┐
       │  Resolve target host            │
       │  body.host → state.hosts.find() │
       │  fallback: hosts.first()        │
       │  on miss → 400 / 409            │
       └─────────────┬───────────────────┘
                     ▼
       ┌─────────────────────────────────┐
       │  Task::load(dir, project, id)   │
       │  reads ~/.sipag/projects/.../   │
       │      tasks/<id>.toml            │
       └─────────────┬───────────────────┘
                     ▼
       ┌──────────────────────────────────┐
       │  Role::load(dir, project, role)  │
       │  reads .../roles/<role>.toml     │
       │  → role.command                  │
       │    e.g. "claude --dangerously-   │
       │          skip-permissions"       │
       └─────────────┬────────────────────┘
                     ▼
       ┌──────────────────────────────────┐
       │  build_dispatch_prompt(...)      │
       │  composes:                       │
       │   - Objective + KR context       │
       │   - "grep with diwa first" nudge │
       │   - the task title + body        │
       └─────────────┬────────────────────┘
                     ▼
       ┌──────────────────────────────────┐
       │  use_v2 = SIPAG_DISPATCH_V2      │
       │           env var is truthy      │
       │  htmx.rs:1210                    │
       └──────────────────────────────────┘
```

**Log signature for this phase** (all you'll see at INFO level):

```
INFO dispatch: handler entry  task=N project=X host=Y
INFO dispatch: v2 flag resolved task=N use_v2=true
```

---

## Step 2: Create the katulong session (HTTP)

`sipag/src/serve/katulong_proxy.rs::create_or_find_session`

```
sipag                                    katulong
  │                                        │
  │  POST /sessions                        │
  │  Bearer <host.api_key>                 │
  │  { "name": "sipag-d-<12hex>" }         │
  │ ─────────────────────────────────────▶ │
  │                                        │ creates tmux pane,
  │                                        │ assigns an opaque id
  │                                        │
  │  ◀───────────────────────────────────  │  201 { id, name }
  │                                        │
  │   (on 409 — name already exists)       │
  │  GET /sessions ─────────────────────▶  │
  │  ◀───────────────────────────────────  │  full list
  │   recover id by matching .name         │
  │                                        │
  │   validate_id(id) — defense-in-depth   │
  │   against `../admin`, `?inject=`, etc. │
  │                                        │
```

The `Session` type returned is `{id: String, name: String}` (renamed to `TmuxSession` in PR #537).

Sipag persists `task.dispatch_session_id = id` and `task.dispatch_host_id = host.id` back to the TOML so the board can render "running on <host>" badges by stable id (the auto-summarizer renames sessions over time; id is immutable).

**Failure here** → returns `BAD_GATEWAY` with sanitized error body. The browser sees the error inline.

---

## Step 3: The dispatch gate (where dispatch often parks)

`sipag-core/src/gate.rs`, called from `htmx.rs:1877::run_dispatch_gate`

```
                                  ┌─────────────────────────────────┐
                                  │ Project's project.toml          │
                                  │  [[statuses]]                   │
                                  │  name=todo                      │
                                  │  description="logged in, idle.."│
                                  │  dispatchable=true              │
                                  │  ... 5 other statuses ...       │
                                  └────────────┬────────────────────┘
                                               │
sipag                                          │              katulong         ollama (local)
  │                                            │                │                 │
  │ GET /sessions/by-id/:id/output?lines=200 ──┼──────────────▶ │                 │
  │ ◀──────────────────────────────────────────┼─── pane text   │                 │
  │                                            ▼                │                 │
  │   build gemma4 prompt:                                      │                 │
  │   "Here's the pane. Here are the statuses.                  │                 │
  │    Which one? Return JSON                                   │                 │
  │     {status_name, reason, human_action?}"                   │                 │
  │                                                             │                 │
  │ POST /api/chat ─────────────────────────────────────────────┼───────────────▶ │
  │                                                             │                 │
  │ ◀───────────────────────────────────────────────────────────┼──── gemma reply │
  │                                                             │                 │
  │   parse JSON, coerce shape, find the named status in        │                 │
  │   project's statuses                                        │                 │
  │                                                             │                 │
  │   ┌──────────────────────────────────────┐                  │                 │
  │   │  Status has dispatchable=true        │                  │                 │
  │   │   → return Ok(GateOutcome::Dispatch) │                  │                 │
  │   │                                      │                  │                 │
  │   │  Otherwise (incl. error / parse)     │                  │                 │
  │   │   → return Ok(GateOutcome::Parked {  │                  │                 │
  │   │       status_name, reason, ...       │                  │                 │
  │   │     })                               │                  │                 │
  │   └──────────────────────────────────────┘                  │                 │
```

### Gate outcomes in `dispatch_task_handler`

```
htmx.rs:637  match run_dispatch_gate(...).await {
htmx.rs:638    Ok(GateOutcome::Dispatch) => {
htmx.rs:639      info!("dispatch gate: dispatch — proceeding");
                 // ↓ falls through to step 4
htmx.rs:640    Ok(GateOutcome::Parked { status_name, reason }) => {
                 // render board + toast, RETURN immediately
                 // NO LOG ENTRY (silent in server logs)
htmx.rs:659    Err((st, body)) => return err_response(st, body),
               }
```

**Key fact**: the `Parked` branch returns silently from the server's perspective. The user sees a toast in the web UI ("task #N not dispatched — needs-human") but the only thing in the server log is the handler-entry line. With `RUST_LOG=info,sipag_core::gate=debug` you can see the gate's actual decision (gemma's prompt + response).

---

## Step 4: Move task to in-progress

```
htmx.rs:696  move_task(&dir, &project_name, id, "in-progress")
htmx.rs:700  info!("dispatch: moved to in-progress");
htmx.rs:705  Clear stale reason/human_action from any prior parked dispatch
```

This is the first "real" state change. Reachable only if the gate returned `Dispatch`. From here, the rest of dispatch runs as a **background task** — the HTTP request to sipag has already returned to the browser by the time the agent is being launched.

---

## Step 5: The v2 path (rust client + WS attach)

When `SIPAG_DISPATCH_V2=1`, sipag uses the [`katulong-client`](../katulong-client) crate's `KatulongAttachClient` to drive the launch through a WebSocket attach — the same wire protocol a katulong browser tile uses.

`sipag/src/serve/htmx.rs:1280+`

```
sipag                                                  katulong
  │                                                       │
  │   KatulongAttachClient::new(remote).attach(...)       │
  │                                                       │
  │   Open WS:  wss://<host>/ws                           │
  │   Authorization: Bearer <api_key>                     │
  │   Origin: <host_authority>                            │
  │  ════════════════════════════════════════════════════▶│
  │                                                       │
  │   { type:"attach", session:"sipag-d-xxxx", cols, rows}│
  │  ────────────────────────────────────────────────────▶│
  │                                                       │
  │   ◀──── { type:"attached", data:"..." }               │
  │   ◀──── { type:"seq-init", session:..., seq:N }       │
  │                                                       │
  │   (handshake complete; max 10s timeout)               │
  │                                                       │
  │ LOG: "dispatch v2: WS attach open"                    │
  │                                                       │
  │   stripped_offset = attach.stripped_offset()  ← race-safe baseline
  │                                                       │
  │   ┌─ STEP 0b: send launch keystroke ────────────────┐ │
  │   │ attach.input("claude --dangerously-skip-perm\r")│ │
  │   │  → { type:"input", data:"claude ...\r" }        │ │
  │   │ ───────────────────────────────────────────────▶│ │
  │   └─────────────────────────────────────────────────┘ │
  │                                                       │
  │ LOG: "dispatch v2: launch keystroke sent"             │
  │                                                       │
  │   ┌─ STEP 1: wait for claude TUI ready ─────────────┐ │
  │   │ attach.wait_for(tui_ready_re,                   │ │
  │   │                 FromOffset(stripped_offset),    │ │
  │   │                 timeout=30s)                    │ │
  │   │                                                 │ │
  │   │  ◀──── { type:"output", session, data:"..." } * │ │
  │   │                                                 │ │
  │   │  matches regex against rolling buffer           │ │
  │   │  (ANSI-stripped view)                           │ │
  │   └─────────────────────────────────────────────────┘ │
  │                                                       │
  │ LOG: "dispatch v2: TUI ready"                         │
  │                                                       │
  │   ┌─ STEP 2: paste the prompt body ─────────────────┐ │
  │   │ attach.input(prompt)                            │ │
  │   │  → { type:"input", data:"## Context\n..." }     │ │
  │   │ ───────────────────────────────────────────────▶│ │
  │   └─────────────────────────────────────────────────┘ │
  │                                                       │
  │   ┌─ STEP 3a: best-effort wait for echo (3s) ──────┐  │
  │   │ wait_for(paste_echo_regex(task_title),         │  │
  │   │          FromNow, 3s) — failure logged, OK     │  │
  │   └────────────────────────────────────────────────┘  │
  │                                                       │
  │ LOG: "dispatch v2: paste sent"                        │
  │                                                       │
  │   ┌─ STEP 3b: submit ──────────────────────────────┐  │
  │   │ attach.press(KeyName::Enter)                   │  │
  │   │  → { type:"input", data:"\r" }                 │  │
  │   │ ──────────────────────────────────────────────▶│  │
  │   └────────────────────────────────────────────────┘  │
  │                                                       │
  │ LOG: "dispatch v2: submit sent"                       │
  │                                                       │
  │   ┌─ STEP 4: wait for "esc to interrupt" ─────────┐   │
  │   │ Confirms Claude is actively processing.       │   │
  │   │ Last v2 milestone. attach.close() after this. │   │
  │   └───────────────────────────────────────────────┘   │
```

Key properties of the v2 path:

- **Full-duplex WS**: send and receive concurrently. Same shape as a browser tile.
- **Rolling buffer + `wait_for`** is the right primitive for "wait until Claude shows X." No polling.
- **Each keystroke is a separate frame**: launch → wait → paste → wait → submit. Failure to fire at any step shows in logs.
- **The attach handle's `Drop`** aborts the WS read/write tasks if the dispatch function bails early — no leaked sockets.

---

## Step 5 (alternate): The legacy path

When `SIPAG_DISPATCH_V2` is unset/false (the **default today**), dispatch uses HTTP `/exec` + a background `verify_and_heal_dispatch` task.

```
sipag                                              katulong                 ollama
  │                                                   │                       │
  │  POST /sessions/by-id/:id/exec                    │                       │
  │  { "input": "claude --dangerously-skip-perm\r" }  │                       │
  │  ───────────────────────────────────────────────▶ │                       │
  │  ◀──────────────────────────────── 200            │                       │
  │                                                   │                       │
  │  spawn verify_and_heal_dispatch (background)      │                       │
  │                                                   │                       │
  │  loop every ~5-30s:                               │                       │
  │   GET /sessions/by-id/:id/output ────────────────▶│                       │
  │   ◀────────────────────────────────────── pane    │                       │
  │   POST /api/chat ──────────────────────────────────────────────────────▶  │
  │   ◀────────────────────────────────────────── gemma's recovery proposal   │
  │   { "input": <recovery keystrokes> } → /exec ───▶ │                       │
  │   ...                                             │                       │
  │                                                   │                       │
  │  (exits when "esc to interrupt" or human-action)  │                       │
```

This is the path **issue [#528](https://github.com/Dorky-Robot/sipag/issues/528)** lives in (LLM-emitted bytes posted to a PTY without validation). Phase 2 #11 in `modules.md` §9 retires this path entirely once v2 has baked.

---

## Where each log line appears

```
htmx.rs:557  "dispatch: handler entry"            ← every dispatch
htmx.rs:586  "dispatch: v2 flag resolved"         ← every dispatch
htmx.rs:639  "dispatch gate: dispatch — proceeding" ← only on Dispatch
htmx.rs:700  "dispatch: moved to in-progress"     ← only after gate Dispatch
htmx.rs:1306 "dispatch v2: WS attach open"        ← only on v2
htmx.rs:1337 "dispatch v2: launch keystroke sent" ← only on v2
htmx.rs:1365 "dispatch v2: TUI ready"             ← only on v2
htmx.rs:1407 "dispatch v2: paste sent"            ← only on v2
htmx.rs:1426 "dispatch v2: submit sent"           ← only on v2

(legacy path also logs but is being retired; not shown)
```

**What "silent" failure looks like**: the dispatch handler logs `handler entry` and `v2 flag resolved`, then nothing else, and the task stays `todo`. That's the Parked outcome of the gate. To diagnose, run with `RUST_LOG=info,sipag_core::gate=debug,sipag::serve::htmx=debug`.

---

## Task state transitions

```
                              dispatch button clicked
                                       │
                                       ▼
                ┌──────────────────────────────────────┐
                │ status: todo                         │
                │  reason: None                        │
                │  human_action: None                  │
                │  dispatch_session_id: None           │
                │  dispatch_host_id: None              │
                └──────────────────┬───────────────────┘
                                   │ session created
                                   ▼
                ┌──────────────────────────────────────┐
                │ status: todo                         │
                │  dispatch_session_id: Some(<id>)     │ ← persisted before gate runs;
                │  dispatch_host_id: Some(<host>)      │   board shows "running on <host>"
                └──────────────────┬───────────────────┘
                                   │ gate decides
                  ┌────────────────┴────────────────┐
                  │                                 │
                  ▼ Dispatch                        ▼ Parked
   ┌──────────────────────────────┐  ┌────────────────────────────────┐
   │ move_task → "in-progress"    │  │ status stays "todo"            │
   │  reason: None                │  │  reason: Some(gemma's reason)  │
   │  human_action: None          │  │  human_action: Some(...) maybe │
   │  ↓ continue to v2 or legacy  │  │  (handler returns toast)       │
   └──────────────┬───────────────┘  └────────────────────────────────┘
                  │
                  ▼ v2 succeeds (claude is processing)
   ┌──────────────────────────────┐
   │ status: in-progress          │
   │  (nudge loop / SSE observer  │
   │   later moves to "review" or │
   │   "needs-human")             │
   └──────────────────────────────┘
```

---

## Failure modes (and where to look)

| Symptom | Most likely cause | Where to look |
|---|---|---|
| `dispatch` button does nothing; toast says "not dispatched — <status>" | Gate parked the task (gemma classified pane as non-dispatchable) | `RUST_LOG=...gate=debug` → see gemma's prompt + reply; possibly tune project.toml status descriptions |
| Toast says "not dispatched — needs-human", reason is blank | Gate **errored** (gemma timeout, parse failure, etc.) and fell back to needs-human | Same as above. Probably worth `RUST_LOG=...llm=debug` too. Common cause: gemma model is slow (gemma4:31b) and exceeded gate timeout. |
| Logs show `WS attach open` then `attach open failed` later | Auth, TLS, or Origin header issue against katulong WS | Check `~/.sipag/hosts.toml` apiKey for that host; verify `wss://<host>/ws` is reachable; check Origin allow-list |
| Logs show `launch keystroke sent` but never `TUI ready` | Claude binary isn't on PATH in the katulong container, or worktree path doesn't exist | SSH to the katulong host, check `claude --version` in the relevant `/work/<project>` |
| `paste sent` logged but `submit sent` never appears | `attach.press(Enter)` errored — the WS attach went terminal mid-flow | Look for `attach is closed` or `session ended with exit code N` in the rust client's `AttachError` chain |
| Multiple `sipag-d-<hex>` sessions accumulating with no agent | Orphans from failed dispatches | DELETE them via `/sessions/by-id/:id` on the host directly |

---

## What dispatch is becoming

Per [`modules.md`](modules.md) §9 phase queue:

- **Phase 1 #3** — the gate retires into the broader lens-worker abstraction. "Dispatch readiness" becomes a lens text just like any other Steering entry; the bridge worker (first lens-worker) replaces the polling shape entirely.
- **Phase 2 #11** — the legacy verify_and_heal_dispatch path is deleted (closes issue #528 by deletion, not patching).
- **Phase 2 #6** — sipag's async HTTP path to katulong moves to the `katulong-client` async client; the wire-side body-cap closes issue #527.

Until those land, the flow above is what's running.

---

## Files involved

| File | Role |
|---|---|
| `sipag/src/serve/htmx.rs:529` | `dispatch_task_handler` — HTTP entry point |
| `sipag/src/serve/htmx.rs:1280+` | v2 path (background task) |
| `sipag/src/serve/htmx.rs:1577` | `verify_and_heal_dispatch` — legacy nudge loop |
| `sipag/src/serve/htmx.rs:1877` | `run_dispatch_gate` — wraps `gate::classify` |
| `sipag-core/src/gate.rs` | Gate classifier (gemma4 via ollama) |
| `sipag-core/src/llm.rs` | Ollama HTTP client; `OLLAMA_HOST` + `OLLAMA_MODEL` env vars |
| `sipag-core/src/board/{task,role,project}.rs` | The data model the handler reads/writes |
| `sipag/src/serve/katulong_proxy.rs::create_or_find_session` | HTTP session-create with 409 fallback |
| `katulong-client/src/attach.rs` | `KatulongAttachClient` — v2's WS attach client |
| `katulong-client/src/http.rs` | `KatulongClient` — sync HTTP REST surface |
| `katulong-client/src/protocol.rs` | `Inbound`/`Outbound` WS protocol types |
