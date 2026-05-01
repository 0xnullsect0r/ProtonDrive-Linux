//! Safe Rust wrapper around the Go-side `libprotonbridge.so`.
//!
//! All exported C functions accept and return JSON-encoded
//! null-terminated strings. The bridge owns the underlying Proton
//! Drive session; this Rust crate provides a high-level [`Bridge`]
//! handle that wraps every call in [`tokio::task::spawn_blocking`]
//! so that long-running Go calls (which pin the calling OS thread)
//! do not stall the async runtime.

#![allow(non_camel_case_types)]

use serde::{Deserialize, Serialize};
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_longlong};
use thiserror::Error;

mod ffi {
    use super::*;
    extern "C" {
        pub fn pd_free(p: *mut c_char);
        pub fn pd_version() -> *mut c_char;
        pub fn pd_init(args: *const c_char) -> *mut c_char;
        pub fn pd_login(session: c_longlong, args: *const c_char) -> *mut c_char;
        pub fn pd_login_hv(session: c_longlong, args: *const c_char) -> *mut c_char;
        pub fn pd_resume(session: c_longlong, args: *const c_char) -> *mut c_char;
        pub fn pd_logout(session: c_longlong) -> *mut c_char;
        pub fn pd_root_link_id(session: c_longlong) -> *mut c_char;
        pub fn pd_list(session: c_longlong, folder_id: *const c_char) -> *mut c_char;
        pub fn pd_get_link(session: c_longlong, link_id: *const c_char) -> *mut c_char;
        pub fn pd_create_folder(
            session: c_longlong,
            parent_id: *const c_char,
            name: *const c_char,
        ) -> *mut c_char;
        pub fn pd_upload(
            session: c_longlong,
            parent_id: *const c_char,
            name: *const c_char,
            src_path: *const c_char,
        ) -> *mut c_char;
        pub fn pd_download(
            session: c_longlong,
            link_id: *const c_char,
            dst_path: *const c_char,
        ) -> *mut c_char;
        pub fn pd_move(
            session: c_longlong,
            src_id: *const c_char,
            dst_parent_id: *const c_char,
            dst_name: *const c_char,
        ) -> *mut c_char;
        pub fn pd_trash(session: c_longlong, link_id: *const c_char) -> *mut c_char;
        pub fn pd_about(session: c_longlong) -> *mut c_char;
        pub fn pd_search(
            session: c_longlong,
            folder_id: *const c_char,
            name: *const c_char,
        ) -> *mut c_char;
        pub fn pd_events(session: c_longlong, since: c_longlong) -> *mut c_char;
        #[allow(dead_code)]
        pub fn pd_set_log_level(level: *const c_char) -> *mut c_char;
    }
}

