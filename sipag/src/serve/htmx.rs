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
    KrStance, Observation, ProjectKind, Task, TaskStatus, MISC_PROJECT,
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
            "/htmx/projects/:name/tasks/:id/dispatch",
            post(dispatch_task_handler),
        )
        .route("/htmx/attention", get(attention_fragment))
        .route(
            "/htmx/observations/:obs_id/kr",
            post(observation_kr_handler),
        )
        .route(
            "/htmx/observations/:obs_id/kr/reject",
            post(observation_kr_reject_handler),
        )
        .route(
            "/htmx/observations/:obs_id/transcript",
            get(observation_transcript_handler),
        )
        .route(
            "/htmx/sessions/:host_id/:uuid/respond",
            post(claude_respond_handler),
        )
        .route("/htmx/insights/hint", get(insights_hint))
        .route("/htmx/debug/topics", get(debug_topics))
}

/// Lists every known pubsub topic. Used by the in-page debug panel
/// to show which streams are live; the panel then subscribes to each
/// over the WebSocket so the user can watch traffic without a
/// browser inspector.
async fn debug_topics(State(state): State<AppState>) -> Response {
    let topics = state.broker.list_topics().unwrap_or_default();
    axum::Json(topics).into_response()
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
    // Build a context-rich prompt: the "why" (objective aspiration +
    // KR), a nudge to grep the project's commit history with diwa
    // before writing code, then the actual task. Mirrors how a human
    // would brief Claude when starting a session manually.
    let prompt = build_dispatch_prompt(&dir, &project_name, &task);
    // Two-step dispatch: sync exec sends just the launch command
    // (e.g. `claude\r`) so we get fast HTTP feedback if katulong is
    // unreachable. The background task waits for claude's TUI to be
    // ready (auto-approving the trust prompt if seen), then pastes
    // the prompt as one bracketed-paste message — same shape as a
    // human typing into the TUI, with normal permission prompts left
    // intact for human approval from the iPad.
    let launch_cmd = build_launch_cmd(&role_command);
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
            // Don't echo `{e}` into the response — reqwest's Display
            // includes the request URL, leaking the tunnel hostname.
            // Full detail is in the warn log above.
            return err_response(
                StatusCode::BAD_GATEWAY,
                format!("create session on {}: network error", host.id),
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
    let session_id = match super::extract_session_id(create_resp, &host.id).await {
        Ok(id) => id,
        Err((st, body)) => return err_response(st, body),
    };

    let exec_url = format!("{}/sessions/by-id/{session_id}/exec", host.base_url());
    let exec_resp = match state
        .http
        .post(&exec_url)
        .bearer_auth(&host.api_key)
        .json(&serde_json::json!({ "input": launch_cmd }))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            warn!(host = %host.id, error = %e, "POST exec failed");
            return err_response(
                StatusCode::BAD_GATEWAY,
                format!("exec on {}: network error", host.id),
            );
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

    // Spawn a background task that waits for claude's TUI to be
    // ready, auto-approves the one-time trust prompt if seen, pastes
    // the task prompt as one message, then verifies + heals via
    // gemma4. The HTTP response goes back to the iPad immediately;
    // outcome surfaces via `dispatch.outcome` broker events.
    let sid = session_id.clone();
    let state_bg = state.clone();
    let host_bg = host.clone();
    let project_bg = project_name.clone();
    let session_bg = session.clone();
    let role_bg = role_command.clone();
    let prompt_bg = prompt.clone();
    tokio::spawn(async move {
        verify_and_heal_dispatch(
            state_bg, host_bg, sid, role_bg, prompt_bg, project_bg, id, session_bg,
        )
        .await;
    });

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

async fn attention_fragment(State(state): State<AppState>) -> Response {
    let snap = board_view::load_snapshot(&state).await;
    html_response(board_view::attention_strip(&snap))
}

/// Lazy-loaded transcript fragment for an ended (or active) session.
/// Bridges to katulong's `/api/claude-transcript/:uuid` on the host the
/// observation lives on. Returns an HTML fragment that swaps into the
/// transcript tab panel. Falls back to a friendly "not available" when
/// katulong's endpoint 404s (broker meta missing — known issue queued
/// as a katulong task).
async fn observation_transcript_handler(
    AxumPath(obs_id): AxumPath<String>,
    State(state): State<AppState>,
) -> Response {
    let dir = load_dir();
    let obs = match Observation::load(&dir, &obs_id) {
        Ok(o) => o,
        Err(_) => return err_response(StatusCode::NOT_FOUND, "observation not found"),
    };
    if obs.claude_uuid.is_empty() {
        return html_response(maud::html! {
            div.transcript-empty.subtle {
                "no Claude session UUID was captured for this observation — "
                "transcript not available"
            }
        });
    }
    let host = match state.hosts.find(&obs.host) {
        Some(h) => h,
        None => {
            return html_response(maud::html! {
                div.transcript-empty.subtle {
                    "host '" (obs.host) "' is no longer configured — transcript not available"
                }
            });
        }
    };
    let url = format!(
        "{}/api/claude-transcript/{}?limit=500",
        host.base_url(),
        obs.claude_uuid
    );
    let resp = match state.http.get(&url).bearer_auth(&host.api_key).send().await {
        Ok(r) => r,
        Err(e) => {
            // `e.to_string()` would render reqwest::Error::Display, which
            // embeds the request URL — leaking the tunnel hostname into
            // the HTML fragment served to the browser. Keep detail in the
            // log; show a generic message in the UI.
            warn!(host = %obs.host, error = %e, "GET /api/claude-transcript failed");
            return html_response(maud::html! {
                div.transcript-empty.subtle {
                    "transcript fetch failed: network error"
                }
            });
        }
    };
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return html_response(maud::html! {
            div.transcript-empty.subtle {
                "transcript not available (HTTP " (status.as_u16()) ")"
                @if !body.is_empty() {
                    " — " (body)
                }
            }
        });
    }
    let parsed: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(_) => {
            return html_response(maud::html! {
                div.transcript-empty.subtle { "transcript response was malformed" }
            });
        }
    };
    let entries: Vec<board_view::FeedEntry> = parsed
        .get("entries")
        .and_then(|e| serde_json::from_value(e.clone()).ok())
        .unwrap_or_default();
    html_response(board_view::transcript_panel(&entries))
}

