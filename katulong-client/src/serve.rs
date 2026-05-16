//! `katulong-client serve` — notebook-style web UI for stepping
//! through library calls and watching the katulong session reflect
//! them in real time.
//!
//! Spawns a fresh katulong subprocess on a free local port, holds a
//! single persistent attach to a session, and exposes the same
//! library calls the CLI wraps as POST/GET endpoints. The notebook
//! page (`notebook.html`, embedded via `include_str!`) renders
//! hardcoded cells — create, paste, press, wait-for, lines,
//! snapshot — each with a Play button and an output area. An
//! iframe pinned to the katulong URL shows the live session next
//! to the cells, so every click is visible side-by-side.
//!
//! Designed for the validation loop the headless client was built
//! to enable: click → library call → katulong PTY update →
//! browser reflects the change. No multi-terminal copy-paste.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::{
    AttachError, KatulongAttach, KatulongAttachClient, KatulongClient, KeyName, RegexMatch,
    RemoteConfig, WaitFrom,
};

pub struct ServeOpts {
    /// Port the notebook UI listens on. The user opens
    /// `http://127.0.0.1:<port>` in a browser.
    pub port: u16,
    /// Path to a local katulong checkout (the directory with
    /// `server.js`). Required.
    pub katulong_repo: PathBuf,
}

/// Run the serve subcommand: spawn katulong, start the axum server,
/// wait for Ctrl-C, tear down.
pub async fn run(opts: ServeOpts) -> Result<()> {
    let server_js = opts.katulong_repo.join("server.js");
    if !server_js.exists() {
        bail!(
            "katulong_repo does not contain a server.js: {:?}",
            opts.katulong_repo
        );
    }

    let katulong_port = free_port()?;
    let data_dir = tempfile::tempdir()?;
    // Per-sandbox tmux socket. Without this, the sandbox katulong
    // shares the default tmux socket with any other katulong instance
    // the operator happens to have running (e.g. their daily-driver
    // katulong on a different port). The other instance discovers
    // our newly-spawned `kat_<id>` session, adopts it as external,
    // and runs its OWN `tmux -C attach-session -d -t …` — the `-d`
    // detaches OUR control client, our control proc closes with
    // code 0, katulong relays exit:0 to the Rust attach, and the
    // next /api/input fails with "session ended with exit code 0".
    // KATULONG_TMUX_SOCKET must match /^[A-Za-z0-9_-]+$/ on the
    // katulong side, so we only use the process id (which already
    // does).
    let tmux_socket = format!("sipag-sandbox-{}", std::process::id());
    println!(
        "[serve] spawning katulong on 127.0.0.1:{katulong_port} \
         (state in {:?}, tmux socket {tmux_socket})",
        data_dir.path()
    );

    let child = Command::new("node")
        .arg(&server_js)
        .current_dir(&opts.katulong_repo)
        .env("PORT", katulong_port.to_string())
        .env("KATULONG_BIND_HOST", "127.0.0.1")
        .env("KATULONG_DATA_DIR", data_dir.path())
        .env("KATULONG_TMUX_SOCKET", &tmux_socket)
        .env("LOG_LEVEL", "warn")
        .env("NODE_ENV", "production")
        // Suppress katulong's own logs so the notebook UI is the
        // primary surface. Operators who want them can tail
        // KATULONG_DATA_DIR or run the sandbox example instead.
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    let _guard = TeardownGuard {
        child: Some(child),
        data_dir: Some(data_dir),
        tmux_socket: tmux_socket.clone(),
    };

    wait_until_ready(katulong_port, Duration::from_secs(15))?;
    println!("[serve] katulong is up on 127.0.0.1:{katulong_port}");

    let remote = RemoteConfig {
        url: format!("http://127.0.0.1:{katulong_port}"),
        api_key: "unused-because-localhost".to_string(),
    };
    let state = Arc::new(ServeState {
        remote: remote.clone(),
        attach_client: KatulongAttachClient::new(remote.clone()),
        http: KatulongClient::new(remote.url.clone(), remote.api_key.clone()),
        current: Mutex::new(None),
    });

    let app = Router::new()
        .route("/", get(notebook_html))
        .route("/api/state", get(api_state))
        .route("/api/sessions", get(api_sessions))
        .route("/api/create", post(api_create))
        .route("/api/input", post(api_input))
        .route("/api/paste", post(api_paste))
        .route("/api/press", post(api_press))
        .route("/api/wait-for", post(api_wait_for))
        .route("/api/lines", get(api_lines))
        .route("/api/close", post(api_close))
        .route("/api/reset", post(api_reset))
        .route("/api/snapshot", get(api_snapshot))
        .with_state(state.clone());

    let bind = format!("127.0.0.1:{}", opts.port);
    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .with_context(|| format!("bind {bind}"))?;
    let serve_url = format!("http://{}", listener.local_addr()?);

    println!();
    println!("─────────────────────────────────────────────────────────────────────────");
    println!(" Open the notebook UI in your browser:");
    println!("   {serve_url}/");
    println!();
    println!(" The page embeds an iframe of the katulong session at:");
    println!("   {}/", remote.url);
    println!();
    println!(" Click Play on each cell to step through; the iframe reflects");
    println!(" every change live. Ctrl-C in THIS terminal to tear down.");
    println!("─────────────────────────────────────────────────────────────────────────");

    let server = axum::serve(listener, app);
    tokio::select! {
        result = server => {
            result.context("axum server error")?;
        }
        _ = tokio::signal::ctrl_c() => {
            println!();
            println!("[serve] tearing down");
        }
    }

    // Best-effort: close the persistent attach (if any) before
    // dropping the guard. `TeardownGuard::drop` kills katulong;
    // tokio runtime shuts down after we return.
    if let Some((_, attach)) = state.current.lock().await.take() {
        let _ = tokio::time::timeout(Duration::from_secs(2), attach.close()).await;
    }
    Ok(())
}

