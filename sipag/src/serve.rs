//! Week-1 spike: `sipag serve` — the agent-manager backplane.
//!
//! An axum server that:
//!   - loads `~/.sipag/hosts.toml` and keeps the API keys in memory
//!     (so the browser never sees them)
//!   - exposes `GET /api/hosts` → `[{id, url}]` (no keys)
//!   - proxies `GET /api/hosts/:id/crew/*` to that host's katulong with
//!     `Authorization: Bearer <apiKey>` (and `POST` / `DELETE` when we
//!     need them; the spike only wires `GET` for read-only browsing)
//!   - serves the ClojureScript SPA from `./web/public/`
//!
//! This is the Booster-4 version: single binary, disk-based assets, no
//! SSE yet, no dispatch yet. Enough to prove that a browser-served cljs
//! UI can render live crew state across three real katulongs.

use anyhow::{Context, Result};
use axum::{
    extract::{Path as AxumPath, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json, Response},
    routing::{delete, get, patch, post},
    Router,
};
use serde::{Deserialize, Serialize};
use sipag_core::auth::{
    random_token, sessions_dir, Credential, Session, SetupPurpose, SetupToken, User,
};
use sipag_core::board::{
    add_task, create_project_with_kind, delete_project, list_project_names, list_tasks,
    load_project, move_task, KeyResult, KrStance, ProjectKind, Role, Task,
};
use sipag_core::config::default_sipag_dir;
use sipag_core::hosts::{default_hosts_path, HostsConfig};
use sipag_core::katulong::session_name;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tower_cookies::{cookie::SameSite, Cookie, CookieManagerLayer, Cookies};
use tower_http::services::ServeDir;
use tracing::{info, warn};
use webauthn_rs::prelude::*;
use webauthn_rs::Webauthn;

#[derive(Clone)]
pub(crate) struct AppState {
    hosts: Arc<HostsConfig>,
    http: reqwest::Client,
    /// Sipag data dir — `~/.sipag` by default, overridable for tests
    /// via `SIPAG_DIR`. New handlers (auth) read this; older board
    /// handlers still use `default_sipag_dir()` directly until they
    /// earn a refactor.
    pub(crate) sipag_dir: std::path::PathBuf,
    /// External base URL for minting links the user opens in a
    /// browser (setup-token URLs, the eventual install redirects).
    /// `SIPAG_PUBLIC_URL` env var > `http://localhost:<port>` default.
    pub(crate) public_url: String,
    /// WebAuthn relying-party context. Built from `public_url` at
    /// startup; rp_id is the URL's host, origin is the URL itself.
    webauthn: Arc<Webauthn>,
    /// Server-side state for in-flight registration ceremonies. Keyed
    /// by a fresh state_id minted at register/begin and echoed by the
    /// client at register/finish. Carries the setup_token so finish
    /// can consume it on success.
    pending_register: Arc<Mutex<HashMap<String, PendingRegister>>>,
    /// Server-side state for in-flight authentication ceremonies.
    /// Keyed by a fresh state_id minted at assert/begin.
    pending_auth: Arc<Mutex<HashMap<String, PasskeyAuthentication>>>,
}

struct PendingRegister {
    state: PasskeyRegistration,
    setup_token: String,
}

/// Build a `Webauthn` from a public URL like `http://localhost:7100`
/// or `https://sipag.felixflor.es`. RP ID is the URL's host, origin
/// is the URL itself.
pub(crate) fn build_webauthn(public_url: &str) -> Result<Webauthn> {
    let url = ::url::Url::parse(public_url)
        .with_context(|| format!("invalid public_url: {public_url}"))?;
    let rp_id = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("public_url has no host: {public_url}"))?
        .to_string();
    let webauthn = WebauthnBuilder::new(&rp_id, &url)
        .context("WebauthnBuilder::new failed")?
        .rp_name("Sipag")
        .build()
        .context("WebauthnBuilder::build failed")?;
    Ok(webauthn)
}

/// Construct a router pointing at the given sipag dir, with empty
/// hosts and a stub web_root. For integration tests that want to
/// exercise routes without binding a real port and without touching
/// the user's `~/.sipag`.
pub fn build_test_router(
    sipag_dir: std::path::PathBuf,
    public_url: String,
) -> Router {
    let webauthn = Arc::new(
        build_webauthn(&public_url).expect("build_webauthn for tests"),
    );
    let state = AppState {
        hosts: Arc::new(HostsConfig::default()),
        http: reqwest::Client::new(),
        sipag_dir,
        public_url,
        webauthn,
        pending_register: Arc::new(Mutex::new(HashMap::new())),
        pending_auth: Arc::new(Mutex::new(HashMap::new())),
    };
    // Tests don't need a real web_root — point at a directory that
    // exists (the project root) so ServeDir doesn't panic on init.
    // Real HTML responses come from the routes, not from static files.
    let web_root = std::env::current_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."));
    build_router(state, web_root)
}

/// Build the axum router for tests and `serve`. Kept separate from
/// `async_run` so integration tests can hand-construct an `AppState`
/// pointing at a tempdir and exercise the routes without binding a
/// real port.
pub(crate) fn build_router(state: AppState, web_root: std::path::PathBuf) -> Router {
    let auth_state = state.clone();
    Router::new()
        .route("/api/hosts", get(list_hosts))
        .route("/api/hosts/:id/sessions", get(proxy_sessions))
        .route(
            "/api/hosts/:id/sessions/by-id/:sid/status",
            get(proxy_session_status),
        )
        .route(
            "/api/projects",
            get(list_projects).post(create_project_handler),
        )
        .route("/api/projects/:name", delete(delete_project_handler))
        .route(
            "/api/projects/:name/key-results",
            post(create_kr_handler),
        )
        .route(
            "/api/projects/:name/key-results/:id",
            patch(update_kr_handler).delete(delete_kr_handler),
        )
        .route("/api/projects/:name/tasks", post(create_task_handler))
        .route(
            "/api/projects/:name/tasks/:id",
            patch(update_task_handler).delete(delete_task_handler),
        )
        .route(
            "/api/projects/:name/tasks/:id/dispatch",
            post(dispatch_task_handler),
        )
        // Track A — passkey enrollment bootstrap.
        .route("/setup", get(setup_get_handler))
        .route("/login", get(login_get_handler))
        // Track A — WebAuthn ceremony.
        .route("/api/auth/register/begin", post(register_begin))
        .route("/api/auth/register/finish", post(register_finish))
        .route("/api/auth/assert/begin", post(assert_begin))
        .route("/api/auth/assert/finish", post(assert_finish))
        .route("/api/auth/logout", post(logout_handler))
        .fallback_service(ServeDir::new(&web_root).append_index_html_on_directories(true))
        .layer(axum::middleware::from_fn_with_state(
            auth_state,
            auth_middleware,
        ))
        .layer(CookieManagerLayer::new())
        .with_state(state)
}

