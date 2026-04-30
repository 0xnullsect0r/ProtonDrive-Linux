//! Helpers around the Proton Drive **events feed**.
//!
//! The feed is a forward-only log of node mutations. The poller persists the
//! latest `EventID` in the metadata DB and asks for everything after it on
//! each tick. If `Refresh != 0`, the server is asking us to drop our cache
//! and re-list the tree from scratch.

use crate::api::model::EventsResp;

#[derive(Debug, Clone, Copy)]
pub enum EventKind {
    Delete,
    Create,
    Update,
    UpdateMeta,
    Unknown,
}

impl From<i32> for EventKind {
    fn from(v: i32) -> Self {
        match v {
            0 => Self::Delete,
            1 => Self::Create,
            2 => Self::Update,
            3 => Self::UpdateMeta,
            _ => Self::Unknown,
        }
    }
}

pub fn server_requested_full_resync(r: &EventsResp) -> bool {
    r.Refresh != 0
}
