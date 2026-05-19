//! Async HTTP client for katulong's session API.
//!
//! Sibling of [`crate::KatulongClient`] (sync, curl-based) and
//! [`crate::KatulongAttachClient`] (async, WebSocket attach). Adds:
//!
//! - **Async/await** so axum handlers don't need to `spawn_blocking`
//!   around the sync curl path
//! - **Per-call response body cap** (streamed, so we abort *before*
//!   buffering the full payload) — closes sipag #527, which is the
//!   load-bearing security driver for this module
//!
//! The wire format (URLs, JSON shapes, session-id validation) is
//! shared with [`crate::KatulongClient`] via the pure-function helpers
//! in [`crate::http`]. **Don't fork those** — every URL change must
//! land in one place.
//!
//! ## When to use which client
//!
//! | Need | Use |
//! |---|---|
//! | sync code path (CLI sync entrypoints) | [`crate::KatulongClient`] |
//! | async HTTP from an axum handler | [`KatulongAsyncClient`] (this) |
//! | sustained interaction (input + waits) | [`crate::KatulongAttachClient`] |

use crate::http::{
    exec_url, generate_dispatch_session_name, kill_url, output_lines_url, sessions_url, status_url,
    RemoteConfig, TmuxSession, TmuxSessionStatus,
};
use anyhow::{Context, Result};
use futures_util::StreamExt;
use reqwest::{Client, Response, StatusCode};
use serde::de::DeserializeOwned;
use std::time::Duration;
use thiserror::Error;

/// Default response-body cap: 1 MiB. Sized for session-lifecycle JSON
/// (sessions list, status, output-lines) which is well under 100 KB
/// in practice. The cap is defense-in-depth: a misbehaving or
/// malicious katulong streaming an unbounded body must not OOM the
/// sipag process. See sipag issue #527.
pub const DEFAULT_BODY_CAP: usize = 1024 * 1024;

/// Larger cap for endpoints that legitimately return bulkier bodies
/// (Claude transcript JSONL). 10 MiB accommodates long sessions while
/// still bounding worst-case memory. Same defense-in-depth rationale
/// as [`DEFAULT_BODY_CAP`].
pub const TRANSCRIPT_BODY_CAP: usize = 10 * 1024 * 1024;

/// Typed failure modes for [`KatulongAsyncClient`]. Lets callers
/// react to specific failure shapes (e.g., a `BadSessionId` is a
/// trust-boundary violation worth alerting on, while `Http(404)` is
/// often just a stale session id and can be retried).
#[derive(Debug, Error)]
pub enum KatulongAsyncError {
    /// Network failure, connection refused, TLS, etc. — anything the
    /// reqwest transport surfaces before we get a status code back.
    #[error("transport error: {0}")]
    Transport(#[from] reqwest::Error),

    /// HTTP non-2xx response. `body` is already truncated to whatever
    /// the body cap allowed and trimmed; the full body is *not*
    /// preserved (would defeat the cap).
    #[error("HTTP {status}: {body}")]
    Http { status: StatusCode, body: String },

    /// Response body exceeded the per-call cap. The request was
    /// aborted before the full body was buffered — load on the
    /// process is bounded to `cap + one chunk`.
    #[error("response body exceeded {cap} byte cap (sipag #527)")]
    BodyTooLarge { cap: usize },

    /// Response body parsed but JSON was invalid for the requested
    /// type. Distinguishes "katulong is up but sent garbage" from
    /// "katulong is down" — the former is a katulong bug.
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),

    /// A session id returned by katulong didn't pass
    /// [`crate::http::is_valid_session_id`]. Indicates a compromised
    /// or misbehaving katulong attempting path traversal / injection
    /// via the id field. See `TmuxSession::validate_id` for context.
    #[error("invalid session id from upstream: {0}")]
    BadSessionId(String),
}

/// Convenience alias. Used in this module; callers may or may not
/// prefer to spell out the full `Result<T, KatulongAsyncError>`.
pub type AsyncResult<T> = std::result::Result<T, KatulongAsyncError>;

/// Async sibling of [`crate::KatulongClient`]. Holds a
/// [`reqwest::Client`] and the remote credentials; every call applies
/// a streaming body cap (see [`DEFAULT_BODY_CAP`] /
/// [`TRANSCRIPT_BODY_CAP`]) before deserialization.
///
/// Cheap to clone — the inner reqwest::Client shares its connection
/// pool across clones.
#[derive(Clone)]
pub struct KatulongAsyncClient {
    inner: Client,
    url: String,
    api_key: String,
}

