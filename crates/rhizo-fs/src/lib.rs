//! The file system of a cell (spec `files`, design D1–D3 of
//! `add-cell-files`): in memory, private to the cell, shared copy-on-write
//! with its forks, and encodable into its snapshots. It also holds the
//! cell's other open objects: pipes, event counters and `epoll` instances,
//! and the handles of its sockets, which live in the kernel (design D1 of
//! `add-sockets-and-poll`).
//!
//! The inode table holds reference-counted inodes; a regular file is a size
//! and a list of reference-counted 4 KiB blocks. Cloning an [`Fs`] (a fork)
//! costs one count per inode; a later change makes private only what it
//! touches (`Arc::make_mut`). Nothing here reads a clock or touches hardware.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

mod codec;
mod io;

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

pub use codec::DecodeError;
pub use io::{EpollOp, Interest, PIPE_BUF, PIPE_CAP};

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
/// Most file data a cell may hold, and the largest file size (bytes).
pub const DATA_MAX: u64 = 256 << 20;
/// Most files and directories a cell may have (the root included): bounds
/// the kernel memory its names take, and so how deep a path can nest.
pub const INODES_MAX: usize = 4096;
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
    /// The cell holds [`DATA_MAX`] bytes of file data, or [`INODES_MAX`]
    /// files and directories.
    NoSpace,
    /// Past the largest file size.
    TooBig,
    /// It would wait (`EAGAIN`).
    Again,
    /// The read end of a pipe is closed (`EPIPE`).
    Pipe,
    /// Not allowed for this kind of descriptor (`EPERM`).
    Perm,
    /// Not a socket descriptor (`ENOTSOCK`).
    NotSocket,
    /// A file that cannot be opened: a socket (`ENXIO`).
    NoDevice,
    /// The path names no socket (`ECONNREFUSED`).
    Refused,
}

impl Error {
    /// The Linux `errno` value.
    pub const fn errno(self) -> i64 {
        match self {
            Error::Perm => 1,
            Error::NotFound => 2,
            Error::NoDevice => 6,
            Error::Again => 11,
            Error::Pipe => 32,
            Error::NotSocket => 88,
            Error::Refused => 111,
            Error::BadFd => 9,
            Error::Busy => 16,
            Error::Exists => 17,
            Error::NotDir => 20,
            Error::IsDir => 21,
            Error::Invalid => 22,
            Error::TooManyFiles => 24,
            Error::TooBig => 27,
            Error::NoSpace => 28,
            Error::NameTooLong => 36,
            Error::NotEmpty => 39,
        }
    }
}

/// Result of a file system operation.
pub type Result<T> = core::result::Result<T, Error>;

type Block = [u8; BLOCK];

/// A block shared between forks. The count lives apart from the data
/// (`Arc<Box<_>>`, not `Arc<Block>`), so the data fills exactly one page of
/// the kernel heap instead of spilling 16 bytes into a second one.
type Shared = Arc<Box<Block>>;

/// One page of a regular file as it is now, for mapping it into a cell
/// (`mmap`, the loader): the file's block, held. While it is held, the
/// block cannot change in place (a write to the file copies it first), so
/// [`key`](Page::key) names these bytes and no others.
#[derive(Debug, Clone)]
pub struct Page {
    block: Option<Shared>,
    len: usize,
}

impl Page {
    /// Identity of the page's contents: two pages alive at once have the
    /// same key only if they hold the same bytes. `None` for a page with no
    /// file data (a hole, or past the end), which reads as zeros.
    pub fn key(&self) -> Option<(usize, usize)> {
        let block = self.block.as_ref()?;
        Some((Arc::as_ptr(block) as usize, self.len))
    }

    /// The file's bytes in this page (at most [`BLOCK`]); the rest of the
    /// page reads as zero.
    pub fn bytes(&self) -> &[u8] {
        self.block.as_ref().map_or(&[], |b| &b[..self.len])
    }
}

/// Contents of a regular file: blocks never written are holes (zeros).
#[derive(Debug, Clone, Default)]
struct File {
    size: u64,
    blocks: Vec<Option<Shared>>,
}

impl File {
    /// Blocks written (holes excluded).
    fn held(&self) -> u64 {
        self.blocks.iter().filter(|b| b.is_some()).count() as u64
    }
}

