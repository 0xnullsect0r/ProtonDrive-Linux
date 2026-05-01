//! SQLite-backed 3-way state store.
//!
//! Tracks a mapping between local filesystem entries (under the sync
//! root) and remote Proton Drive links, including the last seen
//! fingerprint on each side. The reconciliation stage diffs the
//! observed state against what's recorded here.

use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StateError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

#[derive(Debug, Clone)]
pub struct Mapping {
    pub id: i64,
    pub rel_path: String,
    pub link_id: String,
    pub parent_link_id: String,
    pub is_folder: bool,
    pub local_size: i64,
    pub local_mtime: i64,
    pub local_hash: Option<String>,
    pub remote_size: i64,
    pub remote_mtime: i64,
    pub remote_hash: Option<String>,
    /// True if the file content is present on local disk; false for
    /// virtual placeholder entries served by the FUSE layer.
    pub is_materialized: bool,
}

pub struct State {
    pub conn: Connection,
}

impl State {
    pub fn open(path: &Path) -> Result<Self, StateError> {
        if let Some(p) = path.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        run_migrations(&conn)?;
        Ok(Self { conn })
    }

    pub fn open_in_memory() -> Result<Self, StateError> {
        let conn = Connection::open_in_memory()?;
        run_migrations(&conn)?;
        Ok(Self { conn })
    }

