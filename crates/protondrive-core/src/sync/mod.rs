//! Sync engine: 20-second poller, manual refresh, pin manager.

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;
use tracing::{info, warn};

use crate::api::ApiClient;
use crate::cache::{BlobCache, MetadataDb};
use crate::Result;

/// Cursor key in the metadata kv table.
const CURSOR_KEY: &str = "events_cursor";

#[derive(Clone)]
pub struct SyncEngine {
    api:     ApiClient,
    db:      MetadataDb,
    blobs:   Arc<BlobCache>,
    poll_interval: Duration,
    /// Trigger a poll right now (e.g. user clicked "Refresh" in the tray).
    refresh: Arc<Notify>,
}

impl SyncEngine {
    pub fn new(api: ApiClient, db: MetadataDb, blobs: Arc<BlobCache>, poll_interval: Duration) -> Self {
        Self { api, db, blobs, poll_interval, refresh: Arc::new(Notify::new()) }
    }

    /// Trigger an immediate poll. Safe to call from any thread.
    pub fn refresh_now(&self) { self.refresh.notify_one(); }

    /// Run forever. Spawn this on the tokio runtime.
    pub async fn run(self) {
        let mut ticker = tokio::time::interval(self.poll_interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = ticker.tick()           => {}
                _ = self.refresh.notified() => { info!("manual refresh"); }
            }
            if let Err(e) = self.poll_once().await {
                warn!(error=%e, "sync poll failed");
            }
        }
    }

    async fn poll_once(&self) -> Result<()> {
        let cursor = self.db.get_kv(CURSOR_KEY)?.unwrap_or_default();
        match self.api.events(&cursor).await {
            Ok(resp) => {
                if crate::api::events::server_requested_full_resync(&resp) {
                    warn!("server requested full resync — TODO: re-list shares");
                }
                for ev in &resp.Events {
                    self.apply_event(ev)?;
                }
                self.db.put_kv(CURSOR_KEY, &resp.EventID)?;
                // Re-check pinned items once per tick so they stay warm.
                self.refresh_pinned().await?;
                // Keep cache under cap.
                let _ = self.blobs.evict_if_needed();
                Ok(())
            }
            Err(crate::Error::NotImplemented(_)) => {
                // Expected until the API client is implemented; don't spam logs.
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    fn apply_event(&self, _ev: &crate::api::model::EventEntry) -> Result<()> {
        // TODO: convert encrypted Link payloads → Node, decrypt name with the
        // crypto module, upsert/delete in the metadata DB.
        Ok(())
    }

    async fn refresh_pinned(&self) -> Result<()> {
        for n in self.db.pinned_nodes()? {
            tracing::trace!(node=%n.id.0, "pinned-keepalive (todo: prefetch blocks)");
            // TODO: walk pinned subtree, re-fetch any block whose revision changed.
        }
        Ok(())
    }
}
