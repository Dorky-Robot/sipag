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
    routing::get,
    Router,
};
use serde::Serialize;
use sipag_core::board::{list_project_names, list_tasks, load_project};
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
        // Board (objectives + tasks) — the primary surface. Mesh above
        // is background context.
        .route("/api/projects", get(list_projects))
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
    created: String,
    updated: String,
}

#[derive(Serialize)]
struct ProjectView {
    name: String,
    repo: String,
    statuses: Vec<String>,
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
        let tasks_view = tasks
            .into_iter()
            .map(|t| TaskView {
                id: t.id,
                title: t.title,
                status: t.status.to_string(),
                role: t.role,
                labels: t.labels,
                created: t.created,
                updated: t.updated,
            })
            .collect();
        out.push(ProjectView {
            name: project.name,
            repo: project.repo,
            statuses: project.statuses,
            tasks: tasks_view,
        });
    }
    Json(out).into_response()
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
