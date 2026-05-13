# Dispatch implementation plan — sipag as a katulong client

Companion to [`dispatch-design.md`](./dispatch-design.md). That doc
captures the *why*; this one captures the *how*, in enough detail
that the actual code change is mechanical. No source code in this
file — just shapes, schemas, file layouts, and ordered work items.

All facts in this doc are drawn from reading the katulong code at
HEAD `dadbbf6` (2026-05-13). Citations point to specific
files/lines so revisions to katulong can be tracked against the
contract sipag depends on.

## 0. Goals and non-goals

**Goal.** Replace sipag's current "compose tmux send-keys + curl
exec" dispatch with a long-lived katulong client connection that
speaks the same wire protocol the browser tile speaks. After
landing, sipag's dispatch path drives a katulong session the same
way a human in a browser would, with the same correctness, and
inherits future katulong client features automatically.

**In scope (this PR / sequence of PRs):**

- A new sipag-core module (`katulong::client`) that maintains an
  authenticated transport connection to katulong, sends protocol
  messages, consumes the pull-model output stream, and exposes a
  small public API to dispatch and observation callers.
- Rewriting `sipag/src/serve/htmx.rs::dispatch_task_handler` to
  use the new client.
- Deleting `wrap_bracketed_paste`, `verify_and_heal_dispatch`,
  and the nudge keystroke loop. Keeping `gate::classify` and
  `nudge::next_step` for state classification (see §9).

**Out of scope (deliberate deferrals):**

- WebRTC DataChannel handling. Sipag-to-katulong runs through a
  Cloudflare tunnel; the transport will always be WebSocket in
  practice. The katulong-side `ClientTransport` abstraction is
  agnostic, so this is a future free upgrade, not a feature gap.
- New katulong endpoints. The auth + protocol research (§2, §3)
  confirmed everything sipag needs already exists.
- Rewriting the CLI dispatch path (`sipag/src/cli.rs::run_dispatch_task`).
  The CLI uses one-shot `claude -p '<title>'` which is a different
  failure mode; defer to a follow-up.
- Replacing sipag's HTTP katulong client. `create_session`,
  `list_sessions`, `kill_session`, `session_output_lines`, and the
  URL helpers stay — they're still useful for one-shot read calls
  and for session-lifecycle operations that don't need a long
  connection.

## 1. Architecture at a glance

```
┌────────────────────────────────────────────────────────────────────┐
│ sipag-serve (mac2024)                                              │
│                                                                    │
│  dispatch_task_handler                                             │
│        │                                                           │
│        ▼                                                           │
│   gate::classify ───────────── reads ──┐                           │
│        │                               │                           │
│        ▼                               │                           │
│   client::attach(session_id) ─────────┐│                           │
│        │                              ││                           │
│   ┌────▼─────────────────────────┐    ││                           │
│   │ KatulongAttach (per session) │    ││                           │
│   │  - WS transport              │    ││                           │
│   │  - protocol encoder/decoder  │    ││                           │
│   │  - rolling output buffer     │◀───┘                            │
│   │  - cursor/seq tracking       │                                 │
│   │  - wait_for futures          │                                 │
│   │  - pull driver               │                                 │
│   └──────────────┬───────────────┘                                 │
│                  │                                                 │
└──────────────────┼─────────────────────────────────────────────────┘
                   │ WebSocket  (Authorization: Bearer <apiKey>)
                   ▼
┌────────────────────────────────────────────────────────────────────┐
│ katulong (og / mini / prime — wherever the dispatch fires)         │
│                                                                    │
│   server.js → server-upgrade.js → ws-manager.js                    │
│                                                                    │
│   ClientTransport ─── transport.send()/on(message) ───┐            │
│        │                                              │            │
│        │  ┌──── tmux send-keys -H (chunked) ──┐       │            │
│        │  ▼                                   ▼       │            │
│   sessionManager.writeInput()         RingBuffer ──── pull responses
│                                                                    │
└────────────────────────────────────────────────────────────────────┘
```

Three layers on the sipag side:

1. **Transport** — a tokio-tungstenite WebSocket connection,
   opened with bearer auth headers. The only katulong-specific
   detail at this layer is the URL path; everything else is
   standard WS.
2. **Protocol** — JSON message encode/decode for the message
   types listed in §3. Maintains the pull cursor and per-session
   state.
3. **Public API** — `attach(session_id) -> KatulongAttach`,
   plus methods on the attach handle: `input`, `paste`, `press`,
   `wait_for`, `last_n_lines`, `screenshot`, `cursor`, `close`.

## 2. Auth path

Confirmed working today, no katulong change required.

**What sipag sends on the WS upgrade request:**

```
GET /ws  HTTP/1.1
Host: katulong-og.felixflor.es
Upgrade: websocket
Connection: Upgrade
Authorization: Bearer <apiKey from ~/.katulong/remote.json>
Sec-WebSocket-Key: <random>
Sec-WebSocket-Version: 13
```

**What katulong does** (`server.js:148-194` `isAuthenticated`,
`server-upgrade.js:74-124`):

1. Calls `isAuthenticated(req)`.
2. Checks trust path: if loopback + valid `X-Katulong-Auth` →
   bypass (not applicable to us).
3. Checks bearer: `Authorization: Bearer <key>`. Looks up via
   `state.findApiKey(apiKey)` (`auth-state.js:503-512`),
   constant-time scrypt compare (`auth-tokens.js:54-61`).
4. On success: returns `{ authenticated: true, sessionToken: null,
   credentialId: null, apiKeyId: keyData.id }`.
