//! Maud templates for the HTMX-rendered sipag board.
//!
//! Server-side replacement for `web/src/sipag/app.cljs`. Same three
//! sections (objectives, standing, ideas), same topbar, idea-box,
//! dispatch picker, and toast — but rendered on the server and mutated
//! through `/htmx/*` endpoints.
//!
//! Data loading mirrors `serve::board::list_projects` so the JSON API
//! and the HTML view stay in sync.

use crate::serve::state::AppState;
use maud::{html, Markup, PreEscaped, DOCTYPE};
use sipag_core::board::{
    list_project_names, list_tasks, load_project, KeyResult, Project, ProjectKind, Task, TaskStatus,
};
use sipag_core::hosts::Host;
use sipag_core::pubsub::Envelope;
use std::collections::BTreeMap;

// ── public data shape ────────────────────────────────────────────────

/// Snapshot of the sipag world the templates render off of. Loaded
/// fresh on every request — TOML reads are cheap and avoiding cache
/// invalidation is worth more than micro-perf.
pub struct BoardSnapshot {
    pub projects: Vec<ProjectView>,
    pub hosts: Vec<HostSummary>,
    /// host_id → vec of session names the host reports running.
    pub sessions: BTreeMap<String, Vec<String>>,
    pub error: Option<String>,
}

pub struct ProjectView {
    pub name: String,
    pub kind: ProjectKind,
    pub key_results: Vec<KeyResult>,
    pub tasks: Vec<Task>,
}

#[derive(Clone)]
pub struct HostSummary {
    pub id: String,
    pub url: String,
}

// ── data loading ─────────────────────────────────────────────────────

/// Load a fresh snapshot. Network failures fetching sessions are
/// swallowed (the `▸ running on` indicator just won't appear for that
/// host) — same as the cljs SPA.
pub async fn load_snapshot(state: &AppState) -> BoardSnapshot {
    let projects = match load_projects_blocking(&state.sipag_dir) {
        Ok(p) => p,
        Err(e) => {
            return BoardSnapshot {
                projects: Vec::new(),
                hosts: state.hosts.hosts.iter().map(host_summary).collect(),
                sessions: BTreeMap::new(),
                error: Some(format!("{e}")),
            }
        }
    };

    let mut sessions = BTreeMap::new();
    for h in &state.hosts.hosts {
        let names = fetch_session_names(state, h).await.unwrap_or_default();
        sessions.insert(h.id.clone(), names);
    }

    BoardSnapshot {
        projects,
        hosts: state.hosts.hosts.iter().map(host_summary).collect(),
        sessions,
        error: None,
    }
}

fn host_summary(h: &Host) -> HostSummary {
    HostSummary {
        id: h.id.clone(),
        url: h.base_url().to_string(),
    }
}

fn load_projects_blocking(sipag_dir: &std::path::Path) -> anyhow::Result<Vec<ProjectView>> {
    let names = list_project_names(sipag_dir)?;
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        let Project {
            name: pname, kind, ..
        } = match load_project(sipag_dir, &name) {
            Ok(p) => p,
            Err(_) => continue,
        };
        let tasks = list_tasks(sipag_dir, &name, None).unwrap_or_default();
        let key_results = KeyResult::list(sipag_dir, &name).unwrap_or_default();
        out.push(ProjectView {
            name: pname,
            kind,
            key_results,
            tasks,
        });
    }
    Ok(out)
}

async fn fetch_session_names(state: &AppState, host: &Host) -> anyhow::Result<Vec<String>> {
    #[derive(serde::Deserialize)]
    struct Sess {
        #[serde(default)]
        name: String,
    }
    let url = format!("{}/sessions", host.base_url());
    let resp = state
        .http
        .get(&url)
        .bearer_auth(&host.api_key)
        .send()
        .await?;
    if !resp.status().is_success() {
        return Ok(Vec::new());
    }
    let rows: Vec<Sess> = resp.json().await.unwrap_or_default();
    Ok(rows.into_iter().map(|s| s.name).collect())
}

// ── helpers ─────────────────────────────────────────────────────────

fn is_active(t: &Task) -> bool {
    matches!(
        t.status,
        TaskStatus::Todo | TaskStatus::InProgress | TaskStatus::Review
    )
}

fn is_idea(t: &Task) -> bool {
    matches!(t.status, TaskStatus::Backlog)
}

