use sipag_core::auth::{AuthStore, WebAuthnService};
use sipag_core::hosts::HostsConfig;
use std::path::PathBuf;
use std::sync::Arc;

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
}