/// Mark the current gemma4 proposal for an observation as rejected.
/// We cache `Rejected { hash }` keyed to the summary hash, so this only
/// suppresses the proposal until katulong's summarizer rewrites the
/// summary — at which point gemma4 takes another swing.
async fn observation_kr_reject_handler(
    AxumPath(obs_id): AxumPath<String>,
    State(state): State<AppState>,
) -> Response {
    use crate::serve::categorize::ProposalState;
    let mut cache = state.kr_proposals.write().await;
    let new_state = match cache.get(&obs_id) {
        Some(ProposalState::Some { hash, .. }) => Some(ProposalState::Rejected { hash: *hash }),
        Some(ProposalState::NoFit { hash }) => Some(ProposalState::Rejected { hash: *hash }),
        // Nothing to reject — proposal is pending or absent. No-op,
        // re-render so HTMX has something to swap.
        _ => None,
    };
    if let Some(s) = new_state {
        cache.insert(obs_id.clone(), s);
    }
    drop(cache);
    let payload = serde_json::json!({
        "obs": obs_id,
        "actor": "human",
    });
    let _ = state
        .broker
        .publish("observations/activity", "kr.rejected", payload);
    html_response(render_board(&state).await)
}

#[derive(Deserialize)]
struct ClaudeRespondForm {
    #[serde(default)]
    text: String,
}