#[derive(Debug, Error)]
pub enum BridgeError {
    #[error("bridge: {0}")]
    Bridge(String),
    #[error("bridge: invalid string: {0}")]
    Nul(#[from] std::ffi::NulError),
    #[error("bridge: utf-8: {0}")]
    Utf8(#[from] std::str::Utf8Error),
    #[error("bridge: json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("bridge: tokio join: {0}")]
    Join(#[from] tokio::task::JoinError),
}

#[derive(Debug, Default, Serialize)]
pub struct InitArgs {
    pub app_version: String,
    pub user_agent: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub data_folder_name: String,
    pub enable_caching: bool,
    pub concurrent_blocks: i32,
    pub concurrent_crypto: i32,
    pub replace_existing: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub credential_cache_file: String,
}

#[derive(Debug, Serialize)]
pub struct LoginArgs {
    pub username: String,
    pub password: String,
    pub mailbox_password: String,
    pub two_fa: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credential {
    pub uid: String,
    pub access_token: String,
    pub refresh_token: String,
    pub salted_key_pass: String,
}

/// The outcome of a first-time login attempt.
#[derive(Debug, Clone)]
pub enum LoginOutcome {
    /// Authentication succeeded; credentials are returned.
    Success(Credential),
    /// Proton requires the user to complete a Human Verification challenge.
    /// Open `https://verify.proton.me/?methods=<methods>&token=<hv_token>&theme=dark`
    /// in a browser, capture the resulting token, then call `Bridge::login_hv`.
    HvRequired {
        hv_token: String,
        methods: Vec<String>,
    },
}

#[derive(Debug, Clone, Deserialize)]
pub struct Link {
    pub link_id: String,
    pub parent_link_id: String,
    pub name: String,
    pub is_folder: bool,
    pub mime_type: String,
    pub size: i64,
    pub modify_time: i64,
    pub create_time: i64,
    pub state: i32,
    pub hash: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UploadResult {
    pub link_id: String,
    pub size: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AboutInfo {
    pub id: String,
    pub name: String,
    pub email: String,
    pub display: String,
    pub used: i64,
    pub max: i64,
    pub now: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EventEntry {
    pub link_id: String,
    pub parent_id: String,
    pub name: String,
    pub is_folder: bool,
    pub modify_time: i64,
    pub size: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EventBatch {
    pub now: i64,
    pub events: Vec<EventEntry>,
}

#[derive(Deserialize)]
struct Response<T> {
    #[serde(
        default = "Option::default",
        bound(deserialize = "T: Deserialize<'de>")
    )]
    ok: Option<T>,
    #[serde(default)]
    err: Option<String>,
}

fn cstr(s: &str) -> Result<CString, BridgeError> {
    Ok(CString::new(s)?)
}

unsafe fn take_cstring(p: *mut c_char) -> Result<String, BridgeError> {
    if p.is_null() {
        return Err(BridgeError::Bridge("null pointer from bridge".into()));
    }
    let s = CStr::from_ptr(p).to_str()?.to_owned();
    ffi::pd_free(p);
    Ok(s)
}

fn parse<T: for<'de> Deserialize<'de>>(json: &str) -> Result<T, BridgeError> {
    let r: Response<T> = serde_json::from_str(json)?;
    if let Some(e) = r.err {
        return Err(BridgeError::Bridge(e));
    }
    r.ok.ok_or_else(|| BridgeError::Bridge("empty bridge response".into()))
}

fn parse_unit(json: &str) -> Result<(), BridgeError> {
    #[derive(Deserialize)]
    struct R {
        #[serde(default)]
        err: Option<String>,
    }
    let r: R = serde_json::from_str(json)?;
    match r.err {
        Some(e) if !e.is_empty() => Err(BridgeError::Bridge(e)),
        _ => Ok(()),
    }
}

/// Returns the linked-in Go bridge library version.
pub fn version() -> Result<String, BridgeError> {
    unsafe {
        let p = ffi::pd_version();
        if p.is_null() {
            return Ok("unknown".into());
        }
        let s = CStr::from_ptr(p).to_str()?.to_owned();
        // pd_version uses C.CString without the response wrapper, so
        // it must also be freed via pd_free.
        ffi::pd_free(p);
        Ok(s)
    }
}

/// A handle to an authenticated Proton Drive session inside the Go bridge.
#[derive(Debug, Clone)]
pub struct Bridge {
    session: c_longlong,
}

impl Bridge {
    /// Initialise the bridge with configuration. Does not authenticate.
    pub async fn init(args: InitArgs) -> Result<Self, BridgeError> {
        let json = serde_json::to_string(&args)?;
        let s = tokio::task::spawn_blocking(move || -> Result<c_longlong, BridgeError> {
            let c = cstr(&json)?;
            unsafe {
                let raw = ffi::pd_init(c.as_ptr());
                let json = take_cstring(raw)?;
                #[derive(Deserialize)]
                struct R {
                    session: i64,
                }
                let r: R = parse(&json)?;
                Ok(r.session as c_longlong)
            }
        })
        .await??;
        Ok(Self { session: s })
    }

    /// Log in with username/password (+ optional TOTP and mailbox password).
    /// Returns `LoginOutcome::HvRequired` when Proton demands a CAPTCHA.
    pub async fn login(&self, args: LoginArgs) -> Result<LoginOutcome, BridgeError> {
        let json = serde_json::to_string(&args)?;
        let session = self.session;
        tokio::task::spawn_blocking(move || -> Result<LoginOutcome, BridgeError> {
            let c = cstr(&json)?;
            unsafe {
                let raw = ffi::pd_login(session, c.as_ptr());
                let json = take_cstring(raw)?;
                // Check for {"ok":{"status":"hv_required",...}} before trying
                // to parse as a Credential.
                #[derive(Deserialize)]
                struct MaybeHv {
                    #[serde(default)]
                    ok: Option<serde_json::Value>,
                    #[serde(default)]
                    err: Option<String>,
                }
                let probe: MaybeHv = serde_json::from_str(&json)?;
                if let Some(e) = probe.err {
                    if !e.is_empty() {
                        return Err(BridgeError::Bridge(e));
                    }
                }
                if let Some(v) = probe.ok {
                    if v.get("status").and_then(|s| s.as_str()) == Some("hv_required") {
                        let hv_token = v["hv_token"].as_str().unwrap_or_default().to_string();
                        let methods = v["methods"]
                            .as_array()
                            .map(|a| {
                                a.iter()
                                    .filter_map(|x| x.as_str().map(String::from))
                                    .collect()
                            })
                            .unwrap_or_default();
                        return Ok(LoginOutcome::HvRequired { hv_token, methods });
                    }
                    let cred: Credential = serde_json::from_value(v)?;
                    return Ok(LoginOutcome::Success(cred));
                }
                Err(BridgeError::Bridge("empty bridge response".into()))
            }
        })
        .await?
    }

    /// Complete a login that was blocked by Human Verification.
    /// `hv_type` is typically `"captcha"`. `hv_token` is the solution token
    /// returned by `https://verify.proton.me/`. `fresh_two_fa` is a freshly
    /// generated TOTP code (the original one may have expired).
    pub async fn login_hv(
        &self,
        hv_type: &str,
        hv_token: &str,
        fresh_two_fa: Option<&str>,
    ) -> Result<Credential, BridgeError> {
        #[derive(Serialize)]
        struct HvArgs<'a> {
            hv_type: &'a str,
            hv_token: &'a str,
            two_fa: &'a str,
        }
        let args = HvArgs {
            hv_type,
            hv_token,
            two_fa: fresh_two_fa.unwrap_or_default(),
        };
        let json = serde_json::to_string(&args)?;
        let session = self.session;
        tokio::task::spawn_blocking(move || -> Result<Credential, BridgeError> {
            let c = cstr(&json)?;
            unsafe {
                let raw = ffi::pd_login_hv(session, c.as_ptr());
                let json = take_cstring(raw)?;
                parse(&json)
            }
        })
        .await?
    }

    /// Resume an existing session from a saved [`Credential`].
    pub async fn resume(&self, cred: Credential) -> Result<Credential, BridgeError> {
        let json = serde_json::to_string(&cred)?;
        let session = self.session;
        tokio::task::spawn_blocking(move || -> Result<Credential, BridgeError> {
            let c = cstr(&json)?;
            unsafe {
                let raw = ffi::pd_resume(session, c.as_ptr());
                let json = take_cstring(raw)?;
                parse(&json)
            }
        })
        .await?
    }

    pub async fn logout(self) -> Result<(), BridgeError> {
        let session = self.session;
        tokio::task::spawn_blocking(move || -> Result<(), BridgeError> {
            unsafe {
                let raw = ffi::pd_logout(session);
                let json = take_cstring(raw)?;
                parse_unit(&json)
            }
        })
        .await?
    }

    pub async fn root_link_id(&self) -> Result<String, BridgeError> {
        let session = self.session;
        tokio::task::spawn_blocking(move || -> Result<String, BridgeError> {
            unsafe {
                let raw = ffi::pd_root_link_id(session);
                let json = take_cstring(raw)?;
                parse(&json)
            }
        })
        .await?
    }

    pub async fn list(&self, folder_id: &str) -> Result<Vec<Link>, BridgeError> {
        let id = folder_id.to_owned();
        let session = self.session;
        tokio::task::spawn_blocking(move || -> Result<Vec<Link>, BridgeError> {
            let c = cstr(&id)?;
            unsafe {
                let raw = ffi::pd_list(session, c.as_ptr());
                let json = take_cstring(raw)?;
                parse(&json)
            }
        })
        .await?
    }

    pub async fn get_link(&self, link_id: &str) -> Result<Link, BridgeError> {
        let id = link_id.to_owned();
        let session = self.session;
        tokio::task::spawn_blocking(move || -> Result<Link, BridgeError> {
            let c = cstr(&id)?;
            unsafe {
                let raw = ffi::pd_get_link(session, c.as_ptr());
                let json = take_cstring(raw)?;
                parse(&json)
            }
        })
        .await?
    }

    pub async fn create_folder(&self, parent_id: &str, name: &str) -> Result<String, BridgeError> {
        let p = parent_id.to_owned();
        let n = name.to_owned();
        let session = self.session;
        tokio::task::spawn_blocking(move || -> Result<String, BridgeError> {
            let cp = cstr(&p)?;
            let cn = cstr(&n)?;
            unsafe {
                let raw = ffi::pd_create_folder(session, cp.as_ptr(), cn.as_ptr());
                let json = take_cstring(raw)?;
                parse(&json)
            }
        })
        .await?
    }

    pub async fn upload(
        &self,
        parent_id: &str,
        name: &str,
        src_path: &std::path::Path,
    ) -> Result<UploadResult, BridgeError> {
        let p = parent_id.to_owned();
        let n = name.to_owned();
        let src = src_path.to_string_lossy().to_string();
        let session = self.session;
        tokio::task::spawn_blocking(move || -> Result<UploadResult, BridgeError> {
            let cp = cstr(&p)?;
            let cn = cstr(&n)?;
            let cs = cstr(&src)?;
            unsafe {
                let raw = ffi::pd_upload(session, cp.as_ptr(), cn.as_ptr(), cs.as_ptr());
                let json = take_cstring(raw)?;
                parse(&json)
            }
        })
        .await?
    }

    pub async fn download(
        &self,
        link_id: &str,
        dst_path: &std::path::Path,
    ) -> Result<i64, BridgeError> {
        let id = link_id.to_owned();
        let dst = dst_path.to_string_lossy().to_string();
        let session = self.session;
        tokio::task::spawn_blocking(move || -> Result<i64, BridgeError> {
            let cid = cstr(&id)?;
            let cd = cstr(&dst)?;
            unsafe {
                let raw = ffi::pd_download(session, cid.as_ptr(), cd.as_ptr());
                let json = take_cstring(raw)?;
                #[derive(Deserialize)]
                struct R {
                    size: i64,
                }
                let r: R = parse(&json)?;
                Ok(r.size)
            }
        })
        .await?
    }

    pub async fn rename_or_move(
        &self,
        src_id: &str,
        dst_parent_id: &str,
        dst_name: &str,
    ) -> Result<(), BridgeError> {
        let s = src_id.to_owned();
        let p = dst_parent_id.to_owned();
        let n = dst_name.to_owned();
        let session = self.session;
        tokio::task::spawn_blocking(move || -> Result<(), BridgeError> {
            let cs = cstr(&s)?;
            let cp = cstr(&p)?;
            let cn = cstr(&n)?;
            unsafe {
                let raw = ffi::pd_move(session, cs.as_ptr(), cp.as_ptr(), cn.as_ptr());
                let json = take_cstring(raw)?;
                parse_unit(&json)
            }
        })
        .await?
    }

    pub async fn trash(&self, link_id: &str) -> Result<(), BridgeError> {
        let id = link_id.to_owned();
        let session = self.session;
        tokio::task::spawn_blocking(move || -> Result<(), BridgeError> {
            let c = cstr(&id)?;
            unsafe {
                let raw = ffi::pd_trash(session, c.as_ptr());
                let json = take_cstring(raw)?;
                parse_unit(&json)
            }
        })
        .await?
    }

    pub async fn about(&self) -> Result<AboutInfo, BridgeError> {
        let session = self.session;
        tokio::task::spawn_blocking(move || -> Result<AboutInfo, BridgeError> {
            unsafe {
                let raw = ffi::pd_about(session);
                let json = take_cstring(raw)?;
                parse(&json)
            }
        })
        .await?
    }

    pub async fn search(&self, folder_id: &str, name: &str) -> Result<Option<Link>, BridgeError> {
        let f = folder_id.to_owned();
        let n = name.to_owned();
        let session = self.session;
        tokio::task::spawn_blocking(move || -> Result<Option<Link>, BridgeError> {
            let cf = cstr(&f)?;
            let cn = cstr(&n)?;
            unsafe {
                let raw = ffi::pd_search(session, cf.as_ptr(), cn.as_ptr());
                let json = take_cstring(raw)?;
                let v: serde_json::Value = serde_json::from_str(&json)?;
                if let Some(err) = v.get("err").and_then(|x| x.as_str()) {
                    if !err.is_empty() {
                        return Err(BridgeError::Bridge(err.into()));
                    }
                }
                let ok = v.get("ok").cloned().unwrap_or(serde_json::Value::Null);
                if ok.is_null() {
                    Ok(None)
                } else {
                    Ok(Some(serde_json::from_value(ok)?))
                }
            }
        })
        .await?
    }

    pub async fn events(&self, since: i64) -> Result<EventBatch, BridgeError> {
        let session = self.session;
        tokio::task::spawn_blocking(move || -> Result<EventBatch, BridgeError> {
            unsafe {
                let raw = ffi::pd_events(session, since);
                let json = take_cstring(raw)?;
                parse(&json)
            }
        })
        .await?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_returns_something() {
        let v = version().expect("version");
        assert!(!v.is_empty());
    }

    #[tokio::test]
    async fn init_creates_session() {
        let b = Bridge::init(InitArgs {
            app_version: "web-drive@5.0.30.0".into(),
            user_agent: "ProtonDrive-Linux/0.1.0 test".into(),
            ..Default::default()
        })
        .await
        .expect("init");
        assert!(b.session > 0);
    }
}