// ── auth middleware ──────────────────────────────────────────────────
//
// Decides whether an inbound request reaches the route handler:
//
//   1. Public path? (login / setup / auth ceremony / static assets)
//      → through, no checks.
//   2. `Host` header is localhost / 127.0.0.1 / ::1?
//      → through (dev convenience). The Host header survives
//        cloudflared end-to-end, so a tunneled request carries
//        the public hostname (e.g. sipag.felixflor.es) and falls
//        out of this branch.
//   3. Has a `sipag_session` cookie pointing at a non-expired
//      Session record? → through, refresh `last_active`.
//   4. Otherwise:
//      - /api/* → 401 JSON
//      - else  → 302 → /login

fn is_public_path(path: &str) -> bool {
    if path.starts_with("/api/auth/") {
        return true;
    }
    matches!(path, "/login" | "/setup" | "/style.css" | "/favicon.ico")
        || path.starts_with("/js/")
}

fn host_header_is_local(headers: &axum::http::HeaderMap) -> bool {
    let Some(host) = headers.get(axum::http::header::HOST).and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    let hostname = host.split(':').next().unwrap_or("").trim().to_ascii_lowercase();
    matches!(hostname.as_str(), "localhost" | "127.0.0.1" | "::1" | "[::1]")
}

async fn auth_middleware(
    State(state): State<AppState>,
    cookies: Cookies,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let path = request.uri().path().to_string();

    if is_public_path(&path) || host_header_is_local(request.headers()) {
        return next.run(request).await;
    }

    // Session cookie path.
    if let Some(cookie) = cookies.get(SESSION_COOKIE) {
        let token = cookie.value().to_string();
        if let Ok(mut session) = Session::load(&state.sipag_dir, &token) {
            if !session.is_expired() {
                // Sliding expiry — touch best-effort.
                session.touch();
                let _ = session.save(&state.sipag_dir);
                return next.run(request).await;
            }
        }
    }

    // Not authenticated.
    if path.starts_with("/api/") {
        json_error(
            StatusCode::UNAUTHORIZED,
            "auth.required",
            "Sign in at /login to use this endpoint.",
        )
    } else {
        axum::response::Redirect::to("/login").into_response()
    }
}

/// Entry point — called from the `Serve` CLI branch.
pub fn run(port: u16, web_root: std::path::PathBuf) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime")?;
    runtime.block_on(async_run(port, web_root))
}

async fn async_run(port: u16, web_root: std::path::PathBuf) -> Result<()> {
    // Bring up tracing if the user hasn't already.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("sipag=info,tower_http=info")),
        )
        .try_init();

    let hosts_path = default_hosts_path();
    let hosts = HostsConfig::load().with_context(|| {
        format!("failed to load hosts from {}", hosts_path.display())
    })?;

    if hosts.hosts.is_empty() {
        warn!(
            "no hosts configured — create {} (see extras/hosts.toml.example)",
            hosts_path.display()
        );
    } else {
        info!("loaded {} host(s): {:?}", hosts.hosts.len(),
              hosts.hosts.iter().map(|h| &h.id).collect::<Vec<_>>());
    }

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("failed to build reqwest client")?;

    let public_url = std::env::var("SIPAG_PUBLIC_URL")
        .unwrap_or_else(|_| format!("http://localhost:{port}"));

    let webauthn =
        Arc::new(build_webauthn(&public_url).context("WebAuthn init failed")?);

    let state = AppState {
        hosts: Arc::new(hosts),
        http,
        sipag_dir: sipag_core::config::default_sipag_dir(),
        public_url,
        webauthn,
        pending_register: Arc::new(Mutex::new(HashMap::new())),
        pending_auth: Arc::new(Mutex::new(HashMap::new())),
    };

    let app = build_router(state, web_root.clone());

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    info!("sipag serve listening on http://{} (web root: {})", addr, web_root.display());

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind {}", addr))?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server error")?;
    Ok(())
}

async fn shutdown_signal() {
    use tokio::signal;
    let ctrl_c = async {
        let _ = signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        let mut s = signal::unix::signal(signal::unix::SignalKind::terminate()).unwrap();
        s.recv().await;
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = term => {},
    }
    info!("shutting down");
}

// ── handlers ─────────────────────────────────────────────────────────────

/// Public view of a configured host. Deliberately does not include apiKey.
#[derive(Serialize)]
struct HostSummary {
    id: String,
    url: String,
}

// ── board handlers ────────────────────────────────────────────────────

#[derive(Serialize)]
struct TaskView {
    id: u64,
    title: String,
    status: String,
    role: String,
    labels: Vec<String>,
    key_results: Vec<u64>,
    created: String,
    updated: String,
}

impl From<Task> for TaskView {
    fn from(t: Task) -> Self {
        Self {
            id: t.id,
            title: t.title,
            status: t.status.to_string(),
            role: t.role,
            labels: t.labels,
            key_results: t.key_results,
            created: t.created,
            updated: t.updated,
        }
    }
}

#[derive(Serialize)]
struct KrView {
    id: u64,
    title: String,
    stance: String,
    created: String,
}

impl From<KeyResult> for KrView {
    fn from(k: KeyResult) -> Self {
        Self {
            id: k.id,
            title: k.title,
            stance: k.stance.to_string(),
            created: k.created,
        }
    }
}

#[derive(Serialize)]
struct ProjectView {
    name: String,
    repo: String,
    kind: String,
    statuses: Vec<String>,
    key_results: Vec<KrView>,
    tasks: Vec<TaskView>,
}

