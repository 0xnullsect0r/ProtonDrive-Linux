//! # protondrive-core
//!
//! Core library for the unofficial Linux Proton Drive client.
//!
//! Layers (each in its own module):
//!
//! - [`config`] — on-disk config + paths (XDG dirs).
//! - [`keyring`] — Secret Service / libsecret credential storage.
//! - [`auth`] — Proton SRP authentication + TOTP.
//! - [`api`] — REST client for the Proton Drive endpoints.
//! - [`crypto`] — OpenPGP layer (key/share/node/content keys, block decryption).
//! - [`cache`] — SQLite metadata DB + content-addressed blob cache.
//! - [`sync`] — periodic poller, pin manager, refresh-on-demand.
//! - [`types`] — shared data types (NodeId, Revision, etc.).
//!
//! See [`Daemon`] for the top-level orchestrator the UI / FUSE layer uses.

pub mod api;
pub mod auth;
pub mod cache;
pub mod config;
pub mod crypto;
pub mod error;
pub mod keyring;
pub mod sync;
pub mod types;

mod daemon;
pub use daemon::Daemon;
pub use error::{Error, Result};
