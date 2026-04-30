//! Wire-format DTOs for the Proton Drive API.
//!
//! These are deliberately *minimal* — only the fields we actually use today.
//! Extend them as you port more endpoints.

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct ApiEnvelope<T> {
    pub Code: i64,
    #[serde(default)]
    pub Error: Option<String>,
    #[serde(flatten)]
    pub data: T,
}

#[derive(Debug, Deserialize)]
pub struct AuthInfoResp {
    pub Modulus: String,
    pub ServerEphemeral: String,
    pub Salt: String,
    pub Version: u8,
    pub SRPSession: String,
}

#[derive(Debug, Serialize)]
pub struct AuthReq<'a> {
    pub Username: &'a str,
    pub ClientEphemeral: &'a str,
    pub ClientProof: &'a str,
    pub SRPSession: &'a str,
}

#[derive(Debug, Deserialize)]
pub struct AuthResp {
    pub UID: String,
    pub AccessToken: String,
    pub RefreshToken: String,
    pub TokenType: String,
    pub Scopes: Vec<String>,
    pub ServerProof: String,
}

#[derive(Debug, Serialize)]
pub struct TwoFactorReq<'a> {
    pub TwoFactorCode: &'a str,
}

#[derive(Debug, Deserialize)]
pub struct LinkResp {
    pub LinkID: String,
    pub ParentLinkID: Option<String>,
    pub Type: i32, // 1=folder, 2=file
    pub Name: String, // base64-encrypted
    pub Size: u64,
    pub MIMEType: Option<String>,
    pub ModifyTime: i64,
    pub ActiveRevisionID: Option<String>,
    /// PGP-encrypted node key armored.
    pub NodeKey: String,
    pub NodePassphrase: String,
    pub NodePassphraseSignature: String,
}

/// One server-side change in the events feed.
#[derive(Debug, Deserialize)]
pub struct EventEntry {
    pub EventID: String,
    pub EventType: i32, // 0=delete, 1=create, 2=update, 3=update_metadata
    pub Link: Option<LinkResp>,
    pub LinkID: String,
}

#[derive(Debug, Deserialize)]
pub struct EventsResp {
    pub EventID: String,
    pub Events: Vec<EventEntry>,
    pub More: i32,
    pub Refresh: i32,
}
