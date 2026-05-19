use crate::serve::categorize::ProposalState;
use sipag_core::auth::{AuthStore, WebAuthnService};
use sipag_core::hosts::HostsConfig;
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
}
