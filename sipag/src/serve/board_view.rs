//! Maud templates for the HTMX-rendered sipag board.
//!
//! Server-side replacement for `web/src/sipag/app.cljs`. Same three
//! sections (objectives, standing, ideas), same topbar, idea-box,
//! dispatch picker, and toast — but rendered on the server and mutated
//! through `/htmx/*` endpoints.
//!
//! Data loading mirrors `serve::board::list_projects` so the JSON API
//! and the HTML view stay in sync.

use crate::serve::categorize::{propose_kr, summary_hash, KrChoice, KrProposal, ProposalState};
use crate::serve::state::AppState;
use maud::{html, Markup, PreEscaped, DOCTYPE};
use sipag_core::board::{
    list_project_names, list_tasks, load_project, KeyResult, Project, ProjectKind, Task, TaskStatus,
};
use sipag_mesh::Host;
use std::collections::{BTreeMap, HashMap};

// ── public data shape ────────────────────────────────────────────────

/// Snapshot of the sipag world the templates render off of. Loaded
/// fresh on every request — TOML reads are cheap and avoiding cache
/// invalidation is worth more than micro-perf.
pub struct BoardSnapshot {
    /// Objectives — the asymptotic things we're optimizing for. Top of
    /// the board. Each carries its KRs and the initiatives serving it.
    pub objectives: Vec<ObjectiveView>,
    pub projects: Vec<ProjectView>,
    pub hosts: Vec<HostSummary>,
    /// host_id → vec of session names the host reports running.
    pub sessions: BTreeMap<String, Vec<String>>,
    /// (host_id, session_name) → live meta from katulong's GET /sessions.
    /// Source of truth lives in each host's ~/.katulong/sessions.json;
    /// sipag re-fetches on every snapshot rather than caching to disk so
    /// there's no second source of truth to drift.
    pub live: BTreeMap<(String, String), LiveSessionMeta>,
    /// Observations the observer task has captured. Includes both
    /// uncategorized (`project == "misc"`) and categorized records.
    /// Sorted by `last_seen` desc.
    pub observations: Vec<sipag_core::board::Observation>,
    /// gemma4-suggested KR per misc observation id. Populated lazily by
    /// background tasks; appears in the UI as an accept/pick-another chip.
    pub proposals: HashMap<String, KrProposal>,
    /// Every active (not-done) KR across all projects, used to populate
    /// the "pick another" dropdown on each misc row.
    pub kr_choices: Vec<KrChoice>,
    /// Recent Claude-transcript entries per observation id. Fetched
    /// from each host's `/api/claude-transcript/:uuid` endpoint at
    /// snapshot time; rendered into the expanded `<details>` body so
    /// the user can scan what Claude is doing without leaving sipag.
    pub feeds: HashMap<String, Vec<FeedEntry>>,
    pub error: Option<String>,
    /// Process-lifetime token used as a `?v=` cache-buster on JS asset
    /// URLs. Changes on every server restart so iPad Safari (and other
    /// aggressive HTTP caches) can't keep serving stale transport.js.
    pub boot_id: String,
}

/// Subset of katulong's `/sessions[i].meta` that the board renders.
/// Re-fetched on every snapshot — never persisted on the sipag side.
#[derive(Default)]
pub struct LiveSessionMeta {
    pub auto_title: Option<String>,
    pub summary_short: Option<String>,
    pub summary_long: Option<String>,
    pub cwd: Option<String>,
    pub claude_uuid: Option<String>,
}

/// One Claude-transcript entry as exposed by katulong's
/// `/api/claude-transcript/:uuid` — already normalized server-side.
/// Only the fields sipag's row renders are kept.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
// uuid + ts are deserialized for completeness / future dedup + sort
// keys, but no consumer reads them yet. Suppress dead_code so the
// schema can grow without churning the struct each time.
#[allow(dead_code)]
pub enum FeedEntry {
    User {
        #[serde(default)]
        uuid: String,
        #[serde(default)]
        ts: i64,
        #[serde(default)]
        text: String,
    },
    Assistant {
        #[serde(default)]
        uuid: String,
        #[serde(default)]
        ts: i64,
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        tools: Vec<FeedTool>,
    },
    ToolResult {
        #[serde(default)]
        uuid: String,
        #[serde(default)]
        ts: i64,
        #[serde(default)]
        text: String,
    },
}

#[derive(Debug, Clone, serde::Deserialize)]
#[allow(dead_code)]
pub struct FeedTool {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub target: String,
}

/// Boot id assigned once per process. Used to cache-bust JS assets.
fn boot_id() -> &'static str {
    use std::sync::OnceLock;
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis().to_string())
            .unwrap_or_else(|_| "0".into())
    })
}

pub struct ProjectView {
    pub name: String,
    pub kind: ProjectKind,
    pub key_results: Vec<KeyResult>,
    pub tasks: Vec<Task>,
    pub serves: Vec<String>,
}

/// One objective with its key results + the list of initiatives
/// (project names) that serve it. KRs here are objective-scoped (live
/// under `~/.sipag/objectives/<id>/key-results/`).
pub struct ObjectiveView {
    pub id: String,
    pub name: String,
    pub aspiration: String,
    pub key_results: Vec<KeyResult>,
    /// Project names whose `serves` list contains this objective's id.
    pub serving_initiatives: Vec<String>,
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
                objectives: Vec::new(),
                projects: Vec::new(),
                hosts: state.hosts.hosts.iter().map(host_summary).collect(),
                sessions: BTreeMap::new(),
                live: BTreeMap::new(),
                observations: Vec::new(),
                proposals: HashMap::new(),
                kr_choices: Vec::new(),
                feeds: HashMap::new(),
                error: Some(format!("{e}")),
                boot_id: boot_id().to_string(),
            }
        }
    };

    let mut sessions = BTreeMap::new();
    let mut live = BTreeMap::new();
    for h in &state.hosts.hosts {
        let rows = fetch_sessions_full(state, h).await.unwrap_or_default();
        // Per-host id list, used by `task_running_on` to match the
        // task's pinned `dispatch_session_id` against live sessions.
        // Was a name list before opaque dispatch naming landed.
        let ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();
        sessions.insert(h.id.clone(), ids);
        for r in rows {
            let key = (h.id.clone(), r.name.clone());
            live.insert(key, r.into_live_meta());
        }
    }

    let observations =
        sipag_core::board::Observation::list(&state.sipag_dir, None).unwrap_or_default();

    let objectives = load_objectives_blocking(&state.sipag_dir, &projects);
    let kr_choices = build_kr_choices(&objectives);
    let proposals = resolve_proposals(state, &observations, &live, &kr_choices).await;
    let feeds = fetch_feeds(state, &observations, &live).await;

    BoardSnapshot {
        objectives,
        projects,
        hosts: state.hosts.hosts.iter().map(host_summary).collect(),
        sessions,
        live,
        observations,
        proposals,
        kr_choices,
        feeds,
        error: None,
        boot_id: boot_id().to_string(),
    }
}