5. `server-upgrade.js:122` destructures only `{ sessionToken,
   credentialId }` — both null for our case. `apiKeyId` is
   silently dropped (see §10 "Open improvements").
6. `wss.handleUpgrade()` → `wsManager.handleConnection(ws,
   { sessionToken: null, credentialId: null })`.
7. Sipag is now a registered WS client with a server-generated
   `clientId` (UUID, `ws-manager.js:260`).

**Re-auth window** (`ws-manager.js:308-322`): every 60 seconds the
server re-validates the credentials it was opened with. For
bearer-auth clients (both null), this is a no-op — the API key
itself is durable; nothing to re-check. The 60s timer still fires
but finds nothing to invalidate. Sipag does not need to handle
re-auth specifically; if the API key is rotated, the WS will be
closed with code 1008 and sipag reconnects with the new key.

**On the wire URL.** Sipag connects to:

```
wss://<host>.felixflor.es/ws
```

There's only one `/ws` endpoint; routing to a specific session
happens via the `attach` message after the WS is open
(§3.1, §4).

## 3. Wire protocol contract

All messages are JSON, single-line. Field types: strings unless
noted. The protocol is symmetric between WebSocket and DataChannel
(per `ClientTransport`); sipag treats the underlying transport as
opaque and only cares about the JSON shape.

### 3.1 Outbound messages (sipag → katulong)

Sipag must implement these. Reference: `ws-manager.js:329-567`.

| Type | Required fields | Optional | Server behavior | Response |
|------|----------------|----------|-----------------|----------|
| `attach` | `type`, `session`, `cols`, `rows` | — | Snapshots screen, registers client in routing table | `attached` + `seq-init` + `data-available` |
| `switch` | `type`, `session`, `cols`, `rows` | — | Re-points client at new session without snapshot | `switched` + `seq-init` + `data-available` |
| `input` | `type`, `data` | `session` (defaults to active) | Forwards data to PTY via `sessionManager.writeInput` | none |
| `resize` | `type`, `cols`, `rows` | `session` | Resizes PTY; broadcasts `resize-sync` if no explicit session | none |
| `pull` | `type`, `fromSeq` | `session` | Returns buffered output from fromSeq | `pull-response` or `pull-snapshot` or empty `pull-response` (backpressure) |
| `resync` | `type` | `session` | Forces a fresh snapshot for drift recovery | `pull-snapshot` |
| `subscribe` | `type`, `session`, `cols`, `rows` | — | Adds secondary session subscription (carousel) | `subscribed` (+ optionally `seq-init`) |
| `unsubscribe` | `type`, `session` | — | Removes subscription | `unsubscribed` |
| `set-tab-icon` | `type`, `session`, `icon` | — | Sets session icon metadata | broadcasts `tab-icon-changed` |
| `ping` | `type` | — | Application heartbeat (separate from WS-level ping) | `pong` |

**Messages sipag does NOT need to implement** (for the dispatch
use case): `subscribe`, `unsubscribe`, `set-tab-icon`,
`rtc-offer`, `rtc-ice-candidate`. We use exactly one session per
attach.

**Input encoding for the dispatch use case.** Sipag's `input`
messages just carry raw byte strings. The protocol does not have
separate "paste" or "keypress" message types — the bytes are the
escape sequence. Specifically:

- `claude\r` → `{type:"input", data:"claude\r"}` — launch.
- `\x1b[200~<body>\x1b[201~` (no trailing `\r`) →
  `{type:"input", data:"[200~<body>[201~"}` — paste
  body.
- `\r` → `{type:"input", data:"\r"}` — submit.

The temporal split between paste body and submit Enter is implicit
in sending them as separate `input` messages. Server-side, each
becomes its own `writeInput` → its own `send-keys -H` (which
itself chunks at 4096 bytes if the body is large). The submit `\r`
hits Claude's TUI as a distinct keystroke, outside the bracketed
paste markers — exactly the property `wrap_bracketed_paste` got
wrong.

### 3.2 Inbound messages (katulong → sipag)

Sipag must handle, route, or explicitly ignore each of these.
Reference: `ws-manager.js:124-237, 329-560`.

**Must handle:**

| Type | Fields | Meaning | Sipag action |
|------|--------|---------|--------------|
| `attached` | `session`, `data` | Initial buffer snapshot in escape-sequence form | Load into rolling buffer; xterm-headless if we ever need cursor parsing |
| `seq-init` | `session`, `seq` | Cursor starting position | Set internal cursor to `seq` |
| `switched` | `session` | Switch acknowledged (no buffer) | Update active session |
| `output` | `session`, `data`, `fromSeq`, `cursor` | Push output (zero round-trip path) | If `fromSeq == cursor`: append to buffer, advance cursor to `cursor`. Otherwise: ignore and wait for pull. |
| `data-available` | `session` | Lightweight nudge | Issue `pull` with current cursor |
| `pull-response` | `session`, `data`, `cursor` | Normal pull data | Append `data` to buffer, advance cursor to `cursor` |
| `pull-snapshot` | `session`, `data`, `cursor` | Recovery snapshot (eviction or resync) | Reset buffer to `data`, set cursor to `cursor` |
| `exit` | `session`, `code` | Session ended | Resolve any pending wait_for futures with error, mark attach as terminal |
| `state-check` | `session`, `fingerprint`, `seq` | Drift probe | Compute local fingerprint; if mismatch, send `resync` |
| `error` | `message` | Server-side error | Surface to caller; potentially terminal |
| `pong` | — | Heartbeat ack | Update lastPongAt for connection health |

