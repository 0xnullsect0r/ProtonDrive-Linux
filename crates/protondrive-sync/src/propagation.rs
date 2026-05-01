//! Propagation — apply resolved [`Operation`]s by talking to the
//! Proton bridge and the local filesystem.

use crate::agent::{Direction, SyncEvent, SyncEventTx};
use crate::reconciliation::Operation;
use crate::state::{Mapping, State};
use chrono::Utc;
use protondrive_bridge::Bridge;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub struct Propagator {
    pub bridge: Bridge,
    pub root: PathBuf,
    pub state: Arc<parking_lot::Mutex<State>>,
    pub root_link_id: String,
    /// Optional channel for emitting sync activity events to the UI.
    pub events_tx: Option<SyncEventTx>,
}

impl Propagator {
    pub async fn apply(&self, ops: Vec<Operation>) {
        // Separate pure-state ops (no network I/O) from ops that need the bridge.
        // Batching the state-only ops into a single transaction is dramatically
        // faster on large initial scans (100K+ files).
        let mut batch_mappings: Vec<Mapping> = Vec::new();
        let mut other_ops: Vec<Operation> = Vec::new();

        for op in ops {
            match op {
                Operation::DownloadNew {
                    ref rel,
                    ref link_id,
                    ref parent_link_id,
                    size,
                    mtime,
                    ..
                } => {
                    let parent = if parent_link_id.is_empty() {
                        match self.parent_link_for(rel).await {
                            Ok(p) => p,
                            Err(e) => {
                                tracing::warn!(error=%e, "parent_link_for failed, skipping");
                                continue;
                            }
                        }
                    } else {
                        parent_link_id.clone()
                    };
                    batch_mappings.push(Mapping {
                        id: 0,
                        rel_path: rel.clone(),
                        link_id: link_id.clone(),
                        parent_link_id: parent,
                        is_folder: false,
                        local_size: size,
                        local_mtime: mtime,
                        local_hash: None,
                        remote_size: size,
                        remote_mtime: mtime,
                        remote_hash: None,
                        is_materialized: false,
                    });
                }
                Operation::CreateLocalDir {
                    ref rel,
                    ref link_id,
                    ref parent_link_id,
                } => {
                    let parent = if parent_link_id.is_empty() {
                        match self.parent_link_for(rel).await {
                            Ok(p) => p,
                            Err(e) => {
                                tracing::warn!(error=%e, "parent_link_for failed, skipping");
                                continue;
                            }
                        }
                    } else {
                        parent_link_id.clone()
                    };
                    batch_mappings.push(Mapping {
                        id: 0,
                        rel_path: rel.clone(),
                        link_id: link_id.clone(),
                        parent_link_id: parent,
                        is_folder: true,
                        local_size: 0,
                        local_mtime: 0,
                        local_hash: None,
                        remote_size: 0,
                        remote_mtime: 0,
                        remote_hash: None,
                        is_materialized: false,
                    });
                }
                other => other_ops.push(other),
            }
        }

        // Commit all placeholder mappings in one transaction.
        if !batch_mappings.is_empty() {
            let count = batch_mappings.len();
            if let Err(e) = self.state.lock().upsert_mappings_batch(&batch_mappings) {
                tracing::warn!(error=%e, "batch upsert failed");
            } else {
                tracing::debug!(count, "batch-inserted placeholder mappings");
                // Emit a single "synced" event per entry for UI progress.
                for m in &batch_mappings {
                    self.emit_synced(&m.rel_path, Direction::Down);
                }
            }
        }

        // Process remaining ops that need the bridge or disk I/O.
        for op in other_ops {
            if let Err(e) = self.apply_one(op).await {
                tracing::warn!(error = %e, "propagation step failed");
            }
        }
    }

    fn emit_synced(&self, rel: &str, direction: Direction) {
        if let Some(tx) = &self.events_tx {
            let _ = tx.send(SyncEvent::Synced {
                rel: rel.to_string(),
                direction,
                at: Utc::now(),
            });
        }
    }

