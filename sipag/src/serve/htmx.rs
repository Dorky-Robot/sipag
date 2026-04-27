//! HTMX action endpoints.
//!
//! These return HTML fragments rather than JSON. Most mutations return
//! the full `<main class="board">` because the user-visible side
//! effects fan out (a status cycle changes counts, a dispatch changes
//! "running on" markers across rows). Coarse swaps keep the handler
//! code small; HTMX's morphdom-style outerHTML swap makes this cheap.
//!
//! The matching JSON `/api/*` endpoints in `board.rs` are still
//! authoritative — these are just thin re-renders.

use crate::serve::board_view::{
    self, board_main, dispatch_picker, idea_box, load_snapshot, new_kr_form, new_objective_form,
    new_standing_form, new_task_form, oob_clear_dispatch_picker, oob_toast, page,
};
use crate::serve::state::AppState;
use axum::{
    extract::{Form, Path as AxumPath, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::{delete, get, patch, post},
    Router,
};
use maud::{html, Markup};
use serde::Deserialize;
use sipag_core::board::{
    add_task, create_project_with_kind, delete_project, load_project, move_task, KeyResult,
    KrStance, ProjectKind, Task, TaskStatus,
};
use sipag_core::katulong::session_name;
use tracing::warn;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(page_root))
        .route("/htmx/board", get(board_fragment))
        .route("/htmx/idea-box", get(idea_box_fragment))
        .route("/htmx/dispatch-picker", get(dispatch_picker_fragment))
        .route("/htmx/forms/:key", get(form_fragment))
        .route("/htmx/projects", post(create_project))
        .route("/htmx/projects/:name", delete(delete_project_handler))
        .route("/htmx/projects/:name/key-results", post(create_kr_handler))
        .route(
            "/htmx/projects/:name/key-results/:id",
            patch(update_kr_handler).delete(delete_kr_handler),
        )
        .route(
            "/htmx/projects/:name/key-results/:id/labels",
            post(kr_labels_handler),
        )
        .route(
            "/htmx/projects/:name/key-results/:id/done",
            post(kr_done_handler),
        )
        .route(
            "/htmx/projects/:name/key-results/:id/discourse",
            post(kr_discourse_handler),
        )
        .route("/htmx/projects/:name/tasks", post(create_task_handler))
        .route(
            "/htmx/projects/:name/tasks/:id",
            patch(update_task_handler).delete(delete_task_handler),
        )
        .route(
            "/htmx/projects/:name/tasks/:id/labels",
            post(task_labels_handler),
        )
        .route(
            "/htmx/projects/:name/tasks/:id/discourse",
            post(task_discourse_handler),
        )
        .route(
            "/htmx/projects/:name/tasks/:id/dispatch",
            post(dispatch_task_handler),
        )
        .route(
            "/htmx/discourse/:kind/:project/:id",
            get(discourse_fragment),
        )
        .route("/htmx/attention", get(attention_fragment))
        .route("/htmx/ticker", get(ticker_fragment))
        .route("/htmx/insights/hint", get(insights_hint))
}

// ── shared response builders ────────────────────────────────────────

async fn render_board(state: &AppState) -> Markup {
    let snap = load_snapshot(state).await;
    board_main(&snap)
}

fn html_response(markup: Markup) -> Response {
    Html(markup.into_string()).into_response()
}

fn err_response(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, msg.into()).into_response()
}

// ── full page ───────────────────────────────────────────────────────

async fn page_root(State(state): State<AppState>) -> Response {
    let snap = load_snapshot(&state).await;
    html_response(page(&snap))
}

// ── board fragment ──────────────────────────────────────────────────

async fn board_fragment(State(state): State<AppState>) -> Response {
    html_response(render_board(&state).await)
}

// ── idea-box fragment ───────────────────────────────────────────────

#[derive(Deserialize)]
struct IdeaBoxQuery {
    #[serde(default)]
    open: Option<u8>,
}

