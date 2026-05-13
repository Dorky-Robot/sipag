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
    KrStance, Observation, Project, ProjectKind, Task, TaskStatus, MISC_PROJECT,
};
use sipag_core::gate::{self, GateInput};
use sipag_core::katulong::client::{
    AttachError, KatulongAttachClient, KeyName, WaitFrom, DEFAULT_ATTACH_COLS, DEFAULT_ATTACH_ROWS,
};
use sipag_core::katulong::RemoteConfig;
use sipag_core::nudge::{self, NudgeInput};
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
    // Compute the v2 flag once, here, so both the synchronous HTTP
    // exec gating and the background spawn use the same value. v2
    // owns the launch keystroke itself via `attach.input()`, so we
    // skip the sync HTTP exec entirely when v2 is enabled — sending
    // the launch over both paths runs the role command twice.
    let use_v2 = dispatch_v2_enabled();
    // Each dispatch creates a fresh katulong session with an opaque
    // sipag-prefixed name. Katulong's auto-summarizer renames it from
    // session content later; sipag tracks the session by its
    // immutable id (persisted on the task as `dispatch_session_id`),
    // not by the name. Avoids the stale-session-reused class of bug
    // that came from pinning `{project}--{role}` names.
    let session = sipag_core::katulong::generate_dispatch_session_name();

    let session_id = match super::katulong_proxy::create_or_find_session(
        &state.http,
        host.base_url(),
        &host.api_key,
        &host.id,
        &session,
    )
    .await
    {
        Ok(id) => id,
        Err((st, body)) => return err_response(st, body),
    };

    // Pin the dispatch to this task so the board's "running on" badge
    // matches by session id rather than by name. Done as a separate
    // load+save because the task may have prior gate state we want
    // to preserve (status, reason, etc. — the nudge loop will clear
    // those on its first persist if appropriate).
    if let Ok(mut t) = Task::load(&dir, &project_name, id) {
        t.dispatch_session_id = Some(session_id.clone());
        t.dispatch_host_id = Some(host.id.clone());
        t.updated = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        if let Err(e) = t.save(&dir, &project_name) {
            warn!(
                project = %project_name,
                task = id,
                error = %e,
                "dispatch: failed to persist dispatch_session_id/host"
            );
        }
    }

    // Dispatch gate — gemma4 classifies the pane's current state
    // against the project's declared statuses before we type anything.
    // Replaces the per-quirk detection (login banner, permission
    // prompt, mid-paste, etc.) with one LLM judgment. Fails closed:
    // any error from the classifier (gemma down, project missing
    // `dispatchable = true`, model returned unparsable JSON the
    // coercion couldn't recover) aborts dispatch with the task
    // parked at whatever gemma chose (or `needs-human` by fallback).
    match run_dispatch_gate(&state, host, &session_id, &project_name, &task).await {
        Ok(GateOutcome::Dispatch) => {
            // fall through to exec the launch command
        }
        Ok(GateOutcome::Parked {
            status_name,
            reason,
        }) => {
            let toast = format!(
                "task #{id} not dispatched — {status_name}{}",
                reason
                    .as_deref()
                    .map(|r| format!(": {r}"))
                    .unwrap_or_default()
            );
            let board = render_board(&state).await;
            let combined = html! {
                (board)
                (oob_toast(&toast))
                (oob_clear_dispatch_picker())
            };
            return html_response(combined);
        }
        Err((st, body)) => return err_response(st, body),
    }

    if !use_v2 {
        let exec_url = sipag_core::katulong::exec_url(host.base_url(), &session_id);
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
            let raw = exec_resp.text().await.unwrap_or_default();
            warn!(host = %host.id, status = %st, body = %raw, "POST exec returned non-2xx");
            let txt = super::katulong_proxy::sanitize_upstream_body(&raw);
            return err_response(
                StatusCode::BAD_GATEWAY,
                format!("exec on {}: HTTP {st}: {txt}", host.id),
            );
        }
    }

    if let Err(e) = move_task(&dir, &project_name, id, "in-progress") {
        warn!(
            project = %project_name,
            task = id,
            error = %e,
            "task move to in-progress failed (worker is already running)"
        );
    }
    // Clear any reason/human_action left over from a prior parked
    // dispatch — once we're firing, those notes no longer apply. Done
    // as a separate load+save because `move_task` only updates status.
    if let Ok(mut t) = Task::load(&dir, &project_name, id) {
        if t.reason.is_some() || t.human_action.is_some() {
            t.reason = None;
            t.human_action = None;
            t.updated = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
            let _ = t.save(&dir, &project_name);
        }
    }

    // Two dispatch back-ends, gated by `SIPAG_DISPATCH_V2`:
    //
    //   v2 (the attach client): opens a long-lived katulong WS
    //   attach, types `claude\r`, waits on the rolling buffer for
    //   the TUI ready marker, pastes the prompt body, presses
    //   Enter, waits for "esc to interrupt" to confirm Claude is
    //   processing. Every keystroke and submit is a separate
    //   protocol message, which is the property the bug fix (PR
    //   #532) was built around.
    //
    //   legacy: gemma4 keystroke-driving nudge loop via
    //   `verify_and_heal_dispatch`. Kept as a fallback while v2
    //   bakes in production; scheduled for removal in
    //   `docs/dispatch-implementation-plan.md` §11 step 7 once
    //   v2 has proven out.
    //
    // Enable v2 by setting `SIPAG_DISPATCH_V2=1` in the
    // LaunchAgent's environment.
    //
    // Known issue across both paths (documented, deferred):
    // concurrent dispatches of the same task ID race — each call
    // creates its own katulong session, persists its own
    // `dispatch_session_id` last-writer-wins, and spawns its own
    // background task. Worth a per-task in-flight set. v2 doesn't
    // change the race shape; both paths spawn one driver per call.
    let sid = session_id.clone();
    let state_bg = state.clone();
    let host_bg = host.clone();
    let project_bg = project_name.clone();
    let session_bg = session.clone();
    let role_bg = role_command.clone();
    let prompt_bg = prompt.clone();
    let title_bg = task.title.clone();
    tokio::spawn(async move {
        if use_v2 {
            dispatch_via_attach_client(
                state_bg, host_bg, sid, role_bg, prompt_bg, project_bg, id, session_bg, title_bg,
            )
            .await;
        } else {
            verify_and_heal_dispatch(
                state_bg, host_bg, sid, role_bg, prompt_bg, project_bg, id, session_bg,
            )
            .await;
        }
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
    // `obs.claude_uuid` flows in from `KatulongSession.meta.claude.uuid`
    // via the observer poll path — i.e. server-supplied. Validate
    // before URL interpolation for the same reason as session ids
    // (#526). The validator's allow-list is a superset of UUID format.
    if !sipag_core::katulong::is_valid_session_id(&obs.claude_uuid) {
        warn!(
            obs = %obs_id, claude_uuid = %obs.claude_uuid,
            "rejected invalid claude_uuid from upstream observation"
        );
        return html_response(maud::html! {
            div.transcript-empty.subtle { "transcript fetch failed: invalid identifier" }
        });
    }
    let url = sipag_core::katulong::claude_transcript_url(host.base_url(), &obs.claude_uuid, 500);
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
        // Don't render the response body inline — even with maud's
        // HTML auto-escaping, an attacker-controlled body could
        // disrupt layout or pad the fragment with garbage. Operator
        // detail goes to the warn log; the user gets a clean status.
        let raw_body = resp.text().await.unwrap_or_default();
        warn!(host = %obs.host, status = status.as_u16(), body = %raw_body, "transcript fetch returned non-2xx");
        return html_response(maud::html! {
            div.transcript-empty.subtle {
                "transcript not available (HTTP " (status.as_u16()) ")"
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
    // `uuid` comes from the request URL path, supplied by an
    // authenticated sipag user. Validate before forwarding to a
    // katulong URL so a user can't steer the outbound POST at
    // arbitrary paths via `/`, `?`, `..`, etc.
    if !sipag_core::katulong::is_valid_session_id(&uuid) {
        warn!(host = %host_id, uuid = %uuid, "rejected invalid uuid in claude_respond request");
        return err_response(StatusCode::BAD_REQUEST, "invalid uuid");
    }
    let url = sipag_core::katulong::claude_respond_url(host.base_url(), &uuid);
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
        let raw = resp.text().await.unwrap_or_default();
        warn!(host = %host.id, status = %st, body = %raw, "POST /api/claude/respond returned non-2xx");
        let txt = super::katulong_proxy::sanitize_upstream_body(&raw);
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

/// Returns true when `SIPAG_DISPATCH_V2` is truthy. Truthy = any
/// value not in {"", "0", "false", "no", "off"} (case-insensitive).
/// Unset is treated as off.
fn dispatch_v2_enabled() -> bool {
    match std::env::var("SIPAG_DISPATCH_V2") {
        Ok(v) => {
            let s = v.trim();
            !matches!(
                s.to_ascii_lowercase().as_str(),
                "" | "0" | "false" | "no" | "off"
            )
        }
        Err(_) => false,
    }
}

/// Attach-client-driven dispatch (v2 path).
///
/// Opens a long-lived katulong WS attach to the session that was
/// just created by `dispatch_task_handler`, then drives the
/// keystroke handshake mechanically (5 steps, 0-indexed):
///
/// * step 0 — attach open + `input("<role_command>\r")`. The sync
///   HTTP exec is intentionally skipped when v2 is enabled so the
///   role command isn't launched twice.
/// * step 1 — `wait_for(tui_ready_re, FromOffset(pre_launch_offset))`.
///   The offset is snapshotted *before* the launch so the launch
///   echo can't slip into the FromNow snapshot race.
/// * step 2 — `paste(prompt)` (bracketed-paste body, no trailing
///   `\r` — that was the original dispatch bug).
/// * step 3 — `press(KeyName::Enter)` to submit. (A best-effort
///   3s `wait_for(echo of task title)` runs immediately before
///   the submit; an echo timeout is logged but does NOT fail-fast
///   or report as step 3 — only the `press` itself does.)
/// * step 4 — `wait_for(claude_processing_re)` confirms Claude has
///   started processing.
///
/// Every step is a `KatulongAttach` API call; no `wrap_bracketed_paste`,
/// no hand-rolled keystroke routing. Each step has an explicit
/// timeout; failures publish a `dispatch.outcome` event with the
/// step index and a short reason string. On success: `step=4`,
/// `outcome="success"`. Operator tooling MUST key on `(outcome,
/// step)` together — `step` alone is ambiguous because v1's nudge
/// loop publishes iteration counts into the same field.
#[allow(clippy::too_many_arguments)]
async fn dispatch_via_attach_client(
    state: AppState,
    host: sipag_core::hosts::Host,
    session_id: String,
    role_command: String,
    prompt: String,
    project_name: String,
    task_id: u64,
    session_name_str: String,
    task_title: String,
) {
    use std::time::Duration;

    let remote = RemoteConfig {
        url: host.url.clone(),
        api_key: host.api_key.clone(),
    };
    let client = KatulongAttachClient::new(remote);

    // Step 0a: open the attach.
    let attach = match client
        .attach(&session_id, DEFAULT_ATTACH_COLS, DEFAULT_ATTACH_ROWS)
        .await
    {
        Ok(a) => a,
        Err(e) => {
            warn!(host = %host.id, error = %e, "dispatch v2: attach open failed");
            publish_dispatch_outcome(
                &state,
                &host.id,
                &session_name_str,
                &project_name,
                task_id,
                "failed",
                0,
                &v2_step_reason("attach open", e),
            );
            return;
        }
    };

    // Snapshot the stripped-buffer length *before* sending the
    // launch keystroke. The TUI-ready wait will start matching from
    // this offset, so we can't miss a fast render that lands in the
    // gap between `input(...)` returning and `wait_for(...)`
    // registering. See `WaitFrom::FromOffset` docs.
    let pre_launch_offset = attach.stripped_offset().await;

    // Step 0b: send the launch keystroke (e.g., `claude\r`).
    let launch = format!("{role_command}\r");
    if let Err(e) = attach.input(&launch).await {
        finish_v2(
            attach,
            &state,
            &host.id,
            &session_name_str,
            &project_name,
            task_id,
            "failed",
            0,
            &v2_step_reason("launch input", e),
        )
        .await;
        return;
    }

    // Step 1: wait for Claude's TUI to render. 30s accommodates
    // cold starts where MCP servers / auth checks delay the first
    // frame.
    if let Err(e) = attach
        .wait_for(
            tui_ready_re(),
            WaitFrom::FromOffset(pre_launch_offset),
            Some(Duration::from_secs(30)),
        )
        .await
    {
        finish_v2(
            attach,
            &state,
            &host.id,
            &session_name_str,
            &project_name,
            task_id,
            "failed",
            1,
            &v2_step_reason("TUI ready wait", e),
        )
        .await;
        return;
    }

    // Step 2: paste the prompt body.
    if let Err(e) = attach.paste(&prompt).await {
        finish_v2(
            attach,
            &state,
            &host.id,
            &session_name_str,
            &project_name,
            task_id,
            "failed",
            2,
            &v2_step_reason("paste", e),
        )
        .await;
        return;
    }

    // Step 3a: best-effort echo wait. We match the *task title*
    // (the unique-per-dispatch portion of the prompt) rather than
    // the prompt prefix — the prompt body always starts with the
    // same `## Context` header, which would degenerate to a no-op
    // match in any rolling buffer that still has prior dispatch
    // content. A missed echo is logged and we proceed to submit.
    if let Ok(re) = paste_echo_regex(&task_title) {
        if let Err(e) = attach
            .wait_for(&re, WaitFrom::FromNow, Some(Duration::from_secs(3)))
            .await
        {
            warn!(
                task = task_id,
                error = %e,
                "dispatch v2: paste echo not observed; proceeding to submit"
            );
        }
    }

    // Step 3b: submit.
    if let Err(e) = attach.press(KeyName::Enter).await {
        finish_v2(
            attach,
            &state,
            &host.id,
            &session_name_str,
            &project_name,
            task_id,
            "failed",
            3,
            &v2_step_reason("submit", e),
        )
        .await;
        return;
    }

    // Step 4: confirm Claude started processing.
    if let Err(e) = attach
        .wait_for(
            claude_processing_re(),
            WaitFrom::FromNow,
            Some(Duration::from_secs(10)),
        )
        .await
    {
        finish_v2(
            attach,
            &state,
            &host.id,
            &session_name_str,
            &project_name,
            task_id,
            "failed",
            4,
            &v2_step_reason("processing wait", e),
        )
        .await;
        return;
    }

    finish_v2(
        attach,
        &state,
        &host.id,
        &session_name_str,
        &project_name,
        task_id,
        "success",
        4,
        "",
    )
    .await;
}

/// Closes the attach and publishes the dispatch outcome. Pulled out
/// to keep the v2 driver's many error paths terse and to ensure the
/// attach is always closed cleanly even on early returns.
#[allow(clippy::too_many_arguments)]
async fn finish_v2(
    attach: sipag_core::katulong::client::KatulongAttach,
    state: &AppState,
    host_id: &str,
    session_name_str: &str,
    project_name: &str,
    task_id: u64,
    outcome: &str,
    step: u8,
    reason: &str,
) {
    publish_dispatch_outcome(
        state,
        host_id,
        session_name_str,
        project_name,
        task_id,
        outcome,
        step,
        reason,
    );
    attach.close().await;
}

/// Build a regex that matches the first ~20 visible chars of the
/// supplied string — used to confirm the paste echoed into Claude's
/// input box. Escapes regex metacharacters so an `^` or `*` in the
/// title doesn't blow up the matcher. Returns `Err` if the trimmed
/// prefix is empty (which would compile to a regex that matches
/// every position and silently turn the echo wait into a no-op).
fn paste_echo_regex(s: &str) -> Result<regex::Regex, regex::Error> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err(regex::Error::Syntax("empty echo source".to_string()));
    }
    let prefix: String = trimmed.chars().take(20).collect();
    let escaped = regex::escape(prefix.trim());
    if escaped.is_empty() {
        return Err(regex::Error::Syntax(
            "empty echo prefix after trim".to_string(),
        ));
    }
    regex::Regex::new(&escaped)
}

/// One short phrase the operator will see in the `dispatch.outcome`
/// event when an attach step failed. Splits out the variants worth
/// distinguishing for triage; everything else falls through to the
/// `Display` impl.
///
/// The final string is run through `sanitize_upstream_body` so any
/// peer-influenced fragments (server messages, WS protocol errors,
/// tungstenite I/O errors carrying remote text) can't smuggle
/// control characters or escape sequences into the broker event.
fn v2_step_reason(step: &str, err: AttachError) -> String {
    let raw = match err {
        AttachError::Timeout(d) => {
            format!("{step}: timed out after {:.1}s", d.as_secs_f32())
        }
        AttachError::SessionExited(code) => {
            format!("{step}: katulong session exited (code {code})")
        }
        AttachError::SessionRemoved => format!("{step}: katulong session was removed"),
        AttachError::Closed => format!("{step}: attach closed"),
        AttachError::Server(msg) => format!("{step}: katulong: {msg}"),
        other => format!("{step}: {other}"),
    };
    super::katulong_proxy::sanitize_upstream_body(&raw)
}

/// Pattern matching Claude Code's "ready for input" signal. We
/// match the help hint, the version banner, or the bottom-of-pane
/// prompt indicator (`> ` at end of buffer). We deliberately do
/// NOT include `"esc to interrupt"` — that's the BUSY indicator
/// (matched by `claude_processing_re`), and a stale match would
/// resolve the ready-wait against the previous dispatch's tail.
///
/// `expect()` is fine: the pattern is a compile-time literal and
/// failure here is a developer bug surfaced by the test suite.
fn tui_ready_re() -> &'static regex::Regex {
    static CELL: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    CELL.get_or_init(|| {
        regex::Regex::new(r"/help|Claude Code|>\s*$").expect("tui_ready_re compiles")
    })
}

/// Pattern matching Claude Code's "I'm processing your request"
/// indicator. `"esc to interrupt"` is the load-bearing string that
/// only appears while Claude is actively running a tool / streaming
/// a response.
fn claude_processing_re() -> &'static regex::Regex {
    static CELL: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    CELL.get_or_init(|| {
        regex::Regex::new(r"esc to interrupt").expect("claude_processing_re compiles")
    })
}

/// Post-launch nudge loop driven by [`sipag_core::nudge::next_step`].
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

    // 20 ticks × 3s = 60s hard ceiling per dispatch. Long enough for
    // a cold gemma + a few back-and-forth nudges, short enough that
    // a stuck dispatch surfaces as `needs-human` instead of pinning
    // a worker indefinitely.
    const MAX_ITERATIONS: u8 = 20;
    const ITERATION_INTERVAL: Duration = Duration::from_secs(3);
    const MAX_GEMMA_FAILURES: u8 = 3;

    let exec_url = sipag_core::katulong::exec_url(host.base_url(), &session_id);
    let dir = load_dir();

    // Load project so gemma sees the full status menu (each with its
    // description) on every tick. Loading once outside the loop is
    // fine — project.toml doesn't churn during a dispatch.
    let project_cfg = match Project::load(&dir, &project_name) {
        Ok(p) => p,
        Err(e) => {
            warn!(
                project = %project_name,
                task = task_id,
                error = %e,
                "nudge loop: load project failed — cannot drive dispatch"
            );
            publish_dispatch_outcome(
                &state,
                &host.id,
                &session_name_str,
                &project_name,
                task_id,
                "failed",
                0,
                "nudge: project load failed",
            );
            return;
        }
    };

    let mut consecutive_gemma_failures: u8 = 0;
    let mut last_persisted_status: Option<String> = None;

    for iteration in 1..=MAX_ITERATIONS {
        // Brief settle before the first poll so the launch `claude\r`
        // has a moment to render; subsequent ticks pace themselves
        // via the end-of-loop sleep.
        if iteration == 1 {
            sleep(Duration::from_secs(1)).await;
        }

        let pane = fetch_pane_scrollback(&state, &host, &session_id).await;
        let decision = match nudge::next_step(
            &state.http,
            NudgeInput {
                task_title: &prompt,
                task_role_cmd: &role_command,
                intended_prompt: &prompt,
                statuses: &project_cfg.statuses,
                session_output: &pane,
                iteration,
                max_iterations: MAX_ITERATIONS,
            },
        )
        .await
        {
            Ok(d) => {
                consecutive_gemma_failures = 0;
                d
            }
            Err(e) => {
                consecutive_gemma_failures += 1;
                warn!(
                    iteration,
                    error = %e,
                    consecutive_failures = consecutive_gemma_failures,
                    "nudge loop: gemma4 call failed"
                );
                if consecutive_gemma_failures >= MAX_GEMMA_FAILURES {
                    park_task_with_reason(
                        &dir,
                        &project_name,
                        task_id,
                        "needs-human",
                        &format!(
                            "nudge loop: gemma4 unavailable after {consecutive_gemma_failures} attempts"
                        ),
                        Some(
                            "Check that local ollama is reachable (OLLAMA_HOST) and that the \
                             configured model is loaded."
                                .into(),
                        ),
                    );
                    publish_dispatch_outcome(
                        &state,
                        &host.id,
                        &session_name_str,
                        &project_name,
                        task_id,
                        "failed",
                        iteration,
                        "nudge: gemma unavailable",
                    );
                    return;
                }
                sleep(ITERATION_INTERVAL).await;
                continue;
            }
        };

        // Persist status change as soon as gemma reports it — the
        // board polls every 5s and the operator should see the row
        // move during the loop, not just at the end.
        if last_persisted_status.as_deref() != Some(decision.status_name.as_str()) {
            persist_task_state(&dir, &project_name, task_id, &decision);
            last_persisted_status = Some(decision.status_name.clone());
        }

        if let Some(keys) = decision.keystrokes.as_deref() {
            if !keys.is_empty() {
                tracing::info!(
                    iteration,
                    status = %decision.status_name,
                    reason = %decision.reason,
                    bytes = keys.len(),
                    "nudge loop: sending keystrokes"
                );
                let _ = state
                    .http
                    .post(&exec_url)
                    .bearer_auth(&host.api_key)
                    .json(&serde_json::json!({ "input": keys }))
                    .send()
                    .await;
            }
        }

        if decision.done {
            tracing::info!(
                iteration,
                status = %decision.status_name,
                reason = %decision.reason,
                "nudge loop: terminal — exiting"
            );
            let outcome = if decision.status_name == "needs-human" {
                "needs-human"
            } else {
                "success"
            };
            publish_dispatch_outcome(
                &state,
                &host.id,
                &session_name_str,
                &project_name,
                task_id,
                outcome,
                iteration,
                &decision.reason,
            );
            return;
        }

        sleep(ITERATION_INTERVAL).await;
    }

    // Iteration budget exhausted without `done=true`. Park with the
    // last status gemma reported (most likely something that wasn't
    // converging) and a fixed reason so the operator can act.
    park_task_with_reason(
        &dir,
        &project_name,
        task_id,
        "needs-human",
        &format!("nudge loop: budget of {MAX_ITERATIONS} iterations exhausted without progress"),
        Some(
            "Inspect the katulong session manually — gemma4 was nudging without converging.".into(),
        ),
    );
    publish_dispatch_outcome(
        &state,
        &host.id,
        &session_name_str,
        &project_name,
        task_id,
        "failed",
        MAX_ITERATIONS,
        "nudge: budget exhausted",
    );
}