    async fn apply_one(&self, op: Operation) -> anyhow::Result<()> {
        match op {
            Operation::CreateRemoteDir {
                rel,
                parent_link_id: _,
            } => {
                let parent = self.parent_link_for(&rel).await?;
                let name = leaf(&rel).to_string();
                let id = retry(|| self.bridge.create_folder(&parent, &name)).await?;
                self.record_mapping(&rel, &id, &parent, true, 0, 0, false)?;
                self.emit_synced(&rel, Direction::Up);
            }
            Operation::CreateLocalDir {
                rel,
                link_id,
                parent_link_id,
            } => {
                // Directories are served virtually by the FUSE layer;
                // we just record the mapping so readdir can list them.
                let parent = if parent_link_id.is_empty() {
                    self.parent_link_for(&rel).await?
                } else {
                    parent_link_id
                };
                self.record_mapping(&rel, &link_id, &parent, true, 0, 0, false)?;
                self.emit_synced(&rel, Direction::Down);
            }
            Operation::UploadNew {
                rel,
                path,
                parent_link_id: _,
            } => {
                let parent = self.parent_link_for(&rel).await?;
                let name = leaf(&rel).to_string();
                let bridge = self.bridge.clone();
                let p2 = path.clone();
                let n2 = name.clone();
                let pl = parent.clone();
                let result = retry(|| {
                    let bridge = bridge.clone();
                    let p2 = p2.clone();
                    let n2 = n2.clone();
                    let pl = pl.clone();
                    async move { bridge.upload(&pl, &n2, &p2).await }
                })
                .await?;
                let size = std::fs::metadata(&path)
                    .map(|m| m.len() as i64)
                    .unwrap_or(0);
                self.record_mapping(&rel, &result.link_id, &parent, false, size, now(), true)?;
                self.emit_synced(&rel, Direction::Up);
            }
            Operation::UploadUpdate { mapping, path } => {
                // For now treat as UploadNew (creates new revision via
                // bridge ReplaceExistingDraft). Real impl would call a
                // dedicated NewRevision endpoint.
                let bridge = self.bridge.clone();
                let p2 = path.clone();
                let n2 = leaf(&mapping.rel_path).to_string();
                let pl = mapping.parent_link_id.clone();
                let _ = retry(|| {
                    let bridge = bridge.clone();
                    let p2 = p2.clone();
                    let n2 = n2.clone();
                    let pl = pl.clone();
                    async move { bridge.upload(&pl, &n2, &p2).await }
                })
                .await?;
                let size = std::fs::metadata(&path)
                    .map(|m| m.len() as i64)
                    .unwrap_or(0);
                self.record_mapping(
                    &mapping.rel_path,
                    &mapping.link_id,
                    &mapping.parent_link_id,
                    false,
                    size,
                    now(),
                    true,
                )?;
                self.emit_synced(&mapping.rel_path, Direction::Up);
            }
            Operation::DownloadNew {
                rel,
                link_id,
                parent_link_id,
                size,
                mtime,
                ..
            } => {
                // Files are served on-demand by the FUSE layer.
                // Just record the mapping as a placeholder (not materialized).
                let parent = if parent_link_id.is_empty() {
                    self.parent_link_for(&rel).await?
                } else {
                    parent_link_id
                };
                self.record_mapping(&rel, &link_id, &parent, false, size, mtime, false)?;
                self.emit_synced(&rel, Direction::Down);
            }
            Operation::DownloadUpdate { mapping } => {
                // Invalidate the cache file if it exists so the FUSE layer
                // will re-download on next access.
                let cache_dir = directories::BaseDirs::new()
                    .map(|b| b.cache_dir().join("protondrive").join("files"))
                    .unwrap_or_else(|| std::path::PathBuf::from("/tmp/protondrive-cache"));
                let cache_path = cache_dir.join(&mapping.link_id);
                if cache_path.exists() {
                    let _ = std::fs::remove_file(&cache_path);
                }
                self.record_mapping(
                    &mapping.rel_path,
                    &mapping.link_id,
                    &mapping.parent_link_id,
                    mapping.is_folder,
                    mapping.remote_size,
                    mapping.remote_mtime,
                    false,
                )?;
                self.emit_synced(&mapping.rel_path, Direction::Down);
            }
            Operation::DeleteRemote { mapping } => {
                if !mapping.link_id.is_empty() {
                    self.bridge.trash(&mapping.link_id).await?;
                }
                self.state.lock().delete_by_rel(&mapping.rel_path)?;
            }
            Operation::DeleteLocal { mapping } => {
                // With FUSE, files aren't on disk — removing from state DB
                // causes them to disappear from the virtual filesystem.
                // Clean up the download cache as well.
                let cache_dir = directories::BaseDirs::new()
                    .map(|b| b.cache_dir().join("protondrive").join("files"))
                    .unwrap_or_else(|| std::path::PathBuf::from("/tmp/protondrive-cache"));
                let _ = std::fs::remove_file(cache_dir.join(&mapping.link_id));
                self.state.lock().delete_by_rel(&mapping.rel_path)?;
            }
            Operation::Conflict { .. } => {
                tracing::warn!("conflict operation passed to propagator without resolution");
            }
        }
        Ok(())
    }

