//! Content-addressed on-disk blob cache.
//!
//! Blobs are keyed by SHA-256 of their **encrypted** content (which is what
//! Proton's API serves) so that two files sharing a block share storage and
//! identity is independent of decryption keys.
//!
//! Layout:
//! ```text
//! $XDG_CACHE_HOME/protondrive/blocks/
//!     ab/cdef0123…  ← file path is `<first-2-hex>/<rest-of-hex>`
//! ```
//!
//! Writes are atomic (`<final>.tmp.<rand>` + `rename`).
//!
//! Eviction is LRU by access time, capped by `max_bytes` from config. Pinned
//! blocks are tracked separately and never evicted.

use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::Result;

pub struct BlobCache {
    root: PathBuf,
    max_bytes: u64,
    pinned: Mutex<HashSet<String>>,
}

impl BlobCache {
    pub fn new(root: impl Into<PathBuf>, max_bytes: u64) -> Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(&root)?;
        Ok(Self {
            root,
            max_bytes,
            pinned: Mutex::new(HashSet::new()),
        })
    }

    pub fn key_for(content: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(content);
        hex::encode(h.finalize())
    }

    fn path_for(&self, key: &str) -> PathBuf {
        let (a, b) = key.split_at(2);
        self.root.join(a).join(b)
    }

    pub fn contains(&self, key: &str) -> bool {
        self.path_for(key).exists()
    }

    pub fn read(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let p = self.path_for(key);
        if !p.exists() {
            return Ok(None);
        }
        // Touch atime so LRU eviction works.
        let _ = std::fs::OpenOptions::new().read(true).open(&p)?;
        Ok(Some(std::fs::read(&p)?))
    }

    pub fn write(&self, key: &str, bytes: &[u8]) -> Result<()> {
        let final_path = self.path_for(key);
        if let Some(parent) = final_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = final_path.with_extension(format!("tmp.{}", rand::random::<u32>()));
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, &final_path)?;
        Ok(())
    }

    pub fn pin(&self, key: &str) {
        self.pinned.lock().insert(key.to_string());
    }
    pub fn unpin(&self, key: &str) {
        self.pinned.lock().remove(key);
    }

    /// Evict least-recently-used unpinned blocks until total size <= max_bytes.
    pub fn evict_if_needed(&self) -> Result<u64> {
        let mut entries: Vec<(PathBuf, std::time::SystemTime, u64, String)> = Vec::new();
        for entry in walkdir::WalkDir::new(&self.root)
            .min_depth(2)
            .into_iter()
            .flatten()
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            let atime = meta.accessed().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            let key = derive_key_from_path(entry.path(), &self.root);
            entries.push((entry.path().to_path_buf(), atime, meta.len(), key));
        }
        let total: u64 = entries.iter().map(|(_, _, sz, _)| *sz).sum();
        if total <= self.max_bytes {
            return Ok(0);
        }

        // Oldest first.
        entries.sort_by_key(|e| e.1);

        let pinned = self.pinned.lock();
        let mut freed = 0u64;
        let mut current = total;
        for (path, _atime, sz, key) in entries {
            if current <= self.max_bytes {
                break;
            }
            if pinned.contains(&key) {
                continue;
            }
            if std::fs::remove_file(&path).is_ok() {
                current = current.saturating_sub(sz);
                freed += sz;
            }
        }
        Ok(freed)
    }
}

fn derive_key_from_path(p: &Path, root: &Path) -> String {
    let rel = p.strip_prefix(root).unwrap_or(p);
    rel.components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect::<Vec<_>>()
        .join("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn roundtrip_and_evict() {
        let dir = tempdir();
        let cache = BlobCache::new(dir.path(), 100).unwrap();
        let small = vec![1u8; 30];
        let key_s = BlobCache::key_for(&small);
        cache.write(&key_s, &small).unwrap();
        assert!(cache.contains(&key_s));
        assert_eq!(
            cache.read(&key_s).unwrap().as_deref(),
            Some(small.as_slice())
        );

        // Push past the cap; eviction should happen.
        let big = vec![2u8; 200];
        let key_b = BlobCache::key_for(&big);
        cache.write(&key_b, &big).unwrap();
        let freed = cache.evict_if_needed().unwrap();
        assert!(freed > 0);
    }

    #[test]
    fn pinned_not_evicted() {
        let dir = tempdir();
        let cache = BlobCache::new(dir.path(), 10).unwrap();
        let small = vec![1u8; 50];
        let key = BlobCache::key_for(&small);
        cache.write(&key, &small).unwrap();
        cache.pin(&key);
        cache.evict_if_needed().unwrap();
        assert!(cache.contains(&key), "pinned blob should survive eviction");
    }

    fn tempdir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    // Bring tempfile in only for tests via dev-dependency — but to keep the
    // crate's dev-deps clean we use a tiny local impl when tempfile is absent.
    // (tempfile is brought in via dev-dependencies; see Cargo.toml.)

    // Force `Write` import to satisfy MSRV in older toolchains.
    #[allow(dead_code)]
    fn _w<W: Write>(_w: W) {}
}
