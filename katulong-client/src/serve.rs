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

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use axum::extract::{Query, Request, State};
use axum::http::{Method, StatusCode};
use axum::middleware::{from_fn_with_state, Next};
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
    let bind = format!("127.0.0.1:{}", opts.port);
    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .with_context(|| format!("bind {bind}"))?;
    let local_addr = listener.local_addr()?;
    let serve_url = format!("http://{local_addr}");
    let port = local_addr.port();
    // Accept the canonical bind authority plus the two common
    // localhost aliases. Visitors routinely type `localhost:<port>`
    // or `[::1]:<port>` and rejecting them is a UX regression that
    // looks like a server bug from the operator's side.
    let accepted_hosts: HashSet<String> = [
        format!("127.0.0.1:{port}"),
        format!("localhost:{port}"),
        format!("[::1]:{port}"),
    ]
    .into_iter()
    .collect();
    let accepted_origins: HashSet<String> = accepted_hosts
        .iter()
        .map(|h| format!("http://{h}"))
        .collect();

    let state = Arc::new(ServeState {
        remote: opts.remote.clone(),
        attach_client: KatulongAttachClient::new(opts.remote.clone()),
        http: KatulongClient::new(opts.remote.url.clone(), opts.remote.api_key.clone()),
        current: Mutex::new(None),
        created_ids: Mutex::new(HashSet::new()),
        accepted_hosts,
        accepted_origins,
    });

    let app = Router::new()
        .route("/", get(notebook_html))
        .route("/api/state", get(api_state))
        .route("/api/sessions", get(api_sessions))
        .route("/api/create", post(api_create))
        .route("/api/paste", post(api_paste))
        .route("/api/press", post(api_press))
        .route("/api/wait-for", post(api_wait_for))
        .route("/api/lines", get(api_lines))
        .route("/api/close", post(api_close))
        .route("/api/reset", post(api_reset))
        .route("/api/snapshot", get(api_snapshot))
        // `/api/input` was a registered route but never wired into the
        // notebook UI; `/api/paste` covers the same bytes-to-attach
        // call. Dropped post-extraction review as dead code.
        .layer(from_fn_with_state(state.clone(), local_request_guard))
        .with_state(state.clone());

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
    /// Session ids THIS notebook process created via `/api/create`.
    /// `/api/reset` cleans up every id in this set — and ONLY ids in
    /// this set — so a notebook pointed at a shared katulong can't
    /// accidentally kill production sipag dispatch sessions (which
    /// use the same `sipag-d-<hex>` namespace). Survives a previous
    /// test's crashed-without-close as long as the serve process
    /// itself is reused.
    created_ids: Mutex<HashSet<String>>,
    /// Host strings the middleware accepts in the `Host:` header.
    /// Built from `expected_host` plus the equivalent `localhost`
    /// alias — operators routinely browse to `http://localhost:<port>`
    /// even when serve binds 127.0.0.1, and rejecting that is a UX
    /// regression. Each entry is a serialized authority
    /// (`<host>:<port>`); no path/scheme parts.
    accepted_hosts: HashSet<String>,
    /// Origin strings the middleware accepts on POSTs. Mirrors
    /// `accepted_hosts` but with `http://` prefixed. Built once at
    /// startup; the middleware does an O(1) hash lookup.
    accepted_origins: HashSet<String>,
}

type SharedState = Arc<ServeState>;

