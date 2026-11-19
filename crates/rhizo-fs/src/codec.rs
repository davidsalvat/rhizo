//! Encoding of a cell's file system for its snapshot (design D3): explicit
//! little endian, every index checked on decode so a corrupt snapshot is
//! refused instead of trusted.
//!
//! Layout: `cwd: u32`, then the inode table (`count: u32`, per slot a
//! present byte, then `kind: u8` (0 directory, 1 file), `links: u32`, and
//! for a directory `parent: u32`, `entries: u32`, each `len: u16, name,
//! ino: u32`; for a file `size: u64`, `blocks: u32`, each a present byte and
//! 4096 bytes when present), the descriptions (`count: u32`, per slot a
//! present byte, then `target: u32` (`u32::MAX` for the console),
//! `offset: u64`, `flags: u32`, `refs: u32`) and the descriptors (`count:
//! u32`, per slot a present byte, then `desc: u32`, `cloexec: u8`).

use super::*;

/// Why encoded file system bytes are refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// The bytes end early or have bytes left over.
    Truncated,
    /// A field is out of range or inconsistent with another.
    Corrupt,
}

const CONSOLE: u32 = u32::MAX;

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
        Ok(Fs(Arc::new(Tables::decode(bytes)?)))
    }
}

impl Tables {
    /// Appends the encoding of this file system to `out`.
    pub fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.cwd.to_le_bytes());
        out.extend_from_slice(&(self.inodes.len() as u32).to_le_bytes());
        for slot in &self.inodes {
            let Some(inode) = slot else {
                out.push(0);
                continue;
            };
            out.push(1);
            match &inode.node {
                Node::Dir { entries, parent } => {
                    out.push(0);
                    out.extend_from_slice(&inode.links.to_le_bytes());
                    out.extend_from_slice(&parent.to_le_bytes());
                    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
                    for (name, ino) in entries {
                        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
                        out.extend_from_slice(name);
                        out.extend_from_slice(&ino.to_le_bytes());
                    }
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
            out.extend_from_slice(&d.target.unwrap_or(CONSOLE).to_le_bytes());
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
    }

    fn decode(bytes: &[u8]) -> core::result::Result<Self, DecodeError> {
        let mut r = Reader(bytes);
        let cwd = r.u32()?;
        let count = r.count()?;
        let mut inodes = Vec::with_capacity(count);
        for _ in 0..count {
            if !r.flag()? {
                inodes.push(None);
                continue;
            }
            let kind = r.u8()?;
            let links = r.u32()?;
            let node = match kind {
                0 => {
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
                            Some(Arc::new(*data))
                        } else {
                            None
                        });
                    }
                    if size > DATA_MAX || blocks.len() as u64 > size.div_ceil(BLOCK as u64) {
                        return Err(DecodeError::Corrupt);
                    }
                    Node::File(File { size, blocks })
                }
                _ => return Err(DecodeError::Corrupt),
            };
            inodes.push(Some(Arc::new(Inode { links, node })));
        }
        let count = r.count()?;
        let mut descs = Vec::with_capacity(count);
        for _ in 0..count {
            if !r.flag()? {
                descs.push(None);
                continue;
            }
            let target = match r.u32()? {
                CONSOLE => None,
                ino => Some(ino),
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
        if !r.0.is_empty() {
            return Err(DecodeError::Truncated);
        }
        let used = inodes
            .iter()
            .flatten()
            .map(|i| match &i.node {
                Node::File(f) => f.held(),
                Node::Dir { .. } => 0,
            })
            .sum();
        Ok(Tables {
            inodes,
            descs,
            fds,
            cwd,
            used,
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
                if let Node::Dir { parent, .. } = c.node
                    && parent != ino as Ino
                {
                    return false;
                }
                names[child as usize] += 1;
            }
        }
        let mut open = vec![false; self.inodes.len()];
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
            if let Some(ino) = desc.target {
                match self.inodes.get(ino as usize) {
                    Some(Some(_)) => open[ino as usize] = true,
                    _ => return false,
                }
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