async fn list_projects() -> Response {
    let dir = default_sipag_dir();
    let names = match list_project_names(&dir) {
        Ok(n) => n,
        Err(e) => {
            warn!("list_project_names failed: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("board error: {e}"),
            )
                .into_response();
        }
    };

    let mut out: Vec<ProjectView> = Vec::with_capacity(names.len());
    for name in names {
        let project = match load_project(&dir, &name) {
            Ok(p) => p,
            Err(e) => {
                warn!("load_project({}) failed: {}", name, e);
                continue;
            }
        };
        let tasks = list_tasks(&dir, &name, None).unwrap_or_default();
        let krs = KeyResult::list(&dir, &name).unwrap_or_default();
        out.push(ProjectView {
            name: project.name,
            repo: project.repo,
            kind: match project.kind {
                ProjectKind::Objective => "objective".into(),
                ProjectKind::Standing => "standing".into(),
            },
            statuses: project.statuses,
            key_results: krs.into_iter().map(KrView::from).collect(),
            tasks: tasks.into_iter().map(TaskView::from).collect(),
        });
    }
    Json(out).into_response()
}

// ── write endpoints ───────────────────────────────────────────────────

#[derive(Deserialize)]
struct CreateProjectBody {
    name: String,
    #[serde(default)]
    repo: String,
    /// "objective" or "standing"; defaults to "objective".
    #[serde(default)]
    kind: Option<String>,
}

async fn create_project_handler(Json(body): Json<CreateProjectBody>) -> Response {
    let dir = default_sipag_dir();
    let kind = match body.kind.as_deref().unwrap_or("objective") {
        "objective" => ProjectKind::Objective,
        "standing" => ProjectKind::Standing,
        other => {
            return (
                StatusCode::BAD_REQUEST,
                format!("unknown kind: {other}"),
            )
                .into_response()
        }
    };
    if body.name.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "name is required").into_response();
    }
    match create_project_with_kind(&dir, &body.name, &body.repo, kind, None) {
        Ok(p) => Json(ProjectView {
            name: p.name,
            repo: p.repo,
            kind: match p.kind {
                ProjectKind::Objective => "objective".into(),
                ProjectKind::Standing => "standing".into(),
            },
            statuses: p.statuses,
            key_results: vec![],
            tasks: vec![],
        })
        .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    }
}

#[derive(Deserialize)]
struct CreateKrBody {
    title: String,
}

async fn create_kr_handler(
    AxumPath(name): AxumPath<String>,
    Json(body): Json<CreateKrBody>,
) -> Response {
    let dir = default_sipag_dir();
    if body.title.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "title is required").into_response();
    }
    if load_project(&dir, &name).is_err() {
        return (StatusCode::NOT_FOUND, format!("project '{name}' not found")).into_response();
    }
    let id = match KeyResult::next_id(&dir, &name) {
        Ok(n) => n,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    };
    let kr = KeyResult {
        id,
        title: body.title,
        stance: KrStance::Green,
        created: chrono::Utc::now()
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string(),
    };
    if let Err(e) = kr.save(&dir, &name) {
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response();
    }
    Json(KrView::from(kr)).into_response()
}

#[derive(Deserialize)]
struct UpdateKrBody {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    stance: Option<String>,
}

async fn update_kr_handler(
    AxumPath((name, id)): AxumPath<(String, u64)>,
    Json(body): Json<UpdateKrBody>,
) -> Response {
    let dir = default_sipag_dir();
    let mut kr = match KeyResult::load(&dir, &name, id) {
        Ok(k) => k,
        Err(_) => return (StatusCode::NOT_FOUND, "KR not found").into_response(),
    };
    if let Some(t) = body.title {
        kr.title = t;
    }
    if let Some(s) = body.stance {
        match KrStance::parse(&s) {
            Some(parsed) => kr.stance = parsed,
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    format!("unknown stance: {s} (expected green|yellow|red|done)"),
                )
                    .into_response()
            }
        }
    }
    if let Err(e) = kr.save(&dir, &name) {
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response();
    }
    Json(KrView::from(kr)).into_response()
}

#[derive(Deserialize)]
struct CreateTaskBody {
    title: String,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    labels: Vec<String>,
    #[serde(default)]
    key_results: Vec<u64>,
}

async fn create_task_handler(
    AxumPath(name): AxumPath<String>,
    Json(body): Json<CreateTaskBody>,
) -> Response {
    let dir = default_sipag_dir();
    if body.title.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "title is required").into_response();
    }
    let mut task = match add_task(&dir, &name, &body.title, body.role.as_deref(), &body.labels) {
        Ok(t) => t,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    };
    if !body.key_results.is_empty() {
        task.key_results = body.key_results;
        if let Err(e) = task.save(&dir, &name) {
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response();
        }
    }
    Json(TaskView::from(task)).into_response()
}

#[derive(Deserialize)]
struct UpdateTaskBody {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    labels: Option<Vec<String>>,
    #[serde(default)]
    key_results: Option<Vec<u64>>,
}

// ── auth: setup-token enrollment ──────────────────────────────────────
//
// First-passkey bootstrap. The CLI mints a single-use token and prints
// a URL like `http://localhost:7100/setup?token=<hex>`. The user opens
// that URL on a trusted device, the token is consumed once, and the
// page renders a register-passkey UI (which talks to the WebAuthn
// register/begin + register/finish endpoints — wired in task #36).
//
// Errors return HTML status pages so the user sees something useful
// when they paste an old / expired / wrong-purpose token.

#[derive(Deserialize)]
struct SetupQuery {
    #[serde(default)]
    token: Option<String>,
}

async fn setup_get_handler(
    State(state): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<SetupQuery>,
) -> Response {
    let Some(token) = q.token.filter(|t| !t.is_empty()) else {
        return setup_error_page(
            StatusCode::NOT_FOUND,
            "missing token",
            "This URL needs a `?token=…` parameter. Run `sipag setup-token` to mint one.",
        );
    };

    // We *don't* consume here — that happens during register/finish so
    // a stale tab can't burn the token before the user actually clicks.
    let stored = match SetupToken::load(&state.sipag_dir, &token) {
        Ok(t) => t,
        Err(_) => {
            return setup_error_page(
                StatusCode::NOT_FOUND,
                "unknown token",
                "This setup token isn't recognized. It may have already been used; mint a fresh one with `sipag setup-token`.",
            );
        }
    };
    if stored.purpose != SetupPurpose::EnrollPasskey {
        return setup_error_page(
            StatusCode::GONE,
            "wrong purpose",
            "This setup token is for a different flow. For first-passkey enrollment, run `sipag setup-token`.",
        );
    }
    if stored.is_expired() {
        return setup_error_page(
            StatusCode::GONE,
            "token expired",
            "This setup token has expired. Mint a fresh one with `sipag setup-token` (10-minute window).",
        );
    }

    setup_register_page(&token)
}

