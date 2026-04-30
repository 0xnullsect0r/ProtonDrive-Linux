//! Shared types used across the core crate, FUSE layer, and UI.
//!
//! These mirror the conceptual model of Proton Drive:
//!
//! - A user has one or more **shares** (root containers).
//! - Each share contains a tree of **links** (folder or file nodes).
//! - File nodes have one or more **revisions**; each revision is composed of
//!   one or more encrypted **blocks** stored on the Proton block servers.

use serde::{Deserialize, Serialize};

/// Opaque identifier for a Proton Drive node (folder or file).
///
/// In the wire protocol this is the `LinkID` string returned by the API.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeId(pub String);

impl NodeId {
    pub fn as_str(&self) -> &str { &self.0 }
}

/// Opaque identifier for a Proton Drive share (the root container of a tree).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ShareId(pub String);

/// Opaque identifier for a file revision.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RevisionId(pub String);

/// Opaque identifier for an encrypted block stored on Proton's CDN.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BlockId(pub String);

/// Folder vs. file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeKind {
    Folder,
    File,
}

/// Decrypted, in-memory representation of a node (folder or file).
///
/// The encrypted equivalents (encrypted name, encrypted node key, etc.) are
/// kept in the SQLite cache so we can re-decrypt without another round trip.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub parent: Option<NodeId>,
    pub share: ShareId,
    pub kind: NodeKind,
    pub name: String,
    pub size: u64,
    /// Unix mtime in seconds.
    pub mtime: i64,
    pub mime_type: Option<String>,
    /// Most recent revision id (files only).
    pub active_revision: Option<RevisionId>,
    /// True if the user pinned this node for "always available offline".
    pub pinned: bool,
}
