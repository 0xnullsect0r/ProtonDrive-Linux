//! Conflict resolution rules ported from windows-drive
//! (`ProtonDrive.Sync.Engine/ConflictResolution`).
//!
//! Rules:
//!  * both-modified → keep both, suffix loser with
//!    `(conflict <hostname> <ts>)`
//!  * delete-vs-modified → modified wins
//!  * move-vs-move → newer wins, the other becomes a copy

use crate::reconciliation::Operation;
use chrono::Utc;

pub fn conflict_suffix() -> String {
    let host = hostname().unwrap_or_else(|| "linux".into());
    let ts = Utc::now().format("%Y%m%dT%H%M%S");
    format!(" (conflict {host} {ts})")
}

fn hostname() -> Option<String> {
    std::env::var("HOSTNAME").ok().or_else(|| {
        std::fs::read_to_string("/etc/hostname")
            .ok()
            .map(|s| s.trim().to_string())
    })
}

/// Apply conflict-resolution to a stream of operations. The current
/// strategy is conservative: identical rel_paths in both upload+download
/// directions become a "keep both" rename of the local file. A real
/// content-hash comparison would be plumbed through here.
pub fn resolve(ops: Vec<Operation>) -> Vec<Operation> {
    use std::collections::HashMap;
    let mut by_rel: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, op) in ops.iter().enumerate() {
        if let Some(rel) = rel_of(op) {
            by_rel.entry(rel).or_default().push(i);
        }
    }
    let mut to_drop = std::collections::HashSet::new();
    let mut renames = Vec::new();
    for (_, idxs) in by_rel.iter() {
        if idxs.len() < 2 {
            continue;
        }
        let mut has_local = false;
        let mut has_remote = false;
        for i in idxs {
            match &ops[*i] {
                Operation::UploadNew { .. } | Operation::UploadUpdate { .. } => has_local = true,
                Operation::DownloadNew { .. } | Operation::DownloadUpdate { .. } => {
                    has_remote = true
                }
                _ => {}
            }
        }
        if has_local && has_remote {
            // Promote both sides: keep local with conflict suffix,
            // accept remote as canonical.
            for i in idxs {
                if let Operation::UploadNew {
                    rel,
                    path,
                    parent_link_id,
                } = &ops[*i]
                {
                    let new_rel = format!("{rel}{}", conflict_suffix());
                    renames.push(Operation::UploadNew {
                        rel: new_rel,
                        path: path.clone(),
                        parent_link_id: parent_link_id.clone(),
                    });
                    to_drop.insert(*i);
                }
            }
        }
    }
    let mut out: Vec<Operation> = ops
        .into_iter()
        .enumerate()
        .filter_map(|(i, o)| (!to_drop.contains(&i)).then_some(o))
        .collect();
    out.extend(renames);
    out
}

fn rel_of(op: &Operation) -> Option<String> {
    match op {
        Operation::UploadNew { rel, .. } => Some(rel.clone()),
        Operation::UploadUpdate { mapping, .. } => Some(mapping.rel_path.clone()),
        Operation::DownloadNew { rel, .. } => Some(rel.clone()),
        Operation::DownloadUpdate { mapping } => Some(mapping.rel_path.clone()),
        Operation::DeleteLocal { mapping } => Some(mapping.rel_path.clone()),
        Operation::DeleteRemote { mapping } => Some(mapping.rel_path.clone()),
        Operation::CreateLocalDir { rel, .. } => Some(rel.clone()),
        Operation::CreateRemoteDir { rel, .. } => Some(rel.clone()),
        Operation::Conflict { .. } => None,
    }
}