fn setup_register_page(token: &str) -> Response {
    let html = format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Register your passkey · sipag</title>
<style>
  body {{ font: 15px/1.5 ui-sans-serif, system-ui, -apple-system, sans-serif;
         background: #0e1013; color: #e6e8eb;
         max-width: 460px; margin: 80px auto; padding: 24px; }}
  h1 {{ font-size: 18px; font-weight: 600; margin: 0 0 12px; }}
  p  {{ color: #9aa3ae; margin: 0 0 16px; }}
  label {{ display: block; color: #9aa3ae; margin-bottom: 4px; font-size: 13px; }}
  input {{ width: 100%; background: #1a1f26; border: 1px solid #262c34;
           color: #e6e8eb; padding: 8px 10px; border-radius: 4px;
           font: inherit; margin-bottom: 16px; box-sizing: border-box; }}
  input:focus {{ border-color: #7aa2f7; outline: none; }}
  button {{ background: #7aa2f7; color: #0e1013; border: none;
            padding: 10px 18px; border-radius: 6px; font: inherit;
            font-size: 14px; cursor: pointer; }}
  button:hover {{ filter: brightness(1.1); }}
  button:disabled {{ opacity: 0.5; cursor: progress; }}
  .err {{ color: #f7768e; margin-top: 12px;
          font-family: ui-monospace, monospace; font-size: 13px;
          word-break: break-word; }}
</style>
</head>
<body>
<h1>Register your passkey</h1>
<p>You're enrolling the first passkey for this sipag. Use your device's biometric or hardware key.</p>
<label for="label">Label (optional)</label>
<input id="label" placeholder="e.g. felix-iphone" autocomplete="off">
<button id="register">Register passkey</button>
<div class="err" id="err"></div>
<script>
  const TOKEN = {token_json};
  const btn = document.getElementById('register');
  const errEl = document.getElementById('err');
  const labelEl = document.getElementById('label');

  function b64urlToBytes(s) {{
    s = s.replace(/-/g, '+').replace(/_/g, '/');
    while (s.length % 4) s += '=';
    const bin = atob(s);
    const bytes = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
    return bytes;
  }}
  function bytesToB64url(buf) {{
    const bytes = new Uint8Array(buf);
    let s = '';
    for (const b of bytes) s += String.fromCharCode(b);
    return btoa(s).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  }}

  btn.addEventListener('click', async () => {{
    btn.disabled = true;
    errEl.textContent = '';
    try {{
      const beginResp = await fetch('/api/auth/register/begin', {{
        method: 'POST',
        headers: {{ 'Content-Type': 'application/json' }},
        body: JSON.stringify({{ setup_token: TOKEN }}),
      }});
      if (!beginResp.ok) {{
        const t = await beginResp.text();
        throw new Error('register/begin: HTTP ' + beginResp.status + ' ' + t);
      }}
      const {{ state_id, challenge }} = await beginResp.json();

      const opts = challenge.publicKey;
      opts.challenge = b64urlToBytes(opts.challenge);
      opts.user.id = b64urlToBytes(opts.user.id);
      if (Array.isArray(opts.excludeCredentials)) {{
        for (const c of opts.excludeCredentials) c.id = b64urlToBytes(c.id);
      }}

      const cred = await navigator.credentials.create({{ publicKey: opts }});

      const credentialJson = {{
        id: cred.id,
        rawId: bytesToB64url(cred.rawId),
        type: cred.type,
        response: {{
          attestationObject: bytesToB64url(cred.response.attestationObject),
          clientDataJSON: bytesToB64url(cred.response.clientDataJSON),
        }},
        extensions: cred.getClientExtensionResults ? cred.getClientExtensionResults() : {{}},
      }};

      const finResp = await fetch('/api/auth/register/finish', {{
        method: 'POST',
        headers: {{ 'Content-Type': 'application/json' }},
        body: JSON.stringify({{
          state_id,
          credential: credentialJson,
          label: labelEl.value || '',
        }}),
      }});
      if (!finResp.ok) {{
        const t = await finResp.text();
        throw new Error('register/finish: HTTP ' + finResp.status + ' ' + t);
      }}

      window.location = '/';
    }} catch (e) {{
      btn.disabled = false;
      errEl.textContent = String(e && e.message || e);
    }}
  }});
</script>
</body>
</html>"#,
        token_json = serde_json::to_string(token).unwrap_or_else(|_| "\"\"".to_string()),
    );
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html,
    )
        .into_response()
}

fn setup_error_page(status: StatusCode, heading: &str, body: &str) -> Response {
    let html = format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>{heading} · sipag</title>
<style>
  body {{ font: 15px/1.5 ui-sans-serif, system-ui, -apple-system, sans-serif;
         background: #0e1013; color: #e6e8eb;
         max-width: 460px; margin: 80px auto; padding: 24px; }}
  h1 {{ font-size: 18px; font-weight: 600; margin: 0 0 12px; color: #f7768e; }}
  p  {{ color: #9aa3ae; margin: 0; }}
  code {{ background: #14181d; padding: 1px 6px; border-radius: 3px;
          font-family: ui-monospace, monospace; font-size: 13px; }}
</style>
</head>
<body>
<h1>{heading}</h1>
<p>{body}</p>
</body>
</html>"#,
        heading = html_escape(heading),
        body = html_escape(body),
    );
    (
        status,
        [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html,
    )
        .into_response()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

// ── auth: /login HTML ────────────────────────────────────────────────
//
// Branches on whether any credentials are enrolled:
//   - empty store → directs user to `sipag setup-token` for first
//     enrollment
//   - with credentials → "Sign in with passkey" UI that drives the
//     assert/begin → navigator.credentials.get() → assert/finish flow

async fn login_get_handler(State(state): State<AppState>) -> Response {
    let has_creds = Credential::list(&state.sipag_dir)
        .map(|c| !c.is_empty())
        .unwrap_or(false);
    let html = if has_creds {
        login_signin_page()
    } else {
        login_empty_page()
    };
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html,
    )
        .into_response()
}

fn login_empty_page() -> String {
    r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Sign in · sipag</title>
<style>
  body { font: 15px/1.5 ui-sans-serif, system-ui, -apple-system, sans-serif;
         background: #0e1013; color: #e6e8eb;
         max-width: 460px; margin: 80px auto; padding: 24px; }
  h1 { font-size: 18px; font-weight: 600; margin: 0 0 12px; }
  p  { color: #9aa3ae; margin: 0 0 16px; }
  pre { background: #14181d; padding: 12px; border-radius: 6px;
        font-family: ui-monospace, monospace; font-size: 13px;
        overflow-x: auto; margin: 0; }
  code { color: #7aa2f7; }
</style>
</head>
<body>
<h1>No passkeys yet</h1>
<p>This sipag instance hasn't been bootstrapped. On the host running sipag, mint a setup token and open the printed URL on a trusted device:</p>
<pre><code>sipag setup-token</code></pre>
</body>
</html>"#
        .to_string()
}

fn login_signin_page() -> String {
    // The whole flow:
    //   1. POST /api/auth/assert/begin          → { state_id, challenge }
    //   2. navigator.credentials.get(challenge) → PublicKeyCredential
    //   3. POST /api/auth/assert/finish         → { ok, credential_id } + cookie
    //   4. window.location = '/'
    //
    // Base64URL ↔ ArrayBuffer helpers inlined so the page is
    // dependency-free.
    r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Sign in · sipag</title>
<style>
  body { font: 15px/1.5 ui-sans-serif, system-ui, -apple-system, sans-serif;
         background: #0e1013; color: #e6e8eb;
         max-width: 460px; margin: 80px auto; padding: 24px; }
  h1 { font-size: 18px; font-weight: 600; margin: 0 0 12px; }
  p  { color: #9aa3ae; margin: 0 0 16px; }
  button { background: #7aa2f7; color: #0e1013; border: none;
           padding: 10px 18px; border-radius: 6px; font: inherit;
           font-size: 14px; cursor: pointer; }
  button:hover { filter: brightness(1.1); }
  button:disabled { opacity: 0.5; cursor: progress; }
  .err { color: #f7768e; margin-top: 12px;
         font-family: ui-monospace, monospace; font-size: 13px;
         word-break: break-word; }
</style>
</head>
<body>
<h1>Sign in</h1>
<p>Use the passkey enrolled with this sipag.</p>
<button id="signin">Sign in with passkey</button>
<div class="err" id="err"></div>
<script>
  const btn = document.getElementById('signin');
  const errEl = document.getElementById('err');

  function b64urlToBytes(s) {
    s = s.replace(/-/g, '+').replace(/_/g, '/');
    while (s.length % 4) s += '=';
    const bin = atob(s);
    const bytes = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
    return bytes;
  }
  function bytesToB64url(buf) {
    const bytes = new Uint8Array(buf);
    let s = '';
    for (const b of bytes) s += String.fromCharCode(b);
    return btoa(s).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  }

  btn.addEventListener('click', async () => {
    btn.disabled = true;
    errEl.textContent = '';
    try {
      const beginResp = await fetch('/api/auth/assert/begin', { method: 'POST' });
      if (!beginResp.ok) throw new Error('assert/begin: HTTP ' + beginResp.status);
      const { state_id, challenge } = await beginResp.json();

      const opts = challenge.publicKey;
      opts.challenge = b64urlToBytes(opts.challenge);
      if (Array.isArray(opts.allowCredentials)) {
        for (const c of opts.allowCredentials) c.id = b64urlToBytes(c.id);
      }

      const cred = await navigator.credentials.get({ publicKey: opts });

      const credentialJson = {
        id: cred.id,
        rawId: bytesToB64url(cred.rawId),
        type: cred.type,
        response: {
          authenticatorData: bytesToB64url(cred.response.authenticatorData),
          clientDataJSON: bytesToB64url(cred.response.clientDataJSON),
          signature: bytesToB64url(cred.response.signature),
          userHandle: cred.response.userHandle ? bytesToB64url(cred.response.userHandle) : null,
        },
        extensions: cred.getClientExtensionResults ? cred.getClientExtensionResults() : {},
      };

      const finResp = await fetch('/api/auth/assert/finish', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ state_id, credential: credentialJson }),
      });
      if (!finResp.ok) {
        const t = await finResp.text();
        throw new Error('assert/finish: HTTP ' + finResp.status + ' ' + t);
      }

      window.location = '/';
    } catch (e) {
      btn.disabled = false;
      errEl.textContent = String(e && e.message || e);
    }
  });
</script>
</body>
</html>"#
        .to_string()
}

// ── auth: WebAuthn ceremony ───────────────────────────────────────────
//
// Four endpoints, two ceremonies:
//
//   register: setup_token → begin → (browser does WebAuthn) → finish
//             → Credential persisted, setup_token consumed,
//               session cookie set
//
//   assert:   begin (looks up enrolled credentials) → (browser asserts)
//             → finish → session cookie set
//
// Plus /api/auth/logout to clear the cookie + session record.
//
// Server-side state for in-flight ceremonies lives in two HashMaps in
// AppState. Each entry is keyed by a fresh `state_id` we mint at begin
// and the client echoes at finish. Lost on process restart — the user
// just retries. For a single-user system, this is fine.

const SESSION_COOKIE: &str = "sipag_session";

fn json_error(status: StatusCode, code: &str, detail: &str) -> Response {
    (
        status,
        Json(serde_json::json!({ "error": code, "detail": detail })),
    )
        .into_response()
}

#[derive(Deserialize)]
struct RegisterBeginBody {
    setup_token: String,
}

#[derive(Serialize)]
struct RegisterBeginResponse {
    state_id: String,
    challenge: CreationChallengeResponse,
}

async fn register_begin(
    State(state): State<AppState>,
    Json(body): Json<RegisterBeginBody>,
) -> Response {
    if body.setup_token.is_empty() {
        return json_error(
            StatusCode::BAD_REQUEST,
            "setup_token.required",
            "setup_token is required",
        );
    }
    let stored = match SetupToken::load(&state.sipag_dir, &body.setup_token) {
        Ok(t) => t,
        Err(_) => {
            return json_error(
                StatusCode::NOT_FOUND,
                "setup_token.unknown",
                "setup token not found",
            );
        }
    };
    if stored.purpose != SetupPurpose::EnrollPasskey {
        return json_error(
            StatusCode::GONE,
            "setup_token.wrong_purpose",
            "setup token is for a different flow",
        );
    }
    if stored.is_expired() {
        return json_error(
            StatusCode::GONE,
            "setup_token.expired",
            "setup token expired",
        );
    }

    // Bootstrap the user record on first use.
    let user = match User::load_or_create("Sipag Owner") {
        Ok(u) => u,
        Err(e) => {
            warn!("user::load_or_create failed: {e}");
            return json_error(StatusCode::INTERNAL_SERVER_ERROR, "user.io", &e.to_string());
        }
    };

    // No existing credentials to exclude on first enrollment, but we
    // pass the IDs of any already-stored creds so a re-enroll re-uses
    // the same authenticator only if the user actually wants that.
    let exclude: Option<Vec<CredentialID>> = Credential::list(&state.sipag_dir)
        .ok()
        .map(|creds| {
            creds
                .into_iter()
                .filter_map(|c| {
                    serde_json::from_value::<Passkey>(c.passkey)
                        .ok()
                        .map(|pk| pk.cred_id().clone())
                })
                .collect::<Vec<_>>()
        })
        .filter(|v: &Vec<_>| !v.is_empty());

    let (challenge, reg_state) = match state.webauthn.start_passkey_registration(
        user.id,
        &user.display_name,
        &user.display_name,
        exclude,
    ) {
        Ok(t) => t,
        Err(e) => {
            warn!("start_passkey_registration failed: {e}");
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "webauthn.start_register",
                &e.to_string(),
            );
        }
    };

    let state_id = random_token(16);
    state
        .pending_register
        .lock()
        .expect("pending_register mutex poisoned")
        .insert(
            state_id.clone(),
            PendingRegister {
                state: reg_state,
                setup_token: body.setup_token,
            },
        );

    Json(RegisterBeginResponse {
        state_id,
        challenge,
    })
    .into_response()
}

#[derive(Deserialize)]
struct RegisterFinishBody {
    state_id: String,
    credential: RegisterPublicKeyCredential,
    #[serde(default)]
    label: String,
}

#[derive(Serialize)]
struct AuthSuccess {
    ok: bool,
    credential_id: String,
}

async fn register_finish(
    State(state): State<AppState>,
    cookies: Cookies,
    Json(body): Json<RegisterFinishBody>,
) -> Response {
    let pending = match state
        .pending_register
        .lock()
        .expect("pending_register mutex poisoned")
        .remove(&body.state_id)
    {
        Some(p) => p,
        None => {
            return json_error(
                StatusCode::NOT_FOUND,
                "register.state_unknown",
                "registration state not found; restart enrollment",
            );
        }
    };

    let passkey = match state
        .webauthn
        .finish_passkey_registration(&body.credential, &pending.state)
    {
        Ok(pk) => pk,
        Err(e) => {
            warn!("finish_passkey_registration failed: {e}");
            return json_error(
                StatusCode::BAD_REQUEST,
                "webauthn.finish_register",
                &e.to_string(),
            );
        }
    };

    // Burn the setup token now — registration succeeded.
    if let Err(e) =
        SetupToken::consume(&state.sipag_dir, &pending.setup_token, SetupPurpose::EnrollPasskey)
    {
        warn!("setup_token consume failed after register: {e}");
        // Continue — credential is valid; an unconsumed token at worst
        // wastes a 10-minute window.
    }

    let cred_id_b64 = url_safe_base64(passkey.cred_id().as_ref());
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let cred = Credential {
        id: cred_id_b64.clone(),
        label: body.label,
        created: now.clone(),
        last_used: Some(now),
        passkey: serde_json::to_value(&passkey).unwrap_or(serde_json::Value::Null),
    };
    if let Err(e) = cred.save(&state.sipag_dir) {
        return json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "credential.save",
            &e.to_string(),
        );
    }

    issue_session_cookie(&state, &cookies, &cred_id_b64);

    Json(AuthSuccess {
        ok: true,
        credential_id: cred_id_b64,
    })
    .into_response()
}

#[derive(Serialize)]
struct AssertBeginResponse {
    state_id: String,
    challenge: RequestChallengeResponse,
}

async fn assert_begin(State(state): State<AppState>) -> Response {
    let creds = match Credential::list(&state.sipag_dir) {
        Ok(c) => c,
        Err(e) => {
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "credential.list",
                &e.to_string(),
            );
        }
    };
    if creds.is_empty() {
        return json_error(
            StatusCode::NOT_FOUND,
            "credential.none",
            "no credentials enrolled — run `sipag setup-token`",
        );
    }
    let passkeys: Vec<Passkey> = creds
        .into_iter()
        .filter_map(|c| serde_json::from_value::<Passkey>(c.passkey).ok())
        .collect();
    if passkeys.is_empty() {
        return json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "credential.unparseable",
            "stored credentials could not be deserialized",
        );
    }

    let (challenge, auth_state) = match state.webauthn.start_passkey_authentication(&passkeys) {
        Ok(t) => t,
        Err(e) => {
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "webauthn.start_assert",
                &e.to_string(),
            );
        }
    };

    let state_id = random_token(16);
    state
        .pending_auth
        .lock()
        .expect("pending_auth mutex poisoned")
        .insert(state_id.clone(), auth_state);

    Json(AssertBeginResponse {
        state_id,
        challenge,
    })
    .into_response()
}

#[derive(Deserialize)]
struct AssertFinishBody {
    state_id: String,
    credential: PublicKeyCredential,
}

async fn assert_finish(
    State(state): State<AppState>,
    cookies: Cookies,
    Json(body): Json<AssertFinishBody>,
) -> Response {
    let auth_state = match state
        .pending_auth
        .lock()
        .expect("pending_auth mutex poisoned")
        .remove(&body.state_id)
    {
        Some(s) => s,
        None => {
            return json_error(
                StatusCode::NOT_FOUND,
                "assert.state_unknown",
                "authentication state not found; retry login",
            );
        }
    };

    let result = match state
        .webauthn
        .finish_passkey_authentication(&body.credential, &auth_state)
    {
        Ok(r) => r,
        Err(e) => {
            warn!("finish_passkey_authentication failed: {e}");
            return json_error(
                StatusCode::UNAUTHORIZED,
                "webauthn.finish_assert",
                &e.to_string(),
            );
        }
    };

    let cred_id_b64 = url_safe_base64(result.cred_id().as_ref());

    // Best-effort touch: load the credential record, bump last_used,
    // and (later) update the passkey if its counter changed.
    if let Ok(mut rec) = Credential::load(&state.sipag_dir, &cred_id_b64) {
        rec.last_used = Some(chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string());
        if let Err(e) = rec.save(&state.sipag_dir) {
            warn!("credential touch save failed: {e}");
        }
    }

    issue_session_cookie(&state, &cookies, &cred_id_b64);

    Json(AuthSuccess {
        ok: true,
        credential_id: cred_id_b64,
    })
    .into_response()
}

async fn logout_handler(State(state): State<AppState>, cookies: Cookies) -> Response {
    if let Some(c) = cookies.get(SESSION_COOKIE) {
        let _ = Session::delete(&state.sipag_dir, c.value());
    }
    let mut clear = Cookie::new(SESSION_COOKIE, "");
    clear.set_path("/");
    cookies.remove(clear);
    Json(serde_json::json!({ "ok": true })).into_response()
}

/// Mint a Session, persist it, and set the session cookie on the
/// response. Caller has already verified the user.
fn issue_session_cookie(state: &AppState, cookies: &Cookies, credential_id: &str) {
    let token = random_token(32);
    let session = Session::new(token.clone(), credential_id.to_string());
    if let Err(e) = session.save(&state.sipag_dir) {
        warn!("session save failed: {e}");
        // We still set the cookie — the next request will fail to load
        // the session and the user will get redirected to /login.
    }
    let secure = state.public_url.starts_with("https://");
    let cookie = Cookie::build((SESSION_COOKIE, token))
        .http_only(true)
        .secure(secure)
        .same_site(SameSite::Lax)
        .path("/")
        .build();
    cookies.add(cookie);
    let _ = sessions_dir(&state.sipag_dir);
}

fn url_safe_base64(bytes: &[u8]) -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    URL_SAFE_NO_PAD.encode(bytes)
}

async fn delete_project_handler(AxumPath(name): AxumPath<String>) -> Response {
    let dir = default_sipag_dir();
    match delete_project(&dir, &name) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    }
}

async fn delete_kr_handler(AxumPath((name, id)): AxumPath<(String, u64)>) -> Response {
    let dir = default_sipag_dir();
    match KeyResult::delete(&dir, &name, id) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    }
}