    async fn parent_link_for(&self, rel: &str) -> anyhow::Result<String> {
        let parent_rel = parent(rel);
        if parent_rel.is_empty() {
            return Ok(self.root_link_id.clone());
        }
        if let Some(m) = self.state.lock().get_by_rel(&parent_rel)? {
            return Ok(m.link_id);
        }
        // Best effort: walk down from the root, creating folders as
        // needed. The reconciliation stage normally ensures parents
        // exist before children, so this is a fallback.
        let mut cur = self.root_link_id.clone();
        let mut acc = String::new();
        for seg in parent_rel.split('/') {
            if seg.is_empty() {
                continue;
            }
            acc = if acc.is_empty() {
                seg.into()
            } else {
                format!("{acc}/{seg}")
            };
            if let Some(m) = self.state.lock().get_by_rel(&acc)? {
                cur = m.link_id;
                continue;
            }
            let id = self.bridge.create_folder(&cur, seg).await?;
            self.record_mapping(&acc, &id, &cur, true, 0, 0, false)?;
            cur = id;
        }
        Ok(cur)
    }

    #[allow(clippy::too_many_arguments)]
    fn record_mapping(
        &self,
        rel: &str,
        link_id: &str,
        parent_link_id: &str,
        is_folder: bool,
        size: i64,
        mtime: i64,
        is_materialized: bool,
    ) -> anyhow::Result<()> {
        let m = Mapping {
            id: 0,
            rel_path: rel.into(),
            link_id: link_id.into(),
            parent_link_id: parent_link_id.into(),
            is_folder,
            local_size: size,
            local_mtime: mtime,
            local_hash: None,
            remote_size: size,
            remote_mtime: mtime,
            remote_hash: None,
            is_materialized,
        };
        self.state.lock().upsert_mapping(&m)?;
        Ok(())
    }
}

fn leaf(rel: &str) -> &str {
    rel.rsplit('/').next().unwrap_or(rel)
}

fn parent(rel: &str) -> String {
    match rel.rfind('/') {
        Some(i) => rel[..i].to_string(),
        None => String::new(),
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

async fn retry<T, E, F, Fut>(mut f: F) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
    E: std::fmt::Display,
{
    let mut attempt = 0;
    loop {
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) if attempt >= 3 => return Err(e),
            Err(e) => {
                tracing::warn!(error = %e, attempt, "retrying after error");
                tokio::time::sleep(Duration::from_millis(500 * (1 << attempt))).await;
                attempt += 1;
            }
        }
    }
}

#[allow(dead_code)]
fn _path_unused(_p: &Path) {}
