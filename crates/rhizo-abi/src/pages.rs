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
/// `limit` pages: a page past it empties the cache first.
#[derive(Debug)]
pub struct PageCache {
    pages: BTreeMap<(usize, usize), (Page, u64)>,
    limit: usize,
}

impl PageCache {
    /// An empty cache of at most `limit` pages.
    pub const fn new(limit: usize) -> Self {
        Self {
            pages: BTreeMap::new(),
            limit,
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

    /// Drops every page; frames that mappings still use stay theirs.
    /// Returns whether there was one.
    pub fn clear(&mut self, frames: &mut impl Frames) -> bool {
        let any = !self.pages.is_empty();
        for (_, (_, phys)) in core::mem::take(&mut self.pages) {
            frames.free(phys, PageSize::Small);
        }
        any
    }

    /// A frame for `page` with one reference for the caller, and whether it
    /// was written now: the kept frame of the same contents, or a new one
    /// (kept, unless the page has no file data: then it is the caller's
    /// alone, zeroed). When the kept frame cannot count one more reference,
    /// the caller gets a copy of its own.
    pub fn get(
        &mut self,
        page: &Page,
        mem: &mut impl PhysicalMemory,
        frames: &mut impl Frames,
    ) -> Option<(u64, bool)> {
        let Some(key) = page.key() else {
            let phys = frames.alloc()?;
            mem.zero_frame(phys);
            return Some((phys, true));
        };
        if let Some(&(_, phys)) = self.pages.get(&key) {
            if frames.try_share(phys) {
                return Some((phys, false));
            }
            // Its count is full (a cell can map one page without end,
            // finding S14): a copy of the caller's own.
            let copy = frames.alloc()?;
            mem.zero_frame(copy);
            write_bytes(mem, copy, page.bytes());
            return Some((copy, true));
        }
        if self.pages.len() >= self.limit {
            self.clear(frames);
        }
        let phys = frames.alloc()?;
        mem.zero_frame(phys);
        write_bytes(mem, phys, page.bytes());
        frames.share(phys);
        self.pages.insert(key, (page.clone(), phys));
        Some((phys, true))
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
        assert!(cache.clear(&mut frames));
        assert!(cache.is_empty() && !cache.clear(&mut frames));
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
        assert_eq!(got[..2], [(kept, true), (kept, false)], "shared while it counts");
        for &(copy, fresh) in &got[2..] {
            assert!(copy != kept && fresh && !frames.is_shared(copy));
            assert_eq!(mem.read_u64(copy), u64::from(b'!'), "the page's bytes");
        }
        for (phys, _) in got {
            frames.free(phys, PageSize::Small);
        }
        cache.clear(&mut frames);
        assert_eq!(frames.outstanding(), 0);
    }

    #[test]
    fn a_full_cache_starts_over() {
        let mut fs = rhizo_fs::Fs::new();
        fs.put_file(b"/f", &[1u8; 3 * 4096]).unwrap();
        let (ino, _) = fs.file_at(b"/f").unwrap();
        let mut mem = FakePhysicalMemory::new();
        let mut frames = CountingFrames::new(1024);
        let mut cache = PageCache::new(2);
        for i in 0..3 {
            let (phys, _) = cache
                .get(&fs.page(ino, i).unwrap(), &mut mem, &mut frames)
                .unwrap();
            frames.free(phys, PageSize::Small);
        }
        assert_eq!(cache.len(), 1);
        cache.clear(&mut frames);
        assert_eq!(frames.outstanding(), 0);
    }
}
