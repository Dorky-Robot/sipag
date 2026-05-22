use crate::bridge::BridgeWiring;
use crate::serve::categorize::ProposalState;
use katulong_client::KatulongAsyncClient;
use sipag_core::auth::{AuthStore, WebAuthnService};
use sipag_mesh::{Host, HostsConfig};
use sipag_pubsub::Broker;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Shared application state passed to every handler.
#[derive(Clone)]
pub struct AppState {
    pub hosts: Arc<HostsConfig>,
    pub http: reqwest::Client,
    /// Sipag data dir — `~/.sipag` by default, overridable for tests via
    /// `SIPAG_DIR`.
    pub sipag_dir: PathBuf,
    /// External base URL for minting links the user opens in a browser.
    /// `SIPAG_PUBLIC_URL` env var > `http://localhost:<port>` default.
    pub public_url: String,
    /// `Set-Cookie; Secure` flag — true when `public_url` is `https://`.
    pub cookie_secure: bool,
    /// File-backed auth state. Holds credentials, sessions, setup tokens,
    /// and the WebAuthn user handle, all in one atomic JSON file.
    pub auth_store: Arc<AuthStore>,
    /// In-memory WebAuthn ceremony coordinator. Pending registration /
    /// authentication state lives here.
    pub webauthn: Arc<WebAuthnService>,
    /// File-backed pub/sub broker. Workers publish progress; the WS
    /// endpoint forwards live envelopes to the browser.
    pub broker: Broker,
    /// Whether autonomous workers are running. The CLI flag
    /// `serve --workers` flips this on.
    pub workers_enabled: bool,
    /// In-memory cache of gemma4 KR proposals for misc observations.
    /// Keyed by Observation id (`<host>--<session>`). Re-fetched at
    /// render time when the underlying summary hash changes; never
    /// persisted (proposals are advisory render-time hints).
    pub kr_proposals: Arc<RwLock<HashMap<String, ProposalState>>>,
    /// Shared ollama-bridge wiring. `Some` when
    /// `~/.ollama-bridge/remote.json` was loadable at startup;
    /// `None` when the bridge isn't configured. Consumed by the
    /// dispatch gate (`dispatch_gate::classify` via `wiring.chat`)
    /// and by the lens scheduler (`serve/lens_scheduler.rs` via
    /// `wiring.chat + wiring.embedder`). When None, the gate path
    /// fails closed at dispatch time with a clear "configure the
    /// bridge" error — same shape as today's gemma-unreachable
    /// failure mode, just earlier in the call chain.
    ///
    /// `BridgeWiring: Clone` (the gate fold added `Clone` to its
    /// underlying types), so AppState's per-request `.clone()` stays
    /// cheap.
    pub bridge: Option<BridgeWiring>,
}

impl AppState {
    /// Materialize a [`KatulongAsyncClient`] bound to `host`, sharing
    /// the AppState's reqwest::Client (so connection pool + UA +
    /// timeout configuration are uniform across all outbound HTTP).
    ///
    /// Cheap to call per request — `reqwest::Client` is Arc-internal,
    /// so `clone()` is a refcount bump.
    pub fn katulong_for(&self, host: &Host) -> KatulongAsyncClient {
        KatulongAsyncClient::with_client(
            self.http.clone(),
            host.base_url().to_string(),
            host.api_key.clone(),
        )
    }
}
