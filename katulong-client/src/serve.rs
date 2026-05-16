//! `katulong-client serve` — notebook-style web UI for stepping
//! through library calls and watching the katulong session reflect
//! them in real time.
//!
//! Targets a REAL katulong instance (resolved by the CLI's standard
//! `resolve_remote` path — `--url`/`--api-key`, env, or
//! `~/.katulong/remote.json`). That's the same katulong production
//! sipag dispatches against, which is the point: clicking ▶ on a
//! notebook cell exercises the exact `katulong-client` code path the
//! dispatcher will use in anger. Sessions you create here appear in
//! the same katulong's session list and can be opened in any
//! katulong browser tab.
//!
//! Earlier iterations spawned a hermetic katulong subprocess for
//! isolation. That made the notebook a toy — it couldn't reproduce
//! the multi-client / shared-tmux / "real katulong" failure modes
//! the library has to survive. Use the integration-test harness
//! (`tests/common`) when you need a hermetic katulong for assertions;
//! use `serve` when you need to drive your real one.
//!
//! Each notebook owns one current session at a time. `/api/create`
//! makes a fresh `sipag-d-<hex>` session on the configured katulong
//! and opens a WS attach to it. Subsequent cells operate on that
//! attach. `/api/close` and `/api/reset` clean up just that session
//! — neither touches sessions you didn't create here.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::{
    AttachError, KatulongAttach, KatulongAttachClient, KatulongClient, KeyName, RegexMatch,
    RemoteConfig, Session, WaitFrom,
};

pub struct ServeOpts {
    /// Port the notebook UI listens on. The user opens
    /// `http://127.0.0.1:<port>` in a browser.
    pub port: u16,
    /// The katulong this notebook drives. Resolved by the CLI from
    /// flags / env / `~/.katulong/remote.json` and passed in.
    pub remote: RemoteConfig,
}

/// Run the serve subcommand: start the axum server, wait for Ctrl-C.
/// No subprocess management — the operator's katulong is the truth.
pub async fn run(opts: ServeOpts) -> Result<()> {
    let state = Arc::new(ServeState {
        remote: opts.remote.clone(),
        attach_client: KatulongAttachClient::new(opts.remote.clone()),
        http: KatulongClient::new(opts.remote.url.clone(), opts.remote.api_key.clone()),
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
    println!(" Driving katulong at:");
    println!("   {}/", opts.remote.url);
    println!();
    println!(" Each cell creates / drives a fresh `sipag-d-…` session on");
    println!(" that katulong. Open the session there to watch it in real time.");
    println!(" Ctrl-C in THIS terminal to stop the notebook (your katulong");
    println!(" keeps running).");
    println!("─────────────────────────────────────────────────────────────────────────");

    let server = axum::serve(listener, app);
    tokio::select! {
        result = server => {
            result.context("axum server error")?;
        }
        _ = tokio::signal::ctrl_c() => {
            println!();
            println!("[serve] stopping notebook");
        }
    }

    // Best-effort: close + kill the current session so we don't
    // leave it dangling on the operator's real katulong when they
    // Ctrl-C the notebook.
    teardown_current(&state).await;
    Ok(())
}

// ── server state ────────────────────────────────────────────────

struct ServeState {
    remote: RemoteConfig,
    attach_client: KatulongAttachClient,
    http: KatulongClient,
    /// `(session, attach)` for the cell the user is driving.
    /// `session` retains both name and id so we can DELETE on
    /// teardown without re-listing. Replaced on subsequent
    /// `/api/create`s; cleared on `/api/close` and `/api/reset`.
    current: Mutex<Option<(Session, KatulongAttach)>>,
}

type SharedState = Arc<ServeState>;

async fn teardown_current(state: &SharedState) {
    let taken = state.current.lock().await.take();
    if let Some((session, attach)) = taken {
        let _ = tokio::time::timeout(Duration::from_secs(2), attach.close()).await;
        let http = state.http.clone();
        let id = session.id.clone();
        let _ = tokio::task::spawn_blocking(move || http.kill_session(&id)).await;
    }
}

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
    let current_session = s
        .current
        .lock()
        .await
        .as_ref()
        .map(|(sess, _)| sess.name.clone());
    Json(StateResp {
        katulong_url: s.remote.url.clone(),
        current_session,
    })
}

// ── /api/sessions ───────────────────────────────────────────────

async fn api_sessions(State(s): State<SharedState>) -> ApiResult<Json<Vec<Session>>> {
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
    // Close + kill any previous session so we don't pile them up on
    // the operator's katulong across notebook clicks.
    teardown_current(&s).await;

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
    let resp = CreateResp {
        name: session.name.clone(),
        id: session.id.clone(),
    };
    *s.current.lock().await = Some((session, attach));
    Ok(Json(resp))
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
    teardown_current(&s).await;
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

/// Test-isolation aid: close the notebook's current attach and
/// delete the one session we created. Used by the Playwright
/// `beforeEach` to start each test from a known empty state.
///
/// Critically, this DOES NOT touch any other session on the
/// operator's katulong. Earlier iterations nuked everything on the
/// underlying katulong; that was safe when serve owned a hermetic
/// subprocess but would obliterate the operator's daily-driver
/// sessions now that we target a real katulong.
async fn api_reset(State(s): State<SharedState>) -> ApiResult<Json<OkResp>> {
    teardown_current(&s).await;
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
