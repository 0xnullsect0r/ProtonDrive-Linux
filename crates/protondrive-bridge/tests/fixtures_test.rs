//! Fixture-based tests for Bridge response deserialization.
//!
//! These tests exercise the JSON parsing layer independently of the Go
//! shared library so they run in standard CI without live Proton API
//! access.  Each test loads a recorded JSON fixture from
//! `tests/fixtures/` and asserts that the Rust types deserialize
//! correctly.

use protondrive_bridge::{AboutInfo, Credential, EventBatch, EventEntry};
use serde::Deserialize;

// ── helpers ────────────────────────────────────────────────────────────────

fn fixture(name: &str) -> String {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read fixture {name}: {e}"))
}

/// Minimal stand-in for the internal `Response<T>` wrapper so we can
/// parse the same JSON shapes used by the bridge.
#[derive(Deserialize)]
struct Envelope<T> {
    ok: Option<T>,
    #[serde(default)]
    err: Option<String>,
}

// ── EventBatch ──────────────────────────────────────────────────────────────

#[test]
fn event_batch_full_parses() {
    let json = fixture("event_batch_full.json");
    let batch: EventBatch = serde_json::from_str(&json).expect("EventBatch parse");
    assert_eq!(batch.now, 1_700_000_100);
    assert_eq!(batch.events.len(), 3);
}

#[test]
fn event_batch_entry_fields() {
    let json = fixture("event_batch_full.json");
    let batch: EventBatch = serde_json::from_str(&json).unwrap();
    let file: &EventEntry = batch
        .events
        .iter()
        .find(|e| !e.is_folder)
        .expect("at least one file entry");
    assert_eq!(file.link_id, "L1abc");
    assert_eq!(file.name, "Documents/report.pdf");
    assert_eq!(file.size, 204_800);
    assert!(!file.is_folder);
}

#[test]
fn event_batch_folder_entry() {
    let json = fixture("event_batch_full.json");
    let batch: EventBatch = serde_json::from_str(&json).unwrap();
    let folder: &EventEntry = batch
        .events
        .iter()
        .find(|e| e.is_folder)
        .expect("at least one folder entry");
    assert!(folder.is_folder);
    assert_eq!(folder.link_id, "L2def");
}

#[test]
fn event_batch_empty_parses() {
    let json = fixture("event_batch_empty.json");
    let batch: EventBatch = serde_json::from_str(&json).expect("empty EventBatch parse");
    assert_eq!(batch.now, 1_700_000_200);
    assert!(batch.events.is_empty());
}

// ── Login responses ─────────────────────────────────────────────────────────

#[test]
fn login_success_parses_credential() {
    let json = fixture("login_success.json");
    let env: Envelope<Credential> = serde_json::from_str(&json).expect("login_success parse");
    assert!(env.err.is_none());
    let cred = env.ok.expect("ok field present");
    assert_eq!(cred.uid, "user-uid-123");
    assert!(!cred.access_token.is_empty());
    assert!(!cred.refresh_token.is_empty());
}

#[test]
fn login_hv_required_parses() {
    let json = fixture("login_hv_required.json");
    let env: Envelope<serde_json::Value> = serde_json::from_str(&json).expect("hv_required parse");
    assert!(env.err.is_none());
    let v = env.ok.expect("ok field present");
    assert_eq!(
        v.get("status").and_then(|s| s.as_str()),
        Some("hv_required")
    );
    assert!(v["hv_token"].as_str().is_some());
    let methods = v["methods"].as_array().expect("methods array");
    assert!(!methods.is_empty());
}

// ── AboutInfo ───────────────────────────────────────────────────────────────

#[test]
fn about_info_parses() {
    let json = fixture("about_info.json");
    let info: AboutInfo = serde_json::from_str(&json).expect("AboutInfo parse");
    assert_eq!(info.email, "alice@proton.me");
    assert!(info.max > info.used);
}
