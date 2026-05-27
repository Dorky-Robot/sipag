//! SSE subscriber for katulong's pub/sub broker.
//!
//! Third wire surface alongside the WS attach client (`attach.rs`)
//! and the async HTTP client (`async_http.rs`). Subscribes to a
//! katulong topic (typically `claude/<uuid>` or `sessions/<id>/*`)
//! and yields parsed [`KatulongEvent`]s as an async [`Stream`].
//!
//! ## Wire shape
//!
//! katulong's broker emits standard
//! [server-sent events](https://html.spec.whatwg.org/multipage/server-sent-events.html):
//!
//! ```text
//! event: <type>
//! data: <json>
//!
//! ```
//!
//! The `data:` payload is a JSON object whose minimum shape is
//! `{ seq, event, timestamp, ... }`. Per-event-type fields are
//! captured into [`KatulongEvent::extra`] via `#[serde(flatten)]`
//! so callers can deserialize the typed shape they care about
//! without this module having to know every event type up front.
//!
//! ## Resumption
//!
//! [`subscribe`] takes a `from_seq` parameter; katulong's broker
//! honors `?fromSeq=N` and replays any retained events with
//! `seq >= N` before catching up to live. The wrapper does NOT
//! auto-reconnect — callers (the lens-worker bridge, today's
//! `sipag sub` CLI tomorrow) drive reconnection by tracking the
//! highest seen `seq` and re-subscribing from `seq + 1` after the
//! stream ends.
//!
//! ## Errors
//!
//! [`SseError`] surfaces transport failures (network, HTTP 4xx/5xx
//! on connect, malformed event JSON) as `Err` items in the stream
//! rather than silently swallowing them. The bridge worker will
//! decide its own retry policy per error class.

use futures_util::stream::Stream;
use serde::Deserialize;
use std::pin::Pin;
use std::task::{Context as TaskContext, Poll};

/// One event from a katulong pub/sub topic. Parsed from the SSE
/// `data:` payload (which is JSON).
///
/// The "core" fields (`seq` / `event` / `timestamp` / optional
/// `session`) are pulled out as typed fields; per-event-type
/// payload lives in [`Self::extra`] so callers deserialize against
/// their own shape without this module having to enumerate every
/// event type katulong might emit.
#[derive(Debug, Clone, Deserialize)]
pub struct KatulongEvent {
    /// Sequence number assigned by katulong's broker. Monotonic
    /// per-topic. Pass `seq + 1` as `from_seq` on reconnect to
    /// resume without duplicates.
    pub seq: u64,
    /// Event type (free-form per topic). Examples:
    /// - on `claude/<uuid>`: `permission-request`, `agent-done`,
    ///   `tool-use`, ...
    /// - on `observations/activity`: `dispatch.outcome`, ...
    pub event: String,
    /// RFC3339 timestamp recorded by the broker.
    pub timestamp: String,
    /// Session id when the event is session-scoped. `None` for
    /// project- or topic-level events.
    #[serde(default)]
    pub session: Option<String>,
    /// All other fields from the JSON payload. Consumers
    /// `serde_json::from_value(serde_json::Value::Object(event.extra))`
    /// into their per-event-type shape.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// What can go wrong subscribing to or reading from a katulong
/// SSE stream.
#[derive(Debug, thiserror::Error)]
pub enum SseError {
    /// `reqwest::Client::execute` failed — DNS, TCP, TLS, etc.
    #[error("transport error: {0}")]
    Transport(String),
    /// Katulong returned non-2xx on the SSE connect request.
    /// `status` is the HTTP status; `body` is the truncated response
    /// body (capped so a multi-MB error page doesn't blow up the log).
    #[error("connect failed: HTTP {status}{}",
        if .body.is_empty() { String::new() }
        else { format!(" — {}", .body) }
    )]
    Connect { status: u16, body: String },
    /// A `data:` payload arrived but wasn't valid JSON of the
    /// [`KatulongEvent`] shape. The malformed payload is logged
    /// but the stream continues (this variant is constructed by
    /// [`KatulongEventStream`] but immediately propagated as a
    /// stream item; consumers can choose to skip or stop).
    #[error("malformed event JSON: {0}")]
    BadEvent(String),
    /// IO error reading the response body chunk stream.
    #[error("body read error: {0}")]
    Body(String),
    /// A single SSE line (bytes up to the next `\n`) exceeded
    /// [`LINE_BUF_CAP`]. Defends against a hostile or buggy upstream
    /// streaming an unbounded line and OOMing the subscriber. The
    /// stream is terminated after this error — subsequent
    /// `poll_next` returns `None`. The cap is per-line, not
    /// per-buffer: a chunk containing many short lines drains
    /// normally even if its total size exceeds the cap.
    #[error("SSE line exceeded {limit} bytes without a newline")]
    LineTooLong { limit: usize },
    /// One SSE event's accumulated `data:` payload would exceed
    /// [`EVENT_BUF_CAP`] if the next `data:` line were appended.
    /// Same defense as `LineTooLong` but for the per-event
    /// accumulator. The stream is terminated after this error.
    #[error("SSE event payload would exceed {limit} bytes")]
    EventTooLarge { limit: usize },
}

