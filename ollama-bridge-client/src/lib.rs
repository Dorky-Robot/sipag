//! Rust client for [`dorky-robot/ollama-bridge`](https://github.com/Dorky-Robot/ollama-bridge).
//!
//! The bridge is an Elixir queue + bearer-auth daemon that sits in
//! front of a local `ollama serve` on a GPU host. Solves two real
//! problems `ollama serve` has out of the box:
//!
//! 1. **No auth.** Exposing ollama across machines requires bearer
//!    auth somewhere; the bridge does it before parsing any body so
//!    unauthorized callers can't make the bridge read megabyte-sized
//!    JSON.
//! 2. **No concurrency control.** Three callers hitting one GPU host
//!    concurrently unloads/reloads `gemma4:31b` between requests
//!    (47 GB on/off the GPU each time). The bridge serializes through
//!    a single-worker queue with sha256-based 60s dedup so the GPU
//!    actually keeps up.
//!
//! Sipag never talks to ollama directly — same strict-layer-coupling
//! discipline as `Claude → katulong → sipag`. See
//! `[[feedback-strict-layer-coupling]]` and `[[reference-ollama-bridge]]`
//! in the sipag memory directory.
//!
//! ## Wire shape (read the bridge's README for the canonical spec)
//!
//! - `POST /enqueue` with `{endpoint, body}` → `{hash, status}`.
//!   `endpoint` is `/api/chat` / `/api/generate` / `/api/embed`;
//!   `body` is the same JSON ollama expects (minus `stream`).
//! - `GET /jobs/:hash` → full job view with `result` when
//!   `status == "done"` or `error` when `status == "error"`.
//! - `GET /api/tags` / `/api/show` / `/api/ps` — read-only
//!   pass-through to upstream ollama (for probes).
//! - `POST /api/chat` / `/api/generate` / `/api/embed` — **refused**
//!   with 409 (forces all generation through the queue). The client
//!   never hits these directly.
//!
//! ## When to use which method
//!
//! | Need | Use |
//! |---|---|
//! | "submit a chat completion, give me the result" | [`OllamaBridgeClient::submit_and_wait`] (polls for you) |
//! | "fire and forget, I'll poll the hash myself later" | [`OllamaBridgeClient::enqueue`] + [`OllamaBridgeClient::poll`] |
//! | "is the bridge up? is gemma4 loaded?" | [`OllamaBridgeClient::tags`] (lightweight probe) |
//! | model metadata | [`OllamaBridgeClient::show`] |
//! | currently-loaded models | [`OllamaBridgeClient::ps`] |

use anyhow::{Context, Result};
use futures_util::StreamExt;
use reqwest::{Client, Response, StatusCode};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;
use thiserror::Error;
use tracing::debug;

/// Default response-body cap: 1 MiB. Job-view responses are
/// well-bounded (small JSON envelopes), but defensive: a misbehaving
/// or compromised bridge must not OOM the sipag process. Matches the
/// pattern in `katulong-client::async_http::DEFAULT_BODY_CAP`.
pub const DEFAULT_BODY_CAP: usize = 1024 * 1024;

/// Larger cap for endpoints that return bulkier bodies — currently
/// just the pass-through probes (`/api/tags` can list dozens of
/// models; `/api/ps` can list multiple loaded models with metadata).
/// 10 MiB matches `katulong-client::async_http::TRANSCRIPT_BODY_CAP`'s
/// rationale: legitimately bigger than the lifecycle JSON, but still
/// bounded.
pub const PROBE_BODY_CAP: usize = 10 * 1024 * 1024;

/// Default per-poll interval when waiting for a job to finish.
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(500);

// ── connection config ───────────────────────────────────────────────

/// Remote connection config — URL of the bridge daemon + bearer
/// token. Mirrors `katulong-client::RemoteConfig` in shape.
///
/// `Debug` is hand-implemented so the bearer token stays out of logs.
#[derive(Clone, Serialize, Deserialize)]
pub struct RemoteConfig {
    pub url: String,
    #[serde(rename = "apiKey")]
    pub api_key: String,
}

impl std::fmt::Debug for RemoteConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteConfig")
            .field("url", &self.url)
            .field("api_key", &"<redacted>")
            .finish()
    }
}

impl RemoteConfig {
    /// Load from `~/.ollama-bridge/remote.json`.
    pub fn load() -> Result<Self> {
        let home = std::env::var("HOME").context("HOME not set")?;
        let path = std::path::PathBuf::from(home)
            .join(".ollama-bridge")
            .join("remote.json");
        Self::load_from(&path)
    }

    /// Load from a specific path. Rejects empty `url` / `apiKey` so a
    /// misconfigured file fails fast with a clear message instead of
    /// surfacing as a confusing 401 / DNS error at request time.
    pub fn load_from(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let config: Self = serde_json::from_str(&content)
            .with_context(|| format!("invalid JSON in {}", path.display()))?;
        if config.url.is_empty() {
            anyhow::bail!("'url' is empty in {}", path.display());
        }
        if config.api_key.is_empty() {
            anyhow::bail!("'apiKey' is empty in {}", path.display());
        }
        Ok(config)
    }
}

// ── wire types ──────────────────────────────────────────────────────

/// One of the three endpoints the bridge queues. The bridge refuses
/// `/enqueue` requests with other endpoints (422
/// `invalid_endpoint`); this enum constrains callers at compile time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum JobEndpoint {
    #[serde(rename = "/api/chat")]
    Chat,
    #[serde(rename = "/api/generate")]
    Generate,
    #[serde(rename = "/api/embed")]
    Embed,
}

impl JobEndpoint {
    /// Wire-format string (`"/api/chat"` etc.). Used internally for
    /// log lines and the JSON `endpoint` field.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "/api/chat",
            Self::Generate => "/api/generate",
            Self::Embed => "/api/embed",
        }
    }
}

