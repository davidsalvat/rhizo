//! Pages of files mapped into cells (design D1 of `add-dynamic-linking`):
//! kept in frames by content, so every mapping of the same bytes in one
//! domain shares one read-only frame, and every mapping of a public page,
//! in any domain, shares the frame its publication filled (design D21 of
//! `add-cell-hierarchy`).

use alloc::collections::{BTreeMap, BTreeSet};
use rhizo_cell::memory::write_bytes;
use rhizo_cell::space::Frames;
use rhizo_fs::Page;
use rhizo_hal::PhysicalMemory;
use rhizo_mm::paging::PageSize;

/// The scope of public pages: shared by every domain, filled only by
/// [`PageCache::publish`]. No domain has this identifier.
pub const PUBLIC: u32 = 0;

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

/// A kept page's key: its scope (a domain), then the identity of its
/// contents ([`Page::key`]).
type Key = (u32, usize, usize);

/// A page kept: the page itself (so its bytes cannot change), its frame,
/// with one reference of the cache's, and how many publications pin it.
#[derive(Debug)]
struct Kept {
    page: Page,
    phys: u64,
    pins: u32,
}

/// How full a domain's part of the cache is.
#[derive(Debug, Clone, Copy, Default)]
struct Scope {
    len: usize,
    /// Pages refused for want of room before the next look for pages
    /// nobody can map: a part full of pages files hold is not searched on
    /// every miss.
    wait: usize,
}

/// File pages kept in frames, by scope and by the identity of their
/// contents (design D21 of `add-cell-hierarchy`): a domain's pages are
/// found only by its own cells, so a page filled for one domain is never
/// handed to another, and a hit or a miss tells a cell only about its own
/// domain. Public pages ([`PUBLIC`]) are found by every domain, for pages
/// of public files only ([`Page::is_public`]); they are filled when the
/// host publishes them and kept while pinned, so a map of one always hits.
///
/// Each domain keeps at most `limit` pages. No cell can make the cache drop
/// a page another cell maps (design D3 of `close-cross-cell-leaks`): a full
/// domain drops only its pages no file holds any more, which nobody can map
/// again, and a page it has no room for is the caller's own copy;
/// [`trim`](Self::trim) drops only what frees memory. Public pages are kept
/// apart, so that dropping every domain's pages gives back every record of
/// them.
#[derive(Debug)]
pub struct PageCache {
    /// Domains' pages, and their frames.
    pages: BTreeMap<Key, Kept>,
    frames: BTreeSet<u64>,
    scopes: BTreeMap<u32, Scope>,
    /// Public pages, by contents, and their frames.
    public: BTreeMap<(usize, usize), Kept>,
    public_frames: BTreeSet<u64>,
    limit: usize,
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

/// Drops from `pages` the pages that are not pinned and for which `keep`
/// says no, giving back their frames (mappings keep theirs) and forgetting
/// them in `owners`; calls `gone` with each one's key.
fn drop_from<K: Ord + Copy, A: Frames>(
    pages: &mut BTreeMap<K, Kept>,
    owners: &mut BTreeSet<u64>,
    frames: &mut A,
    keep: &mut impl FnMut(&K, &Page, u64, &A) -> bool,
    gone: &mut impl FnMut(&K),
) {
    pages.retain(|key, kept| {
        let stays = kept.pins > 0 || keep(key, &kept.page, kept.phys, frames);
        if !stays {
            frames.free(kept.phys, PageSize::Small);
            owners.remove(&kept.phys);
            gone(key);
        }
        stays
    });
    if pages.is_empty() {
        // Gives back their last nodes.
        (*pages, *owners) = Default::default();
    }
}

impl PageCache {
    /// An empty cache of at most `limit` pages per domain.
    pub const fn new(limit: usize) -> Self {
        Self {
            pages: BTreeMap::new(),
            frames: BTreeSet::new(),
            scopes: BTreeMap::new(),
            public: BTreeMap::new(),
            public_frames: BTreeSet::new(),
            limit,
        }
    }

