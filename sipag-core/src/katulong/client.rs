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
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

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
    FromNow,
}

/// Named keystrokes for `KatulongAttach::press`.
#[derive(Debug, Clone)]
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
    /// arrives, or fails with `Timeout` after 10s.
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

        // Open WS.
        let (ws, _http_resp) = connect_async(req)
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
        let handshake_result = timeout(
            Duration::from_secs(10),
            run_handshake(&mut reader, &session),
        )
        .await;
        let (initial_buffer, initial_seq) = match handshake_result {
            Ok(Ok(pair)) => pair,
            Ok(Err(e)) => return Err(e),
            Err(_) => return Err(AttachError::Timeout(Duration::from_secs(10))),
        };

        // Set up the shared state and spawn tasks.
        let state = Arc::new(Mutex::new(AttachState::new(initial_buffer, initial_seq)));
        let (writer_tx, writer_rx) = mpsc::channel::<Outbound>(64);

        let writer_handle = tokio::spawn(writer_task(writer, writer_rx));
        let reader_handle = tokio::spawn(reader_task(
            reader,
            state.clone(),
            writer_tx.clone(),
            session.clone(),
        ));

        Ok(KatulongAttach {
            session_name: session,
            writer_tx,
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
        let msg: Inbound = serde_json::from_str(&text)
            .map_err(|e| AttachError::Wire(format!("handshake parse: {e}: {text}")))?;
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

/// One open attach to a katulong session. All methods are `&self`
/// — the handle is `Clone` (cheap, just two `Arc`s) so multiple
/// driver tasks can share it.
pub struct KatulongAttach {
    session_name: String,
    writer_tx: mpsc::Sender<Outbound>,
    state: Arc<Mutex<AttachState>>,
    // Held so the tasks aren't dropped while the attach is alive.
    // Aborted on `close()`.
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
        let wrapped = format!("\u{001b}[200~{body}\u{001b}[201~");
        self.input(wrapped).await
    }

    /// Send a named keystroke as its byte representation.
    pub async fn press(&self, key: KeyName) -> AttachResult<()> {
        self.input(key.bytes().to_string()).await
    }

    /// Inform katulong of new PTY dimensions. Sipag isn't rendering
    /// anything, but TUI apps reflow based on PTY size, so it's
    /// worth setting a reasonable default after attach.
    pub async fn resize(&self, cols: u16, rows: u16) -> AttachResult<()> {
        self.writer_tx
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

    /// Close the attach. Drops the writer channel (writer task
    /// exits cleanly) and aborts the reader task.
    pub async fn close(mut self) {
        // Dropping writer_tx signals the writer task to exit.
        drop(self.writer_tx);
        if let Some(h) = self._writer_handle.take() {
            let _ = h.await;
        }
        if let Some(h) = self._reader_handle.take() {
            h.abort();
        }
    }
}

// ── internal state ──────────────────────────────────────────────

/// Soft cap on the rolling buffer. When exceeded, we drop bytes
/// from the front (oldest). Pending `wait_for` lower-bounds shift
/// accordingly.
const BUFFER_SOFT_CAP: usize = 1_048_576; // 1 MiB

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

#[derive(Debug, Clone)]
enum TerminalReason {
    Exit(i32),
    SessionRemoved,
    Server(String),
    Closed,
}

impl From<TerminalReason> for AttachError {
    fn from(r: TerminalReason) -> Self {
        match r {
            TerminalReason::Exit(code) => AttachError::SessionExited(code),
            TerminalReason::SessionRemoved => AttachError::SessionRemoved,
            TerminalReason::Server(m) => AttachError::Server(m),
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
        s.rolling.extend(initial_buffer.bytes());
        s
    }

    /// Append new bytes; evict from the front if over the soft cap;
    /// try to resolve any pending waits.
    fn append_bytes(&mut self, bytes: &[u8]) {
        self.rolling.extend(bytes.iter().copied());
        if self.rolling.len() > BUFFER_SOFT_CAP {
            let drop_n = self.rolling.len() - BUFFER_SOFT_CAP;
            self.rolling.drain(..drop_n);
            for w in self.pending_waits.iter_mut() {
                w.lower_bound = w.lower_bound.saturating_sub(drop_n);
            }
        }
        self.try_resolve_waits();
    }

    /// Replace the entire buffer (snapshot recovery). After this,
    /// all pending wait_for `lower_bound`s become meaningless — we
    /// reset them to 0 so the next match starts from the snapshot.
    fn replace_buffer(&mut self, bytes: Vec<u8>) {
        self.rolling.clear();
        self.rolling.extend(bytes);
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
///   `0x40..=0x7e`
/// - OSC sequences: `ESC ] ... BEL` or `ESC ] ... ESC \`
/// - Standalone two-byte ESC + intermediate (charset selection etc.)
///
/// Anything else passes through. Carriage returns (`\r`) and
/// newlines (`\n`) are preserved so line-based queries work.
fn strip_ansi_for_matching(buf: &[u8]) -> Vec<u8> {
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
            match buf[i + 1] {
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
                _ => {
                    // Two-byte escape (charset selection, etc.).
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

async fn writer_task(mut writer: WsWriter, mut rx: mpsc::Receiver<Outbound>) {
    while let Some(msg) = rx.recv().await {
        let json = match serde_json::to_string(&msg) {
            Ok(j) => j,
            Err(e) => {
                tracing::warn!(error = %e, "attach writer: serialize failed; dropping message");
                continue;
            }
        };
        if let Err(e) = writer.send(Message::text(json)).await {
            tracing::warn!(error = %e, "attach writer: send failed; closing channel");
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
                    .mark_terminal(TerminalReason::Server(e.to_string()));
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
                tracing::warn!(error = %e, raw = %text, "attach reader: bad JSON; ignoring");
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
                // Gap — request a pull to fill in. Use try_send so a
                // full writer channel doesn't deadlock; if it's
                // full, the next data-available will trigger another.
                let from = st.cursor;
                drop(st);
                let _ = writer_tx.try_send(Outbound::Pull {
                    from_seq: from,
                    session: Some(session_name.to_string()),
                });
            }
        }
        Inbound::DataAvailable { session } if session == session_name => {
            let from = state.lock().await.cursor;
            let _ = writer_tx.try_send(Outbound::Pull {
                from_seq: from,
                session: Some(session_name.to_string()),
            });
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
            tracing::warn!(message = %message, "attach reader: katulong reported error");
            // Not always terminal — katulong sends `error` for
            // individual bad messages. Leave the attach alive.
        }
        Inbound::Pong => { /* heartbeat ack; deferred */ }
        _ => { /* ignored types */ }
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
}