/// Persist a nudge decision's status/reason/human_action onto the
/// task file. Logs and swallows errors — the dispatch outcome event
/// is the authoritative signal, and a transient task-save failure
/// shouldn't tear down the loop mid-flight.
fn persist_task_state(
    dir: &std::path::Path,
    project_name: &str,
    task_id: u64,
    decision: &sipag_core::nudge::NudgeDecision,
) {
    let mut t = match Task::load(dir, project_name, task_id) {
        Ok(t) => t,
        Err(e) => {
            warn!(
                project = %project_name,
                task = task_id,
                error = %e,
                "nudge loop: task reload failed during status persist"
            );
            return;
        }
    };
    t.status = TaskStatus::parse(&decision.status_name);
    t.reason = if decision.reason.trim().is_empty() {
        None
    } else {
        Some(decision.reason.clone())
    };
    t.human_action = decision.human_action.clone();
    t.updated = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    if let Err(e) = t.save(dir, project_name) {
        warn!(
            project = %project_name,
            task = task_id,
            error = %e,
            "nudge loop: task save failed during status persist"
        );
    }
}

/// Park a task at an arbitrary status with a fixed reason — used by
/// the nudge loop's terminal failure paths (gemma unavailable, budget
/// exhausted) where no `NudgeDecision` exists to copy from.
fn park_task_with_reason(
    dir: &std::path::Path,
    project_name: &str,
    task_id: u64,
    status_name: &str,
    reason: &str,
    human_action: Option<String>,
) {
    let mut t = match Task::load(dir, project_name, task_id) {
        Ok(t) => t,
        Err(e) => {
            warn!(
                project = %project_name,
                task = task_id,
                error = %e,
                "nudge loop: task reload failed during park"
            );
            return;
        }
    };
    t.status = TaskStatus::parse(status_name);
    t.reason = Some(reason.to_string());
    t.human_action = human_action;
    t.updated = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    if let Err(e) = t.save(dir, project_name) {
        warn!(
            project = %project_name,
            task = task_id,
            error = %e,
            "nudge loop: task save failed during park"
        );
    }
}

