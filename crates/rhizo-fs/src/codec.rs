//! Encoding of a cell's file system for its snapshot (design D3): explicit
//! little endian, every index checked on decode so a corrupt snapshot is
//! refused instead of trusted.
//!
//! Layout: `cwd: u32`, then the inode table (`count: u32`, per slot a
//! present byte, then `kind: u8` (0 directory, 1 file, 2 socket, 3 entry
//! of `/run/rhizo`, 4 the directory `/run/rhizo`), `links: u32`, and for a
//! directory (either kind) `parent: u32`, `entries: u32`, each `len: u16,
//! name, ino: u32`; for a file `size: u64`, `blocks: u32`, each a present
//! byte and 4096 bytes when present; for a socket `name: u64`; for an entry
//! of `/run/rhizo` `tag: u8`, 0 a socket then its index `u32` among the
//! socket records, as a description's, 1 the inbox, 2 the copy
//! generation),
//! the descriptions (`count: u32`, per slot a present byte, then the target
//! (`tag: u8`: 0 console, 1 inode, 2 pipe read end, 3 pipe write end,
//! 4 event counter, 5 `epoll`, 6 socket; then `index: u32` but for the
//! console), `offset: u64`, `flags: u32`, `refs: u32`), the descriptors
//! (`count: u32`, per slot a present byte, then `desc: u32`, `cloexec: u8`)
//! and the objects (`count: u32`, per slot a present byte, then `kind: u8`:
//! 0 pipe, `readers: u32`, `writers: u32`, `len: u32`, bytes; 1 event
//! counter, `count: u64`, `semaphore: u8`; 2 `epoll`, `entries: u32`, each
//! `fd: u32`, `events: u32`, `data: u64`), then the host's input pipe
//! (`u32`: the object index, `u32::MAX` for none; design D1 of
//! `add-interactive-chat`). A socket target's index is the kernel's
//! business (a snapshot's socket record). A cell handle of `/run/rhizo`
//! (a grant) is not encoded: grants carry no authority across a snapshot
//! (design D13 of `add-cell-hierarchy`), so its name and inode are left
//! out.

use super::*;

/// Why encoded file system bytes are refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// The bytes end early or have bytes left over.
    Truncated,
    /// A field is out of range or inconsistent with another.
    Corrupt,
}

use io::Object;

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn take(&mut self, n: usize) -> core::result::Result<&[u8], DecodeError> {
        if self.0.len() < n {
            return Err(DecodeError::Truncated);
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }
    fn u8(&mut self) -> core::result::Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }
    fn flag(&mut self) -> core::result::Result<bool, DecodeError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(DecodeError::Corrupt),
        }
    }
    fn u16(&mut self) -> core::result::Result<u16, DecodeError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> core::result::Result<u32, DecodeError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> core::result::Result<u64, DecodeError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    /// A slot count, bounded by the bytes left (each slot takes at least one).
    fn count(&mut self) -> core::result::Result<usize, DecodeError> {
        let n = self.u32()? as usize;
        if n > self.0.len() {
            return Err(DecodeError::Truncated);
        }
        Ok(n)
    }
}

impl Fs {
    /// Decodes a file system encoded by [`Tables::encode`]; `bytes` must
    /// hold exactly one. Refuses anything that would break an invariant.
    pub fn decode(bytes: &[u8]) -> core::result::Result<Self, DecodeError> {
        let tables = Tables::decode(bytes)?;
        if tables.consistent() {
            Ok(Fs(Arc::new(tables)))
        } else {
            Err(DecodeError::Corrupt)
        }
    }

    /// [`Fs::decode`] without the consistency check, for bytes `decode`
    /// already accepted (a validated snapshot): resuming a cell skips the
    /// second check. Bytes that were never checked may give a file system
    /// whose operations panic.
    pub fn decode_trusted(bytes: &[u8]) -> core::result::Result<Self, DecodeError> {
        Ok(Fs(Arc::new(Tables::decode_with(bytes, None)?)))
    }

    /// [`Fs::decode_trusted`] sharing with `base` every inode encoded the
    /// same at the same number: a cell's files resumed from a snapshot
    /// share what they still have of the tree they started from (its
    /// `/run/rhizo` among them) instead of holding copies, as forks share
    /// them.
    pub fn decode_trusted_sharing(
        bytes: &[u8],
        base: &Base,
    ) -> core::result::Result<Self, DecodeError> {
        Ok(Fs(Arc::new(Tables::decode_with(bytes, Some(base))?)))
    }
}