/// For every misc observation that has a Claude UUID in its live meta,
/// pull the last few transcript entries from the owning katulong host.
/// Errors degrade silently (empty feed → no panel content). Polled
/// per-render rather than streamed; the existing pulse signal carries
/// the "something happened" cue for now.
async fn fetch_feeds(
    state: &AppState,
    observations: &[sipag_core::board::Observation],
    live: &BTreeMap<(String, String), LiveSessionMeta>,
) -> HashMap<String, Vec<FeedEntry>> {
    const FEED_LIMIT: u32 = 8;
    let mut out = HashMap::new();
    for obs in observations {
        if obs.project != sipag_core::board::MISC_PROJECT {
            continue;
        }
        let uuid = match live
            .get(&(obs.host.clone(), obs.session.clone()))
            .and_then(|m| m.claude_uuid.as_deref())
        {
            Some(u) if !u.is_empty() => u.to_string(),
            _ => continue,
        };
        let host = match state.hosts.hosts.iter().find(|h| h.id == obs.host) {
            Some(h) => h,
            None => continue,
        };
        let entries = fetch_recent_transcript(state, host, &uuid, FEED_LIMIT).await;
        if !entries.is_empty() {
            out.insert(obs.id(), entries);
        }
    }
    out
}

async fn fetch_recent_transcript(
    _state: &AppState,
    _host: &Host,
    _uuid: &str,
    _limit: u32,
) -> Vec<FeedEntry> {
    // The Claude-transcript-proxy path (sipag → katulong → Claude
    // JSONL) was retired in §9 #7. Session activity is now captured
    // by the bridge lens-worker via SSE. A corpus-backed feed view
    // is a follow-up UI task; until then the feed panel is empty.
    Vec::new()
}

/// Flatten every active (not-done) KR across all *objectives* into the
/// list gemma4 picks from and the "pick another" dropdown shows.
/// Project-level (legacy) KRs are deliberately excluded — categorize
/// is the seam where we push everything into the objective-shaped model.
fn build_kr_choices(objectives: &[ObjectiveView]) -> Vec<KrChoice> {
    objectives
        .iter()
        .flat_map(|o| {
            o.key_results
                .iter()
                .filter(|kr| !kr.done)
                .map(|kr| KrChoice {
                    objective: o.id.clone(),
                    objective_aspiration: o.aspiration.clone(),
                    kr: kr.id,
                    kr_title: kr.title.clone(),
                })
        })
        .collect()
}

/// Read the current proposal cache for every misc observation that has
/// a non-empty live summary, and fire-and-forget background gemma4
/// calls for cache misses / stale entries. The render uses whatever
/// proposals are *already* resolved; new ones land on the next render.
async fn resolve_proposals(
    state: &AppState,
    observations: &[sipag_core::board::Observation],
    live: &BTreeMap<(String, String), LiveSessionMeta>,
    kr_choices: &[KrChoice],
) -> HashMap<String, KrProposal> {
    let mut resolved: HashMap<String, KrProposal> = HashMap::new();
    if kr_choices.is_empty() {
        return resolved;
    }
    for obs in observations {
        if obs.project != sipag_core::board::MISC_PROJECT {
            continue;
        }
        let summary = match live
            .get(&(obs.host.clone(), obs.session.clone()))
            .and_then(|m| m.summary_short.as_deref())
        {
            Some(s) if !s.trim().is_empty() => s.to_string(),
            _ => continue,
        };
        let id = obs.id();
        let hash = summary_hash(&summary);

        let needs_run = {
            let cache = state.kr_proposals.read().await;
            match cache.get(&id) {
                Some(ProposalState::Some { proposal, hash: h }) if *h == hash => {
                    resolved.insert(id.clone(), proposal.clone());
                    false
                }
                Some(ProposalState::NoFit { hash: h }) if *h == hash => false,
                Some(ProposalState::Rejected { hash: h }) if *h == hash => false,
                Some(ProposalState::Pending) => false,
                _ => true,
            }
        };

        if needs_run {
            // Mark Pending and fire background task. We don't .await it
            // — the next render reads the cache and picks up the result.
            {
                let mut cache = state.kr_proposals.write().await;
                cache.insert(id.clone(), ProposalState::Pending);
            }
            let http = state.http.clone();
            let cache_ref = state.kr_proposals.clone();
            let kr_choices = kr_choices.to_vec();
            let summary = summary.clone();
            let id_clone = id.clone();
            let host = obs.host.clone();
            let session = obs.session.clone();
            let broker = state.broker.clone();
            tokio::spawn(async move {
                tracing::info!("categorize: spawning gemma4 call for {}/{}", host, session);
                let started = std::time::Instant::now();
                let proposal = propose_kr(&http, &summary, &kr_choices).await;
                let elapsed = started.elapsed();
                match &proposal {
                    Some(p) => tracing::info!(
                        "categorize: {}/{} → {}/kr#{} '{}' ({}%) in {:?}",
                        host,
                        session,
                        p.objective,
                        p.kr,
                        p.kr_title,
                        p.confidence,
                        elapsed
                    ),
                    None => {
                        tracing::info!("categorize: {}/{} → no fit in {:?}", host, session, elapsed)
                    }
                }
                if let Some(ref p) = proposal {
                    let payload = serde_json::json!({
                        "host": host,
                        "session": session,
                        "objective": p.objective,
                        "kr": p.kr,
                        "kr_title": p.kr_title,
                        "confidence": p.confidence,
                        "reason": p.reason,
                    });
                    let _ = broker.publish("observations/activity", "kr.proposed", payload);
                }
                let new_state = match proposal {
                    Some(p) => ProposalState::Some { proposal: p, hash },
                    None => ProposalState::NoFit { hash },
                };
                let mut cache = cache_ref.write().await;
                cache.insert(id_clone, new_state);
            });
        }
    }
    resolved
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
            name: pname,
            kind,
            serves,
            ..
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
            serves,
        });
    }
    Ok(out)
}