async fn idea_box_fragment(
    State(state): State<AppState>,
    Query(q): Query<IdeaBoxQuery>,
) -> Response {
    let snap = load_snapshot(&state).await;
    let open = q.open.unwrap_or(0) == 1;
    html_response(idea_box(&snap.projects, open))
}

// ── dispatch picker fragment ────────────────────────────────────────

#[derive(Deserialize)]
struct DispatchPickerQuery {
    project: Option<String>,
    task: Option<u64>,
    #[serde(default)]
    clear: Option<u8>,
}

async fn dispatch_picker_fragment(
    State(state): State<AppState>,
    Query(q): Query<DispatchPickerQuery>,
) -> Response {
    if q.clear.unwrap_or(0) == 1 {
        return html_response(html! {});
    }
    let project = match q.project {
        Some(p) if !p.is_empty() => p,
        _ => return err_response(StatusCode::BAD_REQUEST, "project required"),
    };
    let task = match q.task {
        Some(t) => t,
        None => return err_response(StatusCode::BAD_REQUEST, "task required"),
    };
    let snap = load_snapshot(&state).await;
    html_response(dispatch_picker(&project, task, &snap.hosts))
}

// ── form open/close fragment ────────────────────────────────────────

#[derive(Deserialize)]
struct FormQuery {
    #[serde(default)]
    open: Option<u8>,
    #[serde(default)]
    project: Option<String>,
}

async fn form_fragment(AxumPath(key): AxumPath<String>, Query(q): Query<FormQuery>) -> Response {
    let open = q.open.unwrap_or(0) == 1;
    let project = q.project.as_deref().unwrap_or("");

    let markup = match key.as_str() {
        "new-objective" => new_objective_form(open),
        "new-standing" => new_standing_form(open),
        "new-kr" => {
            if project.is_empty() {
                return err_response(StatusCode::BAD_REQUEST, "project required");
            }
            new_kr_form(project, open)
        }
        "new-task" => {
            if project.is_empty() {
                return err_response(StatusCode::BAD_REQUEST, "project required");
            }
            // Detect kind to pick the right submit label.
            let label = match load_project(&load_dir(), project) {
                Ok(p) if matches!(p.kind, ProjectKind::Standing) => "concern",
                _ => "task",
            };
            new_task_form(project, open, label)
        }
        _ => return err_response(StatusCode::NOT_FOUND, "unknown form"),
    };
    html_response(markup)
}

fn load_dir() -> std::path::PathBuf {
    sipag_core::config::default_sipag_dir()
}

// ── projects ────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct CreateProjectForm {
    name: String,
    #[serde(default)]
    repo: String,
    #[serde(default)]
    kind: Option<String>,
}

async fn create_project(
    State(state): State<AppState>,
    Form(body): Form<CreateProjectForm>,
) -> Response {
    let dir = load_dir();
    if body.name.trim().is_empty() {
        return err_response(StatusCode::BAD_REQUEST, "name is required");
    }
    let kind = match body.kind.as_deref().unwrap_or("objective") {
        "objective" => ProjectKind::Objective,
        "standing" => ProjectKind::Standing,
        other => return err_response(StatusCode::BAD_REQUEST, format!("unknown kind: {other}")),
    };
    if let Err(e) = create_project_with_kind(&dir, body.name.trim(), &body.repo, kind, None) {
        return err_response(StatusCode::INTERNAL_SERVER_ERROR, format!("{e}"));
    }
    html_response(render_board(&state).await)
}

async fn delete_project_handler(
    AxumPath(name): AxumPath<String>,
    State(state): State<AppState>,
) -> Response {
    let dir = load_dir();
    if let Err(e) = delete_project(&dir, &name) {
        return err_response(StatusCode::INTERNAL_SERVER_ERROR, format!("{e}"));
    }
    html_response(render_board(&state).await)
}

// ── key results ─────────────────────────────────────────────────────

#[derive(Deserialize)]
struct CreateKrForm {
    title: String,
}