    /// Keeps at most `limit` pages per domain from now on: a domain past
    /// it drops its pages no file holds on its next miss, or copies.
    pub fn set_limit(&mut self, limit: usize) {
        self.limit = limit;
    }

    /// Pages kept.
    pub fn len(&self) -> usize {
        self.pages.len() + self.public.len()
    }

    /// Whether no page is kept.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Pages kept for `scope` (a domain, or [`PUBLIC`]).
    pub fn kept(&self, scope: u32) -> usize {
        match scope {
            PUBLIC => self.public.len(),
            _ => self.scopes.get(&scope).map_or(0, |s| s.len),
        }
    }

    /// Whether `phys` is the frame of a page kept, of any scope.
    pub fn holds(&self, phys: u64) -> bool {
        self.frames.contains(&phys) || self.is_public(phys)
    }

    /// Whether `phys` is the frame of a public page: one every domain may
    /// share.
    pub fn is_public(&self, phys: u64) -> bool {
        self.public_frames.contains(&phys)
    }

    /// Drops the pages kept that are not pinned and for which `keep` says
    /// no, giving back their frames; returns whether one was dropped.
    fn drop_unless<A: Frames>(
        &mut self,
        frames: &mut A,
        mut keep: impl FnMut(u32, &Page, u64, &A) -> bool,
    ) -> bool {
        let before = self.len();
        let scopes = &mut self.scopes;
        drop_from(
            &mut self.pages,
            &mut self.frames,
            frames,
            &mut |key: &Key, page, phys, frames| keep(key.0, page, phys, frames),
            &mut |key: &Key| {
                if let Some(scope) = scopes.get_mut(&key.0) {
                    scope.len -= 1;
                }
            },
        );
        scopes.retain(|_, s| s.len > 0);
        if scopes.is_empty() {
            *scopes = BTreeMap::new();
        }
        drop_from(
            &mut self.public,
            &mut self.public_frames,
            frames,
            &mut |_, page, phys, frames| keep(PUBLIC, page, phys, frames),
            &mut |_| {},
        );
        self.len() != before
    }

    /// Drops every page whose dropping frees memory: those no mapping
    /// uses, and those no file holds any more. A page a cell maps from a
    /// file stays, so memory pressure from one cell never drops the pages
    /// another uses, and a pinned public page stays. Returns whether one
    /// was dropped.
    pub fn trim(&mut self, frames: &mut impl Frames) -> bool {
        self.drop_unless(frames, |_, page, phys, frames| {
            page.is_held_elsewhere() && frames.is_shared(phys)
        })
    }

    /// Room for one more page of `scope`: drops that domain's pages no
    /// file holds any more (nobody can map them again), looking at most
    /// once every `limit / 16` refusals while it stays full.
    fn room(&mut self, scope: u32, frames: &mut impl Frames) -> bool {
        let s = self.scopes.get(&scope).copied().unwrap_or_default();
        if s.len < self.limit {
            return true;
        }
        if s.wait > 0 {
            self.scopes.entry(scope).or_default().wait -= 1;
            return false;
        }
        self.drop_unless(frames, |of, page, _, _| {
            of != scope || page.is_held_elsewhere()
        });
        let room = self.kept(scope) < self.limit;
        if !room {
            self.scopes.entry(scope).or_default().wait = self.limit / 16;
        }
        room
    }

