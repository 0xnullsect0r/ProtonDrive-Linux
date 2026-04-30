//! Secret Service (libsecret) credential storage.
//!
//! Stores three secrets per Proton account:
//! - `password`     — the Proton account password (used to derive the SRP proof
//!                    and to unlock the user's PGP private keys).
//! - `totp_secret`  — the Base32 TOTP key (the *seed*, not a 6-digit code), so
//!                    we can re-authenticate unattended.
//! - `refresh_token`— the most recent OAuth-style refresh token from `/auth`,
//!                    so most launches don't need to repeat full SRP.
//!
//! Items are tagged with `application=protondrive-linux` and `account=<email>`
//! so they show up in the Seahorse / KWalletManager UI grouped by account.

use secret_service::{EncryptionType, SecretService};

use crate::{Error, Result};

const APP_TAG: &str = "protondrive-linux";

/// Names for the three secret kinds we persist.
#[derive(Debug, Clone, Copy)]
pub enum Slot {
    Password,
    TotpSecret,
    RefreshToken,
}

impl Slot {
    fn key(self) -> &'static str {
        match self {
            Slot::Password     => "password",
            Slot::TotpSecret   => "totp_secret",
            Slot::RefreshToken => "refresh_token",
        }
    }
}

pub struct Keyring {
    account: String,
}

impl Keyring {
    pub fn for_account(email: impl Into<String>) -> Self {
        Self { account: email.into() }
    }

    fn attrs<'a>(&'a self, slot: Slot) -> std::collections::HashMap<&'a str, &'a str> {
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
        let collection = ss.get_default_collection()
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
        let Some(item) = items.unlocked.into_iter().next().or_else(|| items.locked.into_iter().next())
        else {
            return Ok(None);
        };
        item.unlock().await.map_err(|e| Error::Keyring(e.to_string()))?;
        let bytes = item.get_secret().await.map_err(|e| Error::Keyring(e.to_string()))?;
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
            item.delete().await.map_err(|e| Error::Keyring(e.to_string()))?;
        }
        Ok(())
    }
}
