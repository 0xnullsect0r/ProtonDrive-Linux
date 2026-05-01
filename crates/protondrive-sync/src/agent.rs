//! Top-level sync agent — wires the local watcher, remote events
//! poller, reconciliation, conflict resolution, propagation and
//! consolidation into one runnable pipeline.

use crate::conflict::resolve;
use crate::consolidation::consolidate;
use crate::local::{LocalChange, LocalWatcher};
use crate::propagation::Propagator;
use crate::reconciliation::{reconcile, Observations, Operation};
use crate::remote::{RemoteChange, RemoteWatcher};
use crate::state::State;
use chrono::{DateTime, Utc};
use protondrive_bridge::Bridge;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub enum SyncEvent {
    Started,
    /// Sync loop completed a full cycle; `at` is the completion time.
    Idle {
        at: DateTime<Utc>,
    },
    Busy {
        queue: usize,
    },
    Error {
        message: String,
    },
    /// A file/folder has entered the transfer queue (before it starts).
    FileQueued {
        rel: String,
        direction: Direction,
    },
    /// A file was successfully synced.
    Synced {
        rel: String,
        direction: Direction,
        at: DateTime<Utc>,
    },
}

#[derive(Debug, Clone)]
pub enum Direction {
    Up,
    Down,
}

pub type SyncEventTx = broadcast::Sender<SyncEvent>;

pub struct SyncAgent {
    pub bridge: Bridge,
    pub state: Arc<parking_lot::Mutex<State>>,
    pub root: PathBuf,
    pub events_tx: SyncEventTx,
    /// Top-level folder names to skip on the remote side (selective sync).
    pub excluded_paths: Vec<String>,
    /// Optional channel to receive manual "resync now" signals.
    resync_rx: Option<mpsc::UnboundedReceiver<()>>,
}

impl SyncAgent {
    pub fn new(bridge: Bridge, state: State, root: PathBuf) -> Self {
        let (tx, _) = broadcast::channel(256);
        Self {
            bridge,
            state: Arc::new(parking_lot::Mutex::new(state)),
            root,
            events_tx: tx,
            excluded_paths: Vec::new(),
            resync_rx: None,
        }
    }

    /// Like `new`, but also accepts a resync trigger channel (for "Sync Now").
    pub fn new_with_resync(
        bridge: Bridge,
        state: State,
        root: PathBuf,
        resync_rx: mpsc::UnboundedReceiver<()>,
    ) -> Self {
        let (tx, _) = broadcast::channel(256);
        Self {
            bridge,
            state: Arc::new(parking_lot::Mutex::new(state)),
            root,
            events_tx: tx,
            excluded_paths: Vec::new(),
            resync_rx: Some(resync_rx),
        }
    }

    /// Like `new_with_resync`, but accepts a pre-existing state Arc so it can
    /// be shared with the FUSE layer.
    pub fn new_with_state_and_resync(
        bridge: Bridge,
        state: Arc<parking_lot::Mutex<State>>,
        root: PathBuf,
        resync_rx: mpsc::UnboundedReceiver<()>,
    ) -> Self {
        let (tx, _) = broadcast::channel(256);
        Self {
            bridge,
            state,
            root,
            events_tx: tx,
            excluded_paths: Vec::new(),
            resync_rx: Some(resync_rx),
        }
    }