/// Submission to `POST /enqueue`. The `body` is the same JSON ollama
/// would have received directly — caller is responsible for shape.
#[derive(Debug, Clone, Serialize)]
pub struct EnqueueRequest {
    pub endpoint: JobEndpoint,
    pub body: serde_json::Value,
}

/// Successful `POST /enqueue` response. `status` is the bridge's
/// current view of the job — usually `queued`, but can be `running`
/// (if a same-hash job is mid-execution and the dedup window matched)
/// or `done` (if the dedup window cached a finished result).
#[derive(Debug, Clone, Deserialize)]
pub struct EnqueueResponse {
    pub hash: String,
    pub status: JobStatus,
}

/// One job in the bridge's queue. Same shape as `GET /jobs/:hash`
/// (plus `EnqueueResponse` reuses the `status` field as a flat
/// transition target). `result` is present iff `status == Done`;
/// `error` is present iff `status == Error`.
#[derive(Debug, Clone, Deserialize)]
pub struct JobView {
    pub hash: String,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub body: Option<serde_json::Value>,
    pub status: JobStatus,
    #[serde(default)]
    pub enqueued_at: Option<u64>,
    #[serde(default)]
    pub started_at: Option<u64>,
    #[serde(default)]
    pub finished_at: Option<u64>,
    #[serde(default)]
    pub result: Option<serde_json::Value>,
    #[serde(default)]
    pub error: Option<serde_json::Value>,
    #[serde(default)]
    pub attempts: u32,
}

/// Job lifecycle. The bridge transitions strictly
/// `Queued → Running → Done | Error`. Dedup may cause an enqueue to
/// return any of these as the initial status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Queued,
    Running,
    Done,
    Error,
}

impl JobStatus {
    /// `true` for `Done` or `Error` — the bridge will not transition
    /// past these. Polling can stop.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Error)
    }
}

// ── errors ──────────────────────────────────────────────────────────

/// Typed failure modes. Mirrors the structured-error pattern in
/// `katulong-client::async_http::KatulongAsyncError` so call sites
/// can match by variant rather than parsing strings.
#[derive(Debug, Error)]
pub enum BridgeError {
    /// Network failure, TLS, connection refused — anything the
    /// reqwest transport surfaces before we get a status code back.
    #[error("transport error: {0}")]
    Transport(#[from] reqwest::Error),

    /// HTTP non-2xx response from the bridge. Already body-capped.
    #[error("HTTP {status}: {body}")]
    Http { status: StatusCode, body: String },

    /// Bridge's queue is full (`503` with `Retry-After`). The bridge
    /// suggests `retry_after_seconds` in the JSON body; if parsing
    /// fails we still capture the header in `retry_after`.
    #[error("queue full; retry after {retry_after:?} seconds")]
    QueueFull { retry_after: Option<u64> },

    /// `submit_and_wait` polled until `timeout` and the job hadn't
    /// reached a terminal state. The hash is preserved so the caller
    /// can poll again later (the job is still running on the
    /// bridge).
    #[error("job {hash} did not finish within {timeout:?}; still polling-eligible")]
    JobTimeout { hash: String, timeout: Duration },

    /// `submit_and_wait` returned a job whose status was `Error`.
    /// `error` is the bridge's structured error payload, opaque to
    /// this crate.
    #[error("job failed: {error}")]
    JobErrored {
        hash: String,
        error: serde_json::Value,
    },

    /// Response body exceeded the per-call cap. Same #527 hygiene as
    /// `katulong-client::async_http`.
    #[error("response body exceeded {cap} byte cap (sipag #527)")]
    BodyTooLarge { cap: usize },

    /// Response body parsed but JSON was invalid for the requested
    /// type. "Bridge is up but speaking garbage" — distinct from
    /// "bridge is down" (Transport).
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),

    /// A hash returned by the bridge didn't pass
    /// [`is_valid_hash`]. Indicates a compromised or misbehaving
    /// bridge.
    #[error("invalid hash from upstream: {0}")]
    BadHash(String),
}

pub type BridgeResult<T> = std::result::Result<T, BridgeError>;