/// What the gate decided about a dispatch attempt.
enum GateOutcome {
    /// The session matches the project's dispatchable status — fire
    /// the launch command.
    Dispatch,
    /// Gemma classified the session into some other column. The task
    /// has already been parked there (status + reason + human_action
    /// persisted) by the time this is returned; the caller just needs
    /// to render a toast.
    Parked {
        status_name: String,
        reason: Option<String>,
    },
}

/// Run the dispatch gate for the web-UI path. On classify failure
/// (gemma down, project missing `dispatchable = true`, parse problems
/// the coercion couldn't recover) returns `Err((StatusCode, body))`
/// so the caller can fail closed.
///
/// Side effect: when the chosen status isn't the dispatchable one,
/// the task file is updated in place (status + reason + human_action
/// + updated timestamp) before the function returns.
async fn run_dispatch_gate(
    state: &AppState,
    host: &sipag_core::hosts::Host,
    session_id: &str,
    project_name: &str,
    task: &Task,
) -> Result<GateOutcome, (StatusCode, String)> {
    let dir = load_dir();
    let project_cfg = match Project::load(&dir, project_name) {
        Ok(p) => p,
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("load project: {e}"),
            ))
        }
    };
    let dispatchable_name = match project_cfg.dispatchable_status() {
        Ok(s) => s.name.clone(),
        Err(e) => return Err((StatusCode::CONFLICT, format!("{e}"))),
    };

    let role_command = sipag_core::board::Role::load(&dir, project_name, &task.role)
        .map(|r| r.command)
        .unwrap_or_else(|_| "claude".to_string());

    // Same pane source `verify_and_heal_dispatch` uses for self-heal
    // — last 80 lines via captureVisiblePane plain text. Empty string
    // when katulong is unreachable; the gate handles that case (gemma
    // will see no signal and route to needs-human).
    let session_output = fetch_pane_scrollback(state, host, session_id).await;

    let decision = match gate::classify(
        &state.http,
        GateInput {
            task_title: &task.title,
            task_role: &role_command,
            statuses: &project_cfg.statuses,
            session_output: &session_output,
        },
    )
    .await
    {
        Ok(d) => d,
        Err(e) => {
            warn!(
                project = %project_name,
                task = task.id,
                error = %e,
                "dispatch gate: classify call failed"
            );
            return Err((
                StatusCode::BAD_GATEWAY,
                format!(
                    "dispatch gate: gemma4 classify failed — aborting dispatch. {}",
                    e
                ),
            ));
        }
    };

    if decision.status_name == dispatchable_name {
        return Ok(GateOutcome::Dispatch);
    }

    // Park the task at gemma's chosen status, persisting the reason +
    // human_action so the board renders the blocker next to the row.
    let mut t = match Task::load(&dir, project_name, task.id) {
        Ok(t) => t,
        Err(e) => {
            warn!(
                project = %project_name,
                task = task.id,
                error = %e,
                "dispatch gate: parked task reload failed"
            );
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("reload task: {e}"),
            ));
        }
    };
    t.status = TaskStatus::parse(&decision.status_name);
    t.reason = if decision.reason.trim().is_empty() {
        None
    } else {
        Some(decision.reason.clone())
    };
    t.human_action = decision.human_action.clone();
    t.updated = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    if let Err(e) = t.save(&dir, project_name) {
        warn!(
            project = %project_name,
            task = task.id,
            error = %e,
            "dispatch gate: parked task save failed"
        );
        return Err((StatusCode::INTERNAL_SERVER_ERROR, format!("save task: {e}")));
    }

    Ok(GateOutcome::Parked {
        status_name: decision.status_name,
        reason: t.reason,
    })
}

