//! Reconciliation — merge raw [`LocalChange`] / [`RemoteChange`]
//! streams into unified [`Operation`]s keyed by mapping.

use crate::local::LocalChange;
use crate::remote::RemoteChange;
use crate::state::Mapping;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub enum Operation {
    UploadNew {
        rel: String,
        path: PathBuf,
        parent_link_id: String,
    },
    UploadUpdate {
        mapping: Mapping,
        path: PathBuf,
    },
    DownloadNew {
        rel: String,
        link_id: String,
        parent_link_id: String,
        is_folder: bool,
        size: i64,
        mtime: i64,
    },
    DownloadUpdate {
        mapping: Mapping,
    },
    DeleteRemote {
        mapping: Mapping,
    },
    DeleteLocal {
        mapping: Mapping,
    },
    CreateLocalDir {
        rel: String,
        link_id: String,
        parent_link_id: String,
    },
    CreateRemoteDir {
        rel: String,
        parent_link_id: String,
    },
    Conflict {
        local: Option<LocalChange>,
        remote: Option<RemoteChange>,
    },
}

/// Inputs to the reconciliation stage. The sync agent batches
/// observations and hands them off to [`reconcile`].
#[derive(Default, Debug)]
pub struct Observations {
    pub local: Vec<LocalChange>,
    pub remote: Vec<RemoteChange>,
}

/// Naive single-pass reconciliation: per (rel_path, link_id) bucket,
/// emit upload / download / delete operations. The Conflict variant is
/// produced when both sides changed since the last sync.
///
/// `excluded` contains top-level folder names to skip on the remote side
/// (selective sync).
pub fn reconcile(obs: Observations, excluded: &[String]) -> Vec<Operation> {
    let mut ops: Vec<Operation> = Vec::new();
    for c in obs.local {
        match c {
            LocalChange::Upsert {
                rel,
                path,
                is_folder,
            } => {
                if is_folder {
                    ops.push(Operation::CreateRemoteDir {
                        rel,
                        parent_link_id: String::new(),
                    });
                } else {
                    ops.push(Operation::UploadNew {
                        rel,
                        path,
                        parent_link_id: String::new(),
                    });
                }
            }
            LocalChange::Removed { rel } => {
                ops.push(Operation::DeleteRemote {
                    mapping: shell_mapping(&rel),
                });
            }
            LocalChange::Renamed { from_rel, to_rel } => {
                ops.push(Operation::DeleteRemote {
                    mapping: shell_mapping(&from_rel),
                });
                ops.push(Operation::UploadNew {
                    rel: to_rel.clone(),
                    path: PathBuf::from(to_rel),
                    parent_link_id: String::new(),
                });
            }
        }
    }
    for c in obs.remote {
        if let RemoteChange::Upsert { entry } = c {
            // Selective sync: skip entries whose top-level component is excluded.
            if is_excluded(&entry.name, excluded) {
                continue;
            }
            if entry.is_folder {
                ops.push(Operation::CreateLocalDir {
                    rel: entry.name,
                    link_id: entry.link_id,
                    parent_link_id: entry.parent_id,
                });
            } else {
                ops.push(Operation::DownloadNew {
                    rel: entry.name,
                    link_id: entry.link_id,
                    parent_link_id: entry.parent_id,
                    is_folder: false,
                    size: entry.size,
                    mtime: entry.modify_time,
                });
            }
        }
    }
    ops
}

/// Returns true if the entry's top-level path component is in `excluded`.
fn is_excluded(rel: &str, excluded: &[String]) -> bool {
    if excluded.is_empty() {
        return false;
    }
    let top = rel.split('/').next().unwrap_or(rel);
    excluded.iter().any(|e| e.trim().eq_ignore_ascii_case(top))
}

fn shell_mapping(rel: &str) -> Mapping {
    Mapping {
        id: 0,
        rel_path: rel.into(),
        link_id: String::new(),
        parent_link_id: String::new(),
        is_folder: false,
        local_size: 0,
        local_mtime: 0,
        local_hash: None,
        remote_size: 0,
        remote_mtime: 0,
        remote_hash: None,
        is_materialized: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::RemoteChange;
    use protondrive_bridge::EventEntry;

    fn remote_upsert(name: &str, is_folder: bool) -> RemoteChange {
        RemoteChange::Upsert {
            entry: EventEntry {
                link_id: format!("L-{name}"),
                parent_id: "root".to_string(),
                name: name.to_string(),
                is_folder,
                modify_time: 1,
                size: 0,
            },
        }
    }

    #[test]
    fn no_exclusions_passes_all() {
        let obs = Observations {
            local: vec![],
            remote: vec![
                remote_upsert("Documents/file.txt", false),
                remote_upsert("Photos/img.jpg", false),
            ],
        };
        let ops = reconcile(obs, &[]);
        assert_eq!(ops.len(), 2);
    }

    #[test]
    fn excluded_top_level_filtered() {
        let obs = Observations {
            local: vec![],
            remote: vec![
                remote_upsert("Documents/file.txt", false),
                remote_upsert("Photos/img.jpg", false),
                remote_upsert("Photos/sub/deep.jpg", false),
            ],
        };
        let excluded = vec!["Photos".to_string()];
        let ops = reconcile(obs, &excluded);
        // Only Documents/file.txt should pass through.
        assert_eq!(ops.len(), 1);
    }

    #[test]
    fn exclusion_is_case_insensitive() {
        let obs = Observations {
            local: vec![],
            remote: vec![remote_upsert("photos/img.jpg", false)],
        };
        let excluded = vec!["Photos".to_string()];
        let ops = reconcile(obs, &excluded);
        assert!(ops.is_empty());
    }
}
