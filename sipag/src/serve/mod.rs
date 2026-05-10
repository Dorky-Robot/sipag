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
mod categorize;
mod cookie;
mod devices;
mod error;
mod htmx;
mod insights;
mod login;
mod observers;
mod state;
mod tokens;
mod workers;
mod ws;

pub use state::AppState;

use anyhow::{Context, Result};
use axum::http::StatusCode;
use axum::Router;
use sipag_core::auth::{auth_state_path, AuthStore, WebAuthnService};
use sipag_core::config::default_sipag_dir;
use sipag_core::hosts::{default_hosts_path, HostsConfig};
use sipag_core::pubsub::Broker;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tower_http::services::ServeDir;
use tracing::{info, warn};

/// Parse `POST /sessions` response as `sipag_core::katulong::Session`
/// and return the session id. Returns a (status, body) pair on parse
/// failure so callers can adapt to their preferred response idiom
/// (axum tuple-into-response, htmx err_response, etc.). Shared
/// between `board.rs` and `htmx.rs` to keep wire-format knowledge in
/// one place.
pub(super) async fn extract_session_id(
    resp: reqwest::Response,
    host_id: &str,
) -> std::result::Result<String, (StatusCode, String)> {
    match resp.json::<sipag_core::katulong::Session>().await {
        Ok(s) => Ok(s.id),
        Err(e) => {
            warn!(host = %host_id, error = %e, "parse session create response failed");
            Err((
                StatusCode::BAD_GATEWAY,
                format!("create session on {host_id}: invalid response: {e}"),
            ))
        }
    }
}

/// Idempotent create-or-find by session name. POSTs `/sessions` and
/// returns the new session's id; on 409 (already exists) falls back
/// to `GET /sessions` and finds by name. Mirrors the behavior of
/// `KatulongClient::create_session` in sipag-core for the
/// reqwest/async transport that serve/ uses, so the dispatch path
/// is idempotent across both transports. Returns a ready-to-respond
/// (status, body) pair on any failure.
pub(super) async fn create_or_find_session(
    http: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    host_id: &str,
    name: &str,
) -> std::result::Result<String, (StatusCode, String)> {
    let url = sipag_core::katulong::sessions_url(base_url);
    let create_resp = match http
        .post(&url)
        .bearer_auth(api_key)
        .json(&serde_json::json!({ "name": name }))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            // `error = %e` carries the request URL via reqwest's
            // Display impl — keep it in the warn log only, never in
            // the response body (would leak the tunnel hostname).
            warn!(host = %host_id, error = %e, "POST /sessions failed");
            return Err((
                StatusCode::BAD_GATEWAY,
                format!("create session on {host_id}: network error"),
            ));
        }
    };

    match create_resp.status().as_u16() {
        200 | 201 => extract_session_id(create_resp, host_id).await,
        409 => {
            // Already exists. List and find by name — same recovery
            // path as KatulongClient::create_session.
            let list_resp = match http.get(&url).bearer_auth(api_key).send().await {
                Ok(r) => r,
                Err(e) => {
                    warn!(host = %host_id, error = %e, "GET /sessions for 409 fallback failed");
                    return Err((
                        StatusCode::BAD_GATEWAY,
                        format!("create session on {host_id}: network error"),
                    ));
                }
            };
            if !list_resp.status().is_success() {
                let st = list_resp.status();
                let body = list_resp.text().await.unwrap_or_default();
                return Err((
                    StatusCode::BAD_GATEWAY,
                    format!(
                        "create session on {host_id}: 409 fallback list failed: HTTP {st}: {body}"
                    ),
                ));
            }
            let sessions: Vec<sipag_core::katulong::Session> = match list_resp.json().await {
                Ok(s) => s,
                Err(e) => {
                    warn!(host = %host_id, error = %e, "parse /sessions list failed");
                    return Err((
                        StatusCode::BAD_GATEWAY,
                        format!("create session on {host_id}: invalid list response"),
                    ));
                }
            };
            sessions
                .into_iter()
                .find(|s| s.name == name)
                .map(|s| s.id)
                .ok_or_else(|| {
                    (
                        StatusCode::BAD_GATEWAY,
                        format!(
                            "session '{name}' on {host_id} returned 409 but list lookup missed it"
                        ),
                    )
                })
        }
        code => {
            let body = create_resp.text().await.unwrap_or_default();
            Err((
                StatusCode::BAD_GATEWAY,
                format!("create session on {host_id}: HTTP {code}: {body}"),
            ))
        }
    }
}

/// CLI entry — called from the `Serve` branch.
pub fn run(port: u16, web_root: PathBuf, workers_enabled: bool) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime")?;
    runtime.block_on(async_run(port, web_root, workers_enabled))
}