// ── server state ────────────────────────────────────────────────

struct ServeState {
    remote: RemoteConfig,
    attach_client: KatulongAttachClient,
    http: KatulongClient,
    /// `(session_name, attach)` for the cell the user is driving.
    /// Created on POST /api/create; replaced on subsequent calls
    /// (each call closes the previous attach first).
    current: Mutex<Option<(String, KatulongAttach)>>,
}

type SharedState = Arc<ServeState>;

// ── HTML page ───────────────────────────────────────────────────

async fn notebook_html() -> Html<&'static str> {
    Html(include_str!("notebook.html"))
}

// ── /api/state ──────────────────────────────────────────────────

#[derive(Serialize)]
struct StateResp {
    katulong_url: String,
    current_session: Option<String>,
}

async fn api_state(State(s): State<SharedState>) -> Json<StateResp> {
    let current_session = s.current.lock().await.as_ref().map(|(n, _)| n.clone());
    Json(StateResp {
        katulong_url: s.remote.url.clone(),
        current_session,
    })
}

// ── /api/sessions ───────────────────────────────────────────────

async fn api_sessions(State(s): State<SharedState>) -> ApiResult<Json<Vec<crate::Session>>> {
    // KatulongClient methods are sync (curl shell-out); jump to a
    // blocking task so we don't pin the runtime.
    let http = s.http.clone();
    let sessions = tokio::task::spawn_blocking(move || http.list_sessions())
        .await
        .map_err(|e| ApiError::internal(format!("join: {e}")))?
        .map_err(|e| ApiError::internal(format!("list sessions: {e}")))?;
    Ok(Json(sessions))
}

// ── /api/create ─────────────────────────────────────────────────

#[derive(Serialize)]
struct CreateResp {
    name: String,
    id: String,
}

async fn api_create(State(s): State<SharedState>) -> ApiResult<Json<CreateResp>> {
    // Close any previous attach so we don't leak background tasks.
    if let Some((_, attach)) = s.current.lock().await.take() {
        let _ = tokio::time::timeout(Duration::from_secs(2), attach.close()).await;
    }
    let http = s.http.clone();
    let session = tokio::task::spawn_blocking(move || http.create_dispatch_session())
        .await
        .map_err(|e| ApiError::internal(format!("join: {e}")))?
        .map_err(|e| ApiError::internal(format!("create session: {e}")))?;
    let attach = s
        .attach_client
        .attach(&session.name, 120, 40)
        .await
        .map_err(|e| ApiError::internal(format!("attach: {e}")))?;
    *s.current.lock().await = Some((session.name.clone(), attach));
    Ok(Json(CreateResp {
        name: session.name,
        id: session.id,
    }))
}

// ── /api/input ──────────────────────────────────────────────────

#[derive(Deserialize)]
struct InputReq {
    bytes: String,
}

async fn api_input(
    State(s): State<SharedState>,
    Json(req): Json<InputReq>,
) -> ApiResult<Json<OkResp>> {
    let guard = s.current.lock().await;
    let (_, attach) = guard.as_ref().ok_or_else(no_session)?;
    attach
        .input(req.bytes)
        .await
        .map_err(ApiError::from_attach)?;
    Ok(Json(OkResp { ok: true }))
}

// ── /api/paste ──────────────────────────────────────────────────

#[derive(Deserialize)]
struct PasteReq {
    body: String,
}

