//! `protondrive-sync` — bidirectional sync engine ported from
//! ProtonDriveApps/windows-drive's 4-stage pipeline:
//!
//! Reconciliation → ConflictResolution → Propagation → Consolidation.
//!
//! Top-level entry point: [`agent::SyncAgent`].

pub mod agent;
pub mod conflict;
pub mod consolidation;
pub mod local;
pub mod propagation;
pub mod reconciliation;
pub mod remote;
pub mod state;

pub use agent::{SyncAgent, SyncEvent};
