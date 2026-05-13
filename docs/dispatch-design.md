# Dispatch design — what works, what's broken, where to go

A working doc for the sipag dispatch path. Edit freely; the goal is to
converge on one approach we both agree on before writing more code.

## 0. Glossary

- **Dispatch**: hitting "dispatch" on a task in the sipag board (or
  `sipag dispatch <id>` from the CLI) so a Claude Code agent
  somewhere starts working on it.
- **Pane**: a single tmux pane running a session, viewable in
  katulong's UI at `katulong-<host>.felixflor.es/?s=<name>`.
- **TUI**: the interactive Claude Code v2.x program that runs inside
  a pane and accepts pasted prompts + Enter to submit.
- **Bracketed paste**: the terminal escape sequence pair
  `\x1b[200~ … \x1b[201~` that tells the receiving program "the bytes
  in between are one chunk of pasted text, not typed input." Claude's
  TUI groups everything between the markers into one message; an
  internal `\n` is a literal newline in the message body, not a
  submit.

## 1. What dispatch is supposed to do

```
[User clicks dispatch in board]
   │
   ▼
[sipag-serve: dispatch_task_handler]
   │
   ├─ load task / role / project
   ├─ pick host from picker
   ├─ pre-dispatch gate (gemma4): "is this pane ready?"
   ├─ create katulong session  (currently: opaque sipag-d-<hex>)
   ├─ exec `claude\r`           ── launches Claude TUI in the pane
   ├─ wait for TUI ready
   ├─ paste the task prompt    ── multi-line markdown briefing
   ├─ press Enter to submit    ── *** this is the bug ***
   └─ observe + classify until done or human needed
```

End state we want: pane shows Claude working on the task — the
"esc to interrupt" indicator is visible, tools are firing, output
is flowing. Task on the board shows `in-progress` with the
katulong session id pinned.

## 2. The reference: how katulong does this for itself

From the diwa research, katulong already has a working version of
the paste-then-submit dance. It's exposed as
`POST /api/claude/respond/:uuid` and used by katulong's own UI
(e.g., the feed-tile reply input). Internally it calls
`replayPasteSequence`, which:

1. For each token (text or image), calls `session.write(text)`.
2. After all tokens, if `submit: true`, calls a *separate*
   `session.write("\r")` for the submit Enter.
3. `Session.write` chunks each call at `SEND_KEYS_MAX_BYTES = 4096`
   bytes and ships each chunk as its own `tmux send-keys -H` command.

The load-bearing detail: **the body and the submit Enter are
*always* separate writes**, which means they hit tmux as separate
`send-keys -H` commands, which means Claude's TUI sees the closing
`\x1b[201~` and the submit `\r` as distinct keystrokes. The body
ends, the paste closes, *then* Enter submits.

Endpoint signature:

```
POST /api/claude/respond/<uuid>
Authorization: Bearer <katulong api key>
Content-Type: application/json

{ "text": "<the full prompt body>" }
```

`<uuid>` here is **Claude's session uuid**, not katulong's session
id. Claude registers its uuid with katulong via a SessionStart hook
shortly after launch. Until that hook fires, no uuid is in
`session.meta.claude.uuid` and the endpoint returns 404 "No session
found for that Claude uuid."

## 3. What sipag has tried

| When | SHA | Approach | Outcome |
|------|-----|----------|---------|
| 2026-04-01 | `3d58d9b` | `claude -p '<title>'` — one shell-quoted command, headless mode | Permission prompts couldn't render; abandoned |
| 2026-05-07 | `9a5e68a` | Interactive TUI + `wrap_bracketed_paste(prompt)` → one exec call with body + trailing `\r` | Prompt lands in input box, Enter doesn't fire. The `\r` gets absorbed as a newline inside the paste. Hidden by `agent.running` false-positive verification |
| 2026-05-09 | `8dfbfb5` | id-keyed routes, type-safe session id | Tangential — fixed silent 404s, not the paste bug |
| 2026-05-10 | `a7246c1` | session id format validation | Tangential — security hardening, not the paste bug |
| 2026-05-12 (today, AM) | uncommitted | `gate::classify` pre-flight + `nudge::next_step` loop replacing `verify_and_heal` | Gate works; nudge loop fails because (a) gemma re-pastes instead of sending bare `\n`, (b) each gemma call is 2-6 minutes cold-load |
| 2026-05-12 (today, PM) | uncommitted | Opaque session naming (`sipag-d-<hex>`) | Works in isolation, but doesn't fix the paste/submit bug below it |

The recurring root cause across every iteration since 9a5e68a:
**sipag treats "paste body + Enter" as one exec call.** Every
attempt has been at *something else* — quoting, gating, nudging,
naming — while leaving the paste/submit join broken.

## 4. What's in the code right now (2026-05-13)

