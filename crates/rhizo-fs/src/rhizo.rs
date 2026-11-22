//! `/run/rhizo` (design D9 of `add-cell-hierarchy`): the directory through
//! which a cell uses its handles, kept by the kernel in the cell's own
//! tree, so nothing about it is shared and renaming in it signals nothing.
//!
//! It holds `generation` (the cell's copy generation, design D22), `inbox`
//! and `kernel` once the kernel puts them there, and the directories
//! `chan/` (channel endpoints) and `cell/` (cell handles), each present
//! while it holds an entry. Entries are named by the label the granter
//! chose, unique in their directory. The cell may list, stat, open and
//! connect them and `unlink` an entry of `chan/` or `cell/`, or `kernel`, to
//! drop the handle; creating, renaming or writing anything in the
//! directory fails with `EROFS`, and the directory itself does not move.
//!
//! What an entry stands for is a number the kernel gave ([`Kept`]): a
//! socket of the kernel's, with the reference the entry holds, which the
//! kernel releases once unlinked ([`Tables::take_released`]) like any
//! socket a description held, or a grant, handed back to the kernel when
//! unlinked ([`Tables::take_dropped`]).

use super::*;

/// What an entry of `/run/rhizo` stands for: numbers of the kernel,
/// opaque here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kept {
    /// A socket of the kernel, with one reference the entry holds: a
    /// channel endpoint (`chan/<label>`, `kernel`) or the inbox.
    Socket(u32),
    /// A grant of the kernel's table (`cell/<label>`).
    Grant(u32),
    /// The cell's inbox, which the kernel keeps for it by the cell's
    /// identifier (`inbox`): the same entry in every cell's tree, so trees
    /// shared copy-on-write stay shared.
    Inbox,
    /// The cell's copy generation, 8 bytes the kernel gives when it is
    /// opened (`generation`).
    Generation,
}

impl Kept {
    pub(crate) fn kind(self) -> Kind {
        match self {
            Kept::Socket(_) | Kept::Grant(_) | Kept::Inbox => Kind::Socket,
            Kept::Generation => Kind::File,
        }
    }

    pub(crate) fn size(self) -> u64 {
        match self {
            Kept::Generation => 8,
            _ => 0,
        }
    }
}

/// Where an entry of `/run/rhizo` goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place<'a> {
    /// `chan/<label>`.
    Chan(&'a [u8]),
    /// `cell/<label>`.
    Cell(&'a [u8]),
    /// `inbox`.
    Inbox,
    /// `kernel`.
    Kernel,
}

impl Place<'_> {
    /// The subdirectory and name of the entry.
    fn split(&self) -> (Option<&'static [u8]>, &[u8]) {
        match *self {
            Place::Chan(label) => (Some(b"chan"), label),
            Place::Cell(label) => (Some(b"cell"), label),
            Place::Inbox => (None, b"inbox"),
            Place::Kernel => (None, b"kernel"),
        }
    }
}

const GENERATION: &[u8] = b"generation";

impl Tables {
    /// Whether directory `dir` is kept by the kernel: `/run/rhizo`, or one
    /// of its subdirectories (only the kernel makes them).
    pub(crate) fn kept_dir(&self, dir: Ino) -> bool {
        let Some(rhizo) = self.rhizo else {
            return false;
        };
        dir == rhizo || matches!(self.inode(dir).node, Node::Dir { parent, .. } if parent == rhizo)
    }