/// Cap on how much of the connect-failure body we paste into
/// [`SseError::Connect`]. Mirrors `katulong-client::async_http`'s
/// `DEFAULT_BODY_CAP` philosophy: bounded error context, no
/// unbounded log inflation if the upstream returns a huge page.
const CONNECT_ERROR_BODY_CAP: usize = 1024;

/// Cap on the line buffer — bytes between two `\n`s on the SSE
/// wire. 256 KiB is well above any realistic katulong event
/// payload (gemma prompts, file contents, etc.) while bounding
/// the OOM blast radius from a hostile or buggy upstream that
/// streams without newlines.
pub const LINE_BUF_CAP: usize = 256 * 1024;

/// Cap on the per-event payload buffer — sum of all `data:` lines
/// in one event. 1 MiB matches `katulong-client::async_http`'s
/// `DEFAULT_BODY_CAP`; same philosophy (consumer can't tell the
/// difference between "huge legitimate event" and "adversarial
/// stuffing," so bound both).
pub const EVENT_BUF_CAP: usize = 1024 * 1024;

/// Truncate a `String` at a byte index that may not be on a UTF-8
/// char boundary. Walks back to the nearest boundary at or before
/// `target_len` and truncates there. `String::truncate` itself
/// PANICS on a non-boundary index — this wrapper is the safe form
/// for arbitrary upstream content that may contain multi-byte chars
/// straddling the cap.
fn truncate_at_char_boundary(s: &mut String, target_len: usize) {
    if s.len() <= target_len {
        return;
    }
    let mut cap = target_len;
    while !s.is_char_boundary(cap) {
        cap -= 1;
    }
    s.truncate(cap);
}

/// Async stream of events from a katulong pub/sub topic. Created
/// via [`subscribe`].
///
/// Yields `Result<KatulongEvent, SseError>` one event at a time.
/// The stream ends when the underlying HTTP response body ends
/// (which for SSE is typically only on network drop, server
/// shutdown, or the broker actively closing the connection).
/// Callers drive reconnection.
pub struct KatulongEventStream {
    body: Pin<Box<dyn Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send>>,
    // Line-decoder state: buffered bytes that haven't yet formed a
    // complete `\n`-terminated line.
    line_buf: Vec<u8>,
    // SSE event accumulator: when a blank line arrives, we emit
    // the buffered `data:` content (parsed as JSON) and reset.
    data_buf: String,
    // Once a cap-trip surfaces `LineTooLong` / `EventTooLarge`, the
    // wire is in an unrecoverable mid-event state — the next bytes
    // would be parsed as the tail of an oversized event. Latch
    // termination so subsequent polls return `None`, matching what
    // the error variants' doc comments promise.
    terminated: bool,
}

