//! Top-level orchestrator. The UI and CLI talk to this.
//!
//! Owns: the [`protondrive_bridge::Bridge`] handle (Proton API + crypto),
//! the [`Config`], the keyring, and the in-process sync state. The
//! actual sync engine lives in `protondrive-sync`; we instantiate it
//! when [`Daemon::start_sync`] is called after a successful login.

use parking_lot::Mutex;
use protondrive_bridge::{Bridge, InitArgs, LoginArgs, LoginOutcome};
use std::sync::Arc;

use crate::config::{Config, Paths};
use crate::keyring::{Keyring, Slot};
use crate::Result;

// IMPORTANT: Proton's API gates the `x-pm-appversion` header to a known set
// of client identifiers and returns HTTP 400 / Code 2064 ("Invalid Section
// Name") for anything it doesn't recognise. We impersonate the official
// web-drive client (this is also what rclone and Proton-API-Bridge use by
// default for third-party clients).
const APP_VERSION: &str = "web-drive@5.0.30.0";

#[derive(Clone)]
pub struct Daemon {
    pub config: Arc<Mutex<Config>>,
    pub paths: Arc<Paths>,
    pub bridge: Arc<Mutex<Option<Bridge>>>,
}

impl Daemon {
    pub fn init() -> Result<Self> {
        let paths = Paths::discover()?;
        paths.ensure()?;
        let config = Config::load_or_default(&paths.config_file())?;
        Ok(Self {
            config: Arc::new(Mutex::new(config)),
            paths: Arc::new(paths),
            bridge: Arc::new(Mutex::new(None)),
        })
    }

    pub fn save_config(&self) -> Result<()> {
        let cfg = self.config.lock().clone();
        cfg.save(&self.paths.config_file())
    }

    pub async fn ensure_bridge(
        &self,
    ) -> std::result::Result<Bridge, protondrive_bridge::BridgeError> {
        if let Some(b) = self.bridge.lock().clone() {
            return Ok(b);
        }
        let b = Bridge::init(InitArgs {
            app_version: APP_VERSION.into(),
            user_agent: format!("ProtonDrive-Linux/{}", env!("CARGO_PKG_VERSION")),
            enable_caching: true,
            concurrent_blocks: 5,
            concurrent_crypto: 3,
            replace_existing: true,
            ..Default::default()
        })
        .await?;
        *self.bridge.lock() = Some(b.clone());
        Ok(b)
    }

    /// Try to resume a session from the keyring. Returns `Ok(true)` on success.
    pub async fn try_resume(&self) -> Result<bool> {
        let email = match self.config.lock().email.clone() {
            Some(e) => e,
            None => return Ok(false),
        };
        let kr = Keyring::for_account(email);
        let (uid, at, rt, skp) = tokio::join!(
            kr.fetch(Slot::Uid),
            kr.fetch(Slot::AccessToken),
            kr.fetch(Slot::RefreshToken),
            kr.fetch(Slot::SaltedKeyPass),
        );
        let (uid, at, rt, skp) = match (uid?, at?, rt?, skp?) {
            (Some(a), Some(b), Some(c), Some(d)) => (a, b, c, d),
            _ => return Ok(false),
        };
        let bridge = self
            .ensure_bridge()
            .await
            .map_err(|e| crate::Error::Other(e.to_string()))?;
        let cred = bridge
            .resume(protondrive_bridge::Credential {
                uid,
                access_token: at,
                refresh_token: rt,
                salted_key_pass: skp,
            })
            .await
            .map_err(|e| crate::Error::Auth(e.to_string()))?;
        self.persist_credential(&cred).await?;
        Ok(true)
    }

    pub async fn login(
        &self,
        email: &str,
        password: &str,
        mailbox_password: Option<&str>,
        totp_code: Option<&str>,
    ) -> Result<LoginOutcome> {
        self.config.lock().email = Some(email.into());
        self.save_config()?;
        let bridge = self
            .ensure_bridge()
            .await
            .map_err(|e| crate::Error::Other(e.to_string()))?;
        let outcome = bridge
            .login(LoginArgs {
                username: email.into(),
                password: password.into(),
                mailbox_password: mailbox_password.unwrap_or_default().into(),
                two_fa: totp_code.unwrap_or_default().into(),
            })
            .await
            .map_err(|e| crate::Error::Auth(e.to_string()))?;
        if let LoginOutcome::Success(ref cred) = outcome {
            self.persist_credential(cred).await?;
        }
        Ok(outcome)
    }

    /// Complete a login that was blocked by Human Verification (code 9001).
    /// `hv_type` is `"captcha"`, `"email"`, or `"sms"`.
    /// `hv_token` is the solution token from verify.proton.me.
    /// `fresh_two_fa` is a freshly generated TOTP code (optional).
    pub async fn login_hv(
        &self,
        hv_type: &str,
        hv_token: &str,
        fresh_two_fa: Option<&str>,
    ) -> Result<()> {
        let bridge = self
            .ensure_bridge()
            .await
            .map_err(|e| crate::Error::Other(e.to_string()))?;
        let cred = bridge
            .login_hv(hv_type, hv_token, fresh_two_fa)
            .await
            .map_err(|e| crate::Error::Auth(e.to_string()))?;
        self.persist_credential(&cred).await?;
        Ok(())
    }

    async fn persist_credential(&self, cred: &protondrive_bridge::Credential) -> Result<()> {
        let email = match self.config.lock().email.clone() {
            Some(e) => e,
            None => return Ok(()),
        };
        let kr = Keyring::for_account(email);
        kr.store(Slot::Uid, &cred.uid).await?;
        kr.store(Slot::AccessToken, &cred.access_token).await?;
        kr.store(Slot::RefreshToken, &cred.refresh_token).await?;
        kr.store(Slot::SaltedKeyPass, &cred.salted_key_pass).await?;
        Ok(())
    }
}