async fn delete_task_handler(AxumPath((name, id)): AxumPath<(String, u64)>) -> Response {
    let dir = default_sipag_dir();
    match Task::delete(&dir, &name, id) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    }
}

// ── dispatch ──────────────────────────────────────────────────────────
//
// `POST /api/projects/:n/tasks/:id/dispatch` is the seam where the
// board (objectives + KRs + tasks) meets the mesh (katulong hosts).
//
// Body:
//   { "host": "mini" }      // optional; defaults to first in hosts.toml
//
// Flow:
//   1. Load task + role (or default).
//   2. POST <host>/sessions  → idempotent create, returns {id, name}.
//   3. POST <host>/sessions/by-id/<id>/exec
//      with the agent command derived from role.command + task title.
//   4. Move task to in-progress.
//   5. Return { task, host, session_id }.
//
// Deliberately *not* doing here:
//   - worktree setup (the existing helper assumes /work/<project> docker
//     paths, which don't fit a real-mac dispatch path; revisit when we
//     have a host-specific worktree scheme)
//   - kill-then-respawn on a task that's already running (out of scope)

#[derive(Deserialize)]
struct DispatchBody {
    #[serde(default)]
    host: Option<String>,
}

#[derive(Serialize)]
struct DispatchResponse {
    task: TaskView,
    host: String,
    /// Katulong session id for the spawned worker.
    session_id: Option<String>,
    session_name: String,
}

