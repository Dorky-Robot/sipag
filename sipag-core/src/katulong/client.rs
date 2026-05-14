//! Long-lived katulong attach client.
//!
//! Sipag opens a WebSocket to katulong as a "headless katulong
//! client" — the same kind of client the browser tile is, with the
//! same auth, the same wire protocol, the same lifecycle — just
//! without a human rendering pixels. See
//! `docs/dispatch-design.md` §5a and
//! `docs/dispatch-implementation-plan.md` §5 for the design;
//! `protocol.rs` for the JSON message contract.
//!
//! ## Architecture
//!
//! Each `attach()` opens a fresh WebSocket with `Authorization:
//! Bearer <api-key>` (the same bearer the HTTP routes use; the WS
//! upgrade accepts it per katulong's `server.js:158-174`). Two
//! tokio tasks then run for the lifetime of the attach:
//!
//! - **reader task** — owns the WS read half; deserializes inbound
//!   protocol messages; updates the shared `AttachState` (rolling
//!   buffer, cursor, terminal flag, pending wait_for resolutions).
//! - **writer task** — owns the WS write half; pulls outbound
//!   messages off an `mpsc::Receiver` and serializes them. Both
//!   `KatulongAttach` methods and the reader task itself push to
//!   the same `mpsc::Sender` (e.g., the reader pushes `Pull` when
//!   it sees `DataAvailable`).
//!
//! Shared state is one `Arc<Mutex<AttachState>>`. The mutex is
//! held briefly — append to the buffer, run pending matchers,
//! release. No async work happens under the lock.
//!
//! ## Deferred (intentionally)
//!
//! - **Heartbeat / pong watchdog.** `Outbound::Ping` is wired into
//!   the protocol; a periodic sender is a future addition.
//! - **Reconnect supervision.** WS close marks the attach
//!   terminal. Callers can detect via `wait_for` returning
//!   `AttachError::Closed` and re-`attach()` themselves.
//! - **Drift detection.** `Inbound::StateCheck` is parsed but
//!   ignored. Resync only happens on `PullSnapshot`.
//! - **DataChannel transport.** Tunnel-fronted sipag will always
//!   use WebSocket; DC is a browser-only optimization.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use regex::Regex;
use thiserror::Error;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async_with_config, MaybeTlsStream, WebSocketStream};

use super::protocol::{Inbound, Outbound};
use super::RemoteConfig;

// ── public types ────────────────────────────────────────────────

/// Errors surfaced by the attach client. Anything that closes the
/// attach turns into one of these.
#[derive(Debug, Error)]
pub enum AttachError {
    #[error("invalid katulong URL: {0}")]
    InvalidUrl(String),
    #[error("websocket connect failed: {0}")]
    Connect(String),
    #[error("websocket protocol error: {0}")]
    Wire(String),
    /// Transport-layer failure: the WS stream errored or closed
    /// unexpectedly. Distinct from `Server`, which is a katulong
    /// application-level error message delivered over a healthy
    /// transport.
    #[error("websocket transport error: {0}")]
    Transport(String),
    /// Application-level error reported by katulong via
    /// `{type:"error", message}`.
    #[error("katulong server error: {0}")]
    Server(String),
    #[error("session ended with exit code {0}")]
    SessionExited(i32),
    #[error("session was removed by the server")]
    SessionRemoved,
    #[error("attach is closed")]
    Closed,
    #[error("operation timed out after {0:?}")]
    Timeout(Duration),
}

pub type AttachResult<T> = std::result::Result<T, AttachError>;

/// Where to start matching for a `wait_for` call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitFrom {
    /// Match against the entire live rolling buffer (including
    /// the initial snapshot from `attached`). Good when the
    /// pattern might already be visible when `wait_for` is called.
    FromAttach,
    /// Match only against bytes that arrive after this call. Good
    /// when re-dispatching: an old "esc to interrupt" string from
    /// a previous run shouldn't satisfy a new wait.
    ///
    /// Race note: the lower bound is captured at `wait_for`
    /// registration time, not at the moment the caller last sent
    /// input. If the caller needs the lower bound to predate a
    /// specific keystroke send (e.g., "wait for whatever appears
    /// after `claude\r`"), use `FromOffset` with a pre-send
    /// snapshot from `stripped_offset()`.
    FromNow,
    /// Match only against bytes at or after the given offset into
    /// the ANSI-stripped rolling buffer. Pair with
    /// `KatulongAttach::stripped_offset()` taken *before* the
    /// triggering input to close the FromNow snapshot race.
    ///
    /// If the buffer is evicted past the supplied offset between
    /// the snapshot and the `wait_for` registration, the lower
    /// bound silently clamps to the current length — same effect
    /// as `FromNow`. Under the default 1 MiB soft cap, evicting
    /// past a microsecond-old snapshot is not a realistic concern.
    FromOffset(usize),
}

/// Named keystrokes for `KatulongAttach::press`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum KeyName {
    Enter,
    Escape,
    Tab,
    Backspace,
    CtrlC,
    CtrlD,
    Up,
    Down,
    Left,
    Right,
    /// Arbitrary raw bytes — escape hatch for keys we haven't
    /// modeled (function keys, etc.).
    Raw(String),
}

impl KeyName {
    /// The literal bytes katulong should write to the PTY.
    /// Reference: xterm.js default keymap.
    pub fn bytes(&self) -> &str {
        match self {
            Self::Enter => "\r",
            Self::Escape => "\u{001b}",
            Self::Tab => "\t",
            Self::Backspace => "\u{007f}",
            Self::CtrlC => "\u{0003}",
            Self::CtrlD => "\u{0004}",
            Self::Up => "\u{001b}[A",
            Self::Down => "\u{001b}[B",
            Self::Right => "\u{001b}[C",
            Self::Left => "\u{001b}[D",
            Self::Raw(s) => s,
        }
    }
}

/// A regex match returned by `wait_for`. Offsets are byte indexes
/// into the ANSI-stripped view of the rolling buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegexMatch {
    pub start: usize,
    pub end: usize,
    pub matched_text: String,
}

// ── client factory ──────────────────────────────────────────────

/// Factory for opening attaches. Holds the bearer-auth config; one
/// `KatulongAttachClient` can produce many concurrent attaches.
#[derive(Debug, Clone)]
pub struct KatulongAttachClient {
    remote: Arc<RemoteConfig>,
}

impl KatulongAttachClient {
    pub fn new(remote: RemoteConfig) -> Self {
        Self {
            remote: Arc::new(remote),
        }
    }

    /// Open a fresh attach for `session_id` (by name or id; both
    /// flow through the same `{type:"attach", session}` message).
    /// Blocks until the initial `attached` + `seq-init` handshake
    /// arrives, or fails with `Timeout` after `HANDSHAKE_TIMEOUT`.
    pub async fn attach(
        &self,
        session_id: impl Into<String>,
        cols: u16,
        rows: u16,
    ) -> AttachResult<KatulongAttach> {
        let session = session_id.into();
        let url = ws_url(&self.remote.url);

        // Build the upgrade request with bearer auth header.
        let mut req = url
            .as_str()
            .into_client_request()
            .map_err(|e| AttachError::InvalidUrl(e.to_string()))?;
        let auth = format!("Bearer {}", self.remote.api_key);
        req.headers_mut().insert(
            "Authorization",
            auth.parse()
                .map_err(|e| AttachError::InvalidUrl(format!("invalid auth header: {e}")))?,
        );
        // Katulong's WS upgrade handler refuses requests whose
        // `Origin` host doesn't match the request `Host`. Browsers
        // set Origin automatically; tokio-tungstenite does not.
        let origin = build_origin(&self.remote.url)?;
        req.headers_mut().insert(
            "Origin",
            origin
                .parse()
                .map_err(|e| AttachError::InvalidUrl(format!("invalid origin: {e}")))?,
        );

        // Open WS with explicit message-size limits. Bounds the
        // transient allocation when katulong (or anything posing as
        // katulong via a compromised tunnel) sends an outsized
        // frame. Aligned with the rolling buffer's soft cap so we
        // don't accept frames we couldn't usefully hold anyway.
        let cfg = WebSocketConfig {
            max_message_size: Some(MAX_MESSAGE_BYTES),
            max_frame_size: Some(MAX_MESSAGE_BYTES),
            ..Default::default()
        };
        let (ws, _http_resp) = connect_async_with_config(req, Some(cfg), false)
            .await
            .map_err(|e| AttachError::Connect(e.to_string()))?;
        let (mut writer, mut reader) = ws.split();

        // Send `attach` immediately.
        let attach_msg = Outbound::Attach {
            session: session.clone(),
            cols,
            rows,
        };
        let frame = Message::text(
            serde_json::to_string(&attach_msg)
                .map_err(|e| AttachError::Wire(format!("serialize attach: {e}")))?,
        );
        writer
            .send(frame)
            .await
            .map_err(|e| AttachError::Wire(e.to_string()))?;

        // Drive the handshake: wait for both `attached` and `seq-init`.
        // The follow-up `data-available` is fine to consume here or
        // let the reader task handle it later — we exit the
        // handshake as soon as both required messages have arrived.
        let handshake_result =
            timeout(HANDSHAKE_TIMEOUT, run_handshake(&mut reader, &session)).await;
        let (initial_buffer, initial_seq) = match handshake_result {
            Ok(Ok(pair)) => pair,
            Ok(Err(e)) => return Err(e),
            Err(_) => return Err(AttachError::Timeout(HANDSHAKE_TIMEOUT)),
        };

        // Set up the shared state and spawn tasks.
        let state = Arc::new(Mutex::new(AttachState::new(initial_buffer, initial_seq)));
        let (writer_tx, writer_rx) = mpsc::channel::<Outbound>(OUTBOUND_CHANNEL_BOUND);

        let writer_handle = tokio::spawn(writer_task(writer, writer_rx, state.clone()));
        let reader_handle = tokio::spawn(reader_task(
            reader,
            state.clone(),
            writer_tx.clone(),
            session.clone(),
        ));

        Ok(KatulongAttach {
            session_name: session,
            writer_tx: Some(writer_tx),
            state,
            _writer_handle: Some(writer_handle),
            _reader_handle: Some(reader_handle),
        })
    }
}