**Should handle (one-line responses or state updates):**

| Type | Fields | Sipag action |
|------|--------|--------------|
| `session-removed` | `session` | If matches attach's session: terminal, resolve pending with error |
| `session-renamed` | `name`, `id` | Update cached name |
| `session-updated` | `session`, `data` (full meta) | Update cached meta — useful for `meta.claude.uuid` first-appearance |
| `resize-sync` | `cols`, `rows` | Update cached dims (we don't render but it's good hygiene) |

**Safe to ignore:**

`child-count-update`, `paste-complete`, `open-tab`, `notification`,
`topic-new`, `device-auth-request`, `credential-registered`,
`tab-icon-changed`, `subscribed`, `unsubscribed`. These are
UI-only or for use cases sipag doesn't participate in.

**Should never see (WebRTC-only):** `rtc-answer`, signaling
responses. If they arrive, ignore.

### 3.3 Pull-model invariants

These are the load-bearing properties sipag must preserve.
Reference: `ring-buffer.js`, `pull-manager.js`, `ws-manager.js:417-473`.

1. **Cursor is a byte offset.** It is monotonic and never resets
   within a session's lifetime (`ring-buffer.js:43` —
   `totalBytes` never resets). It does NOT reset on session
   restart; it does NOT reset on sipag reconnect.
2. **Cursor advances only after data is written into the local
   buffer.** Mirror the pull-manager's pattern
   (`pull-manager.js:127`): pull → on success write+advance. If
   the write fails (e.g., panic in regex matcher), the cursor
   should not advance; next pull will get the same data again.
3. **`data-available` carries no data.** It's a nudge. Sipag's
   response is to send `pull` with `fromSeq = current_cursor`.
4. **Server may return an empty `pull-response`** when sipag is
   backpressured (`bufferedAmount > WS_BACKPRESSURE_BYTES =
   1 MB`). The empty response still includes a fresh `cursor`;
   sipag advances to it and pulls again. This is the server's
   "skip ahead" mechanism — it intentionally drops bytes when
   the client can't keep up.
5. **`pull-snapshot` is a buffer reset.** When sipag receives it,
   it must clear its rolling buffer and replace with the
   snapshot's `data`, then set cursor to `cursor`. Any pending
   `wait_for` futures need to be re-evaluated against the new
   buffer content.
6. **Attach is not idempotent.** Re-sending `attach` after a
   transport drop yields a fresh `seq-init` with the *current*
   server cursor — there's no fromSeq in attach. Any output
   between the old cursor and the new one is recovered via the
   first `pull-snapshot` from `pull` (with old fromSeq → almost
   certainly evicted → snapshot path).

## 4. Connection lifecycle

```
[idle]
  │
  │ client::attach(session_id, host)
  ▼
[connecting]                          // ws_connect with bearer header
  │
  │ WS upgrade accepted
  ▼
[opened]
  │
  │ send {type:"attach", session, cols, rows}
  ▼
[awaiting-attached]
  │
  │ recv {type:"attached", data}      // initial buffer
  │ recv {type:"seq-init", seq}       // initial cursor
  │ recv {type:"data-available"}      // nudge
  ▼
[attached-active]  ◀──────────────────┐
  │                                   │
  │ -- send input ─→ no response      │
  │ -- send pull --→ recv response ───┤
  │ -- recv output ←─ apply ─────────┤
  │ -- recv data-available ─→ pull ──┤
  │ -- recv state-check ─→ resync? ──┤
  │ -- send ping ─→ recv pong ───────┤
  │                                   │
  │ ws drop / network blip            │
  ▼                                   │
[disconnected]                        │
  │                                   │
  │ reconnect (new WS, same bearer)   │
  │ send attach again                 │
  └───────────────────────────────────┘

  │ recv {type:"exit"}  ──or──  caller calls close()
  ▼
[terminal]
```

**Reconnect details.** Per the research:

- A new WebSocket is opened with the same bearer header.
- Sipag gets a new server-side `clientId` — katulong does not
  track client identity across reconnects.
- Sipag re-sends `attach` for the same session name. Katulong
  treats it as a fresh attach: snapshots the current screen,
  emits new `attached` + `seq-init`. Sipag's cursor jumps to
  the new `seq-init` value, which may be far ahead of where it
  was before the drop. Output in the gap is recovered through
  the first `pull` cycle (which most likely takes the eviction
  path → `pull-snapshot`).
- All pending `wait_for` futures stay pending. The match might
  fire against the gap content via the snapshot, or against
  fresh output that arrives after.

**Backoff.** Reconnect on WS close with exponential backoff,
capped (e.g., 250ms, 500ms, 1s, 2s, 5s, then 5s steady). On code
1008 (auth invalidated), give up — surface to caller as a
permanent error and let them re-create the attach handle with a
fresh bearer.

**Heartbeat.** Send `{type:"ping"}` every 30 seconds. If no `pong`
within 10 seconds, close the WS and reconnect. This catches
half-open connections that the underlying TCP keepalive misses.

## 5. New module: `sipag-core::katulong::client`

