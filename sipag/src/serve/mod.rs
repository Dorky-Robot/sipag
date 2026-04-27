//! `sipag serve` — the agent-manager backplane.
//!
//! HTTP entrypoint. Auth uses a single `auth.json` state file and the
//! katulong-shaped first-device-localhost / pair-by-setup-token flow.
//! See `auth_middleware`, `auth`, `tokens`, `devices`, `login` for the
//! pieces.

mod access;
mod auth;
mod auth_middleware;
mod board;
mod board_view;
mod cookie;
mod devices;
mod error;
mod htmx;
mod insights;
mod login;
mod state;
mod tokens;

pub use state::AppState;

use anyhow::{Context, Result};
use axum::Router;
use sipag_core::auth::{auth_state_path, AuthStore, WebAuthnService};
use sipag_core::config::default_sipag_dir;
use sipag_core::hosts::{default_hosts_path, HostsConfig};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tower_http::services::ServeDir;
use tracing::{info, warn};

/// CLI entry — called from the `Serve` branch.
pub fn run(port: u16, web_root: PathBuf) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime")?;
    runtime.block_on(async_run(port, web_root))
}

async fn async_run(port: u16, web_root: PathBuf) -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| {
                    tracing_subscriber::EnvFilter::new("sipag=info,tower_http=info")
                }),
        )
        .try_init();

    let hosts_path = default_hosts_path();
    let hosts = HostsConfig::load()
        .with_context(|| format!("failed to load hosts from {}", hosts_path.display()))?;

    if hosts.hosts.is_empty() {
        warn!(
            "no hosts configured — create {} (see extras/hosts.toml.example)",
            hosts_path.display()
        );
    } else {
        info!(
            "loaded {} host(s): {:?}",
            hosts.hosts.len(),
            hosts.hosts.iter().map(|h| &h.id).collect::<Vec<_>>()
        );
    }

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("failed to build reqwest client")?;

    let public_url =
        std::env::var("SIPAG_PUBLIC_URL").unwrap_or_else(|_| format!("http://localhost:{port}"));

    let sipag_dir = default_sipag_dir();
    let state = build_state(sipag_dir, public_url, hosts, http).await?;

    let app = build_router(state, web_root.clone());

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    info!(
        "sipag serve listening on http://{} (web root: {})",
        addr,
        web_root.display()
    );

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind {}", addr))?;
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .context("server error")?;
    Ok(())
}

async fn build_state(
    sipag_dir: PathBuf,
    public_url: String,
    hosts: HostsConfig,
    http: reqwest::Client,
) -> Result<AppState> {
    let cookie_secure = public_url.starts_with("https://");
    let webauthn = build_webauthn(&public_url)?;
    let auth_store = AuthStore::open(auth_state_path(&sipag_dir))
        .await
        .context("AuthStore open failed")?;

    Ok(AppState {
        hosts: Arc::new(hosts),
        http,
        sipag_dir,
        public_url,
        cookie_secure,
        auth_store: Arc::new(auth_store),
        webauthn: Arc::new(webauthn),
    })
}

fn build_webauthn(public_url: &str) -> Result<WebAuthnService> {
    let url = url::Url::parse(public_url)
        .with_context(|| format!("invalid public_url: {public_url}"))?;
    let rp_id = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("public_url has no host: {public_url}"))?
        .to_string();
    WebAuthnService::new(&rp_id, "Sipag", public_url)
        .map_err(|e| anyhow::anyhow!("WebAuthnService::new: {e}"))
}

/// Construct an `AppState` and Router pointing at a tempdir, for tests.
pub async fn build_test_state(sipag_dir: PathBuf, public_url: String) -> Result<AppState> {
    let http = reqwest::Client::new();
    build_state(sipag_dir, public_url, HostsConfig::default(), http).await
}

pub fn build_router(state: AppState, web_root: PathBuf) -> Router {
    build_router_inner(state, web_root, false)
}

/// Test-only router where the fallback peer (used when ConnectInfo
/// isn't wired) is loopback. axum-test doesn't set up
/// `into_make_service_with_connect_info`, so without this tests that
/// expect localhost-bypass would always classify as remote.
pub fn build_test_router(state: AppState, web_root: PathBuf) -> Router {
    build_router_inner(state, web_root, true)
}

fn build_router_inner(state: AppState, web_root: PathBuf, test_loopback_peer: bool) -> Router {
    let base = Router::new()
        .merge(auth::routes())
        .merge(tokens::routes())
        .merge(devices::routes())
        .merge(login::routes())
        .merge(board::routes())
        .merge(insights::routes())
        .merge(htmx::routes())
        .fallback_service(ServeDir::new(&web_root).append_index_html_on_directories(true))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth_middleware::gate,
        ));
    // The connect-info filler must run BEFORE `gate` (and the
    // handlers). axum applies layers in reverse order, so declaring
    // it *after* gate puts it on the outside — which is what we want.
    let with_ci = if test_loopback_peer {
        base.layer(axum::middleware::from_fn(
            auth_middleware::ensure_loopback_connect_info,
        ))
    } else {
        base.layer(axum::middleware::from_fn(
            auth_middleware::ensure_connect_info,
        ))
    };
    with_ci.with_state(state)
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
