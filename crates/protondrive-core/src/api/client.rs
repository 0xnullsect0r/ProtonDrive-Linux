//! Thin HTTP wrapper around `reqwest`. Handles the Proton-specific headers
//! (`x-pm-uid`, `x-pm-appversion`, bearer token) and the `Code != 1000` envelope.
//!
//! Most methods are stubs — fill them in alongside [`crate::api::model`] as you
//! port endpoints from the reference clients.

use parking_lot::RwLock;
use reqwest::{header, Client, Method, RequestBuilder, Url};
use std::sync::Arc;

use crate::auth::Session;
use crate::{Error, Result};

const DEFAULT_BASE: &str = "https://drive-api.proton.me";
const APP_VERSION:  &str = "linux-drive@0.1.0";
const USER_AGENT:   &str = "ProtonDrive-Linux/0.1.0";

#[derive(Clone)]
pub struct ApiClient {
    inner:   Client,
    base:    Url,
    session: Arc<RwLock<Option<Session>>>,
}

impl ApiClient {
    pub fn new() -> Result<Self> {
        let inner = Client::builder()
            .user_agent(USER_AGENT)
            .https_only(true)
            .build()?;
        Ok(Self {
            inner,
            base: Url::parse(DEFAULT_BASE).expect("valid base url"),
            session: Arc::new(RwLock::new(None)),
        })
    }

    pub fn set_session(&self, s: Session) { *self.session.write() = Some(s); }
    pub fn clear_session(&self)            { *self.session.write() = None; }
    pub fn session(&self) -> Option<Session> { self.session.read().clone() }

    /// Build a request with Proton's standard headers + bearer auth (if logged in).
    fn request(&self, method: Method, path: &str) -> Result<RequestBuilder> {
        let url = self.base.join(path).map_err(|e| Error::Other(e.to_string()))?;
        let mut req = self.inner.request(method, url)
            .header("x-pm-appversion", APP_VERSION)
            .header(header::ACCEPT, "application/vnd.protonmail.v1+json");
        if let Some(s) = self.session.read().as_ref() {
            req = req.header("x-pm-uid", &s.uid)
                     .bearer_auth(&s.access_token);
        }
        Ok(req)
    }

    // --- High-level operations (stubs) -----------------------------------

    /// Begin SRP login. Returns the info needed to compute the client proof.
    pub async fn auth_info(&self, _email: &str) -> Result<crate::auth::srp::AuthInfo> {
        Err(Error::NotImplemented("ApiClient::auth_info"))
    }

    /// Complete SRP. Returns a (possibly 2FA-pending) session.
    pub async fn auth(
        &self,
        _email: &str,
        _proof: &crate::auth::srp::ClientProof,
        _srp_session: &str,
    ) -> Result<Session> {
        Err(Error::NotImplemented("ApiClient::auth"))
    }

    /// Submit a TOTP code to a 2FA-pending session.
    pub async fn auth_2fa(&self, _totp_code: &str) -> Result<()> {
        Err(Error::NotImplemented("ApiClient::auth_2fa"))
    }

    pub async fn refresh(&self) -> Result<()> {
        Err(Error::NotImplemented("ApiClient::refresh"))
    }

    /// Pull the events feed since `cursor`.
    pub async fn events(&self, _cursor: &str) -> Result<super::model::EventsResp> {
        Err(Error::NotImplemented("ApiClient::events"))
    }

    /// Download a single encrypted block by its signed CDN URL.
    pub async fn download_block(&self, _url: &str) -> Result<bytes::Bytes> {
        Err(Error::NotImplemented("ApiClient::download_block"))
    }

    /// `req()` is exposed for the events module and any future caller.
    pub fn raw(&self) -> &Client { &self.inner }
    pub fn base_url(&self) -> &Url { &self.base }
    /// Used by tests to point at a mock server.
    pub fn with_base_url(mut self, base: Url) -> Self { self.base = base; self }
    /// Build helper for sub-modules.
    pub(crate) fn req(&self, method: Method, path: &str) -> Result<RequestBuilder> {
        self.request(method, path)
    }
}