/// Convert the configured https://... URL into a wss://... base
/// with `/ws` appended. Mirrors katulong's mounted WS endpoint.
fn ws_url(base: &str) -> String {
    let base = base.trim_end_matches('/');
    let scheme_swapped = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        // Assume wss:// if no scheme; tungstenite will reject if invalid.
        format!("wss://{base}")
    };
    format!("{scheme_swapped}/ws")
}

/// Build the `Origin` header value for the WS upgrade request, per
/// RFC 6454: scheme + host[+port], NO userinfo / path / query /
/// fragment. We preserve the authority verbatim (including an
/// explicit `:443` / `:80`) because tungstenite copies the URL's
/// literal authority into the `Host` header — going through
/// `url::Url::port()` would normalize the default port away, and
/// katulong's check is `new URL(origin).host === host` (string
/// equality), so a normalized Origin against an explicit-port Host
/// would silently 403.
fn build_origin(base: &str) -> AttachResult<String> {
    let base = base.trim_end_matches('/');
    let (scheme, rest) = base
        .strip_prefix("https://")
        .map(|r| ("https", r))
        .or_else(|| base.strip_prefix("http://").map(|r| ("http", r)))
        .ok_or_else(|| AttachError::InvalidUrl(format!("base url must be http(s): {base}")))?;
    // Drop userinfo (user:pass@host → host).
    let rest = rest.rsplit_once('@').map(|(_, h)| h).unwrap_or(rest);
    // Drop path, query, fragment.
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .filter(|a| !a.is_empty())
        .ok_or_else(|| AttachError::InvalidUrl("base url has no authority".into()))?;
    Ok(format!("{scheme}://{authority}"))
}

async fn run_handshake(
    reader: &mut WsReader,
    expected_session: &str,
) -> AttachResult<(String, u64)> {
    let mut buffer: Option<String> = None;
    let mut seq: Option<u64> = None;
    loop {
        let frame = match reader.next().await {
            Some(Ok(f)) => f,
            Some(Err(e)) => return Err(AttachError::Wire(e.to_string())),
            None => {
                return Err(AttachError::Wire(
                    "connection closed during handshake".into(),
                ))
            }
        };
        let text = match frame {
            Message::Text(t) => t.to_string(),
            Message::Close(_) => {
                return Err(AttachError::Wire("WS closed during handshake".into()));
            }
            _ => continue,
        };
        let msg: Inbound = serde_json::from_str(&text).map_err(|e| {
            AttachError::Wire(format!("handshake parse: {e}: {}", truncate_for_log(&text)))
        })?;
        match msg {
            Inbound::Attached { session, data } if session == expected_session => {
                buffer = Some(data);
            }
            Inbound::SeqInit { session, seq: s } if session == expected_session => {
                seq = Some(s);
            }
            Inbound::DataAvailable { session } if session == expected_session => {
                if let (Some(b), Some(s)) = (buffer.as_ref(), seq) {
                    return Ok((b.clone(), s));
                }
                // Otherwise keep waiting — `data-available` can arrive
                // before `attached`/`seq-init` if the server is racey.
            }
            Inbound::Error { message } => return Err(AttachError::Server(message)),
            _ => { /* ignore — not part of the handshake */ }
        }
        // Allow exiting on attached+seq even if data-available is
        // delayed; the next pull will pick up any pending output.
        if let (Some(b), Some(s)) = (buffer.as_ref(), seq) {
            return Ok((b.clone(), s));
        }
    }
}

// ── attach handle ───────────────────────────────────────────────

/// One open attach to a katulong session. Owned by exactly one
/// caller; `close()` consumes it. All other methods take `&self`
/// and share the underlying `Arc<Mutex<_>>`-backed state with the
/// reader and writer tasks.
///
/// `KatulongAttach` is intentionally not `Clone` because `close()`
/// needs to consume by value to await the writer task and abort
/// the reader. Callers that want to share access across multiple
/// driver tasks should wrap it in their own `Arc` and arrange for
/// exactly one of those owners to call `close()`.
#[derive(Debug)]
pub struct KatulongAttach {
    session_name: String,
    // `Option` so `close()` and `Drop` can both take ownership of
    // the sender without conflicting. None after either runs.
    writer_tx: Option<mpsc::Sender<Outbound>>,
    state: Arc<Mutex<AttachState>>,
    // Held so the tasks aren't dropped while the attach is alive.
    // Aborted on `close()` or `Drop`.
    _writer_handle: Option<JoinHandle<()>>,
    _reader_handle: Option<JoinHandle<()>>,
}

impl KatulongAttach {
    pub fn session_name(&self) -> &str {
        &self.session_name
    }

    /// Send raw bytes as `{type:"input", data:"..."}`. The bytes
    /// reach the PTY exactly as supplied — including escape
    /// sequences. Each call is one protocol message; if you want
    /// paste-then-submit, send the paste body via `paste()` then
    /// the Enter via `press(KeyName::Enter)` as separate calls.
    pub async fn input(&self, bytes: impl Into<String>) -> AttachResult<()> {
        let data = bytes.into();
        self.writer_tx
            .as_ref()
            .ok_or(AttachError::Closed)?
            .send(Outbound::Input {
                data,
                session: Some(self.session_name.clone()),
            })
            .await
            .map_err(|_| AttachError::Closed)
    }

    /// Send a bracketed-paste body. Does NOT include a trailing
    /// submit Enter — call `press(KeyName::Enter)` afterwards as a
    /// separate message, which is the whole point of fixing the
    /// original bug.
    pub async fn paste(&self, body: &str) -> AttachResult<()> {
        self.input(wrap_paste(body)).await
    }

    /// Send a named keystroke as its byte representation.
    pub async fn press(&self, key: KeyName) -> AttachResult<()> {
        self.input(key.bytes().to_string()).await
    }

    /// Current length of the ANSI-stripped rolling buffer. Snapshot
    /// this *before* sending an input that you want to wait on, then
    /// pass it as `WaitFrom::FromOffset(_)` to close the snapshot
    /// race in `FromNow`.
    pub async fn stripped_offset(&self) -> usize {
        let st = self.state.lock().await;
        st.stripped_view().len()
    }

    /// Inform katulong of new PTY dimensions. Sipag isn't rendering
    /// anything, but TUI apps reflow based on PTY size, so it's
    /// worth setting a reasonable default after attach.
    pub async fn resize(&self, cols: u16, rows: u16) -> AttachResult<()> {
        self.writer_tx
            .as_ref()
            .ok_or(AttachError::Closed)?
            .send(Outbound::Resize {
                cols,
                rows,
                session: Some(self.session_name.clone()),
            })
            .await
            .map_err(|_| AttachError::Closed)
    }

    /// Resolve when `pattern` matches against the ANSI-stripped
    /// rolling buffer. `since` controls the starting offset; see
    /// `WaitFrom`.
    ///
    /// If `max_wait` is `Some`, the future resolves to
    /// `AttachError::Timeout` after the duration elapses; otherwise
    /// it waits indefinitely (or until the attach goes terminal).
    pub async fn wait_for(
        &self,
        pattern: &Regex,
        since: WaitFrom,
        max_wait: Option<Duration>,
    ) -> AttachResult<RegexMatch> {
        // Fast path: maybe the pattern is already in the buffer.
        // Also registers the pending wait under the same lock so we
        // don't miss output that arrives between the immediate check
        // and the registration.
        let rx = {
            let mut st = self.state.lock().await;
            if let Some(reason) = st.terminal.as_ref() {
                return Err(reason.clone().into());
            }
            let stripped = st.stripped_view();
            let lower_bound = match since {
                WaitFrom::FromAttach => 0,
                WaitFrom::FromNow => stripped.len(),
                // Clamp to current length: if `offset` exceeds it,
                // the buffer was evicted past our snapshot — best we
                // can do is wait for new content.
                WaitFrom::FromOffset(offset) => offset.min(stripped.len()),
            };
            if let Some(m) = match_at(&stripped, pattern, lower_bound) {
                return Ok(m);
            }
            let (tx, rx) = oneshot::channel();
            st.pending_waits.push(PendingWait {
                pattern: pattern.clone(),
                lower_bound,
                waker: tx,
            });
            rx
        };

        match max_wait {
            Some(d) => match timeout(d, rx).await {
                Ok(Ok(result)) => result,
                Ok(Err(_)) => Err(AttachError::Closed),
                Err(_) => Err(AttachError::Timeout(d)),
            },
            None => rx.await.map_err(|_| AttachError::Closed)?,
        }
    }

    /// Last `n` lines of the visible buffer (ANSI-stripped, plain
    /// text). Synchronous over a quick lock — fine to call from
    /// the hot path.
    pub async fn last_n_lines(&self, n: usize) -> Vec<String> {
        let st = self.state.lock().await;
        let stripped = st.stripped_view();
        let text = String::from_utf8_lossy(&stripped);
        let mut tail: Vec<String> = text.lines().rev().take(n).map(|s| s.to_string()).collect();
        tail.reverse();
        tail
    }

    /// Raw rolling-buffer bytes (still containing escape sequences).
    /// Useful for diagnostics; most callers want `last_n_lines` or
    /// `wait_for`.
    pub async fn buffer_snapshot(&self) -> Vec<u8> {
        self.state.lock().await.raw_view()
    }

