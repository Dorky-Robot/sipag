//! Passive observer of katulong sessions across all configured hosts.
//!
//! sipag is bidirectional with katulong: sometimes sipag dispatches a
//! session (`sipag dispatch <task_id>`), but the user can also start
//! sessions directly from katulong's UI. The observer makes the second
//! flow first-class — every 15s it polls each host's `GET /sessions`
//! endpoint and writes/updates an `Observation` record on disk. The UI
//! then surfaces these under the synthetic `misc` project until a
//! categorize worker (separate module) maps them to a real KR.
//!
//! Why polling and not a websocket subscription? Two reasons:
//! 1. Katulong's existing pub/sub is per-watchlist-topic; there isn't
//!    yet a `sessions/lifecycle` topic we can subscribe to. Polling is
//!    a low-cost first cut that doesn't depend on katulong changes.
//! 2. 15s × 3 hosts = 12 calls/min — well within the bridge budget,
//!    and the staleness floor matches the rest of sipag's UI cadence.
//!
//! When katulong gains a `sessions/lifecycle` pub/sub topic, this file
//! is the natural place to add a subscriber that suplements the poll.

use crate::serve::state::AppState;
use anyhow::{Context, Result};
use serde::Deserialize;
use sipag_core::board::Observation;
use sipag_core::hosts::Host;
use std::collections::HashSet;
use std::time::Duration;

const POLL_INTERVAL: Duration = Duration::from_secs(15);

/// Spawn the observer task. No-op if no hosts are configured.
pub fn spawn(state: AppState) {
    if state.hosts.hosts.is_empty() {
        tracing::warn!("observers: no hosts in hosts.toml; observer task not started");
        return;
    }
    tokio::spawn(run(state));
}

async fn run(state: AppState) {
    tracing::info!(
        "observers: starting; polling {} host(s) every {}s",
        state.hosts.hosts.len(),
        POLL_INTERVAL.as_secs()
    );
    let mut interval = tokio::time::interval(POLL_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        if let Err(e) = scan_all_hosts(&state).await {
            tracing::warn!("observers: scan cycle failed: {e}");
        }
    }
}

async fn scan_all_hosts(state: &AppState) -> Result<()> {
    for host in &state.hosts.hosts {
        match scan_host(state, host).await {
            Ok(n) => {
                if n > 0 {
                    tracing::debug!("observers: host {} → {} session(s)", host.id, n);
                }
            }
            Err(e) => {
                tracing::warn!("observers: host {} failed: {e}", host.id);
            }
        }
    }
    // Mark observations for hosts we polled but no longer see as ended.
    // We do this AFTER all hosts so a transient network failure on one
    // host doesn't flip its observations to ended for one cycle.
    Ok(())
}

/// Single-host scan. Returns the number of live sessions seen.
async fn scan_host(state: &AppState, host: &Host) -> Result<usize> {
    let url = format!("{}/sessions", host.base_url());
    let resp = state
        .http
        .get(&url)
        .bearer_auth(&host.api_key)
        .header("accept", "application/json")
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;

    if !resp.status().is_success() {
        anyhow::bail!("{} returned HTTP {}", url, resp.status());
    }

    let body: Vec<KatulongSession> = resp
        .json()
        .await
        .context("parse katulong /sessions response")?;

    let now = chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    let live_ids: HashSet<String> = body
        .iter()
        .map(|s| Observation::id_for(&host.id, &s.name))
        .collect();

    // Upsert each live session.
    for s in &body {
        if let Err(e) = upsert_observation(state, host, s, &now) {
            tracing::warn!(
                "observers: upsert {}--{} failed: {e}",
                host.id,
                s.name
            );
        }
    }

    // Mark observations for this host that were active but no longer
    // appear in the listing as ended. We scope by host so a flaky host
    // doesn't taint the others.
    if let Err(e) = mark_missing_as_ended(state, &host.id, &live_ids, &now) {
        tracing::warn!("observers: mark-ended for {} failed: {e}", host.id);
    }

    Ok(body.len())
}

fn upsert_observation(
    state: &AppState,
    host: &Host,
    s: &KatulongSession,
    now: &str,
) -> Result<()> {
    let id = Observation::id_for(&host.id, &s.name);
    let path = Observation::path(&state.sipag_dir, &id);

    let (mut obs, was_new) = if path.exists() {
        match Observation::load(&state.sipag_dir, &id) {
            Ok(o) => (o, false),
            Err(_) => (fresh(host, s, now, &state.sipag_dir), true),
        }
    } else {
        (fresh(host, s, now, &state.sipag_dir), true)
    };

    // Update mutable fields. We never overwrite first_seen, project,
    // kr_id, labels, or summary — those are owned by the human or the
    // categorize worker.
    obs.last_seen = now.to_string();
    obs.session_id = s.id.clone();
    obs.status = if s.alive { "active".into() } else { "ended".into() };

    // Archive katulong-side meta on the observation — once captured,
    // these fields outlive the session's presence in /sessions and let
    // the ended-row detail panel render without any host round-trip.
    if let Some(uuid) = s.meta_claude_uuid() {
        if !uuid.is_empty() {
            obs.claude_uuid = uuid.to_string();
        }
    }
    if let Some(title) = s.meta_auto_title() {
        if !title.is_empty() {
            obs.auto_title = title.to_string();
        }
    }
    if let Some(long) = s.meta_summary_long() {
        if !long.is_empty() {
            obs.summary_long = long.to_string();
        }
    }
    if let Some(cwd) = s.meta_cwd() {
        if !cwd.is_empty() {
            obs.cwd = cwd.to_string();
        }
    }

    obs.save(&state.sipag_dir)?;

    if was_new {
        tracing::info!(
            "observers: new session {}/{} (id={})",
            host.id,
            s.name,
            s.id.chars().take(8).collect::<String>()
        );
        // Announce on the per-host activity topic so the UI / future
        // categorize worker can react. Best-effort.
        let payload = serde_json::json!({
            "host": host.id,
            "session": s.name,
            "session_id": s.id,
            "alive": s.alive,
        });
        let _ = state
            .broker
            .publish("observations/activity", "session.observed", payload);
    }
    Ok(())
}

