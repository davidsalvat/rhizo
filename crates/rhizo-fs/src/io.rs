//! Open objects other than files (design D1 and D5 of
//! `add-sockets-and-poll`): pipes, event counters and `epoll` instances,
//! private to the cell like its files; descriptors of sockets, which live in
//! the kernel and are opaque handles here; and Unix socket names, files of
//! type socket holding a name number.

use super::*;
use alloc::collections::VecDeque;

/// Bytes a pipe holds.
pub const PIPE_CAP: usize = 64 << 10;
/// Writes up to this size go into a pipe whole or not at all.
pub const PIPE_BUF: usize = 4096;

const IN: u32 = 0x1;
const OUT: u32 = 0x4;
const ERR: u32 = 0x8;
const HUP: u32 = 0x10;

/// An object reached only through this cell's descriptors.
#[derive(Debug, Clone)]
pub(crate) enum Object {
    /// Bytes in flight, and how many descriptions hold each end.
    Pipe {
        data: VecDeque<u8>,
        readers: u32,
        writers: u32,
    },
    Event {
        count: u64,
        semaphore: bool,
    },
    Epoll(Vec<Interest>),
}

/// One entry of an `epoll` interest list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Interest {
    pub fd: Fd,
    /// Requested events and flags (`EPOLLONESHOT`, `EPOLLET`) as given.
    pub events: u32,
    pub data: u64,
}

/// What `epoll_ctl` does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EpollOp {
    Add,
    Modify,
    Delete,
}

impl Tables {
    fn alloc_object(&mut self, object: Object) -> u32 {
        let object = Some(Arc::new(object));
        match self.objects.iter().position(Option::is_none) {
            Some(i) => {
                self.objects[i] = object;
                i as u32
            }
            None => {
                self.objects.push(object);
                (self.objects.len() - 1) as u32
            }
        }
    }

    fn object(&self, i: u32) -> &Object {
        self.objects[i as usize]
            .as_deref()
            .expect("objects in use are valid")
    }

    fn object_mut(&mut self, i: u32) -> &mut Object {
        Arc::make_mut(
            self.objects[i as usize]
                .as_mut()
                .expect("objects in use are valid"),
        )
    }

    /// Whether `n` descriptors are free.
    fn fds_free(&self, n: usize) -> bool {
        (0..FD_MAX)
            .filter(|&i| self.fds.get(i).is_none_or(Option::is_none))
            .take(n)
            .count()
            == n
    }

    /// A new description of `target` and the lowest free descriptor for it.
    fn open_target(&mut self, target: Target, flags: u32, cloexec: bool) -> Result<Fd> {
        let desc = self.new_desc(target, flags);
        self.new_fd(desc, 0, cloexec)
            .inspect_err(|_| self.descs[desc as usize] = None)
    }

    fn status(nonblock: bool, access: u32) -> u32 {
        access | if nonblock { O_NONBLOCK_FLAG } else { 0 }
    }

    /// `pipe2`: the read end, then the write end.
    pub fn pipe(&mut self, nonblock: bool, cloexec: bool) -> Result<(Fd, Fd)> {
        if !self.fds_free(2) {
            return Err(Error::TooManyFiles);
        }
        let pipe = self.alloc_object(Object::Pipe {
            data: VecDeque::new(),
            readers: 1,
            writers: 1,
        });
        let read = self.open_target(
            Target::Pipe(pipe, false),
            Self::status(nonblock, 0),
            cloexec,
        )?;
        let write =
            self.open_target(Target::Pipe(pipe, true), Self::status(nonblock, 1), cloexec)?;
        Ok((read, write))
    }

    /// `eventfd2`.
    pub fn eventfd(
        &mut self,
        count: u64,
        semaphore: bool,
        nonblock: bool,
        cloexec: bool,
    ) -> Result<Fd> {
        if !self.fds_free(1) {
            return Err(Error::TooManyFiles);
        }
        let event = self.alloc_object(Object::Event { count, semaphore });
        self.open_target(Target::Event(event), Self::status(nonblock, 2), cloexec)
    }

    /// `epoll_create1`.
    pub fn epoll_create(&mut self, cloexec: bool) -> Result<Fd> {
        if !self.fds_free(1) {
            return Err(Error::TooManyFiles);
        }
        let epoll = self.alloc_object(Object::Epoll(Vec::new()));
        self.open_target(Target::Epoll(epoll), 2, cloexec)
    }