async fn async_run(port: u16, web_root: PathBuf, workers_enabled: bool) -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
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

    // Cloudflare's Browser Integrity Check 403s requests whose UA looks
    // like a non-browser library (e.g. reqwest's default
    // `reqwest/x.y.z`). When `OLLAMA_HOST` points at a tunnel-fronted
    // bridge, those checks fire — keep "Mozilla" in the UA so we get
    // through while still identifying ourselves as sipag.
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(600))
        .user_agent(concat!("Mozilla/5.0 sipag/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("failed to build reqwest client")?;

    let public_url =
        std::env::var("SIPAG_PUBLIC_URL").unwrap_or_else(|_| format!("http://localhost:{port}"));

    let sipag_dir = default_sipag_dir();
    let state = build_state(sipag_dir, public_url, hosts, http, workers_enabled).await?;

    if workers_enabled {
        info!("workers enabled — scheduler will dispatch label-driven workers");
        workers::spawn_scheduler(state.clone());
        // Observers track katulong sessions across all configured hosts
        // and surface them as Observations under `misc`. Same gate as
        // the worker scheduler so a `serve` without `--workers` is a
        // pure read-only board.
        observers::spawn(state.clone());
    } else {
        info!("workers disabled — pass --workers to enable autonomous dispatch");
    }

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
    workers_enabled: bool,
) -> Result<AppState> {
    let cookie_secure = public_url.starts_with("https://");
    let webauthn = build_webauthn(&public_url)?;
    let auth_store = AuthStore::open(auth_state_path(&sipag_dir))
        .await
        .context("AuthStore open failed")?;
    let broker = Broker::open(&sipag_dir).context("Broker::open failed")?;

    Ok(AppState {
        hosts: Arc::new(hosts),
        http,
        sipag_dir,
        public_url,
        cookie_secure,
        auth_store: Arc::new(auth_store),
        webauthn: Arc::new(webauthn),
        broker,
        workers_enabled,
        kr_proposals: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
    })
}

fn build_webauthn(public_url: &str) -> Result<WebAuthnService> {
    let url =
        url::Url::parse(public_url).with_context(|| format!("invalid public_url: {public_url}"))?;
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
    build_state(sipag_dir, public_url, HostsConfig::default(), http, false).await
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
        .merge(ws::routes())
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
    let with_ci = with_ci.with_state(state);
    // Dev-loop accelerator. SIPAG_DEV=1 mounts tower-livereload, which
    // injects a tiny browser-side script that polls /livereload. The
    // layer's version flips on every server restart (triggers iPad
    // refresh after `cargo watch` rebuilds) AND when the static-file
    // watcher below sees a change (triggers iPad refresh on CSS/JS
    // edits without restarting sipag). Production never sees this —
    // the env var is only set by `bin/sipag-dev`.
    if std::env::var("SIPAG_DEV").as_deref() == Ok("1") {
        let layer = tower_livereload::LiveReloadLayer::new();
        let reloader = layer.reloader();
        spawn_static_watcher(web_root, reloader);
        with_ci.layer(layer)
    } else {
        with_ci
    }
}

/// Watch `web_root` recursively for changes and ping the livereload
/// reloader. Debounces rapid bursts (e.g., editor saves that hit
/// multiple files in quick succession) so we send at most one reload
/// per ~150ms of activity. Runs on a dedicated OS thread because
/// `notify`'s recommended_watcher uses a blocking std::sync::mpsc
/// callback channel.
fn spawn_static_watcher(web_root: PathBuf, reloader: tower_livereload::Reloader) {
    use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
    use std::sync::mpsc::channel;
    use std::time::Duration;

    std::thread::Builder::new()
        .name("sipag-livereload".into())
        .spawn(move || {
            let (tx, rx) = channel();
            let mut watcher: RecommendedWatcher = match notify::recommended_watcher(move |res| {
                let _ = tx.send(res);
            }) {
                Ok(w) => w,
                Err(e) => {
                    tracing::warn!("livereload watcher init: {e}");
                    return;
                }
            };
            if let Err(e) = watcher.watch(&web_root, RecursiveMode::Recursive) {
                tracing::warn!("livereload watch '{}' failed: {e}", web_root.display());
                return;
            }
            tracing::info!(
                "livereload: watching {} for static changes",
                web_root.display()
            );
            // The watcher must outlive this thread; pinning it in a
            // local var keeps it alive for the loop below.
            let _watcher_keepalive = watcher;
            loop {
                let evt = match rx.recv() {
                    Ok(Ok(e)) => e,
                    Ok(Err(e)) => {
                        tracing::warn!("livereload watcher error: {e}");
                        continue;
                    }
                    Err(_) => break, // sender dropped — server shutting down
                };
                if !matches!(
                    evt.kind,
                    EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
                ) {
                    continue;
                }
                // Debounce: drain any siblings that arrive within a
                // short window (editors often touch a temp file then
                // rename, surfacing as 2-3 events per save).
                std::thread::sleep(Duration::from_millis(150));
                while rx.try_recv().is_ok() {}
                tracing::info!("livereload: static change → triggering reload");
                reloader.reload();
            }
        })
        .expect("spawn livereload watcher thread");
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