/// Forward a user-typed reply to katulong's `/api/claude/respond/:uuid`
/// on the right host. Same shape as katulong's feed-tile reply input —
/// types text, Enter sends, ends with a real Enter at the pane.
async fn claude_respond_handler(
    AxumPath((host_id, uuid)): AxumPath<(String, String)>,
    State(state): State<AppState>,
    Form(body): Form<ClaudeRespondForm>,
) -> Response {
    let trimmed = body.text.trim();
    if trimmed.is_empty() {
        return StatusCode::NO_CONTENT.into_response();
    }
    let host = match state.hosts.find(&host_id) {
        Some(h) => h,
        None => return err_response(StatusCode::BAD_REQUEST, format!("unknown host: {host_id}")),
    };
    let url = format!("{}/api/claude/respond/{}", host.base_url(), uuid);
    let resp = match state
        .http
        .post(&url)
        .bearer_auth(&host.api_key)
        .json(&serde_json::json!({ "text": trimmed }))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            warn!(host = %host.id, error = %e, "POST /api/claude/respond failed");
            return err_response(
                StatusCode::BAD_GATEWAY,
                format!("respond on {}: network error", host.id),
            );
        }
    };
    if !resp.status().is_success() {
        let st = resp.status();
        let txt = resp.text().await.unwrap_or_default();
        return err_response(
            StatusCode::BAD_GATEWAY,
            format!("respond on {}: HTTP {st}: {txt}", host.id),
        );
    }
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
struct ObservationKrForm {
    /// Objective id to file the observation under. Empty resets the
    /// observation back to the misc tray (clears all kr_refs).
    #[serde(default)]
    objective: String,
    /// KR id within the objective. Required when `objective` is non-empty.
    #[serde(default)]
    kr: u64,
}

/// Append a (objective, kr) ref to an observation, or clear all refs
/// when the form is empty (back to misc). Cross-cutting by design — a
/// single session can serve multiple KRs across multiple objectives.
/// To support that without a sharper UI, every accept here adds a new
/// ref rather than replacing; the user can clear-and-re-add if they
/// want a single ref.
async fn observation_kr_handler(
    AxumPath(obs_id): AxumPath<String>,
    State(state): State<AppState>,
    Form(body): Form<ObservationKrForm>,
) -> Response {
    use sipag_core::board::KrRef;
    let dir = load_dir();
    let mut obs = match Observation::load(&dir, &obs_id) {
        Ok(o) => o,
        Err(_) => return err_response(StatusCode::NOT_FOUND, "observation not found"),
    };
    let objective = body.objective.trim().to_string();
    if objective.is_empty() {
        // Empty objective = clear categorization. Strip kr_refs and
        // reset legacy fields back to misc so the inbox sees it again.
        obs.kr_refs.clear();
        obs.project = MISC_PROJECT.to_string();
        obs.kr_id = 0;
    } else {
        let new_ref = KrRef {
            objective: objective.clone(),
            kr: body.kr,
        };
        // Idempotent — don't duplicate an existing ref.
        if !obs
            .kr_refs
            .iter()
            .any(|r| r.objective == new_ref.objective && r.kr == new_ref.kr)
        {
            obs.kr_refs.push(new_ref);
        }
        // Legacy fields stay in sync with the FIRST ref so existing
        // project-scoped views don't go blank during the transition.
        if obs.project == MISC_PROJECT {
            obs.project = objective.clone();
            obs.kr_id = body.kr;
        }
    }
    if let Err(e) = obs.save(&dir) {
        return err_response(StatusCode::INTERNAL_SERVER_ERROR, format!("{e}"));
    }
    {
        let mut cache = state.kr_proposals.write().await;
        cache.remove(&obs_id);
    }
    let payload = serde_json::json!({
        "obs": obs.id(),
        "objective": objective,
        "kr": body.kr,
        "actor": "human",
    });
    let _ = state
        .broker
        .publish("observations/activity", "kr.assigned", payload);
    html_response(render_board(&state).await)
}