fn task_running_on<'a>(
    task: &Task,
    project_name: &str,
    sessions: &'a BTreeMap<String, Vec<String>>,
) -> Option<&'a str> {
    let needle = format!("{project_name}--{}", task.role);
    sessions
        .iter()
        .find(|(_, names)| names.iter().any(|n| n == &needle))
        .map(|(id, _)| id.as_str())
}

fn stance_symbol(s: &str) -> &'static str {
    match s {
        "green" => "●",
        "yellow" => "◐",
        "red" => "○",
        "done" => "✓",
        _ => "·",
    }
}

// ── full page ───────────────────────────────────────────────────────

pub fn page(snap: &BoardSnapshot) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width,initial-scale=1";
                title { "sipag" }
                link rel="stylesheet" href="/style.css";
                script src="/js/htmx.min.js" {}
                script src="/js/transport.js" {}
                script src="/js/sipag-live.js" defer {}
            }
            body {
                #app {
                    (topbar(snap))
                    (attention_strip(snap))
                    (board_main(snap))
                    (idea_box(&snap.projects, false))
                    // Mount points for HTMX OOB swaps and live (WS) updates.
                    div #dispatch-picker-mount {}
                    div #toast-mount {}
                    div #ticker.ticker {}
                }
                script {
                    (PreEscaped(INLINE_JS))
                }
            }
        }
    }
}

// ── topbar ──────────────────────────────────────────────────────────

fn topbar(snap: &BoardSnapshot) -> Markup {
    let total_active: usize = snap
        .projects
        .iter()
        .map(|p| p.tasks.iter().filter(|t| is_active(t)).count())
        .sum();
    let host_count = snap.hosts.len();
    html! {
        header.topbar {
            h1 { "sipag" }
            span.subtle { " · what we're optimizing for" }
            span.spacer {}
            @if let Some(err) = snap.error.as_deref() {
                span.err-chip { "error: " (err) }
            } @else {
                span.mesh-chip {
                    (host_count) @if host_count == 1 { " host" } @else { " hosts" }
                }
                span.subtle { " · " (total_active) " active" }
            }
        }
    }
}

// ── board (the polled fragment) ─────────────────────────────────────

/// The whole `<main class="board">`. Returned by `/htmx/board` and by
/// every mutation endpoint. Polls every 5s, but the trigger condition
/// skips the swap when the user is typing in a form so we don't yank
/// half-typed input out from under their fingers.
pub fn board_main(snap: &BoardSnapshot) -> Markup {
    let (objectives, standing): (Vec<&ProjectView>, Vec<&ProjectView>) = snap
        .projects
        .iter()
        .partition(|p| matches!(p.kind, ProjectKind::Objective));

    html! {
        main.board
            id="board"
            "hx-get"="/htmx/board"
            "hx-trigger"="every 5s [!document.activeElement || !document.activeElement.matches('input,textarea')]"
            "hx-swap"="outerHTML"
        {
            div.section-head { "objectives" }
            @if objectives.is_empty() {
                (empty_objectives())
            } @else {
                @for p in &objectives {
                    (objective_card(p, &snap.sessions, &snap.hosts))
                }
            }
            div.section-actions {
                (new_objective_form(false))
            }

            div.section-head { "standing" }
            @if standing.is_empty() {
                div.objective-empty.subtle { "no standing concerns yet" }
            } @else {
                @for p in &standing {
                    (standing_card(p, &snap.sessions, &snap.hosts))
                }
            }
            div.section-actions {
                (new_standing_form(false))
            }
        }
    }
}

fn empty_objectives() -> Markup {
    html! {
        div.empty-state {
            h2 { "no objectives yet" }
            p { "what are you optimizing for?" }
            pre { code { "sipag project add <name> --repo owner/repo" } }
            p.subtle { "or use the + objective button below." }
        }
    }
}

// ── objective card ──────────────────────────────────────────────────