impl Stream for KatulongEventStream {
    type Item = Result<KatulongEvent, SseError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        if self.terminated {
            return Poll::Ready(None);
        }
        loop {
            // Try to extract a complete SSE event from what's
            // already in `line_buf` + `data_buf` before pulling
            // more bytes from the network. The SSE record
            // separator is a blank line, i.e. `\n\n` or `\n`
            // after a `\n`.
            if let Some(event) = self.drain_complete_event() {
                if matches!(event, Err(SseError::EventTooLarge { .. })) {
                    self.terminated = true;
                }
                return Poll::Ready(Some(event));
            }

            // Need more bytes. Poll the body stream.
            match self.body.as_mut().poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => {
                    // Stream ended. If we have a half-built event
                    // sitting in the buffer it's malformed (no
                    // trailing blank line); discard it. The
                    // reconnect contract (consumer resumes from
                    // `last_seen_seq + 1`; broker replays retained
                    // events) covers the lost-event case.
                    self.terminated = true;
                    return Poll::Ready(None);
                }
                Poll::Ready(Some(Err(e))) => {
                    self.terminated = true;
                    return Poll::Ready(Some(Err(SseError::Body(e.to_string()))));
                }
                Poll::Ready(Some(Ok(chunk))) => {
                    self.line_buf.extend_from_slice(&chunk);
                    // The cap is per-line, not per-buffer: a chunk
                    // containing many short lines is fine — they'll
                    // drain on the next loop iteration. Only an
                    // unterminated leading line bigger than the cap
                    // is the OOM vector we're guarding against.
                    let first_line_len = self
                        .line_buf
                        .iter()
                        .position(|&b| b == b'\n')
                        .unwrap_or(self.line_buf.len());
                    if first_line_len > LINE_BUF_CAP {
                        self.line_buf.clear();
                        self.data_buf.clear();
                        self.terminated = true;
                        return Poll::Ready(Some(Err(SseError::LineTooLong {
                            limit: LINE_BUF_CAP,
                        })));
                    }
                    // Loop around to try draining again.
                }
            }
        }
    }
}

impl KatulongEventStream {
    /// Pull complete SSE events out of the line buffer. Returns
    /// `Some(_)` when a blank-line-terminated event is available;
    /// `None` when we need more bytes.
    ///
    /// SSE wire format (the subset katulong uses):
    /// ```text
    /// event: <type>
    /// data: <line1>
    /// data: <line2>
    ///
    /// ```
    /// Multiple `data:` lines concatenate with `\n`. The blank line
    /// terminates the event; we then parse the accumulated data as
    /// JSON.
    fn drain_complete_event(&mut self) -> Option<Result<KatulongEvent, SseError>> {
        while let Some(line) = pop_line(&mut self.line_buf) {
            if line.is_empty() {
                // Blank line — end of event.
                if self.data_buf.is_empty() {
                    // Heartbeat / keepalive comment lines (`:` lines)
                    // were already dropped; a blank line with no
                    // accumulated data is a noop.
                    continue;
                }
                let json = std::mem::take(&mut self.data_buf);
                return Some(parse_event_json(&json));
            }
            if line.starts_with(':') {
                // Comment / heartbeat — ignore.
                continue;
            }
            if let Some(data) = line.strip_prefix("data:") {
                let data = data.strip_prefix(' ').unwrap_or(data);
                let separator = if self.data_buf.is_empty() { 0 } else { 1 };
                let projected = self
                    .data_buf
                    .len()
                    .saturating_add(separator)
                    .saturating_add(data.len());
                // Pre-check: refuse to grow `data_buf` past the cap.
                // Matches `line_buf`'s pre-check; same OOM-blast-
                // radius rationale.
                if projected > EVENT_BUF_CAP {
                    self.data_buf.clear();
                    return Some(Err(SseError::EventTooLarge {
                        limit: EVENT_BUF_CAP,
                    }));
                }
                if separator == 1 {
                    self.data_buf.push('\n');
                }
                self.data_buf.push_str(data);
                continue;
            }
            // SSE has `event:`, `id:`, `retry:` lines too. We
            // ignore them — the event type lives inside the JSON
            // `event` field, and we don't honor retry hints (the
            // bridge worker drives reconnect explicitly).
        }
        None
    }
}