async fn create_kr_handler(
    AxumPath(name): AxumPath<String>,
    State(state): State<AppState>,
    Form(body): Form<CreateKrForm>,
) -> Response {
    let dir = load_dir();
    if body.title.trim().is_empty() {
        return err_response(StatusCode::BAD_REQUEST, "title is required");
    }
    if load_project(&dir, &name).is_err() {
        return err_response(StatusCode::NOT_FOUND, format!("project '{name}' not found"));
    }
    let id = match KeyResult::next_id(&dir, &name) {
        Ok(n) => n,
        Err(e) => return err_response(StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")),
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
        return err_response(StatusCode::INTERNAL_SERVER_ERROR, format!("{e}"));
    }
    html_response(render_board(&state).await)
}

#[derive(Deserialize)]
struct UpdateKrForm {
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    stance: Option<String>,
}

async fn update_kr_handler(
    AxumPath((name, id)): AxumPath<(String, u64)>,
    State(state): State<AppState>,
    Form(body): Form<UpdateKrForm>,
) -> Response {
    let dir = load_dir();
    let mut kr = match KeyResult::load(&dir, &name, id) {
        Ok(k) => k,
        Err(_) => return err_response(StatusCode::NOT_FOUND, "KR not found"),
    };

    match body.action.as_deref() {
        Some("cycle") => {
            kr.stance = cycle_stance(kr.stance);
        }
        _ => {
            if let Some(t) = body.title {
                kr.title = t;
            }
            if let Some(s) = body.stance {
                match KrStance::parse(&s) {
                    Some(parsed) => kr.stance = parsed,
                    None => {
                        return err_response(
                            StatusCode::BAD_REQUEST,
                            format!("unknown stance: {s}"),
                        )
                    }
                }
            }
        }
    }
    if let Err(e) = kr.save(&dir, &name) {
        return err_response(StatusCode::INTERNAL_SERVER_ERROR, format!("{e}"));
    }
    html_response(render_board(&state).await)
}

async fn delete_kr_handler(
    AxumPath((name, id)): AxumPath<(String, u64)>,
    State(state): State<AppState>,
) -> Response {
    let dir = load_dir();
    if let Err(e) = KeyResult::delete(&dir, &name, id) {
        return err_response(StatusCode::INTERNAL_SERVER_ERROR, format!("{e}"));
    }
    html_response(render_board(&state).await)
}

fn cycle_stance(s: KrStance) -> KrStance {
    match s {
        KrStance::Green => KrStance::Yellow,
        KrStance::Yellow => KrStance::Red,
        KrStance::Red => KrStance::Done,
        KrStance::Done => KrStance::Green,
    }
}

// ── tasks ───────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct CreateTaskForm {
    title: String,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    labels: Option<String>,
    #[serde(default)]
    kr: Option<String>,
    /// Optional secondary spelling — same field, in case the form
    /// posts as `key_results` directly.
    #[serde(default)]
    key_results: Option<String>,
}