fn objective_card(
    p: &ProjectView,
    sessions: &BTreeMap<String, Vec<String>>,
    hosts: &[HostSummary],
) -> Markup {
    let active: Vec<&Task> = p.tasks.iter().filter(|t| is_active(t)).collect();
    let project_seg = urlencode(&p.name);
    let project_endpoint = format!("/htmx/projects/{project_seg}");
    let loose: Vec<&&Task> = active.iter().filter(|t| t.key_results.is_empty()).collect();

    html! {
        section.objective {
            header.objective-head {
                h2 { (p.name) }
                span.subtle {
                    (active.len()) " active · " (p.key_results.len()) " KR"
                    @if p.key_results.len() != 1 { "s" }
                }
                button.row-delete
                    "hx-delete"=(project_endpoint)
                    "hx-target"="#board"
                    "hx-swap"="outerHTML"
                    "hx-confirm"={"Delete '" (p.name) "'? This removes all KRs and tasks under it."}
                    title="delete objective"
                { "×" }
            }

            @if p.key_results.is_empty() {
                div.objective-empty { "no key results yet — what does success look like?" }
            } @else {
                @for kr in &p.key_results {
                    (kr_row(kr, &p.name, &active, sessions, hosts))
                }
            }

            @if !loose.is_empty() {
                div.loose {
                    div.loose-head { "loose " span.subtle { "no KR" } }
                    ul.tasks {
                        @for t in &loose {
                            (task_row(t, &p.name, sessions, hosts))
                        }
                    }
                }
            }

            div.objective-actions {
                (new_kr_form(&p.name, false))
                (new_task_form(&p.name, false, "task"))
            }
        }
    }
}

fn standing_card(
    p: &ProjectView,
    sessions: &BTreeMap<String, Vec<String>>,
    hosts: &[HostSummary],
) -> Markup {
    let active: Vec<&Task> = p.tasks.iter().filter(|t| is_active(t)).collect();
    let project_seg = urlencode(&p.name);
    let project_endpoint = format!("/htmx/projects/{project_seg}");

    html! {
        section.standing {
            header.objective-head {
                h2 { (p.name) }
                span.subtle { (active.len()) " active" }
                button.row-delete
                    "hx-delete"=(project_endpoint)
                    "hx-target"="#board"
                    "hx-swap"="outerHTML"
                    "hx-confirm"={"Delete '" (p.name) "'? This removes all tasks under it."}
                    title="delete standing"
                { "×" }
            }
            @if active.is_empty() {
                div.objective-empty { "nothing now" }
            } @else {
                ul.tasks {
                    @for t in &active {
                        (task_row(t, &p.name, sessions, hosts))
                    }
                }
            }
            div.objective-actions {
                (new_task_form(&p.name, false, "concern"))
            }
        }
    }
}

// ── KR row ──────────────────────────────────────────────────────────

fn kr_row(
    kr: &KeyResult,
    project_name: &str,
    active_tasks: &[&Task],
    sessions: &BTreeMap<String, Vec<String>>,
    hosts: &[HostSummary],
) -> Markup {
    let stance = kr.stance.as_str();
    let project_seg = urlencode(project_name);
    let kr_endpoint = format!("/htmx/projects/{project_seg}/key-results/{}", kr.id);
    let labels_endpoint = format!("{kr_endpoint}/labels");
    let done_endpoint = format!("{kr_endpoint}/done");
    let discourse_topic = format!("key-results/{}/{}/discourse", project_name, kr.id);
    let kr_tasks: Vec<&&Task> = active_tasks
        .iter()
        .filter(|t| t.key_results.contains(&kr.id))
        .collect();
    let working = kr.labels.iter().any(|l| l == "research" || l == "expand");
    let kr_class = if working {
        "kr working"
    } else if kr.done {
        "kr done"
    } else {
        "kr"
    };

    html! {
        div class=(kr_class) data-kind="key-results" data-project=(project_name) data-id=(kr.id) {
            div.kr-head {
                button.kr-stance.{(stance)}
                    "hx-patch"=(kr_endpoint)
                    "hx-vals"="{\"action\":\"cycle\"}"
                    "hx-target"="#board"
                    "hx-swap"="outerHTML"
                    title={(stance) " — click to cycle"}
                { (stance_symbol(stance)) }
                span.kr-title { (kr.title) }
                button.kr-done
                    "hx-post"=(done_endpoint)
                    "hx-target"="#board"
                    "hx-swap"="outerHTML"
                    title="toggle done"
                { @if kr.done { "✓" } @else { "○" } }
                button.row-delete
                    "hx-delete"=(kr_endpoint)
                    "hx-target"="#board"
                    "hx-swap"="outerHTML"
                    "hx-confirm"="Delete this key result? Tasks attached to it will become loose."
                    title="delete KR"
                { "×" }
            }
            (label_chips(&kr.labels, &labels_endpoint))
            (quick_label_row(&kr.labels, &labels_endpoint))
            (discourse_drawer(&discourse_topic, "key-results", project_name, kr.id))
            @if !kr_tasks.is_empty() {
                ul.tasks.kr-tasks {
                    @for t in &kr_tasks {
                        (task_row(t, project_name, sessions, hosts))
                    }
                }
            }
        }
    }
}