/// Validate that a hash string matches the bridge's wire format
/// (64 lower-hex chars, `/^[a-f0-9]{64}$/`). Defense-in-depth before
/// interpolating into a URL path segment.
pub fn is_valid_hash(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

// ── client ──────────────────────────────────────────────────────────

/// HTTP client for the bridge. Holds a [`reqwest::Client`] + the
/// remote config; every call applies bearer auth and a streaming
/// body cap before deserialization.
///
/// Cheap to clone (reqwest::Client is Arc-internal).
#[derive(Clone)]
pub struct OllamaBridgeClient {
    inner: Client,
    url: String,
    api_key: String,
}

impl OllamaBridgeClient {
    /// Load from `~/.ollama-bridge/remote.json` and build a fresh
    /// reqwest::Client with sane defaults (60s timeout for enqueue/
    /// poll; this is NOT the timeout for the underlying ollama job,
    /// which can run for minutes — that's handled by
    /// [`Self::submit_and_wait`]'s own timeout).
    pub fn from_remote_json() -> Result<Self> {
        let cfg = RemoteConfig::load()?;
        Self::new(cfg.url, cfg.api_key)
    }

    /// Build with explicit URL + API key, using an internal
    /// reqwest::Client. For callers that need to share an existing
    /// client (e.g. sipag's serve, which keeps a Mozilla-shaped UA),
    /// see [`Self::with_client`].
    pub fn new(url: String, api_key: String) -> Result<Self> {
        let inner = Client::builder()
            .timeout(Duration::from_secs(60))
            .user_agent(concat!("ollama-bridge-client/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("failed to build reqwest::Client for OllamaBridgeClient")?;
        Ok(Self::with_client(inner, url, api_key))
    }

    /// Bring-your-own reqwest::Client. Useful when the caller has
    /// already configured a client with specific UA / timeout /
    /// connection-pool settings.
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

    // ── job submission + polling ────────────────────────────────────

    /// Submit a job. Returns the bridge's `{hash, status}` view
    /// immediately (no polling). The job runs in the bridge's
    /// background worker.
    ///
    /// `status` is most often `Queued`, but can be `Running` or
    /// `Done` if the dedup window (60s on `sha256({endpoint, body})`)
    /// matched a previous submission.
    pub async fn enqueue(
        &self,
        endpoint: JobEndpoint,
        body: serde_json::Value,
    ) -> BridgeResult<EnqueueResponse> {
        let url = format!("{}/enqueue", self.url);
        let req = EnqueueRequest { endpoint, body };
        let resp = self
            .inner
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&req)
            .send()
            .await?;
        let status = resp.status();
        let headers = resp.headers().clone();
        let bytes = bytes_capped(resp, DEFAULT_BODY_CAP).await?;
        if status.as_u16() == 503 {
            return Err(BridgeError::QueueFull {
                retry_after: parse_retry_after(&headers, &bytes),
            });
        }
        if !status.is_success() {
            return Err(BridgeError::Http {
                status,
                body: sanitize_error_body(&bytes),
            });
        }
        let parsed: EnqueueResponse = serde_json::from_slice(&bytes)?;
        if !is_valid_hash(&parsed.hash) {
            return Err(BridgeError::BadHash(parsed.hash));
        }
        debug!(hash = %parsed.hash, status = ?parsed.status, "bridge: enqueued");
        Ok(parsed)
    }

    /// Poll one job by hash. Returns the full [`JobView`] (which
    /// includes `result` when done or `error` when errored).
    pub async fn poll(&self, hash: &str) -> BridgeResult<JobView> {
        if !is_valid_hash(hash) {
            return Err(BridgeError::BadHash(hash.to_string()));
        }
        let url = format!("{}/jobs/{}", self.url, hash);
        let resp = self
            .inner
            .get(&url)
            .bearer_auth(&self.api_key)
            .send()
            .await?;
        let status = resp.status();
        let bytes = bytes_capped(resp, DEFAULT_BODY_CAP).await?;
        if !status.is_success() {
            return Err(BridgeError::Http {
                status,
                body: sanitize_error_body(&bytes),
            });
        }
        let view: JobView = serde_json::from_slice(&bytes)?;
        if !is_valid_hash(&view.hash) {
            return Err(BridgeError::BadHash(view.hash));
        }
        Ok(view)
    }

    /// Submit a job + poll until terminal (done/error) or the
    /// `timeout` elapses. Returns the `result` JSON on success;
    /// otherwise [`BridgeError::JobErrored`] or
    /// [`BridgeError::JobTimeout`] as appropriate.
    ///
    /// Polling cadence is fixed at 500ms (small enough to feel
    /// snappy on cached responses, sparse enough not to hammer the
    /// bridge). The hash is preserved across timeouts so the caller
    /// can retry [`Self::poll`] later if they want to.
    ///
    /// This is the convenience helper most callers want; the bare
    /// [`Self::enqueue`] + [`Self::poll`] split is for callers that
    /// need to track multiple jobs concurrently (e.g. a lens-worker
    /// scheduler fanning out N embed jobs).
    pub async fn submit_and_wait(
        &self,
        endpoint: JobEndpoint,
        body: serde_json::Value,
        timeout: Duration,
    ) -> BridgeResult<serde_json::Value> {
        let enq = self.enqueue(endpoint, body).await?;
        let hash = enq.hash.clone();

        // If dedup hit a finished job, the initial poll returns the
        // cached result without any waiting.
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let view = self.poll(&hash).await?;
            match view.status {
                JobStatus::Done => {
                    return Ok(view.result.unwrap_or(serde_json::Value::Null));
                }
                JobStatus::Error => {
                    return Err(BridgeError::JobErrored {
                        hash: view.hash,
                        error: view.error.unwrap_or(serde_json::Value::Null),
                    });
                }
                JobStatus::Queued | JobStatus::Running => {
                    if tokio::time::Instant::now() >= deadline {
                        return Err(BridgeError::JobTimeout { hash, timeout });
                    }
                    tokio::time::sleep(DEFAULT_POLL_INTERVAL).await;
                }
            }
        }
    }

    // ── pass-through probes ─────────────────────────────────────────

    /// `GET /api/tags` — list models known to the upstream ollama.
    /// Returned verbatim from ollama; this crate doesn't model the
    /// shape because it's only used for probes.
    pub async fn tags(&self) -> BridgeResult<serde_json::Value> {
        self.get_capped(&format!("{}/api/tags", self.url), PROBE_BODY_CAP)
            .await
    }

    /// `GET /api/show` — model metadata. Body shape is upstream
    /// ollama's; passed through.
    pub async fn show(&self) -> BridgeResult<serde_json::Value> {
        self.get_capped(&format!("{}/api/show", self.url), PROBE_BODY_CAP)
            .await
    }

    /// `GET /api/ps` — currently-loaded models.
    pub async fn ps(&self) -> BridgeResult<serde_json::Value> {
        self.get_capped(&format!("{}/api/ps", self.url), PROBE_BODY_CAP)
            .await
    }

    // ── helpers ─────────────────────────────────────────────────────

    /// GET + bearer auth + body cap + JSON parse. Used by the probe
    /// methods. Exposed publicly so callers can hit arbitrary
    /// bridge-side GET endpoints (the bridge only exposes the three
    /// probes today, but the door is open for future read-only
    /// endpoints the bridge might add).
    pub async fn get_capped(&self, url: &str, cap: usize) -> BridgeResult<serde_json::Value> {
        let resp = self
            .inner
            .get(url)
            .bearer_auth(&self.api_key)
            .send()
            .await?;
        let status = resp.status();
        let bytes = bytes_capped(resp, cap).await?;
        if !status.is_success() {
            return Err(BridgeError::Http {
                status,
                body: sanitize_error_body(&bytes),
            });
        }
        Ok(serde_json::from_slice(&bytes)?)
    }
}

/// Stream a reqwest response into memory, aborting if the accumulated
/// body exceeds `cap` bytes. Same shape as
/// `katulong-client::async_http::bytes_capped`.
pub async fn bytes_capped(resp: Response, cap: usize) -> BridgeResult<Vec<u8>> {
    let mut acc: Vec<u8> = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if acc.len() + chunk.len() > cap {
            return Err(BridgeError::BodyTooLarge { cap });
        }
        acc.extend_from_slice(&chunk);
    }
    Ok(acc)
}

