//! Authentication helpers.
//!
//! The actual SRP/2FA exchange is performed by the Go bridge
//! (`protondrive-bridge`); this module only ships the small helpers
//! the UI needs locally — a TOTP-secret validator and code preview.

pub mod totp;