/// Compose the prompt fed to `claude -p` when sipag dispatches a
/// task. Three sections: **Context** (initiative + objective
/// aspiration + KR title — the "why"), a **Research** nudge that
/// directs the agent to use `diwa` before touching code, and the
/// **Task** itself (the user-authored task title).
///
/// Lookups are best-effort — missing project / objective / KR
/// degrade silently to whatever sections we can fill. The minimum
/// useful output is always at least the Task section.
fn build_dispatch_prompt(sipag_dir: &std::path::Path, project_name: &str, task: &Task) -> String {
    let project = sipag_core::board::load_project(sipag_dir, project_name).ok();
    let first_objective_id = project.as_ref().and_then(|p| p.serves.first().cloned());
    let aspiration = first_objective_id
        .as_ref()
        .and_then(|id| sipag_core::board::Objective::load(sipag_dir, id).ok())
        .map(|o| o.aspiration);
    let kr_title = match (&first_objective_id, task.key_results.first().copied()) {
        (Some(obj_id), Some(kr_id)) if kr_id != 0 => {
            sipag_core::board::KeyResult::load_for_objective(sipag_dir, obj_id, kr_id)
                .ok()
                .map(|k| k.title)
        }
        _ => None,
    };

    let mut out = String::new();
    out.push_str("## Context\n\n");
    out.push_str(&format!("Initiative: **{}**\n", project_name));
    if let Some(asp) = aspiration.filter(|s| !s.is_empty()) {
        out.push_str(&format!("Objective: \"{}\"\n", asp));
    }
    if let Some(kr) = kr_title.filter(|s| !s.is_empty()) {
        out.push_str(&format!("Key Result: \"{}\"\n", kr));
    }
    out.push_str("\n## Research first\n\n");
    out.push_str(&format!(
        "Before touching code, run `diwa search {project_name} \"<terms relevant to this task>\"` \
         to ground yourself in past decisions, prior attempts, and related work in this initiative. \
         Skim the most relevant commits and let any cited files / SHAs / PR numbers expand into \
         further `diwa search` queries. Don't write code until the picture is clear.\n",
    ));
    out.push_str("\n## Task\n\n");
    out.push_str(&task.title);
    out
}