fn load_objectives_blocking(
    sipag_dir: &std::path::Path,
    projects: &[ProjectView],
) -> Vec<ObjectiveView> {
    let objectives = match sipag_core::board::Objective::list(sipag_dir) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    objectives
        .into_iter()
        .map(|o| {
            let key_results = sipag_core::board::KeyResult::list_for_objective(sipag_dir, &o.id)
                .unwrap_or_default();
            let serving_initiatives = projects
                .iter()
                .filter(|p| p.serves.iter().any(|sid| sid == &o.id))
                .map(|p| p.name.clone())
                .collect();
            ObjectiveView {
                id: o.id,
                name: o.name,
                aspiration: o.aspiration,
                key_results,
                serving_initiatives,
            }
        })
        .collect()
}

async fn fetch_sessions_full(state: &AppState, host: &Host) -> anyhow::Result<Vec<RemoteSession>> {
    // Body-capped GET /sessions via the async client (sipag #527).
    // `list_sessions` returns the strongly-typed `TmuxSession` shape;
    // we project just the fields this board view cares about via the
    // `RemoteSession` local struct. Drop-on-error semantics preserved
    // — a misbehaving katulong returning oversized JSON now fails
    // closed (Vec::new) instead of OOM'ing the process.
    let url = sipag_core::katulong::sessions_url(host.base_url());
    let body: Vec<RemoteSession> = match state
        .katulong_for(host)
        .get_capped(&url, katulong_client::DEFAULT_BODY_CAP)
        .await
    {
        Ok(b) => b,
        Err(_) => return Ok(Vec::new()),
    };
    Ok(body)
}

/// Subset of katulong's `/sessions` row that the board cares about.
#[derive(serde::Deserialize)]
struct RemoteSession {
    /// Katulong's immutable session id (nanoid-shaped). Required for
    /// `task_running_on` to match a task's pinned
    /// `dispatch_session_id` against the live session list.
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    meta: Option<RemoteMeta>,
}

#[derive(serde::Deserialize, Default)]
struct RemoteMeta {
    #[serde(rename = "autoTitle", default)]
    auto_title: Option<String>,
    #[serde(default)]
    summary: Option<RemoteSummary>,
    #[serde(default)]
    pane: Option<RemotePane>,
    #[serde(default)]
    claude: Option<RemoteClaude>,
}

#[derive(serde::Deserialize, Default)]
struct RemoteSummary {
    #[serde(default)]
    short: Option<String>,
    #[serde(default)]
    long: Option<String>,
}

#[derive(serde::Deserialize, Default)]
struct RemotePane {
    #[serde(default)]
    cwd: Option<String>,
}

#[derive(serde::Deserialize, Default)]
struct RemoteClaude {
    #[serde(default)]
    uuid: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
}

impl RemoteSession {
    fn into_live_meta(self) -> LiveSessionMeta {
        let meta = self.meta.unwrap_or_default();
        let cwd = meta
            .pane
            .as_ref()
            .and_then(|p| p.cwd.clone())
            .or_else(|| meta.claude.as_ref().and_then(|c| c.cwd.clone()));
        let claude_uuid = meta.claude.as_ref().and_then(|c| c.uuid.clone());
        let (summary_short, summary_long) = meta
            .summary
            .map(|s| (s.short, s.long))
            .unwrap_or((None, None));
        LiveSessionMeta {
            auto_title: meta.auto_title,
            summary_short,
            summary_long,
            cwd,
            claude_uuid,
        }
    }
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
    _project_name: &str,
    sessions: &'a BTreeMap<String, Vec<String>>,
) -> Option<&'a str> {
    // Match by the dispatch session id sipag stamped on the task at
    // dispatch time. The previous `{project}--{role}` name-match
    // produced false positives (any session with that name shape, in
    // any project state, lit up "running on") and couldn't survive
    // katulong's auto-summarizer renaming the session. With opaque
    // ids, the back-pointer is stable until the task is re-dispatched.
    //
    // `sessions` is a per-host map of session ids (legacy name kept
    // for code-search parity even though it's now ids, not names).
    let target_id = task.dispatch_session_id.as_deref()?;
    let target_host = task.dispatch_host_id.as_deref();
    sessions.iter().find_map(|(host_id, ids)| {
        if let Some(want_host) = target_host {
            if host_id != want_host {
                return None;
            }
        }
        if ids.iter().any(|i| i == target_id) {
            Some(host_id.as_str())
        } else {
            None
        }
    })
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
                link rel="stylesheet" href=(format!("/style.css?v={}", snap.boot_id));
                script src="/js/htmx.min.js" {}
                // idiomorph-ext registers a "morph" swap algorithm
                // with htmx so the 5s board poll only updates the bits
                // that actually changed instead of nuking the whole
                // <main>. Preserves <details> open state, scroll
                // position, focus, and hover/inspection.
                script src="/js/idiomorph-ext.min.js" {}
                // Cache-bust the JS each restart so iPad Safari can't
                // serve stale copies. The version is the server's start
                // time as millis (set in build_state).
                script src=(format!("/js/transport.js?v={}", snap.boot_id)) {}
                script src=(format!("/js/sipag-live.js?v={}", snap.boot_id)) defer {}
                script src=(format!("/js/sipag-debug.js?v={}", snap.boot_id)) defer {}
            }
            body "hx-ext"="morph" {
                #app {
                    (topbar(snap))
                    (attention_strip(snap))
                    (board_main(snap))
                    (idea_box(&snap.projects, false))
                    // Mount points for HTMX OOB swaps and toasts.
                    div #dispatch-picker-mount {}
                    div #toast-mount {}
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
            "hx-swap"="morph"
        {
            (inbox(snap))

            div.section-head { "objectives" span.subtle { " — what we're optimizing for, asymptotically" } }
            @if snap.objectives.is_empty() {
                (empty_objectives_state())
            } @else {
                @for o in &snap.objectives {
                    (objective_card_v2(snap, o))
                }
            }

            div.section-head { "initiatives" span.subtle { " — current means of approach" } }
            @if objectives.is_empty() && standing.is_empty() {
                (empty_objectives())
            }
            @if !objectives.is_empty() {
                @for p in &objectives {
                    (objective_card(snap, p))
                }
            }
            div.section-actions {
                (new_objective_form(false))
            }

            @if !standing.is_empty() {
                div.section-head { "keeping the lights on" span.subtle { " — operational" } }
                @for p in &standing {
                    (standing_card(snap, p))
                }
                div.section-actions {
                    (new_standing_form(false))
                }
            } @else {
                div.section-actions {
                    (new_standing_form(false))
                }
            }

            (ended_section(snap))
        }
    }
}

