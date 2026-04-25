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
use sipag_core::board::{
    add_task, create_project_with_kind, delete_project, list_project_names, list_tasks,
    load_project, move_task, KeyResult, KrStance, ProjectKind, Role, Task,
};
use sipag_core::katulong::session_name;
use sipag_core::config::default_sipag_dir;
use sipag_core::hosts::{default_hosts_path, HostsConfig};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tower_http::services::ServeDir;
use tracing::{info, warn};

#[derive(Clone)]
struct AppState {
    hosts: Arc<HostsConfig>,
    http: reqwest::Client,
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

    let state = AppState {
        hosts: Arc::new(hosts),
        http,
    };

    let app = Router::new()
        .route("/api/hosts", get(list_hosts))
        // Katulong has no /crew HTTP routes — `crew` is a naming
        // convention on /sessions. We expose /sessions verbatim, plus
        // the per-id status endpoint used to derive worker state.
        .route("/api/hosts/:id/sessions", get(proxy_sessions))
        .route(
            "/api/hosts/:id/sessions/by-id/:sid/status",
            get(proxy_session_status),
        )
        // Board (objectives + KRs + tasks) — the primary surface.
        // Mesh above is background context.
        .route("/api/projects", get(list_projects).post(create_project_handler))
        .route(
            "/api/projects/:name",
            delete(delete_project_handler),
        )
        .route(
            "/api/projects/:name/key-results",
            post(create_kr_handler),
        )
        .route(
            "/api/projects/:name/key-results/:id",
            patch(update_kr_handler).delete(delete_kr_handler),
        )
        .route(
            "/api/projects/:name/tasks",
            post(create_task_handler),
        )
        .route(
            "/api/projects/:name/tasks/:id",
            patch(update_task_handler).delete(delete_task_handler),
        )
        .route(
            "/api/projects/:name/tasks/:id/dispatch",
            post(dispatch_task_handler),
        )
        .fallback_service(ServeDir::new(&web_root).append_index_html_on_directories(true))
        .with_state(state);

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
