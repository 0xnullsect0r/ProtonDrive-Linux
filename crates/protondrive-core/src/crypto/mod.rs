//! OpenPGP layer.
//!
//! Proton Drive's encryption model (mirroring `proton-drive-web`):
//!
//! ```text
//! mailbox password ──► UserPGPKeys (decrypt private keys)
//!                          │
//!                          ▼
//!                   AddressKey (per-address PGP keypair)
//!                          │
//!                          ▼
//!                   ShareKey  (PGP-encrypted to the address key)
//!                          │
//!                          ▼
//!                   NodeKey   (PGP-encrypted to parent node key, or share key for root)
//!                          │
//!                          ├──► node Name (ASCII-armored, encrypted to NodeKey)
//!                          │
//!                          ▼
//!                ContentKeyPacket (per file revision; symmetric session key
//!                                  encrypted to NodeKey)
//!                          │
//!                          ▼
//!                  Block plaintext  (symmetric AES-256, see RFC 4880 §13.9)
//! ```
//!
//! This module is **stubbed**. Implementations should use the [`pgp`] crate.
//! Tests should be added against fixtures captured from a real account
//! (with secrets scrubbed) — see `crates/protondrive-core/tests/fixtures/`.

use crate::{Error, Result};

pub struct DecryptedKeyring {
    /// Address-key fingerprint → unlocked PGP secret key (DER-serialised).
    pub address_keys: std::collections::HashMap<String, Vec<u8>>,
}

pub fn unlock_user_keys(
    _encrypted_keys: &[(String, String)], // (fingerprint, armored privkey)
    _mailbox_password: &str,
    _key_salt_b64: &str,
) -> Result<DecryptedKeyring> {
    Err(Error::NotImplemented("crypto::unlock_user_keys"))
}

pub fn decrypt_node_name(
    _ciphertext_armored: &str,
    _node_key_unlocked: &[u8],
) -> Result<String> {
    Err(Error::NotImplemented("crypto::decrypt_node_name"))
}

pub fn decrypt_block(
    _ciphertext: &[u8],
    _content_key_packet: &[u8],
    _node_key_unlocked: &[u8],
) -> Result<Vec<u8>> {
    Err(Error::NotImplemented("crypto::decrypt_block"))
}
