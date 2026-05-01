//! Consolidation — post-propagation bookkeeping. Currently a no-op
//! placeholder that emits a SyncEvent::Idle marker; the propagator
//! already updates the state DB.

use crate::agent::{SyncEvent, SyncEventTx};
use chrono::Utc;

pub fn consolidate(tx: &SyncEventTx) {
    let _ = tx.send(SyncEvent::Idle { at: Utc::now() });
}
