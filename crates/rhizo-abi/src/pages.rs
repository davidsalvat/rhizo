//! Pages of files mapped into cells (design D1 of `add-dynamic-linking`):
//! kept in frames by content, so every mapping of the same bytes, in any
//! cell, shares one read-only frame.

use alloc::collections::BTreeMap;
use rhizo_cell::memory::write_bytes;
use rhizo_cell::space::Frames;
use rhizo_fs::Page;
use rhizo_hal::PhysicalMemory;
use rhizo_mm::paging::PageSize;

/// Where the frames of mapped file pages come from.
pub trait FilePages<M, A> {
    /// A frame holding `page`'s bytes, the rest of it zero, with one
    /// reference for the caller; `None` when memory runs out.
    fn frame(&mut self, page: &Page, mem: &mut M, frames: &mut A) -> Option<u64>;

    /// `phys` was just written and will be executable (the kernel makes
    /// instruction fetch see it).
    fn code(&mut self, phys: u64) {
        let _ = phys;
    }
}

/// File pages kept in frames, by the identity of their contents
/// ([`Page::key`]). Each holds its page, so its bytes cannot change while
/// kept, and one reference to its frame; mappings take more. At most
/// `limit` pages. No cell can make the cache drop a page another cell maps
/// (design D3 of `close-cross-cell-leaks`): a full cache drops only the
/// pages no file holds any more, which nobody can map again, and a page it
/// has no room for is the caller's own copy; [`trim`](Self::trim) drops
/// only what frees memory.
#[derive(Debug)]
pub struct PageCache {
    pages: BTreeMap<(usize, usize), (Page, u64)>,
    limit: usize,
    /// Pages refused for want of room before the next look for pages
    /// nobody can map: a cache full of pages files hold is not searched
    /// on every miss.
    wait: usize,
}

/// What [`PageCache::share`] found for a page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Share {
    /// The kept frame, with one reference for the caller, and whether it
    /// was written now.
    Kept(u64, bool),
    /// The page is not kept (no file data, a frame that counts no more
    /// references, or no room): the caller takes a copy of its own
    /// ([`own_copy`]).
    Own,
    /// Memory ran out.
    OutOfMemory,
}

/// A frame of the caller's own holding `page`'s bytes, the rest zero.
pub fn own_copy(
    page: &Page,
    mem: &mut impl PhysicalMemory,
    frames: &mut impl Frames,
) -> Option<u64> {
    let phys = frames.alloc()?;
    mem.zero_frame(phys);
    write_bytes(mem, phys, page.bytes());
    Some(phys)
}

impl PageCache {
    /// An empty cache of at most `limit` pages.
    pub const fn new(limit: usize) -> Self {
        Self {
            pages: BTreeMap::new(),
            limit,
            wait: 0,
        }
    }

    /// Pages kept.
    pub fn len(&self) -> usize {
        self.pages.len()
    }

    /// Whether no page is kept.
    pub fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }

    /// Drops the pages kept for which `keep` says no, giving back their
    /// frames (mappings keep theirs); returns whether one was dropped.
    fn drop_unless<A: Frames>(
        &mut self,
        frames: &mut A,
        keep: impl Fn(&Page, u64, &A) -> bool,
    ) -> bool {
        let before = self.pages.len();
        self.pages.retain(|_, (page, phys)| {
            let kept = keep(page, *phys, frames);
            if !kept {
                frames.free(*phys, PageSize::Small);
            }
            kept
        });
        if self.pages.is_empty() {
            self.pages = BTreeMap::new(); // gives back its last node
        }
        self.pages.len() != before
    }

    /// Drops every page whose dropping frees memory: those no mapping
    /// uses, and those no file holds any more. A page a cell maps from a
    /// file stays, so memory pressure from one cell never drops the pages
    /// another uses. Returns whether one was dropped.
    pub fn trim(&mut self, frames: &mut impl Frames) -> bool {
        self.drop_unless(frames, |page, phys, frames| {
            page.is_held_elsewhere() && frames.is_shared(phys)
        })
    }

    /// Room for one more page: drops the pages no file holds any more
    /// (nobody can map them again), looking at most once every
    /// `limit / 16` refusals while the cache stays full.
    fn room(&mut self, frames: &mut impl Frames) -> bool {
        if self.pages.len() < self.limit {
            return true;
        }
        if self.wait > 0 {
            self.wait -= 1;
            return false;
        }
        self.drop_unless(frames, |page, _, _| page.is_held_elsewhere());
        let room = self.pages.len() < self.limit;
        if !room {
            self.wait = self.limit / 16;
        }
        room
    }

    /// The kept frame of `page`'s contents with one reference for the
    /// caller, or a new one kept for it; or that the caller takes a copy
    /// of its own ([`Share::Own`]).
    pub fn share(
        &mut self,
        page: &Page,
        mem: &mut impl PhysicalMemory,
        frames: &mut impl Frames,
    ) -> Share {
        let Some(key) = page.key() else {
            return Share::Own;
        };
        if let Some(&(_, phys)) = self.pages.get(&key) {
            // Its count may be full (a cell can map one page without end,
            // finding S14).
            return match frames.try_share(phys) {
                true => Share::Kept(phys, false),
                false => Share::Own,
            };
        }
        if !self.room(frames) {
            return Share::Own;
        }
        let Some(phys) = frames.alloc() else {
            return Share::OutOfMemory;
        };
        mem.zero_frame(phys);
        write_bytes(mem, phys, page.bytes());
        frames.share(phys);
        self.pages.insert(key, (page.clone(), phys));
        Share::Kept(phys, true)
    }

    /// A frame for `page` with one reference for the caller, and whether it
    /// was written now: as [`share`](Self::share) says, with the caller's
    /// own copy taken from `frames` too.
    pub fn get(
        &mut self,
        page: &Page,
        mem: &mut impl PhysicalMemory,
        frames: &mut impl Frames,
    ) -> Option<(u64, bool)> {
        match self.share(page, mem, frames) {
            Share::Kept(phys, fresh) => Some((phys, fresh)),
            Share::Own => own_copy(page, mem, frames).map(|phys| (phys, true)),
            Share::OutOfMemory => None,
        }
    }
}