async fn teardown_current(state: &SharedState) {
    let taken = state.current.lock().await.take();
    if let Some((session, attach)) = taken {
        let _ = tokio::time::timeout(Duration::from_secs(2), attach.close()).await;
        let http = state.http.clone();
        let id = session.id.clone();
        state.created_ids.lock().await.remove(&id);
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
    s.created_ids.lock().await.insert(session.id.clone());
    *s.current.lock().await = Some((session, attach));
    Ok(Json(resp))
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
/// delete every session THIS notebook process previously created
/// via `/api/create`. Used by the Playwright `beforeEach` to start
/// each test from a known empty state — also catches leftovers
/// from a prior test that crashed before its own `/api/close`.
///
/// Critically, this is keyed by an id-set the notebook itself
/// populates on every `/api/create`, NOT a prefix-match on the
/// session-name namespace. Production sipag's `dispatch` path
/// creates sessions in the same `sipag-d-<hex>` namespace as the
/// notebook does; an earlier iteration of this handler swept by
/// prefix and would have hard-killed in-flight production
/// dispatches if the operator pointed the notebook at a shared
/// katulong. The notebook-owned id-set rules that out.
async fn api_reset(State(s): State<SharedState>) -> ApiResult<Json<OkResp>> {
    teardown_current(&s).await;
    // Drain the id-set so subsequent /api/reset calls don't try to
    // kill ids we already killed (and so the set doesn't grow
    // unboundedly across a long-lived serve).
    let ids: Vec<String> = s.created_ids.lock().await.drain().collect();
    let http = s.http.clone();
    tokio::task::spawn_blocking(move || {
        for id in ids {
            let _ = http.kill_session(&id);
        }
    })
    .await
    .map_err(|e| ApiError::internal(format!("reset sweep join: {e}")))?;
    Ok(Json(OkResp { ok: true }))
}

// ── request guard ───────────────────────────────────────────────

/// Defends the local notebook server against:
///
/// 1. **DNS rebinding.** A browser tab on `attacker.example` whose DNS
///    resolves first to a public IP then rebinds to `127.0.0.1` can
///    issue requests that reach a localhost server. The `Host:` header
///    is the only attacker-controlled value that has to match the
///    legitimate authority — we reject anything else.
///
/// 2. **CSRF.** A page on another origin can issue a no-body POST as a
///    "simple request" with no preflight (e.g.,
///    `fetch('http://127.0.0.1:8765/api/create', {method:'POST'})`).
///    We reject POSTs whose `Origin:` doesn't match our local
///    authority. Browsers ALWAYS attach `Origin:` to POSTs (even
///    no-cors), so a malicious page can't fake its absence to slip
///    through. POSTs with NO `Origin:` are allowed: those come from
///    non-browser clients (curl, Playwright's API request context,
///    sipag's CLI) which aren't a CSRF vector.
///
/// GETs aren't state-changing but the Host check still applies, so a
/// rebound origin can't read state or session lists either.
async fn local_request_guard(
    State(state): State<SharedState>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let host = req.headers().get("host").and_then(|v| v.to_str().ok());
    let origin = req.headers().get("origin").and_then(|v| v.to_str().ok());
    check_local_request(
        &state.accepted_hosts,
        &state.accepted_origins,
        req.method(),
        host,
        origin,
    )?;
    Ok(next.run(req).await)
}

/// Pure policy decision behind `local_request_guard`. Returns `Ok` if
/// the request should be allowed through, `Err(FORBIDDEN)` otherwise.
/// Extracted so the negative paths are testable without an axum
/// `Router` + `tower::Service` rig.
fn check_local_request(
    accepted_hosts: &HashSet<String>,
    accepted_origins: &HashSet<String>,
    method: &Method,
    host: Option<&str>,
    origin: Option<&str>,
) -> Result<(), StatusCode> {
    // DNS-rebinding defense: `Host:` MUST be present and MUST be
    // one of the accepted authorities (127.0.0.1, localhost, [::1]
    // — all at our port).
    match host {
        Some(h) if accepted_hosts.contains(h) => {}
        _ => return Err(StatusCode::FORBIDDEN),
    }
    // CSRF defense for state-changing POSTs: if `Origin:` is present,
    // it MUST be in the accepted set. Origin-less POSTs are non-
    // browser callers (curl, Playwright's API context, sipag's CLI)
    // and aren't a CSRF vector.
    if method == Method::POST {
        if let Some(origin) = origin {
            if !accepted_origins.contains(origin) {
                return Err(StatusCode::FORBIDDEN);
            }
        }
    }
    Ok(())
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

// ── tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_key_maps_canonical_names() {
        assert!(matches!(parse_key("enter"), Ok(KeyName::Enter)));
        assert!(matches!(parse_key("ENTER"), Ok(KeyName::Enter)));
        assert!(matches!(parse_key("Return"), Ok(KeyName::Enter)));
        assert!(matches!(parse_key("escape"), Ok(KeyName::Escape)));
        assert!(matches!(parse_key("esc"), Ok(KeyName::Escape)));
        assert!(matches!(parse_key("tab"), Ok(KeyName::Tab)));
        assert!(matches!(parse_key("backspace"), Ok(KeyName::Backspace)));
        assert!(matches!(parse_key("bs"), Ok(KeyName::Backspace)));
        assert!(matches!(parse_key("ctrl-c"), Ok(KeyName::CtrlC)));
        assert!(matches!(parse_key("ctrlc"), Ok(KeyName::CtrlC)));
        assert!(matches!(parse_key("^c"), Ok(KeyName::CtrlC)));
        assert!(matches!(parse_key("ctrl-d"), Ok(KeyName::CtrlD)));
        assert!(matches!(parse_key("up"), Ok(KeyName::Up)));
        assert!(matches!(parse_key("down"), Ok(KeyName::Down)));
        assert!(matches!(parse_key("left"), Ok(KeyName::Left)));
        assert!(matches!(parse_key("right"), Ok(KeyName::Right)));
    }

    #[test]
    fn parse_key_rejects_unknown() {
        let err = parse_key("f12").unwrap_err();
        assert!(
            err.contains("unknown key 'f12'"),
            "error message should name the rejected key: {err}"
        );
        assert!(
            err.contains("enter"),
            "error message should hint at the legal set: {err}"
        );
    }

    #[test]
    fn parse_wait_from_maps_canonical_names() {
        assert!(matches!(parse_wait_from("now"), Ok(WaitFrom::FromNow)));
        assert!(matches!(
            parse_wait_from("attach"),
            Ok(WaitFrom::FromAttach)
        ));
        assert!(matches!(
            parse_wait_from("from-attach"),
            Ok(WaitFrom::FromAttach)
        ));
    }

    #[test]
    fn parse_wait_from_parses_numeric_offsets() {
        let v = parse_wait_from("0").unwrap();
        assert!(matches!(v, WaitFrom::FromOffset(0)));
        let v = parse_wait_from("4096").unwrap();
        assert!(matches!(v, WaitFrom::FromOffset(4096)));
    }

    #[test]
    fn parse_wait_from_rejects_garbage() {
        let err = parse_wait_from("yesterday").unwrap_err();
        assert!(
            err.contains("yesterday"),
            "error message should name the rejected value: {err}"
        );
        assert!(
            err.contains("'now'"),
            "error message should hint at the legal set: {err}"
        );
    }

    // The middleware is exercised end-to-end by every Playwright
    // test (the notebook UI hits /api/* through browser fetch with
    // a matching Origin, the diagnostic tests hit /api/* through
    // Playwright's API context with no Origin), but the negative
    // paths deserve a unit-level guard so a regression loosening the
    // check fails fast in `cargo test`.
    fn test_acceptlists() -> (HashSet<String>, HashSet<String>) {
        let hosts: HashSet<String> = [
            "127.0.0.1:8765".to_string(),
            "localhost:8765".to_string(),
            "[::1]:8765".to_string(),
        ]
        .into_iter()
        .collect();
        let origins: HashSet<String> = hosts.iter().map(|h| format!("http://{h}")).collect();
        (hosts, origins)
    }

    #[test]
    fn guard_rejects_post_with_foreign_origin() {
        let (h, o) = test_acceptlists();
        assert_eq!(
            check_local_request(
                &h,
                &o,
                &Method::POST,
                Some("127.0.0.1:8765"),
                Some("http://attacker.example")
            ),
            Err(StatusCode::FORBIDDEN)
        );
    }

    #[test]
    fn guard_accepts_post_with_matching_origin() {
        let (h, o) = test_acceptlists();
        assert!(check_local_request(
            &h,
            &o,
            &Method::POST,
            Some("127.0.0.1:8765"),
            Some("http://127.0.0.1:8765")
        )
        .is_ok());
    }

    #[test]
    fn guard_accepts_post_with_no_origin() {
        // Non-browser callers (curl, Playwright's API context, sipag's
        // CLI when it calls /api/* directly) don't set Origin, and they
        // aren't a CSRF vector — allow them through.
        let (h, o) = test_acceptlists();
        assert!(check_local_request(&h, &o, &Method::POST, Some("127.0.0.1:8765"), None).is_ok());
    }

    #[test]
    fn guard_rejects_request_with_wrong_host() {
        let (h, o) = test_acceptlists();
        assert_eq!(
            check_local_request(&h, &o, &Method::GET, Some("attacker.example:8765"), None),
            Err(StatusCode::FORBIDDEN)
        );
    }

    #[test]
    fn guard_rejects_request_with_no_host() {
        let (h, o) = test_acceptlists();
        assert_eq!(
            check_local_request(&h, &o, &Method::GET, None, None),
            Err(StatusCode::FORBIDDEN)
        );
    }

    #[test]
    fn guard_accepts_localhost_host_and_origin() {
        // Operators routinely type `localhost:<port>` instead of the
        // 127.0.0.1 form the startup banner prints. The middleware
        // must accept both.
        let (h, o) = test_acceptlists();
        assert!(check_local_request(
            &h,
            &o,
            &Method::POST,
            Some("localhost:8765"),
            Some("http://localhost:8765")
        )
        .is_ok());
        assert!(check_local_request(&h, &o, &Method::GET, Some("localhost:8765"), None).is_ok());
    }

    #[test]
    fn guard_rejects_empty_host() {
        // Pin the empty-string-vs-Some(_) check so a future
        // unwrap_or("") refactor on the parsed header doesn't
        // accidentally let it through.
        let (h, o) = test_acceptlists();
        assert_eq!(
            check_local_request(&h, &o, &Method::GET, Some(""), None),
            Err(StatusCode::FORBIDDEN)
        );
    }

    #[test]
    fn guard_rejects_origin_port_elision() {
        // `Origin: http://127.0.0.1` (no port) is what a browser
        // would send if serve ran on port 80. Pin the strict match
        // so a future "support port 80" PR can't loosen it
        // accidentally.
        let (h, o) = test_acceptlists();
        assert_eq!(
            check_local_request(
                &h,
                &o,
                &Method::POST,
                Some("127.0.0.1:8765"),
                Some("http://127.0.0.1")
            ),
            Err(StatusCode::FORBIDDEN)
        );
    }

    #[test]
    fn guard_rejects_https_origin_against_http_serve() {
        // Future "https serve" support must not be a side-effect of
        // browser-side https — the operator must opt in deliberately.
        let (h, o) = test_acceptlists();
        assert_eq!(
            check_local_request(
                &h,
                &o,
                &Method::POST,
                Some("127.0.0.1:8765"),
                Some("https://127.0.0.1:8765")
            ),
            Err(StatusCode::FORBIDDEN)
        );
    }
}