async fn dispatch_task_handler(
    AxumPath((project_name, id)): AxumPath<(String, u64)>,
    State(state): State<AppState>,
    body: Option<Json<DispatchBody>>,
) -> Response {
    let dir = default_sipag_dir();
    let want_host = body.as_ref().and_then(|b| b.0.host.clone());

    // Resolve target host: explicit body.host > first in hosts.toml.
    let host = match want_host {
        Some(id) => match state.hosts.find(&id) {
            Some(h) => h,
            None => {
                return (StatusCode::BAD_REQUEST, format!("unknown host: {id}"))
                    .into_response()
            }
        },
        None => match state.hosts.hosts.first() {
            Some(h) => h,
            None => {
                return (
                    StatusCode::CONFLICT,
                    "no hosts configured — populate ~/.sipag/hosts.toml",
                )
                    .into_response()
            }
        },
    };

    // Load task.
    let task = match Task::load(&dir, &project_name, id) {
        Ok(t) => t,
        Err(_) => return (StatusCode::NOT_FOUND, "task not found").into_response(),
    };

    // Load role (fall back to a default — agentic dispatch shouldn't fail
    // just because a role.toml hasn't been written yet).
    let role_command = Role::load(&dir, &project_name, &task.role)
        .map(|r| r.command)
        .unwrap_or_else(|_| "claude".to_string());

    // Build agent command. Title is JSON-encoded so embedded quotes,
    // backslashes, and newlines escape correctly when the shell sees it.
    let title_quoted = serde_json::to_string(&task.title)
        .unwrap_or_else(|_| format!("\"task #{id}\""));
    let agent_cmd = format!(
        "{} -p {}",
        role_command,
        title_quoted
    );
    let session = session_name(&project_name, &task.role);

    // 1. Create session (idempotent).
    let create_url = format!("{}/sessions", host.base_url());
    let create_resp = match state
        .http
        .post(&create_url)
        .bearer_auth(&host.api_key)
        .json(&serde_json::json!({ "name": session }))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            warn!(host = %host.id, error = %e, "POST /sessions failed");
            return (
                StatusCode::BAD_GATEWAY,
                format!("create session on {}: {e}", host.id),
            )
                .into_response();
        }
    };
    if !create_resp.status().is_success() {
        let st = create_resp.status();
        let body = create_resp.text().await.unwrap_or_default();
        return (
            StatusCode::BAD_GATEWAY,
            format!("create session on {}: HTTP {st}: {body}", host.id),
        )
            .into_response();
    }

    // Capture session id when present (idempotent create returns it).
    #[derive(Deserialize)]
    struct SessionCreated {
        #[serde(default)]
        id: Option<String>,
    }
    let session_id = create_resp
        .json::<SessionCreated>()
        .await
        .ok()
        .and_then(|s| s.id);

    // 2. Exec the agent command.
    let exec_url = if let Some(sid) = session_id.as_ref() {
        format!("{}/sessions/by-id/{}/exec", host.base_url(), sid)
    } else {
        // Fall back to name-keyed exec if the create response didn't
        // include an id (older katulong builds).
        format!(
            "{}/sessions/{}/exec",
            host.base_url(),
            session
        )
    };
    let exec_resp = match state
        .http
        .post(&exec_url)
        .bearer_auth(&host.api_key)
        .json(&serde_json::json!({ "input": agent_cmd }))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            warn!(host = %host.id, error = %e, "POST exec failed");
            return (
                StatusCode::BAD_GATEWAY,
                format!("exec on {}: {e}", host.id),
            )
                .into_response();
        }
    };
    if !exec_resp.status().is_success() {
        let st = exec_resp.status();
        let body = exec_resp.text().await.unwrap_or_default();
        return (
            StatusCode::BAD_GATEWAY,
            format!("exec on {}: HTTP {st}: {body}", host.id),
        )
            .into_response();
    }

    // 3. Move task to in-progress.
    if let Err(e) = move_task(&dir, &project_name, id, "in-progress") {
        warn!(
            project = %project_name,
            task = id,
            error = %e,
            "task moved to in-progress failed (worker already running on katulong)"
        );
    }

    // 4. Return the updated task + dispatch metadata.
    let updated = match Task::load(&dir, &project_name, id) {
        Ok(t) => t,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    };
    Json(DispatchResponse {
        task: TaskView::from(updated),
        host: host.id.clone(),
        session_id,
        session_name: session,
    })
    .into_response()
}