fn empty_objectives_state() -> Markup {
    html! {
        div.empty-state.subtle {
            "no objectives yet — define an asymptotic outcome you're optimizing toward"
        }
    }
}

/// Render an objective: aspiration sentence + KRs + serving initiatives.
/// KRs aggregate observations from any initiative serving the objective
/// (via observation.kr_refs).
fn objective_card_v2(snap: &BoardSnapshot, o: &ObjectiveView) -> Markup {
    let active_session_count = snap
        .observations
        .iter()
        .filter(|obs| obs.status == "active" && obs.kr_refs.iter().any(|r| r.objective == o.id))
        .count();
    html! {
        section.objective-v2 {
            header.obj-v2-head {
                h2.obj-v2-aspiration { (o.aspiration) }
                div.obj-v2-meta.subtle {
                    span.obj-v2-id { (o.name) }
                    @if !o.serving_initiatives.is_empty() {
                        span { " · served by " }
                        @for (i, init) in o.serving_initiatives.iter().enumerate() {
                            @if i > 0 { span { ", " } }
                            span.obj-v2-initiative { (init) }
                        }
                    } @else {
                        span { " · no initiative yet" }
                    }
                    @if active_session_count > 0 {
                        span { " · " (active_session_count) " active session"
                            @if active_session_count != 1 { "s" }
                        }
                    }
                }
            }
            @if o.key_results.is_empty() {
                div.obj-v2-empty.subtle { "no key results yet — what trends would show we're approaching it?" }
            } @else {
                div.obj-v2-krs {
                    @for kr in &o.key_results {
                        (objective_kr_row(snap, &o.id, kr))
                    }
                }
            }
        }
    }
}