    /// Close the attach.
    ///
    /// **Invariant: the order below is load-bearing — do not
    /// reorder.** The writer channel cannot close until the reader
    /// task is aborted and joined, because the reader holds a
    /// `writer_tx.clone()` (so it can fire Pull on DataAvailable).
    /// Moving `writer_tx.take()` first to match the conventional
    /// "drop senders before joining" pattern reintroduces the
    /// deadlock — the writer task blocks forever on `rx.recv()`
    /// and `h.await` hangs. See the inline `CRITICAL ORDERING`
    /// block for the proof.
    pub async fn close(mut self) {
        // CRITICAL ORDERING: see doc-comment invariant.
        //
        // (1) Reader task holds `writer_tx.clone()`. Aborting +
        //     joining drops the reader future and releases that
        //     sender clone.
        if let Some(h) = self._reader_handle.take() {
            h.abort();
            let _ = h.await;
        }
        // (2) With the reader's clone gone, dropping our sender is
        //     the LAST sender drop — the channel closes.
        let _ = self.writer_tx.take();
        // (3) Writer task's `rx.recv()` now returns None, it sends
        //     a WS Close and exits. Awaiting completes promptly.
        if let Some(h) = self._writer_handle.take() {
            let _ = h.await;
        }
    }
}

impl Drop for KatulongAttach {
    /// Safety-net for callers that don't reach `close().await` — a
    /// panic in a dispatch task, a runtime shutdown, an explicit
    /// abort. Aborts both background tasks rather than letting them
    /// outlive the attach and leak the WS read half. `close().await`
    /// is still the preferred path: it lets the writer task drain
    /// gracefully via the dropped `writer_tx`.
    fn drop(&mut self) {
        if let Some(h) = self._writer_handle.take() {
            h.abort();
        }
        if let Some(h) = self._reader_handle.take() {
            h.abort();
        }
    }
}

// ── tunable constants ───────────────────────────────────────────

/// Soft cap on the rolling buffer. When exceeded, we drop bytes
/// from the front (oldest). Pending `wait_for` lower-bounds shift
/// accordingly. Aligned with katulong's per-client
/// `WS_BACKPRESSURE_BYTES = 1 MiB` so we never carry less history
/// than katulong was willing to buffer for us.
///
/// Not configurable yet — see `docs/dispatch-implementation-plan.md`
/// §5.3 for the design rationale.
const BUFFER_SOFT_CAP: usize = 1_048_576; // 1 MiB

/// Hard cap per inbound WebSocket message, enforced by the
/// transport itself via `WebSocketConfig::max_message_size`. Caps
/// katulong-supplied data BEFORE the rolling-buffer eviction can
/// run, so a malicious or compromised peer can't blow up sipag's
/// memory by sending a 60 MB `pull-snapshot`. Generous enough for
/// typical pane snapshots (~tens of KiB); small enough to refuse
/// pathological payloads.
const MAX_MESSAGE_BYTES: usize = 2 * 1024 * 1024; // 2 MiB

/// How long to wait for the initial `attached` + `seq-init`
/// handshake to complete. Browser tile handshakes empirically
/// finish in 50-200ms; 10s is "we should have noticed by now."
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Bound on the outbound message channel. Sized for: paste body +
/// submit + a couple of pulls + a heartbeat in flight at once. Drop
/// policy when full varies by call site (`try_send` for gap-fill
/// pulls where the next nudge will retry; awaiting `send` for
/// `DataAvailable`-driven pulls where the server won't necessarily
/// re-nudge).
const OUTBOUND_CHANNEL_BOUND: usize = 64;

/// Truncate untrusted server payloads to this many bytes before
/// they appear in error messages or tracing logs. Prevents a
/// chatty/malicious peer from filling logs or bloating
/// error-chain `Display` output.
const MAX_LOG_PAYLOAD_LEN: usize = 256;

/// Recommended terminal width for new attaches. Matches the
/// browser tile's default so TUIs reflow consistently for both
/// human and programmatic clients.
pub const DEFAULT_ATTACH_COLS: u16 = 120;

/// Recommended terminal height for new attaches. See
/// `DEFAULT_ATTACH_COLS`.
pub const DEFAULT_ATTACH_ROWS: u16 = 40;

// ── internal state ──────────────────────────────────────────────

#[derive(Debug)]
struct AttachState {
    /// Raw bytes received from `Attached.data` + each
    /// `PullResponse.data` + each `Output.data`. Includes ANSI
    /// escapes; we strip on demand for matching.
    rolling: VecDeque<u8>,
    /// Sipag's current cursor in the session's byte-offset
    /// sequence. Advanced after every successful append from
    /// `PullResponse`, `PullSnapshot`, or in-order `Output`.
    cursor: u64,
    /// `None` while the attach is healthy; `Some(reason)` after
    /// `exit`, `session-removed`, WS close, or fatal error.
    terminal: Option<TerminalReason>,
    /// `wait_for` calls that haven't matched yet.
    pending_waits: Vec<PendingWait>,
}

/// Why the attach went terminal. Tracked so pending `wait_for`
/// futures can resolve with the right `AttachError` variant.
///
/// Notably absent: a `Server` variant for `Inbound::Error`. The
/// protocol uses `error` messages for non-fatal per-request
/// rejections (e.g., a malformed `Pull`); we log and continue
/// rather than collapsing the attach. If a future need emerges to
/// promote certain server errors to terminal, add the variant and
/// the corresponding `AttachError::Server` mapping.
#[derive(Debug, Clone)]
enum TerminalReason {
    Exit(i32),
    SessionRemoved,
    /// Transport-layer failure (WS read/write error, dropped
    /// connection mid-stream).
    Transport(String),
    Closed,
}

impl From<TerminalReason> for AttachError {
    fn from(r: TerminalReason) -> Self {
        match r {
            TerminalReason::Exit(code) => AttachError::SessionExited(code),
            TerminalReason::SessionRemoved => AttachError::SessionRemoved,
            TerminalReason::Transport(m) => AttachError::Transport(m),
            TerminalReason::Closed => AttachError::Closed,
        }
    }
}

struct PendingWait {
    pattern: Regex,
    /// Match only against `stripped[lower_bound..]`. Shifts down
    /// when the rolling buffer evicts from the front.
    lower_bound: usize,
    waker: oneshot::Sender<AttachResult<RegexMatch>>,
}

impl std::fmt::Debug for PendingWait {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingWait")
            .field("pattern", &self.pattern.as_str())
            .field("lower_bound", &self.lower_bound)
            .finish_non_exhaustive()
    }
}

impl AttachState {
    fn new(initial_buffer: String, initial_seq: u64) -> Self {
        let mut s = Self {
            rolling: VecDeque::new(),
            cursor: initial_seq,
            terminal: None,
            pending_waits: Vec::new(),
        };
        // Apply the same cap as `replace_buffer` so the initial
        // handshake snapshot can't sit above `BUFFER_SOFT_CAP`
        // until the next append. Keeps the tail (most recent
        // content) when trimming.
        let bytes = initial_buffer.into_bytes();
        if bytes.len() > BUFFER_SOFT_CAP {
            let start = bytes.len() - BUFFER_SOFT_CAP;
            s.rolling.extend(&bytes[start..]);
        } else {
            s.rolling.extend(bytes);
        }
        s
    }

    /// Append new bytes; evict from the front if over the soft cap;
    /// try to resolve any pending waits.
    ///
    /// On eviction, `pending_waits[*].lower_bound` is in the
    /// ANSI-stripped coordinate system, NOT raw bytes. Shifting it
    /// by the raw byte count would over-shift whenever the evicted
    /// prefix contained ANSI escapes — a `FromNow` wait could then
    /// match against retained pre-existing content that should have
    /// been excluded. We compute the stripped-view delta of the
    /// evicted prefix and subtract that instead.
    fn append_bytes(&mut self, bytes: &[u8]) {
        self.rolling.extend(bytes.iter().copied());
        if self.rolling.len() > BUFFER_SOFT_CAP {
            let drop_n = self.rolling.len() - BUFFER_SOFT_CAP;
            // Materialize the evicted prefix so we can strip it.
            let evicted: Vec<u8> = self.rolling.iter().take(drop_n).copied().collect();
            let stripped_delta = strip_ansi_for_matching(&evicted).len();
            self.rolling.drain(..drop_n);
            for w in self.pending_waits.iter_mut() {
                w.lower_bound = w.lower_bound.saturating_sub(stripped_delta);
            }
        }
        self.try_resolve_waits();
    }

    /// Replace the entire buffer (snapshot recovery). After this,
    /// all pending wait_for `lower_bound`s become meaningless — we
    /// reset them to 0 so the next match starts from the snapshot.
    ///
    /// Caps `bytes` at `BUFFER_SOFT_CAP` to bound transient
    /// allocation when katulong delivers an oversized snapshot.
    /// When trimming is needed, we keep the TAIL (most recent
    /// content) — the head of a snapshot is typically scrollback
    /// that's of less interest to dispatch matchers.
    fn replace_buffer(&mut self, bytes: Vec<u8>) {
        self.rolling.clear();
        if bytes.len() > BUFFER_SOFT_CAP {
            let start = bytes.len() - BUFFER_SOFT_CAP;
            self.rolling.extend(&bytes[start..]);
        } else {
            self.rolling.extend(bytes);
        }
        for w in self.pending_waits.iter_mut() {
            w.lower_bound = 0;
        }
        self.try_resolve_waits();
    }

    fn try_resolve_waits(&mut self) {
        if self.pending_waits.is_empty() {
            return;
        }
        let stripped = self.stripped_view();
        let mut still_pending = Vec::with_capacity(self.pending_waits.len());
        for wait in std::mem::take(&mut self.pending_waits) {
            if let Some(m) = match_at(&stripped, &wait.pattern, wait.lower_bound) {
                let _ = wait.waker.send(Ok(m));
            } else {
                still_pending.push(wait);
            }
        }
        self.pending_waits = still_pending;
    }

