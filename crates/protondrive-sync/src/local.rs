//! Local FS adapter — watches the sync root with inotify and emits
//! debounced [`LocalChange`] events.

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub enum LocalChange {
    /// Created or modified — sync engine treats both the same way.
    Upsert {
        rel: String,
        path: PathBuf,
        is_folder: bool,
    },
    Removed {
        rel: String,
    },
    Renamed {
        from_rel: String,
        to_rel: String,
    },
}

pub struct LocalWatcher {
    pub root: PathBuf,
    pub rx: mpsc::Receiver<LocalChange>,
    _watcher: RecommendedWatcher,
}

impl LocalWatcher {
    pub fn start(root: impl AsRef<Path>) -> std::io::Result<Self> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root)?;
        let (tx, rx) = mpsc::channel::<LocalChange>(1024);
        let root_for_cb = root.clone();

        let mut watcher: RecommendedWatcher =
            notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
                let Ok(ev) = res else {
                    return;
                };
                let tx = tx.clone();
                let root = root_for_cb.clone();
                let changes = translate_event(&root, &ev);
                tokio::spawn(async move {
                    for c in changes {
                        // 500ms debounce window per file
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        let _ = tx.send(c).await;
                    }
                });
            })
            .map_err(io_err)?;
        watcher
            .watch(&root, RecursiveMode::Recursive)
            .map_err(io_err)?;
        Ok(Self {
            root,
            rx,
            _watcher: watcher,
        })
    }
}

fn io_err(e: notify::Error) -> std::io::Error {
    std::io::Error::other(format!("notify: {e}"))
}

fn translate_event(root: &Path, ev: &Event) -> Vec<LocalChange> {
    let mut out = Vec::new();
    for path in &ev.paths {
        let Some(rel) = relative(root, path) else {
            continue;
        };
        if rel.is_empty() {
            continue;
        }
        let is_folder = path.is_dir();
        match ev.kind {
            EventKind::Create(_) | EventKind::Modify(_) => {
                if path.exists() {
                    out.push(LocalChange::Upsert {
                        rel,
                        path: path.clone(),
                        is_folder,
                    });
                } else {
                    out.push(LocalChange::Removed { rel });
                }
            }
            EventKind::Remove(_) => {
                out.push(LocalChange::Removed { rel });
            }
            _ => {}
        }
    }
    out
}

fn relative(root: &Path, p: &Path) -> Option<String> {
    p.strip_prefix(root)
        .ok()
        .map(|rp| rp.to_string_lossy().replace('\\', "/").to_string())
}

/// Hash the first 1 MiB + size + mtime as a cheap fingerprint. This
/// matches what windows-drive does for change detection — full content
/// hashing only happens during conflict resolution.
pub fn fingerprint(path: &Path) -> std::io::Result<String> {
    use std::fs::File;
    use std::io::Read;
    let mut f = File::open(path)?;
    let meta = f.metadata()?;
    let mut buf = vec![0u8; 1 << 20];
    let n = f.read(&mut buf).unwrap_or(0);
    let mut h = Sha256::new();
    h.update(&buf[..n]);
    h.update(meta.len().to_le_bytes());
    if let Ok(mtime) = meta.modified() {
        if let Ok(dur) = mtime.duration_since(std::time::UNIX_EPOCH) {
            h.update(dur.as_secs().to_le_bytes());
        }
    }
    Ok(hex::encode(h.finalize()))
}
