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
            Err(_) => (fresh(host, s, now), true),
        }
    } else {
        (fresh(host, s, now), true)
    };

    // Update mutable fields. We never overwrite first_seen, project,
    // kr_id, labels, or summary — those are owned by the human or the
    // categorize worker.
    obs.last_seen = now.to_string();
    obs.session_id = s.id.clone();
    obs.status = if s.alive { "active".into() } else { "ended".into() };

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

fn fresh(host: &Host, s: &KatulongSession, now: &str) -> Observation {
    Observation {
        host: host.id.clone(),
        session: s.name.clone(),
        session_id: s.id.clone(),
        first_seen: now.to_string(),
        last_seen: now.to_string(),
        status: if s.alive { "active".into() } else { "ended".into() },
        project: sipag_core::board::MISC_PROJECT.to_string(),
        kr_id: 0,
        labels: Vec::new(),
        summary: String::new(),
    }
}

/// Subset of katulong's `/sessions` response we care about.
/// The full payload also includes tmuxSession, tmuxPane, hasChildProcesses,
/// external, icon, meta — we intentionally ignore those for the
/// observation-level view. The categorize worker can re-fetch via REST
/// when it needs deeper signal.
#[derive(Debug, Deserialize)]
struct KatulongSession {
    id: String,
    name: String,
    #[serde(default)]
    alive: bool,
}