    /// Like `new`, but accepts a pre-existing state Arc.
    #[allow(dead_code)]
    pub fn new_with_state(
        bridge: Bridge,
        state: Arc<parking_lot::Mutex<State>>,
        root: PathBuf,
    ) -> Self {
        let (tx, _) = broadcast::channel(256);
        Self {
            bridge,
            state,
            root,
            events_tx: tx,
            excluded_paths: Vec::new(),
            resync_rx: None,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<SyncEvent> {
        self.events_tx.subscribe()
    }

    /// Run forever. Cancellation is achieved by dropping the future.
    pub async fn run(mut self) -> anyhow::Result<()> {
        let _ = self.events_tx.send(SyncEvent::Started);
        let root_link_id = match self.bridge.root_link_id().await {
            Ok(id) => id,
            Err(e) => {
                let msg = format!("drive init failed: {e}");
                let _ = self.events_tx.send(SyncEvent::Error {
                    message: msg.clone(),
                });
                return Err(anyhow::anyhow!(msg));
            }
        };

        let mut local = LocalWatcher::start(&self.root)?;
        let mut remote = RemoteWatcher::start(self.bridge.clone(), self.state.clone());

        let propagator = Propagator {
            bridge: self.bridge.clone(),
            root: self.root.clone(),
            state: self.state.clone(),
            root_link_id,
            events_tx: Some(self.events_tx.clone()),
        };

        // 2-second batch ticker for watcher events.
        let mut batch_interval = tokio::time::interval(std::time::Duration::from_secs(2));
        // 20-second ticker for idle "heartbeat" Idle events and forcing a cycle.
        let mut heartbeat_interval = tokio::time::interval(std::time::Duration::from_secs(20));
        let mut buf = Observations::default();

        loop {
            tokio::select! {
                Some(c) = local.rx.recv() => buf.local.push(c),
                Some(c) = remote.rx.recv() => buf.remote.push(c),
                Some(_) = async {
                    if let Some(rx) = self.resync_rx.as_mut() {
                        rx.recv().await
                    } else {
                        std::future::pending().await
                    }
                } => {
                    // Manual resync: process immediately whatever we have (possibly empty).
                    let take = std::mem::take(&mut buf);
                    let _ = self.events_tx.send(SyncEvent::Busy { queue: take.local.len() + take.remote.len() + 1 });
                    let ops = reconcile(take, &self.excluded_paths);
                    let resolved = resolve(ops);
                    emit_queued(&resolved, &self.events_tx);
                    propagator.apply(resolved).await;
                    consolidate(&self.events_tx);
                }
                _ = heartbeat_interval.tick() => {
                    // Periodic heartbeat: emit Idle (or Busy if there's backlog).
                    let take = std::mem::take(&mut buf);
                    if take.local.is_empty() && take.remote.is_empty() {
                        let _ = self.events_tx.send(SyncEvent::Idle { at: Utc::now() });
                    } else {
                        let _ = self.events_tx.send(SyncEvent::Busy {
                            queue: take.local.len() + take.remote.len(),
                        });
                        let ops = reconcile(take, &self.excluded_paths);
                        let resolved = resolve(ops);
                        emit_queued(&resolved, &self.events_tx);
                        propagator.apply(resolved).await;
                        consolidate(&self.events_tx);
                    }
                }
                _ = batch_interval.tick() => {
                    if buf.local.is_empty() && buf.remote.is_empty() {
                        continue;
                    }
                    let take = std::mem::take(&mut buf);
                    let _ = self.events_tx.send(SyncEvent::Busy {
                        queue: take.local.len() + take.remote.len(),
                    });
                    let ops = reconcile(take, &self.excluded_paths);
                    let resolved = resolve(ops);
                    emit_queued(&resolved, &self.events_tx);
                    propagator.apply(resolved).await;
                    consolidate(&self.events_tx);
                }
            }
        }
    }
}

/// Emit a `FileQueued` event for every transfer operation so the UI can
/// show the full download/upload queue before work begins.
fn emit_queued(ops: &[Operation], tx: &SyncEventTx) {
    for op in ops {
        let ev = match op {
            Operation::UploadNew { rel, .. } => Some(SyncEvent::FileQueued {
                rel: rel.clone(),
                direction: Direction::Up,
            }),
            Operation::UploadUpdate { mapping, .. } => Some(SyncEvent::FileQueued {
                rel: mapping.rel_path.clone(),
                direction: Direction::Up,
            }),
            Operation::DownloadNew { rel, .. } => Some(SyncEvent::FileQueued {
                rel: rel.clone(),
                direction: Direction::Down,
            }),
            Operation::DownloadUpdate { mapping } => Some(SyncEvent::FileQueued {
                rel: mapping.rel_path.clone(),
                direction: Direction::Down,
            }),
            Operation::CreateLocalDir { rel, .. } => Some(SyncEvent::FileQueued {
                rel: rel.clone(),
                direction: Direction::Down,
            }),
            Operation::CreateRemoteDir { rel, .. } => Some(SyncEvent::FileQueued {
                rel: rel.clone(),
                direction: Direction::Up,
            }),
            _ => None,
        };
        if let Some(ev) = ev {
            let _ = tx.send(ev);
        }
    }
}

#[allow(dead_code)]
fn _silence_unused(c: LocalChange, r: RemoteChange) -> (LocalChange, RemoteChange) {
    (c, r)
}