/// Background driver that runs after every dispatch.
///
/// 1. Poll the pane for up to ~6s waiting for claude's TUI. If the
///    one-time "Do you trust the files in this folder?" prompt
///    appears, auto-select option 1 (yes) — the human can't see this
///    prompt from the iPad and dispatch would otherwise stall on it.
/// 2. Send the task prompt as a single bracketed-paste message so
///    multi-line markdown lands as one chat turn instead of
///    submitting on the first internal newline.
/// 3. Poll `/sessions/by-id/<sid>/status` — if `agent.running` is true,
///    publish `dispatch.success` and exit.
/// 4. Otherwise, enter a bounded recovery loop:
///    - Fetch recent pane scrollback via `/sessions/by-id/<sid>/output?lines=80`.
///    - Hand `(intended_command, scrollback)` to gemma4 and ask for the
///      next keystrokes to recover.
///    - POST gemma4's keystrokes to `/sessions/by-id/<sid>/exec`.
///    - Wait + recheck status. If it took, publish success; otherwise
///      iterate up to MAX_HEAL_ATTEMPTS times.
/// 5. Surface the final outcome on the `observations/activity` topic
///    so the UI's pulse animation fires on the corresponding row.
#[allow(clippy::too_many_arguments)]
async fn verify_and_heal_dispatch(
    state: AppState,
    host: sipag_core::hosts::Host,
    session_id: String,
    role_command: String,
    prompt: String,
    project_name: String,
    task_id: u64,
    session_name_str: String,
) {
    use std::time::Duration;
    use tokio::time::sleep;

    const TUI_WAIT_TICKS: u8 = 12; // ~6s total at 500ms cadence
    const TUI_WAIT_INTERVAL: Duration = Duration::from_millis(500);
    const POST_PASTE_WAIT: Duration = Duration::from_secs(3);
    const POST_HEAL_WAIT: Duration = Duration::from_secs(5);
    const MAX_HEAL_ATTEMPTS: u8 = 3;

    let exec_url = format!("{}/sessions/by-id/{}/exec", host.base_url(), session_id);

    // Phase 1: wait for the claude TUI to be ready, auto-approving
    // the trust-this-folder prompt if seen. Trust prompt only shows
    // on first invocation in a fresh directory; most dispatches will
    // skip straight to "TUI visible" within a tick or two.
    let mut trust_approved = false;
    for _ in 0..TUI_WAIT_TICKS {
        sleep(TUI_WAIT_INTERVAL).await;
        let pane = fetch_pane_scrollback(&state, &host, &session_id).await;
        if !trust_approved && pane_shows_trust_prompt(&pane) {
            tracing::info!(
                host = %host.id,
                session = %session_name_str,
                "trust prompt detected — sending '1' to approve"
            );
            let _ = state
                .http
                .post(&exec_url)
                .bearer_auth(&host.api_key)
                .json(&serde_json::json!({ "input": "1\r" }))
                .send()
                .await;
            trust_approved = true;
            // Loop again to wait for the TUI to settle after approval.
            continue;
        }
        // No trust prompt; assume claude is ready (or near-ready) and
        // proceed to paste. A small race here is fine — if the paste
        // arrives before the input box is interactive, the heal loop
        // will pick up the slack via gemma4.
        break;
    }

    // Phase 2: paste the prompt as one message.
    let prompt_input = wrap_bracketed_paste(&prompt);
    let _ = state
        .http
        .post(&exec_url)
        .bearer_auth(&host.api_key)
        .json(&serde_json::json!({ "input": prompt_input }))
        .send()
        .await;

    // Phase 3: verify claude is processing.
    sleep(POST_PASTE_WAIT).await;
    if check_agent_running(&state, &host, &session_id).await {
        publish_dispatch_outcome(
            &state,
            &host.id,
            &session_name_str,
            &project_name,
            task_id,
            "success",
            0,
            "",
        );
        return;
    }

    tracing::warn!(
        host = %host.id,
        session = %session_name_str,
        task = task_id,
        "dispatch verification failed — entering gemma4 self-heal loop"
    );

    // Synthetic "intended command" description for gemma4. The model
    // sees what we tried to accomplish (launch + paste) so it can
    // reason about scrollback and propose recovery keystrokes.
    let intended_command = format!(
        "Launch `{role_command}` interactively in the pane, then paste this prompt as a single \
         bracketed-paste message:\n---\n{prompt}\n---"
    );

    for attempt in 1..=MAX_HEAL_ATTEMPTS {
        let scrollback = fetch_pane_scrollback(&state, &host, &session_id).await;
        let recovery = match propose_recovery(&state, &intended_command, &scrollback).await {
            Some(r) => r,
            None => {
                tracing::warn!(
                    attempt,
                    "gemma4 declined to propose a recovery action — giving up"
                );
                publish_dispatch_outcome(
                    &state,
                    &host.id,
                    &session_name_str,
                    &project_name,
                    task_id,
                    "unrecoverable",
                    attempt,
                    "gemma4 declined to propose recovery",
                );
                return;
            }
        };
        if recovery.input.is_empty() {
            tracing::info!(attempt, reason = %recovery.reason, "gemma4 marked dispatch unrecoverable");
            publish_dispatch_outcome(
                &state,
                &host.id,
                &session_name_str,
                &project_name,
                task_id,
                "unrecoverable",
                attempt,
                &recovery.reason,
            );
            return;
        }
        tracing::info!(
            attempt,
            reason = %recovery.reason,
            "gemma4 proposed recovery keystrokes; sending"
        );
        let _ = state
            .http
            .post(&exec_url)
            .bearer_auth(&host.api_key)
            .json(&serde_json::json!({ "input": recovery.input }))
            .send()
            .await;
        sleep(POST_HEAL_WAIT).await;
        if check_agent_running(&state, &host, &session_id).await {
            tracing::info!(attempt, "self-heal succeeded");
            publish_dispatch_outcome(
                &state,
                &host.id,
                &session_name_str,
                &project_name,
                task_id,
                "self-healed",
                attempt,
                &recovery.reason,
            );
            return;
        }
    }

    tracing::warn!(
        host = %host.id,
        session = %session_name_str,
        "self-heal exhausted attempts; giving up"
    );
    publish_dispatch_outcome(
        &state,
        &host.id,
        &session_name_str,
        &project_name,
        task_id,
        "failed",
        MAX_HEAL_ATTEMPTS,
        "exhausted attempts",
    );
}