- `sipag-core/src/katulong.rs`
  - `KatulongClient::create_session(name)` + `create_dispatch_session()` (auto-named).
  - `exec_session(id, input)` — sync curl POST `/sessions/by-id/<id>/exec`.
  - `session_output_lines(id, n)` — pull last N lines plain text.

- `sipag-core/src/gate.rs` — pre-dispatch classifier. Returns
  `GateDecision { status_name, reason, human_action }`. Reads a
  pane snapshot, picks one of the project's statuses. **Working.**

- `sipag-core/src/nudge.rs` — per-tick decision: `{ status_name,
  reason, human_action, keystrokes, done }`. Built for a 20-tick /
  60s loop. **Not working in practice** — gemma is too slow and
  doesn't transition state correctly.

- `sipag/src/serve/htmx.rs::dispatch_task_handler`
  - Generates `sipag-d-<hex>` name.
  - `create_or_find_session` (always creates because name is unique).
  - Pre-dispatch gate.
  - Persists `dispatch_session_id` + `dispatch_host_id` on the task.
  - Exec `claude\r`.
  - Spawns `verify_and_heal_dispatch` async (the nudge loop).

- `sipag/src/serve/htmx.rs::verify_and_heal_dispatch` — 20-iteration
  loop that calls `nudge::next_step`, sends gemma's keystrokes,
  parks task on terminal decisions or budget exhaustion. **Working
  shape; failing in practice** because gemma can't handle the
  paste/submit split.

- `sipag/src/serve/board_view.rs::task_running_on` — matches
  `task.dispatch_session_id == session.id`. **Working.**

## 5. Root cause analysis

> **Note** (added after iteration 1 of this doc): the real diagnosis
> is that katulong's remote-control surface is unfinished. See §5a.
> The three observations below are still true, but they're the
> symptom-level reading. §5a is the wider framing we landed on.

The dispatch keeps stalling at the same point: the prompt body
arrives in Claude's input box, the cursor parks at the end, no
submit. Three observations point to one cause:

1. **The reference (katulong's `replayPasteSequence`) always
   separates body and submit Enter into different `Session.write()`
   calls.** That's the only reliable way the submit lands as a
   distinct keystroke.

2. **`wrap_bracketed_paste` formats body + `\r` as one string in
   one exec call.** Whatever katulong's exec endpoint does inside,
   the `\r` arrives as the last byte of the paste body. Claude's
   TUI never sees an Enter keystroke outside the paste markers.

3. **Gemma can't compensate for #2.** Every nudge iteration with the
   stuck pane just decides to paste again. The model is being asked
   to be a state machine for a mechanical handshake — that's the
   wrong abstraction.

Everything else we've added (gate, nudge, opaque naming, schema
fields) is fine. None of it fixes the join.

## 5a. The wider framing: katulong's remote-control surface is unfinished

Zooming out, every iteration in §3 was sipag trying to compensate for
katulong only exposing two ways for an external caller to drive a
session:

| Endpoint | Shape | What it's actually good for |
|----------|-------|------------------------------|
| `POST /sessions/by-id/:id/exec` | Raw bytes via `tmux send-keys -H` | "Type these literal characters." Caller owns bracketed-paste markers, chunk-size limits, settle timing, submit-Enter-as-separate-keystroke, prompt-readiness waits. |
| `POST /api/claude/respond/:uuid` | High-level: `{text, submit}` → `replayPasteSequence` | Pasting into a known-running Claude TUI. Keyed by Claude's uuid, so unusable before Claude's SessionStart hook fires or for non-Claude sessions. |

Nothing in between. Every external integration that wants to drive
a session beyond "type one line and press Enter" ends up
reimplementing pieces of `replayPasteSequence`. Sipag is the fourth
or fifth reinvention (sipag alone has three: shell-quoted `claude -p`,
`wrap_bracketed_paste`, and the nudge loop).

### The fix is leverage, not invention

Katulong already has a high-correctness path for typing-into-panes:
**the browser**. Every keystroke in the xterm tile — multi-line
pastes, special keys, submit Enters — already flows through a wire
protocol that katulong's server side knows how to route to tmux
correctly. The browser does the right thing because katulong's
server already does the right thing on its behalf.

#### The protocol is the layer, not the transport

Crucial nuance (per diwa commits `a7519f5`, `3756516`, `b668dbc`,
`53e2a40`, `2c06a8b`): the wire katulong uses between the browser
and the server is **transport-agnostic**. Bytes flow through a
`ClientTransport` (server) / `TransportLayer` (client) abstraction
that opaquely swaps between WebSocket and WebRTC DataChannel under
an "exactly one transport carries data at a time" invariant. Every
`send()` call site in `ws-manager.js` and `connection-manager.js`
goes through `transport.send(...)`, never raw `ws.send(...)` —
PR #553's regression bug was the proof that raw socket leaks break
correctness as soon as the active transport isn't WS.