async fn update_task_handler(
    AxumPath((name, id)): AxumPath<(String, u64)>,
    Json(body): Json<UpdateTaskBody>,
) -> Response {
    let dir = default_sipag_dir();
    // For status changes we use move_task to keep the timestamp logic
    // consistent with the CLI; everything else we apply directly.
    if let Some(s) = body.status.as_deref() {
        if let Err(e) = move_task(&dir, &name, id, s) {
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response();
        }
    }
    let mut task = match Task::load(&dir, &name, id) {
        Ok(t) => t,
        Err(_) => return (StatusCode::NOT_FOUND, "task not found").into_response(),
    };
    let mut dirty = false;
    if let Some(t) = body.title {
        task.title = t;
        dirty = true;
    }
    if let Some(r) = body.role {
        task.role = r;
        dirty = true;
    }
    if let Some(l) = body.labels {
        task.labels = l;
        dirty = true;
    }
    if let Some(krs) = body.key_results {
        task.key_results = krs;
        dirty = true;
    }
    if dirty {
        task.updated = chrono::Utc::now()
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();
        if let Err(e) = task.save(&dir, &name) {
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response();
        }
    }
    Json(TaskView::from(task)).into_response()
}

async fn list_hosts(State(state): State<AppState>) -> Json<Vec<HostSummary>> {
    let summaries = state
        .hosts
        .hosts
        .iter()
        .map(|h| HostSummary {
            id: h.id.clone(),
            url: h.base_url().to_string(),
        })
        .collect();
    Json(summaries)
}