#[derive(Debug, Clone)]
enum Node {
    Dir {
        entries: BTreeMap<Vec<u8>, Ino>,
        parent: Ino,
    },
    File(File),
    /// A Unix socket's name: the number the kernel's sockets know it by
    /// (0: no socket any more).
    Socket(u64),
}

#[derive(Debug, Clone)]
struct Inode {
    /// Directory entries naming it (1 for a directory while it exists).
    links: u32,
    node: Node,
}

/// What a descriptor or a name refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The cell's console (standard input, output, error).
    Console,
    /// A regular file.
    File,
    /// A directory (opened for `getdents64`).
    Dir,
    /// A pipe end.
    Pipe,
    /// An event counter (`eventfd`).
    Event,
    /// An `epoll` instance.
    Epoll,
    /// A socket, or a Unix socket's name.
    Socket,
}

/// What an open file description refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Console,
    Inode(Ino),
    /// Object index; `true` for the write end.
    Pipe(u32, bool),
    Event(u32),
    Epoll(u32),
    /// A socket handle of the kernel (`rhizo-net`), opaque here.
    Socket(u32),
}

/// An open file description, shared by duplicated descriptors.
#[derive(Debug, Clone)]
struct Desc {
    target: Target,
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
/// `O_NONBLOCK` (the same value on both architectures).
pub const O_NONBLOCK_FLAG: u32 = 0x800;

/// The file system and open descriptors of one cell: its [`Tables`], shared
/// with the cell's forks (and with every cell spawned without files) until
/// one of them changes something. A clone costs one reference count; the
/// first change after it copies the tables (not the file data).
#[derive(Debug, Clone)]
pub struct Fs(Arc<Tables>);

impl Fs {
    /// An empty tree (`/`) with descriptors 0–2 on the console.
    pub fn new() -> Self {
        Self(Arc::new(Tables::new()))
    }

    /// Whether `self` and `other` share their tables (nothing changed since
    /// one was cloned from the other).
    pub fn shares_with(&self, other: &Fs) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Default for Fs {
    fn default() -> Self {
        Self::new()
    }
}

impl core::ops::Deref for Fs {
    type Target = Tables;
    fn deref(&self) -> &Tables {
        &self.0
    }
}

impl core::ops::DerefMut for Fs {
    /// Changes go to this cell's own copy of the tables.
    fn deref_mut(&mut self) -> &mut Tables {
        Arc::make_mut(&mut self.0)
    }
}

/// The tables of a file system: inodes, open descriptions, descriptors,
/// other open objects and the working directory. Reached through an [`Fs`].
#[derive(Debug, Clone)]
pub struct Tables {
    inodes: Vec<Option<Arc<Inode>>>,
    descs: Vec<Option<Desc>>,
    /// Descriptor → description index, and the close-on-exec flag.
    fds: Vec<Option<(u32, bool)>>,
    cwd: Ino,
    /// File blocks held (shared ones included), at most `DATA_MAX / BLOCK`.
    used: u64,
    /// Pipes, event counters and `epoll` instances.
    objects: Vec<Option<Arc<io::Object>>>,
    /// Socket handles whose last description closed, for the kernel to
    /// release ([`Tables::take_released`]); empty between system calls.
    released: Vec<u32>,
    /// The pipe whose write end the host holds, outside the descriptors:
    /// standard input from the host ([`Tables::host_input`]).
    input: Option<u32>,

