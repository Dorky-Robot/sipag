//! Auth state for sipag's standalone subdomain.
//!
//! Layout under `<sipag_dir>/`:
//!
//! ```text
//! ~/.sipag/
//! ├── user.json                   single-user record (id, display_name)
//! ├── credentials/                per-passkey metadata (one JSON file each)
//! │   └── <cred-id>.json
//! ├── sessions/                   active session cookies (one TOML each)
//! │   └── <session-token>.toml
//! └── setup-tokens/               pending bootstrap tokens (one TOML each)
//!     └── <token>.toml
//! ```
//!
//! sipag-core deliberately does not depend on `webauthn-rs`. The
//! binary crate owns the WebAuthn lifecycle; sipag-core just persists
//! the metadata side (user record, opaque-blob credentials, sessions,
//! setup tokens) as plain files. This keeps sipag-core dependency-light
//! and lets us swap WebAuthn implementations later without touching
//! disk format.

mod credential;
mod session;
mod setup_token;
mod user;

pub use credential::Credential;
pub use session::Session;
pub use setup_token::{SetupPurpose, SetupToken};
pub use user::User;

use std::path::{Path, PathBuf};

use crate::config::default_sipag_dir;

pub fn credentials_dir(sipag_dir: &Path) -> PathBuf {
    sipag_dir.join("credentials")
}

pub fn sessions_dir(sipag_dir: &Path) -> PathBuf {
    sipag_dir.join("sessions")
}

pub fn setup_tokens_dir(sipag_dir: &Path) -> PathBuf {
    sipag_dir.join("setup-tokens")
}

/// Generate a hex-encoded random token of the given raw byte length.
/// Used for setup tokens, session tokens, and similar one-shot secrets.
/// 32 bytes of randomness → 64 hex chars; sufficient for any token in
/// this codebase.
pub fn random_token(bytes: usize) -> String {
    use rand::RngCore;
    let mut buf = vec![0u8; bytes];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    let mut hex = String::with_capacity(bytes * 2);
    for b in buf {
        hex.push_str(&format!("{:02x}", b));
    }
    hex
}

/// Convenience: resolves to the default sipag dir. Modules call this
/// so consumers can swap in a custom dir for tests via `SIPAG_DIR`.
pub fn auth_dir() -> PathBuf {
    default_sipag_dir()
}