    /// A descriptor for kernel socket `handle`. On failure the caller still
    /// holds the handle.
    pub fn socket_fd(&mut self, handle: u32, nonblock: bool, cloexec: bool) -> Result<Fd> {
        self.open_target(Target::Socket(handle), Self::status(nonblock, 2), cloexec)
    }

    /// The socket handle of `fd`.
    pub fn socket(&self, fd: Fd) -> Result<u32> {
        match self.desc(fd)?.target {
            Target::Socket(h) => Ok(h),
            _ => Err(Error::NotSocket),
        }
    }

    /// The socket handles of every description (each once).
    pub fn sockets(&self) -> impl Iterator<Item = u32> + '_ {
        self.descs.iter().flatten().filter_map(|d| match d.target {
            Target::Socket(h) => Some(h),
            _ => None,
        })
    }

    /// Replaces every socket handle `h` by `f(h)`.
    pub fn map_sockets(&mut self, mut f: impl FnMut(u32) -> u32) {
        for d in self.descs.iter_mut().flatten() {
            if let Target::Socket(h) = &mut d.target {
                *h = f(*h);
            }
        }
    }

    /// Counts changes to pipes, event counters and `epoll` instances: data
    /// in or out, an end closed, an interest changed. Only the cell's own
    /// threads make them, so a thread waiting on one of these waits for
    /// another thread of its cell (design D3 of `add-socket-wait-queues`).
    pub fn changes(&self) -> u64 {
        self.changes
    }

    /// Whether socket handles wait to be released.
    pub fn has_released(&self) -> bool {
        !self.released.is_empty()
    }

    /// The socket handles whose last description closed since the last
    /// call: the caller releases them.
    pub fn take_released(&mut self) -> Vec<u32> {
        core::mem::take(&mut self.released)
    }

    /// One description of `target` (not a file or socket) is gone.
    pub(crate) fn drop_end(&mut self, target: Target) {
        self.changes += 1;
        let i = match target {
            Target::Pipe(i, write) => {
                let Object::Pipe {
                    readers, writers, ..
                } = self.object_mut(i)
                else {
                    unreachable!("pipe descriptions refer to pipes")
                };
                if write {
                    *writers -= 1;
                } else {
                    *readers -= 1;
                }
                if *readers + *writers > 0 {
                    return;
                }
                i
            }
            Target::Event(i) | Target::Epoll(i) => i,
            _ => return,
        };
        self.objects[i as usize] = None;
    }

    /// The pipe and end of `fd`.
    fn pipe_of(&self, fd: Fd) -> Result<(u32, bool)> {
        match self.desc(fd)?.target {
            Target::Pipe(i, write) => Ok((i, write)),
            _ => Err(Error::Invalid),
        }
    }

    /// Reads a pipe: hands `take` up to `len` bytes in pieces, consuming
    /// what it accepts. Empty: 0 once every write end is closed, `Again`
    /// before.
    pub fn read_pipe(
        &mut self,
        fd: Fd,
        len: usize,
        take: &mut dyn FnMut(&[u8]) -> bool,
    ) -> Result<usize> {
        let (i, write) = self.pipe_of(fd)?;
        if write {
            return Err(Error::BadFd);
        }
        let Object::Pipe { data, writers, .. } = self.object(i) else {
            unreachable!("pipe descriptions refer to pipes")
        };
        if data.is_empty() {
            return if *writers == 0 {
                Ok(0)
            } else {
                Err(Error::Again)
            };
        }
        let mut done = 0;
        let (a, b) = data.as_slices();
        for piece in [a, b] {
            let n = piece.len().min(len - done);
            if n == 0 || !take(&piece[..n]) {
                break;
            }
            done += n;
        }
        if done > 0
            && let Object::Pipe { data, .. } = self.object_mut(i)
        {
            data.drain(..done);
            self.changes += 1;
        }
        Ok(done)
    }

    /// Writes a pipe: as many bytes as fit (a write of at most
    /// [`PIPE_BUF`] bytes whole or not at all); `Again` if none fit, `Pipe`
    /// once every read end is closed.
    pub fn write_pipe(&mut self, fd: Fd, bytes: &[u8]) -> Result<usize> {
        let (i, write) = self.pipe_of(fd)?;
        if !write {
            return Err(Error::BadFd);
        }
        let Object::Pipe { data, readers, .. } = self.object(i) else {
            unreachable!("pipe descriptions refer to pipes")
        };
        if *readers == 0 {
            return Err(Error::Pipe);
        }
        let room = PIPE_CAP - data.len();
        if room == 0 || (bytes.len() <= PIPE_BUF && room < bytes.len()) {
            return Err(Error::Again);
        }
        let n = room.min(bytes.len());
        if let Object::Pipe { data, .. } = self.object_mut(i) {
            data.extend(&bytes[..n]);
        }
        self.changes += 1;
        Ok(n)
    }

    fn event_of(&self, fd: Fd) -> Result<u32> {
        match self.desc(fd)?.target {
            Target::Event(i) => Ok(i),
            _ => Err(Error::Invalid),
        }
    }

    /// Reads an event counter: its value (cleared), or 1 (subtracted) for a
    /// semaphore; `Again` at 0.
    pub fn read_event(&mut self, fd: Fd) -> Result<u64> {
        let i = self.event_of(fd)?;
        let Object::Event { count, semaphore } = self.object(i) else {
            unreachable!("event descriptions refer to counters")
        };
        if *count == 0 {
            return Err(Error::Again);
        }
        let take = if *semaphore { 1 } else { *count };
        if let Object::Event { count, .. } = self.object_mut(i) {
            *count -= take;
        }
        self.changes += 1;
        Ok(take)
    }

    /// Adds `value` to an event counter; `Again` if it would pass
    /// `u64::MAX - 1`.
    pub fn write_event(&mut self, fd: Fd, value: u64) -> Result<()> {
        let i = self.event_of(fd)?;
        if value == u64::MAX {
            return Err(Error::Invalid);
        }
        let Object::Event { count, .. } = self.object(i) else {
            unreachable!("event descriptions refer to counters")
        };
        if count.checked_add(value).is_none_or(|c| c > u64::MAX - 1) {
            return Err(Error::Again);
        }
        if let Object::Event { count, .. } = self.object_mut(i) {
            *count += value;
        }
        self.changes += 1;
        Ok(())
    }

    /// The `poll` events of `fd`; `socket` gives those of a socket handle.
    pub fn poll(&self, fd: Fd, socket: &dyn Fn(u32) -> u32) -> Result<u32> {
        Ok(self.poll_target(self.desc(fd)?.target, socket, true))
    }

    fn poll_target(&self, target: Target, socket: &dyn Fn(u32) -> u32, nested: bool) -> u32 {
        match target {
            Target::Console | Target::Inode(_) => IN | OUT,
            Target::Socket(h) => socket(h),
            Target::Pipe(i, write) => {
                let Object::Pipe {
                    data,
                    readers,
                    writers,
                } = self.object(i)
                else {
                    return 0;
                };
                match write {
                    false if *writers == 0 => HUP | if data.is_empty() { 0 } else { IN },
                    false => {
                        if data.is_empty() {
                            0
                        } else {
                            IN
                        }
                    }
                    true if *readers == 0 => OUT | ERR,
                    true => {
                        if data.len() < PIPE_CAP {
                            OUT
                        } else {
                            0
                        }
                    }
                }
            }
            Target::Event(i) => {
                let Object::Event { count, .. } = self.object(i) else {
                    return 0;
                };
                (if *count > 0 { IN } else { 0 }) | if *count < u64::MAX - 1 { OUT } else { 0 }
            }
            // Readable when one of its entries is ready (one level deep).
            Target::Epoll(i) => {
                let Object::Epoll(list) = self.object(i) else {
                    return 0;
                };
                let ready = nested
                    && list.iter().any(|e| {
                        self.desc(e.fd).is_ok_and(|d| {
                            self.poll_target(d.target, socket, false) & (e.events | ERR | HUP) != 0
                        })
                    });
                if ready { IN } else { 0 }
            }
        }
    }

    fn epoll_of(&self, fd: Fd) -> Result<u32> {
        match self.desc(fd)?.target {
            Target::Epoll(i) => Ok(i),
            _ => Err(Error::Invalid),
        }
    }

    /// `epoll_ctl`.
    pub fn epoll_ctl(
        &mut self,
        epfd: Fd,
        op: EpollOp,
        fd: Fd,
        events: u32,
        data: u64,
    ) -> Result<()> {
        let i = self.epoll_of(epfd)?;
        let target = self.desc(fd)?.target;
        if fd == epfd {
            return Err(Error::Invalid);
        }
        if let Target::Inode(_) = target {
            return Err(Error::Perm); // regular files and directories
        }
        self.changes += 1;
        let Object::Epoll(list) = self.object_mut(i) else {
            unreachable!("epoll descriptions refer to epoll instances")
        };
        let at = list.iter().position(|e| e.fd == fd);
        match (op, at) {
            (EpollOp::Add, Some(_)) => Err(Error::Exists),
            (EpollOp::Add, None) => {
                list.push(Interest { fd, events, data });
                Ok(())
            }
            (_, None) => Err(Error::NotFound),
            (EpollOp::Modify, Some(at)) => {
                list[at] = Interest { fd, events, data };
                Ok(())
            }
            (EpollOp::Delete, Some(at)) => {
                list.remove(at);
                Ok(())
            }
        }
    }

    /// The interest list of `epoll` instance `epfd`.
    pub fn epoll_interests(&self, epfd: Fd) -> Result<&[Interest]> {
        let i = self.epoll_of(epfd)?;
        match self.object(i) {
            Object::Epoll(list) => Ok(list),
            _ => Err(Error::Invalid),
        }
    }

    /// Keeps only the flags `keep` of `fd`'s entry in `epfd` (a one-shot
    /// entry after it fired).
    pub fn epoll_mask(&mut self, epfd: Fd, fd: Fd, keep: u32) -> Result<()> {
        let i = self.epoll_of(epfd)?;
        if let Object::Epoll(list) = self.object_mut(i)
            && let Some(e) = list.iter_mut().find(|e| e.fd == fd)
        {
            e.events &= keep;
        }
        Ok(())
    }

    /// Drops `fd` from every interest list (it was closed).
    pub(crate) fn forget_interest(&mut self, fd: Fd) {
        for i in 0..self.objects.len() {
            let listed = matches!(
                self.objects[i].as_deref(),
                Some(Object::Epoll(list)) if list.iter().any(|e| e.fd == fd)
            );
            if listed && let Object::Epoll(list) = self.object_mut(i as u32) {
                list.retain(|e| e.fd != fd);
            }
        }
    }

    /// `bind` of a Unix socket: creates a file of type socket at `path`
    /// holding `name` (`Exists` if the path exists).
    pub fn bind_name(&mut self, dirfd: Option<Fd>, path: &[u8], name: u64) -> Result<()> {
        let (dir, last) = self.parent_of(dirfd, path)?;
        let last = last.ok_or(Error::Exists)?;
        let Node::Dir { entries, .. } = &self.inode(dir).node else {
            return Err(Error::NotDir);
        };
        if entries.contains_key(last) {
            return Err(Error::Exists);
        }
        let ino = self.alloc_inode(Inode {
            links: 1,
            node: Node::Socket(name),
        })?;
        if let Node::Dir { entries, .. } = &mut self.inode_mut(dir).node {
            entries.insert(last.to_vec(), ino);
        }
        Ok(())
    }

    /// The name number of the Unix socket file at `path` (`Refused` if
    /// the file is not a socket).
    pub fn socket_name(&self, dirfd: Option<Fd>, path: &[u8]) -> Result<u64> {
        match self.inode(self.lookup(dirfd, path)?).node {
            Node::Socket(name) if name != 0 => Ok(name),
            _ => Err(Error::Refused),
        }
    }

    /// The name numbers of the Unix socket files.
    pub fn socket_names(&self) -> impl Iterator<Item = u64> + '_ {
        self.inodes.iter().flatten().filter_map(|i| match i.node {
            Node::Socket(name) if name != 0 => Some(name),
            _ => None,
        })
    }

    /// Replaces every socket file's name number `n` by `f(n)`.
    pub fn map_socket_names(&mut self, mut f: impl FnMut(u64) -> u64) {
        for i in 0..self.inodes.len() {
            let name = match self.inodes[i].as_deref() {
                Some(Inode {
                    node: Node::Socket(name),
                    ..
                }) => *name,
                _ => continue,
            };
            let new = f(name);
            if new != name {
                self.inode_mut(i as Ino).node = Node::Socket(new);
            }
        }
    }
}