/// Render one KR under an objective. Aggregates two kinds of work
/// across every initiative that serves the objective:
/// - **Tasks** from any initiative whose `Task.key_results` contains
///   this KR's id (legacy linkage — Task.key_results is `Vec<u64>` and
///   resolves against the served objective's KRs by id).
/// - **Active observations** that point at this KR via `kr_refs`.
fn objective_kr_row(
    snap: &BoardSnapshot,
    objective_id: &str,
    kr: &sipag_core::board::KeyResult,
) -> Markup {
    let stance = kr.stance.as_str();
    let kr_obs: Vec<&sipag_core::board::Observation> = snap
        .observations
        .iter()
        .filter(|obs| {
            obs.status == "active"
                && obs
                    .kr_refs
                    .iter()
                    .any(|r| r.objective == objective_id && r.kr == kr.id)
        })
        .collect();
    // Tasks from initiatives that serve this objective and reference
    // this KR id. The (project, task) pair is preserved so each task's
    // dispatch button + endpoints route correctly.
    let kr_tasks: Vec<(&str, &Task)> = snap
        .projects
        .iter()
        .filter(|p| p.serves.iter().any(|sid| sid == objective_id))
        .flat_map(|p| {
            p.tasks
                .iter()
                .filter(|t| is_active(t) && t.key_results.contains(&kr.id))
                .map(move |t| (p.name.as_str(), t))
        })
        .collect();
    let work_count = kr_tasks.len() + kr_obs.len();
    html! {
        div.obj-v2-kr {
            div.obj-v2-kr-head {
                span.kr-stance.{(stance)} { (stance_symbol(stance)) }
                span.kr-title { (kr.title) }
                @if work_count > 0 {
                    span.kr-active-badge { (work_count) " active" }
                }
            }
            @if !kr_tasks.is_empty() {
                ul.tasks.kr-tasks {
                    @for (project_name, t) in &kr_tasks {
                        (task_row(t, project_name, &snap.sessions, &snap.hosts))
                    }
                }
            }
            @if !kr_obs.is_empty() {
                ul.live-misc.kr-sessions {
                    @for obs in &kr_obs {
                        (live_obs_row(snap, obs))
                    }
                }
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

fn objective_card(snap: &BoardSnapshot, p: &ProjectView) -> Markup {
    let active: Vec<&Task> = p.tasks.iter().filter(|t| is_active(t)).collect();
    let project_seg = urlencode(&p.name);
    let project_endpoint = format!("/htmx/projects/{project_seg}");
    let loose: Vec<&&Task> = active.iter().filter(|t| t.key_results.is_empty()).collect();
    let loose_obs = loose_observations_for_project(snap, &p.name);
    let active_session_count: usize = p
        .key_results
        .iter()
        .map(|k| observations_for_kr(snap, &p.name, k.id).len())
        .sum::<usize>()
        + loose_obs.len();

    html! {
        section.objective {
            header.objective-head {
                h2 { (p.name) }
                span.subtle {
                    (active.len()) " active · " (p.key_results.len()) " KR"
                    @if p.key_results.len() != 1 { "s" }
                    @if active_session_count > 0 {
                        " · " (active_session_count) " session"
                        @if active_session_count != 1 { "s" }
                    }
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
                    (kr_row(snap, kr, &p.name, &active))
                }
            }

            @if !loose.is_empty() || !loose_obs.is_empty() {
                div.loose {
                    div.loose-head { "loose " span.subtle { "no KR" } }
                    @if !loose.is_empty() {
                        ul.tasks {
                            @for t in &loose {
                                (task_row(t, &p.name, &snap.sessions, &snap.hosts))
                            }
                        }
                    }
                    @if !loose_obs.is_empty() {
                        ul.live-misc {
                            @for obs in &loose_obs {
                                (live_obs_row(snap, obs))
                            }
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

fn standing_card(snap: &BoardSnapshot, p: &ProjectView) -> Markup {
    let active: Vec<&Task> = p.tasks.iter().filter(|t| is_active(t)).collect();
    let project_seg = urlencode(&p.name);
    let project_endpoint = format!("/htmx/projects/{project_seg}");
    let project_obs = loose_observations_for_project(snap, &p.name);

    html! {
        section.standing {
            header.objective-head {
                h2 { (p.name) }
                span.subtle {
                    (active.len()) " active"
                    @if !project_obs.is_empty() {
                        " · " (project_obs.len()) " session"
                        @if project_obs.len() != 1 { "s" }
                    }
                }
                button.row-delete
                    "hx-delete"=(project_endpoint)
                    "hx-target"="#board"
                    "hx-swap"="outerHTML"
                    "hx-confirm"={"Delete '" (p.name) "'? This removes all tasks under it."}
                    title="delete standing"
                { "×" }
            }
            @if active.is_empty() && project_obs.is_empty() {
                div.objective-empty { "nothing now" }
            }
            @if !active.is_empty() {
                ul.tasks {
                    @for t in &active {
                        (task_row(t, &p.name, &snap.sessions, &snap.hosts))
                    }
                }
            }
            @if !project_obs.is_empty() {
                ul.live-misc {
                    @for obs in &project_obs {
                        (live_obs_row(snap, obs))
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
    snap: &BoardSnapshot,
    kr: &KeyResult,
    project_name: &str,
    active_tasks: &[&Task],
) -> Markup {
    let stance = kr.stance.as_str();
    let project_seg = urlencode(project_name);
    let kr_endpoint = format!("/htmx/projects/{project_seg}/key-results/{}", kr.id);
    let labels_endpoint = format!("{kr_endpoint}/labels");
    let done_endpoint = format!("{kr_endpoint}/done");
    let kr_tasks: Vec<&&Task> = active_tasks
        .iter()
        .filter(|t| t.key_results.contains(&kr.id))
        .collect();
    let kr_obs = observations_for_kr(snap, project_name, kr.id);
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
                @if !kr_obs.is_empty() {
                    span.kr-active-badge title="active sessions" {
                        (kr_obs.len()) " active"
                    }
                }
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
            @if !kr_tasks.is_empty() {
                ul.tasks.kr-tasks {
                    @for t in &kr_tasks {
                        (task_row(t, project_name, &snap.sessions, &snap.hosts))
                    }
                }
            }
            @if !kr_obs.is_empty() {
                ul.live-misc.kr-sessions {
                    @for obs in &kr_obs {
                        (live_obs_row(snap, obs))
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
            @if let Some(host_id) = running_on {
                @let session_name = format!("{}--{}", project_name, task.role);
                @let host_url = hosts.iter().find(|h| h.id == host_id).map(|h| h.url.as_str());
                div.task-foot {
                    span.subtle { "▸ running on " (host_id) " " }
                    @if let Some(url) = host_url {
                        a.task-jump
                            href={(url) "/?s=" (urlencode(&session_name))}
                            rel="noopener"
                            title={"jump to " (session_name) " on " (host_id)}
                        { "↗" }
                    }
                }
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
                    @for (t, proj_name) in &ideas {
                        @let project_seg = urlencode(proj_name);
                        @let task_endpoint = format!("/htmx/projects/{project_seg}/tasks/{}", t.id);
                        li.idea {
                            span.task-id { "#" (t.id) }
                            span.task-title { (t.title) }
                            span.subtle { " · " (proj_name) }
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
    // Collapse the quick-toggle row behind a <details>. Applied
    // labels stay visible (rendered by `label_chips` separately);
    // the quick-add picker is opt-in to keep the per-item row from
    // being noisy.
    html! {
        details.quick-toggle {
            summary { "labels" }
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
}

// ── live activity (observations) ────────────────────────────────────

/// Tray of katulong sessions sipag has noticed across all configured
/// hosts. Sessions filed under `misc` are uncategorized — the
/// categorize worker (or the user) hasn't yet mapped them to a KR.
/// Active sessions show first; ended ones grey out below.
/// Triage queue at the top of the board: active sessions that haven't
/// been categorized yet. Hidden when empty. Categorized sessions are
/// rendered inline under their KR; ended sessions live in
/// `ended_section` at the bottom.
pub fn inbox(snap: &BoardSnapshot) -> Markup {
    // A session is "uncategorized" when neither the legacy project
    // field nor the new kr_refs has any signal.
    let active_misc: Vec<&sipag_core::board::Observation> = snap
        .observations
        .iter()
        .filter(|o| {
            o.status == "active"
                && o.project == sipag_core::board::MISC_PROJECT
                && o.kr_refs.is_empty()
        })
        .collect();
    if active_misc.is_empty() {
        return html! {};
    }
    html! {
        section.inbox {
            div.section-head {
                "inbox"
                span.subtle { " · " (active_misc.len()) " to triage" }
            }
            ul.live-misc {
                @for obs in &active_misc {
                    (live_obs_row(snap, obs))
                }
            }
        }
    }
}

/// Collapsed history of ended observations. Rendered at the bottom of
/// the board, off the way of the live work.
pub fn ended_section(snap: &BoardSnapshot) -> Markup {
    let ended: Vec<&sipag_core::board::Observation> = snap
        .observations
        .iter()
        .filter(|o| o.status != "active")
        .collect();
    if ended.is_empty() {
        return html! {};
    }
    html! {
        section.ended-section {
            details.live-ended {
                summary {
                    span.section-head-inline { "ended" }
                    span.subtle { " · " (ended.len()) }
                    span.ended-chevron.subtle { " ▸" }
                }
                ul.live-misc.ended {
                    @for obs in &ended {
                        (live_obs_row(snap, obs))
                    }
                }
            }
        }
    }
}

/// Active observations that belong under (project, kr_id). Used by
/// kr_row to render rolled-up sessions inline.
fn observations_for_kr<'a>(
    snap: &'a BoardSnapshot,
    project: &str,
    kr_id: u64,
) -> Vec<&'a sipag_core::board::Observation> {
    snap.observations
        .iter()
        .filter(|o| o.status == "active" && o.project == project && o.kr_id == kr_id)
        .collect()
}

/// Active observations categorized to a project but not to any KR
/// within it (kr_id == 0). Rendered in the project's "loose" area so
/// they're visible even when not yet attached to a specific outcome.
fn loose_observations_for_project<'a>(
    snap: &'a BoardSnapshot,
    project: &str,
) -> Vec<&'a sipag_core::board::Observation> {
    snap.observations
        .iter()
        .filter(|o| o.status == "active" && o.project == project && o.kr_id == 0)
        .collect()
}

fn live_obs_row(snap: &BoardSnapshot, obs: &sipag_core::board::Observation) -> Markup {
    let host_url = find_host_url(snap, &obs.host);
    let live = snap.live.get(&(obs.host.clone(), obs.session.clone()));
    let proposal = snap.proposals.get(&obs.id());
    let kr_choices = &snap.kr_choices;
    let _feed = snap.feeds.get(&obs.id()); // reserved for gemma4 task-progress inference
                                           // `?s=<name>` is katulong's deep-link primitive — its boot path
                                           // (app.js around line 97) reads the param and calls
                                           // `activateSession(name)` if a tile already exists for it, or
                                           // creates one and makes it active otherwise. So clicking always
                                           // resolves to the canonical "this tile is now front-and-center"
                                           // state regardless of whether the session was already open.
                                           //
                                           // We deliberately do NOT set `target="_blank"`. On iOS/macOS,
                                           // when the user has installed katulong's domain as a PWA, the OS
                                           // routes plain in-scope navigations to the PWA; `target="_blank"`
                                           // forces the external-browser path and defeats that. Without a
                                           // target, devices without the PWA installed still get a sensible
                                           // browser-tab open. (Sipag PWA users get sent OUT of the sipag
                                           // PWA — the link is to a different origin, so this is the right
                                           // behavior; we don't want sipag to host katulong as a fragment.)
    let katulong_url = host_url.map(|u| format!("{u}/?s={}", urlencode(&obs.session)));
    // Prefer live snapshot data when present (active sessions); fall
    // back to the archived obs.* fields. This is what gives ended
    // sessions a meaningful row label after katulong stops listing them.
    let auto_title = live
        .and_then(|l| l.auto_title.as_deref())
        .or_else(|| Some(obs.auto_title.as_str()).filter(|s| !s.is_empty()));
    let summary_short = live.and_then(|l| l.summary_short.as_deref());
    let summary_long = live
        .and_then(|l| l.summary_long.as_deref())
        .or_else(|| Some(obs.summary_long.as_str()).filter(|s| !s.is_empty()));
    let cwd_full = live
        .and_then(|l| l.cwd.as_deref())
        .or_else(|| Some(obs.cwd.as_str()).filter(|s| !s.is_empty()));
    let cwd_basename = cwd_full.map(path_basename);
    let claude_uuid = live
        .and_then(|l| l.claude_uuid.as_deref())
        .or_else(|| Some(obs.claude_uuid.as_str()).filter(|s| !s.is_empty()));
    // data-key persists open state across the 5s board poll — the
    // restoration script in sipag-live.js stores opened keys in a Set
    // and re-opens them after every htmx swap.
    let key = format!("{}--{}", obs.host, obs.session);
    let is_ended = obs.status != "active";
    let has_detail_body = summary_long.is_some_and(|s| !s.is_empty())
        || cwd_full.is_some_and(|s| !s.is_empty())
        || claude_uuid.is_some_and(|s| !s.is_empty());

    html! {
        li.live-obs data-host=(obs.host) data-session=(obs.session) {
            details.live-obs-details data-key=(key) {
                summary.live-obs-summary-row {
                    span.live-obs-host { (obs.host) }
                    span.subtle { "/" }
                    span.live-obs-session { (obs.session) }
                    @if let Some(t) = auto_title {
                        @if t != obs.session && !t.is_empty() {
                            span.live-obs-title { " · " (t) }
                        }
                    }
                    @if let Some(c) = cwd_basename {
                        @if !c.is_empty() {
                            span.live-obs-cwd.subtle { " · " (c) }
                        }
                    }
                    span.live-obs-age.subtle { " · " (relative_time(&obs.last_seen)) }
                    @if let Some(s) = summary_short {
                        @if !s.is_empty() {
                            span.live-obs-live-summary.subtle { " — " (s) }
                        }
                    }
                    @if !obs.summary.is_empty() {
                        span.live-obs-categorized-summary { " · " (obs.summary) }
                    }
                    @if obs.kr_id != 0 {
                        span.live-obs-kr { " · KR#" (obs.kr_id) " in " (obs.project) }
                    }
                }
                @if let Some(ref u) = katulong_url {
                    a.live-obs-deeplink href=(u) rel="noopener" aria-label="Open in katulong" { "↗" }
                }
                @if is_ended {
                    (ended_detail_body(snap, obs, summary_long, cwd_full, claude_uuid))
                } @else {
                    div.live-obs-detail {
                        @if has_detail_body {
                            @if let Some(long) = summary_long {
                                @if !long.is_empty() {
                                    p.live-obs-detail-summary { (long) }
                                }
                            }
                            @if let Some(c) = cwd_full {
                                @if !c.is_empty() {
                                    div.live-obs-detail-meta {
                                        span.subtle { "cwd " }
                                        code { (c) }
                                    }
                                }
                            }
                            @if let Some(uuid) = claude_uuid {
                                @if !uuid.is_empty() {
                                    div.live-obs-detail-meta {
                                        span.subtle { "claude " }
                                        code { (uuid) }
                                    }
                                }
                            }
                        } @else {
                            p.live-obs-detail-empty.subtle {
                                "no live context yet — will populate on next summarizer cycle"
                            }
                        }
                        @if obs.project != sipag_core::board::MISC_PROJECT && obs.kr_id != 0 {
                            (progress_panel(snap, &obs.project, obs.kr_id))
                        } @else if obs.project == sipag_core::board::MISC_PROJECT && !kr_choices.is_empty() {
                            (kr_proposal_chip(&obs.id(), proposal, kr_choices))
                        }
                        @if let Some(uuid) = claude_uuid {
                            @if !uuid.is_empty() {
                                (reply_input(&obs.host, uuid))
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Detail body for ended observations: a tag strip (objectives + KRs +
/// initiative + cwd + Claude UUID) and a two-tab panel (summary &
/// learnings / full transcript). Transcript is lazy-loaded via HTMX
/// when the user activates that tab.
fn ended_detail_body(
    snap: &BoardSnapshot,
    obs: &sipag_core::board::Observation,
    summary_long: Option<&str>,
    cwd_full: Option<&str>,
    claude_uuid: Option<&str>,
) -> Markup {
    let id = obs.id();
    let tab_name = format!("ended-tab-{}", id);
    let summary_tab_id = format!("ended-tab-summary-{}", id);
    let transcript_tab_id = format!("ended-tab-transcript-{}", id);
    let transcript_panel_id = format!("ended-panel-transcript-{}", id);
    let transcript_url = format!("/htmx/observations/{}/transcript", urlencode(&id));

    // Build the tag list. Each KR ref → (objective, kr title). The
    // initiative is the legacy obs.project field when set to a real
    // project name.
    let mut objectives_seen: std::collections::BTreeSet<String> = Default::default();
    let mut tag_chips: Vec<Markup> = Vec::new();
    for kref in &obs.kr_refs {
        let kr_title = snap
            .objectives
            .iter()
            .find(|o| o.id == kref.objective)
            .and_then(|o| o.key_results.iter().find(|k| k.id == kref.kr))
            .map(|k| k.title.clone())
            .unwrap_or_else(|| format!("KR#{}", kref.kr));
        let obj_label = kref.objective.clone();
        if objectives_seen.insert(obj_label.clone()) {
            tag_chips.push(html! {
                span.ended-tag.ended-tag-objective {
                    span.ended-tag-key { "objective" }
                    span.ended-tag-val { (obj_label) }
                }
            });
        }
        tag_chips.push(html! {
            span.ended-tag.ended-tag-kr {
                span.ended-tag-key { "kr" }
                span.ended-tag-val { (kr_title) }
            }
        });
    }
    if !obs.project.is_empty() && obs.project != sipag_core::board::MISC_PROJECT {
        tag_chips.push(html! {
            span.ended-tag.ended-tag-initiative {
                span.ended-tag-key { "initiative" }
                span.ended-tag-val { (obs.project) }
            }
        });
    }
    if let Some(c) = cwd_full {
        if !c.is_empty() {
            tag_chips.push(html! {
                span.ended-tag.ended-tag-cwd {
                    span.ended-tag-key { "cwd" }
                    span.ended-tag-val { code { (c) } }
                }
            });
        }
    }
    if let Some(u) = claude_uuid {
        if !u.is_empty() {
            let short: String = u.chars().take(8).collect();
            tag_chips.push(html! {
                span.ended-tag.ended-tag-uuid {
                    span.ended-tag-key { "claude" }
                    span.ended-tag-val { code { (short) "…" } }
                }
            });
        }
    }

    html! {
        div.live-obs-detail.ended-detail {
            @if tag_chips.is_empty() {
                p.live-obs-detail-empty.subtle {
                    "no tags captured for this session — it ran before sipag started archiving meta"
                }
            } @else {
                div.ended-tags {
                    @for chip in &tag_chips { (chip) }
                }
            }
            div.ended-tabs {
                input #(summary_tab_id) type="radio" name=(tab_name) checked;
                label.ended-tab-label for=(summary_tab_id) { "summary & learnings" }
                input #(transcript_tab_id) type="radio" name=(tab_name);
                label.ended-tab-label for=(transcript_tab_id)
                    "hx-get"=(transcript_url)
                    "hx-target"={"#" (transcript_panel_id)}
                    "hx-trigger"="click once"
                    "hx-swap"="innerHTML" {
                    "transcript"
                }
                div.ended-panel.ended-panel-summary {
                    @if let Some(long) = summary_long {
                        @if !long.is_empty() {
                            p.live-obs-detail-summary { (long) }
                        } @else {
                            p.live-obs-detail-empty.subtle {
                                "no summary captured before this session ended"
                            }
                        }
                    } @else {
                        p.live-obs-detail-empty.subtle {
                            "no summary captured before this session ended"
                        }
                    }
                    @if obs.status == "ended" {
                        p.subtle.ended-meta {
                            "ran from " (relative_time(&obs.first_seen))
                            " to " (relative_time(&obs.last_seen))
                        }
                    }
                }
                div.ended-panel.ended-panel-transcript id=(transcript_panel_id) {
                    p.subtle { "click the transcript tab above to load…" }
                }
            }
        }
    }
}

fn path_basename(p: &str) -> &str {
    p.trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(p)
}

/// Render the KR's tasks as a checklist for a categorized session.
/// Tap the box → cycles status (todo → in-progress → review → done).
/// This is the feature-level "what's done" view that replaced the
/// per-tool bullets.
fn progress_panel(snap: &BoardSnapshot, project_name: &str, kr_id: u64) -> Markup {
    let project = match snap.projects.iter().find(|p| p.name == project_name) {
        Some(p) => p,
        None => return html! {},
    };
    let kr_title = project
        .key_results
        .iter()
        .find(|k| k.id == kr_id)
        .map(|k| k.title.as_str())
        .unwrap_or("");
    let tasks: Vec<&Task> = project
        .tasks
        .iter()
        .filter(|t| t.key_results.contains(&kr_id))
        .collect();
    html! {
        div.progress-panel {
            div.progress-panel-header {
                span.progress-panel-label.subtle { "progress" }
                @if !kr_title.is_empty() {
                    span.progress-panel-kr-title { (kr_title) }
                }
            }
            @if tasks.is_empty() {
                p.progress-panel-empty.subtle {
                    "no tasks under this KR yet — add one from the project to track progress"
                }
            } @else {
                ul.progress-tasks {
                    @for t in &tasks {
                        (progress_task_row(project_name, t))
                    }
                }
            }
        }
    }
}

fn progress_task_row(project_name: &str, t: &Task) -> Markup {
    let url = format!("/htmx/projects/{}/tasks/{}", urlencode(project_name), t.id);
    let (glyph, classmod) = match t.status {
        TaskStatus::Done => ("☑", "done"),
        TaskStatus::Review => ("◐", "review"),
        TaskStatus::InProgress => ("◧", "in-progress"),
        TaskStatus::Todo => ("☐", "todo"),
        TaskStatus::Backlog => ("·", "backlog"),
        TaskStatus::Custom(_) => ("·", "custom"),
    };
    let class = format!("progress-task progress-task-{}", classmod);
    html! {
        li.(class) data-task-id=(t.id) {
            form.progress-task-toggle
                "hx-patch"=(url)
                "hx-target"="main.board"
                "hx-swap"="outerHTML" {
                input type="hidden" name="action" value="cycle-status";
                button.progress-task-checkbox type="submit" aria-label="cycle status" {
                    (glyph)
                }
            }
            span.progress-task-title { (t.title) }
        }
    }
}

/// Reply-to-Claude input. Submitting POSTs to sipag's bridge endpoint,
/// which forwards to katulong's /api/claude/respond/:uuid on the host.
fn reply_input(host_id: &str, uuid: &str) -> Markup {
    let url = format!(
        "/htmx/sessions/{}/{}/respond",
        urlencode(host_id),
        urlencode(uuid)
    );
    html! {
        form.reply-form
            "hx-post"=(url)
            "hx-swap"="none"
            "hx-on::after-request"="this.reset()" {
            input.reply-input
                type="text"
                name="text"
                placeholder="reply to claude — Enter to send"
                autocomplete="off";
            button.reply-send type="submit" aria-label="send" { "⏎" }
        }
    }
}

/// The categorize affordance inside an expanded misc row. Either
/// renders gemma4's proposal with an accept button, or just the
/// pick-another dropdown when gemma4 hasn't returned anything yet.
fn kr_proposal_chip(
    obs_id: &str,
    proposal: Option<&KrProposal>,
    kr_choices: &[KrChoice],
) -> Markup {
    let post_url = format!("/htmx/observations/{}/kr", urlencode(obs_id));
    let reject_url = format!("/htmx/observations/{}/kr/reject", urlencode(obs_id));
    html! {
        div.kr-proposal {
            @if let Some(p) = proposal {
                div.kr-proposal-row {
                    span.kr-proposal-label.subtle { "fits → " }
                    span.kr-proposal-objective.subtle { (p.objective) " / " }
                    span.kr-proposal-target { (p.kr_title) }
                    span.kr-proposal-confidence.subtle { " (" (p.confidence) "%)" }
                    @if !p.reason.is_empty() {
                        span.kr-proposal-reason.subtle { " — " (p.reason) }
                    }
                    div.kr-proposal-actions {
                        form.kr-proposal-accept-form
                            "hx-post"=(post_url)
                            "hx-target"="main.board"
                            "hx-swap"="outerHTML" {
                            input type="hidden" name="objective" value=(p.objective);
                            input type="hidden" name="kr" value=(p.kr);
                            button.kr-proposal-accept type="submit" { "accept" }
                        }
                        form.kr-proposal-reject-form
                            "hx-post"=(reject_url)
                            "hx-target"="main.board"
                            "hx-swap"="outerHTML" {
                            button.kr-proposal-reject type="submit" { "reject" }
                        }
                    }
                }
            } @else {
                span.kr-proposal-label.subtle { "uncategorized — " }
            }
            details.kr-proposal-pick-another {
                summary { "pick another" }
                ul.kr-proposal-list {
                    @for kr in kr_choices {
                        li {
                            form
                                "hx-post"=(post_url)
                                "hx-target"="main.board"
                                "hx-swap"="outerHTML" {
                                input type="hidden" name="objective" value=(kr.objective);
                                input type="hidden" name="kr" value=(kr.kr);
                                button type="submit" {
                                    span.subtle { (kr.objective) " / " }
                                    (kr.kr_title)
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn find_host_url<'a>(snap: &'a BoardSnapshot, host_id: &str) -> Option<&'a str> {
    snap.hosts
        .iter()
        .find(|h| h.id == host_id)
        .map(|h| h.url.as_str())
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
        div
            #attention-strip
            class=(if rows.is_empty() { "attention-strip empty" } else { "attention-strip" })
            "hx-get"="/htmx/attention"
            "hx-trigger"="every 5s [!document.activeElement || !document.activeElement.matches('input,textarea')]"
            "hx-swap"="outerHTML" {
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
