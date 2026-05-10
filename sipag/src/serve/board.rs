//! Board, KR, task, dispatch, and host proxy routes.
//!
//! Everything that isn't auth-related. Authenticated by the surrounding
//! middleware — handlers here don't re-check the gate.

use crate::serve::state::AppState;
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
use sipag_core::config::default_sipag_dir;
use sipag_core::katulong::session_name;
use tracing::warn;

pub fn routes() -> Router<AppState> {
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
        .route("/api/projects/:name/key-results", post(create_kr_handler))
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
}

// ---------- views ----------

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
    labels: Vec<String>,
    done: bool,
}

impl From<KeyResult> for KrView {
    fn from(k: KeyResult) -> Self {
        Self {
            id: k.id,
            title: k.title,
            stance: k.stance.to_string(),
            created: k.created,
            labels: k.labels,
            done: k.done,
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

#[derive(Serialize)]
struct HostSummary {
    id: String,
    url: String,
}

// ---------- projects ----------

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

#[derive(Deserialize)]
struct CreateProjectBody {
    name: String,
    #[serde(default)]
    repo: String,
    #[serde(default)]
    kind: Option<String>,
}

async fn create_project_handler(Json(body): Json<CreateProjectBody>) -> Response {
    let dir = default_sipag_dir();
    let kind = match body.kind.as_deref().unwrap_or("objective") {
        "objective" => ProjectKind::Objective,
        "standing" => ProjectKind::Standing,
        other => {
            return (StatusCode::BAD_REQUEST, format!("unknown kind: {other}")).into_response()
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

async fn delete_project_handler(AxumPath(name): AxumPath<String>) -> Response {
    let dir = default_sipag_dir();
    match delete_project(&dir, &name) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    }
}

// ---------- key results ----------

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
        created: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        labels: Vec::new(),
        done: false,
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

async fn delete_kr_handler(AxumPath((name, id)): AxumPath<(String, u64)>) -> Response {
    let dir = default_sipag_dir();
    match KeyResult::delete(&dir, &name, id) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    }
}

// ---------- tasks ----------

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

async fn update_task_handler(
    AxumPath((name, id)): AxumPath<(String, u64)>,
    Json(body): Json<UpdateTaskBody>,
) -> Response {
    let dir = default_sipag_dir();
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
        task.updated = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        if let Err(e) = task.save(&dir, &name) {
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response();
        }
    }
    Json(TaskView::from(task)).into_response()
}

async fn delete_task_handler(AxumPath((name, id)): AxumPath<(String, u64)>) -> Response {
    let dir = default_sipag_dir();
    match Task::delete(&dir, &name, id) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    }
}

// ---------- dispatch ----------

#[derive(Deserialize)]
struct DispatchBody {
    #[serde(default)]
    host: Option<String>,
}

#[derive(Serialize)]
struct DispatchResponse {
    task: TaskView,
    host: String,
    // Always `Some` since the by-id migration: a missing id now causes
    // the handler to return BAD_GATEWAY before this struct is built.
    // Kept `Option<String>` for wire compat with any external JSON
    // consumer; flatten to `String` on the next API revision.
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

    let host = match want_host {
        Some(id) => match state.hosts.find(&id) {
            Some(h) => h,
            None => {
                return (StatusCode::BAD_REQUEST, format!("unknown host: {id}")).into_response()
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

    let task = match Task::load(&dir, &project_name, id) {
        Ok(t) => t,
        Err(_) => return (StatusCode::NOT_FOUND, "task not found").into_response(),
    };

    let role_command = Role::load(&dir, &project_name, &task.role)
        .map(|r| r.command)
        .unwrap_or_else(|_| "claude".to_string());

    let title_quoted =
        serde_json::to_string(&task.title).unwrap_or_else(|_| format!("\"task #{id}\""));
    let agent_cmd = format!("{} -p {}", role_command, title_quoted);
    let session = session_name(&project_name, &task.role);

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
            // Don't echo `{e}` into the response: reqwest's Display
            // includes the request URL, which leaks the (private)
            // tunnel hostname to the caller. Full detail is in the
            // warn log above.
            return (
                StatusCode::BAD_GATEWAY,
                format!("create session on {}: network error", host.id),
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

    let session_id = match super::extract_session_id(create_resp, &host.id).await {
        Ok(id) => id,
        Err((st, body)) => return (st, body).into_response(),
    };

    let exec_url = format!("{}/sessions/by-id/{session_id}/exec", host.base_url());
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
                format!("exec on {}: network error", host.id),
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

    if let Err(e) = move_task(&dir, &project_name, id, "in-progress") {
        warn!(
            project = %project_name,
            task = id,
            error = %e,
            "task moved to in-progress failed (worker already running on katulong)"
        );
    }

    let updated = match Task::load(&dir, &project_name, id) {
        Ok(t) => t,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    };
    Json(DispatchResponse {
        task: TaskView::from(updated),
        host: host.id.clone(),
        session_id: Some(session_id),
        session_name: session,
    })
    .into_response()
}

// ---------- hosts proxy ----------

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

async fn proxy_sessions(AxumPath(id): AxumPath<String>, State(state): State<AppState>) -> Response {
    proxy_get(&state, &id, "/sessions").await
}

async fn proxy_session_status(
    AxumPath((id, sid)): AxumPath<(String, String)>,
    State(state): State<AppState>,
) -> Response {
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

    match state.http.get(&url).bearer_auth(&host.api_key).send().await {
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
                        format!("failed to read {host_id} response"),
                    )
                        .into_response();
                }
            };
            (status, headers, body).into_response()
        }
        Err(e) => {
            // `error = %e` already includes the request URL via reqwest's
            // Display impl. Don't add `url` as a separate structured field
            // — that just gives log-forwarding pipelines a second copy of
            // the tunnel hostname to spread.
            warn!(host = host_id, path, error = %e, "proxy request failed");
            (
                StatusCode::BAD_GATEWAY,
                format!("failed to reach {host_id}: network error"),
            )
                .into_response()
        }
    }
}