    fn mark_terminal(&mut self, reason: TerminalReason) {
        if self.terminal.is_some() {
            return;
        }
        self.terminal = Some(reason.clone());
        for wait in std::mem::take(&mut self.pending_waits) {
            let _ = wait.waker.send(Err(reason.clone().into()));
        }
    }

    fn raw_view(&self) -> Vec<u8> {
        let (a, b) = self.rolling.as_slices();
        let mut v = Vec::with_capacity(self.rolling.len());
        v.extend_from_slice(a);
        v.extend_from_slice(b);
        v
    }

    fn stripped_view(&self) -> Vec<u8> {
        strip_ansi_for_matching(&self.raw_view())
    }
}

// ── ANSI scrubbing ──────────────────────────────────────────────

/// Strip the most common ANSI escape sequences from `buf` for
/// pattern matching. Not a full xterm emulator — just enough to
/// make regexes like `r"esc to interrupt"` and `r"Run /login"` hit
/// reliably when those phrases are surrounded by color/cursor
/// escapes.
///
/// Handles:
/// - CSI sequences: `ESC [ <params> <final-byte>` where final is in
///   `0x40..=0x7e` (covers DEC private modes like `ESC [ ?25h`).
/// - OSC sequences: `ESC ] ... BEL` or `ESC ] ... ESC \`.
/// - SS3 sequences: `ESC O <final>` (3 bytes — used for function
///   keys, e.g. `ESC O P` for F1).
/// - Charset designation: `ESC ( <ch>`, `ESC ) <ch>`, `ESC * <ch>`,
///   `ESC + <ch>`, `ESC - <ch>`, `ESC . <ch>`, `ESC / <ch>` (3 bytes).
/// - Cursor save / restore: `ESC 7`, `ESC 8` (2 bytes).
/// - Two-byte ESC fallback for unrecognized introducers.
///
/// Anything else passes through. Carriage returns (`\r`) and
/// newlines (`\n`) are preserved so line-based queries work.
fn strip_ansi_for_matching(buf: &[u8]) -> Vec<u8> {
    /// Introducers where ESC + intro + 1 more byte form the
    /// complete sequence (not CSI/OSC parameterized).
    const THREE_BYTE_INTRODUCERS: &[u8] = b"O()*+-./";

    let mut out = Vec::with_capacity(buf.len());
    let mut i = 0;
    while i < buf.len() {
        let b = buf[i];
        if b == 0x1b {
            // ESC. Look at the next byte to decide the sequence.
            if i + 1 >= buf.len() {
                // Trailing lone ESC — drop.
                break;
            }
            let next = buf[i + 1];
            match next {
                b'[' => {
                    // CSI: read until a byte in 0x40..=0x7e is the final.
                    i += 2;
                    while i < buf.len() {
                        let c = buf[i];
                        i += 1;
                        if (0x40..=0x7e).contains(&c) {
                            break;
                        }
                    }
                }
                b']' => {
                    // OSC: read until BEL or ST (ESC \).
                    i += 2;
                    while i < buf.len() {
                        if buf[i] == 0x07 {
                            i += 1;
                            break;
                        }
                        if buf[i] == 0x1b && i + 1 < buf.len() && buf[i + 1] == b'\\' {
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                }
                _ if THREE_BYTE_INTRODUCERS.contains(&next) && i + 2 < buf.len() => {
                    // SS3 (ESC O X) or charset designation
                    // (ESC ( X / ESC * X etc.) — consume all three
                    // bytes so the trailing X doesn't leak through.
                    i += 3;
                }
                _ => {
                    // Two-byte escape fallback: ESC 7 (save cursor),
                    // ESC 8 (restore cursor), ESC = (application
                    // keypad), ESC > (normal keypad), and anything
                    // else with just an introducer byte.
                    i += 2;
                }
            }
        } else {
            out.push(b);
            i += 1;
        }
    }
    out
}

/// Run `pattern` against `stripped[lower_bound..]` and return a
/// `RegexMatch` with offsets translated back to absolute positions
/// in `stripped`. Returns `None` on no match or invalid UTF-8 in
/// the searchable region.
fn match_at(stripped: &[u8], pattern: &Regex, lower_bound: usize) -> Option<RegexMatch> {
    if lower_bound >= stripped.len() {
        return None;
    }
    let slice = std::str::from_utf8(&stripped[lower_bound..]).ok()?;
    let m = pattern.find(slice)?;
    Some(RegexMatch {
        start: lower_bound + m.start(),
        end: lower_bound + m.end(),
        matched_text: m.as_str().to_string(),
    })
}

// ── transport tasks ─────────────────────────────────────────────

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;
type WsWriter = SplitSink<WsStream, Message>;
type WsReader = SplitStream<WsStream>;

async fn writer_task(
    mut writer: WsWriter,
    mut rx: mpsc::Receiver<Outbound>,
    state: Arc<Mutex<AttachState>>,
) {
    while let Some(msg) = rx.recv().await {
        let json = match serde_json::to_string(&msg) {
            Ok(j) => j,
            Err(e) => {
                tracing::warn!(error = %e, "attach writer: serialize failed; dropping message");
                continue;
            }
        };
        if let Err(e) = writer.send(Message::text(json)).await {
            // Writer failed — propagate to state so callers waiting
            // on the reader stop spinning silently. Without this the
            // reader keeps draining frames while every send back to
            // katulong is dropped, and `wait_for` callers wait until
            // their own timeouts fire.
            tracing::warn!(error = %e, "attach writer: send failed; marking terminal");
            state
                .lock()
                .await
                .mark_terminal(TerminalReason::Transport(format!(
                    "writer send failed: {e}"
                )));
            break;
        }
    }
    let _ = writer.close().await;
}

async fn reader_task(
    mut reader: WsReader,
    state: Arc<Mutex<AttachState>>,
    writer_tx: mpsc::Sender<Outbound>,
    session_name: String,
) {
    while let Some(frame) = reader.next().await {
        let frame = match frame {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!(error = %e, "attach reader: ws error; marking terminal");
                state
                    .lock()
                    .await
                    .mark_terminal(TerminalReason::Transport(e.to_string()));
                return;
            }
        };
        let text = match frame {
            Message::Text(t) => t.to_string(),
            Message::Close(_) => {
                state.lock().await.mark_terminal(TerminalReason::Closed);
                return;
            }
            // Binary, ping, pong handled by tungstenite internally
            // for the most part; we don't expect them in protocol.
            _ => continue,
        };
        let msg: Inbound = match serde_json::from_str(&text) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    raw = %truncate_for_log(&text),
                    "attach reader: bad JSON; ignoring"
                );
                continue;
            }
        };
        dispatch_inbound(msg, &state, &writer_tx, &session_name).await;
    }
    state.lock().await.mark_terminal(TerminalReason::Closed);
}

async fn dispatch_inbound(
    msg: Inbound,
    state: &Arc<Mutex<AttachState>>,
    writer_tx: &mpsc::Sender<Outbound>,
    session_name: &str,
) {
    match msg {
        Inbound::Attached { session, data } if session == session_name => {
            // Treated as a snapshot (reconnect or re-attach).
            let mut st = state.lock().await;
            st.replace_buffer(data.into_bytes());
        }
        Inbound::SeqInit { session, seq } if session == session_name => {
            let mut st = state.lock().await;
            st.cursor = seq;
        }
        Inbound::PullResponse {
            session,
            data,
            cursor,
        } if session == session_name => {
            let mut st = state.lock().await;
            if !data.is_empty() {
                st.append_bytes(data.as_bytes());
            }
            st.cursor = cursor;
        }
        Inbound::PullSnapshot {
            session,
            data,
            cursor,
        } if session == session_name => {
            let mut st = state.lock().await;
            st.replace_buffer(data.into_bytes());
            st.cursor = cursor;
        }
        Inbound::Output {
            session,
            data,
            from_seq,
            cursor,
        } if session == session_name => {
            let mut st = state.lock().await;
            if from_seq == st.cursor {
                st.append_bytes(data.as_bytes());
                st.cursor = cursor;
            } else {
                // Gap — request a pull to fill in. `try_send` is
                // safe here because every subsequent `Output` or
                // `DataAvailable` re-triggers a pull; a dropped pull
                // doesn't strand the stream as long as more output
                // is flowing.
                let from = st.cursor;
                drop(st);
                let _ = writer_tx.try_send(Outbound::Pull {
                    from_seq: from,
                    session: Some(session_name.to_string()),
                });
            }
        }
        Inbound::DataAvailable { session } if session == session_name => {
            // DataAvailable is the server saying "I have something
            // for you but I'm not pushing it inline." If we drop
            // the resulting Pull and no further server event fires,
            // the stream stalls. Await `send` so backpressure
            // surfaces as the channel filling rather than data loss.
            //
            // **Stall mode worth knowing for future debugging**:
            // if the writer task is alive but stuck (e.g., the WS
            // peer is slow to acknowledge writes), this `await`
            // blocks the reader loop. The reader stops draining
            // incoming WS frames, TCP backpressure propagates
            // back to katulong, and katulong's per-client
            // `WS_BACKPRESSURE_BYTES` threshold kicks in. A
            // "katulong says it backpressured us" report should
            // point here first.
            let from = state.lock().await.cursor;
            if let Err(e) = writer_tx
                .send(Outbound::Pull {
                    from_seq: from,
                    session: Some(session_name.to_string()),
                })
                .await
            {
                tracing::warn!(
                    error = %e,
                    "attach reader: pull send failed (writer task dead); marking terminal"
                );
                state.lock().await.mark_terminal(TerminalReason::Closed);
            }
        }
        Inbound::Exit { session, code } if session == session_name => {
            state.lock().await.mark_terminal(TerminalReason::Exit(code));
        }
        Inbound::SessionRemoved { session } if session == session_name => {
            state
                .lock()
                .await
                .mark_terminal(TerminalReason::SessionRemoved);
        }
        Inbound::StateCheck { .. } => {
            // Drift detection — deferred; see file-level docs.
        }
        Inbound::Error { message } => {
            tracing::warn!(
                message = %truncate_for_log(&message),
                "attach reader: katulong reported error"
            );
            // Not always terminal — katulong sends `error` for
            // individual bad messages. Leave the attach alive.
        }
        Inbound::Pong => { /* heartbeat ack; deferred */ }
        _ => { /* ignored types */ }
    }
}

