use serde::{Deserialize, Serialize};
use std::time::SystemTime;

/// A registered WebAuthn credential (passkey).
///
/// Every credential in sipag is a platform-internal passkey — new devices
/// are paired via setup token, not via WebAuthn's cross-device (QR/hybrid)
/// transport. So there's no per-credential `transports` field:
/// `allowCredentials` emits the constant `["internal"]` when generating
/// auth options.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Credential {
    pub id: String,
    pub public_key: Vec<u8>,
    pub name: Option<String>,
    pub counter: u32,
    #[serde(with = "crate::state::systime")]
    pub created_at: SystemTime,
    /// Link back to the setup token that paired this device, if any. Lets
    /// the UI show tokens as "unused" vs "paired device <name>" and cascade
    /// a token revocation into removing the device it created.
    #[serde(default)]
    pub setup_token_id: Option<String>,
}