/// Files cells start from, their encoding read once: decoding a cell's
/// files against them shares what the cell kept of them
/// ([`Fs::decode_trusted_sharing`]).
#[derive(Debug, Clone)]
pub struct Base {
    fs: Fs,
    bytes: Vec<u8>,
    /// Where each slot of the inode table of `bytes` ends.
    slots: Vec<usize>,
}

/// Where the inode table of an encoding starts: after `cwd` and its count.
const TABLE: usize = 8;

impl Base {
    pub fn new(fs: Fs) -> Self {
        let mut bytes = Vec::new();
        fs.encode(&mut bytes);
        let mut slots = Vec::new();
        let mut r = Reader(&bytes);
        if r.u32().is_ok()
            && let Ok(n) = r.count()
        {
            for _ in 0..n {
                if skip_inode(&mut r).is_err() {
                    break;
                }
                slots.push(bytes.len() - r.0.len());
            }
        }
        Self { fs, bytes, slots }
    }

    /// The files.
    pub fn fs(&self) -> &Fs {
        &self.fs
    }

    /// Their encoding.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The encoding of inode slot `i`, if the base has it.
    fn slot(&self, i: usize) -> Option<&[u8]> {
        let start = match i {
            0 => TABLE,
            _ => *self.slots.get(i - 1)?,
        };
        self.bytes.get(start..*self.slots.get(i)?)
    }

    /// Inode slot `i` shared, if `mine` starts with its encoding (a slot's
    /// encoding delimits itself: the same bytes, the same slot), with that
    /// encoding's length.
    fn same(&self, i: usize, mine: &[u8]) -> Option<(usize, Option<Arc<Inode>>)> {
        let theirs = self.slot(i)?;
        if !mine.starts_with(theirs) {
            return None;
        }
        match (theirs.first(), self.fs.inodes.get(i)) {
            (Some(0), _) => Some((theirs.len(), None)),
            (Some(_), Some(Some(inode))) => Some((theirs.len(), Some(inode.clone()))),
            _ => None,
        }
    }
}

/// Moves `r` past one slot of the inode table, present or not, allocating
/// nothing.
fn skip_inode(r: &mut Reader) -> core::result::Result<(), DecodeError> {
    if !r.flag()? {
        return Ok(());
    }
    let kind = r.u8()?;
    r.u32()?; // links
    match kind {
        0 | 4 => {
            r.u32()?; // parent
            for _ in 0..r.count()? {
                let len = r.u16()? as usize;
                r.take(len + 4)?;
            }
        }
        1 => {
            r.u64()?; // size
            for _ in 0..r.count()? {
                if r.flag()? {
                    r.take(BLOCK)?;
                }
            }
        }
        2 => {
            r.u64()?;
        }
        3 => {
            if r.u8()? == 0 {
                r.u32()?;
            }
        }
        _ => return Err(DecodeError::Corrupt),
    }
    Ok(())
}