/// Wrap a paste body in bracketed-paste markers (no trailing
/// submit). Pulled out as a free function so the byte shape — the
/// load-bearing invariant of this whole PR — can be unit-tested
/// without standing up an attach handle.
pub(crate) fn wrap_paste(body: &str) -> String {
    format!("\u{001b}[200~{body}\u{001b}[201~")
}

/// Truncate a string for inclusion in error messages or tracing
/// logs. Bounds the size of untrusted server payloads before they
/// enter sipag's log pipeline.
fn truncate_for_log(s: &str) -> String {
    if s.len() <= MAX_LOG_PAYLOAD_LEN {
        s.to_string()
    } else {
        // Snip on a char boundary so we don't produce invalid UTF-8.
        let mut end = MAX_LOG_PAYLOAD_LEN;
        while !s.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        let mut out = s[..end].to_string();
        out.push('…');
        out
    }
}

// ── tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ws_url_swaps_https_for_wss() {
        assert_eq!(
            ws_url("https://katulong-og.felixflor.es"),
            "wss://katulong-og.felixflor.es/ws"
        );
        assert_eq!(
            ws_url("https://katulong-og.felixflor.es/"),
            "wss://katulong-og.felixflor.es/ws"
        );
    }

    #[test]
    fn ws_url_swaps_http_for_ws() {
        assert_eq!(ws_url("http://127.0.0.1:8080"), "ws://127.0.0.1:8080/ws");
    }

    #[test]
    fn build_origin_preserves_authority_verbatim() {
        // Default port omitted: Host header has no port, Origin
        // shouldn't either.
        assert_eq!(
            build_origin("https://katulong.example").unwrap(),
            "https://katulong.example"
        );
        // Default port explicit: Host header keeps it (tungstenite
        // preserves authority verbatim), Origin must too — katulong's
        // `new URL(origin).host === host` is exact string equality.
        assert_eq!(
            build_origin("https://katulong.example:443").unwrap(),
            "https://katulong.example:443"
        );
        assert_eq!(
            build_origin("http://127.0.0.1:7100").unwrap(),
            "http://127.0.0.1:7100"
        );
        // Non-default port: same.
        assert_eq!(
            build_origin("https://katulong.example:8443").unwrap(),
            "https://katulong.example:8443"
        );
    }

    #[test]
    fn build_origin_strips_path_query_fragment() {
        assert_eq!(
            build_origin("https://katulong.example/some/path").unwrap(),
            "https://katulong.example"
        );
        assert_eq!(
            build_origin("https://katulong.example/?q=x").unwrap(),
            "https://katulong.example"
        );
        assert_eq!(
            build_origin("https://katulong.example#frag").unwrap(),
            "https://katulong.example"
        );
        assert_eq!(
            build_origin("https://katulong.example:443/api?x=1#y").unwrap(),
            "https://katulong.example:443"
        );
    }

    #[test]
    fn build_origin_strips_userinfo() {
        // Misconfigured URL with embedded credentials must NOT leak
        // them into the Origin header.
        let o = build_origin("https://user:pass@katulong.example").unwrap();
        assert_eq!(o, "https://katulong.example");
        assert!(!o.contains("user"), "userinfo leaked: {o}");
        assert!(!o.contains("pass"), "userinfo leaked: {o}");
        // With port + path.
        assert_eq!(
            build_origin("https://u:p@katulong.example:8443/api").unwrap(),
            "https://katulong.example:8443"
        );
    }

    #[test]
    fn build_origin_trims_trailing_slash() {
        assert_eq!(
            build_origin("https://katulong.example/").unwrap(),
            "https://katulong.example"
        );
    }

    #[test]
    fn build_origin_rejects_non_http_scheme() {
        assert!(build_origin("ftp://katulong.example").is_err());
        assert!(build_origin("katulong.example").is_err()); // no scheme
        assert!(build_origin("https://").is_err()); // no authority
    }

    #[test]
    fn keyname_bytes_match_xterm_default_map() {
        assert_eq!(KeyName::Enter.bytes(), "\r");
        assert_eq!(KeyName::Escape.bytes(), "\u{001b}");
        assert_eq!(KeyName::CtrlC.bytes(), "\u{0003}");
        assert_eq!(KeyName::CtrlD.bytes(), "\u{0004}");
        assert_eq!(KeyName::Tab.bytes(), "\t");
        assert_eq!(KeyName::Up.bytes(), "\u{001b}[A");
        assert_eq!(KeyName::Down.bytes(), "\u{001b}[B");
        assert_eq!(KeyName::Right.bytes(), "\u{001b}[C");
        assert_eq!(KeyName::Left.bytes(), "\u{001b}[D");
        assert_eq!(KeyName::Raw("custom".into()).bytes(), "custom");
    }

    // ── ANSI stripping ──────────────────────────────────────

    #[test]
    fn strip_ansi_removes_csi_color() {
        let input = b"hello\x1b[31mworld\x1b[0m";
        assert_eq!(strip_ansi_for_matching(input), b"helloworld");
    }

    #[test]
    fn strip_ansi_preserves_plain_text_and_newlines() {
        let input = b"line one\nline two\rmid";
        assert_eq!(strip_ansi_for_matching(input), input);
    }

    #[test]
    fn strip_ansi_removes_clear_screen() {
        let input = b"\x1b[2Jhello";
        assert_eq!(strip_ansi_for_matching(input), b"hello");
    }

    #[test]
    fn strip_ansi_removes_cursor_position() {
        // ESC [ 12 ; 34 H — move cursor to row 12 col 34
        let input = b"before\x1b[12;34Hafter";
        assert_eq!(strip_ansi_for_matching(input), b"beforeafter");
    }

    #[test]
    fn strip_ansi_handles_osc_terminated_by_bel() {
        // OSC for setting window title: ESC ] 0 ; title BEL
        let input = b"\x1b]0;my title\x07after";
        assert_eq!(strip_ansi_for_matching(input), b"after");
    }

    #[test]
    fn strip_ansi_handles_osc_terminated_by_st() {
        // OSC terminated by ESC \
        let input = b"\x1b]0;t\x1b\\after";
        assert_eq!(strip_ansi_for_matching(input), b"after");
    }

    #[test]
    fn strip_ansi_drops_trailing_lone_esc() {
        let input = b"trailing\x1b";
        assert_eq!(strip_ansi_for_matching(input), b"trailing");
    }

    #[test]
    fn strip_ansi_keeps_paste_marker_payload() {
        // Bracketed-paste markers are themselves CSI sequences
        // (ESC [ 200 ~ and ESC [ 201 ~). After stripping, only the
        // body remains — which is the right behavior for matching
        // against pane content.
        let input = b"\x1b[200~hello world\x1b[201~";
        assert_eq!(strip_ansi_for_matching(input), b"hello world");
    }

    #[test]
    fn strip_ansi_removes_dec_private_mode() {
        // ESC [ ? 25 h — show cursor (DEC private mode set). The
        // `?` is a parameter byte; final `h` is in 0x40..=0x7e.
        let input = b"\x1b[?25hvisible";
        assert_eq!(strip_ansi_for_matching(input), b"visible");
    }

    #[test]
    fn strip_ansi_handles_cursor_save_restore() {
        // ESC 7 / ESC 8 are 2-byte sequences. Default 2-byte arm
        // should consume both bytes cleanly.
        let input = b"a\x1b7middle\x1b8z";
        assert_eq!(strip_ansi_for_matching(input), b"amiddlez");
    }

    #[test]
    fn strip_ansi_removes_ss3_function_key() {
        // ESC O P — F1 keystroke (SS3). The trailing P is part of
        // the sequence and must NOT leak into the output.
        let input = b"before\x1bOPafter";
        assert_eq!(strip_ansi_for_matching(input), b"beforeafter");
    }

    #[test]
    fn strip_ansi_removes_charset_designation() {
        // ESC ( B — designate G0 as USASCII. The trailing B is
        // part of the sequence.
        let input = b"start\x1b(Bend";
        assert_eq!(strip_ansi_for_matching(input), b"startend");
    }

    // ── wrap_paste ──────────────────────────────────────────

    #[test]
    fn wrap_paste_produces_bpm_with_no_trailing_cr() {
        // Load-bearing invariant — the original dispatch bug was a
        // trailing \r getting absorbed into the paste. Pin the
        // exact byte shape so a future edit can't regress it.
        let wrapped = wrap_paste("hello world");
        assert_eq!(wrapped, "\u{001b}[200~hello world\u{001b}[201~");
        assert!(
            !wrapped.ends_with('\r'),
            "paste body must NOT end with carriage return; \
             submit Enter is sent as a separate input() call"
        );
    }

    #[test]
    fn wrap_paste_round_trips_multi_line_body() {
        let body = "## Context\n\nLine A\nLine B\n";
        let wrapped = wrap_paste(body);
        assert!(wrapped.starts_with("\u{001b}[200~"));
        assert!(wrapped.ends_with("\u{001b}[201~"));
        let inner = &wrapped["\u{001b}[200~".len()..wrapped.len() - "\u{001b}[201~".len()];
        assert_eq!(inner, body);
    }

    // ── truncate_for_log ────────────────────────────────────

    #[test]
    fn truncate_for_log_passes_short_strings_through() {
        assert_eq!(truncate_for_log("short"), "short");
    }

    #[test]
    fn truncate_for_log_caps_at_max_payload_with_ellipsis() {
        let long = "x".repeat(MAX_LOG_PAYLOAD_LEN * 2);
        let out = truncate_for_log(&long);
        assert!(out.ends_with('…'));
        // The bytes-up-to-the-ellipsis must not exceed
        // MAX_LOG_PAYLOAD_LEN; the trailing char itself adds a few.
        let body_len = out.trim_end_matches('…').len();
        assert!(body_len <= MAX_LOG_PAYLOAD_LEN);
    }

    #[test]
    fn truncate_for_log_respects_utf8_char_boundary() {
        // Build a string whose MAX_LOG_PAYLOAD_LEN-th byte falls
        // mid-character. truncate_for_log should snap back to a
        // valid char boundary, not slice mid-codepoint.
        let mut s = "a".repeat(MAX_LOG_PAYLOAD_LEN - 1);
        s.push('é'); // 2-byte char that straddles the boundary
        let out = truncate_for_log(&s);
        // Resulting string should be valid UTF-8 by construction
        // (truncate_for_log returns a String) and end with `…`.
        assert!(out.ends_with('…'));
        let _ =
            std::str::from_utf8(out.as_bytes()).expect("truncate_for_log produced invalid UTF-8");
    }

    // ── match_at ────────────────────────────────────────────

    #[test]
    fn match_at_finds_basic_pattern() {
        let stripped = b"line one\nesc to interrupt\nline three";
        let re = Regex::new(r"esc to interrupt").unwrap();
        let m = match_at(stripped, &re, 0).expect("should match");
        assert_eq!(m.matched_text, "esc to interrupt");
        assert_eq!(m.start, 9);
        assert_eq!(m.end, 25);
    }

    #[test]
    fn match_at_respects_lower_bound() {
        // Same string, but lower_bound skips past the first occurrence.
        let stripped = b"foo bar foo bar";
        let re = Regex::new(r"foo").unwrap();
        let first = match_at(stripped, &re, 0).unwrap();
        assert_eq!(first.start, 0);
        let second = match_at(stripped, &re, 4).unwrap();
        assert_eq!(second.start, 8);
    }

    #[test]
    fn match_at_returns_none_when_lower_bound_at_end() {
        let stripped = b"hello";
        let re = Regex::new(r"hello").unwrap();
        assert!(match_at(stripped, &re, 5).is_none());
        assert!(match_at(stripped, &re, 100).is_none());
    }

    #[test]
    fn match_at_returns_none_on_no_match() {
        let stripped = b"hello world";
        let re = Regex::new(r"nope").unwrap();
        assert!(match_at(stripped, &re, 0).is_none());
    }

    // ── AttachState ────────────────────────────────────────

    #[test]
    fn attach_state_new_seeds_buffer_and_cursor() {
        let st = AttachState::new("snap".into(), 42);
        assert_eq!(st.cursor, 42);
        assert_eq!(st.raw_view(), b"snap");
        assert!(st.pending_waits.is_empty());
        assert!(st.terminal.is_none());
    }

    #[test]
    fn attach_state_append_extends_buffer() {
        let mut st = AttachState::new("hello".into(), 0);
        st.append_bytes(b" world");
        assert_eq!(st.raw_view(), b"hello world");
    }

    #[test]
    fn attach_state_append_evicts_when_over_cap() {
        let mut st = AttachState::new(String::new(), 0);
        // Fill past the cap.
        let chunk = vec![b'x'; BUFFER_SOFT_CAP / 4];
        for _ in 0..6 {
            st.append_bytes(&chunk);
        }
        assert_eq!(
            st.rolling.len(),
            BUFFER_SOFT_CAP,
            "buffer should be capped at BUFFER_SOFT_CAP"
        );
    }

    #[test]
    fn attach_state_replace_clears_then_extends() {
        let mut st = AttachState::new("before".into(), 0);
        st.replace_buffer(b"after".to_vec());
        assert_eq!(st.raw_view(), b"after");
    }

    #[tokio::test]
    async fn pending_wait_resolves_when_pattern_arrives() {
        let mut st = AttachState::new(String::new(), 0);
        let (tx, rx) = oneshot::channel();
        let pattern = Regex::new(r"hello").unwrap();
        st.pending_waits.push(PendingWait {
            pattern,
            lower_bound: 0,
            waker: tx,
        });

        // Initially not resolved.
        st.try_resolve_waits();
        assert_eq!(st.pending_waits.len(), 1);

        // Append bytes that match — wait should resolve.
        st.append_bytes(b"prefix hello suffix");
        assert!(
            st.pending_waits.is_empty(),
            "wait should have been consumed"
        );
        let result = rx.await.unwrap().unwrap();
        assert_eq!(result.matched_text, "hello");
    }

    #[tokio::test]
    async fn from_now_lower_bound_skips_pre_existing_match() {
        // Simulates WaitFrom::FromNow registration: caller looked at
        // the buffer length first, then registered with that as the
        // lower bound. Pre-existing matches don't satisfy the wait.
        let mut st = AttachState::new("old hello".into(), 0);
        let pre_existing_len = st.stripped_view().len();
        let (tx, rx) = oneshot::channel();
        let pattern = Regex::new(r"hello").unwrap();
        st.pending_waits.push(PendingWait {
            pattern,
            lower_bound: pre_existing_len,
            waker: tx,
        });
        st.try_resolve_waits();
        assert_eq!(
            st.pending_waits.len(),
            1,
            "should not have resolved against pre-existing match"
        );

        // Now append a new match — that should resolve.
        st.append_bytes(b" then hello again");
        assert!(st.pending_waits.is_empty());
        let result = rx.await.unwrap().unwrap();
        assert_eq!(result.matched_text, "hello");
    }

    #[tokio::test]
    async fn mark_terminal_resolves_all_pending_with_error() {
        let mut st = AttachState::new(String::new(), 0);
        let (tx1, rx1) = oneshot::channel();
        let (tx2, rx2) = oneshot::channel();
        let pat = Regex::new(r"never").unwrap();
        st.pending_waits.push(PendingWait {
            pattern: pat.clone(),
            lower_bound: 0,
            waker: tx1,
        });
        st.pending_waits.push(PendingWait {
            pattern: pat,
            lower_bound: 0,
            waker: tx2,
        });

        st.mark_terminal(TerminalReason::Exit(0));
        assert!(st.pending_waits.is_empty());
        assert!(rx1.await.unwrap().is_err());
        assert!(rx2.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn replace_buffer_resets_pending_wait_lower_bounds() {
        // wait_for with FromNow registers a lower_bound > 0. When
        // a snapshot arrives, those bounds are no longer meaningful
        // (the buffer was replaced wholesale). Verify they reset to 0
        // so the next match can fire against the snapshot content.
        let mut st = AttachState::new("preamble".into(), 0);
        let (tx, rx) = oneshot::channel();
        let pattern = Regex::new(r"target").unwrap();
        st.pending_waits.push(PendingWait {
            pattern,
            lower_bound: st.stripped_view().len(), // FromNow at this point
            waker: tx,
        });

        // Snapshot arrives containing the target.
        st.replace_buffer(b"new world contains target word".to_vec());
        assert!(st.pending_waits.is_empty());
        let result = rx.await.unwrap().unwrap();
        assert_eq!(result.matched_text, "target");
    }

    #[test]
    fn eviction_shifts_pending_lower_bound() {
        // When the buffer evicts from the front, any pending
        // wait's lower_bound must shift down (saturating at 0) so
        // the match window stays consistent.
        let mut st = AttachState::new(String::new(), 0);
        let (tx, _rx) = oneshot::channel();
        st.pending_waits.push(PendingWait {
            pattern: Regex::new(r"unreachable").unwrap(),
            lower_bound: 100,
            waker: tx,
        });
        // Force eviction by filling past cap.
        let big = vec![b'x'; BUFFER_SOFT_CAP + 1024];
        st.append_bytes(&big);
        // The buffer was completely flooded; lower_bound should
        // have shifted but stays within reason.
        assert!(st.pending_waits[0].lower_bound < 100);
    }

    #[tokio::test]
    async fn from_now_does_not_match_pre_existing_after_ansi_only_eviction() {
        // Replaces an earlier shape (`from_now_resists_eviction_with_ansi_in_evicted_region`)
        // that didn't actually exercise the bug: it evicted the
        // ENTIRE initial buffer, so the lower_bound over-shift
        // produced the same observable outcome under both buggy
        // and fixed code. This rewrite leaves matching content
        // in the retained region so the two code paths diverge.
        //
        // Coordinate-system regression test for the eviction
        // shift bug. The bug was: eviction shifted `lower_bound`
        // by the RAW byte count, but `lower_bound` lives in the
        // ANSI-stripped view. If the evicted prefix contained
        // MORE raw bytes than stripped bytes (ANSI escapes weigh
        // many raw, zero stripped), the over-shift dropped
        // `lower_bound` below the offset of retained pre-existing
        // matching content, causing a FromNow wait to falsely
        // resolve.
        //
        // Construction:
        //   raw      = "x"*10 + (20 × "\x1b[31m" = 100 raw, 0 stripped) + "hello"
        //   stripped = "xxxxxxxxxxhello"  (15 bytes)
        //
        // FromNow wait at stripped offset 15. Then we append just
        // enough pure-ANSI bytes that eviction removes exactly
        // 110 raw bytes (the "x"*10 + the ANSI block), but
        // "hello" stays in the rolling buffer.
        //   drop_n         = 110 raw bytes
        //   stripped_delta = 10 ("x"*10; the ANSI block strips to
        //                    nothing)
        //
        // Under the OLD buggy code: `lower_bound = 15 - 110`
        // saturates to 0; "hello" at stripped offset 0 matches;
        // wait WRONGLY resolves.
        //
        // Under the FIXED code: `lower_bound = 15 - 10 = 5`;
        // "hello" sits at stripped offset 0..5, NOT in
        // `stripped[5..]`; wait correctly stays pending.
        let mut st = AttachState::new(String::new(), 0);
        let mut initial = vec![b'x'; 10];
        for _ in 0..20 {
            initial.extend_from_slice(b"\x1b[31m"); // 5 raw, 0 stripped
        }
        initial.extend_from_slice(b"hello");
        st.append_bytes(&initial);
        assert_eq!(
            st.stripped_view().len(),
            15,
            "test setup: stripped buffer should be 'xxxxxxxxxxhello' (15)"
        );

        let (tx, mut rx) = oneshot::channel();
        st.pending_waits.push(PendingWait {
            pattern: Regex::new(r"hello").unwrap(),
            lower_bound: 15, // FromNow at current stripped tail
            waker: tx,
        });

        // Pad with pure-ANSI bytes so rolling grows to exactly
        // BUFFER_SOFT_CAP + 110. Eviction will then drop the
        // first 110 raw bytes — the "x"*10 + the ANSI block.
        let target_overflow = 10 + 100;
        let need = BUFFER_SOFT_CAP + target_overflow - st.rolling.len();
        let chunk = b"\x1b[31m";
        let mut flood = Vec::with_capacity(need);
        while flood.len() + chunk.len() <= need {
            flood.extend_from_slice(chunk);
        }
        flood.extend(std::iter::repeat_n(b'y', need - flood.len()));
        st.append_bytes(&flood);

        // Sanity: rolling capped at the soft cap, "hello" still
        // present in the stripped view.
        assert_eq!(st.rolling.len(), BUFFER_SOFT_CAP);
        let stripped_post = st.stripped_view();
        let stripped_str = std::str::from_utf8(&stripped_post).unwrap();
        assert!(
            stripped_str.contains("hello"),
            "test setup invalid: 'hello' was evicted along with the ANSI block"
        );

        // The wait must stay pending. Under the old buggy code
        // this would have resolved against the retained "hello".
        assert!(
            !st.pending_waits.is_empty(),
            "wait wrongly resolved against pre-existing 'hello' after \
             ANSI-only eviction over-shifted the lower_bound"
        );
        assert!(rx.try_recv().is_err());

        // Append a NEW "hello" — should resolve cleanly.
        st.append_bytes(b"\nfresh hello here");
        let res = rx.await.unwrap().unwrap();
        assert_eq!(res.matched_text, "hello");
    }

    #[test]
    fn new_trims_oversized_initial_buffer_to_soft_cap() {
        // Mirror of `replace_buffer_trims_oversized_snapshot_to_soft_cap`
        // for the constructor path. A katulong handshake that
        // delivers an outsized `attached.data` (somehow, even
        // though `MAX_MESSAGE_BYTES` is enforced by tungstenite)
        // must not produce a rolling buffer above `BUFFER_SOFT_CAP`.
        let mut huge = vec![b'a'; BUFFER_SOFT_CAP - 4];
        huge.extend_from_slice(b"tail");
        let mut oversized = vec![b'a'; 2048];
        oversized.extend(huge);
        let st = AttachState::new(String::from_utf8(oversized).unwrap(), 0);
        assert_eq!(st.rolling.len(), BUFFER_SOFT_CAP);
        let raw = st.raw_view();
        assert!(raw.ends_with(b"tail"));
    }

    #[test]
    fn replace_buffer_trims_oversized_snapshot_to_soft_cap() {
        // A katulong-supplied snapshot larger than BUFFER_SOFT_CAP
        // must NOT blow up the rolling buffer. We trim from the
        // FRONT (keep the most recent tail) and continue.
        let mut st = AttachState::new(String::new(), 0);
        let mut huge = vec![b'a'; BUFFER_SOFT_CAP - 4];
        huge.extend_from_slice(b"tail");
        // Total length = BUFFER_SOFT_CAP. Now grow it past the cap.
        let mut oversized = vec![b'a'; 2048];
        oversized.extend(huge);
        st.replace_buffer(oversized);
        assert_eq!(st.rolling.len(), BUFFER_SOFT_CAP);
        // The recent tail is still present (we kept the back, not
        // the front).
        let raw = st.raw_view();
        assert!(raw.ends_with(b"tail"));
    }

    // ── dispatch_inbound ────────────────────────────────────

    fn make_state_and_writer() -> (
        Arc<Mutex<AttachState>>,
        mpsc::Sender<Outbound>,
        mpsc::Receiver<Outbound>,
    ) {
        let state = Arc::new(Mutex::new(AttachState::new(String::new(), 0)));
        let (tx, rx) = mpsc::channel(8);
        (state, tx, rx)
    }

    #[tokio::test]
    async fn dispatch_inbound_pull_response_appends_and_advances_cursor() {
        let (state, tx, _rx) = make_state_and_writer();
        dispatch_inbound(
            Inbound::PullResponse {
                session: "s".into(),
                data: "hello".into(),
                cursor: 42,
            },
            &state,
            &tx,
            "s",
        )
        .await;
        let st = state.lock().await;
        assert_eq!(st.raw_view(), b"hello");
        assert_eq!(st.cursor, 42);
    }

    #[tokio::test]
    async fn dispatch_inbound_pull_response_empty_data_advances_cursor_only() {
        // Backpressure-skip path: server returns empty data with
        // an advanced cursor. Buffer stays unchanged; cursor jumps.
        let (state, tx, _rx) = make_state_and_writer();
        {
            let mut st = state.lock().await;
            st.append_bytes(b"existing");
        }
        dispatch_inbound(
            Inbound::PullResponse {
                session: "s".into(),
                data: String::new(),
                cursor: 99_999,
            },
            &state,
            &tx,
            "s",
        )
        .await;
        let st = state.lock().await;
        assert_eq!(st.raw_view(), b"existing");
        assert_eq!(st.cursor, 99_999);
    }

    #[tokio::test]
    async fn dispatch_inbound_output_in_order_appends() {
        let (state, tx, _rx) = make_state_and_writer();
        {
            let mut st = state.lock().await;
            st.cursor = 10;
        }
        dispatch_inbound(
            Inbound::Output {
                session: "s".into(),
                data: "abc".into(),
                from_seq: 10,
                cursor: 13,
            },
            &state,
            &tx,
            "s",
        )
        .await;
        let st = state.lock().await;
        assert_eq!(st.raw_view(), b"abc");
        assert_eq!(st.cursor, 13);
    }

    #[tokio::test]
    async fn dispatch_inbound_output_gap_triggers_pull() {
        let (state, tx, mut rx) = make_state_and_writer();
        {
            let mut st = state.lock().await;
            st.cursor = 100;
        }
        dispatch_inbound(
            Inbound::Output {
                session: "s".into(),
                data: "lost".into(),
                from_seq: 200, // gap — doesn't match cursor 100
                cursor: 204,
            },
            &state,
            &tx,
            "s",
        )
        .await;
        // Buffer unchanged (gap detected).
        let st = state.lock().await;
        assert_eq!(st.raw_view(), b"");
        assert_eq!(st.cursor, 100);
        drop(st);
        // A pull from current cursor should have been queued.
        let queued = rx.try_recv().expect("expected a Pull message");
        match queued {
            Outbound::Pull { from_seq, session } => {
                assert_eq!(from_seq, 100);
                assert_eq!(session.as_deref(), Some("s"));
            }
            other => panic!("expected Pull, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_inbound_data_available_pulls_from_current_cursor() {
        let (state, tx, mut rx) = make_state_and_writer();
        {
            let mut st = state.lock().await;
            st.cursor = 12_345;
        }
        dispatch_inbound(
            Inbound::DataAvailable {
                session: "s".into(),
            },
            &state,
            &tx,
            "s",
        )
        .await;
        let queued = rx.try_recv().expect("expected a Pull message");
        match queued {
            Outbound::Pull { from_seq, session } => {
                assert_eq!(from_seq, 12_345);
                assert_eq!(session.as_deref(), Some("s"));
            }
            other => panic!("expected Pull, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_inbound_session_removed_marks_terminal() {
        let (state, tx, _rx) = make_state_and_writer();
        dispatch_inbound(
            Inbound::SessionRemoved {
                session: "s".into(),
            },
            &state,
            &tx,
            "s",
        )
        .await;
        let st = state.lock().await;
        assert!(matches!(st.terminal, Some(TerminalReason::SessionRemoved)));
    }

    #[tokio::test]
    async fn dispatch_inbound_exit_marks_terminal_with_code() {
        let (state, tx, _rx) = make_state_and_writer();
        dispatch_inbound(
            Inbound::Exit {
                session: "s".into(),
                code: -1,
            },
            &state,
            &tx,
            "s",
        )
        .await;
        let st = state.lock().await;
        assert!(matches!(st.terminal, Some(TerminalReason::Exit(-1))));
    }

    #[tokio::test]
    async fn dispatch_inbound_ignores_mismatched_session() {
        // A message tagged with a different session must be a no-op
        // — sipag attaches to exactly one session per handle, and a
        // multiplexed katulong sending another session's frames by
        // mistake shouldn't mutate our state.
        let (state, tx, _rx) = make_state_and_writer();
        {
            let mut st = state.lock().await;
            st.append_bytes(b"original");
            st.cursor = 8;
        }
        dispatch_inbound(
            Inbound::PullResponse {
                session: "other-session".into(),
                data: "leak".into(),
                cursor: 12,
            },
            &state,
            &tx,
            "s",
        )
        .await;
        let st = state.lock().await;
        assert_eq!(st.raw_view(), b"original");
        assert_eq!(st.cursor, 8);
    }

    #[tokio::test]
    async fn dispatch_inbound_error_logs_but_does_not_terminate() {
        let (state, tx, _rx) = make_state_and_writer();
        dispatch_inbound(
            Inbound::Error {
                message: "Invalid request".into(),
            },
            &state,
            &tx,
            "s",
        )
        .await;
        let st = state.lock().await;
        assert!(
            st.terminal.is_none(),
            "Inbound::Error is not terminal; katulong sends it for individual bad messages"
        );
    }

    #[tokio::test]
    async fn dispatch_inbound_pull_snapshot_replaces_buffer_and_sets_cursor() {
        // Snapshot path: replace-buffer + cursor reset. Round-2
        // reviewer flagged this arm was untested.
        let (state, tx, _rx) = make_state_and_writer();
        {
            let mut st = state.lock().await;
            st.append_bytes(b"stale prior content");
            st.cursor = 99;
        }
        dispatch_inbound(
            Inbound::PullSnapshot {
                session: "s".into(),
                data: "fresh".into(),
                cursor: 4242,
            },
            &state,
            &tx,
            "s",
        )
        .await;
        let st = state.lock().await;
        assert_eq!(st.raw_view(), b"fresh");
        assert_eq!(st.cursor, 4242);
    }

    #[tokio::test]
    async fn dispatch_inbound_pull_snapshot_ignores_mismatched_session() {
        let (state, tx, _rx) = make_state_and_writer();
        {
            let mut st = state.lock().await;
            st.append_bytes(b"unchanged");
            st.cursor = 7;
        }
        dispatch_inbound(
            Inbound::PullSnapshot {
                session: "other".into(),
                data: "leak".into(),
                cursor: 999,
            },
            &state,
            &tx,
            "s",
        )
        .await;
        let st = state.lock().await;
        assert_eq!(st.raw_view(), b"unchanged");
        assert_eq!(st.cursor, 7);
    }

    // ── mark_terminal idempotency ───────────────────────────

    #[tokio::test]
    async fn mark_terminal_is_idempotent_first_writer_wins() {
        // Both reader and writer can call `mark_terminal`
        // concurrently when a transport breaks. The early-return
        // when already-terminal preserves the first reason and
        // prevents double-wake of resolved waiters.
        let mut st = AttachState::new(String::new(), 0);
        let (tx, rx) = oneshot::channel();
        st.pending_waits.push(PendingWait {
            pattern: Regex::new(r"never").unwrap(),
            lower_bound: 0,
            waker: tx,
        });

        st.mark_terminal(TerminalReason::Exit(0));
        // First call drained the pending waits.
        assert!(st.pending_waits.is_empty());

        // Second call must be a no-op: still terminal with the
        // original reason; no panic, no second waker send.
        st.mark_terminal(TerminalReason::Transport("late".into()));
        assert!(matches!(st.terminal, Some(TerminalReason::Exit(0))));

        // The waker received exactly one Err — confirmed by
        // awaiting the rx and observing the SessionExited error.
        let err = rx.await.unwrap().unwrap_err();
        assert!(matches!(err, AttachError::SessionExited(0)));
    }

    // ── wait_for timeout branch ─────────────────────────────

    fn make_test_attach() -> KatulongAttach {
        // Synthesizes a `KatulongAttach` without standing up a
        // real WS. The reader/writer task handles are `None`;
        // the only operations exercised in these tests are
        // `wait_for` (which only touches state + the oneshot
        // channels) and direct state manipulation through the
        // lock. NOT safe for tests that actually need keystrokes
        // to ride through the writer task.
        let state = Arc::new(Mutex::new(AttachState::new(String::new(), 0)));
        let (tx, _rx) = mpsc::channel(8);
        KatulongAttach {
            session_name: "test".into(),
            writer_tx: Some(tx),
            state,
            _writer_handle: None,
            _reader_handle: None,
        }
    }

    #[tokio::test]
    async fn wait_for_returns_timeout_after_max_wait() {
        // Pin the timeout branch of `wait_for` — round-1 + round-2
        // reviewers both flagged this was untested. Uses a real
        // 50ms timeout rather than tokio's `start_paused` (which
        // would require the `test-util` feature) — fast enough
        // that the test still runs in well under a second.
        let attach = make_test_attach();
        let re = Regex::new(r"never-matches").unwrap();
        let waited = Duration::from_millis(50);
        let result = attach
            .wait_for(&re, WaitFrom::FromAttach, Some(waited))
            .await;
        match result {
            Err(AttachError::Timeout(d)) => assert_eq!(d, waited),
            other => panic!("expected AttachError::Timeout, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn wait_for_immediate_match_returns_without_registering() {
        // If the pattern is already in the buffer, `wait_for` must
        // return without ever registering a pending wait. Pins the
        // fast path so a future edit can't silently make every
        // call go through the oneshot.
        let attach = make_test_attach();
        {
            let mut st = attach.state.lock().await;
            st.append_bytes(b"hello world");
        }
        let re = Regex::new(r"hello").unwrap();
        let result = attach
            .wait_for(&re, WaitFrom::FromAttach, Some(Duration::from_millis(50)))
            .await;
        assert!(result.is_ok());
        // No pending waits should remain (the immediate match
        // returned before registration).
        let st = attach.state.lock().await;
        assert!(st.pending_waits.is_empty());
    }

    #[tokio::test]
    async fn wait_for_returns_terminal_immediately_when_already_terminal() {
        let attach = make_test_attach();
        {
            let mut st = attach.state.lock().await;
            st.mark_terminal(TerminalReason::SessionRemoved);
        }
        let re = Regex::new(r"nomatch").unwrap();
        let result = attach
            .wait_for(&re, WaitFrom::FromAttach, Some(Duration::from_secs(1)))
            .await;
        assert!(matches!(result, Err(AttachError::SessionRemoved)));
    }

    #[tokio::test]
    async fn from_offset_skips_pre_snapshot_content_and_matches_post() {
        // Pins the snapshot-before-input pattern used by the v2
        // dispatch driver: seed the buffer with `"hello"`, take a
        // stripped_offset snapshot, then append more bytes that
        // *also* contain `"hello"`. FromOffset(snapshot) must skip
        // the pre-snapshot `"hello"` and resolve on the new one.
        let attach = make_test_attach();
        {
            let mut st = attach.state.lock().await;
            st.append_bytes(b"hello before snapshot\n");
        }
        let snapshot = attach.stripped_offset().await;
        assert!(snapshot > 0, "snapshot should be past the seeded content");

        // Register the wait, then append matching content.
        let attach_arc = std::sync::Arc::new(attach);
        let waiter = {
            let a = std::sync::Arc::clone(&attach_arc);
            let re = Regex::new(r"hello").unwrap();
            tokio::spawn(async move {
                a.wait_for(
                    &re,
                    WaitFrom::FromOffset(snapshot),
                    Some(Duration::from_secs(1)),
                )
                .await
            })
        };
        // Give the spawned task a moment to acquire the lock and
        // register its pending wait before we append.
        tokio::task::yield_now().await;
        {
            let mut st = attach_arc.state.lock().await;
            st.append_bytes(b"hello after snapshot");
        }
        let m = waiter.await.unwrap().unwrap();
        assert_eq!(m.matched_text, "hello");
        assert!(
            m.start >= snapshot,
            "match must be in the post-snapshot region: start={} snapshot={}",
            m.start,
            snapshot
        );
    }

    #[tokio::test]
    async fn from_offset_clamps_when_buffer_evicted_past_snapshot() {
        // If the buffer shrinks below the snapshot (replace_buffer,
        // or — in production — eviction), FromOffset(N) must clamp
        // to current length rather than reject. Documented behavior
        // in `WaitFrom::FromOffset` rustdoc.
        let attach = make_test_attach();
        {
            let mut st = attach.state.lock().await;
            st.append_bytes(b"longer pre-existing buffer content");
        }
        let stale_offset = attach.stripped_offset().await + 10_000;

        // Replace the buffer with shorter content — pending offsets
        // become "in the future" from FromOffset's perspective.
        {
            let mut st = attach.state.lock().await;
            st.replace_buffer(b"short".to_vec());
        }
        let re = Regex::new(r"target").unwrap();

        let attach_arc = std::sync::Arc::new(attach);
        let waiter = {
            let a = std::sync::Arc::clone(&attach_arc);
            tokio::spawn(async move {
                a.wait_for(
                    &re,
                    WaitFrom::FromOffset(stale_offset),
                    Some(Duration::from_secs(1)),
                )
                .await
            })
        };
        tokio::task::yield_now().await;
        {
            let mut st = attach_arc.state.lock().await;
            st.append_bytes(b" target appears");
        }
        let m = waiter.await.unwrap().unwrap();
        assert_eq!(m.matched_text, "target");
    }

    // ── writer-task transport propagation ───────────────────

    #[tokio::test]
    async fn mark_terminal_transport_resolves_pending_waits_with_attach_error_transport() {
        // The HIGH correctness fix in this PR: when the writer
        // task can't send (WS write-half error), it must call
        // `mark_terminal(TerminalReason::Transport(...))` on the
        // shared state. Doing so resolves all pending `wait_for`
        // futures with `AttachError::Transport(...)` — they
        // STOP waiting silently.
        //
        // Testing the actual writer_task end-to-end requires a
        // mockable WS sink. The propagation contract — terminal
        // reason → waiter error — is what callers depend on, and
        // is what we exercise here directly.
        let mut st = AttachState::new(String::new(), 0);
        let (tx, rx) = oneshot::channel();
        st.pending_waits.push(PendingWait {
            pattern: Regex::new(r"nomatch").unwrap(),
            lower_bound: 0,
            waker: tx,
        });
        st.mark_terminal(TerminalReason::Transport(
            "writer send failed: connection reset".into(),
        ));
        let err = rx.await.unwrap().unwrap_err();
        match err {
            AttachError::Transport(m) => {
                assert!(m.contains("writer send failed"));
                assert!(m.contains("connection reset"));
            }
            other => panic!("expected AttachError::Transport, got {other:?}"),
        }
    }
}
