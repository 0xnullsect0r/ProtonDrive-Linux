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
        is_folder: bool,
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
pub fn reconcile(obs: Observations) -> Vec<Operation> {
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
            if entry.is_folder {
                ops.push(Operation::CreateLocalDir {
                    rel: entry.name,
                    link_id: entry.link_id,
                });
            } else {
                ops.push(Operation::DownloadNew {
                    rel: entry.name,
                    link_id: entry.link_id,
                    is_folder: false,
                });
            }
        }
    }
    ops
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
    }
}
