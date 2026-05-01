//! # protondrive-core
//!
//! Core library for the unofficial Linux Proton Drive client. The
//! protocol-level work (SRP, OpenPGP, Drive REST, events, blocks)
//! lives in the Go-backed `protondrive-bridge` crate; this crate
//! ships the cross-cutting glue: configuration, secrets storage,
//! TOTP helpers and the [`Daemon`] orchestrator the UI talks to.

pub mod auth;
pub mod cache;
pub mod config;
pub mod error;
pub mod keyring;
pub mod types;

mod daemon;
pub use daemon::Daemon;
pub use error::{Error, Result};
pub use protondrive_bridge::LoginOutcome;
