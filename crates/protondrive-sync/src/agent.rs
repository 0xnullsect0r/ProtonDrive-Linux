//! Top-level sync agent — wires the local watcher, remote events
//! poller, reconciliation, conflict resolution, propagation and
//! consolidation into one runnable pipeline.

use crate::conflict::resolve;
use crate::consolidation::consolidate;
use crate::local::{LocalChange, LocalWatcher};
use crate::propagation::Propagator;
use crate::reconciliation::{reconcile, Observations};
use crate::remote::{RemoteChange, RemoteWatcher};
use crate::state::State;
use protondrive_bridge::Bridge;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::broadcast;

#[derive(Debug, Clone)]
pub enum SyncEvent {
    Started,
    Idle,
    Busy { queue: usize },
    Error { message: String },
    Synced { rel: String, direction: Direction },
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
}

impl SyncAgent {
    pub fn new(bridge: Bridge, state: State, root: PathBuf) -> Self {
        let (tx, _) = broadcast::channel(64);
        Self {
            bridge,
            state: Arc::new(parking_lot::Mutex::new(state)),
            root,
            events_tx: tx,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<SyncEvent> {
        self.events_tx.subscribe()
    }

    /// Run forever. Cancellation is achieved by dropping the future.
    pub async fn run(self) -> anyhow::Result<()> {
        let _ = self.events_tx.send(SyncEvent::Started);
        let root_link_id = self.bridge.root_link_id().await?;

        let mut local = LocalWatcher::start(&self.root)?;
        let mut remote = RemoteWatcher::start(self.bridge.clone(), self.state.clone());

        let propagator = Propagator {
            bridge: self.bridge.clone(),
            root: self.root.clone(),
            state: self.state.clone(),
            root_link_id,
        };

        let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));
        let mut buf = Observations::default();

        loop {
            tokio::select! {
                Some(c) = local.rx.recv() => buf.local.push(c),
                Some(c) = remote.rx.recv() => buf.remote.push(c),
                _ = interval.tick() => {
                    if buf.local.is_empty() && buf.remote.is_empty() {
                        continue;
                    }
                    let take = std::mem::take(&mut buf);
                    let _ = self.events_tx.send(SyncEvent::Busy {
                        queue: take.local.len() + take.remote.len(),
                    });
                    let ops = reconcile(take);
                    let resolved = resolve(ops);
                    propagator.apply(resolved).await;
                    consolidate(&self.events_tx);
                }
            }
        }
    }
}

#[allow(dead_code)]
fn _silence_unused(c: LocalChange, r: RemoteChange) -> (LocalChange, RemoteChange) {
    (c, r)
}
