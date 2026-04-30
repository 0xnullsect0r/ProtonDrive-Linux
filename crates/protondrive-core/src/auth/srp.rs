//! Proton's SRP-6a variant — **stub**.
//!
//! TODO(protocol): port from `proton-go-api/pkg/srp`. Key facts from the
//! reference clients:
//!
//! - Modulus `N` (2048-bit) is downloaded fresh from `/auth/info` each login,
//!   wrapped in a server-signed envelope; we must verify that signature against
//!   Proton's hard-coded modulus signing key before using it.
//! - Hash is `SHA-256` (reduced 4× to a 256-bit `EXPANDED_HASH`, see
//!   `expand_hash` in the JS impl).
//! - Generator `g = 2`.
//! - The "password hash" fed into SRP is **not** the raw password — it's
//!   `bcrypt($password, $salt)` where the salt is derived from the
//!   `Salt` field of `/auth/info` (auth version ≥ 4).
//!
//! Once ported, [`compute_proof`] returns the `ClientEphemeral` and
//! `ClientProof` fields the `/auth` endpoint expects.

use crate::{Error, Result};

#[derive(Debug, Clone)]
pub struct AuthInfo {
    pub modulus_b64: String,
    pub server_ephemeral_b64: String,
    pub salt_b64: String,
    pub version: u8,
}

#[derive(Debug, Clone)]
pub struct ClientProof {
    pub client_ephemeral_b64: String,
    pub client_proof_b64: String,
    /// Expected server proof — verify after `/auth` responds.
    pub expected_server_proof_b64: String,
    /// Bytes of the shared session key K (used as PGP key passphrase salt).
    pub shared_session_key: Vec<u8>,
}

pub fn compute_proof(_password: &str, _info: &AuthInfo) -> Result<ClientProof> {
    Err(Error::NotImplemented(
        "Proton SRP — port from proton-go-api/pkg/srp",
    ))
}