/// Sanitize an upstream error body before storing it in
/// `BridgeError::Http { body }`. Filters ASCII control characters
/// (anything but `\n` and `\t`) and truncates at 1024 chars so a
/// compromised or misbehaving bridge can't smuggle ANSI escape
/// sequences into operator log/terminal output via `BridgeError`'s
/// `Display` impl. Matches the spirit of
/// `sipag/src/serve/upstream.rs::sanitize_upstream_body` (closer
/// to home), but keeps the helper local so this crate has no
/// dependency on sipag.
fn sanitize_error_body(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(bytes);
    s.trim()
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .take(1024)
        .collect()
}

/// Best-effort parse of `Retry-After` (either the HTTP header or the
/// `retry_after_seconds` field in the JSON body — the bridge sets
/// both, but only one needs to survive for the caller to respect
/// backpressure).
fn parse_retry_after(headers: &reqwest::header::HeaderMap, body: &[u8]) -> Option<u64> {
    // Prefer the structured JSON body if it parses.
    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) {
        if let Some(secs) = v.get("retry_after_seconds").and_then(|s| s.as_u64()) {
            return Some(secs);
        }
    }
    // Fall back to the `Retry-After` header.
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        routing::{get, post},
        Json, Router,
    };
    use serde_json::json;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    async fn spawn_test_server(app: Router) -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        addr
    }

    fn dummy_hash() -> &'static str {
        // 64 lower-hex chars — passes is_valid_hash.
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
    }

    #[test]
    fn is_valid_hash_accepts_64_lower_hex() {
        assert!(is_valid_hash(dummy_hash()));
    }

    #[test]
    fn is_valid_hash_rejects_wrong_length() {
        assert!(!is_valid_hash(""));
        assert!(!is_valid_hash(&"a".repeat(63)));
        assert!(!is_valid_hash(&"a".repeat(65)));
    }

    #[test]
    fn is_valid_hash_rejects_uppercase_and_non_hex() {
        // Uppercase is wire-illegal per the bridge's regex.
        assert!(!is_valid_hash(&"A".repeat(64)));
        // Path-traversal attempts rejected too.
        let mut traversal = "a".repeat(60);
        traversal.push_str("/../");
        assert!(!is_valid_hash(&traversal));
    }

    #[test]
    fn job_status_terminal_is_done_or_error() {
        assert!(JobStatus::Done.is_terminal());
        assert!(JobStatus::Error.is_terminal());
        assert!(!JobStatus::Queued.is_terminal());
        assert!(!JobStatus::Running.is_terminal());
    }

    #[test]
    fn job_endpoint_serializes_to_wire_path() {
        assert_eq!(
            serde_json::to_string(&JobEndpoint::Chat).unwrap(),
            "\"/api/chat\""
        );
        assert_eq!(
            serde_json::to_string(&JobEndpoint::Embed).unwrap(),
            "\"/api/embed\""
        );
    }

    #[test]
    fn remote_config_debug_redacts_api_key() {
        let cfg = RemoteConfig {
            url: "https://bridge.example".into(),
            api_key: "supersecret-token-do-not-log".into(),
        };
        let dbg = format!("{cfg:?}");
        assert!(dbg.contains("https://bridge.example"));
        assert!(!dbg.contains("supersecret-token-do-not-log"));
        assert!(dbg.contains("<redacted>"));
    }

    #[test]
    fn remote_config_load_from_rejects_empty_url() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remote.json");
        std::fs::write(&path, r#"{"url": "", "apiKey": "k"}"#).unwrap();
        let err = RemoteConfig::load_from(&path).unwrap_err().to_string();
        assert!(err.contains("'url' is empty"), "got: {err}");
    }

    #[test]
    fn remote_config_load_from_rejects_empty_api_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remote.json");
        std::fs::write(&path, r#"{"url": "https://x", "apiKey": ""}"#).unwrap();
        let err = RemoteConfig::load_from(&path).unwrap_err().to_string();
        assert!(err.contains("'apiKey' is empty"), "got: {err}");
    }

    #[tokio::test]
    async fn enqueue_happy_path() {
        let hash = dummy_hash().to_string();
        let h = hash.clone();
        let app = Router::new().route(
            "/enqueue",
            post(move || {
                let h = h.clone();
                async move { Json(json!({ "hash": h, "status": "queued" })) }
            }),
        );
        let addr = spawn_test_server(app).await;
        let client = OllamaBridgeClient::new(format!("http://{addr}"), "k".into()).unwrap();
        let resp = client
            .enqueue(JobEndpoint::Chat, json!({"model": "gemma4:latest"}))
            .await
            .unwrap();
        assert_eq!(resp.hash, hash);
        assert_eq!(resp.status, JobStatus::Queued);
    }

    #[tokio::test]
    async fn enqueue_queue_full_surfaces_retry_after() {
        let app = Router::new().route(
            "/enqueue",
            post(|| async {
                let body = Json(json!({
                    "error": "queue_full",
                    "retry_after_seconds": 60
                }));
                (
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    [(axum::http::header::RETRY_AFTER, "60")],
                    body,
                )
            }),
        );
        let addr = spawn_test_server(app).await;
        let client = OllamaBridgeClient::new(format!("http://{addr}"), "k".into()).unwrap();
        let err = client
            .enqueue(JobEndpoint::Chat, json!({}))
            .await
            .unwrap_err();
        match err {
            BridgeError::QueueFull { retry_after } => assert_eq!(retry_after, Some(60)),
            other => panic!("expected QueueFull, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn enqueue_rejects_bad_hash_from_upstream() {
        // Compromised bridge returns a hash that doesn't match the
        // wire format — surface as BadHash, never propagate.
        let app = Router::new().route(
            "/enqueue",
            post(|| async { Json(json!({ "hash": "../etc/passwd", "status": "queued" })) }),
        );
        let addr = spawn_test_server(app).await;
        let client = OllamaBridgeClient::new(format!("http://{addr}"), "k".into()).unwrap();
        let err = client
            .enqueue(JobEndpoint::Chat, json!({}))
            .await
            .unwrap_err();
        assert!(matches!(err, BridgeError::BadHash(_)));
    }

    #[tokio::test]
    async fn poll_rejects_bad_hash_locally() {
        // Bad hash on the caller side never hits the wire.
        let client = OllamaBridgeClient::new("http://127.0.0.1:1".into(), "k".into()).unwrap();
        let err = client.poll("not-a-hash").await.unwrap_err();
        assert!(matches!(err, BridgeError::BadHash(_)));
    }

    #[tokio::test]
    async fn submit_and_wait_returns_result_on_done() {
        // Bridge returns Queued on enqueue, then Done on the first
        // poll. submit_and_wait should return the result body
        // without timing out.
        let hash = dummy_hash().to_string();
        let h1 = hash.clone();
        let h2 = hash.clone();
        let app = Router::new()
            .route(
                "/enqueue",
                post(move || {
                    let h = h1.clone();
                    async move { Json(json!({ "hash": h, "status": "queued" })) }
                }),
            )
            .route(
                "/jobs/:hash",
                get(move || {
                    let h = h2.clone();
                    async move {
                        Json(json!({
                            "hash": h,
                            "status": "done",
                            "result": { "message": { "content": "hi" } }
                        }))
                    }
                }),
            );
        let addr = spawn_test_server(app).await;
        let client = OllamaBridgeClient::new(format!("http://{addr}"), "k".into()).unwrap();
        let result = client
            .submit_and_wait(
                JobEndpoint::Chat,
                json!({"model": "gemma4:latest"}),
                Duration::from_secs(5),
            )
            .await
            .unwrap();
        assert_eq!(result["message"]["content"], "hi");
    }

    #[tokio::test]
    async fn submit_and_wait_surfaces_job_error() {
        // Bridge returns Error status on the first poll.
        let hash = dummy_hash().to_string();
        let h1 = hash.clone();
        let h2 = hash.clone();
        let app = Router::new()
            .route(
                "/enqueue",
                post(move || {
                    let h = h1.clone();
                    async move { Json(json!({ "hash": h, "status": "queued" })) }
                }),
            )
            .route(
                "/jobs/:hash",
                get(move || {
                    let h = h2.clone();
                    async move {
                        Json(json!({
                            "hash": h,
                            "status": "error",
                            "error": { "message": "model not found" }
                        }))
                    }
                }),
            );
        let addr = spawn_test_server(app).await;
        let client = OllamaBridgeClient::new(format!("http://{addr}"), "k".into()).unwrap();
        let err = client
            .submit_and_wait(JobEndpoint::Chat, json!({}), Duration::from_secs(5))
            .await
            .unwrap_err();
        match err {
            BridgeError::JobErrored { error, .. } => {
                assert_eq!(error["message"], "model not found");
            }
            other => panic!("expected JobErrored, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn submit_and_wait_times_out_if_job_stays_queued() {
        // Bridge always returns Queued; submit_and_wait should
        // surface JobTimeout after ~timeout elapses.
        let hash = dummy_hash().to_string();
        let h1 = hash.clone();
        let h2 = hash.clone();
        let app = Router::new()
            .route(
                "/enqueue",
                post(move || {
                    let h = h1.clone();
                    async move { Json(json!({ "hash": h, "status": "queued" })) }
                }),
            )
            .route(
                "/jobs/:hash",
                get(move || {
                    let h = h2.clone();
                    async move { Json(json!({ "hash": h, "status": "running" })) }
                }),
            );
        let addr = spawn_test_server(app).await;
        let client = OllamaBridgeClient::new(format!("http://{addr}"), "k".into()).unwrap();
        let started = std::time::Instant::now();
        let err = client
            .submit_and_wait(JobEndpoint::Chat, json!({}), Duration::from_millis(800))
            .await
            .unwrap_err();
        let elapsed = started.elapsed();
        assert!(matches!(err, BridgeError::JobTimeout { .. }));
        // Timeout should fire after ~800ms (one poll cycle past).
        // Generous upper bound — CI variance.
        assert!(elapsed < Duration::from_secs(5), "elapsed: {elapsed:?}");
    }

    #[tokio::test]
    async fn enqueue_includes_bearer_auth_header() {
        let captured: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let cap_clone = captured.clone();
        let h = dummy_hash().to_string();
        let app = Router::new().route(
            "/enqueue",
            post(move |headers: axum::http::HeaderMap| {
                let cap = cap_clone.clone();
                let h = h.clone();
                async move {
                    let auth = headers
                        .get(axum::http::header::AUTHORIZATION)
                        .and_then(|v| v.to_str().ok())
                        .map(|s| s.to_string());
                    *cap.lock().await = auth;
                    Json(json!({ "hash": h, "status": "queued" }))
                }
            }),
        );
        let addr = spawn_test_server(app).await;
        let client = OllamaBridgeClient::new(format!("http://{addr}"), "the-token".into()).unwrap();
        client.enqueue(JobEndpoint::Chat, json!({})).await.unwrap();
        let auth = captured.lock().await.clone();
        assert_eq!(auth.as_deref(), Some("Bearer the-token"));
    }

    #[tokio::test]
    async fn body_cap_rejects_oversized_response() {
        // Bridge returns a 2 KiB body; client capped at 1 KiB —
        // must surface as BodyTooLarge before allocating the full
        // body (sipag #527 hygiene).
        let body = vec![0u8; 2048];
        let app = Router::new().route("/big", get(move || async move { body.clone() }));
        let addr = spawn_test_server(app).await;
        let client = OllamaBridgeClient::new(format!("http://{addr}"), "k".into()).unwrap();
        let err = client
            .get_capped(&format!("http://{addr}/big"), 1024)
            .await
            .unwrap_err();
        match err {
            BridgeError::BodyTooLarge { cap } => assert_eq!(cap, 1024),
            other => panic!("expected BodyTooLarge, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn tags_passthrough_returns_upstream_json() {
        let app = Router::new().route(
            "/api/tags",
            get(|| async { Json(json!({ "models": [{ "name": "gemma4:latest" }] })) }),
        );
        let addr = spawn_test_server(app).await;
        let client = OllamaBridgeClient::new(format!("http://{addr}"), "k".into()).unwrap();
        let v = client.tags().await.unwrap();
        assert_eq!(v["models"][0]["name"], "gemma4:latest");
    }

    #[tokio::test]
    async fn client_strips_trailing_slash_in_url() {
        let c = OllamaBridgeClient::new("https://example.com/".into(), "k".into()).unwrap();
        assert_eq!(c.url(), "https://example.com");
    }

    // ── feature-requirement tests added per user direction ──────────
    //
    // The tests above mostly exercise the happy path + one failure
    // mode per method. These tests cover behavioral requirements
    // documented in the bridge's README that aren't otherwise pinned:
    //
    // - dedup cache: enqueue can return `done` if the same hash
    //   finished within the dedup window; submit_and_wait must
    //   handle that without unnecessary waiting.
    // - all three endpoints work: Chat / Generate / Embed all need
    //   to serialize correctly so the bridge accepts them.
    // - non-503 HTTP errors: bridge returns 401/422/etc. with a JSON
    //   error body; client must surface status + body intact so
    //   operators can debug.
    // - missing `result` on done: bridge guarantees `result` is
    //   present iff `status == "done"`, but the defensive contract
    //   is to return `Value::Null` rather than crash.

    #[tokio::test]
    async fn submit_and_wait_handles_dedup_cached_done() {
        // Bridge contract (README "Dedup" section): an enqueue can
        // return `done` immediately if the same hash finished within
        // the 60s dedup window. submit_and_wait must handle this —
        // poll once, get the cached result, return without sleeping
        // through any polling delay.
        let hash = dummy_hash().to_string();
        let h1 = hash.clone();
        let h2 = hash.clone();
        let app = Router::new()
            .route(
                "/enqueue",
                post(move || {
                    let h = h1.clone();
                    // Cached: enqueue says `done` straightaway.
                    async move { Json(json!({ "hash": h, "status": "done" })) }
                }),
            )
            .route(
                "/jobs/:hash",
                get(move || {
                    let h = h2.clone();
                    async move {
                        Json(json!({
                            "hash": h,
                            "status": "done",
                            "result": { "cached": true, "value": 42 }
                        }))
                    }
                }),
            );
        let addr = spawn_test_server(app).await;
        let client = OllamaBridgeClient::new(format!("http://{addr}"), "k".into()).unwrap();
        let started = std::time::Instant::now();
        let result = client
            .submit_and_wait(
                JobEndpoint::Chat,
                json!({"model": "gemma4:latest"}),
                Duration::from_secs(5),
            )
            .await
            .unwrap();
        let elapsed = started.elapsed();
        assert_eq!(result["cached"], true);
        assert_eq!(result["value"], 42);
        // Should NOT have slept the 500ms poll interval — cached
        // result is available on the first poll.
        assert!(
            elapsed < Duration::from_millis(400),
            "dedup-cached should return without sleeping; took {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn enqueue_supports_all_three_job_endpoints() {
        // The bridge accepts /api/chat, /api/generate, /api/embed.
        // All three must serialize to the correct wire string AND
        // round-trip cleanly. A test capturing the request body
        // confirms the client sends the right `endpoint` value per
        // variant.
        let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let cap_clone = captured.clone();
        let h = dummy_hash().to_string();
        let app = Router::new().route(
            "/enqueue",
            post(move |body: Json<serde_json::Value>| {
                let cap = cap_clone.clone();
                let h = h.clone();
                async move {
                    let ep = body["endpoint"].as_str().unwrap_or("?").to_string();
                    cap.lock().await.push(ep);
                    Json(json!({ "hash": h, "status": "queued" }))
                }
            }),
        );
        let addr = spawn_test_server(app).await;
        let client = OllamaBridgeClient::new(format!("http://{addr}"), "k".into()).unwrap();
        for ep in [JobEndpoint::Chat, JobEndpoint::Generate, JobEndpoint::Embed] {
            client.enqueue(ep, json!({"x": 1})).await.unwrap();
        }
        let seen = captured.lock().await.clone();
        assert_eq!(
            seen,
            vec![
                "/api/chat".to_string(),
                "/api/generate".to_string(),
                "/api/embed".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn enqueue_surfaces_non_503_http_error_with_body() {
        // Bridge can return 401 (unauthorized), 400 (invalid_json),
        // 422 (invalid_body_shape or invalid_endpoint). Operators
        // need the status code AND the error body to diagnose;
        // client must preserve both verbatim.
        let app = Router::new().route(
            "/enqueue",
            post(|| async {
                (
                    axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                    Json(json!({
                        "error": "invalid_body_shape",
                        "detail": "missing 'model' field"
                    })),
                )
            }),
        );
        let addr = spawn_test_server(app).await;
        let client = OllamaBridgeClient::new(format!("http://{addr}"), "k".into()).unwrap();
        let err = client
            .enqueue(JobEndpoint::Chat, json!({}))
            .await
            .unwrap_err();
        match err {
            BridgeError::Http { status, body } => {
                assert_eq!(status.as_u16(), 422);
                // Caller can read the structured error from the body.
                let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
                assert_eq!(parsed["error"], "invalid_body_shape");
                assert!(parsed["detail"].as_str().unwrap().contains("missing"));
            }
            other => panic!("expected Http, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn submit_and_wait_returns_null_when_done_has_no_result_field() {
        // Defensive: the bridge contract says `result` is present
        // iff `status == "done"`, but if a future bridge bug or
        // partial write surfaces `done` without a `result`, the
        // client should return `Value::Null` rather than crash.
        // This pins the no-crash semantics so a future refactor
        // can't silently start panicking on this edge case.
        let hash = dummy_hash().to_string();
        let h1 = hash.clone();
        let h2 = hash.clone();
        let app = Router::new()
            .route(
                "/enqueue",
                post(move || {
                    let h = h1.clone();
                    async move { Json(json!({ "hash": h, "status": "queued" })) }
                }),
            )
            .route(
                "/jobs/:hash",
                get(move || {
                    let h = h2.clone();
                    async move {
                        Json(json!({
                            "hash": h,
                            "status": "done"
                            // result field intentionally omitted
                        }))
                    }
                }),
            );
        let addr = spawn_test_server(app).await;
        let client = OllamaBridgeClient::new(format!("http://{addr}"), "k".into()).unwrap();
        let result = client
            .submit_and_wait(JobEndpoint::Chat, json!({}), Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(result, serde_json::Value::Null);
    }

    #[tokio::test]
    async fn poll_surfaces_404_as_http_error() {
        // Bridge contract: `GET /jobs/:hash` returns 404 if the hash
        // is well-formed but no job exists with that hash (typically
        // because the janitor pruned the finished job past TTL).
        // Callers retrying a stale hash after a restart will see
        // this — must surface as `Http { status: 404, body }` so
        // operators can distinguish "transient bridge down" from
        // "job gone."
        let hash = dummy_hash().to_string();
        let app = Router::new().route(
            "/jobs/:hash",
            get(|| async {
                (
                    axum::http::StatusCode::NOT_FOUND,
                    Json(json!({"error": "not_found"})),
                )
            }),
        );
        let addr = spawn_test_server(app).await;
        let client = OllamaBridgeClient::new(format!("http://{addr}"), "k".into()).unwrap();
        let err = client.poll(&hash).await.unwrap_err();
        match err {
            BridgeError::Http { status, body } => {
                assert_eq!(status.as_u16(), 404);
                assert!(body.contains("not_found"), "got: {body}");
            }
            other => panic!("expected Http(404), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn poll_includes_bearer_auth_header() {
        // Same contract as enqueue: every request must carry the
        // bearer token. Polls are the most-frequent call site by
        // far (every 500ms inside submit_and_wait); a regression
        // here would silently 401-flood the bridge.
        let captured: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let cap_clone = captured.clone();
        let hash = dummy_hash().to_string();
        let h = hash.clone();
        let app = Router::new().route(
            "/jobs/:hash",
            get(move |headers: axum::http::HeaderMap| {
                let cap = cap_clone.clone();
                let h = h.clone();
                async move {
                    let auth = headers
                        .get(axum::http::header::AUTHORIZATION)
                        .and_then(|v| v.to_str().ok())
                        .map(|s| s.to_string());
                    *cap.lock().await = auth;
                    Json(json!({"hash": h, "status": "queued"}))
                }
            }),
        );
        let addr = spawn_test_server(app).await;
        let client = OllamaBridgeClient::new(format!("http://{addr}"), "the-token".into()).unwrap();
        client.poll(&hash).await.unwrap();
        let auth = captured.lock().await.clone();
        assert_eq!(auth.as_deref(), Some("Bearer the-token"));
    }

    #[tokio::test]
    async fn probes_include_bearer_auth_header() {
        // tags() / show() / ps() all route through get_capped; one
        // test on tags() pins the contract that probes carry the
        // bearer token too (covers the third of three auth-bearing
        // call sites: enqueue, poll, and the get_capped family).
        let captured: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let cap_clone = captured.clone();
        let app = Router::new().route(
            "/api/tags",
            get(move |headers: axum::http::HeaderMap| {
                let cap = cap_clone.clone();
                async move {
                    let auth = headers
                        .get(axum::http::header::AUTHORIZATION)
                        .and_then(|v| v.to_str().ok())
                        .map(|s| s.to_string());
                    *cap.lock().await = auth;
                    Json(json!({"models": []}))
                }
            }),
        );
        let addr = spawn_test_server(app).await;
        let client = OllamaBridgeClient::new(format!("http://{addr}"), "the-token".into()).unwrap();
        client.tags().await.unwrap();
        let auth = captured.lock().await.clone();
        assert_eq!(auth.as_deref(), Some("Bearer the-token"));
    }

    #[tokio::test]
    async fn concurrent_submit_and_wait_does_not_interfere() {
        // Bridge use case: a lens-worker fleet fans out N
        // concurrent jobs on one client (Clone-able, shares one
        // reqwest::Client). Each submit_and_wait tracks its own
        // local `hash` — two concurrent calls must NOT cross
        // results. This pins the no-shared-state invariant; a
        // regression introducing a shared `Mutex<last_hash>` (etc.)
        // would manifest as one call returning the other's result.
        //
        // The mock server returns different results based on which
        // distinct hash the GET hits, so a result-swap would surface
        // as a wrong-value assertion.
        let hash_a = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string();
        let hash_b = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string();
        // Track which hash each call enqueued for routing.
        let next_hash: Arc<Mutex<Vec<String>>> =
            Arc::new(Mutex::new(vec![hash_a.clone(), hash_b.clone()]));
        let nh = next_hash.clone();
        let app = Router::new()
            .route(
                "/enqueue",
                post(move || {
                    let nh = nh.clone();
                    async move {
                        // Hand out queued hashes in order.
                        let h = nh.lock().await.remove(0);
                        Json(json!({"hash": h, "status": "queued"}))
                    }
                }),
            )
            .route(
                "/jobs/:hash",
                get(
                    |axum::extract::Path(hash): axum::extract::Path<String>| async move {
                        // Result encodes the hash so a swap shows up.
                        Json(json!({
                            "hash": hash.clone(),
                            "status": "done",
                            "result": { "for_hash": hash }
                        }))
                    },
                ),
            );
        let addr = spawn_test_server(app).await;
        let client = OllamaBridgeClient::new(format!("http://{addr}"), "k".into()).unwrap();
        let c1 = client.clone();
        let c2 = client.clone();
        let (r1, r2) = tokio::join!(
            c1.submit_and_wait(JobEndpoint::Chat, json!({"x": 1}), Duration::from_secs(5)),
            c2.submit_and_wait(JobEndpoint::Chat, json!({"x": 2}), Duration::from_secs(5)),
        );
        let r1 = r1.unwrap();
        let r2 = r2.unwrap();
        // Each call should see the result the bridge dispatched for
        // its enqueued hash — never the OTHER call's.
        let h1 = r1["for_hash"].as_str().unwrap().to_string();
        let h2 = r2["for_hash"].as_str().unwrap().to_string();
        // Both hashes should have been observed exactly once across
        // the two calls. The race makes which hash lands in r1 vs r2
        // nondeterministic, but they must be distinct.
        assert_ne!(h1, h2, "concurrent calls received the same hash");
        let mut seen = vec![h1, h2];
        seen.sort();
        assert_eq!(seen, vec![hash_a, hash_b]);
    }

    #[tokio::test]
    async fn http_error_body_strips_control_characters() {
        // A misbehaving or compromised bridge could embed ANSI
        // escape sequences or other control characters in an error
        // body; if callers later log `BridgeError::Http`'s `Display`
        // impl to a terminal or structured log, those bytes would
        // pass through and disrupt rendering / inject fake log
        // lines. The client sanitizes before storing.
        let app = Router::new().route(
            "/enqueue",
            post(|| async {
                // Bytes: BEL, ESC[31m (red), text, ESC[0m, NUL, valid text.
                let evil = b"\x07\x1b[31mfake-error\x1b[0m\x00real-error".to_vec();
                (
                    axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                    [(axum::http::header::CONTENT_TYPE, "text/plain")],
                    evil,
                )
            }),
        );
        let addr = spawn_test_server(app).await;
        let client = OllamaBridgeClient::new(format!("http://{addr}"), "k".into()).unwrap();
        let err = client
            .enqueue(JobEndpoint::Chat, json!({}))
            .await
            .unwrap_err();
        match err {
            BridgeError::Http { body, .. } => {
                assert!(!body.contains('\x07'), "BEL leaked: {body:?}");
                assert!(!body.contains('\x1b'), "ESC leaked: {body:?}");
                assert!(!body.contains('\x00'), "NUL leaked: {body:?}");
                // The legible payload survives (filter strips the
                // control bytes, not the surrounding text).
                assert!(body.contains("fake-error"), "got: {body:?}");
                assert!(body.contains("real-error"), "got: {body:?}");
            }
            other => panic!("expected Http, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn submit_and_wait_polls_through_queued_to_done() {
        // Bridge contract: a job transitions queued → running →
        // done. submit_and_wait must keep polling through
        // intermediate states without returning early on `running`.
        let hash = dummy_hash().to_string();
        let h1 = hash.clone();
        let h2 = hash.clone();
        let poll_count: Arc<Mutex<u32>> = Arc::new(Mutex::new(0));
        let pc = poll_count.clone();
        let app = Router::new()
            .route(
                "/enqueue",
                post(move || {
                    let h = h1.clone();
                    async move { Json(json!({ "hash": h, "status": "queued" })) }
                }),
            )
            .route(
                "/jobs/:hash",
                get(move || {
                    let h = h2.clone();
                    let pc = pc.clone();
                    async move {
                        let mut count = pc.lock().await;
                        *count += 1;
                        // Sequence: poll 1 = running, poll 2 = done.
                        if *count == 1 {
                            Json(json!({ "hash": h, "status": "running" }))
                        } else {
                            Json(json!({
                                "hash": h,
                                "status": "done",
                                "result": { "done_at_poll": *count }
                            }))
                        }
                    }
                }),
            );
        let addr = spawn_test_server(app).await;
        let client = OllamaBridgeClient::new(format!("http://{addr}"), "k".into()).unwrap();
        let result = client
            .submit_and_wait(JobEndpoint::Chat, json!({}), Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(result["done_at_poll"], 2);
        // Confirm we DID see the running state (would be 1 if we
        // bailed early on `running`).
        let final_count = *poll_count.lock().await;
        assert_eq!(final_count, 2, "expected exactly 2 polls (running → done)");
    }
}
