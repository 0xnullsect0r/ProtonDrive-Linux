//! SQLite-backed metadata cache.
//!
//! Schema is intentionally tiny: one table for nodes, one for known shares,
//! one key/value table for things like the most recent events cursor.
//!
//! Listings (`readdir`, `getattr`) are served entirely from this DB so the
//! FUSE layer doesn't make a network call on every `ls`.

use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::Arc;

use crate::types::{Node, NodeId, NodeKind, RevisionId, ShareId};
use crate::Result;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS kv (
    k TEXT PRIMARY KEY,
    v TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS shares (
    id           TEXT PRIMARY KEY,
    name         TEXT,
    root_link_id TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS nodes (
    id              TEXT PRIMARY KEY,
    parent_id       TEXT,
    share_id        TEXT NOT NULL,
    kind            TEXT NOT NULL CHECK (kind IN ('folder','file')),
    name            TEXT NOT NULL,
    size            INTEGER NOT NULL DEFAULT 0,
    mtime           INTEGER NOT NULL DEFAULT 0,
    mime_type       TEXT,
    active_revision TEXT,
    pinned          INTEGER NOT NULL DEFAULT 0,
    -- Encrypted blobs we keep so we can re-decrypt without another round trip.
    enc_node_key            TEXT,
    enc_node_passphrase     TEXT,
    enc_node_pass_signature TEXT,
    enc_name                TEXT
);

CREATE INDEX IF NOT EXISTS nodes_parent ON nodes(parent_id);
CREATE INDEX IF NOT EXISTS nodes_pinned ON nodes(pinned) WHERE pinned = 1;
"#;

#[derive(Clone)]
pub struct MetadataDb {
    conn: Arc<Mutex<Connection>>,
}

impl MetadataDb {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    // --- kv helpers -------------------------------------------------------

    pub fn get_kv(&self, key: &str) -> Result<Option<String>> {
        let conn = self.conn.lock();
        Ok(conn
            .query_row("SELECT v FROM kv WHERE k = ?1", params![key], |r| r.get(0))
            .optional()?)
    }

    pub fn put_kv(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO kv(k,v) VALUES(?1,?2) ON CONFLICT(k) DO UPDATE SET v=excluded.v",
            params![key, value],
        )?;
        Ok(())
    }

    // --- nodes ------------------------------------------------------------

    pub fn upsert_node(&self, n: &Node) -> Result<()> {
        let kind = match n.kind {
            NodeKind::Folder => "folder",
            NodeKind::File => "file",
        };
        let conn = self.conn.lock();
        conn.execute(
            r#"INSERT INTO nodes
                (id, parent_id, share_id, kind, name, size, mtime, mime_type, active_revision, pinned)
               VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
               ON CONFLICT(id) DO UPDATE SET
                   parent_id       = excluded.parent_id,
                   share_id        = excluded.share_id,
                   kind            = excluded.kind,
                   name            = excluded.name,
                   size            = excluded.size,
                   mtime           = excluded.mtime,
                   mime_type       = excluded.mime_type,
                   active_revision = excluded.active_revision"#,
            params![
                n.id.0, n.parent.as_ref().map(|p| &p.0), n.share.0,
                kind, n.name, n.size, n.mtime, n.mime_type,
                n.active_revision.as_ref().map(|r| &r.0),
                n.pinned as i64,
            ],
        )?;
        Ok(())
    }

    pub fn delete_node(&self, id: &NodeId) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute("DELETE FROM nodes WHERE id = ?1", params![id.0])?;
        Ok(())
    }

    pub fn get_node(&self, id: &NodeId) -> Result<Option<Node>> {
        let conn = self.conn.lock();
        let row = conn.query_row(
            "SELECT id, parent_id, share_id, kind, name, size, mtime, mime_type, active_revision, pinned \
             FROM nodes WHERE id = ?1",
            params![id.0],
            row_to_node,
        ).optional()?;
        Ok(row)
    }

    pub fn list_children(&self, parent: &NodeId) -> Result<Vec<Node>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare_cached(
            "SELECT id, parent_id, share_id, kind, name, size, mtime, mime_type, active_revision, pinned \
             FROM nodes WHERE parent_id = ?1 ORDER BY name",
        )?;
        let rows = stmt.query_map(params![parent.0], row_to_node)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn set_pinned(&self, id: &NodeId, pinned: bool) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE nodes SET pinned = ?1 WHERE id = ?2",
            params![pinned as i64, id.0],
        )?;
        Ok(())
    }

    pub fn pinned_nodes(&self) -> Result<Vec<Node>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare_cached(
            "SELECT id, parent_id, share_id, kind, name, size, mtime, mime_type, active_revision, pinned \
             FROM nodes WHERE pinned = 1",
        )?;
        let rows = stmt.query_map([], row_to_node)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
}

fn row_to_node(r: &rusqlite::Row<'_>) -> rusqlite::Result<Node> {
    let kind: String = r.get(3)?;
    Ok(Node {
        id: NodeId(r.get(0)?),
        parent: r.get::<_, Option<String>>(1)?.map(NodeId),
        share: ShareId(r.get(2)?),
        kind: if kind == "folder" {
            NodeKind::Folder
        } else {
            NodeKind::File
        },
        name: r.get(4)?,
        size: r.get::<_, i64>(5)? as u64,
        mtime: r.get(6)?,
        mime_type: r.get(7)?,
        active_revision: r.get::<_, Option<String>>(8)?.map(RevisionId),
        pinned: r.get::<_, i64>(9)? != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_node(id: &str, parent: Option<&str>, name: &str) -> Node {
        Node {
            id: NodeId(id.into()),
            parent: parent.map(|p| NodeId(p.into())),
            share: ShareId("share-1".into()),
            kind: NodeKind::File,
            name: name.into(),
            size: 42,
            mtime: 0,
            mime_type: None,
            active_revision: None,
            pinned: false,
        }
    }

    #[test]
    fn upsert_and_list() {
        let db = MetadataDb::open_in_memory().unwrap();
        db.upsert_node(&make_node("root", None, "root")).unwrap();
        let mut child = make_node("c1", Some("root"), "alpha.txt");
        child.kind = NodeKind::File;
        db.upsert_node(&child).unwrap();
        let kids = db.list_children(&NodeId("root".into())).unwrap();
        assert_eq!(kids.len(), 1);
        assert_eq!(kids[0].name, "alpha.txt");
    }

    #[test]
    fn pinning_roundtrip() {
        let db = MetadataDb::open_in_memory().unwrap();
        db.upsert_node(&make_node("c1", None, "a")).unwrap();
        assert!(db.pinned_nodes().unwrap().is_empty());
        db.set_pinned(&NodeId("c1".into()), true).unwrap();
        let pinned = db.pinned_nodes().unwrap();
        assert_eq!(pinned.len(), 1);
        assert_eq!(pinned[0].id.0, "c1");
    }

    #[test]
    fn kv_roundtrip() {
        let db = MetadataDb::open_in_memory().unwrap();
        assert_eq!(db.get_kv("cursor").unwrap(), None);
        db.put_kv("cursor", "evt-99").unwrap();
        db.put_kv("cursor", "evt-100").unwrap();
        assert_eq!(db.get_kv("cursor").unwrap().as_deref(), Some("evt-100"));
    }
}