async fn check_agent_running(
    state: &AppState,
    host: &sipag_core::hosts::Host,
    session_id: &str,
) -> bool {
    let url = format!("{}/sessions/by-id/{}/status", host.base_url(), session_id);
    let resp = match state.http.get(&url).bearer_auth(&host.api_key).send().await {
        Ok(r) if r.status().is_success() => r,
        _ => return false,
    };
    let body: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(_) => return false,
    };
    body.get("agent")
        .and_then(|a| a.get("running"))
        .and_then(|r| r.as_bool())
        .unwrap_or(false)
}

async fn fetch_pane_scrollback(
    state: &AppState,
    host: &sipag_core::hosts::Host,
    session_id: &str,
) -> String {
    let url = format!(
        "{}/sessions/by-id/{}/output?lines=80",
        host.base_url(),
        session_id
    );
    let resp = match state.http.get(&url).bearer_auth(&host.api_key).send().await {
        Ok(r) if r.status().is_success() => r,
        _ => return String::new(),
    };
    let body: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(_) => return String::new(),
    };
    body.get("data")
        .and_then(|d| d.as_str())
        .unwrap_or("")
        .to_string()
}

struct RecoveryProposal {
    input: String,
    reason: String,
}

/// Ask gemma4 for the next keystrokes to recover a stuck dispatch.
/// Returns `None` only on transport / parse errors so the caller can
/// distinguish "model declined" (Some with empty input) from "we
/// couldn't even ask."
async fn propose_recovery(
    state: &AppState,
    intended_command: &str,
    scrollback: &str,
) -> Option<RecoveryProposal> {
    use sipag_core::llm::{chat, env_auth, env_host, env_model, ChatMessage, ChatOptions};

    let system = "You are a self-healing dispatch agent for a remote interactive shell.\n\
        Sipag tried to type a command into a tmux pane on a katulong host but the agent \
        didn't launch. Your job: look at what's currently in the pane and propose the \
        next keystrokes that will get the intended command running.\n\
        \n\
        The shell may be in any of these states: in a stuck multi-line continuation \
        (PS2 prompt), inside another REPL, mid-output of a long-running command, \
        showing a recoverable error from a previous attempt. You can send any keys, \
        including newlines (\\n for Enter) and control characters (\\u0003 for Ctrl-C, \
        \\u0004 for Ctrl-D, etc.). Be conservative: prefer small steps that observe \
        before committing.\n\
        \n\
        Reply with a single JSON object on one line. No markdown, no commentary.\n\
        Schema: {\"input\": \"<keys to send next>\", \"reason\": \"<one short phrase>\"}\n\
        \n\
        If the situation is unrecoverable (e.g., wrong host, missing dependency that \
        you can't install from this shell), reply with input set to \"\" and a reason.";

    let user =
        format!("Intended command:\n{intended_command}\n\nRecent pane scrollback:\n{scrollback}",);

    let opts = ChatOptions {
        model: env_model(),
        temperature: 0.2,
        num_predict: Some(1024),
        auth_bearer: env_auth(),
    };
    let messages = vec![ChatMessage::system(system), ChatMessage::user(user)];
    let raw = match chat(&state.http, &env_host(), messages, opts).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("dispatch heal: gemma4 call failed: {e}");
            return None;
        }
    };

    // Same forgiving JSON extractor as categorize.rs uses — models
    // sometimes wrap their JSON answer in prose despite instructions.
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    if end <= start {
        return None;
    }
    let json_str = &raw[start..=end];
    #[derive(serde::Deserialize)]
    struct Reply {
        #[serde(default)]
        input: String,
        #[serde(default)]
        reason: String,
    }
    let parsed: Reply = match serde_json::from_str(json_str) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("dispatch heal: gemma4 reply not JSON ({e}): {raw}");
            return None;
        }
    };
    Some(RecoveryProposal {
        input: parsed.input,
        reason: parsed.reason,
    })
}

#[allow(clippy::too_many_arguments)]
fn publish_dispatch_outcome(
    state: &AppState,
    host_id: &str,
    session_name_str: &str,
    project_name: &str,
    task_id: u64,
    outcome: &str,
    attempts: u8,
    reason: &str,
) {
    let payload = serde_json::json!({
        "host": host_id,
        "session": session_name_str,
        "project": project_name,
        "task_id": task_id,
        "outcome": outcome,
        "attempts": attempts,
        "reason": reason,
    });
    let _ = state
        .broker
        .publish("observations/activity", "dispatch.outcome", payload);
}

