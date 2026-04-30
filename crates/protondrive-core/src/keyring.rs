//! Secret Service (libsecret) credential storage.
//!
//! Stores the four pieces of state needed to resume a Proton Drive
//! session without re-running SRP:
//!
//! - `uid` — the session UID returned by `/auth`.
//! - `access_token` — the current access token.
//! - `refresh_token` — the most recent refresh token.
//! - `salted_key_pass` — the salted mailbox key-pass derivative used to
//!   unlock the user's PGP keys. We **never** store the raw account
//!   password (per Proton's third-party-app policy).
//! - `totp_secret` — the Base32 TOTP key seed (so we can complete 2FA
//!   unattended on token refresh).
//!
//! Items are tagged with `application=protondrive-linux` and
//! `account=<email>` so they show up grouped by account in
//! Seahorse / KWalletManager.

use secret_service::{EncryptionType, SecretService};

use crate::{Error, Result};

const APP_TAG: &str = "protondrive-linux";

/// Names for the secret kinds we persist.
#[derive(Debug, Clone, Copy)]
pub enum Slot {
    Uid,
    AccessToken,
    RefreshToken,
    SaltedKeyPass,
    TotpSecret,
}

impl Slot {
    fn key(self) -> &'static str {
        match self {
            Slot::Uid => "uid",
            Slot::AccessToken => "access_token",
            Slot::RefreshToken => "refresh_token",
            Slot::SaltedKeyPass => "salted_key_pass",
            Slot::TotpSecret => "totp_secret",
        }
    }
}

pub struct Keyring {
    account: String,
}

impl Keyring {
    pub fn for_account(email: impl Into<String>) -> Self {
        Self {
            account: email.into(),
        }
    }

    fn attrs(&self, slot: Slot) -> std::collections::HashMap<&str, &str> {
        let mut m = std::collections::HashMap::new();
        m.insert("application", APP_TAG);
        m.insert("account", self.account.as_str());
        m.insert("slot", slot.key());
        m
    }

    pub async fn store(&self, slot: Slot, secret: &str) -> Result<()> {
        let ss = SecretService::connect(EncryptionType::Dh)
            .await
            .map_err(|e| Error::Keyring(e.to_string()))?;
        let collection = ss
            .get_default_collection()
            .await
            .map_err(|e| Error::Keyring(e.to_string()))?;
        collection
            .create_item(
                &format!("Proton Drive ({}) — {}", self.account, slot.key()),
                self.attrs(slot),
                secret.as_bytes(),
                true, // replace if exists
                "text/plain",
            )
            .await
            .map_err(|e| Error::Keyring(e.to_string()))?;
        Ok(())
    }

    pub async fn fetch(&self, slot: Slot) -> Result<Option<String>> {
        let ss = SecretService::connect(EncryptionType::Dh)
            .await
            .map_err(|e| Error::Keyring(e.to_string()))?;
        let items = ss
            .search_items(self.attrs(slot))
            .await
            .map_err(|e| Error::Keyring(e.to_string()))?;
        let Some(item) = items
            .unlocked
            .into_iter()
            .next()
            .or_else(|| items.locked.into_iter().next())
        else {
            return Ok(None);
        };
        item.unlock()
            .await
            .map_err(|e| Error::Keyring(e.to_string()))?;
        let bytes = item
            .get_secret()
            .await
            .map_err(|e| Error::Keyring(e.to_string()))?;
        Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
    }

    pub async fn delete(&self, slot: Slot) -> Result<()> {
        let ss = SecretService::connect(EncryptionType::Dh)
            .await
            .map_err(|e| Error::Keyring(e.to_string()))?;
        let items = ss
            .search_items(self.attrs(slot))
            .await
            .map_err(|e| Error::Keyring(e.to_string()))?;
        for item in items.unlocked.into_iter().chain(items.locked) {
            item.delete()
                .await
                .map_err(|e| Error::Keyring(e.to_string()))?;
        }
        Ok(())
    }
}
