//! FUSE virtual filesystem for ProtonDrive.
//!
//! Files appear in the sync folder with correct metadata but are not
//! physically present on disk.  On first read, the file is downloaded to
//! `~/.cache/protondrive/files/<link_id>` and served from there.
//!
//! The filesystem is read-only: all write operations return `EROFS`.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fuser::{
    BackgroundSession, FileAttr, FileType, Filesystem, MountOption, ReplyAttr, ReplyCreate,
    ReplyData, ReplyDirectory, ReplyEmpty, ReplyEntry, ReplyOpen, ReplyWrite, Request,
};
use libc::ENOENT;
use parking_lot::Mutex as PLMutex;
use protondrive_bridge::Bridge;
use protondrive_sync::state::State;
use tokio::runtime::Handle;

// Directory entry TTL — 1 second keeps things reasonably live without
// hammering SQLite.
const TTL: Duration = Duration::from_secs(1);

// ── Inode table ──────────────────────────────────────────────────────────────

/// Bidirectional mapping: inode ↔ link_id.
///
/// Inode 1 is always the root (mapped to `root_link_id`).
/// All other inodes are assigned sequentially from 2.
struct InodeTable {
    next: AtomicU64,
    ino_to_link: RwLock<HashMap<u64, String>>,
    link_to_ino: RwLock<HashMap<String, u64>>,
}

impl InodeTable {
    fn new(root_link_id: String) -> Self {
        let mut ino_to_link = HashMap::new();
        let mut link_to_ino = HashMap::new();
        ino_to_link.insert(1u64, root_link_id.clone());
        link_to_ino.insert(root_link_id, 1u64);
        Self {
            next: AtomicU64::new(2),
            ino_to_link: RwLock::new(ino_to_link),
            link_to_ino: RwLock::new(link_to_ino),
        }
    }

    /// Return the inode for `link_id`, assigning a new one if needed.
    fn get_or_assign(&self, link_id: &str) -> u64 {
        {
            if let Ok(r) = self.link_to_ino.read() {
                if let Some(&ino) = r.get(link_id) {
                    return ino;
                }
            }
        }
        let ino = self.next.fetch_add(1, Ordering::SeqCst);
        if let (Ok(mut il), Ok(mut li)) = (self.ino_to_link.write(), self.link_to_ino.write()) {
            il.insert(ino, link_id.to_string());
            li.insert(link_id.to_string(), ino);
        }
        ino
    }

    fn link_id(&self, ino: u64) -> Option<String> {
        self.ino_to_link.read().ok()?.get(&ino).cloned()
    }
}

// ── Main VFS struct ───────────────────────────────────────────────────────────