// ── task row ────────────────────────────────────────────────────────

fn task_row(
    task: &Task,
    project_name: &str,
    sessions: &BTreeMap<String, Vec<String>>,
    hosts: &[HostSummary],
) -> Markup {
    let running_on = task_running_on(task, project_name, sessions);
    let status = task.status.to_string();
    let project_seg = urlencode(project_name);
    let task_endpoint = format!("/htmx/projects/{project_seg}/tasks/{}", task.id);
    let labels_endpoint = format!("{task_endpoint}/labels");
    let dispatch_endpoint = format!("/htmx/projects/{project_seg}/tasks/{}/dispatch", task.id);
    let discourse_topic = format!("tasks/{}/{}/discourse", project_name, task.id);
    let dispatchable = !hosts.is_empty() && running_on.is_none();
    let single_host = hosts.len() == 1;
    let working = task.labels.iter().any(|l| l == "research" || l == "expand");
    let li_class = if working { "task working" } else { "task" };

    html! {
        li class=(li_class) data-kind="tasks" data-project=(project_name) data-id=(task.id) {
            div.task-head {
                span.task-id { "#" (task.id) }
                span.task-title { (task.title) }
                button.status-chip.{(status)}
                    "hx-patch"=(task_endpoint)
                    "hx-vals"="{\"action\":\"cycle-status\"}"
                    "hx-target"="#board"
                    "hx-swap"="outerHTML"
                    title={(status) " — click to cycle"}
                { (status) }

                @if dispatchable {
                    @if single_host {
                        @let host_id = &hosts[0].id;
                        button.dispatch-btn
                            "hx-post"=(dispatch_endpoint)
                            "hx-vals"={"{\"host\":\"" (json_escape(host_id)) "\"}"}
                            "hx-target"="#board"
                            "hx-swap"="outerHTML"
                            title={"dispatch on " (host_id)}
                        { "▷" }
                    } @else {
                        @let picker_url = format!(
                            "/htmx/dispatch-picker?project={}&task={}",
                            urlencode(project_name),
                            task.id
                        );
                        button.dispatch-btn
                            "hx-get"=(picker_url)
                            "hx-target"="#dispatch-picker-mount"
                            "hx-swap"="innerHTML"
                            title="dispatch…"
                        { "▷" }
                    }
                }

                button.row-delete
                    "hx-delete"=(task_endpoint)
                    "hx-target"="#board"
                    "hx-swap"="outerHTML"
                    "hx-confirm"="Delete this task?"
                    title="delete task"
                { "×" }
            }
            (label_chips(&task.labels, &labels_endpoint))
            (quick_label_row(&task.labels, &labels_endpoint))
            (discourse_drawer(&discourse_topic, "tasks", project_name, task.id))
            @if let Some(host_id) = running_on {
                div.task-foot { "▸ running on " (host_id) }
            }
        }
    }
}

// ── idea box ────────────────────────────────────────────────────────

pub fn idea_box(projects: &[ProjectView], open: bool) -> Markup {
    let mut ideas: Vec<(&Task, &str)> = Vec::new();
    for p in projects {
        for t in &p.tasks {
            if is_idea(t) {
                ideas.push((t, &p.name));
            }
        }
    }

    if ideas.is_empty() {
        return html! {
            aside.idea-box id="idea-box" {
                button.idea-toggle disabled { "idea box · empty" }
            }
        };
    }

    let class_attr = if open { "idea-box open" } else { "idea-box" };
    let toggle_url = if open {
        "/htmx/idea-box?open=0"
    } else {
        "/htmx/idea-box?open=1"
    };
    let arrow = if open { "▾" } else { "▸" };

    html! {
        aside class=(class_attr) id="idea-box" {
            button.idea-toggle
                "hx-get"=(toggle_url)
                "hx-target"="#idea-box"
                "hx-swap"="outerHTML"
            { (arrow) " idea box · " (ideas.len()) }
            @if open {
                ul.ideas {
                    @for (t, pn) in &ideas {
                        @let project_seg = urlencode(pn);
                        @let task_endpoint = format!("/htmx/projects/{project_seg}/tasks/{}", t.id);
                        li.idea {
                            span.task-id { "#" (t.id) }
                            span.task-title { (t.title) }
                            span.subtle { " · " (pn) }
                            button.idea-activate
                                "hx-patch"=(task_endpoint)
                                "hx-vals"="{\"action\":\"activate\"}"
                                "hx-target"="#board"
                                "hx-swap"="outerHTML"
                                title="promote to active"
                            { "→ activate" }
                            button.row-delete
                                "hx-delete"=(task_endpoint)
                                "hx-target"="#board"
                                "hx-swap"="outerHTML"
                                "hx-confirm"="Delete this task?"
                                title="delete"
                            { "×" }
                        }
                    }
                }
            }
        }
    }
}