async fn proxy_sessions(
    AxumPath(id): AxumPath<String>,
    State(state): State<AppState>,
) -> Response {
    proxy_get(&state, &id, "/sessions").await
}

async fn proxy_session_status(
    AxumPath((id, sid)): AxumPath<(String, String)>,
    State(state): State<AppState>,
) -> Response {
    // Katulong session IDs are URL-safe nanoids so we pass them
    // through verbatim. Reject anything with a slash or control char
    // so an exotic id can't escape the template.
    if sid.chars().any(|c| c == '/' || c.is_control()) {
        return (StatusCode::BAD_REQUEST, "invalid session id").into_response();
    }
    let path = format!("/sessions/by-id/{}/status", sid);
    proxy_get(&state, &id, &path).await
}

async fn proxy_get(state: &AppState, host_id: &str, path: &str) -> Response {
    let Some(host) = state.hosts.find(host_id) else {
        return (StatusCode::NOT_FOUND, format!("unknown host: {host_id}")).into_response();
    };
    let url = format!("{}{}", host.base_url(), path);

    match state
        .http
        .get(&url)
        .bearer_auth(&host.api_key)
        .send()
        .await
    {
        Ok(resp) => {
            let status = resp.status();
            let mut headers = HeaderMap::new();
            if let Some(ct) = resp.headers().get(reqwest::header::CONTENT_TYPE).cloned() {
                headers.insert(axum::http::header::CONTENT_TYPE, ct);
            }
            let body = match resp.bytes().await {
                Ok(b) => b,
                Err(e) => {
                    warn!(host = host_id, path, error = %e, "read body failed");
                    return (
                        StatusCode::BAD_GATEWAY,
                        format!("failed to read {} response: {e}", host_id),
                    )
                        .into_response();
                }
            };
            (status, headers, body).into_response()
        }
        Err(e) => {
            warn!(host = host_id, path, url, error = %e, "proxy request failed");
            (
                StatusCode::BAD_GATEWAY,
                format!("failed to reach {}: {e}", host_id),
            )
                .into_response()
        }
    }
}
