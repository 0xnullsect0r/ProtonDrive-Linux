//! Sync lifecycle management for the UI.
//!
//! [`SyncController`] owns the running [`SyncAgent`] task and exposes
//! controls (start, stop, trigger resync) along with an event channel
//! that GTK widgets can subscribe to.

use async_channel::{Receiver, Sender};
use parking_lot::Mutex;
use protondrive_core::Daemon;
use protondrive_sync::{
    state::{default_state_path, State},
    SyncAgent, SyncEvent, SyncEventTx,
};
use std::sync::Arc;
use tokio::{runtime::Handle, sync::mpsc, task::JoinHandle};

/// Shared sync state passed to UI pages.
#[derive(Clone)]
pub struct SyncController {
    rt: Handle,
    task: Arc<Mutex<Option<JoinHandle<()>>>>,
    event_tx: Arc<Mutex<Option<SyncEventTx>>>,
    resync_tx: Arc<Mutex<Option<mpsc::UnboundedSender<()>>>>,
    /// Async-channel sender — used internally to bridge tokio→glib.
    ui_tx: Arc<Mutex<Option<Sender<SyncEvent>>>>,
    /// Async-channel receiver — handed out to UI once.
    pub ui_rx: Arc<Mutex<Option<Receiver<SyncEvent>>>>,
    /// Active FUSE mount session — dropped to unmount.
    fuse_session: Arc<Mutex<Option<protondrive_fuse::FuseSession>>>,
    /// Shared SQLite state with the FUSE layer.
    shared_state: Arc<Mutex<Option<Arc<parking_lot::Mutex<State>>>>>,
}

impl SyncController {
    pub fn new(rt: Handle) -> Self {
        let (ui_tx, ui_rx) = async_channel::bounded(256);
        Self {
            rt,
            task: Arc::new(Mutex::new(None)),
            event_tx: Arc::new(Mutex::new(None)),
            resync_tx: Arc::new(Mutex::new(None)),
            ui_tx: Arc::new(Mutex::new(Some(ui_tx))),
            ui_rx: Arc::new(Mutex::new(Some(ui_rx))),
            fuse_session: Arc::new(Mutex::new(None)),
            shared_state: Arc::new(Mutex::new(None)),
        }
    }

    /// Take the async-channel receiver (can only be taken once per controller
    /// instance since it gives ownership to the GTK widget).
    pub fn take_ui_rx(&self) -> Option<Receiver<SyncEvent>> {
        self.ui_rx.lock().take()
    }

    pub fn is_running(&self) -> bool {
        self.task
            .lock()
            .as_ref()
            .map(|h| !h.is_finished())
            .unwrap_or(false)
    }

    /// Start (or restart) the sync agent for the given daemon.
    /// Safe to call from any thread (uses the tokio Handle).
    pub fn start(&self, daemon: &Daemon) -> anyhow::Result<()> {
        // Stop any existing agent (and FUSE mount).
        self.stop_internal();

        let bridge = daemon
            .bridge
            .lock()
            .clone()
            .ok_or_else(|| anyhow::anyhow!("not authenticated"))?;

        let root = daemon.config.lock().sync_root.clone();
        let excluded_paths = daemon.config.lock().excluded_paths.clone();
        let _ = std::fs::create_dir_all(&root);

        let state_path = default_state_path();
        let state = Arc::new(parking_lot::Mutex::new(State::open(&state_path)?));
        *self.shared_state.lock() = Some(state.clone());

        // Fetch root_link_id to start the FUSE mount.  pd_root_link_id() reads
        // in-memory Go state so it's very fast even though it goes through
        // spawn_blocking.
        let root_link_id = {
            let b = bridge.clone();
            match self.rt.block_on(async move { b.root_link_id().await }) {
                Ok(id) => Some(id),
                Err(e) => {
                    tracing::warn!(error=%e, "could not get root_link_id for FUSE; skipping mount");
                    None
                }
            }
        };

        // Start FUSE virtual filesystem.
        if let Some(rid) = root_link_id {
            let cache_dir = dirs::cache_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
                .join("protondrive")
                .join("files");
            let mount_root = root.clone();
            let fuse_state = state.clone();
            let fuse_bridge = bridge.clone();
            let rt_handle = self.rt.clone();

            match protondrive_fuse::mount(fuse_state, fuse_bridge, cache_dir, &mount_root, rid, rt_handle) {
                Ok(session) => {
                    *self.fuse_session.lock() = Some(session);
                    tracing::info!("FUSE mount started at {}", mount_root.display());
                }
                Err(e) => {
                    tracing::warn!(error=%e, "FUSE mount failed; files won't appear in folder");
                }
            }
        }

        let (resync_tx, resync_rx) = mpsc::unbounded_channel();
        let mut agent = SyncAgent::new_with_state_and_resync(bridge, state, root, resync_rx);
        agent.excluded_paths = excluded_paths;
        let broadcast_tx = agent.events_tx.clone();

        // Bridge broadcast → async_channel so glib can receive events.
        let mut bcast_rx = broadcast_tx.subscribe();
        let ui_tx_clone = self.ui_tx.lock().clone();
        self.rt.spawn(async move {
            loop {
                match bcast_rx.recv().await {
                    Ok(ev) => {
                        if let Some(tx) = &ui_tx_clone {
                            let _ = tx.send(ev).await;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(skipped = n, "sync event channel lagged");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        *self.event_tx.lock() = Some(broadcast_tx);
        *self.resync_tx.lock() = Some(resync_tx);

        let handle = self.rt.spawn(async move {
            if let Err(e) = agent.run().await {
                tracing::error!(error = %e, "sync agent stopped");
            }
        });
        *self.task.lock() = Some(handle);
        Ok(())
    }

    /// Trigger an immediate resync without restarting the agent.
    #[allow(dead_code)]
    pub fn trigger_resync(&self) {
        if let Some(tx) = self.resync_tx.lock().as_ref() {
            let _ = tx.send(());
        }
    }

    /// Restart the agent (abort current, start fresh — gives a clean full scan).
    #[allow(dead_code)]
    pub fn restart(&self, daemon: &Daemon) {
        if let Err(e) = self.start(daemon) {
            tracing::warn!(error=%e, "sync restart failed");
        }
    }

    /// Stop the running agent and unmount FUSE.
    pub fn stop(&self) {
        self.stop_internal();
    }

    fn stop_internal(&self) {
        // Drop the event/resync senders first so the broadcast channel closes
        // and the forwarding task exits cleanly.
        *self.event_tx.lock() = None;
        *self.resync_tx.lock() = None;

        if let Some(h) = self.task.lock().take() {
            h.abort();
            let _ = self.rt.block_on(h);
        }

        // Unmount the FUSE filesystem by dropping the BackgroundSession.
        *self.fuse_session.lock() = None;
        *self.shared_state.lock() = None;
    }
}