/// Build the launch command — just the role command + Enter. The
/// prompt is sent separately as a bracketed paste once claude's TUI
/// is ready, so claude can prompt for permissions like a normal
/// interactive session.
fn build_launch_cmd(role_command: &str) -> String {
    format!("{role_command}\r")
}

/// Wrap text in bracketed-paste markers + final Enter so claude's
/// TUI receives multi-line content as a single message instead of
/// submitting on the first internal newline.
///
/// The wrapped string rides through tmux `send-keys -H`, which
/// chunks at 4096 bytes per command (katulong `lib/session.js`
/// `SEND_KEYS_MAX_BYTES`, see katulong commit 1901018 — tmux's yacc
/// parser overflows past ~9997 args). Long prompts whose paste
/// markers straddle a chunk boundary are untested and may not
/// behave as one paste.
fn wrap_bracketed_paste(text: &str) -> String {
    format!("\x1b[200~{text}\x1b[201~\r")
}

/// Detect claude's "Do you trust the files in this folder?" prompt
/// in pane scrollback. This shows on the first invocation in a fresh
/// directory; sipag auto-selects "yes" so dispatch isn't blocked by
/// a one-time prompt the human can't see from the iPad.
fn pane_shows_trust_prompt(pane: &str) -> bool {
    pane.contains("trust the files in this folder") || pane.contains("Do you trust")
}

#[cfg(test)]
mod dispatch_helpers_tests {
    use super::*;

    #[test]
    fn launch_cmd_appends_cr() {
        assert_eq!(build_launch_cmd("claude"), "claude\r");
        assert_eq!(build_launch_cmd("claude --resume"), "claude --resume\r");
    }

    #[test]
    fn bracketed_paste_wraps_with_markers_and_enter() {
        let wrapped = wrap_bracketed_paste("hello");
        assert_eq!(wrapped, "\x1b[200~hello\x1b[201~\r");
    }

    #[test]
    fn bracketed_paste_preserves_multi_line_verbatim() {
        // Multi-line content survives bracketed-paste-wrap unchanged.
        // This is the point: claude's TUI groups the bytes between
        // markers into one message, so embedded \n becomes part of
        // the message instead of submitting after the first line.
        let prompt = "## Context\n\nLine A\n\nLine B";
        let wrapped = wrap_bracketed_paste(prompt);
        assert!(wrapped.starts_with("\x1b[200~"));
        assert!(wrapped.ends_with("\x1b[201~\r"));
        let inner = &wrapped["\x1b[200~".len()..wrapped.len() - "\x1b[201~\r".len()];
        assert_eq!(inner, prompt);
    }

    #[test]
    fn bracketed_paste_preserves_special_chars_verbatim() {
        // The whole reason we abandoned shell-quoting: $, `, ', ",
        // backslashes, and unicode all need to reach claude as-typed.
        // Bracketed paste passes raw bytes through — no escaping at all.
        let prompt =
            "use $HOME and `whoami` and \"quotes\" and \u{2014} em-dash \u{2018}smart\u{2019}";
        let wrapped = wrap_bracketed_paste(prompt);
        let inner = &wrapped["\x1b[200~".len()..wrapped.len() - "\x1b[201~\r".len()];
        assert_eq!(inner, prompt);
    }

    #[test]
    fn trust_prompt_detected_from_real_text() {
        let pane = "Welcome to Claude Code\n\n\
                    Do you trust the files in this folder?\n\
                    Claude Code may read files in this folder...\n\
                    1. Yes, proceed\n2. No, exit";
        assert!(pane_shows_trust_prompt(pane));
    }

    #[test]
    fn trust_prompt_returns_false_for_normal_pane() {
        assert!(!pane_shows_trust_prompt(""));
        assert!(!pane_shows_trust_prompt("~ ❯ "));
        assert!(!pane_shows_trust_prompt("claude is thinking..."));
    }
}