impl<M: PhysicalMemory, A: Frames> FilePages<M, A> for PageCache {
    fn frame(&mut self, page: &Page, mem: &mut M, frames: &mut A) -> Option<u64> {
        self.get(page, mem, frames).map(|(phys, _)| phys)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhizo_cell::testing::CountingFrames;
    use rhizo_hal::testing::FakePhysicalMemory;

    #[test]
    fn same_contents_same_frame_until_the_file_changes() {
        let mut fs = rhizo_fs::Fs::new();
        fs.put_file(b"/lib/x.so", &[3u8; 5000]).unwrap();
        let (ino, _) = fs.file_at(b"/lib/x.so").unwrap();
        let mut mem = FakePhysicalMemory::new();
        let mut frames = CountingFrames::new(1024);
        let mut cache = PageCache::new(8);
        let page = |fs: &rhizo_fs::Fs, i| fs.page(ino, i).unwrap();

        let (a, fresh) = cache.get(&page(&fs, 1), &mut mem, &mut frames).unwrap();
        assert!(fresh);
        let mut bytes = [9u8; 4096];
        mem.read_bytes(a, &mut bytes);
        assert!(bytes[..904].iter().all(|&b| b == 3) && bytes[904..].iter().all(|&b| b == 0));
        // Another cell's copy of the files: the same blocks, the same frame.
        let fork = fs.clone();
        assert_eq!(
            cache.get(&page(&fork, 1), &mut mem, &mut frames),
            Some((a, false))
        );
        assert!(frames.is_shared(a));
        // Past the end: a zeroed frame of the caller's own.
        let (zero, _) = cache.get(&page(&fs, 5), &mut mem, &mut frames).unwrap();
        assert!(!frames.is_shared(zero) && mem.read_u64(zero) == 0);
        assert_eq!(cache.len(), 1);

        // Written, the file's block is another: another frame.
        let flags = rhizo_fs::OpenFlags {
            write: true,
            ..Default::default()
        };
        let fd = fs.open(None, b"/lib/x.so", flags, 1).unwrap();
        fs.write(fd, b"!", Some(4096)).unwrap();
        let (b, fresh) = cache.get(&page(&fs, 1), &mut mem, &mut frames).unwrap();
        assert!(fresh && b != a);
        assert_eq!(mem.read_u64(b) & 0xff, u64::from(b'!'));
        assert_eq!(
            mem.read_u64(a) & 0xff,
            3,
            "the old frame keeps the old bytes"
        );

        // Dropped, only the mappings' references stay.
        for phys in [a, a, b, zero] {
            frames.free(phys, PageSize::Small);
        }
        assert!(cache.trim(&mut frames));
        assert!(cache.is_empty() && !cache.trim(&mut frames));
        assert_eq!(frames.outstanding(), 0);
    }

    /// Finding S14: a page mapped more times than a frame counts gets
    /// copies of its own past the count; nothing panics or leaks.
    #[test]
    fn a_page_shared_without_end_gets_copies() {
        let mut fs = rhizo_fs::Fs::new();
        fs.put_file(b"/f", b"!").unwrap();
        let (ino, _) = fs.file_at(b"/f").unwrap();
        let mut mem = FakePhysicalMemory::new();
        let mut frames = CountingFrames::new(1024);
        frames.max_refs = 3;
        let mut cache = PageCache::new(8);
        let page = fs.page(ino, 0).unwrap();
        let got: Vec<(u64, bool)> = (0..4)
            .map(|_| cache.get(&page, &mut mem, &mut frames).unwrap())
            .collect();
        let kept = got[0].0;
        assert_eq!(
            got[..2],
            [(kept, true), (kept, false)],
            "shared while it counts"
        );
        for &(copy, fresh) in &got[2..] {
            assert!(copy != kept && fresh && !frames.is_shared(copy));
            assert_eq!(mem.read_u64(copy), u64::from(b'!'), "the page's bytes");
        }
        for (phys, _) in got {
            frames.free(phys, PageSize::Small);
        }
        cache.trim(&mut frames);
        assert_eq!(frames.outstanding(), 0);
    }

    /// Hostile (finding S26): one cell cannot empty the cache of the pages
    /// another cell uses by filling it with pages of its own (it emptied
    /// the whole cache before `close-cross-cell-leaks`, a probe-and-flush
    /// channel). A page past the limit is the caller's own copy.
    #[test]
    fn a_full_cache_keeps_what_others_map() {
        let mut victim = rhizo_fs::Fs::new();
        let mut attacker = rhizo_fs::Fs::new();
        victim.put_file(b"/lib", &[1u8; 4096]).unwrap();
        attacker.put_file(b"/own", &[2u8; 3 * 4096]).unwrap();
        let (lib, _) = victim.file_at(b"/lib").unwrap();
        let (own, _) = attacker.file_at(b"/own").unwrap();
        let mut mem = FakePhysicalMemory::new();
        let mut frames = CountingFrames::new(1024);
        let mut cache = PageCache::new(2);
        let page = |fs: &rhizo_fs::Fs, ino, i| fs.page(ino, i).unwrap();
        let (kept, _) = cache
            .get(&page(&victim, lib, 0), &mut mem, &mut frames)
            .unwrap();
        // The attacker maps three pages and lets them go.
        let mut copies = 0;
        for i in 0..3 {
            match cache.share(&page(&attacker, own, i), &mut mem, &mut frames) {
                Share::Kept(phys, _) => frames.free(phys, PageSize::Small),
                Share::Own => copies += 1,
                Share::OutOfMemory => panic!("memory"),
            }
        }
        assert_eq!((cache.len(), copies), (2, 2), "one kept, then copies");
        // Under memory pressure only the attacker's unused page goes.
        assert!(cache.trim(&mut frames));
        assert_eq!(
            cache.get(&page(&victim.clone(), lib, 0), &mut mem, &mut frames),
            Some((kept, false)),
            "the victim's page is still kept"
        );
        // The copy of a page with no room has its bytes.
        let (copy, fresh) = PageCache::new(0)
            .get(&page(&attacker, own, 1), &mut mem, &mut frames)
            .unwrap();
        assert!(fresh && !frames.is_shared(copy));
        assert_eq!(mem.read_u64(copy) & 0xff, 2);
        for phys in [kept, kept, copy] {
            frames.free(phys, PageSize::Small);
        }
        assert!(cache.trim(&mut frames) && cache.is_empty());
        assert_eq!(frames.outstanding(), 0);
    }

    /// A full cache makes room by dropping the pages no file holds any
    /// more: nobody can map those again, so dropping them shows nothing.
    #[test]
    fn a_full_cache_drops_pages_nobody_can_map() {
        let mut fs = rhizo_fs::Fs::new();
        fs.put_file(b"/a", &[1u8; 4096]).unwrap();
        fs.put_file(b"/b", &[2u8; 4096]).unwrap();
        let ino = |fs: &rhizo_fs::Fs, p: &[u8]| fs.file_at(p).unwrap().0;
        let mut mem = FakePhysicalMemory::new();
        let mut frames = CountingFrames::new(1024);
        let mut cache = PageCache::new(1);
        let a = fs.page(ino(&fs, b"/a"), 0).unwrap();
        let (first, _) = cache.get(&a, &mut mem, &mut frames).unwrap();
        drop(a);
        let b = fs.page(ino(&fs, b"/b"), 0).unwrap();
        assert_eq!(cache.share(&b, &mut mem, &mut frames), Share::Own, "full");
        // The file is written over: its old block is the cache's alone.
        let rw = rhizo_fs::OpenFlags {
            write: true,
            ..Default::default()
        };
        let fd = fs.open(None, b"/a", rw, 1).unwrap();
        fs.write(fd, b"!", Some(0)).unwrap();
        let second = cache.share(&b, &mut mem, &mut frames);
        let Share::Kept(phys, true) = second else {
            panic!("{second:?}");
        };
        assert!(phys != first && cache.len() == 1);
        for phys in [first, phys] {
            frames.free(phys, PageSize::Small);
        }
        cache.trim(&mut frames);
        assert_eq!(frames.outstanding(), 0);
    }
}
