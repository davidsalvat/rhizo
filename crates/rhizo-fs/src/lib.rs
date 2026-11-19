//! The file system of a cell (spec `files`, design D1–D3 of
//! `add-cell-files`): in memory, private to the cell, shared copy-on-write
//! with its forks, and encodable into its snapshots.
//!
//! The inode table holds reference-counted inodes; a regular file is a size
//! and a list of reference-counted 4 KiB blocks. Cloning an [`Fs`] (a fork)
//! costs one count per inode; a later change makes private only what it
//! touches (`Arc::make_mut`). Nothing here reads a clock or touches hardware.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

mod codec;

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

pub use codec::DecodeError;

/// Inode number (index in the inode table).
pub type Ino = u32;
/// File descriptor number.
pub type Fd = u32;

/// Size of a file block.
pub const BLOCK: usize = 4096;
/// Longest file name.
pub const NAME_MAX: usize = 255;
/// Most descriptors a cell may have open.
pub const FD_MAX: usize = 1024;
const ROOT: Ino = 0;

/// Why an operation failed, with its Linux error number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    NotFound,
    Exists,
    NotDir,
    IsDir,
    NotEmpty,
    BadFd,
    Invalid,
    NameTooLong,
    TooManyFiles,
    Busy,
}

impl Error {
    /// The Linux `errno` value.
    pub const fn errno(self) -> i64 {
        match self {
            Error::NotFound => 2,
            Error::BadFd => 9,
            Error::Busy => 16,
            Error::Exists => 17,
            Error::NotDir => 20,
            Error::IsDir => 21,
            Error::Invalid => 22,
            Error::TooManyFiles => 24,
            Error::NameTooLong => 36,
            Error::NotEmpty => 39,
        }
    }
}

/// Result of a file system operation.
pub type Result<T> = core::result::Result<T, Error>;

type Block = [u8; BLOCK];

/// Contents of a regular file: blocks never written are holes (zeros).
#[derive(Debug, Clone, Default)]
struct File {
    size: u64,
    blocks: Vec<Option<Arc<Block>>>,
}

#[derive(Debug, Clone)]
enum Node {
    Dir {
        entries: BTreeMap<Vec<u8>, Ino>,
        parent: Ino,
    },
    File(File),
}

#[derive(Debug, Clone)]
struct Inode {
    /// Directory entries naming it (1 for a directory while it exists).
    links: u32,
    node: Node,
}

/// What a descriptor refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The cell's console (standard input, output, error).
    Console,
    /// A regular file.
    File,
    /// A directory (opened for `getdents64`).
    Dir,
}

/// An open file description, shared by duplicated descriptors.
#[derive(Debug, Clone)]
struct Desc {
    target: Option<Ino>, // None: the console
    offset: u64,
    flags: u32,
    refs: u32,
}

/// How to open a file (the architecture-independent part of `open` flags).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OpenFlags {
    pub read: bool,
    pub write: bool,
    pub create: bool,
    pub exclusive: bool,
    pub truncate: bool,
    pub append: bool,
    pub directory: bool,
}

/// `fstat` data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stat {
    pub ino: u64,
    pub kind: Kind,
    pub links: u32,
    pub size: u64,
}

/// One `getdents64` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub ino: u64,
    pub kind: Kind,
    pub name: Vec<u8>,
}

/// Origin of `lseek`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Whence {
    Set,
    Current,
    End,
}

/// File status flags kept for `fcntl(F_GETFL)` (append is applied).
pub const O_APPEND_FLAG: u32 = 0x400;

/// The file system and open descriptors of one cell.
#[derive(Debug, Clone)]
pub struct Fs {
    inodes: Vec<Option<Arc<Inode>>>,
    descs: Vec<Option<Desc>>,
    /// Descriptor → description index, and the close-on-exec flag.
    fds: Vec<Option<(u32, bool)>>,
    cwd: Ino,
}

impl Default for Fs {
    fn default() -> Self {
        Self::new()
    }
}

