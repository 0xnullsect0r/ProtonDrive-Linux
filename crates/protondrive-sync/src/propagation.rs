//! Propagation — apply resolved [`Operation`]s by talking to the
//! Proton bridge and the local filesystem.

use crate::reconciliation::Operation;
use crate::state::{Mapping, State};
use protondrive_bridge::Bridge;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub struct Propagator {
    pub bridge: Bridge,
    pub root: PathBuf,
    pub state: Arc<parking_lot::Mutex<State>>,
    pub root_link_id: String,
}

impl Propagator {
    pub async fn apply(&self, ops: Vec<Operation>) {
        for op in ops {
            if let Err(e) = self.apply_one(op).await {
                tracing::warn!(error = %e, "propagation step failed");
            }
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
                self.record_mapping(&rel, &id, &parent, true, 0, 0)?;
            }
            Operation::CreateLocalDir { rel, link_id } => {
                let abs = self.root.join(&rel);
                std::fs::create_dir_all(&abs)?;
                let parent = self.parent_link_for(&rel).await?;
                self.record_mapping(&rel, &link_id, &parent, true, 0, 0)?;
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
                self.record_mapping(&rel, &result.link_id, &parent, false, size, now())?;
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
                )?;
            }
            Operation::DownloadNew { rel, link_id, .. } => {
                let abs = self.root.join(&rel);
                if let Some(p) = abs.parent() {
                    std::fs::create_dir_all(p)?;
                }
                let bridge = self.bridge.clone();
                let id2 = link_id.clone();
                let abs2 = abs.clone();
                let size = retry(|| {
                    let bridge = bridge.clone();
                    let id2 = id2.clone();
                    let abs2 = abs2.clone();
                    async move { bridge.download(&id2, &abs2).await }
                })
                .await?;
                let parent = self.parent_link_for(&rel).await?;
                self.record_mapping(&rel, &link_id, &parent, false, size, now())?;
            }
            Operation::DownloadUpdate { mapping } => {
                let abs = self.root.join(&mapping.rel_path);
                let bridge = self.bridge.clone();
                let id2 = mapping.link_id.clone();
                let abs2 = abs.clone();
                let size = retry(|| {
                    let bridge = bridge.clone();
                    let id2 = id2.clone();
                    let abs2 = abs2.clone();
                    async move { bridge.download(&id2, &abs2).await }
                })
                .await?;
                self.record_mapping(
                    &mapping.rel_path,
                    &mapping.link_id,
                    &mapping.parent_link_id,
                    mapping.is_folder,
                    size,
                    now(),
                )?;
            }
            Operation::DeleteRemote { mapping } => {
                if !mapping.link_id.is_empty() {
                    self.bridge.trash(&mapping.link_id).await?;
                }
                self.state.lock().delete_by_rel(&mapping.rel_path)?;
            }
            Operation::DeleteLocal { mapping } => {
                let abs = self.root.join(&mapping.rel_path);
                if abs.is_dir() {
                    let _ = std::fs::remove_dir_all(&abs);
                } else {
                    let _ = std::fs::remove_file(&abs);
                }
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
            self.record_mapping(&acc, &id, &cur, true, 0, 0)?;
            cur = id;
        }
        Ok(cur)
    }

    fn record_mapping(
        &self,
        rel: &str,
        link_id: &str,
        parent_link_id: &str,
        is_folder: bool,
        size: i64,
        mtime: i64,
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