async fn create_task_handler(
    AxumPath(name): AxumPath<String>,
    State(state): State<AppState>,
    Form(body): Form<CreateTaskForm>,
) -> Response {
    let dir = load_dir();
    if body.title.trim().is_empty() {
        return err_response(StatusCode::BAD_REQUEST, "title is required");
    }
    let labels = parse_labels(body.labels.as_deref().unwrap_or(""));
    let mut task = match add_task(
        &dir,
        &name,
        body.title.trim(),
        body.role.as_deref(),
        &labels,
    ) {
        Ok(t) => t,
        Err(e) => return err_response(StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")),
    };
    let kr_str = body
        .kr
        .as_deref()
        .or(body.key_results.as_deref())
        .unwrap_or("")
        .trim();
    if !kr_str.is_empty() {
        if let Ok(id) = kr_str.parse::<u64>() {
            task.key_results = vec![id];
            if let Err(e) = task.save(&dir, &name) {
                return err_response(StatusCode::INTERNAL_SERVER_ERROR, format!("{e}"));
            }
        }
    }
    html_response(render_board(&state).await)
}

fn parse_labels(s: &str) -> Vec<String> {
    s.split(',')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

#[derive(Deserialize)]
struct UpdateTaskForm {
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    labels: Option<String>,
}

async fn update_task_handler(
    AxumPath((name, id)): AxumPath<(String, u64)>,
    State(state): State<AppState>,
    Form(body): Form<UpdateTaskForm>,
) -> Response {
    let dir = load_dir();
    let mut task = match Task::load(&dir, &name, id) {
        Ok(t) => t,
        Err(_) => return err_response(StatusCode::NOT_FOUND, "task not found"),
    };

    match body.action.as_deref() {
        Some("cycle-status") => {
            let next = cycle_status(&task.status);
            if let Err(e) = move_task(&dir, &name, id, next) {
                return err_response(StatusCode::INTERNAL_SERVER_ERROR, format!("{e}"));
            }
        }
        Some("activate") => {
            if let Err(e) = move_task(&dir, &name, id, "todo") {
                return err_response(StatusCode::INTERNAL_SERVER_ERROR, format!("{e}"));
            }
        }
        _ => {
            // Generic update
            if let Some(s) = body.status.as_deref() {
                if let Err(e) = move_task(&dir, &name, id, s) {
                    return err_response(StatusCode::INTERNAL_SERVER_ERROR, format!("{e}"));
                }
                task = match Task::load(&dir, &name, id) {
                    Ok(t) => t,
                    Err(_) => return err_response(StatusCode::NOT_FOUND, "task not found"),
                };
            }
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
                task.labels = parse_labels(&l);
                dirty = true;
            }
            if dirty {
                task.updated = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
                if let Err(e) = task.save(&dir, &name) {
                    return err_response(StatusCode::INTERNAL_SERVER_ERROR, format!("{e}"));
                }
            }
        }
    }
    html_response(render_board(&state).await)
}

fn cycle_status(s: &TaskStatus) -> &'static str {
    match s {
        TaskStatus::Todo => "in-progress",
        TaskStatus::InProgress => "review",
        TaskStatus::Review => "done",
        TaskStatus::Done => "backlog",
        TaskStatus::Backlog => "todo",
        TaskStatus::Custom(_) => "todo",
    }
}

async fn delete_task_handler(
    AxumPath((name, id)): AxumPath<(String, u64)>,
    State(state): State<AppState>,
) -> Response {
    let dir = load_dir();
    if let Err(e) = Task::delete(&dir, &name, id) {
        return err_response(StatusCode::INTERNAL_SERVER_ERROR, format!("{e}"));
    }
    html_response(render_board(&state).await)
}

// ── dispatch ────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct DispatchForm {
    #[serde(default)]
    host: Option<String>,
}