// ── dispatch picker ─────────────────────────────────────────────────

pub fn dispatch_picker(project_name: &str, task_id: u64, hosts: &[HostSummary]) -> Markup {
    let dispatch_endpoint = format!(
        "/htmx/projects/{}/tasks/{task_id}/dispatch",
        urlencode(project_name)
    );
    html! {
        div.dispatch-picker-backdrop
            "hx-get"="/htmx/dispatch-picker?clear=1"
            "hx-trigger"="click consume"
            "hx-target"="#dispatch-picker-mount"
            "hx-swap"="innerHTML"
        {
            div.dispatch-picker onclick="event.stopPropagation()" {
                div.dispatch-picker-head { "dispatch task #" (task_id) " on…" }
                ul {
                    @for h in hosts {
                        li {
                            button
                                "hx-post"=(dispatch_endpoint)
                                "hx-vals"={"{\"host\":\"" (json_escape(&h.id)) "\"}"}
                                "hx-target"="#board"
                                "hx-swap"="outerHTML"
                            {
                                (h.id) " "
                                span.subtle { (h.url) }
                            }
                        }
                    }
                }
                button.dispatch-picker-cancel
                    "hx-get"="/htmx/dispatch-picker?clear=1"
                    "hx-target"="#dispatch-picker-mount"
                    "hx-swap"="innerHTML"
                { "cancel" }
            }
        }
    }
}

/// OOB toast — returned alongside a board fragment after dispatch.
/// Auto-dismisses via the inline JS at the bottom of the page.
pub fn oob_toast(message: &str) -> Markup {
    html! {
        div #toast-mount "hx-swap-oob"="innerHTML" {
            div.toast { (message) }
        }
    }
}

/// OOB helper to clear the dispatch picker after a successful dispatch.
pub fn oob_clear_dispatch_picker() -> Markup {
    html! {
        div #dispatch-picker-mount "hx-swap-oob"="innerHTML" {}
    }
}

// ── inline forms ────────────────────────────────────────────────────
//
// All inline forms follow the same shape. The closed form is a
// dashed-border `+ <kind>` button; clicking issues a `hx-get` to
// `/htmx/forms/<key>?...` which returns the open variant. Submit
// targets `#board` and re-renders the entire board, naturally
// collapsing the form back to its `+` state.
//
// Keys (used as DOM ids and as the URL slot in `/htmx/forms/<key>`):
//
//     new-objective                       — root section
//     new-standing                        — root section
//     new-kr                              — opens for the project named in `?project=`
//     new-task                            — opens for the project named in `?project=`
//
// Project name travels via query param so the slug → name round-trip
// is unambiguous (project names allow characters that don't survive
// slugification cleanly).

fn form_dom_id(form_key: &str, project: Option<&str>) -> String {
    match project {
        Some(p) => format!("form-{form_key}--{}", slug(p)),
        None => format!("form-{form_key}"),
    }
}

pub fn new_objective_form(open: bool) -> Markup {
    inline_form(
        "new-objective",
        None,
        "/htmx/projects",
        "post",
        &[
            ("name", "objective name", "name"),
            ("repo", "owner/repo (optional)", "repo"),
        ],
        &[("kind", "objective")],
        "objective",
        open,
    )
}

pub fn new_standing_form(open: bool) -> Markup {
    inline_form(
        "new-standing",
        None,
        "/htmx/projects",
        "post",
        &[("name", "standing concern (e.g. ops, hygiene)", "name")],
        &[("kind", "standing")],
        "standing concern",
        open,
    )
}

pub fn new_kr_form(project: &str, open: bool) -> Markup {
    let endpoint = format!("/htmx/projects/{}/key-results", urlencode(project));
    inline_form(
        "new-kr",
        Some(project),
        &endpoint,
        "post",
        &[(
            "title",
            "what does success look like for this objective?",
            "KR title",
        )],
        &[],
        "key result",
        open,
    )
}

