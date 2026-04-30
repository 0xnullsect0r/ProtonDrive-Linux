//! Consolidation — post-propagation bookkeeping. Currently a no-op
//! placeholder that emits a SyncEvent::Idle marker; the propagator
//! already updates the state DB.

use crate::agent::{SyncEvent, SyncEventTx};

pub fn consolidate(tx: &SyncEventTx) {
    let _ = tx.send(SyncEvent::Idle);
}