The current `sipag-core/src/katulong.rs` stays as the
synchronous HTTP/curl client. The new code lives at
`sipag-core/src/katulong/client.rs` (with `katulong.rs` becoming
`katulong/mod.rs` or staying flat — the rename is a small
mechanical change). The new module is async — pulls in
`tokio-tungstenite` (already in sipag's dependency tree via
axum's WS extras, see `sipag/src/serve/ws.rs`).

### 5.1 Public types

```text
pub struct KatulongAttachClient {
    // shared transport pool, http for the underlying create_session call,
    // hosts config, etc.
}

impl KatulongAttachClient {
    pub fn new(remote: RemoteConfig) -> Self;

    /// Opens a WS, sends attach, returns a handle once the initial
    /// `attached` + `seq-init` have arrived. Bearer-authed.
    pub async fn attach(
        &self,
        session_id: &str,    // or session_name; both work for {type:"attach"}
        cols: u16,
        rows: u16,
    ) -> Result<KatulongAttach>;
}

pub struct KatulongAttach {
    // - WriteHalf<WSStream> wrapped behind a Mutex (or behind a
    //   tokio::sync::mpsc channel to serialize sends)
    // - rolling output buffer (VecDeque<u8> with a soft cap, e.g.
    //   1 MB; tail-truncate on overflow)
    // - current cursor (u64)
    // - session_name (String)
    // - pending wait_for futures registry
    // - background reader task that owns the ReadHalf<WSStream>
    //   and feeds the buffer + matches + state machine
    // - heartbeat task
    // - reconnect supervisor
}

impl KatulongAttach {
    pub async fn input(&self, bytes: &[u8]) -> Result<()>;
    pub async fn paste(&self, body: &str) -> Result<()>;     // wraps in BPM markers, single input()
    pub async fn press(&self, key: KeyName) -> Result<()>;   // Enter, Escape, Tab, CtrlC, CtrlD

    /// Resolves when `re` matches against the rolling buffer.
    /// `since` controls the start point: ::FromAttach (full buffer)
    /// or ::FromNow (only output that arrives after this call).
    pub async fn wait_for(
        &self,
        re: &Regex,
        since: WaitFrom,
        timeout: Option<Duration>,
    ) -> Result<RegexMatch>;

    pub fn last_n_lines(&self, n: usize) -> Vec<String>;
    pub fn buffer_snapshot(&self) -> Bytes;

    /// On-demand katulong HTTP query — same as the existing
    /// sipag_core::katulong::session_output_lines helper, but
    /// async via reqwest instead of curl. Used sparingly.
    pub async fn screenshot(&self) -> Result<String>;

    pub async fn close(self) -> Result<()>;
}

pub enum KeyName {
    Enter,        // "\r"
    Escape,       // "\x1b"
    Tab,          // "\t"
    Backspace,    // "\x7f"
    CtrlC,        // "\x03"
    CtrlD,        // "\x04"
    Up, Down, Left, Right,  // arrow keys
    Raw(String),  // arbitrary bytes
}

pub enum WaitFrom { FromAttach, FromNow }

pub struct RegexMatch {
    pub start: usize,
    pub end: usize,
    pub captures: Vec<String>,
}
```

### 5.2 Internal state machine

The background reader task owns the WS `ReadHalf` and runs a loop:

```text
loop {
    msg = read_next_json_frame();
    match msg.type {
        "attached"      => buffer.clear(); buffer.extend(msg.data); set_session(msg.session);
        "seq-init"      => cursor = msg.seq;
        "switched"      => { /* not used in dispatch */ }
        "output"        =>
            if msg.fromSeq == cursor {
                buffer.extend(msg.data);
                cursor = msg.cursor;
                run_pending_matchers();
            } else {
                // gap — wait for pull to fill in
            }
        "data-available" => send_pull();
        "pull-response"  =>
            if msg.data not empty {
                buffer.extend(msg.data);
                cursor = msg.cursor;
                run_pending_matchers();
            } else {
                cursor = msg.cursor;
                send_pull();   // backpressure-skip; keep advancing
            }
        "pull-snapshot"  =>
            buffer.replace_with(msg.data);
            cursor = msg.cursor;
            run_pending_matchers();   // re-evaluate from new content
        "exit"           => mark terminal; resolve all matchers with Err(Exited(code));
        "state-check"    =>
            if compute_local_fingerprint(buffer) != msg.fingerprint {
                send_resync();
            }
        "error"          => surface; consider terminal depending on context
        "pong"           => update lastPongAt
        _ => /* ignored types */
    }
}
```

The `run_pending_matchers` function walks the registered wait_for
futures, re-runs each regex against the rolling buffer (limited to
the most recent N bytes for the `FromNow` case), and wakes any
that matched.

### 5.3 Rolling buffer policy

- Backing store: `VecDeque<u8>`.
- Soft cap: 1 MB. On overflow, drop the front in chunks until
  back under cap.
- `wait_for(FromAttach)` matches over the whole live buffer.
- `wait_for(FromNow)` records a `lower_bound = buffer.len()` at
  call time and matches only against `buffer[lower_bound..]`.

The buffer holds escape-sequence-laden bytes. For regex matching
we operate on a "decoded text" view — strip CSI sequences via a
small ANSI scrubber, keep the result alongside the raw buffer.
Cost is ~O(n) per chunk applied; the buffer is bounded so this is
fine. Reference: katulong's `lib/ansi-utils.js` does similar work
server-side.

### 5.4 Concurrency model

- One Tokio task owns the WS write half (`SinkExt`); inputs from
  `input()`, `pull` sends, `ping` sends, and `resync` sends all
  go through a tokio mpsc channel into this task.
- One Tokio task owns the WS read half; it deserializes frames
  and dispatches per §5.2.
- One Tokio task drives heartbeat (timer + send ping + watch for
  pong).
- One Tokio task drives reconnect supervision: detects close,
  applies backoff, reopens, replays `attach`.
- The `KatulongAttach` handle returned to callers is `Clone` (it's
  just a couple of `Arc`s) so multiple dispatch helpers can share
  the same attach if needed.

## 6. Dispatch rewrite

Sipag's `dispatch_task_handler` in `sipag/src/serve/htmx.rs` after
this lands:

```text
async fn dispatch_task_handler(project_name, id, body, state) -> Response {
    let task = Task::load(...)?;
    let host = state.hosts.find(body.host)?;
    let role = Role::load(...).ok();
    let prompt = build_dispatch_prompt(...);

    // 1. Create the session via the existing HTTP path (unchanged).
    let session = create_dispatch_session(state.http, host).await?;
    // Persist dispatch_session_id + dispatch_host_id on the task (unchanged).

    // 2. Pre-dispatch gate. Now uses an attach for the read instead of HTTP.
    let mut attach = state.client.attach(&session.id, 120, 40).await?;
    let pane = attach.last_n_lines(40).join("\n");
    let decision = gate::classify(...).await?;
    if decision.status_name != dispatchable {
        park_task_at(...);
        attach.close().await?;
        return parked_response(...);
    }

    // 3. Launch claude.
    attach.input(b"claude\r").await?;

    // 4. Wait for the TUI ready marker.
    attach.wait_for(&TUI_READY_RE, WaitFrom::FromNow, Some(15s)).await?;

    // 5. Paste prompt body (no trailing \r).
    attach.paste(&prompt).await?;

    // 6. Wait for the body to echo into the input box. This is the
    //    new piece: we know the paste landed when we see (a slice of)
    //    the body in the visible buffer.
    attach.wait_for(&prompt_echo_re(&prompt), WaitFrom::FromNow, Some(3s)).await?;

    // 7. Submit.
    attach.press(KeyName::Enter).await?;

    // 8. Wait for processing to begin. "esc to interrupt" is Claude's
    //    indicator that it's running a tool / responding.
    attach.wait_for(&CLAUDE_PROCESSING_RE, WaitFrom::FromNow, Some(10s)).await?;

    // 9. Move task to in-progress.
    move_task_to_in_progress(...);

    // 10. Hand attach off to a long-loop observer (nudge::next_step
    //     every 30-60s) for stuck-task detection. Observer holds
    //     the attach until task moves to a terminal status or N
    //     minutes pass.
    spawn_observer(attach, task_id, project_name);

    success_response()
}
```

Steps 4, 6, 8: timeouts are *individual*, not a global budget.
Each `wait_for` either matches or returns `Err(Timeout)`. The
handler catches each and either retries or parks the task with a
specific reason.

Total expected wall-clock: ~1-5 seconds for a warm session, up to
~20s if Claude is cold-launching. The 60-minute budget the old
nudge loop allowed (20 ticks × ~3min gemma) is gone — gemma is
out of the keystroke path entirely.

## 7. Inspection layer

Three sources of state, each with a clear owner:

### 7.1 Stream-derived (sipag, client-side, free)

Backed by the rolling buffer:

- `attach.last_n_lines(n) -> Vec<String>` — synchronous, just
  reads the buffer's tail.
- `attach.buffer_snapshot() -> Bytes` — full buffer copy.
- `attach.wait_for(re, since, timeout)` — async, resolves when
  the regex matches.
- `attach.has_seen(re) -> bool` — synchronous: has the pattern
  ever appeared in the buffer since attach?

All zero katulong round-trips. These cover ~90% of dispatch and
observer logic.

### 7.2 Server-rendered, already exposed (HTTP GET, on demand)

- `GET /sessions/by-id/:id/output?screen=true` — full rendered
  screen, ANSI escape sequences. Use when we need the cursor
  position by parsing the escape stream, or when the rolling
  buffer's history isn't enough (e.g., the regex hit was N
  screens ago and got truncated). Wrapped as
  `attach.screenshot() -> Result<String>`.
- `GET /sessions/by-id/:id/output?lines=N` — visible-pane plain
  text. Same as `last_n_lines` but takes the server's rendered
  state, not our locally accumulated bytes. Useful when sipag
  reconnected partway through and the buffer is sparse.
- `GET /sessions/by-id/:id/status` — `{alive,
  hasChildProcesses, agent.kind, agent.running, pane.cwd,
  pane.git, claude.uuid, ...}`. Wrapped as `attach.status()`.
- `GET /sessions/by-id/:id/summaries?limit=N` — historical
  summary log. Used by the observer for end-of-task enrichment.
- `GET /api/claude-transcript/:uuid` — only when we know the
  uuid via `meta.claude.uuid` from status.

These all use the same bearer auth as the WS, just via reqwest.
Sipag already has a reqwest client (`AppState::http`); the new
attach client can borrow it for these one-shot calls.

### 7.3 Server-rendered, not yet exposed (potential additions)

These are *not required* for the dispatch path to work, but would
sharpen the toolkit. None are blocking.

- `GET /sessions/by-id/:id/cursor` → `{ row, col, visible, shape }`.
  Source: xterm headless `buffer.active.cursorY/cursorX`. Useful
  for "is the cursor at the input prompt position" detection.
- `GET /sessions/by-id/:id/screen-mode` → `{ altScreen,
  applicationKeypad, mouseTracking }`. Source: xterm `Terminal.modes`.
- `GET /sessions/by-id/:id/pty-size` → `{ cols, rows }`. Source:
  `session._cols/_rows`.

If we end up wanting them, each is a 10-20 line addition to
`lib/routes/app-routes.js`. Park as a follow-up PR.

## 8. Wire encoding details to get right

These are the easy-to-bungle parts. Spell them out so the
implementation passes the conformance bar against the browser.

1. **JSON serialization, one frame per message.** Every WS text
   frame is a single complete JSON object. No newlines inside
   the JSON. `serde_json::to_string` (compact) is the right
   default.
2. **ESC byte as ``.** When we paste a bracketed-paste
   body, the `\x1b` bytes must be JSON-encoded as ``. This
   is automatic with serde_json (control chars are escaped) but
   easy to mess up if we hand-roll strings. Keep all message
   construction in typed structs with derive(Serialize).
3. **`\r` vs `\n`.** Claude's input handler expects `\r` (CR) as
   the submit Enter, not `\n` (LF). Verified from
   `wrap_bracketed_paste`'s existing format and from human
   browser behavior. xterm.js sends `\r` for Enter, not `\n`.
4. **No trailing `\r` in the paste body.** That's the bug we're
   fixing. The `paste()` helper sends exactly
   `\x1b[200~<body>\x1b[201~` — no trailing CR. Submit is a
   separate `press(Enter)` call.
5. **Chunking.** Sipag sends one `input` message per paste body;
   katulong server-side handles the >4096-byte chunking for
   `send-keys -H` (per `lib/session.js` SEND_KEYS_MAX_BYTES, and
   the diwa-cited commit `1901018`). Sipag does not need to
   chunk on its side.
6. **Resize on attach.** Always include sensible `cols`/`rows`
   in the attach. Recommend `cols=120, rows=40` to match the
   browser default. If the PTY ends up with weird dims, TUIs
   like Claude reflow strangely.

## 9. The role of the LLM after this lands

`gate::classify` stays as the pre-dispatch readiness check. Read
input changes: it now reads from `attach.last_n_lines(40)`
instead of HTTP `?lines=40`. Zero functional change otherwise.

`nudge::next_step` and the nudge module stay, but get moved out
of the keystroke loop. The new role is a **long-loop observer**:

- After dispatch fires successfully, the attach is handed to an
  observer task.
- Every 30-60 seconds, the observer calls `nudge::next_step`
  with the current buffer tail.
- Gemma returns `{ status, reason, human_action, keystrokes,
  done }`.
- Sipag persists status/reason/human_action on the task (board
  reflects in real time).
- `keystrokes` is now *informational only* — the observer logs
  what gemma would have sent but doesn't send it. If gemma wants
  to actually unblock something, it's marked `human_action` for
  the human.
- `done=true` ends the observer; closes the attach.
- Hard ceiling: 30 minutes per dispatch observer task.

The observer is cheap (one gemma call per minute is fine on
local hardware) and gives us "task got stuck" detection without
gemma being in the hot path.

## 10. Open improvements (not blocking)

Things katulong could do that would help, but none of which are
required for Option D to ship:

1. **Forward `apiKeyId` through to `wsManager.handleConnection`.**
   `server-upgrade.js:122` currently destructures only
   `{ sessionToken, credentialId }`. Adding `apiKeyId` would let
   the server tag the client with a useful identity ("this WS is
   from sipag"). Trivial change; aids observability.
2. **Add the structured-state endpoints in §7.3.** Cursor row/col,
   screen mode, PTY size. Each 10-20 lines.
3. **Document the wire protocol in
   `katulong/docs/remote-control.md`.** Captures what we're
   contracting on. Pulling sipag-side regressions back across the
   contract becomes obvious if katulong updates the protocol.

## 11. Migration / rollout sequencing

The actual order of operations:

| Step | What | Risk | Reversible? |
|------|------|------|-------------|
| 1 | Add `tokio-tungstenite` to sipag-core's Cargo.toml | None | Yes |
| 2 | Implement `sipag-core::katulong::client` (5.1–5.4) without touching dispatch | None — nothing uses it yet | Yes |
| 3 | Unit tests for the protocol encoder/decoder + the rolling-buffer regex matcher (no live katulong needed; use canned message fixtures) | None | Yes |
| 4 | Integration test: spin up a session via existing HTTP, attach via new client, send `input "echo hi\r"`, wait_for "hi" in output. Run against a real katulong host | Low — read-only confidence check | Yes |
| 5 | Rewrite `dispatch_task_handler` to use the new client, keeping `wrap_bracketed_paste` and the nudge loop in place behind a feature flag (e.g., `SIPAG_DISPATCH_V2=1`) | Medium — coexistence period | Yes — flip the flag off |
| 6 | Run dispatches against `agent-manager` and `katulong` projects in the new path. Verify success rate ≥ 95% | Medium | Yes |
| 7 | Delete `wrap_bracketed_paste`, `verify_and_heal_dispatch`, the nudge keystroke loop. Make `nudge.rs` the observer module per §9 | Low after step 6 succeeds | Hard (deletes code) |
| 8 | Document the protocol contract on the katulong side (`katulong/docs/remote-control.md`) | None | Yes |
| 9 | (Follow-up PR) Add structured-state endpoints if we want them | None | Yes |
| 10 | (Follow-up PR) Wire the CLI dispatch (`sipag/src/cli.rs::run_dispatch_task`) to the new client too | Low | Yes |

Steps 1-4 can happen in one PR (read-only client + tests). Steps
5-6 are a second PR (dispatch behind a flag). Step 7 is a third
PR (cleanup). Step 8 is a separate katulong PR. Steps 9-10 are
follow-ups.

## 12. Open questions remaining

These are decisions still on the table; the implementation will
need an answer to each but the answers are scoped:

- **Reconnect on `exit` vs surface terminal?** If the session
  ends (PTY exits), we get `{type:"exit", code}`. Does sipag's
  observer treat that as a clean end-of-task (move task to
  `review`?) or as a failure (move to `needs-human`?). Probably
  context-dependent: a Claude session ending after producing
  output is a success; ending in the first 10 seconds is a
  failure.
- **Cursor reset on katulong restart.** The research says seq
  doesn't reset within a session, but what about across katulong
  process restarts? `RingBuffer.totalBytes` is in-memory; on a
  fresh katulong process, the session re-spawns and seq starts
  at 0 again. Sipag's persisted `dispatch_session_id` references
  a session id that might not exist post-restart. Behavior:
  attach with the old id returns `error` ("session not found").
  Observer must catch this and re-dispatch (or park) rather
  than spinning.
- **Multi-attach to the same session.** If a human is viewing a
  session in the browser AND sipag has an observer attach on
  the same session, both clients receive output. That's fine —
  the browser does its own pull. But if sipag sends an
  `input()` while the human is typing, both inputs reach the
  PTY. Probably an acceptable behavior (the human can see what
  sipag did) but worth confirming the UX.
- **Default cols/rows for sipag attach.** Suggested 120×40 in §8.
  Worth checking what the browser sends so we don't accidentally
  reflow a TUI mid-flight when a human reconnects.

## 13. Surface area to close

This rewrite is a chance to delete claims, not just add code. Each
existing function or module is a tacit "this is a useful
abstraction" claim. When we land Option D, several claims become
either wrong, redundant, or actively misleading.

Four categories:

### 13.1 Delete with the dispatch rewrite (mechanical removals)

These have one consumer (the dispatch path) and no other reason to
exist. Going away the same PR that rewrites `dispatch_task_handler`.

| File | Symbol | Why it dies |
|------|--------|-------------|
| `sipag/src/serve/htmx.rs` | `wrap_bracketed_paste` | Was the bug — formats body+`\r` as one string. The attach client splits them naturally. |
| `sipag/src/serve/htmx.rs` | `verify_and_heal_dispatch` | The background paste/verify/heal pipeline. Replaced by the explicit attach + `wait_for` flow in §6. |
| `sipag/src/serve/htmx.rs` | `fetch_pane_scrollback` | Async HTTP pane reader. Superseded by `attach.last_n_lines()` (and `attach.screenshot()` for the rare server-render case). |
| `sipag/src/serve/htmx.rs` | `build_launch_cmd` | Builds the string `"claude\r"`. Collapses to a one-line literal at the one call site. |
| `sipag/src/serve/htmx.rs` | `dispatch_helpers_tests` (the module) | Tests `build_launch_cmd` only; the `wrap_bracketed_paste` / trust-prompt tests are gone with their subjects. |
| `sipag-core/src/nudge.rs` | `NudgeDecision::keystrokes` field + the prompt section that asks for it | Gemma is now an observer; never drives keys. Saves it from a doomed responsibility. |
| `sipag-core/src/nudge.rs` | The whole "send Enter in a separate tick" prompt-engineering paragraph in `SYSTEM_PROMPT` | Obsolete once gemma isn't typing. |

Lines of code removed: rough estimate ~350 (including tests).

### 13.2 Deprecate but keep — consumers exist that we haven't migrated yet

These have callers outside the dispatch path. We can't delete them
in the same PR, but we should mark them as legacy and not grow
new consumers. The plan is to retire them once the CLI dispatch
path migrates too (§11 step 10).

| File | Symbol | Note |
|------|--------|------|
| `sipag-core/src/katulong.rs` | `KatulongClient::exec_session` | Used only by `cli.rs::run_dispatch_task`. After CLI migration, gone. Mark with a `#[deprecated]` attribute pointing to the attach client. |
| `sipag-core/src/katulong.rs` | `agent_command` | Builds `claude -p '<title>'` for the CLI. Same migration story. The shell-quoting was always a footgun. |
| `sipag-core/src/katulong.rs` | `worktree_command` | Builds `git worktree add` string for the CLI. Once CLI uses the attach client, the worktree setup becomes a normal `input()` call too. Same migration. |

### 13.3 Reframe in docs — keep them, but document their narrow purpose

These survive because they have legitimate one-shot uses, but
their docstrings should change so they're not the default answer.

| Symbol | Old framing | New framing |
|--------|-------------|-------------|
| `KatulongClient::session_output_lines` | "Pull the last N lines of the visible pane." | "One-shot, synchronous pane read. For sustained interaction with a session, use `attach()` instead — the rolling buffer is free; this call costs a curl + an HTTP round-trip every time." |
| `KatulongClient::session_status` | "Get session status." | "Get session-level metadata (alive, hasChildProcesses, pane.cwd, claude.uuid). **Do not use `status.agent.running` for liveness** — it reports the Claude process existing, not Claude doing work. Use stream-derived signals from an `attach` instead." |
| The whole `output_lines_url`, `status_url`, `kill_url` family | "URL builders for katulong endpoints." | "URL builders for katulong's one-shot HTTP read/write endpoints. The dispatch path uses the WS client (`attach()`); these stay for diagnostic and lifecycle calls that don't need a long connection." |

### 13.4 Keep unchanged — orthogonal to dispatch

These weren't affected by the bug we were chasing and aren't
affected by the rewrite. Naming what survives is half the point of
this exercise:

- **Session lifecycle**: `create_session`, `create_dispatch_session`,
  `list_sessions`, `kill_session`, `Session::validate_id`,
  `is_valid_session_id`, `generate_dispatch_session_name`.
- **Auth/config**: `RemoteConfig`, `RemoteConfig::load`,
  `KatulongClient::from_remote_json`, `RemoteConfig::sub_url`.
- **Web-UI claude reply path**: `claude_respond_url`,
  `htmx.rs::claude_respond_handler`. Different use case (human
  typing a reply into a Claude tile via the web UI), uses
  katulong's existing `/api/claude/respond/:uuid` endpoint.
- **Transcript drill-down**: `claude_transcript_url`. UI history,
  no overlap with dispatch.
- **Worker / observation modules**: `serve/workers/{expand,research}.rs`,
  `serve/categorize.rs`, `serve/observers/*`. These talk to local
  ollama or to katulong's HTTP read endpoints — neither is in the
  dispatch hot path.
- **Task/Project/Role/Status schema**: `board::{Task, Project,
  Status, Role}` and friends. The schema work we did (dispatchable
  flag, reason/human_action fields, dispatch_session_id) all
  survives.

### 13.5 The non-obvious deletion candidate

`KatulongClient::exec_session` is the riskiest survival on the
list. As long as it exists, it's a tempting "just send these
bytes" shortcut that bypasses every correctness property the
attach client gives us. We've already lived through three rounds
of "let's just use exec for this one thing" — that's where
`wrap_bracketed_paste` came from.

Two ways to mitigate:

- **Soft option (the §13.2 plan):** `#[deprecated]` + migrate the
  one remaining caller (CLI dispatch) in a follow-up PR. Removal
  in the PR after that.
- **Hard option:** delete `exec_session` *in the same PR as the
  dispatch rewrite*, and either (a) migrate the CLI dispatch in
  the same PR, or (b) have the CLI dispatch error out with "CLI
  dispatch awaiting Option D migration; use the web UI" until the
  follow-up.

The hard option closes the temptation faster but bundles two
behavior changes. Lean soft, but worth surfacing.

### 13.6 What we are NOT closing, on purpose

For clarity — these are things one might think the rewrite
deprecates, but it doesn't:

- **The whole `sipag-core/src/katulong.rs` module.** It splits;
  the lifecycle / one-shot HTTP code stays here and the new
  attach client lives in a sibling file under `katulong/`.
- **The HTTP API surface in `sipag/src/serve/board.rs`.** That's
  the JSON API for the web UI; orthogonal.
- **The `nudge.rs` module.** Loses its `keystrokes` field and the
  prompt sections about driving keys, but the core
  classifier stays for the long-loop observer.
- **The CLI dispatch path entirely.** It's not migrated *yet*,
  but `sipag dispatch <id>` should not start failing as a side
  effect of this work.

---

## Appendix A: file inventory

Files we'll add:

- `sipag-core/src/katulong/client.rs` — the attach client
- `sipag-core/src/katulong/mod.rs` — module wiring (if we
  promote `katulong.rs` → `katulong/mod.rs`)

Files we'll modify:

- `sipag-core/src/katulong.rs` (or `mod.rs`) — re-exports
- `sipag-core/Cargo.toml` — add `tokio-tungstenite`,
  `tokio-util`, `bytes`, `regex` (probably already there)
- `sipag/src/serve/htmx.rs` — `dispatch_task_handler` rewrite;
  delete `wrap_bracketed_paste`, `verify_and_heal_dispatch`
- `sipag-core/src/nudge.rs` — module purpose comment;
  potentially split into `nudge.rs` (existing) and a new
  `observer.rs` that drives it on a long loop
- `docs/dispatch-design.md` — update §10 to point to this plan

Files in katulong we'd modify (only if we land §10's optional
improvements):

- `lib/server-upgrade.js` — forward `apiKeyId`
- `lib/routes/app-routes.js` — add `cursor`, `screen-mode`,
  `pty-size` endpoints
- `docs/remote-control.md` — new file documenting the wire
  protocol

## Appendix B: research citations

All assertions in this plan are backed by reads of the katulong
code at HEAD `dadbbf6`:

- **Auth at WS upgrade**: `server.js:148-194`,
  `server-upgrade.js:74-124`, `auth-state.js:503-512`,
  `auth-tokens.js:54-61`
- **Wire protocol**: `ws-manager.js:124-237` (broadcast types),
  `ws-manager.js:329-567` (handlers), `client-transport.js`
  (transport abstraction), `public/lib/input-sender.js:11-67`,
  `public/lib/transport-layer.js:29`
- **Pull model**: `ws-manager.js:417-473` (pull handler), `:54`
  (`WS_BACKPRESSURE_BYTES = 1 MB`), `ring-buffer.js:20-93`
  (eviction), `pull-manager.js:28-158` (client cursor mgmt)
- **HTTP read endpoints**: `lib/routes/app-routes.js:822`
  (`/sessions`), `:895-960` (`/output` modes), `:898-909`
  (`/status`), `:910-928` (`/summaries`), `:708-722`
  (`/api/claude-transcript`)
- **Session metadata**: `session.js:19` (4KB meta cap), `:112`
  (meta namespace), `app-routes.js:38-45` (publicMeta filter)
- **Headless xterm**: `lib/screen-state.js:36-46` (state),
  `:150-168` (serialize)