pub struct ProtonVfs {
    state: Arc<PLMutex<State>>,
    bridge: Bridge,
    cache_dir: PathBuf,
    root_link_id: String,
    uid: u32,
    gid: u32,
    inodes: InodeTable,
    rt: Handle,
    /// Per-link download locks to prevent duplicate concurrent downloads.
    dl_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl ProtonVfs {
    pub fn new(
        state: Arc<PLMutex<State>>,
        bridge: Bridge,
        cache_dir: PathBuf,
        root_link_id: String,
        rt: Handle,
    ) -> Self {
        let _ = std::fs::create_dir_all(&cache_dir);

        // SAFETY: getuid/getgid are always safe to call.
        let uid = unsafe { libc::getuid() };
        let gid = unsafe { libc::getgid() };

        let inodes = InodeTable::new(root_link_id.clone());

        // Pre-populate inodes from any mappings already in state.
        if let Ok(all) = state.lock().list_all() {
            for m in all {
                inodes.get_or_assign(&m.link_id);
            }
        }

        Self {
            state,
            bridge,
            cache_dir,
            root_link_id,
            uid,
            gid,
            inodes,
            rt,
            dl_locks: Mutex::new(HashMap::new()),
        }
    }

    // ── Attribute helpers ─────────────────────────────────────────────────────

    fn mapping_to_attr(&self, m: &protondrive_sync::state::Mapping, ino: u64) -> FileAttr {
        let size = m.remote_size.max(0) as u64;
        let ts = UNIX_EPOCH + Duration::from_secs(m.remote_mtime.max(0) as u64);
        let (kind, perm, nlink) = if m.is_folder {
            (FileType::Directory, 0o755u16, 2u32)
        } else {
            (FileType::RegularFile, 0o644u16, 1u32)
        };
        FileAttr {
            ino,
            size,
            blocks: (size + 511) / 512,
            atime: ts,
            mtime: ts,
            ctime: ts,
            crtime: ts,
            kind,
            perm,
            nlink,
            uid: self.uid,
            gid: self.gid,
            rdev: 0,
            blksize: 4096,
            flags: 0,
        }
    }

    fn root_attr(&self) -> FileAttr {
        let now = SystemTime::now();
        FileAttr {
            ino: 1,
            size: 0,
            blocks: 0,
            atime: now,
            mtime: now,
            ctime: now,
            crtime: now,
            kind: FileType::Directory,
            perm: 0o755,
            nlink: 2,
            uid: self.uid,
            gid: self.gid,
            rdev: 0,
            blksize: 4096,
            flags: 0,
        }
    }

    // ── Download-on-demand ────────────────────────────────────────────────────

    /// Ensure the file for `link_id` is present in the cache directory,
    /// downloading it from Proton Drive if not.  Returns the cache path.
    fn ensure_cached(&self, link_id: &str) -> std::io::Result<PathBuf> {
        let cache_path = self.cache_dir.join(link_id);
        if cache_path.exists() {
            return Ok(cache_path);
        }

        // Acquire per-link lock to prevent racing downloads of the same file.
        let lock = {
            let mut locks = self.dl_locks.lock().unwrap();
            locks
                .entry(link_id.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _guard = lock.lock().unwrap();

        // Double-check after acquiring the lock.
        if cache_path.exists() {
            return Ok(cache_path);
        }

        tracing::info!(link_id, "FUSE: downloading file on demand");

        let tmp_path = self.cache_dir.join(format!(".tmp.{link_id}"));
        let bridge = self.bridge.clone();
        let id = link_id.to_owned();
        let tmp2 = tmp_path.clone();

        let result = self
            .rt
            .block_on(async move { bridge.download(&id, &tmp2).await });

        match result {
            Ok(_) => {
                std::fs::rename(&tmp_path, &cache_path).map_err(|e| {
                    let _ = std::fs::remove_file(&tmp_path);
                    e
                })?;
                // Mark as materialized in state.
                if let Ok(state) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    self.state.lock().set_materialized(link_id, true)
                })) {
                    let _ = state;
                }
                Ok(cache_path)
            }
            Err(e) => {
                let _ = std::fs::remove_file(&tmp_path);
                Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    e.to_string(),
                ))
            }
        }
    }

    // ── Internal lookup helpers ───────────────────────────────────────────────

    fn parent_link_for_ino(&self, ino: u64) -> Option<String> {
        if ino == 1 {
            Some(self.root_link_id.clone())
        } else {
            self.inodes.link_id(ino)
        }
    }
}

// ── Filesystem trait impl ────────────────────────────────────────────────────

impl Filesystem for ProtonVfs {
    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let name_str = match name.to_str() {
            Some(s) => s,
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        let parent_link_id = match self.parent_link_for_ino(parent) {
            Some(l) => l,
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        match self.state.lock().list_children_by_parent(&parent_link_id) {
            Ok(children) => {
                for m in &children {
                    let leaf = m.rel_path.rsplit('/').next().unwrap_or(&m.rel_path);
                    if leaf == name_str {
                        let ino = self.inodes.get_or_assign(&m.link_id);
                        reply.entry(&TTL, &self.mapping_to_attr(m, ino), 0);
                        return;
                    }
                }
                reply.error(ENOENT);
            }
            Err(_) => reply.error(ENOENT),
        }
    }

    fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
        if ino == 1 {
            reply.attr(&TTL, &self.root_attr());
            return;
        }
        let link_id = match self.inodes.link_id(ino) {
            Some(l) => l,
            None => {
                reply.error(ENOENT);
                return;
            }
        };
        match self.state.lock().get_by_link(&link_id) {
            Ok(Some(m)) => reply.attr(&TTL, &self.mapping_to_attr(&m, ino)),
            _ => reply.error(ENOENT),
        }
    }

    fn opendir(&mut self, _req: &Request, _ino: u64, _flags: i32, reply: ReplyOpen) {
        reply.opened(0, 0);
    }

