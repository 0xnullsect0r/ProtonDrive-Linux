//! Authentication against the Proton API.
//!
//! Two pieces:
//!
//! - [`srp`] — Proton's SRP variant. **Stub.** Port from
//!   <https://github.com/ProtonDriveApps/proton-go-api> (`pkg/srp`) or the
//!   JS reference at <https://github.com/ProtonMail/WebClients> (`packages/srp`).
//!   Proton uses a custom SRP-6a flavour: SHA-256, modulus from the server,
//!   and the user password is pre-hashed with a custom salted PHC scheme
//!   ("auth version 4" today) before being fed into SRP.
//!
//! - [`totp`] — RFC 6238 TOTP. We accept the Base32 *secret key* the user
//!   pasted in setup, generate the current 6-digit code on demand, and
//!   POST it to `/auth/2fa`.

pub mod srp;
pub mod totp;

use serde::{Deserialize, Serialize};

/// Tokens returned by a successful `/auth` exchange.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub uid: String,
    pub access_token: String,
    pub refresh_token: String,
    pub scopes: Vec<String>,
    /// The user's mailbox-password-derived key passphrase, used by the crypto
    /// layer to unlock the OpenPGP private keys. Held in memory only.
    #[serde(skip)]
    pub key_passphrase: Option<Vec<u8>>,
}

impl Session {
    pub fn needs_2fa(&self) -> bool {
        self.scopes.iter().any(|s| s == "2fa")
    }
}