impl Fs {
    /// An empty tree (`/`) with descriptors 0–2 on the console.
    pub fn new() -> Self {
        let root = Inode {
            links: 1,
            node: Node::Dir {
                entries: BTreeMap::new(),
                parent: ROOT,
            },
        };
        let console = Desc {
            target: None,
            offset: 0,
            flags: 2, // O_RDWR
            refs: 3,
        };
        Self {
            inodes: vec![Some(Arc::new(root))],
            descs: vec![Some(console)],
            fds: vec![Some((0, false)); 3],
            cwd: ROOT,
        }
    }

    fn inode(&self, ino: Ino) -> &Inode {
        self.inodes[ino as usize]
            .as_deref()
            .expect("inode numbers in use are valid")
    }

    fn inode_mut(&mut self, ino: Ino) -> &mut Inode {
        Arc::make_mut(
            self.inodes[ino as usize]
                .as_mut()
                .expect("inode numbers in use are valid"),
        )
    }

    fn alloc_inode(&mut self, inode: Inode) -> Ino {
        let inode = Some(Arc::new(inode));
        match self.inodes.iter().position(Option::is_none) {
            Some(i) => {
                self.inodes[i] = inode;
                i as Ino
            }
            None => {
                self.inodes.push(inode);
                (self.inodes.len() - 1) as Ino
            }
        }
    }

    /// Frees `ino` if no name and no open description refers to it.
    fn maybe_free(&mut self, ino: Ino) {
        let open = self.descs.iter().flatten().any(|d| d.target == Some(ino));
        if self.inode(ino).links == 0 && !open {
            self.inodes[ino as usize] = None;
        }
    }

    fn desc(&self, fd: Fd) -> Result<&Desc> {
        let (d, _) = self
            .fds
            .get(fd as usize)
            .copied()
            .flatten()
            .ok_or(Error::BadFd)?;
        Ok(self.descs[d as usize]
            .as_ref()
            .expect("descriptions in use"))
    }

    fn desc_mut(&mut self, fd: Fd) -> Result<&mut Desc> {
        let (d, _) = self
            .fds
            .get(fd as usize)
            .copied()
            .flatten()
            .ok_or(Error::BadFd)?;
        Ok(self.descs[d as usize]
            .as_mut()
            .expect("descriptions in use"))
    }

    /// What descriptor `fd` refers to.
    pub fn kind(&self, fd: Fd) -> Result<Kind> {
        Ok(match self.desc(fd)?.target {
            None => Kind::Console,
            Some(ino) => kind_of(self.inode(ino)),
        })
    }

    /// The directory a relative path starts from: `dirfd` or the working
    /// directory (`None`, `AT_FDCWD`).
    fn base(&self, dirfd: Option<Fd>, path: &[u8]) -> Result<Ino> {
        if path.first() == Some(&b'/') {
            return Ok(ROOT);
        }
        match dirfd {
            None => Ok(self.cwd),
            Some(fd) => match self.desc(fd)?.target {
                Some(ino) if matches!(self.inode(ino).node, Node::Dir { .. }) => Ok(ino),
                _ => Err(Error::NotDir),
            },
        }
    }