impl KatulongAsyncClient {
    /// Load from `~/.katulong/remote.json` and build a fresh
    /// reqwest::Client with sane defaults (60s timeout, library UA).
    pub fn from_remote_json() -> Result<Self> {
        let cfg = RemoteConfig::load()?;
        Self::new(cfg.url, cfg.api_key)
    }

    /// Build with explicit URL + API key, using an internal
    /// reqwest::Client. For callers that need to share an existing
    /// client (e.g. sipag's serve, which keeps a Mozilla-shaped UA
    /// to avoid Cloudflare BIC challenges), see [`Self::with_client`].
    pub fn new(url: String, api_key: String) -> Result<Self> {
        let inner = Client::builder()
            .timeout(Duration::from_secs(60))
            .user_agent(concat!("katulong-client/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("failed to build reqwest::Client for KatulongAsyncClient")?;
        Ok(Self::with_client(inner, url, api_key))
    }

    /// Bring-your-own reqwest::Client. The caller is responsible for
    /// timeout / UA / connection-pool config. Used by sipag's serve
    /// to share its tunnel-friendly Mozilla UA across all outbound
    /// HTTP.
    pub fn with_client(inner: Client, url: String, api_key: String) -> Self {
        let url = url.trim_end_matches('/').to_string();
        Self {
            inner,
            url,
            api_key,
        }
    }

    /// Base URL the client targets (no trailing slash).
    pub fn url(&self) -> &str {
        &self.url
    }

    // ── session lifecycle ───────────────────────────────────────────

    /// `POST /sessions` — create a session, or return the existing
    /// one with the same name. The server returns 201 with `{id, name}`
    /// on create and 409 with `{error}` on conflict; on conflict this
    /// method falls back to [`Self::list_sessions`] to recover the
    /// existing id, so the call is idempotent.
    ///
    /// Mirrors the sync [`crate::KatulongClient::create_session`].
    pub async fn create_session(&self, name: &str) -> AsyncResult<TmuxSession> {
        let url = sessions_url(&self.url);
        let resp = self
            .inner
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&serde_json::json!({ "name": name }))
            .send()
            .await?;
        let status = resp.status();
        let bytes = bytes_capped(resp, DEFAULT_BODY_CAP).await?;
        match status.as_u16() {
            200 | 201 => {
                let session: TmuxSession = serde_json::from_slice(&bytes)?;
                session
                    .validate_id()
                    .map_err(|e| KatulongAsyncError::BadSessionId(e.to_string()))?;
                Ok(session)
            }
            409 => {
                // Already exists — fall back to list-and-find. Same
                // recovery path as the sync sibling.
                self.list_sessions()
                    .await?
                    .into_iter()
                    .find(|s| s.name == name)
                    .ok_or_else(|| KatulongAsyncError::Http {
                        status: StatusCode::CONFLICT,
                        body: format!("session '{name}' returned 409 but list lookup missed it"),
                    })
            }
            _ => Err(KatulongAsyncError::Http {
                status,
                body: String::from_utf8_lossy(&bytes).trim().to_string(),
            }),
        }
    }

    /// Create a fresh session with an opaque dispatch-shaped name.
    /// See [`generate_dispatch_session_name`] for the naming rationale
    /// and the strict layer coupling note about why dispatch names are
    /// not under sipag/katulong negotiation.
    pub async fn create_dispatch_session(&self) -> AsyncResult<TmuxSession> {
        let name = generate_dispatch_session_name();
        self.create_session(&name).await
    }

    /// `GET /sessions` — list all sessions on this katulong host.
    /// Returned ids are NOT validated by this method individually;
    /// the single internal caller ([`Self::create_session`]'s 409
    /// fallback) validates the one id it picks.
    pub async fn list_sessions(&self) -> AsyncResult<Vec<TmuxSession>> {
        self.get_capped::<Vec<TmuxSession>>(&sessions_url(&self.url), DEFAULT_BODY_CAP)
            .await
    }

    /// `POST /sessions/by-id/{id}/exec` — send a line-oriented input.
    /// Katulong appends `\r` server-side. For sustained interactive
    /// input prefer [`crate::KatulongAttachClient`].
    pub async fn exec_session(&self, id: &str, input: &str) -> AsyncResult<()> {
        let url = exec_url(&self.url, id);
        let resp = self
            .inner
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&serde_json::json!({ "input": input }))
            .send()
            .await?;
        let status = resp.status();
        let bytes = bytes_capped(resp, DEFAULT_BODY_CAP).await?;
        if status.is_success() {
            Ok(())
        } else {
            Err(KatulongAsyncError::Http {
                status,
                body: String::from_utf8_lossy(&bytes).trim().to_string(),
            })
        }
    }