    /// Counts changes to pipes, event counters and `epoll` instances
    /// ([`Tables::changes`]).
    changes: u64,
}

impl Tables {
    /// An empty tree (`/`) with descriptors 0–2 on the console.
    fn new() -> Self {
        let root = Inode {
            links: 1,
            node: Node::Dir {
                entries: BTreeMap::new(),
                parent: ROOT,
            },
        };
        let console = Desc {
            target: Target::Console,
            offset: 0,
            flags: 2, // O_RDWR
            refs: 3,
        };
        Self {
            inodes: vec![Some(Arc::new(root))],
            descs: vec![Some(console)],
            fds: vec![Some((0, false)); 3],
            cwd: ROOT,
            used: 0,
            objects: Vec::new(),
            released: Vec::new(),
            input: None,

            changes: 0,
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

    fn alloc_inode(&mut self, inode: Inode) -> Result<Ino> {
        let inode = Some(Arc::new(inode));
        match self.inodes.iter().position(Option::is_none) {
            Some(i) => {
                self.inodes[i] = inode;
                Ok(i as Ino)
            }
            None if self.inodes.len() >= INODES_MAX => Err(Error::NoSpace),
            None => {
                self.inodes.push(inode);
                Ok((self.inodes.len() - 1) as Ino)
            }
        }
    }

    /// Frees `ino` if no name and no open description refers to it.
    fn maybe_free(&mut self, ino: Ino) {
        let open = self
            .descs
            .iter()
            .flatten()
            .any(|d| d.target == Target::Inode(ino));
        if self.inode(ino).links == 0 && !open {
            if let Node::File(f) = &self.inode(ino).node {
                self.used -= f.held();
            }
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
            Target::Console => Kind::Console,
            Target::Inode(ino) => kind_of(self.inode(ino)),
            Target::Pipe(..) => Kind::Pipe,
            Target::Event(_) => Kind::Event,
            Target::Epoll(_) => Kind::Epoll,
            Target::Socket(_) => Kind::Socket,
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
                Target::Inode(ino) if matches!(self.inode(ino).node, Node::Dir { .. }) => Ok(ino),
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
            _ => Err(Error::NotDir),
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

    fn new_desc(&mut self, target: Target, flags: u32) -> u32 {
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
                })?;
                let Node::Dir { entries, .. } = &mut self.inode_mut(dir).node else {
                    return Err(Error::NotDir);
                };
                entries.insert(name.to_vec(), ino);
                ino
            }
            Err(e) => return Err(e),
        };
        let is_dir = matches!(self.inode(ino).node, Node::Dir { .. });
        if matches!(self.inode(ino).node, Node::Socket(_)) {
            return Err(Error::NoDevice);
        }
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
            let held = f.held();
            *f = File::default();
            self.used -= held;
        }
        let desc = self.new_desc(Target::Inode(ino), access);
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
        self.forget_interest(fd);
        let desc = self.descs[d as usize]
            .as_mut()
            .expect("descriptions in use");
        desc.refs -= 1;
        if desc.refs == 0 {
            let target = desc.target;
            self.descs[d as usize] = None;
            match target {
                Target::Inode(ino) => self.maybe_free(ino),
                Target::Socket(h) => self.released.push(h),
                Target::Console => {}
                Target::Pipe(..) | Target::Event(_) | Target::Epoll(_) => {
                    self.drop_end(target);
                }
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

    /// `fcntl(F_SETFL)`: only `O_APPEND` and `O_NONBLOCK` can change.
    pub fn set_status_flags(&mut self, fd: Fd, flags: u32) -> Result<()> {
        const CHANGEABLE: u32 = O_APPEND_FLAG | O_NONBLOCK_FLAG;
        let desc = self.desc_mut(fd)?;
        desc.flags = (desc.flags & !CHANGEABLE) | (flags & CHANGEABLE);
        Ok(())
    }

    /// Whether `fd` is in non-blocking mode.
    pub fn nonblocking(&self, fd: Fd) -> Result<bool> {
        Ok(self.desc(fd)?.flags & O_NONBLOCK_FLAG != 0)
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
        let Target::Inode(ino) = desc.target else {
            return Err(Error::Invalid);
        };
        match &self.inode(ino).node {
            Node::File(f) => Ok((ino, f, desc)),
            _ => Err(Error::IsDir),
        }
    }

    /// Where a read on `fd` starts: `offset`, or the description's offset.
    /// Fails for descriptors that cannot be read as a file.
    pub fn read_position(&self, fd: Fd, offset: Option<u64>) -> Result<u64> {
        let (_, _, desc) = self.file_of(fd)?;
        if desc.flags & 3 == 1 {
            return Err(Error::BadFd); // O_WRONLY
        }
        Ok(offset.unwrap_or(desc.offset))
    }

    /// Where a write on `fd` starts: `offset`, the end with `O_APPEND`, or the
    /// description's offset. Fails for descriptors not open for writing as a
    /// file.
    pub fn write_position(&self, fd: Fd, offset: Option<u64>) -> Result<u64> {
        if self.desc(fd)?.flags & 3 == 0 {
            return Err(Error::BadFd); // O_RDONLY (directories included)
        }
        let (_, file, desc) = self.file_of(fd)?;
        Ok(match offset {
            Some(at) => at,
            None if desc.flags & O_APPEND_FLAG != 0 => file.size,
            None => desc.offset,
        })
    }

    /// Moves the offset of `fd`'s description (after a read or write that
    /// passed explicit positions).
    pub fn set_offset(&mut self, fd: Fd, offset: u64) -> Result<()> {
        self.desc_mut(fd)?.offset = offset;
        Ok(())
    }

    /// Hands `drain` the file bytes of `fd` from `at`, at most `len` and
    /// within one block (holes read as zeros); returns how many, 0 at the
    /// end, or `None` if `drain` refused them. The offset does not move.
    pub fn read_with(
        &self,
        fd: Fd,
        at: u64,
        len: usize,
        drain: impl FnOnce(&[u8]) -> bool,
    ) -> Result<Option<usize>> {
        const HOLE: Block = [0; BLOCK];
        let (_, file, _) = self.file_of(fd)?;
        let (b, off) = (at as usize / BLOCK, at as usize % BLOCK);
        let n = file
            .size
            .saturating_sub(at)
            .min((BLOCK - off).min(len) as u64) as usize;
        if n == 0 {
            return Ok(Some(0));
        }
        let block = match file.blocks.get(b) {
            Some(Some(block)) => &block[..],
            _ => &HOLE[..],
        };
        Ok(drain(&block[off..off + n]).then_some(n))
    }

    /// Hands `fill` the file bytes of `fd` from `at`, at most `len` and
    /// within one block, to overwrite; returns how many, or `None` if `fill`
    /// failed (then the file is as before). The offset does not move.
    pub fn write_with(
        &mut self,
        fd: Fd,
        at: u64,
        len: usize,
        fill: impl FnOnce(&mut [u8]) -> bool,
    ) -> Result<Option<usize>> {
        let (ino, file, _) = self.file_of(fd)?;
        let (b, off) = (at as usize / BLOCK, at as usize % BLOCK);
        let n = (BLOCK - off).min(len);
        let end = at
            .checked_add(n as u64)
            .filter(|&end| end <= DATA_MAX)
            .ok_or(Error::TooBig)?;
        if n == 0 {
            return Ok(Some(0));
        }
        let new = file.blocks.get(b).is_none_or(Option::is_none);
        if new && self.used >= DATA_MAX / BLOCK as u64 {
            return Err(Error::NoSpace);
        }
        let Node::File(file) = &mut self.inode_mut(ino).node else {
            return Err(Error::IsDir);
        };
        if file.blocks.len() <= b {
            file.blocks.resize(b + 1, None);
        }
        let block = file.blocks[b].get_or_insert_with(|| Arc::new(Box::new([0; BLOCK])));
        if !fill(&mut Arc::make_mut(block)[off..off + n]) {
            if new {
                // Nothing was written: no block, and no block list past the end.
                file.blocks[b] = None;
                file.blocks.truncate((file.size as usize).div_ceil(BLOCK));
            }
            return Ok(None);
        }
        file.size = file.size.max(end);
        if new {
            self.used += 1;
        }
        Ok(Some(n))
    }

    /// `read`/`pread64` into `out`, at `offset` or the description's offset
    /// (which then advances). Returns bytes read (0 at the end).
    pub fn read(&mut self, fd: Fd, out: &mut [u8], offset: Option<u64>) -> Result<usize> {
        let start = self.read_position(fd, offset)?;
        let mut done = 0;
        while done < out.len() {
            let dst = &mut out[done..];
            let got = self.read_with(fd, start + done as u64, dst.len(), |src| {
                dst[..src.len()].copy_from_slice(src);
                true
            })?;
            match got {
                Some(n) if n > 0 => done += n,
                _ => break,
            }
        }
        if offset.is_none() {
            self.set_offset(fd, start + done as u64)?;
        }
        Ok(done)
    }

    /// Writes `data` at `offset` (or at the description's offset, or at the
    /// end with `O_APPEND`); returns bytes written (fewer if the data bound
    /// is reached part way).
    pub fn write(&mut self, fd: Fd, data: &[u8], offset: Option<u64>) -> Result<usize> {
        let start = self.write_position(fd, offset)?;
        let mut done = 0;
        while done < data.len() {
            let src = &data[done..];
            let wrote = self.write_with(fd, start + done as u64, src.len(), |dst| {
                dst.copy_from_slice(&src[..dst.len()]);
                true
            });
            match wrote {
                Ok(Some(n)) => done += n,
                Ok(None) => break,
                Err(e) if done == 0 => return Err(e),
                Err(_) => break,
            }
        }
        if offset.is_none() {
            self.set_offset(fd, start + done as u64)?;
        }
        Ok(done)
    }

    /// `lseek`.
    pub fn seek(&mut self, fd: Fd, offset: i64, whence: Whence) -> Result<u64> {
        let size = match self.desc(fd)?.target {
            Target::Inode(ino) => match &self.inode(ino).node {
                Node::File(f) => f.size,
                _ => 0,
            },
            _ => return Err(Error::Invalid), // ESPIPE on Linux; checked by callers
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
        if size > DATA_MAX {
            return Err(Error::TooBig);
        }
        let Node::File(file) = &mut self.inode_mut(ino).node else {
            return Err(Error::IsDir);
        };
        let blocks = (size as usize).div_ceil(BLOCK);
        let dropped = file
            .blocks
            .get(blocks..)
            .map_or(0, |cut| cut.iter().filter(|b| b.is_some()).count() as u64);
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
        self.used -= dropped;
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
                _ => inode.links,
            },
            size: match &inode.node {
                Node::File(f) => f.size,
                Node::Dir { .. } => BLOCK as u64,
                Node::Socket(_) => 0,
            },
        }
    }

    /// `fstat` (`None` for the console). Objects other than inodes report
    /// their kind and a number of their own.
    pub fn fstat(&self, fd: Fd) -> Result<Option<Stat>> {
        let object = |index: u32, kind| {
            Some(Stat {
                ino: (1 << 32) + u64::from(index),
                kind,
                links: 1,
                size: 0,
            })
        };
        Ok(match self.desc(fd)?.target {
            Target::Console => None,
            Target::Inode(ino) => Some(self.stat_of(ino)),
            Target::Pipe(i, _) => object(i, Kind::Pipe),
            Target::Event(i) => object(i, Kind::Event),
            Target::Epoll(i) => object(i, Kind::Epoll),
            Target::Socket(h) => object(h, Kind::Socket),
        })
    }

    /// The regular file `fd` is open on, and its size, for mapping it.
    pub fn file(&self, fd: Fd) -> Result<(Ino, u64)> {
        let (ino, file, _) = self.file_of(fd)?;
        Ok((ino, file.size))
    }

    /// The regular file `path` names, and its size (to start a program
    /// from it).
    pub fn file_at(&self, path: &[u8]) -> Result<(Ino, u64)> {
        let ino = self.lookup(None, path)?;
        match &self.inode(ino).node {
            Node::File(f) => Ok((ino, f.size)),
            Node::Dir { .. } => Err(Error::IsDir),
            Node::Socket(_) => Err(Error::NoDevice),
        }
    }

    /// Page `index` (of [`BLOCK`] bytes) of regular file `ino`, as it is
    /// now; [`Error::BadFd`] if `ino` is no regular file.
    pub fn page(&self, ino: Ino, index: u64) -> Result<Page> {
        let Some(Node::File(file)) = self
            .inodes
            .get(ino as usize)
            .and_then(Option::as_deref)
            .map(|i| &i.node)
        else {
            return Err(Error::BadFd);
        };
        let at = index.saturating_mul(BLOCK as u64);
        let len = file.size.saturating_sub(at).min(BLOCK as u64) as usize;
        let block = usize::try_from(index)
            .ok()
            .and_then(|i| file.blocks.get(i))
            .and_then(Clone::clone)
            .filter(|_| len > 0);
        Ok(Page { block, len })
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
        })?;
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
            (Node::File(_) | Node::Socket(_), true) => return Err(Error::NotDir),
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
                (Node::File(_) | Node::Socket(_), true) => return Err(Error::NotDir),
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
        let Target::Inode(ino) = desc.target else {
            return Err(Error::NotDir);
        };
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

    /// Bytes of file data this cell holds, blocks it shares with a fork
    /// included (what [`DATA_MAX`] limits).
    pub fn data_bytes(&self) -> u64 {
        self.used * BLOCK as u64
    }
}

fn kind_of(inode: &Inode) -> Kind {
    match inode.node {
        Node::Dir { .. } => Kind::Dir,
        Node::File(_) => Kind::File,
        Node::Socket(_) => Kind::Socket,
    }
}

#[cfg(test)]
mod io_tests;
#[cfg(test)]
mod tests;