async fn dispatch_task_handler(
    AxumPath((project_name, id)): AxumPath<(String, u64)>,
    State(state): State<AppState>,
    Form(body): Form<DispatchForm>,
) -> Response {
    let dir = load_dir();
    let want_host = body.host.clone();

    let host = match want_host {
        Some(id) => match state.hosts.find(&id) {
            Some(h) => h,
            None => return err_response(StatusCode::BAD_REQUEST, format!("unknown host: {id}")),
        },
        None => match state.hosts.hosts.first() {
            Some(h) => h,
            None => {
                return err_response(
                    StatusCode::CONFLICT,
                    "no hosts configured — populate ~/.sipag/hosts.toml",
                )
            }
        },
    };

    let task = match Task::load(&dir, &project_name, id) {
        Ok(t) => t,
        Err(_) => return err_response(StatusCode::NOT_FOUND, "task not found"),
    };

    let role_command = sipag_core::board::Role::load(&dir, &project_name, &task.role)
        .map(|r| r.command)
        .unwrap_or_else(|_| "claude".to_string());
    let title_quoted =
        serde_json::to_string(&task.title).unwrap_or_else(|_| format!("\"task #{id}\""));
    let agent_cmd = format!("{role_command} -p {title_quoted}");
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
            return err_response(
                StatusCode::BAD_GATEWAY,
                format!("create session on {}: {e}", host.id),
            );
        }
    };
    if !create_resp.status().is_success() {
        let st = create_resp.status();
        let txt = create_resp.text().await.unwrap_or_default();
        return err_response(
            StatusCode::BAD_GATEWAY,
            format!("create session on {}: HTTP {st}: {txt}", host.id),
        );
    }
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

    let exec_url = if let Some(sid) = session_id.as_ref() {
        format!("{}/sessions/by-id/{}/exec", host.base_url(), sid)
    } else {
        format!("{}/sessions/{}/exec", host.base_url(), session)
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
            return err_response(StatusCode::BAD_GATEWAY, format!("exec on {}: {e}", host.id));
        }
    };
    if !exec_resp.status().is_success() {
        let st = exec_resp.status();
        let txt = exec_resp.text().await.unwrap_or_default();
        return err_response(
            StatusCode::BAD_GATEWAY,
            format!("exec on {}: HTTP {st}: {txt}", host.id),
        );
    }

    if let Err(e) = move_task(&dir, &project_name, id, "in-progress") {
        warn!(
            project = %project_name,
            task = id,
            error = %e,
            "task move to in-progress failed (worker is already running)"
        );
    }

    let toast_msg = format!("dispatched #{id} on {} · {}", host.id, session);
    let board = render_board(&state).await;
    // Concatenate fragments: board (replaces #board) + OOB toast + OOB
    // dispatch-picker clear. HTMX reads the OOB elements and swaps
    // them into their respective IDs.
    let combined = html! {
        (board)
        (oob_toast(&toast_msg))
        (oob_clear_dispatch_picker())
    };
    html_response(combined)
}

// ── insights hint (consolidated from the spike) ────────────────────

#[derive(Deserialize)]
struct InsightsHintQuery {
    q: Option<String>,
    #[serde(default)]
    repo: Option<String>,
    #[serde(default)]
    n: Option<usize>,
}

async fn insights_hint(Query(q): Query<InsightsHintQuery>) -> Response {
    use crate::serve::insights::search;
    let query = q.q.unwrap_or_default();
    let n = q.n.unwrap_or(3);
    let insights = search(&query, q.repo.as_deref(), n)
        .await
        .unwrap_or_default();
    if insights.is_empty() {
        return Html(String::new()).into_response();
    }
    html_response(board_view::render_hint_rows_external(&insights))
}

// ── label / done / discourse / attention / ticker ───────────────────

#[derive(Deserialize)]
struct LabelChangeForm {
    #[serde(default)]
    add: String,
    #[serde(default)]
    remove: String,
}

