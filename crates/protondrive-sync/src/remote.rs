//! Remote adapter — polls the bridge for Drive events and translates
//! them into [`RemoteChange`].

use crate::state::State;
use protondrive_bridge::{Bridge, EventEntry, Link};
use std::collections::VecDeque;
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
    /// Start the remote watcher.
    ///
    /// On first run (cursor == 0) a streaming BFS scan populates the state
    /// database immediately — files in the root appear within seconds rather
    /// than waiting for a full-tree walk to finish.  After the initial scan
    /// the watcher falls through to regular 30-second event polling.
    pub fn start(
        bridge: Bridge,
        state: Arc<parking_lot::Mutex<State>>,
        root_link_id: String,
    ) -> Self {
        let (tx, rx) = mpsc::channel(1024);
        tokio::spawn(async move {
            let cursor = state.lock().cursor().unwrap_or(0);
            if cursor == 0 {
                tracing::info!("remote: starting initial BFS scan of Proton Drive");
                let scanned = bfs_scan(&bridge, &tx, &root_link_id).await;
                tracing::info!(files = scanned, "remote: initial scan complete");

                // Advance cursor to now so subsequent polls only see new edits.
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                let _ = state.lock().set_cursor(now);
            }

            // Regular delta-sync loop (30 s interval).
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
                // Proton's TOS forbids tight polling; 30 s is the
                // conservative interval for third-party clients.
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
        });
        Self { rx }
    }
}

/// Breadth-first scan of the remote drive, streaming each discovered
/// entry as a [`RemoteChange::Upsert`] immediately after its parent
/// folder is listed — so files at the root appear in seconds.
///
/// Returns the total number of entries discovered.
async fn bfs_scan(
    bridge: &Bridge,
    tx: &mpsc::Sender<RemoteChange>,
    root_link_id: &str,
) -> usize {
    // Queue: (folder_link_id, relative_path_prefix)
    let mut queue: VecDeque<(String, String)> = VecDeque::new();
    queue.push_back((root_link_id.to_owned(), String::new()));

    let mut total = 0usize;

    while let Some((folder_id, prefix)) = queue.pop_front() {
        let children: Vec<Link> = match bridge.list(&folder_id).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    folder_id,
                    "initial scan: list failed, skipping folder"
                );
                continue;
            }
        };

        for link in children {
            let rel_path = if prefix.is_empty() {
                link.name.clone()
            } else {
                format!("{prefix}/{}", link.name)
            };

            let entry = EventEntry {
                link_id: link.link_id.clone(),
                parent_id: link.parent_link_id.clone(),
                name: rel_path.clone(),
                is_folder: link.is_folder,
                modify_time: link.modify_time,
                size: link.size,
            };

            // Stop immediately if the consumer dropped the channel.
            if tx.send(RemoteChange::Upsert { entry }).await.is_err() {
                return total;
            }
            total += 1;

            if link.is_folder {
                queue.push_back((link.link_id, rel_path));
            }
        }

        tracing::debug!(total, prefix = %prefix, "initial scan: folder listed");
    }

    total
}