fn mark_missing_as_ended(
    state: &AppState,
    host_id: &str,
    live_ids: &HashSet<String>,
    now: &str,
) -> Result<()> {
    // Only walk this host's observations. Match by `host` field rather
    // than filename so a future filename scheme change doesn't break.
    let dir = state.sipag_dir.join("observations");
    if !dir.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(&dir)?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("toml") {
            continue;
        }
        let stem = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s,
            None => continue,
        };
        if live_ids.contains(stem) {
            continue;
        }
        let mut obs = match Observation::load(&state.sipag_dir, stem) {
            Ok(o) => o,
            Err(_) => continue,
        };
        if obs.host != host_id {
            continue;
        }
        if obs.status == "ended" {
            continue;
        }
        obs.status = "ended".into();
        obs.last_seen = now.to_string();
        if let Err(e) = obs.save(&state.sipag_dir) {
            tracing::warn!("observers: save ended {}: {e}", stem);
        }
    }
    Ok(())
}

fn fresh(host: &Host, s: &KatulongSession, now: &str, sipag_dir: &std::path::Path) -> Observation {
    let (project, kr_id) = infer_categorization(&s.name, sipag_dir);
    Observation {
        host: host.id.clone(),
        session: s.name.clone(),
        session_id: s.id.clone(),
        first_seen: now.to_string(),
        last_seen: now.to_string(),
        status: if s.alive { "active".into() } else { "ended".into() },
        project,
        kr_id,
        labels: Vec::new(),
        summary: String::new(),
        kr_refs: Vec::new(),
        claude_uuid: String::new(),
        auto_title: String::new(),
        summary_long: String::new(),
        cwd: String::new(),
    }
}

/// If a katulong session name follows sipag's dispatch convention
/// `{project}--{role}` AND that project + role exist in sipag, return
/// the matching project name and (when the task targets exactly one
/// KR) its KR id. Otherwise the session lands in misc and gemma4 will
/// propose a categorization later.
///
/// Sessions a user starts directly in katulong won't match this
/// pattern and naturally land in misc — the gemma4 proposal flow is
/// exactly for those.
fn infer_categorization(session_name: &str, sipag_dir: &std::path::Path) -> (String, u64) {
    let misc = (sipag_core::board::MISC_PROJECT.to_string(), 0u64);
    let (project_name, role) = match session_name.split_once("--") {
        Some((p, r)) if !p.is_empty() && !r.is_empty() => (p, r),
        _ => return misc,
    };
    if sipag_core::board::load_project(sipag_dir, project_name).is_err() {
        return misc;
    }
    let tasks = match sipag_core::board::list_tasks(sipag_dir, project_name, None) {
        Ok(t) => t,
        Err(_) => return (project_name.to_string(), 0),
    };
    let kr_id = tasks
        .iter()
        .find(|t| t.role == role)
        .and_then(|t| {
            if t.key_results.len() == 1 {
                Some(t.key_results[0])
            } else {
                None
            }
        })
        .unwrap_or(0);
    (project_name.to_string(), kr_id)
}

/// Subset of katulong's `/sessions` response we care about.
/// The full payload also includes tmuxSession, tmuxPane, hasChildProcesses,
/// external, icon — we intentionally ignore those for the
/// observation-level view. We *do* pluck the bits of `meta` we want to
/// archive on the Observation so ended sessions still have a label,
/// summary, and Claude UUID after katulong stops listing them.
#[derive(Debug, Deserialize)]
struct KatulongSession {
    id: String,
    name: String,
    #[serde(default)]
    alive: bool,
    #[serde(default)]
    meta: Option<KatulongMeta>,
}

#[derive(Debug, Deserialize, Default)]
struct KatulongMeta {
    #[serde(rename = "autoTitle", default)]
    auto_title: Option<String>,
    #[serde(default)]
    summary: Option<KatulongSummary>,
    #[serde(default)]
    pane: Option<KatulongPane>,
    #[serde(default)]
    claude: Option<KatulongClaude>,
}

#[derive(Debug, Deserialize, Default)]
struct KatulongSummary {
    #[serde(default)]
    long: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct KatulongPane {
    #[serde(default)]
    cwd: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct KatulongClaude {
    #[serde(default)]
    uuid: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
}

impl KatulongSession {
    fn meta_claude_uuid(&self) -> Option<&str> {
        self.meta
            .as_ref()
            .and_then(|m| m.claude.as_ref())
            .and_then(|c| c.uuid.as_deref())
    }
    fn meta_auto_title(&self) -> Option<&str> {
        self.meta.as_ref().and_then(|m| m.auto_title.as_deref())
    }
    fn meta_summary_long(&self) -> Option<&str> {
        self.meta
            .as_ref()
            .and_then(|m| m.summary.as_ref())
            .and_then(|s| s.long.as_deref())
    }
    fn meta_cwd(&self) -> Option<&str> {
        self.meta
            .as_ref()
            .and_then(|m| {
                m.pane
                    .as_ref()
                    .and_then(|p| p.cwd.as_deref())
                    .or_else(|| m.claude.as_ref().and_then(|c| c.cwd.as_deref()))
            })
    }
}