fn parse_csv(s: &str) -> Vec<String> {
    s.split(',')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

fn apply_labels(labels: &mut Vec<String>, add: &[String], remove: &[String]) {
    labels.retain(|l| !remove.iter().any(|r| r == l));
    for a in add {
        if !labels.iter().any(|l| l == a) {
            labels.push(a.clone());
        }
    }
}

async fn kr_labels_handler(
    AxumPath((name, id)): AxumPath<(String, u64)>,
    State(state): State<AppState>,
    Form(body): Form<LabelChangeForm>,
) -> Response {
    let dir = load_dir();
    let mut kr = match KeyResult::load(&dir, &name, id) {
        Ok(k) => k,
        Err(_) => return err_response(StatusCode::NOT_FOUND, "KR not found"),
    };
    let add = parse_csv(&body.add);
    let remove = parse_csv(&body.remove);
    apply_labels(&mut kr.labels, &add, &remove);
    if let Err(e) = kr.save(&dir, &name) {
        return err_response(StatusCode::INTERNAL_SERVER_ERROR, format!("{e}"));
    }
    let topic = format!("key-results/{name}/{id}/discourse");
    let payload = serde_json::json!({
        "add": add,
        "remove": remove,
        "actor": "human",
    });
    let _ = state.broker.publish(&topic, "label.changed", payload);
    html_response(render_board(&state).await)
}

async fn task_labels_handler(
    AxumPath((name, id)): AxumPath<(String, u64)>,
    State(state): State<AppState>,
    Form(body): Form<LabelChangeForm>,
) -> Response {
    let dir = load_dir();
    let mut task = match Task::load(&dir, &name, id) {
        Ok(t) => t,
        Err(_) => return err_response(StatusCode::NOT_FOUND, "task not found"),
    };
    let add = parse_csv(&body.add);
    let remove = parse_csv(&body.remove);
    apply_labels(&mut task.labels, &add, &remove);
    task.updated = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    if let Err(e) = task.save(&dir, &name) {
        return err_response(StatusCode::INTERNAL_SERVER_ERROR, format!("{e}"));
    }
    let topic = format!("tasks/{name}/{id}/discourse");
    let payload = serde_json::json!({
        "add": add,
        "remove": remove,
        "actor": "human",
    });
    let _ = state.broker.publish(&topic, "label.changed", payload);
    html_response(render_board(&state).await)
}

async fn kr_done_handler(
    AxumPath((name, id)): AxumPath<(String, u64)>,
    State(state): State<AppState>,
) -> Response {
    let dir = load_dir();
    let mut kr = match KeyResult::load(&dir, &name, id) {
        Ok(k) => k,
        Err(_) => return err_response(StatusCode::NOT_FOUND, "KR not found"),
    };
    kr.done = !kr.done;
    if let Err(e) = kr.save(&dir, &name) {
        return err_response(StatusCode::INTERNAL_SERVER_ERROR, format!("{e}"));
    }
    let topic = format!("key-results/{name}/{id}/discourse");
    let payload = serde_json::json!({
        "done": kr.done,
        "actor": "human",
    });
    let _ = state.broker.publish(&topic, "done.toggled", payload);
    html_response(render_board(&state).await)
}

#[derive(Deserialize)]
struct DiscoursePost {
    text: String,
}

async fn kr_discourse_handler(
    AxumPath((name, id)): AxumPath<(String, u64)>,
    State(state): State<AppState>,
    Form(body): Form<DiscoursePost>,
) -> Response {
    let trimmed = body.text.trim();
    if trimmed.is_empty() {
        return StatusCode::NO_CONTENT.into_response();
    }
    let topic = format!("key-results/{name}/{id}/discourse");
    let payload = serde_json::json!({
        "text": trimmed,
        "actor": "human",
    });
    let _ = state.broker.publish(&topic, "human.message", payload);
    StatusCode::NO_CONTENT.into_response()
}

async fn task_discourse_handler(
    AxumPath((name, id)): AxumPath<(String, u64)>,
    State(state): State<AppState>,
    Form(body): Form<DiscoursePost>,
) -> Response {
    let trimmed = body.text.trim();
    if trimmed.is_empty() {
        return StatusCode::NO_CONTENT.into_response();
    }
    let topic = format!("tasks/{name}/{id}/discourse");
    let payload = serde_json::json!({
        "text": trimmed,
        "actor": "human",
    });
    let _ = state.broker.publish(&topic, "human.message", payload);
    StatusCode::NO_CONTENT.into_response()
}

async fn discourse_fragment(
    AxumPath((kind, project, id)): AxumPath<(String, String, u64)>,
    State(state): State<AppState>,
) -> Response {
    if kind != "key-results" && kind != "tasks" {
        return err_response(StatusCode::BAD_REQUEST, "invalid kind");
    }
    let topic = format!("{kind}/{project}/{id}/discourse");
    let envs = state.broker.read(&topic, 0).unwrap_or_default();
    html_response(board_view::discourse_panel(&topic, &envs))
}

async fn attention_fragment(State(state): State<AppState>) -> Response {
    let snap = board_view::load_snapshot(&state).await;
    html_response(board_view::attention_strip(&snap))
}

async fn ticker_fragment(State(state): State<AppState>) -> Response {
    let envs = state.broker.read("workers/activity", 0).unwrap_or_default();
    let last_n: Vec<_> = envs.iter().rev().take(5).cloned().collect();
    let mut chronological = last_n;
    chronological.reverse();
    html_response(board_view::ticker(&chronological))
}