async fn fetch_pane_scrollback(
    state: &AppState,
    host: &sipag_core::hosts::Host,
    session_id: &str,
) -> String {
    let url = sipag_core::katulong::output_lines_url(host.base_url(), session_id, 80);
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

#[cfg(test)]
mod dispatch_helpers_tests {
    use super::*;

    #[test]
    fn launch_cmd_appends_cr() {
        assert_eq!(build_launch_cmd("claude"), "claude\r");
        assert_eq!(build_launch_cmd("claude --resume"), "claude --resume\r");
    }

    #[test]
    fn paste_echo_regex_matches_title_prefix() {
        let re = paste_echo_regex("Fix the dispatch race").expect("compiles");
        assert!(re.is_match("...some prefix Fix the dispatch race trailing"));
    }

    #[test]
    fn paste_echo_regex_escapes_meta() {
        // `*` would be a quantifier without escape; `regex::escape`
        // turns it into a literal match.
        let re = paste_echo_regex("** start! [bug]").expect("compiles");
        assert!(re.is_match("pasted: ** start! [bug] continues"));
    }

    #[test]
    fn paste_echo_regex_empty_returns_err() {
        // Vacuous regex (matches every position) would silently turn
        // the echo wait into a no-op — refuse to compile it.
        assert!(paste_echo_regex("").is_err());
        assert!(paste_echo_regex("   \t  \n").is_err());
    }

    #[test]
    fn paste_echo_regex_caps_at_twenty_chars() {
        let title = "abcdefghijklmnopqrstuvwxyz"; // 26 chars
        let re = paste_echo_regex(title).expect("compiles");
        // Matches the first 20 chars but not the suffix.
        assert!(re.is_match("...abcdefghijklmnopqrst..."));
        assert!(!re.is_match("uvwxyz"));
    }

    #[test]
    fn v2_step_reason_formats_timeout_as_seconds() {
        use std::time::Duration;
        assert_eq!(
            v2_step_reason(
                "TUI ready wait",
                AttachError::Timeout(Duration::from_millis(15_500))
            ),
            "TUI ready wait: timed out after 15.5s"
        );
    }

    #[test]
    fn v2_step_reason_distinguishes_session_terminal_states() {
        assert!(v2_step_reason("step", AttachError::SessionExited(2))
            .contains("session exited (code 2)"));
        assert!(v2_step_reason("step", AttachError::SessionRemoved).contains("session was removed"));
        assert!(v2_step_reason("step", AttachError::Closed).contains("attach closed"));
    }

    #[test]
    fn v2_step_reason_sanitizes_server_message() {
        // Control bytes from a malicious / corrupted katulong reply
        // must not land in the broker event verbatim.
        let r = v2_step_reason(
            "step",
            AttachError::Server("evil\x07\x1b[31mred\x1b[0m".to_string()),
        );
        assert!(!r.contains('\x07'), "bell byte leaked: {r:?}");
        assert!(!r.contains('\x1b'), "ESC byte leaked: {r:?}");
    }

    #[test]
    fn v2_step_reason_sanitizes_all_variants_not_just_server() {
        // Round-2 review: Wire / Transport / Connect carry
        // peer-influenced strings (tungstenite error text,
        // truncated WS frames). Sanitization must apply to the
        // final formatted string, not only the Server arm.
        for variant in [
            AttachError::Wire("malformed\x07\x1b[Aframe".to_string()),
            AttachError::Transport("ws\x07\x1bclosed".to_string()),
            AttachError::Connect("dns\x07lookup".to_string()),
            AttachError::InvalidUrl("bad\x1b[31murl".to_string()),
        ] {
            let r = v2_step_reason("step", variant);
            assert!(!r.contains('\x07'), "bell byte leaked: {r:?}");
            assert!(!r.contains('\x1b'), "ESC byte leaked: {r:?}");
        }
    }

    #[test]
    fn dispatch_v2_enabled_off_set() {
        // SAFETY: env vars are process-global. Cargo can parallelize
        // tests within a binary across threads, but a workspace grep
        // confirms SIPAG_DISPATCH_V2 is only read here and from the
        // dispatch HTTP handler — and no other `#[test]` exercises
        // that handler. As long as that invariant holds, this test
        // has the env var to itself.
        for off in ["", "0", "false", "FALSE", "no", "off", "Off"] {
            unsafe {
                std::env::set_var("SIPAG_DISPATCH_V2", off);
            }
            assert!(!dispatch_v2_enabled(), "{off:?} should be off");
        }
        for on in ["1", "true", "yes", "on", "anything-else"] {
            unsafe {
                std::env::set_var("SIPAG_DISPATCH_V2", on);
            }
            assert!(dispatch_v2_enabled(), "{on:?} should be on");
        }
        unsafe {
            std::env::remove_var("SIPAG_DISPATCH_V2");
        }
        assert!(!dispatch_v2_enabled());
    }

    #[test]
    fn static_tui_patterns_compile() {
        // Pins the static patterns so a future edit that breaks a
        // regex fails at test time instead of in production where
        // it would panic the dispatch task.
        let ready = tui_ready_re();
        let processing = claude_processing_re();
        assert!(ready.is_match("Claude Code v0.5"));
        assert!(ready.is_match("/help for help"));
        assert!(ready.is_match("> ")); // empty prompt with cursor
        assert!(
            !ready.is_match("esc to interrupt"),
            "BUSY signal must not satisfy ready wait"
        );
        assert!(processing.is_match("(esc to interrupt)"));
    }
}