async fn api_paste(
    State(s): State<SharedState>,
    Json(req): Json<PasteReq>,
) -> ApiResult<Json<OkResp>> {
    // Same wire shape xterm.js sends on a paste event: one
    // `{type:"input"}` with the body verbatim. The running app
    // decides what to do with the bytes; this client does NOT
    // bracketed-paste-wrap.
    let guard = s.current.lock().await;
    let (_, attach) = guard.as_ref().ok_or_else(no_session)?;
    attach
        .input(req.body.clone())
        .await
        .map_err(ApiError::from_attach)?;
    Ok(Json(OkResp { ok: true }))
}

// ── /api/press ──────────────────────────────────────────────────

#[derive(Deserialize)]
struct PressReq {
    key: String,
}

async fn api_press(
    State(s): State<SharedState>,
    Json(req): Json<PressReq>,
) -> ApiResult<Json<OkResp>> {
    let key = parse_key(&req.key).map_err(ApiError::bad_request)?;
    let guard = s.current.lock().await;
    let (_, attach) = guard.as_ref().ok_or_else(no_session)?;
    attach.press(key).await.map_err(ApiError::from_attach)?;
    Ok(Json(OkResp { ok: true }))
}

// ── /api/wait-for ───────────────────────────────────────────────

#[derive(Deserialize)]
struct WaitForReq {
    pattern: String,
    /// `now` | `attach` | numeric offset.
    from: Option<String>,
    /// Seconds. 0 means no timeout (caller had better be patient).
    timeout: Option<u64>,
}

#[derive(Serialize)]
struct WaitForResp {
    matched_text: String,
    start: usize,
    end: usize,
}

async fn api_wait_for(
    State(s): State<SharedState>,
    Json(req): Json<WaitForReq>,
) -> ApiResult<Json<WaitForResp>> {
    let re = regex::Regex::new(&req.pattern)
        .map_err(|e| ApiError::bad_request(format!("compile pattern: {e}")))?;
    let from =
        parse_wait_from(req.from.as_deref().unwrap_or("now")).map_err(ApiError::bad_request)?;
    let timeout = match req.timeout.unwrap_or(30) {
        0 => None,
        n => Some(Duration::from_secs(n)),
    };
    let guard = s.current.lock().await;
    let (_, attach) = guard.as_ref().ok_or_else(no_session)?;
    let m: RegexMatch = attach
        .wait_for(&re, from, timeout)
        .await
        .map_err(ApiError::from_attach)?;
    Ok(Json(WaitForResp {
        matched_text: m.matched_text,
        start: m.start,
        end: m.end,
    }))
}

// ── /api/lines ──────────────────────────────────────────────────

#[derive(Deserialize)]
struct LinesQuery {
    n: Option<usize>,
}

#[derive(Serialize)]
struct LinesResp {
    lines: Vec<String>,
}

async fn api_lines(
    State(s): State<SharedState>,
    Query(q): Query<LinesQuery>,
) -> ApiResult<Json<LinesResp>> {
    let n = q.n.unwrap_or(40);
    let guard = s.current.lock().await;
    let (_, attach) = guard.as_ref().ok_or_else(no_session)?;
    let lines = attach.last_n_lines(n).await;
    Ok(Json(LinesResp { lines }))
}

// ── /api/close ──────────────────────────────────────────────────

async fn api_close(State(s): State<SharedState>) -> ApiResult<Json<OkResp>> {
    if let Some((_, attach)) = s.current.lock().await.take() {
        let _ = tokio::time::timeout(Duration::from_secs(2), attach.close()).await;
    }
    Ok(Json(OkResp { ok: true }))
}

// ── /api/snapshot ───────────────────────────────────────────────

/// Return the raw rolling-buffer bytes (ANSI escapes intact).
/// The notebook UI feeds these into xterm.js so the visual matches
/// what a browser would see — cursor escapes get applied in 2D
/// space instead of leaving autosuggestion text adjacent in a
/// linearised view. Content-Type is octet-stream so transport
/// doesn't try to decode as UTF-8 at the framing layer.
async fn api_snapshot(State(s): State<SharedState>) -> ApiResult<Response> {
    let guard = s.current.lock().await;
    let (_, attach) = guard.as_ref().ok_or_else(no_session)?;
    let bytes = attach.buffer_snapshot().await;
    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
        bytes,
    )
        .into_response())
}

// ── /api/reset ──────────────────────────────────────────────────

/// Test-isolation aid: close the persistent attach AND kill every
/// session on the underlying katulong. Lets a playwright `beforeEach`
/// start from a known-empty state and avoid the
/// `MAX_SESSIONS=20` accumulation that surfaces when tests share
/// one long-lived `serve` instance.
async fn api_reset(State(s): State<SharedState>) -> ApiResult<Json<OkResp>> {
    if let Some((_, attach)) = s.current.lock().await.take() {
        let _ = tokio::time::timeout(Duration::from_secs(2), attach.close()).await;
    }
    let http = s.http.clone();
    tokio::task::spawn_blocking(move || {
        if let Ok(sessions) = http.list_sessions() {
            for sess in sessions {
                let _ = http.kill_session(&sess.id);
            }
        }
    })
    .await
    .map_err(|e| ApiError::internal(format!("reset join: {e}")))?;
    Ok(Json(OkResp { ok: true }))
}