    /// The kept frame of `page`'s contents for a cell of domain `scope`,
    /// with one reference for the caller, or a new one kept for that
    /// domain; or that the caller takes a copy of its own
    /// ([`Share::Own`]). A public page is found in the public pages first.
    /// Nothing is ever filled for [`PUBLIC`] here: a cell outside any
    /// domain gets its own copy.
    pub fn share(
        &mut self,
        page: &Page,
        scope: u32,
        mem: &mut impl PhysicalMemory,
        frames: &mut impl Frames,
    ) -> Share {
        let Some((block, len)) = page.key() else {
            return Share::Own;
        };
        let public = page
            .is_public()
            .then(|| self.public.get(&(block, len)))
            .flatten();
        let found = public.or_else(|| self.pages.get(&(scope, block, len)));
        if let Some(kept) = found {
            // Its count may be full (a cell can map one page without end,
            // finding S14).
            return match frames.try_share(kept.phys) {
                true => Share::Kept(kept.phys, false),
                false => Share::Own,
            };
        }
        if scope == PUBLIC || !self.room(scope, frames) {
            return Share::Own;
        }
        let Some(phys) = own_copy(page, mem, frames) else {
            return Share::OutOfMemory;
        };
        frames.share(phys); // the cache's reference, and the caller's
        self.frames.insert(phys);
        let page = page.clone();
        let kept = Kept {
            page,
            phys,
            pins: 0,
        };
        self.pages.insert((scope, block, len), kept);
        self.scopes.entry(scope).or_default().len += 1;
        Share::Kept(phys, true)
    }

    /// Publishes `page` (design D21 of `add-cell-hierarchy`): fills its
    /// public frame if it has none yet, and pins it until as many
    /// [`withdraw`](Self::withdraw) calls. Returns the frame and whether it
    /// was written now; `None` for a page that is not public or has no
    /// file data, or when memory runs out.
    pub fn publish(
        &mut self,
        page: &Page,
        mem: &mut impl PhysicalMemory,
        frames: &mut impl Frames,
    ) -> Option<(u64, bool)> {
        let key = page.key().filter(|_| page.is_public())?;
        if let Some(kept) = self.public.get_mut(&key) {
            kept.pins = kept.pins.checked_add(1)?;
            return Some((kept.phys, false));
        }
        let phys = own_copy(page, mem, frames)?; // the cache's reference only
        self.public_frames.insert(phys);
        let page = page.clone();
        self.public.insert(
            key,
            Kept {
                page,
                phys,
                pins: 1,
            },
        );
        Some((phys, true))
    }

    /// Ends one publication of `page`: once no publication pins it, its
    /// public frame goes like any page kept, when nothing maps it.
    pub fn withdraw(&mut self, page: &Page) {
        let kept = page.key().and_then(|key| self.public.get_mut(&key));
        if let Some(kept) = kept {
            kept.pins = kept.pins.saturating_sub(1);
        }
    }

    /// A frame for `page` for a cell of domain `scope` with one reference
    /// for the caller, and whether it was written now: as
    /// [`share`](Self::share) says, with the caller's own copy taken from
    /// `frames` too.
    pub fn get(
        &mut self,
        page: &Page,
        scope: u32,
        mem: &mut impl PhysicalMemory,
        frames: &mut impl Frames,
    ) -> Option<(u64, bool)> {
        match self.share(page, scope, mem, frames) {
            Share::Kept(phys, fresh) => Some((phys, fresh)),
            Share::Own => own_copy(page, mem, frames).map(|phys| (phys, true)),
            Share::OutOfMemory => None,
        }
    }
}

/// A [`PageCache`] as the file pages of the cells of one domain.
#[derive(Debug)]
pub struct Scoped<'a>(pub &'a mut PageCache, pub u32);