/// Task form. `submit_label` is "task" under objectives, "concern"
/// under standing.
pub fn new_task_form(project: &str, open: bool, submit_label: &'static str) -> Markup {
    let endpoint = format!("/htmx/projects/{}/tasks", urlencode(project));
    let mut fields: Vec<(&'static str, &'static str, &'static str)> =
        vec![("title", "what's the next thing to ship?", "task title")];
    // KR id field only makes sense under objectives. We don't know
    // here, but the cljs version showed it for both with the optional
    // hint — keep parity.
    if submit_label == "task" {
        fields.push(("kr", "kr id, e.g. 1", "KR id (optional)"));
    } else {
        // For standing concerns the equivalent form had a single
        // "labels" field after title.
    }
    fields.push(("labels", "labels, comma separated", "labels"));
    inline_form(
        "new-task",
        Some(project),
        &endpoint,
        "post",
        &fields,
        &[],
        submit_label,
        open,
    )
}

#[allow(clippy::too_many_arguments)]
fn inline_form(
    form_key: &str,
    project: Option<&str>,
    endpoint: &str,
    method: &str,
    fields: &[(&str, &str, &str)],
    extra: &[(&str, &str)],
    submit_label: &str,
    open: bool,
) -> Markup {
    let dom_id = form_dom_id(form_key, project);
    let project_q = project
        .map(|p| format!("&project={}", urlencode(p)))
        .unwrap_or_default();
    let toggle_open = format!("/htmx/forms/{form_key}?open=1{project_q}");
    let toggle_close = format!("/htmx/forms/{form_key}?open=0{project_q}");
    let target_id = format!("#{dom_id}");

    if !open {
        return html! {
            div.form id=(dom_id) {
                button.form-toggle
                    "hx-get"=(toggle_open)
                    "hx-target"=(target_id)
                    "hx-swap"="outerHTML"
                { "+ " (submit_label) }
            }
        };
    }

    // Maud doesn't support dynamic attribute *names*, only values. We
    // pick the right attribute by branching on the method.
    let post = method == "post";
    let patch = method == "patch";
    let del = method == "delete";

    html! {
        div.form.open id=(dom_id) {
            form
                "hx-post"=[if post { Some(endpoint) } else { None }]
                "hx-patch"=[if patch { Some(endpoint) } else { None }]
                "hx-delete"=[if del { Some(endpoint) } else { None }]
                "hx-target"="#board"
                "hx-swap"="outerHTML"
            {
                div.form-body {
                    @for (name, placeholder, aria) in fields {
                        input.form-input
                            type="text"
                            name=(name)
                            placeholder=(placeholder)
                            aria-label=(aria)
                            autocomplete="off";
                    }
                    @for (k, v) in extra {
                        input type="hidden" name=(k) value=(v);
                    }
                    div.form-actions {
                        button.form-submit type="submit" { (submit_label) }
                        button.form-cancel
                            type="button"
                            "hx-get"=(toggle_close)
                            "hx-target"=(target_id)
                            "hx-swap"="outerHTML"
                        { "cancel" }
                    }
                }
            }
        }
    }
}

// ── url helpers ─────────────────────────────────────────────────────

fn slug(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Percent-encode a path segment.
pub fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            other => out.push(other),
        }
    }
    out
}

// ── insights hint pane ──────────────────────────────────────────────

/// Render a hint pane fragment for `/htmx/insights/hint`. Mirrors the
/// spike route's output so the existing CSS keeps working.
pub fn render_hint_rows_external(insights: &[crate::serve::insights::Insight]) -> Markup {
    html! {
        ul.hint-rows {
            @for ins in insights {
                @let cls = format!("hint-row hint-{}", cat_class(&ins.category));
                li class=(cls) {
                    span.hint-cat title=(ins.category) { (cat_symbol(&ins.category)) }
                    span.hint-title { (ins.title) }
                    span.hint-meta {
                        @if !ins.repo.is_empty() {
                            (ins.repo) " · "
                        }
                        (relative_time(&ins.commit_date))
                    }
                }
            }
        }
    }
}

fn cat_symbol(c: &str) -> &'static str {
    match c.to_ascii_lowercase().as_str() {
        "decision" => "●",
        "pattern" => "◐",
        "scar" => "○",
        "learning" => "✓",
        _ => "·",
    }
}

fn cat_class(c: &str) -> String {
    c.to_ascii_lowercase()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-')
        .collect()
}