So when we say "the protocol the browser uses," we mean specifically
the JSON message shapes flowing over `transport.send()`:

```jsonc
// Browser → server (input from xterm.js, batched via rAF in input-sender.js)
{ "type": "attach",  "session": "<name>", "cols": 120, "rows": 40 }
{ "type": "switch",  "session": "<name>", "cols": 120, "rows": 40 }
{ "type": "input",   "data": "<raw bytes incl. [200~…[201~ or \r>", "session": "<name>" }
{ "type": "resize",  "cols": 120, "rows": 40, "session": "<name>" }
{ "type": "pull",    "fromSeq": <n>, "session": "<name>" }

// Server → browser (handled in ws-manager.js)
{ "type": "attached",       "session": "<name>", "data": "<snapshot>" }
{ "type": "seq-init",       "session": "<name>", "seq": <n> }
{ "type": "data-available", "session": "<name>" }
{ "type": "pull-response",  "session": "<name>", "data": "<bytes>", "cursor": <n> }
{ "type": "exit",           "session": "<name>", "code": <n> }
{ "type": "error",          "message": "<...>" }
```

The browser's `input-sender.js` confirms this: every typed
character — paste body, Enter, Ctrl-C, all of it — becomes a
`{ type: "input", data: "<bytes>", session: "..." }` message
batched per requestAnimationFrame. There's no special "paste"
message type; the bytes carry the bracketed-paste markers when the
human pasted. The server's `wsMessageHandlers.input` (line 403 of
`ws-manager.js`) just forwards to `sessionManager.writeInput()`,
which lands in tmux via the chunked send-keys.

**The temporal separation that makes paste-then-submit work is
inherent to the protocol**: a paste and a follow-up Enter are
different `{type: "input"}` messages because rAF batches the
paste's bytes alone, and the human's subsequent Enter keystroke
arrives in the next animation frame. Server-side they flow as
separate `writeInput` calls, which means separate `send-keys -H`
commands to tmux — which means Claude's TUI sees the closing
`[201~` and the submit `\r` as distinct keystrokes.

#### What sipag adopts

