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
        .route("/api/hosts/:id/crew/list", get(proxy_crew_list))
        .route("/api/hosts/:id/crew/status", get(proxy_crew_status))
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

async fn proxy_crew_list(
    AxumPath(id): AxumPath<String>,
    State(state): State<AppState>,
) -> Response {
    proxy_get(&state, &id, "/crew/list").await
}

async fn proxy_crew_status(
    AxumPath(id): AxumPath<String>,
    State(state): State<AppState>,
) -> Response {
    proxy_get(&state, &id, "/crew/status").await
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