    fn readdir(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        let link_id = match self.parent_link_for_ino(ino) {
            Some(l) => l,
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        let children = match self.state.lock().list_children_by_parent(&link_id) {
            Ok(c) => c,
            Err(_) => {
                reply.error(libc::EIO);
                return;
            }
        };

        // entries[0] = ".", entries[1] = "..", entries[2..] = children
        let mut entries: Vec<(u64, FileType, String)> = vec![
            (ino, FileType::Directory, ".".to_string()),
            (ino, FileType::Directory, "..".to_string()),
        ];
        for m in &children {
            let child_ino = self.inodes.get_or_assign(&m.link_id);
            let kind = if m.is_folder {
                FileType::Directory
            } else {
                FileType::RegularFile
            };
            let leaf = m
                .rel_path
                .rsplit('/')
                .next()
                .unwrap_or(&m.rel_path)
                .to_string();
            entries.push((child_ino, kind, leaf));
        }

        for (i, (entry_ino, kind, name)) in entries.into_iter().enumerate().skip(offset as usize) {
            if reply.add(entry_ino, (i + 1) as i64, kind, &name) {
                break;
            }
        }
        reply.ok();
    }

    fn open(&mut self, _req: &Request, _ino: u64, flags: i32, reply: ReplyOpen) {
        // Reject write-mode opens on a read-only filesystem.
        if flags & libc::O_WRONLY != 0 || flags & libc::O_RDWR != 0 {
            reply.error(libc::EROFS);
            return;
        }
        reply.opened(0, fuser::consts::FOPEN_KEEP_CACHE);
    }

    fn read(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock: Option<u64>,
        reply: ReplyData,
    ) {
        let link_id = match self.inodes.link_id(ino) {
            Some(l) => l,
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        match self.ensure_cached(&link_id) {
            Ok(path) => match std::fs::File::open(&path) {
                Ok(mut f) => {
                    if offset > 0 {
                        if f.seek(SeekFrom::Start(offset as u64)).is_err() {
                            reply.error(libc::EIO);
                            return;
                        }
                    }
                    let mut buf = vec![0u8; size as usize];
                    match f.read(&mut buf) {
                        Ok(n) => {
                            buf.truncate(n);
                            reply.data(&buf);
                        }
                        Err(_) => reply.error(libc::EIO),
                    }
                }
                Err(_) => reply.error(libc::EIO),
            },
            Err(e) => {
                tracing::warn!(link_id, error=%e, "FUSE read: download failed");
                reply.error(libc::EIO);
            }
        }
    }

    // ── All write operations return EROFS ────────────────────────────────────

    fn write(
        &mut self,
        _req: &Request,
        _ino: u64,
        _fh: u64,
        _offset: i64,
        _data: &[u8],
        _write_flags: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyWrite,
    ) {
        reply.error(libc::EROFS);
    }

    fn create(
        &mut self,
        _req: &Request,
        _parent: u64,
        _name: &OsStr,
        _mode: u32,
        _umask: u32,
        _flags: i32,
        reply: ReplyCreate,
    ) {
        reply.error(libc::EROFS);
    }

    fn mkdir(
        &mut self,
        _req: &Request,
        _parent: u64,
        _name: &OsStr,
        _mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        reply.error(libc::EROFS);
    }

    fn unlink(&mut self, _req: &Request, _parent: u64, _name: &OsStr, reply: ReplyEmpty) {
        reply.error(libc::EROFS);
    }

    fn rmdir(&mut self, _req: &Request, _parent: u64, _name: &OsStr, reply: ReplyEmpty) {
        reply.error(libc::EROFS);
    }

    fn rename(
        &mut self,
        _req: &Request,
        _parent: u64,
        _name: &OsStr,
        _newparent: u64,
        _newname: &OsStr,
        _flags: u32,
        reply: ReplyEmpty,
    ) {
        reply.error(libc::EROFS);
    }

    fn setattr(
        &mut self,
        _req: &Request,
        _ino: u64,
        _mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        _size: Option<u64>,
        _atime: Option<fuser::TimeOrNow>,
        _mtime: Option<fuser::TimeOrNow>,
        _ctime: Option<SystemTime>,
        _fh: Option<u64>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        reply.error(libc::EROFS);
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Type alias for the FUSE background session handle.
/// Dropping this value unmounts the filesystem.
pub type FuseSession = BackgroundSession;

/// Mount the ProtonDrive virtual filesystem at `mount_point`.
///
/// Returns a [`BackgroundSession`] that keeps the mount alive.  Dropping
/// it unmounts the filesystem.
pub fn mount(
    state: Arc<PLMutex<State>>,
    bridge: Bridge,
    cache_dir: PathBuf,
    mount_point: &Path,
    root_link_id: String,
    rt: Handle,
) -> std::io::Result<BackgroundSession> {
    let _ = std::fs::create_dir_all(mount_point);

    // If a previous run crashed without unmounting, try to clean up.
    // `fusermount -u` is a no-op if nothing is mounted there.
    if let Some(p) = mount_point.to_str() {
        let _ = std::process::Command::new("fusermount")
            .args(["-u", "-z", p])
            .output();
    }

    let vfs = ProtonVfs::new(state, bridge, cache_dir, root_link_id, rt);
    let options = [
        MountOption::FSName("ProtonDrive".to_string()),
        MountOption::RO,
        MountOption::NoSuid,
        MountOption::NoDev,
    ];
    fuser::spawn_mount2(vfs, mount_point, &options)
}