    /// The inodes named in `/run/rhizo` and its subdirectories (none
    /// before it exists): only there are entries of the kernel, so finding
    /// them never walks the cell's own files.
    pub(crate) fn kept_inos(&self) -> impl Iterator<Item = Ino> + '_ {
        self.kept_inos_in(self.rhizo)
    }

    /// [`kept_inos`](Self::kept_inos) under `rhizo`, `/run/rhizo` or none.
    pub(crate) fn kept_inos_in(&self, rhizo: Option<Ino>) -> impl Iterator<Item = Ino> + '_ {
        let names = move |dir: Ino| {
            match &self.inode(dir).node {
                Node::Dir { entries, .. } => Some(entries.values().copied()),
                _ => None,
            }
            .into_iter()
            .flatten()
        };
        rhizo.into_iter().flat_map(move |rhizo| {
            names(rhizo).flat_map(move |i| {
                let sub = matches!(self.inode(i).node, Node::Dir { .. });
                core::iter::once(i).chain(sub.then(|| names(i)).into_iter().flatten())
            })
        })
    }

    /// Whether `ino` is `/run/rhizo` or one of the directories above it.
    pub(crate) fn holds_rhizo(&self, ino: Ino) -> bool {
        let Some(mut at) = self.rhizo else {
            return false;
        };
        loop {
            if at == ino {
                return true;
            }
            match self.inode(at).node {
                Node::Dir { parent, .. } if at != ROOT && parent != at => at = parent,
                _ => return false,
            }
        }
    }

    /// `unlinkat` of `ino` in kept directory `dir` (or of `/run/rhizo`
    /// itself): an entry of `chan/` or `cell/`, or `kernel`, is dropped;
    /// anything else is refused.
    pub(crate) fn unlink_kept(&mut self, dir: Ino, ino: Ino, rmdir: bool) -> Result<()> {
        let droppable = match self.inode(ino).node {
            Node::Kept(Kept::Generation) => false,
            Node::Kept(_) => Some(dir) != self.rhizo || self.name_in(dir, ino) == Some(b"kernel"),
            Node::Dir { .. } if Some(ino) == self.rhizo => return Err(Error::Busy),
            _ => false,
        };
        if !droppable {
            return Err(Error::ReadOnly);
        }
        if rmdir {
            return Err(Error::NotDir);
        }
        let Some(name) = self.name_in(dir, ino).map(<[u8]>::to_vec) else {
            return Err(Error::NotFound);
        };
        self.remove_kept(dir, &name);
        Ok(())
    }

    /// The name of `ino` in directory `dir`.
    fn name_in(&self, dir: Ino, ino: Ino) -> Option<&[u8]> {
        let Node::Dir { entries, .. } = &self.inode(dir).node else {
            return None;
        };
        entries
            .iter()
            .find(|&(_, &i)| i == ino)
            .map(|(n, _)| n.as_slice())
    }

    /// Removes entry `name` of kept directory `dir` and hands what it held
    /// to the kernel; a subdirectory left empty goes too.
    fn remove_kept(&mut self, dir: Ino, name: &[u8]) -> Option<Kept> {
        let Node::Dir { entries, .. } = &mut self.inode_mut(dir).node else {
            return None;
        };
        let ino = entries.remove(name)?;
        let kept = match self.inode(ino).node {
            Node::Kept(k) => k,
            _ => unreachable!("kept directories hold kept entries"),
        };
        self.inodes[ino as usize] = None;
        match kept {
            Kept::Socket(h) => {
                self.kept_sockets -= 1;
                self.released.push(h);
            }
            Kept::Grant(g) => self.dropped.push((g, name.to_vec())),
            Kept::Inbox | Kept::Generation => {}
        }
        let empty =
            matches!(&self.inode(dir).node, Node::Dir { entries, .. } if entries.is_empty());
        if Some(dir) != self.rhizo && empty && dir != self.cwd {
            let rhizo = self.rhizo.expect("a kept subdirectory is in /run/rhizo");
            if let Node::Dir { entries, .. } = &mut self.inode_mut(rhizo).node {
                entries.retain(|_, &mut i| i != dir);
            }
            let inode = self.inode_mut(dir);
            inode.links = 0;
            if let Node::Dir { parent, .. } = &mut inode.node {
                *parent = dir;
            }
            self.maybe_free(dir);
        }
        Some(kept)
    }

    /// Makes `/run/rhizo` with its `generation` (and `/run` if missing),
    /// taking over a directory already at that path (a cell's tree
    /// restored from a snapshot). The kernel calls it before the cell runs.
    pub fn rhizo_dir(&mut self) -> Result<()> {
        if self.rhizo.is_some() {
            return Ok(());
        }
        for path in [&b"/run"[..], b"/run/rhizo"] {
            match self.mkdir(None, path) {
                Ok(()) | Err(Error::Exists) => {}
                Err(e) => return Err(e),
            }
        }
        let rhizo = self.lookup(None, b"/run/rhizo")?;
        let Node::Dir { entries, .. } = &self.inode(rhizo).node else {
            return Err(Error::NotDir);
        };
        if !entries.contains_key(GENERATION) {
            self.add_entry(rhizo, GENERATION, Kept::Generation)?;
        }
        self.rhizo = Some(rhizo);
        Ok(())
    }

    /// A new entry `name` holding `kept` in directory `dir`.
    fn add_entry(&mut self, dir: Ino, name: &[u8], kept: Kept) -> Result<()> {
        let ino = self.alloc_inode(Inode {
            links: 1,
            node: Node::Kept(kept),
        })?;
        if let Node::Dir { entries, .. } = &mut self.inode_mut(dir).node {
            entries.insert(name.to_vec(), ino);
        }
        if let Kept::Socket(_) = kept {
            self.kept_sockets += 1;
        }
        Ok(())
    }

    /// Whether `/run/rhizo` exists ([`rhizo_dir`](Self::rhizo_dir)).
    pub fn has_rhizo(&self) -> bool {
        self.rhizo.is_some()
    }

    /// The kernel puts an entry holding `kept` at `place` (making
    /// `/run/rhizo` and the subdirectory if needed). Refused: a label that
    /// is empty, `.`, `..` or holds `/` or NUL (`Invalid`), a place already
    /// taken (`Exists`), [`Kept::Generation`], or [`Kept::Inbox`] anywhere
    /// but at [`Place::Inbox`] (`Invalid`). On failure the caller still
    /// holds what `kept` names.
    pub fn rhizo_put(&mut self, place: Place<'_>, kept: Kept) -> Result<()> {
        let (sub, name) = place.split();
        let bad = name.is_empty()
            || name.len() > NAME_MAX
            || name == b"."
            || name == b".."
            || name.contains(&b'/')
            || name.contains(&0)
            || kept == Kept::Generation
            || (kept == Kept::Inbox && place != Place::Inbox);
        if bad {
            return Err(Error::Invalid);
        }
        self.rhizo_dir()?;
        let rhizo = self.rhizo.expect("made above");
        let dir = match sub {
            None => rhizo,
            Some(sub) => match self.step(rhizo, sub) {
                Ok(dir) => dir,
                Err(Error::NotFound) => {
                    let dir = self.alloc_inode(Inode {
                        links: 1,
                        node: Node::Dir {
                            entries: BTreeMap::new(),
                            parent: rhizo,
                        },
                    })?;
                    if let Node::Dir { entries, .. } = &mut self.inode_mut(rhizo).node {
                        entries.insert(sub.to_vec(), dir);
                    }
                    dir
                }
                Err(e) => return Err(e),
            },
        };
        let taken = matches!(&self.inode(dir).node, Node::Dir { entries, .. } if entries.contains_key(name));
        if taken {
            return Err(Error::Exists);
        }
        let added = self.add_entry(dir, name, kept);
        if added.is_err()
            && dir != rhizo
            && matches!(&self.inode(dir).node, Node::Dir { entries, .. } if entries.is_empty())
        {
            if let Node::Dir { entries, .. } = &mut self.inode_mut(rhizo).node {
                entries.retain(|_, &mut i| i != dir);
            }
            self.inodes[dir as usize] = None;
        }
        added
    }

    /// The kernel takes the entry at `place` away (its handle was revoked
    /// or moved); returns what it held, which the caller now holds
    /// (nothing is handed to [`take_released`](Self::take_released) or
    /// [`take_dropped`](Self::take_dropped)).
    pub fn rhizo_take(&mut self, place: Place<'_>) -> Result<Kept> {
        let rhizo = self.rhizo.ok_or(Error::NotFound)?;
        let (sub, name) = place.split();
        let dir = match sub {
            None => rhizo,
            Some(sub) => self.step(rhizo, sub)?,
        };
        let (released, dropped) = (self.released.len(), self.dropped.len());
        let kept = self.remove_kept(dir, name).ok_or(Error::NotFound)?;
        self.released.truncate(released);
        self.dropped.truncate(dropped);
        Ok(kept)
    }

    /// What the `/run/rhizo` entry `path` stands for, to open or connect
    /// it; `Refused` if `path` names something else.
    pub fn rhizo_entry(&self, dirfd: Option<Fd>, path: &[u8]) -> Result<Kept> {
        match self.inode(self.lookup(dirfd, path)?).node {
            Node::Kept(k) => Ok(k),
            _ => Err(Error::Refused),
        }
    }

    /// Whether grants wait to be dropped.
    pub fn has_dropped(&self) -> bool {
        !self.dropped.is_empty()
    }

    /// The grants of the `/run/rhizo` entries the cell unlinked since the
    /// last call, with the labels they had: the kernel drops each that is
    /// still the cell's under that label (an entry a copy inherited, or
    /// one whose grant was revoked, names none).
    pub fn take_dropped(&mut self) -> Vec<(u32, Vec<u8>)> {
        core::mem::take(&mut self.dropped)
    }
}