fn relative_time(iso: &str) -> String {
    use chrono::{DateTime, Utc};
    let Ok(parsed) = DateTime::parse_from_rfc3339(iso) else {
        return String::new();
    };
    let then: DateTime<Utc> = parsed.with_timezone(&Utc);
    let diff = Utc::now().signed_duration_since(then);
    let mins = diff.num_minutes();
    let hrs = diff.num_hours();
    let days = diff.num_days();
    if mins < 1 {
        "just now".into()
    } else if mins < 60 {
        format!("{}m ago", mins)
    } else if hrs < 24 {
        format!("{}h ago", hrs)
    } else if days < 7 {
        format!("{}d ago", days)
    } else if days < 30 {
        format!("{}w ago", days / 7)
    } else {
        format!("{}mo ago", days / 30)
    }
}

// ── label chips + quick toggles ─────────────────────────────────────

const QUICK_LABELS: &[&str] = &[
    "research",
    "expand",
    "priority",
    "blocked",
    "attention",
    "archive",
    "done",
];

pub fn label_chips(labels: &[String], labels_endpoint: &str) -> Markup {
    html! {
        div.labels {
            @for l in labels {
                span.label.chip {
                    (l)
                    button.chip-x
                        "hx-post"=(labels_endpoint)
                        "hx-vals"={"{\"add\":\"\",\"remove\":\"" (json_escape(l)) "\"}"}
                        "hx-target"="#board"
                        "hx-swap"="outerHTML"
                        title={"remove label '" (l) "'"}
                    { "×" }
                }
            }
            form.label-add
                "hx-post"=(labels_endpoint)
                "hx-target"="#board"
                "hx-swap"="outerHTML"
            {
                input type="hidden" name="remove" value="";
                input type="text" name="add" placeholder="+label" autocomplete="off";
            }
        }
    }
}

pub fn quick_label_row(labels: &[String], labels_endpoint: &str) -> Markup {
    html! {
        div.quick-labels {
            @for l in QUICK_LABELS {
                @let active = labels.iter().any(|x| x == l);
                @let cls = if active { "quick-label active" } else { "quick-label" };
                @let payload = if active {
                    format!("{{\"add\":\"\",\"remove\":\"{l}\"}}")
                } else {
                    format!("{{\"add\":\"{l}\",\"remove\":\"\"}}")
                };
                button class=(cls)
                    "hx-post"=(labels_endpoint)
                    "hx-vals"=(payload)
                    "hx-target"="#board"
                    "hx-swap"="outerHTML"
                    title={(l) " — toggle"}
                { (l) }
            }
        }
    }
}

// ── discourse drawer ────────────────────────────────────────────────

pub fn discourse_drawer(topic: &str, kind: &str, project: &str, id: u64) -> Markup {
    let panel_url = format!("/htmx/discourse/{}/{}/{}", kind, urlencode(project), id);
    let post_url = format!(
        "/htmx/projects/{}/{}/{}/discourse",
        urlencode(project),
        kind,
        id
    );
    html! {
        details.discourse-drawer {
            summary.discourse-toggle { "discourse" }
            div.discourse data-topic=(topic)
                "hx-get"=(panel_url)
                "hx-trigger"="toggle from:closest details once"
                "hx-target"="this"
                "hx-swap"="innerHTML"
            {
                div.discourse-empty.subtle { "loading…" }
            }
            form.discourse-post
                "hx-post"=(post_url)
                "hx-target"="this"
                "hx-swap"="none"
            {
                input type="text" name="text" placeholder="add to the conversation…" autocomplete="off";
                button type="submit" { "post" }
            }
        }
    }
}

pub fn discourse_panel(topic: &str, envs: &[Envelope]) -> Markup {
    html! {
        ul.discourse-log data-topic=(topic) {
            @if envs.is_empty() {
                li.discourse-empty.subtle { "no discourse yet — ping a worker or post a thought" }
            } @else {
                @for env in envs {
                    (discourse_row(env))
                }
            }
        }
    }
}