    /// `GET /sessions/by-id/{id}/status` — session-level metadata
    /// (alive, has-child-processes, ...). See the sync sibling for
    /// the warning about `agent.running` as a "task in flight" signal.
    pub async fn session_status(&self, id: &str) -> AsyncResult<TmuxSessionStatus> {
        self.get_capped::<TmuxSessionStatus>(&status_url(&self.url, id), DEFAULT_BODY_CAP)
            .await
    }

    /// `GET /sessions/by-id/{id}/output?lines=N` — last N pane lines
    /// as plain text (no escapes). Returns the `data` field of the
    /// response, or empty string when the field is missing (mirrors
    /// the sync sibling's drop-on-error semantics).
    ///
    /// For sustained pattern-based observation, prefer the rolling
    /// buffer on [`crate::KatulongAttachClient`].
    pub async fn session_output_lines(&self, id: &str, n: u32) -> AsyncResult<String> {
        let value: serde_json::Value = self
            .get_capped(&output_lines_url(&self.url, id, n), DEFAULT_BODY_CAP)
            .await?;
        Ok(value
            .get("data")
            .and_then(|d| d.as_str())
            .unwrap_or("")
            .to_string())
    }

    /// `DELETE /sessions/by-id/{id}` — kill a session.
    pub async fn kill_session(&self, id: &str) -> AsyncResult<()> {
        let url = kill_url(&self.url, id);
        let resp = self
            .inner
            .delete(&url)
            .bearer_auth(&self.api_key)
            .send()
            .await?;
        let status = resp.status();
        let bytes = bytes_capped(resp, DEFAULT_BODY_CAP).await?;
        if status.is_success() {
            Ok(())
        } else {
            Err(KatulongAsyncError::Http {
                status,
                body: String::from_utf8_lossy(&bytes).trim().to_string(),
            })
        }
    }

    // ── generic GET helpers ─────────────────────────────────────────

    /// GET + bearer-auth + body-cap + JSON deserialize. Exposed
    /// publicly because some callers (transcript fetch, future
    /// custom endpoints) need to pass [`TRANSCRIPT_BODY_CAP`] instead
    /// of the default.
    pub async fn get_capped<T: DeserializeOwned>(&self, url: &str, cap: usize) -> AsyncResult<T> {
        let resp = self
            .inner
            .get(url)
            .bearer_auth(&self.api_key)
            .send()
            .await?;
        let status = resp.status();
        let bytes = bytes_capped(resp, cap).await?;
        if status.is_success() {
            Ok(serde_json::from_slice(&bytes)?)
        } else {
            Err(KatulongAsyncError::Http {
                status,
                body: String::from_utf8_lossy(&bytes).trim().to_string(),
            })
        }
    }

    /// GET + bearer-auth + body-cap, returning the raw body bytes
    /// AND the response status + headers. Use when the caller needs
    /// to pass katulong bytes through verbatim (sipag's
    /// `board::proxy_get` does this for the transparent-passthrough
    /// endpoints). The body cap still applies — that's the whole
    /// point of routing through this method instead of raw reqwest.
    ///
    /// Returns `(status, content_type, bytes)`. The content type is
    /// extracted before the body stream is consumed so the caller
    /// can re-emit it without storing the full Response.
    pub async fn get_bytes_capped(
        &self,
        url: &str,
        cap: usize,
    ) -> AsyncResult<(StatusCode, Option<String>, Vec<u8>)> {
        let resp = self
            .inner
            .get(url)
            .bearer_auth(&self.api_key)
            .send()
            .await?;
        let status = resp.status();
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let bytes = bytes_capped(resp, cap).await?;
        Ok((status, content_type, bytes))
    }
}