Sipag opens **whatever transport** katulong is willing to give it
(in practice always WebSocket for server-to-server through the
Cloudflare tunnel — DataChannel P2P doesn't apply here) and speaks
the *protocol* above through that transport. The transport's
identity is irrelevant; the protocol is the contract.

Two prerequisites for sipag to participate:

1. **Auth at transport-open.** The HTTP routes already gate on
   bearer; the transport-open path needs the same. Whether that's
   a WebSocket subprotocol header, a URL token query param, or a
   first-message handshake — katulong picks the shape; sipag
   implements. (Looking at `ws-manager.js`, the WS upgrade path
   currently runs through a session-token / device-credential
   check — programmatic callers need a bearer-token equivalent.)
2. **Stable wire docs.** The message shapes above need to be
   contractual, not an implementation detail of the browser
   client. One markdown page in katulong's docs/, listing each
   message and its semantics. The seq/cursor pull model is the
   load-bearing reactive primitive; that's what we want pinned.

After that, sipag's client looks like:

```rust
let mut session = client.attach(&session_id).await?;
//   ^^^^^^^^^^^^ opens katulong's transport (WS for us, DC unused),
//                sends {type:"attach"}, accumulates output frames
//                into a rolling buffer.

session.input("claude\r").await?;                       // {type:"input", data:"claude\r"}
session.wait_for(r"esc to interrupt|> ").await?;        // resolves when
                                                         //   the rolling
                                                         //   buffer matches
session.input("[200~").await?;                    // paste start
session.input(&prompt).await?;                          // paste body
session.input("[201~").await?;                    // paste end
session.input("\r").await?;                             // submit (separate
                                                         //   call → separate
                                                         //   protocol msg →
                                                         //   separate
                                                         //   send-keys)
```

Or with the four `input()` calls fluently composed into a
`paste(text)` helper that handles the markers and a `press("Enter")`
helper that knows the keycodes — same protocol underneath, just
nicer ergonomics in the Rust client.

No HTTP polling. No `timeout_ms` on every call. No new `wait_for`
endpoint to clog katulong's request pool. The `wait_for` is
**client-side**, implemented on top of the same pull-driven output
stream the browser already consumes — it's a regex test against
the rolling buffer with a future that resolves when a chunk
completes the match.

### Reactive over timer-based

The user-facing behavior we want is "as soon as Claude is ready,
paste; as soon as the paste lands, submit; as soon as submission
takes, report success." Every "as soon as" is an event, not a
timer. Polling-with-timeout (the §5a-v1 `wait_for` HTTP endpoint)
forces a worst-case ceiling on each step's latency.

Subscribing to the existing output stream removes that ceiling:

| Step | Reactive shape | Timer-based shape (worse) |
|------|----------------|---------------------------|
| TUI ready | resolve on first `esc to interrupt` byte chunk | poll every 500ms; cap at 10s |
| Paste landed | resolve on echo of paste body in output | sleep 150ms and hope |
| Submit took | resolve on first non-prompt agent token | sleep 3s; check status |

The promise/callback shape generalizes: future callers can compose
the same primitive (`attach → send_input → wait_for_match`) into
arbitrary terminal-automation flows. It's the Playwright shape, not
the Selenium shape.

### Why this is the right boundary

- **Knowledge of the receiver belongs to the receiver.** Tmux quirks,
  bracketed-paste rules, send-keys chunking — all of it is
  information about how katulong's panes accept input. Forcing
  every caller to re-learn it is what causes the long tail of "this
  worked, then we changed X, now it doesn't."
- **The browser path is the proof.** If the same wire protocol gets
  a human's keystrokes typed correctly, it'll get sipag's typed
  correctly. We don't need a parallel "for programmatic callers"
  pathway; we need the existing pathway to also accept
  programmatic callers.
- **Reactive scales further than polling.** Polling burns budget
  even when nothing changes. The output stream emits exactly when
  something happens. Same observability, lower overhead, and
  composable into arbitrary "as soon as X, do Y" flows.

### Sipag as a browser-user emulator (the full version of the idea)

Pulled all the way through, this isn't "sipag speaks the protocol."
It's **sipag *is* a katulong client**, of the same kind the browser
tile is — just headless. Like Playwright / Puppeteer to Chrome:
same auth, same transport, same wire, same lifecycle. The only
thing missing is a human in the chair.

The architectural inversion that buys us everything:

- **Katulong doesn't get a "programmatic API."** It already has a
  client protocol; it gets a second kind of client. The class of
  "external integration" disappears as a concept — there are just
  *clients*, and they're either browsers driven by humans or
  processes driven by sipag.
- **Auth becomes a user, not a token.** Sipag holds a credential
  the way a paired device does. The credential authorizes "be a
  katulong user," not "call this specific endpoint." Permissions
  (which sessions, which hosts) are user-level, exactly as they
  are for a human.
- **Future katulong features fall in for free.** If katulong adds
  image paste, multi-pane carousel, a new session-meta field, a
  richer pull-response payload — sipag inherits it. We never have
  to update a sipag-specific surface, because there isn't one.
- **One protocol contract.** The conformance bar for "what does
  the client do" is the same conformance bar humans hit every day.
  When the browser works, sipag works. When sipag fails, you
  reproduce by opening a browser tile and trying the same flow.

What this means for the work we actually take on:

| Question | Answer under "sipag is a client" |
|----------|----------------------------------|
| Auth at transport-open | **Likely already done.** Katulong's HTTP routes already accept `Authorization: Bearer <api-key>` (the existing `~/.katulong/remote.json` shape). If the transport-open path accepts the same header, sipag has everything it needs. The diwa results suggest the WS upgrade currently keys on session-token / device-credential; the small katulong change is "also accept bearer." Worst case, sipag goes through the device-credential issuance flow once (like adding a new browser device). |
| Sipag's identity inside katulong | A registered user, distinguishable by credential. Showing up in `wsClients` like any other connection. The "running on" badge could even display "sipag" the same way it shows a paired device name today. |
| What sipag has to implement | A minimal client: transport open (with auth), `{type:"attach"}` to a session, an input encoder, a pull-driven output buffer with a regex matcher on top. Same ~150 LOC as before, but framed as "headless katulong client" not "remote-control library." |
| Does sipag track UI state (tabs, focus, viewport)? | **No.** A browser user *happens* to have UI state because they're rendering pixels. Sipag's "user" attaches to whatever sessions it cares about (the ones a dispatch is currently driving) and ignores the rest. No tabs, no focus, no viewport — just `attach` / `input` / `pull` for the sessions it owns. |
| What about features that exist purely client-side in the browser (xterm.js rendering, scroll, search)? | Skip them. Sipag only consumes the wire. Rendering is irrelevant. |

The "browser user emulator" framing also gives us a natural test
strategy: sipag's dispatch behavior should be reproducible by
opening a katulong tile in a browser and performing the same
sequence of inputs. If a flow works in the browser and fails in
sipag, it's an emulator bug. If it fails in both, it's katulong.

### Inspection is the other half — sipag needs eyes, not just hands

Playwright doesn't just `page.click` and `page.fill`. It also
`page.locator(...).innerText()`, `page.screenshot()`,
`page.waitForSelector(...)`. Without inspection, you're driving a
browser blind — you have no idea what state you're in or whether
your last input took. The same applies to sipag-as-katulong-client:
sending input without being able to query the result is exactly the
hand-rolled-bracketed-paste-with-no-feedback problem we already
have, just at a different layer.

For dispatch and observability, sipag needs the terminal-shaped
equivalents:

| Playwright | Katulong-client analog | Where it lives today |
|------------|------------------------|----------------------|
| `page.content()` | Visible-pane text (no escapes) | `GET /sessions/by-id/:id/output?lines=N` |
| `page.screenshot()` | Rendered pane with escapes / cursor / colors | `GET /sessions/by-id/:id/output?screen=true` (serialized via katulong's headless xterm) |
| `page.locator(...).innerText()` | Plain-text slice of visible buffer | Same as `output?lines=N` — sipag substrings client-side |
| `page.waitForSelector(...)` | Resolve when regex hits in the live stream | **Client-side**, on top of the pull-stream rolling buffer — no katulong work needed |
| `page.evaluate(...)` | (no analog — terminal has no scripting surface) | n/a |

Two layers, each with a clear owner:

1. **Server-rendered state (katulong owns).** The headless xterm
   per client already exists and is what produces `?screen=true`
   today. The questions sipag wants answered server-side are
   things you can only know if you've actually played the bytes
   through a terminal emulator: cursor position, whether the TUI
   is on the alt-screen vs normal screen, current viewport
   dimensions, the set of cells visible *right now* (vs.
   reconstructable from the scroll-back). These map to richer
   queries on top of the same headless: `cursor`, `mode`,
   `screen` (which exists), `meta` (which exists). Katulong
   already has the data; making it queryable is exposing what's
   already there.

2. **Stream-derived state (sipag owns, client-side).** Anything
   that's an assertion over time — "has the regex `esc to
   interrupt` appeared since I started watching?", "has any
   output arrived in the last 30 seconds?", "what does the
   last N kilobytes of output look like?" — is a question about
   the byte stream sipag is already consuming via pull-response.
   Sipag maintains the rolling buffer; pattern queries run
   locally. No katulong call needed.

The pragmatic split: **most dispatch logic is stream-derived**
(wait until "esc to interrupt" appears, detect when no output has
flowed for N seconds, watch for a `/login` banner). The handful of
moments that genuinely need server-rendered state — "what's the
cursor at *right now*", "is the TUI in alt-screen" — are query
calls on demand. Both are cheap.

What sipag's client surface ends up looking like once inspection is
first-class:

```rust
// Input side
session.input("claude\r").await?;
session.paste(&prompt).await?;
session.press("Enter").await?;

// Query side — stream-derived (cheap, client-local)
session.wait_for(r"esc to interrupt|> ").await?;
session.last_n_lines(40);                          // sync read of rolling buffer
session.has_seen(r"Run /login").await;             // bool — has the pattern fired since attach

// Query side — server-rendered (on-demand, one HTTP/WS roundtrip)
let snap = session.screenshot().await?;            // full pane bytes incl. escapes
let cur  = session.cursor().await?;                // { row, col, visible }
let meta = session.meta().await?;                  // { auto_title, summary, claude_uuid, ... }
```

The gate module from §7 reads `last_n_lines(80)` from the rolling
buffer instead of polling `/output?lines=80` over HTTP. The nudge
module (kept for long-loop "stuck task" detection) does the same.
The dispatch handler's tight loop — paste, wait for echo, submit,
wait for processing — is entirely stream-derived. The only call
that reaches into katulong-the-server for a fresh render is
`screenshot()`, which we use sparingly.

## 6. Options on the table

**Decision: Option D.** A, B, and C are kept here as design-debt
markers — alternatives we considered and rejected — so future
readers don't have to relitigate them. None of A/B/C address the
real architectural gap (§5a). They patch sipag around the missing
katulong surface; D fills the gap instead.

### Considered and rejected

- **Option A** — split `wrap_bracketed_paste` into two
  `exec_session` calls (paste body, settle 150ms, bare `\r`).
  *Why rejected*: keeps the wrong knowledge (tmux paste mechanics)
  on the sipag side; the 150ms settle is a guess that races on
  slow tunnels; doesn't help future callers.
- **Option B** — wait for Claude's SessionStart hook to register
  `meta.claude.uuid`, then `POST /api/claude/respond/:uuid`.
  *Why rejected*: Claude-specific (requires the hook installed,
  uuid lifetime issues if Claude restarts mid-session); doesn't
  generalize to non-Claude TUIs; still leaves sipag depending on
  a narrow katulong surface instead of the right one.
- **Option C** — A plus repurposing the nudge loop into a
  30-60s post-dispatch observer.
  *Why rejected*: same A problems; the observer half is fine but
  pairs naturally with D anyway (§7).

### Option D — sipag is a katulong client (chosen)

The framing is **sipag = headless katulong client**, the way
Playwright is a headless Chrome client. Not a new API. Not a
remote-control library. Just a second client kind, alongside the
browser, speaking the same protocol over the same transport with
the same auth model. Every future katulong feature falls into sipag
automatically. See §5a "Sipag as a browser-user emulator" for the
full version of the argument.

**Katulong side — possibly nothing for input; maybe one or two queries for inspection.**

For the **input** half (driving keystrokes): the best case is that
the WS upgrade already accepts `Authorization: Bearer <api-key>`
(the existing `~/.katulong/remote.json` shape that HTTP routes
use), in which case sipag has everything it needs to connect as a
programmatic client right now. Worst case: katulong's upgrade path
keys only on session-token / device-credential cookies today, and
the single change is "also accept bearer at upgrade." Either way
it's a small katulong PR, not a new API surface.

For the **inspection** half (querying state — see §5a "Inspection
is the other half"): most queries are stream-derived and sipag does
them client-side over the rolling output buffer. The on-demand
server-rendered queries already exist as HTTP routes
(`/output?screen=true`, `/output?lines=N`, `/status`, `/summaries`).
The genuinely new ones, if we end up wanting them:

- `GET /sessions/by-id/:id/cursor` — `{ row, col, visible, shape }`
  pulled from the headless xterm. Useful for "is the cursor at
  the input prompt" vs "is the cursor mid-output."
- `GET /sessions/by-id/:id/mode` — `{ alt_screen, application_keypad, mouse }`.
  Useful for "is the TUI in interactive mode" detection without
  regex-fingerprinting the byte stream.

Both are reads off state katulong already maintains internally per
client; making them queryable is exposing what's already there.
None of this is required for the dispatch path to work — they're
sharpening tools we'll want once dispatch is solid.

Optional but nice: a `katulong/docs/remote-control.md` page that
contractually documents the wire protocol (message shapes already
in `lib/ws-manager.js:329` and `public/lib/input-sender.js:25`,
plus the pull-model seq/cursor semantics from commit `9997ad7`)
and the inspection endpoints. The doc is for sipag's sake and any
future client author's — not because katulong needs to change
behavior.

**Sipag side — a new `KatulongClient::attach` returning a session handle:**

```rust
let mut session = client.attach(&session_id).await?;
//   ^^^^^^^^^^^^ opens katulong's transport (WS for our
//                server-to-server path through the tunnel),
//                sends {type:"attach"}, drives the pull-model
//                output stream into a rolling buffer.

// Outbound: low-level primitives match the protocol.
session.input("claude\r").await?;                       // {type:"input", data:"claude\r"}

// Inbound: a future over the rolling buffer.
session.wait_for(r"esc to interrupt|> ").await?;

// Sugar on top of input() for ergonomics:
session.paste(&prompt).await?;                          // wraps in BPM markers,
                                                         //   issues one input()
session.press("Enter").await?;                          // separate input() call
                                                         //   → separate
                                                         //   protocol message
                                                         //   → separate
                                                         //   send-keys; the
                                                         //   submit Enter
                                                         //   lands as a
                                                         //   distinct keystroke
```

The `wait_for` lives entirely client-side: a regex run over the
buffer with a future resolved when a `pull-response` chunk
completes the match. No `timeout_ms` server endpoint. Sipag picks
its own ceilings per-call.

The pull-model resumability (per commit `9997ad7`) means a transient
network drop doesn't lose output — sipag's client just sends
`{type:"pull", fromSeq:<last>}` on reconnect.

**Sipag's dispatch handler after Option D ships:**

- Create a session (`POST /sessions`, unchanged).
- `client.attach(session_id)` — opens the transport, primes the
  rolling buffer with the attached snapshot.
- `session.input("claude\r")`.
- `session.wait_for(r"esc to interrupt|> ")`.
- `session.paste(prompt)` then `session.press("Enter")`.
- Keep the attach open for a short observation window (the gate
  module's classify call can run against the rolling buffer just
  as well as it runs against a one-shot `output?lines=N` snapshot).
- Close the attach.

Total ~30 LOC in `dispatch_task_handler`, ~150 LOC in a new
`sipag-core/src/katulong/attach.rs` (transport client + protocol
encoder/decoder + rolling buffer + regex matcher). Drops
`wrap_bracketed_paste`, the nudge keystroke loop, and
`verify_and_heal_dispatch`.

**Pros**:

- Reuses the proven correctness path. If the browser types it
  right, sipag types it right.
- The "one transport at a time" invariant in
  `client-transport.js` means sipag inherits all the work that
  went into avoiding dual-path sequencing bugs (per commits
  `a7519f5`, `9b13326`, `b668dbc`, `3756516`).
- No timer-based polling — pull-model output is reactive over
  whichever transport is active.
- Katulong work is minuscule: auth at upgrade + protocol doc.
- Future callers (kubo, hilma, anything else) inherit the same
  surface.
- If katulong later adds richer transports (QUIC, WebTransport,
  whatever), sipag picks them up for free — it never knew about
  WebSocket in the first place.

**Cons**:

- Sipag now holds a long-lived transport connection per active
  dispatch instead of fire-and-forget HTTP. Lifecycle
  (reconnect, max concurrent attaches, back-pressure) is new
  state to manage.
- Protocol exists but is undocumented; we contract it from
  reading `ws-manager.js` + `input-sender.js`. One-time cost.
- The bearer-auth at transport-open is the one truly new
  katulong piece. Modest; same pattern as the HTTP routes.

**Effort**:

- Katulong: bearer check at upgrade + `docs/remote-control.md`.
  ~1 day.
- Sipag: ~150 LOC for the attach client (transport open +
  protocol encoder + rolling buffer + matcher), ~30 LOC for the
  dispatch rewrite, plus deletions of the legacy paste/nudge
  machinery.

**Where it lives**:

- Katulong: auth code path inside `lib/ws-manager.js`
  upgrade handling; new `docs/remote-control.md`. Protocol
  message handling is already in place (line 329 of
  `ws-manager.js`), no new server-side code.
- Sipag: new module `sipag-core/src/katulong/attach.rs` with the
  transport client; `dispatch_task_handler` rewritten; the whole
  `wrap_bracketed_paste` / `nudge.rs` / `verify_and_heal_dispatch`
  machinery removed from the dispatch hot path. `nudge.rs` may
  survive as a long-loop observer (§7).

**Open questions for D (worth pinning down before code):**

- What's the exact auth shape we want at transport-open? Bearer
  header on the upgrade request is one option (works for
  programmatic callers but some WS servers strip non-standard
  headers); a `?token=` query param is another (less clean but
  more portable); a first-message handshake (send a `{type:"auth",
  token:"..."}` immediately after open and require it before any
  other message) is a third. Which does katulong prefer?
- Are there any output message types currently used that sipag
  needs to handle vs. ignore? (e.g., `paste-complete`,
  `session-renamed`, `state-check` — categorize per the
  `ws-manager.js` switch at line 124.)
- Back-pressure: if sipag's pull is slow, katulong already has
  `WS_BACKPRESSURE_BYTES` logic that emits empty pull-responses
  to keep the client unstuck. Confirm this generalizes to a
  long-running sipag attach (vs. just a browser that's
  occasionally laggy).

## 7. The LLM's actual job

Reframing: gemma is great at *what state is this pane in* — that's
classification. It's bad at *what keystroke advances the state* —
that's a state machine. Mixing both into one nudge tick gives us
slow, inconsistent dispatch.

A cleaner split:

| Job | Owner |
|-----|-------|
| Pre-dispatch readiness check | Gemma (`gate::classify`) |
| Type and submit a prompt | Mechanical (split paste, or katulong's respond endpoint) |
| Detect "stuck on login / needs human / done" while in flight | Gemma (less time-sensitive — every 30s, not every 3s) |
| Recover from arbitrary stuck states with proposed keystrokes | Gemma (last-resort) |

The nudge module isn't wasted — it's the right tool for the
last-resort recovery slot, not the steady-state dispatch.

## 8. Lifecycle questions

These are independent of which option above we pick:

1. **Stale sessions accumulate.** Every dispatch now creates a fresh
   `sipag-d-<hex>` session. On og right now we have three from
   today's testing. Should sipag delete the *previous*
   `dispatch_session_id` of a task when the same task gets
   re-dispatched? Or leave them and let katulong's session-prune /
   the human handle cleanup?

2. **Retries.** If a dispatch errors at the paste/submit step,
   should we (a) reuse the same session and just retry the paste,
   or (b) abandon the session and create a new one? Option A is
   cheaper but inherits the broken pane state. Option B is cleaner
   but burns sessions.

3. **`agent.running` is unreliable.** We've confirmed it reports
   true on an idle Claude TUI (the process exists; not "doing
   work"). Anywhere that branches on `agent.running` should be
   reviewed — it can't tell us "task is being worked on."

## 9. Symptoms we still want explanations for

- **Blank pane** (the most recent screenshot): the pane shows
  nothing at all, not even `~ )` or `claude` text. Hypothesis:
  the browser was on `?s=agent-manager--dev` (a stale leftover
  session) while sipag dispatched to a fresh `sipag-d-<hex>`. So
  the user was looking at the wrong tile. This is consistent with
  the og session list — three new `sipag-d-*` sessions exist;
  `agent-manager--dev` still exists separately. **Not actually
  a sipag bug, but easy to confuse.** Mitigation: dispatch toast
  could include a clickable katulong URL pointing at the *new*
  session, so the user lands on the right tile.

- **`bytes=1568` four iterations in a row** in the prior nudge logs.
  Gemma kept re-pasting the same body. The nudge system prompt
  explicitly says "send Enter in a separate tick" and gemma ignored
  it. Either the prompt isn't clear enough about state, or 31b
  doesn't have the reasoning depth to recognise "the pane shows the
  body already; do something different." Unclear which.

- **Gemma latency**: 2-6 minutes per call locally. This kills any
  loop with a per-tick budget under a minute. Either we cache the
  model warm (run a periodic ping), use a smaller model, or move
  the LLM out of the hot path entirely.

## 10. Open decisions

1. **Decision: go with Option D (WS-based)**, per the latest
   iteration of §5a/§6. A and C remain documented as stop-gaps but
   are no longer the plan.
2. **Sequencing.**
   - (a) Land Option D end-to-end before declaring dispatch
     "working." Risk: a few more days of broken dispatches in the
     interim.
   - (b) Ship A/C as a stop-gap in sipag first to unblock today,
     then build D in parallel, then delete the stop-gap when D
     lands. Risk: two paths in flight; the stop-gap code lives
     longer than intended.
3. **Pre-work for D (research, not code yet) — most of this is
   already done from the diwa pass:**
   - **Transport abstraction surface (confirmed)**:
     `lib/client-transport.js` (server) +
     `public/lib/transport-layer.js` (client). API is `send`,
     `on("message")`, `close`, `transportType`,
     `upgradeToDataChannel`, `downgradeToWebSocket`. Atomic
     single-transport invariant per `a7519f5` / `9b13326` /
     `3756516`. Sipag uses whichever transport is active; in
     practice always WS for tunneled server-to-server.
   - **Outbound message shapes (confirmed)**: see
     `public/lib/input-sender.js` (line 25) for the input shape
     and `lib/ws-manager.js` (line 329) for the full handler
     switch (`attach`, `switch`, `input`, `resize`, `pull`, plus
     auth-related types). Bracketed paste rides as raw bytes
     inside `{type:"input", data:"..."}` — the temporal split
     between paste and submit Enter is implicit in the
     animation-frame batching, not encoded as separate message
     types.
   - **Output / pull semantics (confirmed)**: per
     `ws-manager.js` line 417+ and commit `9997ad7`. Pull-driven,
     seq/cursor-based, with `WS_BACKPRESSURE_BYTES` guarding the
     server's outbound buffer. Sipag mirrors the browser's pull
     manager.
   - **Open**: the exact transport-open auth handshake we want
     for programmatic callers (see "Open questions for D" in §6).
4. **Stale session lifecycle** — auto-delete previous on
   re-dispatch, or leave it? (still open, independent of D)
5. **Nudge module fate.** Under D, keep `nudge.rs` as a
   long-loop observer that runs every 30-60s on dispatched
   sessions to surface "stuck" tasks. Remove it from the
   keystroke path entirely.
6. **Dispatch toast UX** — should it link to the katulong session
   directly so the user opens the right tile?
7. **Gemma warm-pool** — under D this stops mattering (LLM out of
   hot path).

---

## Appendix A: exact log evidence from today

From `~/Library/Logs/sipag/launchd-stdout.log`:

```
07:28:04  nudge iter 1  bytes=1568  reason="Claude Code is initialized and waiting for the task prompt."
07:33:18  nudge iter 2  bytes=1568  reason="Starting the session by pasting the task prompt"
07:35:36  nudge iter 3  bytes=1568  reason="Agent is ready for the task prompt"
07:41:56  nudge iter 4  bytes=1568  reason="Pasting task prompt to agent"
```

Iteration deltas (gemma round-trip wall-clock):

- iter 1 → 2: 5m 14s
- iter 2 → 3: 2m 18s
- iter 3 → 4: 6m 20s

Same 1568 bytes every time — same bracketed paste of the same
prompt. Gemma never sent a bare `\n` to submit.

## Appendix B: file pointers for code review

- Paste/submit join: `sipag/src/serve/htmx.rs::wrap_bracketed_paste`
  (now removed in the uncommitted nudge branch) — and the
  equivalent code path in `verify_and_heal_dispatch`.
- Pane reader: `sipag-core/src/katulong.rs::session_output_lines`
  and the async `sipag/src/serve/htmx.rs::fetch_pane_scrollback`.
- Gate: `sipag-core/src/gate.rs::classify`.
- Nudge: `sipag-core/src/nudge.rs::next_step` + the loop in
  `verify_and_heal_dispatch`.
- Katulong reference: `katulong/lib/routes/app-routes.js` line 733
  (`/api/claude/respond/:uuid` handler), `katulong/lib/paste-sequence.js`
  (`replayPasteSequence`), `katulong/lib/session.js` (`SEND_KEYS_MAX_BYTES`).
