//! Remote adapter — polls the bridge for Drive events and translates
//! them into [`RemoteChange`].

use crate::state::State;
use protondrive_bridge::{Bridge, EventEntry};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub enum RemoteChange {
    Upsert { entry: EventEntry },
    Removed { link_id: String },
}

pub struct RemoteWatcher {
    pub rx: mpsc::Receiver<RemoteChange>,
}

impl RemoteWatcher {
    pub fn start(bridge: Bridge, state: Arc<parking_lot::Mutex<State>>) -> Self {
        let (tx, rx) = mpsc::channel(1024);
        tokio::spawn(async move {
            loop {
                let cursor = state.lock().cursor().unwrap_or(0);
                match bridge.events(cursor).await {
                    Ok(batch) => {
                        for ev in batch.events {
                            let _ = tx.send(RemoteChange::Upsert { entry: ev }).await;
                        }
                        if batch.now > 0 {
                            let _ = state.lock().set_cursor(batch.now);
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "remote events poll failed");
                    }
                }
                // Proton's TOS forbids tight polling; 30s is the
                // conservative interval used by other 3rd-party
                // clients while we don't have a true /events endpoint.
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
        });
        Self { rx }
    }
}
