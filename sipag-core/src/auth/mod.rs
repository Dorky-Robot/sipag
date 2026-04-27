//! Sipag authentication state.
//!
//! Functional-core / imperative-shell split: `AuthState` is an immutable
//! value type with pure transitions (`&self -> Self`); `AuthStore` is the
//! thin imperative boundary that serializes concurrent mutations through
//! a single mutex and persists via atomic temp+rename.
//!
//! On disk, everything lives in a single `auth.json` file under sipag's
//! data dir. The file is locked to mode 0600 on Unix at creation, which
//! `rename(2)` preserves through the atomic swap.
//!
//! Ported from the `katulong-auth` crate. Keeping this in one place
//! means a future swap of the WebAuthn implementation, or a tighter
//! security-review pass over the storage path, lands in one crate.

mod credential;
mod error;
mod random;
mod session;
mod setup_token;
mod state;
mod store;
mod webauthn;

pub use credential::Credential;
pub use error::AuthError;
pub use session::{Session, SessionTokenPlaintext, SESSION_TTL};
pub use setup_token::{PlaintextToken, SetupToken};
pub use state::AuthState;
pub use store::AuthStore;
pub use webauthn::{encode_credential_id, ChallengeId, VerifiedAuthentication, WebAuthnService};

/// Re-export the webauthn-rs wire types the binary crate needs to shape
/// its HTTP request/response bodies. Keeping these behind sipag-core's
/// facade means handlers don't grow a direct dependency on `webauthn-rs`
/// — if we ever swap the underlying library, the surface that changes is
/// this file, not every handler.
pub mod webauthn_wire {
    pub use webauthn_rs::prelude::{
        AuthenticationResult, CreationChallengeResponse, PublicKeyCredential,
        RegisterPublicKeyCredential, RequestChallengeResponse,
    };
}

pub type Result<T> = std::result::Result<T, AuthError>;

use std::path::{Path, PathBuf};

/// Default path to the auth state file inside `sipag_dir`.
pub fn auth_state_path(sipag_dir: &Path) -> PathBuf {
    sipag_dir.join("auth.json")
}