fn discourse_row(env: &Envelope) -> Markup {
    let role = env.kind.as_str();
    let actor = env
        .payload
        .get("actor")
        .and_then(|v| v.as_str())
        .unwrap_or(role);
    let worker = env.payload.get("worker").and_then(|v| v.as_str());
    let text = env
        .payload
        .get("text")
        .and_then(|v| v.as_str())
        .or_else(|| env.payload.get("message").and_then(|v| v.as_str()))
        .unwrap_or("");
    let citations: Vec<&str> = env
        .payload
        .get("citations")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|c| c.as_str()).collect())
        .unwrap_or_default();

    let role_label = match role {
        "human.message" => "human".to_string(),
        "assistant.message" => format!("worker:{}", worker.unwrap_or("ollama")),
        "worker.progress" => format!("worker:{} (progress)", worker.unwrap_or("?")),
        "worker.complete" => format!("worker:{}", worker.unwrap_or("?")),
        "worker.error" => format!("worker:{} (error)", worker.unwrap_or("?")),
        "label.changed" => "system: label".to_string(),
        "done.toggled" => "system: done".to_string(),
        other => other.to_string(),
    };

    html! {
        li.discourse-row data-kind=(role) {
            span.discourse-ts { (env.ts) }
            span.discourse-role { (role_label) }
            @if !text.is_empty() {
                span.discourse-text { (text) }
            }
            @if role == "label.changed" {
                @let add = env.payload.get("add").and_then(|v| v.as_array()).cloned().unwrap_or_default();
                @let remove = env.payload.get("remove").and_then(|v| v.as_array()).cloned().unwrap_or_default();
                span.discourse-text {
                    @for a in &add { "+" (a.as_str().unwrap_or("")) " " }
                    @for r in &remove { "-" (r.as_str().unwrap_or("")) " " }
                }
            }
            @if !citations.is_empty() {
                span.discourse-citations {
                    @for sha in &citations {
                        @let short: String = sha.chars().take(7).collect();
                        span.cite { (short) }
                    }
                }
            }
            @let _ = actor; // referenced for future styling hooks
        }
    }
}

// ── attention strip + ticker ────────────────────────────────────────

pub fn attention_strip(snap: &BoardSnapshot) -> Markup {
    let mut rows: Vec<(String, String, u64, String)> = Vec::new();
    for p in &snap.projects {
        for kr in &p.key_results {
            if kr.labels.iter().any(|l| l == "attention") {
                rows.push((
                    "key-results".to_string(),
                    p.name.clone(),
                    kr.id,
                    kr.title.clone(),
                ));
            }
        }
        for t in &p.tasks {
            if t.labels.iter().any(|l| l == "attention") {
                rows.push(("tasks".to_string(), p.name.clone(), t.id, t.title.clone()));
            }
        }
    }
    html! {
        div #attention-strip class=(if rows.is_empty() { "attention-strip empty" } else { "attention-strip" }) {
            @if rows.is_empty() {
                span.subtle { "all clear" }
            } @else {
                @for (kind, project, id, title) in &rows {
                    div.attention-row {
                        span.attention-kind { (kind) }
                        span.attention-project { (project) }
                        span.attention-id { "#" (id) }
                        span.attention-title { (title) }
                    }
                }
            }
        }
    }
}

pub fn ticker(envs: &[Envelope]) -> Markup {
    html! {
        @for env in envs {
            div.ticker-row data-kind=(env.kind) {
                span.ticker-ts { (env.ts) }
                span.ticker-kind { (env.kind) }
                @let worker = env.payload.get("worker").and_then(|v| v.as_str()).unwrap_or("");
                @let pkind = env.payload.get("kind").and_then(|v| v.as_str()).unwrap_or("");
                @let project = env.payload.get("project").and_then(|v| v.as_str()).unwrap_or("");
                @let id = env.payload.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
                span.ticker-worker { (worker) }
                @if !pkind.is_empty() {
                    span.ticker-target { (pkind) " " (project) " #" (id) }
                }
            }
        }
    }
}

// ── inline JS ───────────────────────────────────────────────────────

/// Minimal client-side glue:
/// 1. Forward Cmd+/ to window.parent so katulong's tile picker still
///    works when sipag is iframed.
/// 2. Auto-dismiss any toast 4 seconds after it lands in the DOM.
const INLINE_JS: &str = r#"
(function () {
  function forwardShortcut(ev) {
    if (!ev.metaKey || ev.key !== '/' || ev.shiftKey) return;
    if (window === window.parent) return;
    try {
      var synthetic = new KeyboardEvent('keydown', {
        key: '/', code: 'Slash', metaKey: true,
        bubbles: true, cancelable: true,
      });
      window.parent.dispatchEvent(synthetic);
    } catch (_) { /* cross-origin, give up */ }
  }
  window.addEventListener('keydown', forwardShortcut, true);

  document.body.addEventListener('htmx:afterSwap', function (ev) {
    var t = ev.target;
    if (t && t.querySelector && t.querySelector('.toast')) {
      setTimeout(function () {
        var mount = document.getElementById('toast-mount');
        if (mount) mount.innerHTML = '';
      }, 4000);
    }
  });
})();
"#;
