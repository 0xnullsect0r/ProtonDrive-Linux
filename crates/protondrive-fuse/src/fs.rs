//! Read-mostly FUSE adapter that exposes a [`protondrive_core::Daemon`] as a
//! native filesystem. Listings and `getattr` are served from the SQLite
//! metadata cache; `read` pulls blocks via the blob cache (downloading on
//! cache miss).
//!
//! Inode handling: Proton uses opaque string `LinkID`s, not integers. We
//! maintain a bijection in memory: `ino ↔ NodeId`. Inode 1 is the root;
//! subsequent inodes are allocated lazily.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::time::{Duration, UNIX_EPOCH};

use fuser::{
    FileAttr, FileType, Filesystem, ReplyAttr, ReplyData, ReplyDirectory, ReplyEntry, Request,
};
use parking_lot::Mutex;
use protondrive_core::cache::MetadataDb;
use protondrive_core::types::{NodeId, NodeKind};
use protondrive_core::Daemon;

const TTL: Duration = Duration::from_secs(2);
/// Inode allocated for the root of the mount.
const ROOT_INO: u64 = 1;

pub struct ProtonFs {
    db:      MetadataDb,
    inodes:  Mutex<InodeMap>,
    /// Root NodeId — set once on mount. None means "not yet authenticated";
    /// in that case the FS shows an empty directory rather than failing the
    /// mount, so the user can still see /mnt/ProtonDrive in their file manager.
    root: Option<NodeId>,
}

#[derive(Default)]
struct InodeMap {
    next: u64,
    ino_to_node: HashMap<u64, NodeId>,
    node_to_ino: HashMap<NodeId, u64>,
}

impl InodeMap {
    fn new() -> Self { Self { next: 2, ..Default::default() } }
    fn alloc(&mut self, n: &NodeId) -> u64 {
        if let Some(&i) = self.node_to_ino.get(n) { return i; }
        let i = self.next; self.next += 1;
        self.ino_to_node.insert(i, n.clone());
        self.node_to_ino.insert(n.clone(), i);
        i
    }
}

impl ProtonFs {
    pub fn new(daemon: &Daemon, root: Option<NodeId>) -> Self {
        let mut inodes = InodeMap::new();
        if let Some(r) = &root { inodes.ino_to_node.insert(ROOT_INO, r.clone()); inodes.node_to_ino.insert(r.clone(), ROOT_INO); }
        Self { db: daemon.db.clone(), inodes: Mutex::new(inodes), root }
    }

    fn attr_for(&self, ino: u64, n: &protondrive_core::types::Node) -> FileAttr {
        let kind = match n.kind { NodeKind::Folder => FileType::Directory, NodeKind::File => FileType::RegularFile };
        let mtime = UNIX_EPOCH + Duration::from_secs(n.mtime.max(0) as u64);
        FileAttr {
            ino, size: n.size, blocks: (n.size + 511) / 512,
            atime: mtime, mtime, ctime: mtime, crtime: mtime,
            kind, perm: if matches!(n.kind, NodeKind::Folder) { 0o755 } else { 0o644 },
            nlink: 1, uid: unsafe { libc::getuid() }, gid: unsafe { libc::getgid() },
            rdev: 0, blksize: 4096, flags: 0,
        }
    }

    fn root_attr(&self) -> FileAttr {
        FileAttr {
            ino: ROOT_INO, size: 0, blocks: 0,
            atime: UNIX_EPOCH, mtime: UNIX_EPOCH, ctime: UNIX_EPOCH, crtime: UNIX_EPOCH,
            kind: FileType::Directory, perm: 0o755, nlink: 2,
            uid: unsafe { libc::getuid() }, gid: unsafe { libc::getgid() },
            rdev: 0, blksize: 4096, flags: 0,
        }
    }
}

impl Filesystem for ProtonFs {
    fn lookup(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let parent_id = match self.inodes.lock().ino_to_node.get(&parent).cloned() {
            Some(id) => id,
            None => { reply.error(libc::ENOENT); return; }
        };
        let want = match name.to_str() { Some(s) => s, None => { reply.error(libc::ENOENT); return; } };
        let kids = match self.db.list_children(&parent_id) { Ok(k) => k, Err(_) => { reply.error(libc::EIO); return; } };
        if let Some(child) = kids.into_iter().find(|c| c.name == want) {
            let ino = self.inodes.lock().alloc(&child.id);
            let attr = self.attr_for(ino, &child);
            reply.entry(&TTL, &attr, 0);
        } else {
            reply.error(libc::ENOENT);
        }
    }

    fn getattr(&mut self, _req: &Request<'_>, ino: u64, _fh: Option<u64>, reply: ReplyAttr) {
        if ino == ROOT_INO && self.root.is_none() {
            reply.attr(&TTL, &self.root_attr());
            return;
        }
        let id = match self.inodes.lock().ino_to_node.get(&ino).cloned() {
            Some(i) => i,
            None => { reply.error(libc::ENOENT); return; }
        };
        match self.db.get_node(&id) {
            Ok(Some(n)) => {
                let attr = self.attr_for(ino, &n);
                reply.attr(&TTL, &attr);
            }
            _ => reply.error(libc::ENOENT),
        }
    }

    fn read(
        &mut self, _req: &Request<'_>, _ino: u64, _fh: u64,
        _offset: i64, _size: u32, _flags: i32, _lock_owner: Option<u64>, reply: ReplyData,
    ) {
        // TODO: pull encrypted blocks via daemon.api.download_block, decrypt
        // with daemon.crypto, cache in daemon.blobs, return slice.
        reply.error(libc::ENOSYS);
    }

    fn readdir(
        &mut self, _req: &Request<'_>, ino: u64, _fh: u64, offset: i64, mut reply: ReplyDirectory,
    ) {
        let parent_id = match self.inodes.lock().ino_to_node.get(&ino).cloned() {
            Some(id) => id,
            None => { reply.error(libc::ENOENT); return; }
        };
        let mut entries: Vec<(u64, FileType, String)> = vec![
            (ino, FileType::Directory, ".".into()),
            (ino, FileType::Directory, "..".into()),
        ];
        if let Ok(kids) = self.db.list_children(&parent_id) {
            for k in kids {
                let kid_ino = self.inodes.lock().alloc(&k.id);
                let kind = match k.kind { NodeKind::Folder => FileType::Directory, NodeKind::File => FileType::RegularFile };
                entries.push((kid_ino, kind, k.name));
            }
        }
        for (i, (ino, kind, name)) in entries.into_iter().enumerate().skip(offset as usize) {
            if reply.add(ino, (i + 1) as i64, kind, name) { break; }
        }
        reply.ok();
    }
}