/// Pop one `\n`-terminated line off the front of `buf`. Returns
/// `None` if no full line is present yet. Trims a single trailing
/// `\r` for CRLF tolerance.
fn pop_line(buf: &mut Vec<u8>) -> Option<String> {
    let newline = buf.iter().position(|&b| b == b'\n')?;
    let line: Vec<u8> = buf.drain(..=newline).take(newline).collect();
    let line = match line.strip_suffix(b"\r") {
        Some(s) => s.to_vec(),
        None => line,
    };
    Some(String::from_utf8_lossy(&line).into_owned())
}

fn parse_event_json(raw: &str) -> Result<KatulongEvent, SseError> {
    serde_json::from_str::<KatulongEvent>(raw)
        .map_err(|e| SseError::BadEvent(format!("{e}: {raw}")))
}

/// Subscribe to a katulong pub/sub topic. Returns a stream that
/// yields events as they arrive.
///
/// `from_seq` is honored by katulong's broker — the broker replays
/// any retained events with `seq >= from_seq` before catching up
/// to live. Pass `0` for "everything still in the broker plus
/// everything new"; pass the previous-call's `last_seen_seq + 1`
/// for clean resumption.
///
/// The stream does NOT auto-reconnect. The caller (bridge worker,
/// future `sipag sub` CLI, ...) re-invokes `subscribe` on
/// transport failure with the last seen `seq + 1`.
pub async fn subscribe(
    http: reqwest::Client,
    base_url: &str,
    api_key: &str,
    topic: &str,
    from_seq: u64,
) -> Result<KatulongEventStream, SseError> {
    let url = sub_url(base_url, topic, from_seq);
    let resp = http
        .get(&url)
        .bearer_auth(api_key)
        .header("Accept", "text/event-stream")
        // SSE is a long-lived connection; no per-request timeout.
        // The underlying reqwest::Client may have a connect timeout;
        // body read is open-ended by design.
        .send()
        .await
        .map_err(|e| SseError::Transport(format!("{e} (url={url})")))?;

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body = resp
            .text()
            .await
            .map(|b| {
                let mut snippet = b.trim().to_string();
                if snippet.len() > CONNECT_ERROR_BODY_CAP {
                    // `String::truncate` PANICS on a non-char-boundary
                    // index — and arbitrary upstream bodies routinely
                    // contain multi-byte UTF-8 (any localized error
                    // page). Walk back to the nearest boundary at or
                    // below the cap.
                    truncate_at_char_boundary(&mut snippet, CONNECT_ERROR_BODY_CAP);
                    snippet.push('…');
                }
                snippet
            })
            .unwrap_or_default();
        return Err(SseError::Connect { status, body });
    }

    Ok(KatulongEventStream {
        body: Box::pin(resp.bytes_stream()),
        line_buf: Vec::new(),
        data_buf: String::new(),
        terminated: false,
    })
}