impl<M: PhysicalMemory, A: Frames> FilePages<M, A> for Scoped<'_> {
    fn frame(&mut self, page: &Page, mem: &mut M, frames: &mut A) -> Option<u64> {
        self.0.get(page, self.1, mem, frames).map(|(phys, _)| phys)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
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

        let (a, fresh) = cache.get(&page(&fs, 1), 1, &mut mem, &mut frames).unwrap();
        assert!(fresh);
        let mut bytes = [9u8; 4096];
        mem.read_bytes(a, &mut bytes);
        assert!(bytes[..904].iter().all(|&b| b == 3) && bytes[904..].iter().all(|&b| b == 0));
        // Another cell's copy of the files: the same blocks, the same frame.
        let fork = fs.clone();
        assert_eq!(
            cache.get(&page(&fork, 1), 1, &mut mem, &mut frames),
            Some((a, false))
        );
        assert!(frames.is_shared(a));
        // Past the end: a zeroed frame of the caller's own.
        let (zero, _) = cache.get(&page(&fs, 5), 1, &mut mem, &mut frames).unwrap();
        assert!(!frames.is_shared(zero) && mem.read_u64(zero) == 0);
        assert_eq!(cache.len(), 1);

        // Written, the file's block is another: another frame.
        let flags = rhizo_fs::OpenFlags {
            write: true,
            ..Default::default()
        };
        let fd = fs.open(None, b"/lib/x.so", flags, 1).unwrap();
        fs.write(fd, b"!", Some(4096)).unwrap();
        let (b, fresh) = cache.get(&page(&fs, 1), 1, &mut mem, &mut frames).unwrap();
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
            .map(|_| cache.get(&page, 1, &mut mem, &mut frames).unwrap())
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
            .get(&page(&victim, lib, 0), 1, &mut mem, &mut frames)
            .unwrap();
        // The attacker maps three pages and lets them go.
        let mut copies = 0;
        for i in 0..3 {
            match cache.share(&page(&attacker, own, i), 1, &mut mem, &mut frames) {
                Share::Kept(phys, _) => frames.free(phys, PageSize::Small),
                Share::Own => copies += 1,
                Share::OutOfMemory => panic!("memory"),
            }
        }
        assert_eq!((cache.len(), copies), (2, 2), "one kept, then copies");
        // Under memory pressure only the attacker's unused page goes.
        assert!(cache.trim(&mut frames));
        assert_eq!(
            cache.get(&page(&victim.clone(), lib, 0), 1, &mut mem, &mut frames),
            Some((kept, false)),
            "the victim's page is still kept"
        );
        // The copy of a page with no room has its bytes.
        let (copy, fresh) = PageCache::new(0)
            .get(&page(&attacker, own, 1), 1, &mut mem, &mut frames)
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
        let (first, _) = cache.get(&a, 1, &mut mem, &mut frames).unwrap();
        drop(a);
        let b = fs.page(ino(&fs, b"/b"), 0).unwrap();
        assert_eq!(
            cache.share(&b, 1, &mut mem, &mut frames),
            Share::Own,
            "full"
        );
        // The file is written over: its old block is the cache's alone.
        let rw = rhizo_fs::OpenFlags {
            write: true,
            ..Default::default()
        };
        let fd = fs.open(None, b"/a", rw, 1).unwrap();
        fs.write(fd, b"!", Some(0)).unwrap();
        let second = cache.share(&b, 1, &mut mem, &mut frames);
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

    /// Design D21 of `add-cell-hierarchy`: the same bytes mapped in two
    /// domains take one frame per domain; a public page that was published
    /// takes one frame for all, pinned until withdrawn; a public page never
    /// published is kept per domain; and one domain's full part leaves the
    /// others' alone.
    #[test]
    fn domains_share_only_public_pages() {
        let mut set = rhizo_fs::Fs::new();
        set.put_file(b"/lib/libc.so", &[1u8; 4096]).unwrap();
        set.put_file(b"/lib/late.so", &[2u8; 4096]).unwrap();
        set.publish();
        set.put_file(b"/data", &[3u8; 3 * 4096]).unwrap(); // private
        let ino = |p: &[u8]| set.file_at(p).unwrap().0;
        let (libc, late, data) = (ino(b"/lib/libc.so"), ino(b"/lib/late.so"), ino(b"/data"));
        let mut mem = FakePhysicalMemory::new();
        let mut frames = CountingFrames::new(1024);
        let mut cache = PageCache::new(2);
        let page = |fs: &rhizo_fs::Fs, ino, i| fs.page(ino, i).unwrap();
        let (one, two) = (set.clone(), set.clone());

        let published = cache.publish(&page(&set, libc, 0), &mut mem, &mut frames);
        let (lib, fresh) = published.unwrap();
        assert!(fresh && cache.is_public(lib) && cache.holds(lib));
        let private = cache.publish(&page(&set, data, 0), &mut mem, &mut frames);
        assert_eq!(private, None, "a private file is not published");
        let mut mapped = Vec::new();
        for (fs, scope) in [(&one, 1), (&two, 2), (&one, 1)] {
            let got = cache.share(&page(fs, libc, 0), scope, &mut mem, &mut frames);
            assert_eq!(got, Share::Kept(lib, false), "a hit everywhere");
            mapped.push(lib);
        }
        let mut share = |fs: &rhizo_fs::Fs, ino, i, scope, cache: &mut PageCache| {
            cache.share(&page(fs, ino, i), scope, &mut mem, &mut frames)
        };
        let Share::Kept(first, true) = share(&one, data, 0, 1, &mut cache) else {
            panic!("filled for domain 1");
        };
        let Share::Kept(second, true) = share(&two, data, 0, 2, &mut cache) else {
            panic!("filled again for domain 2");
        };
        assert_ne!(first, second, "one frame per domain");
        assert!(!cache.is_public(first) && cache.holds(first));
        assert_eq!(
            share(&one, data, 0, 1, &mut cache),
            Share::Kept(first, false)
        );
        mapped.extend([first, second, first]);
        // Public, never published: kept per domain, never as public.
        let Share::Kept(own, true) = share(&one, late, 0, 1, &mut cache) else {
            panic!("filled for domain 1");
        };
        assert!(!cache.is_public(own));
        let Share::Kept(other, true) = share(&two, late, 0, 2, &mut cache) else {
            panic!("filled for domain 2");
        };
        assert_ne!(own, other);
        mapped.extend([own, other]);
        assert_eq!(cache.kept(PUBLIC), 1);
        // A cell outside any domain gets its own copy of what is not public.
        assert_eq!(share(&one, data, 1, PUBLIC, &mut cache), Share::Own);
        // Domain 1 is full (two pages); domain 3 still gets room.
        assert_eq!(share(&one, data, 2, 1, &mut cache), Share::Own, "full");
        let Share::Kept(third, true) = share(&one, data, 2, 3, &mut cache) else {
            panic!("room in domain 3");
        };
        mapped.push(third);
        assert_eq!((cache.kept(1), cache.kept(2), cache.kept(3)), (2, 2, 1));
        cache.set_limit(3);
        let Share::Kept(fourth, true) = share(&one, data, 2, 1, &mut cache) else {
            panic!("room in domain 1 under the new limit");
        };
        mapped.push(fourth);

        // Unmapped and trimmed, the published page stays until withdrawn.
        for phys in mapped {
            frames.free(phys, PageSize::Small);
        }
        drop((one, two));
        assert!(cache.trim(&mut frames));
        assert_eq!(cache.len(), 1, "only the pinned page");
        let again = cache.share(&page(&set, libc, 0), 7, &mut mem, &mut frames);
        assert_eq!(again, Share::Kept(lib, false));
        frames.free(lib, PageSize::Small);
        cache.withdraw(&page(&set, libc, 0));
        assert!(cache.trim(&mut frames) && cache.is_empty());
        assert_eq!(frames.outstanding(), 0);
    }

    #[derive(Debug, Clone)]
    enum Op {
        /// Domain, file, page.
        Map(u32, usize, u64),
        Unmap(usize),
        /// A cell of the domain writes the file.
        Write(u32, usize),
        Publish(usize),
        Withdraw(usize),
        Trim,
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            4 => (1..4u32, 0..3usize, 0..2u64).prop_map(|(d, f, p)| Op::Map(d, f, p)),
            2 => (0..16usize).prop_map(Op::Unmap),
            1 => (1..4u32, 0..3usize).prop_map(|(d, f)| Op::Write(d, f)),
            1 => (0..3usize).prop_map(Op::Publish),
            1 => (0..3usize).prop_map(Op::Withdraw),
            1 => Just(Op::Trim),
        ]
    }

    const FILES: [&[u8]; 3] = [b"/lib/a.so", b"/lib/b.so", b"/data"];

    proptest! {
        /// Design D21 of `add-cell-hierarchy`: whatever three domains map,
        /// write, and the host publishes, withdraws or trims, no key of one
        /// domain ever returns a frame filled for another, public pages of
        /// public files excepted; a frame always holds its page's bytes, no
        /// domain keeps more than its limit, and every frame comes back.
        #[test]
        fn page_cache_scopes(ops in proptest::collection::vec(op(), 1..60)) {
            let mut base = rhizo_fs::Fs::new();
            for (i, path) in FILES.iter().enumerate() {
                base.put_file(path, &[i as u8 + 1; 2 * 4096]).unwrap();
                if i == 1 {
                    base.publish(); // two public files, then a private one
                }
            }
            let ino = |f: usize| base.file_at(FILES[f]).unwrap().0;
            let mut cells: Vec<rhizo_fs::Fs> = (0..4).map(|_| base.clone()).collect();
            let mut mem = FakePhysicalMemory::new();
            let mut frames = CountingFrames::new(4096);
            let mut cache = PageCache::new(3);
            let mut filled: BTreeMap<u64, u32> = BTreeMap::new();
            let (mut mapped, mut pins) = (Vec::new(), Vec::new());
            for op in &ops {
                match *op {
                    Op::Map(d, f, i) => {
                        let page = cells[d as usize].page(ino(f), i).unwrap();
                        match cache.share(&page, d, &mut mem, &mut frames) {
                            Share::Kept(phys, fresh) => {
                                if fresh {
                                    filled.insert(phys, d);
                                }
                                let by = filled[&phys];
                                let allowed = by == d || (by == PUBLIC && page.is_public());
                                prop_assert!(allowed, "{:?}: filled for {}", op, by);
                                let mut bytes = [0u8; 4096];
                                mem.read_bytes(phys, &mut bytes);
                                prop_assert_eq!(&bytes[..], page.bytes());
                                mapped.push(phys);
                            }
                            Share::Own => {}
                            Share::OutOfMemory => prop_assert!(false, "memory"),
                        }
                    }
                    Op::Unmap(i) if i < mapped.len() => {
                        frames.free(mapped.swap_remove(i), PageSize::Small);
                    }
                    Op::Write(d, f) => {
                        let fs = &mut cells[d as usize];
                        let flags = rhizo_fs::OpenFlags {
                            write: true,
                            ..Default::default()
                        };
                        let fd = fs.open(None, FILES[f], flags, 1).unwrap();
                        fs.write(fd, b"!", Some(0)).unwrap();
                    }
                    Op::Publish(f) => {
                        for i in 0..2 {
                            let page = base.page(ino(f), i).unwrap();
                            let published = cache.publish(&page, &mut mem, &mut frames);
                            if let Some((phys, fresh)) = published {
                                if fresh {
                                    filled.insert(phys, PUBLIC);
                                }
                                prop_assert_eq!(filled[&phys], PUBLIC);
                                pins.push(page);
                            }
                        }
                    }
                    Op::Withdraw(f) => {
                        let key = base.page(ino(f), 0).unwrap().key();
                        if let Some(at) = pins.iter().position(|p| p.key() == key) {
                            cache.withdraw(&pins.swap_remove(at));
                        }
                    }
                    Op::Trim => {
                        cache.trim(&mut frames);
                    }
                    _ => {}
                }
                for d in 1..4 {
                    prop_assert!(cache.kept(d) <= 3, "domain {} past its limit", d);
                }
            }
            for phys in mapped {
                frames.free(phys, PageSize::Small);
            }
            for page in pins {
                cache.withdraw(&page);
            }
            drop(cells);
            cache.trim(&mut frames);
            prop_assert!(cache.is_empty());
            prop_assert_eq!(frames.outstanding(), 0);
        }
    }
}