impl Tables {
    /// Appends the encoding of this file system to `out`.
    pub fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.cwd.to_le_bytes());
        out.extend_from_slice(&(self.inodes.len() as u32).to_le_bytes());
        let grant = |ino: &Ino| {
            matches!(
                self.inodes[*ino as usize].as_deref(),
                Some(Inode {
                    node: Node::Kept(Kept::Grant(_)),
                    ..
                })
            )
        };
        for (i, slot) in self.inodes.iter().enumerate() {
            let Some(inode) = slot.as_deref().filter(|_| !grant(&(i as Ino))) else {
                out.push(0);
                continue;
            };
            out.push(1);
            match &inode.node {
                Node::Dir { entries, parent } => {
                    out.push(if self.rhizo == Some(i as Ino) { 4 } else { 0 });
                    out.extend_from_slice(&inode.links.to_le_bytes());
                    out.extend_from_slice(&parent.to_le_bytes());
                    let kept = entries.iter().filter(|(_, ino)| !grant(ino));
                    out.extend_from_slice(&(kept.clone().count() as u32).to_le_bytes());
                    for (name, ino) in kept {
                        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
                        out.extend_from_slice(name);
                        out.extend_from_slice(&ino.to_le_bytes());
                    }
                }
                Node::Kept(kept) => {
                    out.push(3);
                    out.extend_from_slice(&inode.links.to_le_bytes());
                    match kept {
                        Kept::Socket(h) => {
                            out.push(0);
                            out.extend_from_slice(&h.to_le_bytes());
                        }
                        Kept::Inbox => out.push(1),
                        Kept::Generation => out.push(2),
                        Kept::Grant(_) => unreachable!("grants are left out"),
                    }
                }
                Node::Socket(name) => {
                    out.push(2);
                    out.extend_from_slice(&inode.links.to_le_bytes());
                    out.extend_from_slice(&name.to_le_bytes());
                }
                Node::File(file) => {
                    out.push(1);
                    out.extend_from_slice(&inode.links.to_le_bytes());
                    out.extend_from_slice(&file.size.to_le_bytes());
                    out.extend_from_slice(&(file.blocks.len() as u32).to_le_bytes());
                    for block in &file.blocks {
                        match block {
                            Some(data) => {
                                out.push(1);
                                out.extend_from_slice(&data[..]);
                            }
                            None => out.push(0),
                        }
                    }
                }
            }
        }
        out.extend_from_slice(&(self.descs.len() as u32).to_le_bytes());
        for slot in &self.descs {
            let Some(d) = slot else {
                out.push(0);
                continue;
            };
            out.push(1);
            let (tag, index) = match d.target {
                Target::Console => (0, None),
                Target::Inode(i) => (1, Some(i)),
                Target::Pipe(i, false) => (2, Some(i)),
                Target::Pipe(i, true) => (3, Some(i)),
                Target::Event(i) => (4, Some(i)),
                Target::Epoll(i) => (5, Some(i)),
                Target::Socket(h) => (6, Some(h)),
            };
            out.push(tag);
            if let Some(i) = index {
                out.extend_from_slice(&i.to_le_bytes());
            }
            out.extend_from_slice(&d.offset.to_le_bytes());
            out.extend_from_slice(&d.flags.to_le_bytes());
            out.extend_from_slice(&d.refs.to_le_bytes());
        }
        out.extend_from_slice(&(self.fds.len() as u32).to_le_bytes());
        for slot in &self.fds {
            match slot {
                Some((desc, cloexec)) => {
                    out.push(1);
                    out.extend_from_slice(&desc.to_le_bytes());
                    out.push(*cloexec as u8);
                }
                None => out.push(0),
            }
        }
        out.extend_from_slice(&(self.objects.len() as u32).to_le_bytes());
        for slot in &self.objects {
            let Some(object) = slot else {
                out.push(0);
                continue;
            };
            out.push(1);
            match &**object {
                Object::Pipe {
                    data,
                    readers,
                    writers,
                } => {
                    out.push(0);
                    out.extend_from_slice(&readers.to_le_bytes());
                    out.extend_from_slice(&writers.to_le_bytes());
                    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
                    let (a, b) = data.as_slices();
                    out.extend_from_slice(a);
                    out.extend_from_slice(b);
                }
                Object::Event { count, semaphore } => {
                    out.push(1);
                    out.extend_from_slice(&count.to_le_bytes());
                    out.push(*semaphore as u8);
                }
                Object::Epoll(list) => {
                    out.push(2);
                    out.extend_from_slice(&(list.len() as u32).to_le_bytes());
                    for e in list {
                        out.extend_from_slice(&e.fd.to_le_bytes());
                        out.extend_from_slice(&e.events.to_le_bytes());
                        out.extend_from_slice(&e.data.to_le_bytes());
                    }
                }
            }
        }
        out.extend_from_slice(&self.input.unwrap_or(u32::MAX).to_le_bytes());
    }

    fn decode(bytes: &[u8]) -> core::result::Result<Self, DecodeError> {
        Self::decode_with(bytes, None)
    }

    /// [`decode`](Self::decode), taking from `base` (shared) each inode
    /// its encoding has the same at the same number.
    fn decode_with(bytes: &[u8], base: Option<&Base>) -> core::result::Result<Self, DecodeError> {
        let mut r = Reader(bytes);
        let cwd = r.u32()?;
        let count = r.count()?;
        let mut inodes = Vec::with_capacity(count);
        let mut rhizo = None;
        let mut kept_sockets = 0;
        // The whole inode table as the base's (its count included): shared
        // at once, with what it counts.
        let whole = base.filter(|b| {
            let end = b.slots.last().copied().unwrap_or(0);
            end > TABLE
                && b.slots.len() == b.fs.inodes.len()
                && bytes.get(4..end) == b.bytes.get(4..end)
        });
        if let Some(b) = whole {
            inodes.extend(b.fs.inodes.iter().cloned());
            (rhizo, kept_sockets) = (b.fs.rhizo, b.fs.kept_sockets);
            r.0 = &bytes[b.slots.last().copied().unwrap_or(TABLE)..];
        }
        for i in (0..count).filter(|_| whole.is_none()) {
            if let Some(b) = base
                && let Some((len, shared)) = b.same(i, r.0)
            {
                // Its kind (after the present byte): the directory
                // `/run/rhizo`.
                if r.0.get(1) == Some(&4) && len > 1 {
                    if rhizo.is_some() {
                        return Err(DecodeError::Corrupt);
                    }
                    rhizo = Some(i as Ino);
                }
                r.0 = &r.0[len..];
                if shared
                    .as_ref()
                    .is_some_and(|s| matches!(s.node, Node::Kept(Kept::Socket(_))))
                {
                    kept_sockets += 1;
                }
                inodes.push(shared);
                continue;
            }
            if !r.flag()? {
                inodes.push(None);
                continue;
            }
            let kind = r.u8()?;
            let links = r.u32()?;
            let node = match kind {
                4 if rhizo.is_some() => return Err(DecodeError::Corrupt),
                0 | 4 => {
                    if kind == 4 {
                        rhizo = Some(i as Ino);
                    }
                    let parent = r.u32()?;
                    let mut entries = BTreeMap::new();
                    for _ in 0..r.count()? {
                        let len = r.u16()? as usize;
                        let name = r.take(len)?;
                        let bad = name.is_empty()
                            || len > NAME_MAX
                            || name.contains(&b'/')
                            || name.contains(&0)
                            || name == b"."
                            || name == b"..";
                        if bad {
                            return Err(DecodeError::Corrupt);
                        }
                        let name = name.to_vec();
                        if entries.insert(name, r.u32()?).is_some() {
                            return Err(DecodeError::Corrupt);
                        }
                    }
                    Node::Dir { entries, parent }
                }
                1 => {
                    let size = r.u64()?;
                    let mut blocks = Vec::new();
                    for _ in 0..r.count()? {
                        blocks.push(if r.flag()? {
                            let data: &Block = r.take(BLOCK)?.try_into().unwrap();
                            Some(Arc::new(Box::new(*data)))
                        } else {
                            None
                        });
                    }
                    if size > DATA_MAX || blocks.len() as u64 > size.div_ceil(BLOCK as u64) {
                        return Err(DecodeError::Corrupt);
                    }
                    Node::File(File {
                        size,
                        blocks,
                        public: false,
                    })
                }
                2 => Node::Socket(r.u64()?),
                3 => Node::Kept(match r.u8()? {
                    0 => Kept::Socket(r.u32()?),
                    1 => Kept::Inbox,
                    2 => Kept::Generation,
                    _ => return Err(DecodeError::Corrupt),
                }),
                _ => return Err(DecodeError::Corrupt),
            };
            if let Node::Kept(Kept::Socket(_)) = node {
                kept_sockets += 1;
            }
            inodes.push(Some(Arc::new(Inode { links, node })));
        }
        let count = r.count()?;
        let mut descs = Vec::with_capacity(count);
        for _ in 0..count {
            if !r.flag()? {
                descs.push(None);
                continue;
            }
            let target = match r.u8()? {
                0 => Target::Console,
                1 => Target::Inode(r.u32()?),
                2 => Target::Pipe(r.u32()?, false),
                3 => Target::Pipe(r.u32()?, true),
                4 => Target::Event(r.u32()?),
                5 => Target::Epoll(r.u32()?),
                6 => Target::Socket(r.u32()?),
                _ => return Err(DecodeError::Corrupt),
            };
            descs.push(Some(Desc {
                target,
                offset: r.u64()?,
                flags: r.u32()?,
                refs: r.u32()?,
            }));
        }
        let count = r.count()?;
        let mut fds = Vec::with_capacity(count);
        for _ in 0..count {
            fds.push(if r.flag()? {
                Some((r.u32()?, r.flag()?))
            } else {
                None
            });
        }
        let count = r.count()?;
        let mut objects = Vec::with_capacity(count);
        for _ in 0..count {
            if !r.flag()? {
                objects.push(None);
                continue;
            }
            let object = match r.u8()? {
                0 => {
                    let (readers, writers) = (r.u32()?, r.u32()?);
                    let len = r.u32()? as usize;
                    if len > io::PIPE_CAP {
                        return Err(DecodeError::Corrupt);
                    }
                    let data = r.take(len)?.iter().copied().collect();
                    Object::Pipe {
                        data,
                        readers,
                        writers,
                    }
                }
                1 => Object::Event {
                    count: r.u64()?,
                    semaphore: r.flag()?,
                },
                2 => {
                    let mut list = Vec::new();
                    for _ in 0..r.count()? {
                        list.push(Interest {
                            fd: r.u32()?,
                            events: r.u32()?,
                            data: r.u64()?,
                        });
                    }
                    Object::Epoll(list)
                }
                _ => return Err(DecodeError::Corrupt),
            };
            objects.push(Some(Arc::new(object)));
        }
        let input = Some(r.u32()?).filter(|&i| i != u32::MAX);
        if !r.0.is_empty() {
            return Err(DecodeError::Truncated);
        }
        let (used, names) = match whole {
            Some(b) => (b.fs.used, b.fs.names),
            None => (
                inodes
                    .iter()
                    .flatten()
                    .map(|i| match &i.node {
                        Node::File(f) => f.held(),
                        _ => 0,
                    })
                    .sum(),
                inodes
                    .iter()
                    .flatten()
                    .map(|i| match &i.node {
                        Node::Dir { entries, .. } => {
                            entries.keys().map(|n| NAME_COST + n.len() as u64).sum()
                        }
                        _ => 0,
                    })
                    .sum(),
            ),
        };
        let socket_descs = descs
            .iter()
            .flatten()
            .filter(|d| matches!(d.target, Target::Socket(_)))
            .count() as u32;
        Ok(Tables {
            inodes,
            descs,
            fds,
            cwd,
            used,
            names,
            socket_descs,
            objects,
            released: Vec::new(),
            fd_limit: FD_MAX,
            socket_limit: crate::SOCKETS_DEFAULT,
            input,
            rhizo,
            dropped: Vec::new(),
            kept_sockets,

            changes: 0,
        })
    }

    /// Whether the tables satisfy every invariant the operations rely on.
    fn consistent(&self) -> bool {
        let is_dir = |ino: Ino| {
            matches!(
                self.inodes.get(ino as usize),
                Some(Some(i)) if matches!(i.node, Node::Dir { .. })
            )
        };
        if !is_dir(ROOT)
            || !is_dir(self.cwd)
            || self.rhizo == Some(ROOT)
            || self.fds.len() > FD_MAX
            || self.used > DATA_MAX / BLOCK as u64
        {
            return false;
        }
        // Names: each directory other than the root has exactly one, in its
        // parent; a file has as many as its link count.
        let mut names = vec![0u32; self.inodes.len()];
        for (ino, slot) in self.inodes.iter().enumerate() {
            let Some(Node::Dir { entries, parent }) = slot.as_deref().map(|i| &i.node) else {
                continue;
            };
            if !is_dir(*parent) {
                return false;
            }
            for &child in entries.values() {
                let Some(Some(c)) = self.inodes.get(child as usize) else {
                    return false;
                };
                // Only the kernel's directories hold its entries.
                if matches!(c.node, Node::Kept(_)) && !self.kept_dir(ino as Ino) {
                    return false;
                }
                if let Node::Dir { parent, .. } = c.node
                    && parent != ino as Ino
                {
                    return false;
                }
                names[child as usize] += 1;
            }
        }
        let mut open = vec![false; self.inodes.len()];
        let mut ends = vec![[0u32; 2]; self.objects.len()];
        let mut refs = vec![0u32; self.descs.len()];
        for &(d, _) in self.fds.iter().flatten() {
            match refs.get_mut(d as usize) {
                Some(r) if self.descs[d as usize].is_some() => *r += 1,
                _ => return false,
            }
        }
        for (d, slot) in self.descs.iter().enumerate() {
            let Some(desc) = slot else { continue };
            if desc.refs == 0 || desc.refs != refs[d] {
                return false;
            }
            match desc.target {
                Target::Inode(ino) => match self.inodes.get(ino as usize) {
                    Some(Some(i)) if !matches!(i.node, Node::Socket(_) | Node::Kept(_)) => {
                        open[ino as usize] = true;
                    }
                    _ => return false,
                },
                Target::Pipe(i, write) => match ends.get_mut(i as usize) {
                    Some(e) => e[write as usize] += 1,
                    None => return false,
                },
                Target::Event(i) | Target::Epoll(i) => match ends.get_mut(i as usize) {
                    Some(e) => e[0] += 1,
                    None => return false,
                },
                Target::Console | Target::Socket(_) => {}
            }
            let kind_ok = match (desc.target, self.objects.get(desc_object(desc.target))) {
                (Target::Pipe(..), Some(Some(o))) => matches!(**o, Object::Pipe { .. }),
                (Target::Event(_), Some(Some(o))) => matches!(**o, Object::Event { .. }),
                (Target::Epoll(_), Some(Some(o))) => matches!(**o, Object::Epoll(_)),
                (Target::Pipe(..) | Target::Event(_) | Target::Epoll(_), _) => false,
                _ => true,
            };
            if !kind_ok {
                return false;
            }
        }
        // The host's input is a pipe, its one write end outside the
        // descriptions.
        if let Some(i) = self.input {
            match ends.get_mut(i as usize) {
                Some(e) => e[1] += 1,
                None => return false,
            }
            if !matches!(
                self.objects[i as usize].as_deref(),
                Some(Object::Pipe { .. })
            ) {
                return false;
            }
        }
        // Each object is held by its descriptions (and the host), and only
        // by them.
        for (i, slot) in self.objects.iter().enumerate() {
            let held = ends[i];
            let ok = match slot.as_deref() {
                None => held == [0, 0],
                Some(Object::Pipe {
                    data,
                    readers,
                    writers,
                }) => {
                    [*readers, *writers] == held
                        && readers + writers > 0
                        && data.len() <= io::PIPE_CAP
                }
                Some(Object::Event { count, .. }) => held == [1, 0] && *count < u64::MAX,
                Some(Object::Epoll(list)) => {
                    held == [1, 0]
                        && list.iter().all(|e| (e.fd as usize) < FD_MAX)
                        && list
                            .iter()
                            .enumerate()
                            .all(|(n, e)| list[..n].iter().all(|o| o.fd != e.fd))
                }
            };
            if !ok {
                return false;
            }
        }
        // Every inode is reachable by name or held open; a name count matches.
        for (ino, slot) in self.inodes.iter().enumerate() {
            let Some(inode) = slot else { continue };
            let root = ino as Ino == ROOT;
            let expected = if root { 0 } else { inode.links };
            let links_ok = match inode.node {
                Node::Dir { .. } => inode.links == u32::from(root) || (!root && inode.links <= 1),
                Node::File(_) => true,
                Node::Socket(_) | Node::Kept(_) => !root,
            };
            if names[ino] != expected || !links_ok || (names[ino] == 0 && !root && !open[ino]) {
                return false;
            }
        }
        // The directory tree has no cycles: walking up reaches the root.
        for ino in 0..self.inodes.len() {
            let mut at = ino as Ino;
            for _ in 0..=self.inodes.len() {
                if at == ROOT {
                    break;
                }
                match self.inodes[at as usize].as_deref().map(|i| &i.node) {
                    Some(Node::Dir { parent, .. }) if names[at as usize] == 1 => at = *parent,
                    _ => break,
                }
            }
            if is_dir(ino as Ino) && names[ino] == 1 && at != ROOT {
                return false;
            }
        }
        true
    }
}

/// The object a description refers to (an out-of-range index for other
/// targets).
fn desc_object(target: Target) -> usize {
    match target {
        Target::Pipe(i, _) | Target::Event(i) | Target::Epoll(i) => i as usize,
        _ => usize::MAX,
    }
}