/// Build the SSE subscribe URL. `/` in the topic is `%2F`-encoded
/// so the topic stays one path segment — katulong's broker uses
/// slashes inside topic names (e.g. `crew/<project>/<role>/...`,
/// `claude/<uuid>`). Mirrors [`crate::http::RemoteConfig::sub_url`]
/// — kept in this module so the SSE subscriber doesn't depend on
/// the sync HTTP module's `RemoteConfig` struct.
fn sub_url(base: &str, topic: &str, from_seq: u64) -> String {
    let encoded_topic = topic.replace('/', "%2F");
    format!(
        "{}/sub/{encoded_topic}?fromSeq={from_seq}",
        base.trim_end_matches('/')
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::Notify;

    /// Spin up a tiny TCP server that pretends to be katulong's
    /// SSE endpoint. Calls `responder(stream)` to write the response
    /// (headers + body) once a connection arrives. Returns the
    /// bound URL.
    ///
    /// Hand-rolled to avoid pulling axum / wiremock / mockito into
    /// dev-deps for tests this small.
    async fn spawn_sse_server<F, Fut>(responder: F) -> (String, Arc<Notify>)
    where
        F: FnOnce(tokio::net::TcpStream) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let done = Arc::new(Notify::new());
        let done_clone = done.clone();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            // Drain the request line + headers so the responder
            // doesn't have to. Reads until the request-end blank line.
            let mut buf = [0u8; 4096];
            loop {
                let n = socket.read(&mut buf).await.unwrap_or(0);
                if n == 0 {
                    break;
                }
                // Naive: assume the request fits in one buffer and
                // contains the `\r\n\r\n` terminator. Good enough
                // for test HTTP GETs.
                if buf[..n].windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            responder(socket).await;
            done_clone.notify_one();
        });
        (format!("http://{addr}"), done)
    }

    /// Convenience: write a minimal `200 OK` SSE response head,
    /// then a body the test supplies.
    async fn write_sse_response(socket: &mut tokio::net::TcpStream, body: &str) {
        let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n";
        socket.write_all(head.as_bytes()).await.unwrap();
        socket.write_all(body.as_bytes()).await.unwrap();
        socket.flush().await.unwrap();
    }

    // ── feature-requirement tests ──────────────────────────────────
    //
    // These pin what the SSE subscriber PROMISES to consumers
    // (lens-worker bridge, today's `sipag sub` CLI tomorrow):
    //
    // - Subscribe returns a Stream of typed KatulongEvents parsed
    //   from the SSE `data:` JSON payloads.
    // - Per-event-type fields are accessible via `event.extra`.
    // - Heartbeat / comment lines (`:` prefix) are dropped silently.
    // - Non-2xx connect surfaces as SseError::Connect with bounded
    //   body context.
    // - Malformed event JSON surfaces as a stream Err item rather
    //   than ending the stream.
    // - from_seq is encoded into the URL so katulong's broker can
    //   replay retained events.
    // - Topics containing `/` are percent-encoded so the broker
    //   sees one path segment.

    fn sample_event(seq: u64, kind: &str) -> String {
        format!(
            r#"{{"seq":{seq},"event":"{kind}","timestamp":"2026-05-26T10:00:00Z","session":"sess-1","payload":"hi"}}"#
        )
    }

    #[tokio::test]
    async fn subscribe_parses_two_back_to_back_events() {
        let body = format!(
            "event: x\ndata: {}\n\nevent: y\ndata: {}\n\n",
            sample_event(1, "permission-request"),
            sample_event(2, "agent-done"),
        );
        let (url, done) = spawn_sse_server(move |mut s| async move {
            write_sse_response(&mut s, &body).await;
            // Leave the socket open briefly so the client reads
            // before EOF, then drop it (subscribe's stream ends).
            let _ = s.shutdown().await;
        })
        .await;

        let http = reqwest::Client::new();
        let mut stream = subscribe(http, &url, "test-key", "claude/uuid-1", 0)
            .await
            .expect("connect");

        let first = stream.next().await.unwrap().unwrap();
        assert_eq!(first.seq, 1);
        assert_eq!(first.event, "permission-request");
        assert_eq!(first.session.as_deref(), Some("sess-1"));
        assert_eq!(
            first.extra.get("payload").and_then(|v| v.as_str()),
            Some("hi"),
            "per-event-type fields land in extra"
        );

        let second = stream.next().await.unwrap().unwrap();
        assert_eq!(second.seq, 2);
        assert_eq!(second.event, "agent-done");

        // Server closed the socket after writing — stream ends.
        assert!(stream.next().await.is_none(), "stream ends on EOF");
        done.notified().await;
    }

    #[tokio::test]
    async fn subscribe_ignores_comment_and_keepalive_lines() {
        let body = format!(
            ":heartbeat\n\ndata: {}\n\n:another-heartbeat\n\n",
            sample_event(5, "tool-use"),
        );
        let (url, _done) = spawn_sse_server(move |mut s| async move {
            write_sse_response(&mut s, &body).await;
            let _ = s.shutdown().await;
        })
        .await;

        let http = reqwest::Client::new();
        let mut stream = subscribe(http, &url, "k", "claude/u", 0).await.unwrap();
        let evt = stream.next().await.unwrap().unwrap();
        assert_eq!(evt.seq, 5);
        assert_eq!(evt.event, "tool-use");
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn subscribe_surfaces_malformed_json_as_stream_err() {
        let body = "data: this is definitely not json\n\n";
        let (url, _done) = spawn_sse_server(move |mut s| async move {
            write_sse_response(&mut s, body).await;
            let _ = s.shutdown().await;
        })
        .await;

        let http = reqwest::Client::new();
        let mut stream = subscribe(http, &url, "k", "topic", 0).await.unwrap();
        let item = stream.next().await.unwrap();
        match item {
            Err(SseError::BadEvent(msg)) => {
                assert!(
                    msg.contains("not json"),
                    "BadEvent must include the offending payload for diagnosis; got: {msg}"
                );
            }
            other => panic!("expected SseError::BadEvent, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn subscribe_surfaces_non_2xx_connect_as_connect_error_with_bounded_body() {
        let big_body = "x".repeat(10_000);
        let response = format!(
            "HTTP/1.1 503 Service Unavailable\r\nContent-Length: {}\r\n\r\n{}",
            big_body.len(),
            big_body
        );
        let (url, _done) = spawn_sse_server(move |mut s| async move {
            s.write_all(response.as_bytes()).await.unwrap();
            s.flush().await.unwrap();
            let _ = s.shutdown().await;
        })
        .await;

        let http = reqwest::Client::new();
        // `unwrap_err` would need KatulongEventStream: Debug; manual
        // match avoids that constraint.
        let err = match subscribe(http, &url, "k", "topic", 0).await {
            Ok(_) => panic!("expected a Connect error from a 503 response"),
            Err(e) => e,
        };
        match err {
            SseError::Connect { status, body } => {
                assert_eq!(status, 503);
                assert!(
                    body.len() <= CONNECT_ERROR_BODY_CAP + 4, // + room for "…" ellipsis
                    "connect error body must be capped; got {} bytes",
                    body.len()
                );
                assert!(body.ends_with('…'), "truncation marker present");
            }
            other => panic!("expected SseError::Connect, got {other:?}"),
        }
    }

    #[test]
    fn sub_url_encodes_slash_in_topic_to_single_path_segment() {
        // The katulong broker uses `/` inside topic names; the URL
        // path treats one `/` as a segment separator. Percent-
        // encoding to `%2F` keeps the whole topic one segment.
        let url = sub_url("https://k.example", "claude/uuid-abc", 0);
        assert_eq!(url, "https://k.example/sub/claude%2Fuuid-abc?fromSeq=0");
    }

    #[test]
    fn sub_url_passes_from_seq_through() {
        let url = sub_url("https://k.example/", "topic", 42);
        assert!(url.ends_with("/sub/topic?fromSeq=42"));
    }

    #[test]
    fn sub_url_strips_trailing_slash_on_base() {
        // Operator's RemoteConfig might come with or without a
        // trailing slash; double slashes confuse some intermediaries.
        let with = sub_url("https://k.example/", "t", 0);
        let without = sub_url("https://k.example", "t", 0);
        assert_eq!(with, without);
    }

    // ── round-1 review-fix coverage ─────────────────────────────────

    #[tokio::test]
    async fn event_with_no_session_field_lands_as_none() {
        // Pins the `Option<String>` + `#[serde(default)]` contract on
        // `KatulongEvent::session`. Topics like `observations/activity`
        // emit project-/topic-level events with no session id.
        let payload = r#"{"seq":1,"event":"project-meta","timestamp":"2026-05-26T10:00:00Z"}"#;
        let body = format!("data: {payload}\n\n");
        let (url, _done) = spawn_sse_server(move |mut s| async move {
            write_sse_response(&mut s, &body).await;
            let _ = s.shutdown().await;
        })
        .await;

        let http = reqwest::Client::new();
        let mut stream = subscribe(http, &url, "k", "observations/activity", 0)
            .await
            .unwrap();
        let evt = stream.next().await.unwrap().unwrap();
        assert_eq!(evt.seq, 1);
        assert!(
            evt.session.is_none(),
            "topic-level events must surface session = None, not crash on missing field"
        );
    }

    #[tokio::test]
    async fn multi_line_data_concatenates_with_newline() {
        // SSE spec: multiple `data:` lines in one event concatenate
        // with `\n` between them. Pin the contract — the parser
        // explicitly takes the trouble to implement this and the
        // module doc-comment promises it at the wire-shape example.
        let body = "data: {\"seq\":7,\"event\":\"split\",\n\
                    data: \"timestamp\":\"2026-05-26T10:00:00Z\",\n\
                    data: \"session\":\"sess-x\"}\n\n";
        let (url, _done) = spawn_sse_server(move |mut s| async move {
            write_sse_response(&mut s, body).await;
            let _ = s.shutdown().await;
        })
        .await;

        let http = reqwest::Client::new();
        let mut stream = subscribe(http, &url, "k", "topic", 0).await.unwrap();
        let evt = stream.next().await.unwrap().unwrap();
        assert_eq!(
            evt.seq, 7,
            "multi-line data: must concatenate to one valid JSON document"
        );
        assert_eq!(evt.session.as_deref(), Some("sess-x"));
    }

    #[tokio::test]
    async fn line_buffer_cap_surfaces_line_too_long_error() {
        // Hostile / buggy upstream streams unbounded bytes with no
        // newline. The cap defends against OOM; the error variant
        // lets the consumer log + reconnect.
        //
        // Write enough garbage bytes (no `\n`) to push past
        // LINE_BUF_CAP, then close. Pin that the stream surfaces
        // `LineTooLong` rather than panicking or growing forever.
        let garbage_len = LINE_BUF_CAP + 1024;
        let body = "x".repeat(garbage_len);
        let (url, _done) = spawn_sse_server(move |mut s| async move {
            write_sse_response(&mut s, &body).await;
            let _ = s.shutdown().await;
        })
        .await;

        let http = reqwest::Client::new();
        let mut stream = subscribe(http, &url, "k", "topic", 0).await.unwrap();
        let item = stream
            .next()
            .await
            .expect("must yield an item, not Pending");
        match item {
            Err(SseError::LineTooLong { limit }) => {
                assert_eq!(limit, LINE_BUF_CAP);
            }
            other => panic!("expected LineTooLong, got {other:?}"),
        }
        // Stream is terminated after the cap-trip per the variant's
        // doc — pin the contract so a regression that kept the
        // stream alive (consuming arbitrary mid-event bytes) fails
        // here, not in production.
        assert!(
            stream.next().await.is_none(),
            "stream must terminate after LineTooLong per the variant's doc comment"
        );
    }

    #[tokio::test]
    async fn event_buffer_cap_surfaces_event_too_large_error() {
        // Same defense, different vector: many `data:` lines that
        // total over EVENT_BUF_CAP but each line is well under
        // LINE_BUF_CAP. Keep individual lines tiny (~1 KiB) so the
        // line-cap can't trip first regardless of how reqwest chunks
        // the body off the wire — the line-cap is per-line not
        // per-chunk, but tiny lines remove any ambiguity.
        const LINE_PAYLOAD: usize = 1024;
        let line = "data: ".to_string() + &"a".repeat(LINE_PAYLOAD) + "\n";
        let lines_needed = (EVENT_BUF_CAP / LINE_PAYLOAD) + 2; // overshoot
        let body = line.repeat(lines_needed);
        let (url, _done) = spawn_sse_server(move |mut s| async move {
            write_sse_response(&mut s, &body).await;
            let _ = s.shutdown().await;
        })
        .await;

        let http = reqwest::Client::new();
        let mut stream = subscribe(http, &url, "k", "topic", 0).await.unwrap();
        let item = stream.next().await.expect("must yield an item");
        match item {
            Err(SseError::EventTooLarge { limit }) => {
                assert_eq!(limit, EVENT_BUF_CAP);
            }
            other => panic!("expected EventTooLarge, got {other:?}"),
        }
        // Stream is terminated after the cap-trip; the next poll
        // returns None rather than continuing to read garbage.
        assert!(
            stream.next().await.is_none(),
            "stream must terminate after EventTooLarge per the variant's doc comment"
        );
    }

    #[test]
    fn truncate_at_char_boundary_does_not_panic_on_multibyte_split() {
        // The HIGH from round 1: `String::truncate` panics if the
        // index isn't on a char boundary. Pin the safe wrapper.
        //
        // Build a string where byte position `cap` lands inside a
        // 2-byte char (`ö` = 2 bytes: 0xC3 0xB6).
        let mut s = "a".repeat(1023) + "ö" + &"b".repeat(100);
        truncate_at_char_boundary(&mut s, 1024);
        // Should have truncated to 1023 (just before the ö) to stay
        // on a char boundary, not panicked.
        assert_eq!(s.len(), 1023);
        assert!(s.chars().all(|c| c == 'a'));
    }

    #[test]
    fn truncate_at_char_boundary_noop_when_already_short_enough() {
        let mut s = "hello".to_string();
        truncate_at_char_boundary(&mut s, 1024);
        assert_eq!(s, "hello");
    }

    #[tokio::test]
    async fn body_read_error_mid_stream_surfaces_as_body_variant() {
        // Pin that `SseError::Body` is constructed when the upstream
        // closes mid-event. Without coverage, a regression that
        // silently swallowed body errors (treating them as clean
        // EOF) would slip through and the bridge worker — which
        // distinguishes Body from Transport for retry-class
        // decisions — would lose information.
        //
        // Approach: advertise Content-Length much larger than what
        // we actually send, then close. reqwest's body stream
        // surfaces the premature EOF as an error.
        let response = "HTTP/1.1 200 OK\r\n\
                        Content-Type: text/event-stream\r\n\
                        Content-Length: 1000000\r\n\
                        \r\n\
                        data: {\"se";
        let (url, _done) = spawn_sse_server(move |mut s| async move {
            s.write_all(response.as_bytes()).await.unwrap();
            s.flush().await.unwrap();
            // Drop without writing the rest — reqwest should error
            // on the next body read since Content-Length promised
            // 1 MB and we sent ~10 bytes.
            drop(s);
        })
        .await;

        let http = reqwest::Client::new();
        let mut stream = subscribe(http, &url, "k", "topic", 0).await.unwrap();
        let item = stream
            .next()
            .await
            .expect("must yield an error item, not clean EOF");
        match item {
            Err(SseError::Body(msg)) => {
                assert!(
                    !msg.is_empty(),
                    "Body variant must carry diagnostic text from the underlying reqwest error"
                );
            }
            other => panic!("expected SseError::Body, got {other:?}"),
        }
        // After a body error the stream is terminated.
        assert!(
            stream.next().await.is_none(),
            "stream must terminate after Body error"
        );
    }
}