// ── helpers ─────────────────────────────────────────────────────

#[derive(Serialize)]
struct OkResp {
    ok: bool,
}

fn no_session() -> ApiError {
    ApiError::conflict("no current session — POST /api/create first")
}

fn parse_key(s: &str) -> std::result::Result<KeyName, String> {
    Ok(match s.to_ascii_lowercase().as_str() {
        "enter" | "return" | "\\r" => KeyName::Enter,
        "escape" | "esc" => KeyName::Escape,
        "tab" => KeyName::Tab,
        "backspace" | "bs" => KeyName::Backspace,
        "ctrl-c" | "ctrlc" | "^c" => KeyName::CtrlC,
        "ctrl-d" | "ctrld" | "^d" => KeyName::CtrlD,
        "up" => KeyName::Up,
        "down" => KeyName::Down,
        "left" => KeyName::Left,
        "right" => KeyName::Right,
        other => return Err(format!(
            "unknown key '{other}' — try: enter, escape, tab, backspace, ctrl-c, ctrl-d, up, down, left, right"
        )),
    })
}

fn parse_wait_from(s: &str) -> std::result::Result<WaitFrom, String> {
    Ok(match s {
        "now" => WaitFrom::FromNow,
        "attach" | "from-attach" => WaitFrom::FromAttach,
        other => {
            let offset: usize = other.parse().map_err(|_| {
                format!("from must be 'now', 'attach', or a byte offset (got '{other}')")
            })?;
            WaitFrom::FromOffset(offset)
        }
    })
}

// ── error mapping ───────────────────────────────────────────────

struct ApiError {
    status: StatusCode,
    body: serde_json::Value,
}

type ApiResult<T> = std::result::Result<T, ApiError>;

impl ApiError {
    fn bad_request<S: Into<String>>(msg: S) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            body: serde_json::json!({ "error": msg.into() }),
        }
    }
    fn conflict<S: Into<String>>(msg: S) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            body: serde_json::json!({ "error": msg.into() }),
        }
    }
    fn internal<S: Into<String>>(msg: S) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            body: serde_json::json!({ "error": msg.into() }),
        }
    }
    fn from_attach(e: AttachError) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            body: serde_json::json!({ "error": format!("{e}") }),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}

// ── subprocess + readiness helpers (shared shape with sandbox) ──

struct TeardownGuard {
    child: Option<Child>,
    data_dir: Option<tempfile::TempDir>,
    tmux_socket: String,
}

impl Drop for TeardownGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        // Kill the per-sandbox tmux server. tmux servers outlive their
        // spawning process by design, so without this every sandbox
        // run leaks an orphan tmux server on its private socket.
        let _ = std::process::Command::new("tmux")
            .args(["-L", &self.tmux_socket, "kill-server"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if let Some(dir) = self.data_dir.take() {
            let path = dir.path().to_path_buf();
            drop(dir);
            println!("[serve] katulong killed, state dir cleaned: {path:?}");
        }
    }
}

fn free_port() -> std::io::Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

fn wait_until_ready(port: u16, max: Duration) -> std::io::Result<()> {
    let deadline = Instant::now() + max;
    while Instant::now() < deadline {
        if let Ok(status) = http_head_status(&format!("http://127.0.0.1:{port}/sessions")) {
            if (200..500).contains(&status) {
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(std::io::Error::other(format!(
        "katulong did not respond on 127.0.0.1:{port}/sessions within {max:?}"
    )))
}

fn http_head_status(url: &str) -> std::io::Result<u16> {
    let parsed = url::Url::parse(url).map_err(std::io::Error::other)?;
    let host = parsed
        .host_str()
        .ok_or_else(|| std::io::Error::other("missing host"))?;
    let port = parsed.port().unwrap_or(80);
    let path = parsed.path();
    let mut sock = TcpStream::connect((host, port))?;
    sock.set_read_timeout(Some(Duration::from_millis(500)))?;
    sock.set_write_timeout(Some(Duration::from_millis(500)))?;
    let req = format!("GET {path} HTTP/1.0\r\nHost: {host}\r\n\r\n");
    sock.write_all(req.as_bytes())?;
    let mut buf = [0u8; 64];
    let n = sock.read(&mut buf)?;
    let head = std::str::from_utf8(&buf[..n]).unwrap_or("");
    head.split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| anyhow!("bad status line: {head:?}"))
        .map_err(std::io::Error::other)
}