    pub fn upsert_mapping(&self, m: &Mapping) -> Result<i64, StateError> {
        self.conn.execute(
            "INSERT INTO mappings(rel_path, link_id, parent_link_id, is_folder,
                local_size, local_mtime, local_hash,
                remote_size, remote_mtime, remote_hash, is_materialized)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
             ON CONFLICT(rel_path) DO UPDATE SET
                link_id=excluded.link_id,
                parent_link_id=excluded.parent_link_id,
                is_folder=excluded.is_folder,
                local_size=excluded.local_size,
                local_mtime=excluded.local_mtime,
                local_hash=excluded.local_hash,
                remote_size=excluded.remote_size,
                remote_mtime=excluded.remote_mtime,
                remote_hash=excluded.remote_hash,
                is_materialized=excluded.is_materialized",
            params![
                m.rel_path,
                m.link_id,
                m.parent_link_id,
                m.is_folder as i32,
                m.local_size,
                m.local_mtime,
                m.local_hash,
                m.remote_size,
                m.remote_mtime,
                m.remote_hash,
                m.is_materialized as i32,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Upsert many mappings inside a single transaction.
    ///
    /// For the initial scan (tens of thousands of entries) this is
    /// dramatically faster than individual auto-commit inserts.
    pub fn upsert_mappings_batch(&self, mappings: &[Mapping]) -> Result<(), StateError> {
        if mappings.is_empty() {
            return Ok(());
        }
        self.conn.execute_batch("BEGIN;")?;
        let result = (|| {
            let mut stmt = self.conn.prepare_cached(
                "INSERT INTO mappings(rel_path, link_id, parent_link_id, is_folder,
                    local_size, local_mtime, local_hash,
                    remote_size, remote_mtime, remote_hash, is_materialized)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
                 ON CONFLICT(rel_path) DO UPDATE SET
                    link_id=excluded.link_id,
                    parent_link_id=excluded.parent_link_id,
                    is_folder=excluded.is_folder,
                    local_size=excluded.local_size,
                    local_mtime=excluded.local_mtime,
                    local_hash=excluded.local_hash,
                    remote_size=excluded.remote_size,
                    remote_mtime=excluded.remote_mtime,
                    remote_hash=excluded.remote_hash,
                    is_materialized=excluded.is_materialized",
            )?;
            for m in mappings {
                stmt.execute(params![
                    m.rel_path,
                    m.link_id,
                    m.parent_link_id,
                    m.is_folder as i32,
                    m.local_size,
                    m.local_mtime,
                    m.local_hash,
                    m.remote_size,
                    m.remote_mtime,
                    m.remote_hash,
                    m.is_materialized as i32,
                ])?;
            }
            Ok::<(), StateError>(())
        })();
        match result {
            Ok(()) => {
                self.conn.execute_batch("COMMIT;")?;
                Ok(())
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK;");
                Err(e)
            }
        }
    }


    pub fn set_materialized(&self, link_id: &str, value: bool) -> Result<(), StateError> {
        self.conn.execute(
            "UPDATE mappings SET is_materialized=?1 WHERE link_id=?2",
            params![value as i32, link_id],
        )?;
        Ok(())
    }

    /// List all direct children of a parent directory.
    pub fn list_children_by_parent(
        &self,
        parent_link_id: &str,
    ) -> Result<Vec<Mapping>, StateError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, rel_path, link_id, parent_link_id, is_folder,
                    local_size, local_mtime, local_hash,
                    remote_size, remote_mtime, remote_hash, is_materialized
               FROM mappings WHERE parent_link_id = ?1",
        )?;
        let rows = stmt.query_map(params![parent_link_id], row_to_mapping)?;
        rows.collect::<Result<_, _>>().map_err(StateError::from)
    }

    /// List every mapping in the database (used to pre-populate the inode
    /// table on FUSE mount).
    pub fn list_all(&self) -> Result<Vec<Mapping>, StateError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, rel_path, link_id, parent_link_id, is_folder,
                    local_size, local_mtime, local_hash,
                    remote_size, remote_mtime, remote_hash, is_materialized
               FROM mappings",
        )?;
        let rows = stmt.query_map([], row_to_mapping)?;
        rows.collect::<Result<_, _>>().map_err(StateError::from)
    }

    pub fn delete_by_rel(&self, rel: &str) -> Result<(), StateError> {
        self.conn
            .execute("DELETE FROM mappings WHERE rel_path = ?1", params![rel])?;
        Ok(())
    }

    pub fn get_by_rel(&self, rel: &str) -> Result<Option<Mapping>, StateError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, rel_path, link_id, parent_link_id, is_folder,
                            local_size, local_mtime, local_hash,
                            remote_size, remote_mtime, remote_hash, is_materialized
                       FROM mappings WHERE rel_path = ?1",
        )?;
        let mut rows = stmt.query(params![rel])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_mapping(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn get_by_link(&self, link_id: &str) -> Result<Option<Mapping>, StateError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, rel_path, link_id, parent_link_id, is_folder,
                    local_size, local_mtime, local_hash,
                    remote_size, remote_mtime, remote_hash, is_materialized
               FROM mappings WHERE link_id = ?1",
        )?;
        let mut rows = stmt.query(params![link_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_mapping(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn list_under(&self, prefix: &str) -> Result<Vec<Mapping>, StateError> {
        let pat = format!("{}%", prefix);
        let mut stmt = self.conn.prepare(
            "SELECT id, rel_path, link_id, parent_link_id, is_folder,
                    local_size, local_mtime, local_hash,
                    remote_size, remote_mtime, remote_hash, is_materialized
               FROM mappings WHERE rel_path LIKE ?1",
        )?;
        let rows = stmt.query_map(params![pat], row_to_mapping)?;
        rows.collect::<Result<_, _>>().map_err(StateError::from)
    }

    pub fn cursor(&self) -> Result<i64, StateError> {
        let v: i64 = self
            .conn
            .query_row(
                "SELECT value FROM events_cursor WHERE key='last' LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        Ok(v)
    }

    pub fn set_cursor(&self, v: i64) -> Result<(), StateError> {
        self.conn.execute(
            "INSERT INTO events_cursor(key,value) VALUES('last',?1)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![v],
        )?;
        Ok(())
    }
}

fn row_to_mapping(row: &rusqlite::Row<'_>) -> Result<Mapping, rusqlite::Error> {
    Ok(Mapping {
        id: row.get(0)?,
        rel_path: row.get(1)?,
        link_id: row.get(2)?,
        parent_link_id: row.get(3)?,
        is_folder: row.get::<_, i32>(4)? != 0,
        local_size: row.get(5)?,
        local_mtime: row.get(6)?,
        local_hash: row.get(7)?,
        remote_size: row.get(8)?,
        remote_mtime: row.get(9)?,
        remote_hash: row.get(10)?,
        is_materialized: row.get::<_, i32>(11)? != 0,
    })
}

/// Run all outstanding schema migrations.
///
/// Each entry in `MIGRATIONS` corresponds to a schema version (1-based).
/// `PRAGMA user_version` tracks the last applied version.  On a fresh DB
/// the version is 0, so every migration runs in order.
fn run_migrations(conn: &Connection) -> Result<(), StateError> {
    let current: i32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap_or(0);
    for (i, sql) in MIGRATIONS.iter().enumerate() {
        let target = (i + 1) as i32;
        if current < target {
            conn.execute_batch(sql)?;
            conn.execute_batch(&format!("PRAGMA user_version={target};"))?;
        }
    }
    Ok(())
}

/// Each entry brings the schema from version (index) to version (index+1).
const MIGRATIONS: &[&str] = &[
    // v0 → v1: initial schema
    r#"
CREATE TABLE IF NOT EXISTS mappings (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    rel_path        TEXT    NOT NULL UNIQUE,
    link_id         TEXT    NOT NULL,
    parent_link_id  TEXT    NOT NULL,
    is_folder       INTEGER NOT NULL,
    local_size      INTEGER NOT NULL DEFAULT 0,
    local_mtime     INTEGER NOT NULL DEFAULT 0,
    local_hash      TEXT,
    remote_size     INTEGER NOT NULL DEFAULT 0,
    remote_mtime    INTEGER NOT NULL DEFAULT 0,
    remote_hash     TEXT
);
CREATE INDEX IF NOT EXISTS idx_mappings_link ON mappings(link_id);
CREATE INDEX IF NOT EXISTS idx_mappings_parent ON mappings(parent_link_id);
CREATE TABLE IF NOT EXISTS events_cursor (
    key   TEXT PRIMARY KEY,
    value INTEGER NOT NULL
);
"#,
    // v1 → v2: add is_materialized column for FUSE virtual placeholders
    r#"
ALTER TABLE mappings ADD COLUMN is_materialized INTEGER NOT NULL DEFAULT 0;
"#,
];

pub fn default_state_path() -> PathBuf {
    directories::ProjectDirs::from("me", "Proton", "ProtonDrive-Linux")
        .map(|d| d.data_dir().join("state.sqlite"))
        .unwrap_or_else(|| PathBuf::from(".protondrive-state.sqlite"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_and_get() {
        let s = State::open_in_memory().unwrap();
        let m = Mapping {
            id: 0,
            rel_path: "a/b.txt".into(),
            link_id: "L1".into(),
            parent_link_id: "P1".into(),
            is_folder: false,
            local_size: 10,
            local_mtime: 1,
            local_hash: Some("h".into()),
            remote_size: 10,
            remote_mtime: 1,
            remote_hash: Some("h".into()),
            is_materialized: false,
        };
        s.upsert_mapping(&m).unwrap();
        let got = s.get_by_rel("a/b.txt").unwrap().unwrap();
        assert_eq!(got.link_id, "L1");
    }

    #[test]
    fn cursor_round_trips() {
        let s = State::open_in_memory().unwrap();
        assert_eq!(s.cursor().unwrap(), 0);
        s.set_cursor(42).unwrap();
        assert_eq!(s.cursor().unwrap(), 42);
    }

    #[test]
    fn migration_sets_user_version() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        let v: i32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, MIGRATIONS.len() as i32);
    }

    #[test]
    fn migration_is_idempotent() {
        // Running migrations twice should not panic or corrupt data.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        run_migrations(&conn).unwrap();
        let v: i32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, MIGRATIONS.len() as i32);
    }
}