/// Stream a reqwest response into memory, aborting if the accumulated
/// body exceeds `cap` bytes. The check runs after each chunk, so the
/// upper bound on memory is `cap + max_chunk_size` (well bounded —
/// reqwest's default chunk is single-digit KB to a few MB; pathological
/// chunking would still be bounded by the server's frame size, never
/// "the full body before we noticed").
///
/// Exposed publicly so test harnesses and migration code can invoke
/// the cap on responses obtained outside the [`KatulongAsyncClient`]
/// methods.
pub async fn bytes_capped(resp: Response, cap: usize) -> AsyncResult<Vec<u8>> {
    let mut acc: Vec<u8> = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if acc.len() + chunk.len() > cap {
            return Err(KatulongAsyncError::BodyTooLarge { cap });
        }
        acc.extend_from_slice(&chunk);
    }
    Ok(acc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::get, Router};
    use std::net::SocketAddr;

    /// Spin up an axum server on an ephemeral port serving the given
    /// router. Returns the bound address; the server task lives for
    /// the duration of the test (tokio drops it when the test exits).
    async fn spawn_test_server(app: Router) -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        addr
    }

    #[tokio::test]
    async fn bytes_capped_accepts_body_within_cap() {
        // 512-byte body, 1 KiB cap — should succeed.
        let body = vec![0u8; 512];
        let app = Router::new().route("/data", get(move || async move { body.clone() }));
        let addr = spawn_test_server(app).await;

        let resp = reqwest::get(format!("http://{addr}/data")).await.unwrap();
        let out = bytes_capped(resp, 1024).await.unwrap();
        assert_eq!(out.len(), 512);
    }

    #[tokio::test]
    async fn bytes_capped_rejects_oversized_body() {
        // 2 KiB body, 1 KiB cap — must reject with BodyTooLarge.
        // This is the load-bearing test for sipag #527: a malicious
        // katulong streaming a too-large body MUST be rejected before
        // it can OOM the process.
        let body = vec![0u8; 2048];
        let app = Router::new().route("/data", get(move || async move { body.clone() }));
        let addr = spawn_test_server(app).await;

        let resp = reqwest::get(format!("http://{addr}/data")).await.unwrap();
        let err = bytes_capped(resp, 1024).await.unwrap_err();
        match err {
            KatulongAsyncError::BodyTooLarge { cap } => assert_eq!(cap, 1024),
            other => panic!("expected BodyTooLarge, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn bytes_capped_at_exact_cap_succeeds() {
        // Boundary: body length == cap should succeed (the check is
        // strictly greater-than). Pins the inclusive semantics so a
        // future refactor that flips to `>=` doesn't silently break
        // legitimate at-the-line responses.
        let body = vec![0u8; 1024];
        let app = Router::new().route("/data", get(move || async move { body.clone() }));
        let addr = spawn_test_server(app).await;

        let resp = reqwest::get(format!("http://{addr}/data")).await.unwrap();
        let out = bytes_capped(resp, 1024).await.unwrap();
        assert_eq!(out.len(), 1024);
    }

    #[tokio::test]
    async fn get_capped_parses_json_body() {
        // End-to-end happy path: client builds, makes a GET, body
        // fits cap, JSON parses cleanly.
        #[derive(serde::Deserialize, Debug, PartialEq)]
        struct Payload {
            name: String,
            count: u32,
        }
        let app = Router::new().route(
            "/api",
            get(|| async { axum::Json(serde_json::json!({"name":"foo","count":7})) }),
        );
        let addr = spawn_test_server(app).await;

        let client =
            KatulongAsyncClient::new(format!("http://{addr}"), "test-key".to_string()).unwrap();
        let got: Payload = client
            .get_capped(&format!("http://{addr}/api"), DEFAULT_BODY_CAP)
            .await
            .unwrap();
        assert_eq!(
            got,
            Payload {
                name: "foo".to_string(),
                count: 7
            }
        );
    }

    #[tokio::test]
    async fn get_capped_returns_http_error_on_non_2xx() {
        // Server returns 500 with a small body — the client should
        // surface it as Http{status, body}, NOT swallow it as a parse
        // error.
        let app = Router::new().route(
            "/api",
            get(|| async { (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "boom") }),
        );
        let addr = spawn_test_server(app).await;

        let client =
            KatulongAsyncClient::new(format!("http://{addr}"), "test-key".to_string()).unwrap();
        let err = client
            .get_capped::<serde_json::Value>(&format!("http://{addr}/api"), DEFAULT_BODY_CAP)
            .await
            .unwrap_err();
        match err {
            KatulongAsyncError::Http { status, body } => {
                assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
                assert_eq!(body, "boom");
            }
            other => panic!("expected Http, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn client_strips_trailing_slash_in_url() {
        let c =
            KatulongAsyncClient::new("https://example.com/".to_string(), "k".to_string()).unwrap();
        assert_eq!(c.url(), "https://example.com");
    }
}