    /// Resolves `path` to its parent directory and last name (`None` for
    /// paths that name a directory itself, like `/`, `.` or `a/..`).
    fn parent_of<'p>(&self, dirfd: Option<Fd>, path: &'p [u8]) -> Result<(Ino, Option<&'p [u8]>)> {
        if path.is_empty() {
            return Err(Error::NotFound);
        }
        let mut dir = self.base(dirfd, path)?;
        let parts: Vec<&[u8]> = path
            .split(|&b| b == b'/')
            .filter(|p| !p.is_empty())
            .collect();
        let Some((last, walk)) = parts.split_last() else {
            return Ok((dir, None));
        };
        for part in walk {
            dir = self.step(dir, part)?;
        }
        match *last {
            b"." => Ok((dir, None)),
            b".." => Ok((self.step(dir, b"..")?, None)),
            name if name.len() > NAME_MAX => Err(Error::NameTooLong),
            name => Ok((dir, Some(name))),
        }
    }

    /// One component from directory `dir`.
    fn step(&self, dir: Ino, part: &[u8]) -> Result<Ino> {
        let Node::Dir { entries, parent } = &self.inode(dir).node else {
            return Err(Error::NotDir);
        };
        match part {
            b"." => Ok(dir),
            b".." => Ok(*parent),
            name => {
                let ino = *entries.get(name).ok_or(Error::NotFound)?;
                Ok(ino)
            }
        }
        .and_then(|ino| match self.inode(ino).node {
            Node::Dir { .. } => Ok(ino),
            Node::File(_) => Err(Error::NotDir),
        })
    }

    /// The inode `path` names, if any.
    fn lookup(&self, dirfd: Option<Fd>, path: &[u8]) -> Result<Ino> {
        let trailing_slash = path.ends_with(b"/");
        let (dir, name) = self.parent_of(dirfd, path)?;
        let Some(name) = name else {
            return Ok(dir);
        };
        let Node::Dir { entries, .. } = &self.inode(dir).node else {
            return Err(Error::NotDir);
        };
        let ino = *entries.get(name).ok_or(Error::NotFound)?;
        if trailing_slash && !matches!(self.inode(ino).node, Node::Dir { .. }) {
            return Err(Error::NotDir);
        }
        Ok(ino)
    }

    fn new_fd(&mut self, desc: u32, min: Fd, cloexec: bool) -> Result<Fd> {
        let free = (min as usize..FD_MAX).find(|&i| self.fds.get(i).is_none_or(Option::is_none));
        let fd = free.ok_or(Error::TooManyFiles)?;
        if self.fds.len() <= fd {
            self.fds.resize(fd + 1, None);
        }
        self.fds[fd] = Some((desc, cloexec));
        Ok(fd as Fd)
    }

    fn new_desc(&mut self, target: Option<Ino>, flags: u32) -> u32 {
        let desc = Some(Desc {
            target,
            offset: 0,
            flags,
            refs: 1,
        });
        match self.descs.iter().position(Option::is_none) {
            Some(i) => {
                self.descs[i] = desc;
                i as u32
            }
            None => {
                self.descs.push(desc);
                (self.descs.len() - 1) as u32
            }
        }
    }

    /// `openat`: opens (or creates) `path`; returns the lowest free
    /// descriptor. `access` is the raw `O_ACCMODE | O_APPEND` bits kept for
    /// `fcntl(F_GETFL)`.
    pub fn open(
        &mut self,
        dirfd: Option<Fd>,
        path: &[u8],
        flags: OpenFlags,
        access: u32,
    ) -> Result<Fd> {
        let ino = match self.lookup(dirfd, path) {
            Ok(_) if flags.create && flags.exclusive => return Err(Error::Exists),
            Ok(ino) => ino,
            Err(Error::NotFound) if flags.create => {
                let (dir, name) = self.parent_of(dirfd, path)?;
                let name = name.ok_or(Error::IsDir)?;
                if path.ends_with(b"/") {
                    return Err(Error::IsDir);
                }
                let ino = self.alloc_inode(Inode {
                    links: 1,
                    node: Node::File(File::default()),
                });
                let Node::Dir { entries, .. } = &mut self.inode_mut(dir).node else {
                    return Err(Error::NotDir);
                };
                entries.insert(name.to_vec(), ino);
                ino
            }
            Err(e) => return Err(e),
        };
        let is_dir = matches!(self.inode(ino).node, Node::Dir { .. });
        if flags.directory && !is_dir {
            return Err(Error::NotDir);
        }
        if is_dir && (flags.write || flags.truncate) {
            return Err(Error::IsDir);
        }
        if flags.truncate
            && flags.write
            && let Node::File(f) = &mut self.inode_mut(ino).node
        {
            *f = File::default();
        }
        let desc = self.new_desc(Some(ino), access);
        self.new_fd(desc, 0, false)
            .inspect_err(|_| self.descs[desc as usize] = None)
    }

    /// `close`.
    pub fn close(&mut self, fd: Fd) -> Result<()> {
        let (d, _) = self
            .fds
            .get(fd as usize)
            .copied()
            .flatten()
            .ok_or(Error::BadFd)?;
        self.fds[fd as usize] = None;
        let desc = self.descs[d as usize]
            .as_mut()
            .expect("descriptions in use");
        desc.refs -= 1;
        if desc.refs == 0 {
            let target = desc.target;
            self.descs[d as usize] = None;
            if let Some(ino) = target {
                self.maybe_free(ino);
            }
        }
        Ok(())
    }

    /// `dup` (`F_DUPFD` from `min`) and `dup2`/`dup3` (`exact`).
    pub fn dup(&mut self, fd: Fd, min: Fd, exact: bool, cloexec: bool) -> Result<Fd> {
        let (d, _) = self
            .fds
            .get(fd as usize)
            .copied()
            .flatten()
            .ok_or(Error::BadFd)?;
        if exact {
            if min as usize >= FD_MAX {
                return Err(Error::BadFd);
            }
            if min == fd {
                return Ok(fd);
            }
            if self.fds.get(min as usize).copied().flatten().is_some() {
                self.close(min)?;
            }
        }
        let new = self.new_fd(d, min, cloexec)?;
        self.descs[d as usize]
            .as_mut()
            .expect("descriptions in use")
            .refs += 1;
        Ok(new)
    }

    /// `fcntl(F_GETFL)`.
    pub fn status_flags(&self, fd: Fd) -> Result<u32> {
        Ok(self.desc(fd)?.flags)
    }

    /// `fcntl(F_SETFL)`: only `O_APPEND` can change.
    pub fn set_append(&mut self, fd: Fd, append: bool) -> Result<()> {
        let desc = self.desc_mut(fd)?;
        desc.flags = (desc.flags & !O_APPEND_FLAG) | if append { O_APPEND_FLAG } else { 0 };
        Ok(())
    }

    /// `fcntl(F_GETFD)` / `F_SETFD` (close-on-exec, kept only for `F_GETFD`).
    pub fn cloexec(&mut self, fd: Fd, set: Option<bool>) -> Result<bool> {
        let slot = self
            .fds
            .get_mut(fd as usize)
            .and_then(Option::as_mut)
            .ok_or(Error::BadFd)?;
        if let Some(v) = set {
            slot.1 = v;
        }
        Ok(slot.1)
    }

    fn file_of(&self, fd: Fd) -> Result<(Ino, &File, &Desc)> {
        let desc = self.desc(fd)?;
        let ino = desc.target.ok_or(Error::Invalid)?;
        match &self.inode(ino).node {
            Node::File(f) => Ok((ino, f, desc)),
            Node::Dir { .. } => Err(Error::IsDir),
        }
    }

    /// `pread64`-style read at `offset` (or the description's offset, which
    /// then advances). Returns bytes read (0 at the end).
    pub fn read(&mut self, fd: Fd, out: &mut [u8], offset: Option<u64>) -> Result<usize> {
        let (_, file, desc) = self.file_of(fd)?;
        if desc.flags & 3 == 1 {
            return Err(Error::BadFd); // O_WRONLY
        }
        let at = offset.unwrap_or(desc.offset);
        let n = (file.size.saturating_sub(at).min(out.len() as u64)) as usize;
        let mut done = 0;
        while done < n {
            let pos = at as usize + done;
            let (b, off) = (pos / BLOCK, pos % BLOCK);
            let len = (BLOCK - off).min(n - done);
            let dst = &mut out[done..done + len];
            match file.blocks.get(b) {
                Some(Some(block)) => dst.copy_from_slice(&block[off..off + len]),
                _ => dst.fill(0),
            }
            done += len;
        }
        if offset.is_none() {
            self.desc_mut(fd)?.offset = at + n as u64;
        }
        Ok(n)
    }

    /// Writes `data` at `offset` (or at the description's offset, or at the
    /// end with `O_APPEND`); returns bytes written.
    pub fn write(&mut self, fd: Fd, data: &[u8], offset: Option<u64>) -> Result<usize> {
        let (ino, file, desc) = self.file_of(fd)?;
        if desc.flags & 3 == 0 {
            return Err(Error::BadFd); // O_RDONLY
        }
        let at = match offset {
            Some(at) => at,
            None if desc.flags & O_APPEND_FLAG != 0 => file.size,
            None => desc.offset,
        };
        if data.is_empty() {
            return Ok(0);
        }
        let end = at.checked_add(data.len() as u64).ok_or(Error::Invalid)?;
        let Node::File(file) = &mut self.inode_mut(ino).node else {
            return Err(Error::IsDir);
        };
        let blocks = (end as usize).div_ceil(BLOCK);
        if file.blocks.len() < blocks {
            file.blocks.resize(blocks, None);
        }
        let mut done = 0;
        while done < data.len() {
            let pos = at as usize + done;
            let (b, off) = (pos / BLOCK, pos % BLOCK);
            let n = (BLOCK - off).min(data.len() - done);
            let block = file.blocks[b].get_or_insert_with(|| Arc::new([0; BLOCK]));
            Arc::make_mut(block)[off..off + n].copy_from_slice(&data[done..done + n]);
            done += n;
        }
        file.size = file.size.max(end);
        if offset.is_none() {
            self.desc_mut(fd)?.offset = end;
        }
        Ok(data.len())
    }

    /// `lseek`.
    pub fn seek(&mut self, fd: Fd, offset: i64, whence: Whence) -> Result<u64> {
        let size = match self.desc(fd)?.target {
            None => return Err(Error::Invalid), // ESPIPE on Linux; console only
            Some(ino) => match &self.inode(ino).node {
                Node::File(f) => f.size,
                Node::Dir { .. } => 0,
            },
        };
        let desc = self.desc_mut(fd)?;
        let base = match whence {
            Whence::Set => 0,
            Whence::Current => desc.offset as i64,
            Whence::End => size as i64,
        };
        let new = base
            .checked_add(offset)
            .filter(|&o| o >= 0)
            .ok_or(Error::Invalid)?;
        desc.offset = new as u64;
        Ok(new as u64)
    }

    /// `ftruncate`.
    pub fn truncate(&mut self, fd: Fd, size: u64) -> Result<()> {
        let (ino, _, desc) = self.file_of(fd)?;
        if desc.flags & 3 == 0 {
            return Err(Error::Invalid);
        }
        let Node::File(file) = &mut self.inode_mut(ino).node else {
            return Err(Error::IsDir);
        };
        let blocks = (size as usize).div_ceil(BLOCK);
        file.blocks.truncate(blocks);
        // Bytes past the new end of its last block must read as zero later.
        let keep = size as usize % BLOCK;
        if keep != 0
            && file.blocks.len() == blocks
            && let Some(last) = &mut file.blocks[blocks - 1]
            && last[keep..].iter().any(|&b| b != 0)
        {
            Arc::make_mut(last)[keep..].fill(0);
        }
        file.size = size;
        Ok(())
    }

    fn stat_of(&self, ino: Ino) -> Stat {
        let inode = self.inode(ino);
        Stat {
            ino: ino as u64 + 1,
            kind: kind_of(inode),
            links: match &inode.node {
                Node::Dir { entries, .. } => {
                    2 + entries
                        .values()
                        .filter(|&&i| matches!(self.inode(i).node, Node::Dir { .. }))
                        .count() as u32
                }
                Node::File(_) => inode.links,
            },
            size: match &inode.node {
                Node::File(f) => f.size,
                Node::Dir { .. } => BLOCK as u64,
            },
        }
    }

    /// `fstat` (`None` for the console).
    pub fn fstat(&self, fd: Fd) -> Result<Option<Stat>> {
        Ok(self.desc(fd)?.target.map(|ino| self.stat_of(ino)))
    }

    /// `newfstatat` / `stat` by path.
    pub fn stat(&self, dirfd: Option<Fd>, path: &[u8]) -> Result<Stat> {
        Ok(self.stat_of(self.lookup(dirfd, path)?))
    }

    /// `mkdirat`.
    pub fn mkdir(&mut self, dirfd: Option<Fd>, path: &[u8]) -> Result<()> {
        let (dir, name) = self.parent_of(dirfd, path)?;
        let name = name.ok_or(Error::Exists)?;
        let Node::Dir { entries, .. } = &self.inode(dir).node else {
            return Err(Error::NotDir);
        };
        if entries.contains_key(name) {
            return Err(Error::Exists);
        }
        let ino = self.alloc_inode(Inode {
            links: 1,
            node: Node::Dir {
                entries: BTreeMap::new(),
                parent: dir,
            },
        });
        if let Node::Dir { entries, .. } = &mut self.inode_mut(dir).node {
            entries.insert(name.to_vec(), ino);
        }
        Ok(())
    }

    /// `unlinkat`: a file (`rmdir` false) or an empty directory (true).
    pub fn unlink(&mut self, dirfd: Option<Fd>, path: &[u8], rmdir: bool) -> Result<()> {
        let (dir, name) = self.parent_of(dirfd, path)?;
        let Some(name) = name else {
            return Err(if rmdir { Error::Busy } else { Error::IsDir });
        };
        let Node::Dir { entries, .. } = &self.inode(dir).node else {
            return Err(Error::NotDir);
        };
        let ino = *entries.get(name).ok_or(Error::NotFound)?;
        match (&self.inode(ino).node, rmdir) {
            (Node::Dir { entries, .. }, true) if !entries.is_empty() => {
                return Err(Error::NotEmpty);
            }
            (Node::Dir { .. }, true) if ino == self.cwd => return Err(Error::Busy),
            (Node::Dir { .. }, false) => return Err(Error::IsDir),
            (Node::File(_), true) => return Err(Error::NotDir),
            _ => {}
        }
        if let Node::Dir { entries, .. } = &mut self.inode_mut(dir).node {
            entries.remove(name);
        }
        let inode = self.inode_mut(ino);
        inode.links -= 1;
        if let Node::Dir { parent, .. } = &mut inode.node {
            // A removed directory still open lists nothing; `..` is itself.
            *parent = ino;
        }
        self.maybe_free(ino);
        Ok(())
    }

    /// `renameat`: moves `old` to `new`, replacing a file or an empty
    /// directory there.
    pub fn rename(
        &mut self,
        olddir: Option<Fd>,
        old: &[u8],
        newdir: Option<Fd>,
        new: &[u8],
    ) -> Result<()> {
        let (from_dir, from_name) = self.parent_of(olddir, old)?;
        let from_name = from_name.ok_or(Error::Busy)?;
        let Node::Dir { entries, .. } = &self.inode(from_dir).node else {
            return Err(Error::NotDir);
        };
        let ino = *entries.get(from_name).ok_or(Error::NotFound)?;
        let (to_dir, to_name) = self.parent_of(newdir, new)?;
        let to_name = to_name.ok_or(Error::Busy)?;
        let moving_dir = matches!(self.inode(ino).node, Node::Dir { .. });
        if moving_dir {
            // A directory cannot move into itself or below.
            let mut at = to_dir;
            loop {
                if at == ino {
                    return Err(Error::Invalid);
                }
                let Node::Dir { parent, .. } = self.inode(at).node else {
                    break;
                };
                if at == ROOT {
                    break;
                }
                at = parent;
            }
        }
        let Node::Dir { entries, .. } = &self.inode(to_dir).node else {
            return Err(Error::NotDir);
        };
        if let Some(&target) = entries.get(to_name) {
            if target == ino {
                return Ok(());
            }
            match (&self.inode(target).node, moving_dir) {
                (Node::Dir { .. }, false) => return Err(Error::IsDir),
                (Node::File(_), true) => return Err(Error::NotDir),
                (Node::Dir { entries, .. }, true) if !entries.is_empty() => {
                    return Err(Error::NotEmpty);
                }
                _ => {}
            }
            if let Node::Dir { entries, .. } = &mut self.inode_mut(to_dir).node {
                entries.remove(to_name);
            }
            let inode = self.inode_mut(target);
            inode.links -= 1;
            if let Node::Dir { parent, .. } = &mut inode.node {
                *parent = target;
            }
            self.maybe_free(target);
        }
        if let Node::Dir { entries, .. } = &mut self.inode_mut(from_dir).node {
            entries.remove(from_name);
        }
        if let Node::Dir { entries, .. } = &mut self.inode_mut(to_dir).node {
            entries.insert(to_name.to_vec(), ino);
        }
        if let Node::Dir { parent, .. } = &mut self.inode_mut(ino).node {
            *parent = to_dir;
        }
        Ok(())
    }

    /// `getcwd`: the absolute path of the working directory.
    pub fn getcwd(&self) -> Vec<u8> {
        let mut parts = Vec::new();
        let mut at = self.cwd;
        while at != ROOT {
            let Node::Dir { parent, .. } = self.inode(at).node else {
                break;
            };
            let Node::Dir { entries, .. } = &self.inode(parent).node else {
                break;
            };
            let name = entries
                .iter()
                .find(|&(_, &i)| i == at)
                .map(|(n, _)| n.clone());
            parts.push(name.unwrap_or_default());
            at = parent;
        }
        let mut path = Vec::new();
        for part in parts.iter().rev() {
            path.push(b'/');
            path.extend_from_slice(part);
        }
        if path.is_empty() {
            path.push(b'/');
        }
        path
    }

    /// `chdir`.
    pub fn chdir(&mut self, path: &[u8]) -> Result<()> {
        let ino = self.lookup(None, path)?;
        if !matches!(self.inode(ino).node, Node::Dir { .. }) {
            return Err(Error::NotDir);
        }
        self.cwd = ino;
        Ok(())
    }

    /// `getdents64`: entries of directory `fd` from its offset (an entry
    /// index; `.` and `..` first), handed to `emit` until it returns `false`
    /// (no room); the offset advances past those taken.
    pub fn read_dir(&mut self, fd: Fd, mut emit: impl FnMut(&DirEntry) -> bool) -> Result<usize> {
        let desc = self.desc(fd)?;
        let ino = desc.target.ok_or(Error::NotDir)?;
        let Node::Dir { entries, parent } = &self.inode(ino).node else {
            return Err(Error::NotDir);
        };
        let start = desc.offset as usize;
        let dots = [(b".".to_vec(), ino), (b"..".to_vec(), *parent)];
        let all = dots
            .iter()
            .cloned()
            .chain(entries.iter().map(|(n, &i)| (n.clone(), i)));
        let mut taken = 0;
        for (name, i) in all.skip(start) {
            let entry = DirEntry {
                ino: i as u64 + 1,
                kind: kind_of(self.inode(i)),
                name,
            };
            if !emit(&entry) {
                break;
            }
            taken += 1;
        }
        self.desc_mut(fd)?.offset = (start + taken) as u64;
        Ok(taken)
    }

    /// Creates `path` with `data`, making its directories (host seeding of a
    /// cell's initial files).
    pub fn put_file(&mut self, path: &[u8], data: &[u8]) -> Result<()> {
        let mut prefix = Vec::new();
        let parts: Vec<&[u8]> = path
            .split(|&b| b == b'/')
            .filter(|p| !p.is_empty())
            .collect();
        for part in &parts[..parts.len().saturating_sub(1)] {
            prefix.push(b'/');
            prefix.extend_from_slice(part);
            match self.mkdir(None, &prefix) {
                Ok(()) | Err(Error::Exists) => {}
                Err(e) => return Err(e),
            }
        }
        let flags = OpenFlags {
            write: true,
            create: true,
            truncate: true,
            ..OpenFlags::default()
        };
        let fd = self.open(None, path, flags, 1)?;
        let written = self.write(fd, data, None);
        self.close(fd)?;
        written.map(|_| ())
    }

    /// Bytes of file data this cell holds (blocks it does not share count
    /// fully; shared ones too: a bound, for accounting and tests).
    pub fn data_bytes(&self) -> u64 {
        self.inodes
            .iter()
            .flatten()
            .map(|i| match &i.node {
                Node::File(f) => f.blocks.iter().flatten().count() as u64 * BLOCK as u64,
                Node::Dir { .. } => 0,
            })
            .sum()
    }
}

fn kind_of(inode: &Inode) -> Kind {
    match inode.node {
        Node::Dir { .. } => Kind::Dir,
        Node::File(_) => Kind::File,
    }
}

#[cfg(test)]
mod tests;
