use super::*;
use crate::paging::{Aarch64, PageFlags, X86_64};
use proptest::prelude::*;
use rhizo_hal::testing::{FakePhysicalMemory, RecordingTlb};
use std::collections::BTreeMap;

/// Frames with reference counts; panics on a bad or double free.
#[derive(Default)]
struct Counted {
    next: u64,
    refs: BTreeMap<u64, (u32, PageSize)>,
    recycled: Vec<u64>,
    /// No whole 2 MiB block left: `alloc_large` fails.
    no_large: bool,
}

impl Counted {
    fn take(&mut self, size: PageSize) -> u64 {
        let phys = match size {
            PageSize::Small => self.recycled.pop().unwrap_or_else(|| {
                self.next += FRAME_SIZE;
                (1 << 30) + self.next
            }),
            _ => {
                self.next = (self.next + (2 << 20)) & !((2 << 20) - 1);
                self.next += 2 << 20;
                (1 << 32) + self.next
            }
        };
        assert!(self.refs.insert(phys, (1, size)).is_none());
        phys
    }

    fn live(&self) -> usize {
        self.refs.len()
    }
}

impl Frames for Counted {
    fn alloc(&mut self) -> Option<u64> {
        Some(self.take(PageSize::Small))
    }
    fn alloc_large(&mut self) -> Option<u64> {
        (!self.no_large).then(|| self.take(PageSize::Large))
    }
    fn free(&mut self, phys: u64, size: PageSize) {
        let (count, s) = self
            .refs
            .get_mut(&phys)
            .unwrap_or_else(|| panic!("free of unknown {phys:#x}"));
        assert_eq!(*s, size, "size of {phys:#x}");
        *count -= 1;
        if *count == 0 {
            self.refs.remove(&phys);
            if size == PageSize::Small {
                self.recycled.push(phys);
            }
        }
    }
    fn share(&mut self, phys: u64) {
        self.refs
            .get_mut(&phys)
            .unwrap_or_else(|| panic!("share of unknown {phys:#x}"))
            .0 += 1;
    }
    fn is_shared(&self, phys: u64) -> bool {
        self.refs.get(&phys).is_some_and(|r| r.0 > 1)
    }
}

const RW: PageFlags = PageFlags::USER
    .union(PageFlags::WRITABLE)
    .union(PageFlags::NO_EXECUTE);
const RO: PageFlags = PageFlags::USER.union(PageFlags::NO_EXECUTE);
/// A write-or-execute page, writing (change `node-jit`).
const WX: PageFlags = RW.union(PageFlags::WRITE_OR_EXEC);

struct World<F> {
    mem: FakePhysicalMemory,
    frames: Counted,
    _f: core::marker::PhantomData<F>,
}

impl<F: EntryFormat> World<F> {
    fn new() -> Self {
        Self {
            mem: FakePhysicalMemory::new(),
            frames: Counted::default(),
            _f: core::marker::PhantomData,
        }
    }

    fn table(&mut self) -> PageTable<F> {
        let frames = &mut self.frames;
        PageTable::new(&mut self.mem, &mut || frames.alloc()).unwrap()
    }

    fn map(&mut self, pt: &PageTable<F>, virt: u64, size: PageSize, flags: PageFlags) {
        pt.unshare_path(&mut self.mem, &mut self.frames, virt)
            .unwrap();
        let page = match size {
            PageSize::Small => self.frames.alloc(),
            _ => self.frames.alloc_large(),
        }
        .unwrap();
        self.mem.write_u64(page, 0);
        let frames = &mut self.frames;
        pt.map(
            &mut self.mem,
            &mut || frames.alloc(),
            virt,
            page,
            size,
            flags,
        )
        .unwrap();
    }

    fn unmap(&mut self, pt: &PageTable<F>, virt: u64) {
        pt.unshare_path(&mut self.mem, &mut self.frames, virt)
            .unwrap();
        let (phys, size) = pt
            .unmap(&mut self.mem, &mut RecordingTlb::default(), virt)
            .unwrap();
        self.frames.free(phys, size);
    }

    /// Writes like a cell: resolves copy-on-write first if needed.
    fn write(&mut self, pt: &PageTable<F>, virt: u64, value: u64) -> bool {
        let t = pt.translate(&self.mem, virt).unwrap();
        if !t.flags.is_writable()
            && !pt
                .resolve_write_wx(
                    &mut self.mem,
                    &mut self.frames,
                    &mut RecordingTlb::default(),
                    virt,
                )
                .unwrap()
        {
            return false;
        }
        let t = pt.translate(&self.mem, virt).unwrap();
        assert!(t.flags.is_writable(), "writable after resolving");
        self.mem.write_u64(t.phys & !7, value);
        true
    }

    /// The page `virt` lies in.
    fn page(&self, pt: &PageTable<F>, virt: u64) -> u64 {
        let t = pt.translate(&self.mem, virt).unwrap();
        t.phys & !(t.size.bytes() - 1)
    }

    fn flags(&self, pt: &PageTable<F>, virt: u64) -> PageFlags {
        pt.translate(&self.mem, virt).unwrap().flags
    }

    /// Every page of the user half and how it is mapped.
    fn leaves(&self, pt: &PageTable<F>) -> Vec<(u64, u64, PageSize, PageFlags)> {
        let mut out = Vec::new();
        pt.for_each_lower_leaf(&self.mem, &mut |v, p, s, f| out.push((v, p, s, f)));
        out
    }

    fn read(&self, pt: &PageTable<F>, virt: u64) -> u64 {
        self.mem
            .read_u64(pt.translate(&self.mem, virt).unwrap().phys & !7)
    }

    /// Fetches an instruction like a cell: resolves a writing
    /// write-or-execute page first if needed; returns whether it may run.
    fn fetch(&mut self, pt: &PageTable<F>, virt: u64) -> bool {
        if self.flags(pt, virt).is_executable() {
            return true;
        }
        if !pt
            .resolve_exec(&mut self.mem, &mut self.frames, virt)
            .unwrap()
        {
            return false;
        }
        let flags = self.flags(pt, virt);
        assert!(flags.is_executable() && !flags.is_writable(), "running");
        true
    }
}

/// No leaf of `pt` allows writes and instruction fetches at once, and every
/// write-or-execute page is writing or running (change `node-jit`).
fn assert_never_wx<F: EntryFormat>(w: &World<F>, pt: &PageTable<F>) {
    for (virt, _, _, f) in w.leaves(pt) {
        assert!(
            !(f.is_writable() && f.is_executable()),
            "{virt:#x} writable and executable: {f:?}"
        );
        if f.contains(PageFlags::WRITE_OR_EXEC) {
            let cow = f.contains(PageFlags::COPY_ON_WRITE);
            let writing = (f.is_writable() || cow) && !f.is_executable();
            let running = !f.is_writable() && !cow && f.is_executable();
            assert!(writing || running, "{virt:#x} in no state: {f:?}");
        }
    }
}

/// A write-or-execute page alone: written while writing, made running by a
/// fetch (same frame, read-only, executable), made writing again by a write
/// (same frame, writable, not executable), and so on; an ordinary page
/// never runs.
fn wx_page_flips_on_faults<F: EntryFormat>() {
    let mut w = World::<F>::new();
    let pt = w.table();
    w.map(&pt, 0x1000, PageSize::Small, WX);
    w.map(&pt, 0x2000, PageSize::Small, RW);
    let frame = w.page(&pt, 0x1000);
    for round in 1..4 {
        assert!(w.write(&pt, 0x1000, round));
        let f = w.flags(&pt, 0x1000);
        assert!(f.is_writable() && !f.is_executable(), "writing {f:?}");
        assert!(w.fetch(&pt, 0x1008));
        assert_eq!(w.read(&pt, 0x1000), round);
        assert_eq!(w.page(&pt, 0x1000), frame, "never copied");
        assert_eq!(
            pt.resolve_exec(&mut w.mem, &mut w.frames, 0x1000),
            Ok(true),
            "a stale fault changes nothing"
        );
        assert_never_wx(&w, &pt);
    }
    assert!(w.fetch(&pt, 0x1000));
    let mut tlb = RecordingTlb::default();
    assert_eq!(
        pt.resolve_write(&mut w.mem, &mut w.frames, &mut tlb, 0x1000),
        Ok(false),
        "copy-on-write resolution leaves running pages to resolve_write_wx"
    );
    assert!(w.flags(&pt, 0x1000).is_executable());
    assert!(!w.fetch(&pt, 0x2000), "an ordinary page never runs");
    assert!(w.flags(&pt, 0x2000).is_writable());
    assert_eq!(
        pt.resolve_exec(&mut w.mem, &mut w.frames, 0x3000),
        Ok(false),
        "nothing mapped"
    );
    pt.destroy_lower_half(&mut w.mem, &mut w.frames);
    assert_eq!(w.frames.live(), 0);
}

/// After a fork, each side's changes between writing and running stay its
/// own: a fetch makes the tables private first, and a write copies the
/// frame still shared (spec `isolation`).
fn wx_pages_stay_private_after_fork<F: EntryFormat>() {
    let mut w = World::<F>::new();
    let parent = w.table();
    w.map(&parent, 0x1000, PageSize::Small, WX);
    w.map(&parent, 0x2000, PageSize::Small, WX);
    assert!(w.write(&parent, 0x1000, 1) && w.write(&parent, 0x2000, 2));
    assert!(w.fetch(&parent, 0x2000)); // one running, one writing
    let child = parent.fork_lower_half(&mut w.mem, &mut w.frames).unwrap();
    let shared = w.page(&parent, 0x1000);
    // The child runs the writing page: its tables become its own, and the
    // parent's entry stays writing (copy-on-write now).
    assert!(w.fetch(&child, 0x1000));
    assert!(!w.flags(&parent, 0x1000).is_executable());
    assert_eq!(w.page(&child, 0x1000), shared, "running shares the frame");
    // Each rewrites both pages: private copies, the other's untouched.
    assert!(w.write(&child, 0x1000, 10) && w.write(&child, 0x2000, 20));
    assert_ne!(w.page(&child, 0x1000), shared);
    assert_eq!((w.read(&parent, 0x1000), w.read(&parent, 0x2000)), (1, 2));
    assert!(w.write(&parent, 0x2000, 3));
    assert!(w.fetch(&parent, 0x1000) && w.fetch(&parent, 0x2000));
    assert!(w.fetch(&child, 0x1000) && w.fetch(&child, 0x2000));
    assert_eq!((w.read(&parent, 0x1000), w.read(&parent, 0x2000)), (1, 3));
    assert_eq!((w.read(&child, 0x1000), w.read(&child, 0x2000)), (10, 20));
    for pt in [&parent, &child] {
        assert_never_wx(&w, pt);
        for at in [0x1000, 0x2000] {
            assert!(w.flags(pt, at).contains(PageFlags::WRITE_OR_EXEC));
        }
    }
    for pt in [parent, child] {
        pt.destroy_lower_half(&mut w.mem, &mut w.frames);
    }
    assert_eq!(w.frames.live(), 0);
}

/// A clone (a template's instance) copies writing pages and shares running
/// ones, which a write then copies: the bit survives both ways.
fn wx_pages_in_clones<F: EntryFormat>() {
    let mut w = World::<F>::new();
    let source = w.table();
    w.map(&source, 0x1000, PageSize::Small, WX);
    w.map(&source, 0x2000, PageSize::Small, WX);
    assert!(w.write(&source, 0x1000, 1) && w.write(&source, 0x2000, 2));
    assert!(w.fetch(&source, 0x2000));
    let copy = source
        .clone_lower_half(&mut w.mem, &mut w.frames, 0..LOWER_HALF_END)
        .unwrap();
    assert_ne!(w.page(&copy, 0x1000), w.page(&source, 0x1000), "copied");
    assert_eq!(w.page(&copy, 0x2000), w.page(&source, 0x2000), "shared");
    assert!(w.write(&copy, 0x2000, 20));
    assert_ne!(w.page(&copy, 0x2000), w.page(&source, 0x2000));
    assert_eq!(w.read(&source, 0x2000), 2);
    assert!(w.fetch(&copy, 0x1000) && w.fetch(&copy, 0x2000));
    assert_eq!((w.read(&copy, 0x1000), w.read(&copy, 0x2000)), (1, 20));
    for pt in [&source, &copy] {
        assert_never_wx(&w, pt);
    }
    for pt in [source, copy] {
        pt.destroy_lower_half(&mut w.mem, &mut w.frames);
    }
    assert_eq!(w.frames.live(), 0);
}

#[test]
fn wx_aarch64() {
    wx_page_flips_on_faults::<Aarch64>();
    wx_pages_stay_private_after_fork::<Aarch64>();
    wx_pages_in_clones::<Aarch64>();
}

#[test]
fn wx_x86_64() {
    wx_page_flips_on_faults::<X86_64>();
    wx_pages_stay_private_after_fork::<X86_64>();
    wx_pages_in_clones::<X86_64>();
}

fn fork_shares_everything_and_writes_stay_private<F: EntryFormat>() {
    let mut w = World::<F>::new();
    let parent = w.table();
    w.map(&parent, 0x1000, PageSize::Small, RW);
    w.map(&parent, 0x40_0000, PageSize::Small, RO);
    w.map(&parent, 0x4000_0000_0000, PageSize::Large, RW);
    w.write(&parent, 0x1000, 11);
    w.write(&parent, 0x4000_0000_0008, 22);
    let before = w.frames.live();

    let child = parent.fork_lower_half(&mut w.mem, &mut w.frames).unwrap();
    assert_eq!(
        w.frames.live(),
        before + 1,
        "a fork allocates only the root"
    );
    assert_eq!(
        child.lower_half_tables(&w.mem),
        parent.lower_half_tables(&w.mem)
    );
    for pt in [&parent, &child] {
        assert!(
            !pt.translate(&w.mem, 0x1000).unwrap().flags.is_writable(),
            "shared: read-only"
        );
        assert_eq!(w.read(pt, 0x1000), 11);
    }

    assert!(w.write(&child, 0x1000, 33));
    assert_eq!((w.read(&parent, 0x1000), w.read(&child, 0x1000)), (11, 33));
    assert!(
        w.write(&parent, 0x4000_0000_0008, 44),
        "2 MiB pages are copied too"
    );
    assert_eq!(w.read(&child, 0x4000_0000_0008), 22);
    assert!(
        !w.write(&child, 0x40_0000, 1),
        "a read-only page stays read-only"
    );

    parent.destroy_lower_half(&mut w.mem, &mut w.frames);
    assert_eq!(w.read(&child, 0x1000), 33, "the child outlives its parent");
    assert!(
        w.write(&child, 0x4000_0000_0010, 5),
        "sole owner: made writable"
    );
    child.destroy_lower_half(&mut w.mem, &mut w.frames);
    assert_eq!(w.frames.live(), 0, "every frame returned once");
}

fn mapping_in_a_child_is_not_seen_by_the_parent<F: EntryFormat>() {
    let mut w = World::<F>::new();
    let parent = w.table();
    w.map(&parent, 0x1000, PageSize::Small, RW);
    let child = parent.fork_lower_half(&mut w.mem, &mut w.frames).unwrap();
    w.map(&child, 0x2000, PageSize::Small, RW); // same last-level table
    w.unmap(&child, 0x1000);
    assert!(parent.translate(&w.mem, 0x2000).is_none());
    assert_eq!(w.read(&parent, 0x1000), 0, "still mapped in the parent");
    assert!(child.translate(&w.mem, 0x1000).is_none());
    for pt in [parent, child] {
        pt.destroy_lower_half(&mut w.mem, &mut w.frames);
    }
    assert_eq!(w.frames.live(), 0);
}

/// Without a free 2 MiB block, a write to a shared 2 MiB page copies it
/// into 4 KiB pages: every byte is kept, the write stays private.
fn large_page_copied_as_small_pages<F: EntryFormat>() {
    let mut w = World::<F>::new();
    let parent = w.table();
    w.map(&parent, 0x4000_0000_0000, PageSize::Large, RW);
    w.write(&parent, 0x4000_0000_0008, 22);
    w.write(&parent, 0x4000_001f_f000, 23);
    let child = parent.fork_lower_half(&mut w.mem, &mut w.frames).unwrap();
    w.frames.no_large = true;
    assert!(w.write(&child, 0x4000_0000_0010, 5));
    let t = child.translate(&w.mem, 0x4000_001f_f000).unwrap();
    assert_eq!((t.size, t.flags.is_writable()), (PageSize::Small, true));
    assert_eq!(w.read(&child, 0x4000_0000_0008), 22);
    assert_eq!(w.read(&child, 0x4000_001f_f000), 23);
    assert_eq!(w.read(&parent, 0x4000_0000_0010), 0, "private to the child");
    assert!(
        w.write(&parent, 0x4000_0000_0008, 7),
        "sole owner: writable"
    );
    assert_eq!(w.read(&child, 0x4000_0000_0008), 22);
    for pt in [parent, child] {
        pt.destroy_lower_half(&mut w.mem, &mut w.frames);
    }
    assert_eq!(w.frames.live(), 0, "every frame returned once");
}

/// A page copied for a write replaces the shared one break-before-make:
/// its entry is flushed once, between the two; a page made writable in
/// place (its last owner) is left for the caller to flush.
fn copies_are_break_before_make<F: EntryFormat>() {
    let mut w = World::<F>::new();
    let parent = w.table();
    w.map(&parent, 0x1000, PageSize::Small, RW);
    w.map(&parent, 0x3000, PageSize::Small, RW);
    w.map(&parent, 0x4000_0000_0000, PageSize::Large, RW);
    let child = parent.fork_lower_half(&mut w.mem, &mut w.frames).unwrap();
    for (virt, page) in [(0x1008, 0x1000), (0x4000_0000_1000, 0x4000_0000_0000)] {
        let mut tlb = RecordingTlb::default();
        assert_eq!(
            child.resolve_write(&mut w.mem, &mut w.frames, &mut tlb, virt),
            Ok(true)
        );
        assert_eq!(tlb.flushed, [page], "one flush between the entries");
    }
    parent.destroy_lower_half(&mut w.mem, &mut w.frames);
    let mut tlb = RecordingTlb::default();
    let before = child.translate(&w.mem, 0x3000).unwrap().phys;
    assert_eq!(
        child.resolve_write(&mut w.mem, &mut w.frames, &mut tlb, 0x3000),
        Ok(true)
    );
    assert_eq!(child.translate(&w.mem, 0x3000).unwrap().phys, before);
    assert!(tlb.flushed.is_empty(), "made writable in place");
    child.destroy_lower_half(&mut w.mem, &mut w.frames);
    assert_eq!(w.frames.live(), 0, "every frame returned once");
}

/// A clone (design D2 of `add-image-cache`) has tables of its own, shares
/// read-only pages and starts with a private copy of every writable one:
/// neither side sees the other's writes, and the source is not changed.
fn clone_shares_read_only_pages_and_copies_the_rest<F: EntryFormat>() {
    let mut w = World::<F>::new();
    let source = w.table();
    w.map(&source, 0x40_0000, PageSize::Small, RO);
    w.map(&source, 0x60_0000, PageSize::Small, RW);
    w.map(&source, 0x4000_0000_0000, PageSize::Large, RW);
    w.write(&source, 0x60_0000, 11);
    w.write(&source, 0x4000_0000_0008, 22);
    let ro = w.page(&source, 0x40_0000);
    w.mem.write_u64(ro, 33);
    let before = w.frames.live();
    let tables = source.lower_half_tables(&w.mem);
    let source_leaves = w.leaves(&source);

    let copy = source
        .clone_lower_half(&mut w.mem, &mut w.frames, 0..LOWER_HALF_END)
        .unwrap();
    assert_eq!(w.leaves(&source), source_leaves, "the source is only read");
    assert_eq!(copy.lower_half_tables(&w.mem), tables);
    assert_eq!(
        w.frames.live(),
        before + tables + 2,
        "own tables and the two writable pages, nothing else"
    );
    assert_eq!(w.page(&copy, 0x40_0000), ro, "read-only: shared");
    assert_eq!(w.read(&copy, 0x40_0000), 33);
    assert!(!w.write(&copy, 0x40_0000, 1), "and still read-only");
    for (at, value) in [(0x60_0000, 11), (0x4000_0000_0008, 22)] {
        assert_ne!(w.page(&copy, at), w.page(&source, at));
        assert_eq!(w.read(&copy, at), value, "a private copy");
        assert!(w.flags(&copy, at).is_writable(), "writable at once");
    }
    assert!(w.write(&copy, 0x60_0000, 44));
    assert!(w.write(&source, 0x4000_0000_0008, 55));
    assert_eq!(
        (w.read(&source, 0x60_0000), w.read(&copy, 0x60_0000)),
        (11, 44)
    );
    assert_eq!(w.read(&copy, 0x4000_0000_0008), 22);
    w.map(&copy, 0x7f00_0000_0000, PageSize::Small, RW);
    assert!(source.translate(&w.mem, 0x7f00_0000_0000).is_none());

    source.destroy_lower_half(&mut w.mem, &mut w.frames);
    assert_eq!(w.read(&copy, 0x40_0000), 33, "the copy outlives its source");
    copy.destroy_lower_half(&mut w.mem, &mut w.frames);
    assert_eq!(w.frames.live(), 0, "every frame returned once");

    // Out of frames halfway: everything taken is given back.
    let source = w.table();
    w.map(&source, 0x40_0000, PageSize::Small, RO);
    w.map(&source, 0x4000_0000_0000, PageSize::Large, RW);
    let before = w.frames.live();
    w.frames.no_large = true;
    assert_eq!(
        source
            .clone_lower_half(&mut w.mem, &mut w.frames, 0..LOWER_HALF_END)
            .err(),
        Some(MapError::OutOfFrames)
    );
    assert_eq!(w.frames.live(), before);
    assert!(!w.frames.is_shared(w.page(&source, 0x40_0000)));
    source.destroy_lower_half(&mut w.mem, &mut w.frames);
    assert_eq!(w.frames.live(), 0);
}

/// A full copy (design D21 of `add-cell-hierarchy`) shares only the
/// read-only pages it is told to, whatever the source shares with others:
/// every other page is a private copy with the same bytes and permissions,
/// and its writes take no frame. `shares` sees what another hierarchy can
/// still reach.
fn full_copy_shares_only_kept_pages<F: EntryFormat>() {
    let mut w = World::<F>::new();
    let parent = w.table();
    w.map(&parent, 0x40_0000, PageSize::Small, RO);
    w.map(&parent, 0x41_0000, PageSize::Small, RO);
    w.map(&parent, 0x60_0000, PageSize::Small, WX);
    w.map(&parent, 0x4000_0000_0000, PageSize::Large, RW);
    for (at, value) in [(0x40_0000, 1), (0x41_0000, 2)] {
        let page = w.page(&parent, at);
        w.mem.write_u64(page, value);
    }
    w.write(&parent, 0x60_0000, 3);
    w.write(&parent, 0x4000_0000_0008, 4);
    let public = w.page(&parent, 0x40_0000);
    let mut skip = |p| p == public;
    assert!(!parent.shares(&w.mem, &w.frames, &mut skip), "alone");
    // The source shares its tables and pages with a fork.
    let source = parent.fork_lower_half(&mut w.mem, &mut w.frames).unwrap();
    assert!(source.shares(&w.mem, &w.frames, &mut skip), "with the fork");
    let source_leaves = w.leaves(&source);
    let live = w.frames.live();

    let copy = source
        .copy_lower_half(&mut w.mem, &mut w.frames, &mut |p| p == public)
        .unwrap();
    assert_eq!(w.leaves(&source), source_leaves, "the source is only read");
    let pages = w.leaves(&copy);
    assert_eq!(
        w.frames.live(),
        live + copy.lower_half_tables(&w.mem) + 3,
        "own tables and every page but the public one"
    );
    for ((virt, phys, size, flags), source) in pages.iter().zip(&source_leaves) {
        assert_eq!(
            (*virt, *size, *flags),
            (source.0, source.2, source.3),
            "{virt:#x}"
        );
        assert_eq!(*phys == source.1, *virt == 0x40_0000, "{virt:#x}");
    }
    for (at, value) in [
        (0x40_0000, 1),
        (0x41_0000, 2),
        (0x60_0000, 3),
        (0x4000_0000_0008, 4),
    ] {
        assert_eq!(w.read(&copy, at), value, "{at:#x}");
    }
    assert!(
        !copy.shares(&w.mem, &w.frames, &mut skip),
        "only the public page"
    );
    let live = w.frames.live();
    assert!(w.write(&copy, 0x60_0000, 5) && w.write(&copy, 0x4000_0000_0008, 6));
    assert_eq!(w.frames.live(), live, "its pages are its own: no copy");
    assert_eq!(w.read(&source, 0x60_0000), 3, "the source keeps its bytes");
    assert!(!w.write(&copy, 0x41_0000, 7), "read-only stays read-only");

    parent.destroy_lower_half(&mut w.mem, &mut w.frames);
    assert!(
        !source.shares(&w.mem, &w.frames, &mut skip),
        "the fork ended"
    );
    assert!(
        source.shares(&w.mem, &w.frames, &mut |_| false),
        "the public page"
    );
    for pt in [source, copy] {
        pt.destroy_lower_half(&mut w.mem, &mut w.frames);
    }
    assert_eq!(w.frames.live(), 0, "every frame returned once");

    // No 2 MiB block: the large page comes as 4 KiB pages.
    let source = w.table();
    w.map(&source, 0x4000_0000_0000, PageSize::Large, RW);
    w.write(&source, 0x4000_0000_1008, 8);
    w.frames.no_large = true;
    let copy = source
        .copy_lower_half(&mut w.mem, &mut w.frames, &mut |_| true)
        .unwrap();
    w.frames.no_large = false;
    assert_eq!(w.read(&copy, 0x4000_0000_1008), 8);
    assert_eq!(w.leaves(&copy).len(), 512);
    for pt in [source, copy] {
        pt.destroy_lower_half(&mut w.mem, &mut w.frames);
    }
    assert_eq!(w.frames.live(), 0);
}

/// After `unshare_tables`, a child forked from a parent has tables of its
/// own, every page it shares counts its reference and is write-protected in
/// both, and nothing was copied but tables.
fn unshare_tables_leaves_pages_shared<F: EntryFormat>() {
    let mut w = World::<F>::new();
    let parent = w.table();
    w.map(&parent, 0x1000, PageSize::Small, RW);
    w.map(&parent, 0x40_0000, PageSize::Small, RO);
    w.map(&parent, 0x4000_0000_0000, PageSize::Large, RW);
    let child = parent.fork_lower_half(&mut w.mem, &mut w.frames).unwrap();
    let pages = |w: &World<F>, pt| w.leaves(pt).iter().map(|l| l.1).collect::<Vec<_>>();
    let (before, live) = (pages(&w, &parent), w.frames.live());
    child.unshare_tables(&mut w.mem, &mut w.frames).unwrap();
    assert_eq!(pages(&w, &child), before, "no page copied");
    assert_eq!(
        w.frames.live(),
        live + child.lower_half_tables(&w.mem) - 1,
        "a copy of each table below its root, and nothing else"
    );
    for &page in &before {
        assert!(w.frames.is_shared(page), "{page:#x} counts both");
    }
    for pt in [&parent, &child] {
        assert!(!w.flags(pt, 0x1000).is_writable());
        assert!(!w.flags(pt, 0x4000_0000_0000).is_writable());
    }
    assert!(w.write(&child, 0x1000, 7) && w.write(&parent, 0x1000, 8));
    assert_eq!((w.read(&child, 0x1000), w.read(&parent, 0x1000)), (7, 8));
    for pt in [parent, child] {
        pt.destroy_lower_half(&mut w.mem, &mut w.frames);
    }
    assert_eq!(w.frames.live(), 0);
}

#[test]
fn fork_aarch64() {
    clone_shares_read_only_pages_and_copies_the_rest::<Aarch64>();
    fork_shares_everything_and_writes_stay_private::<Aarch64>();
    mapping_in_a_child_is_not_seen_by_the_parent::<Aarch64>();
    large_page_copied_as_small_pages::<Aarch64>();
    copies_are_break_before_make::<Aarch64>();
    unshare_tables_leaves_pages_shared::<Aarch64>();
    full_copy_shares_only_kept_pages::<Aarch64>();
}

#[test]
fn fork_x86_64() {
    clone_shares_read_only_pages_and_copies_the_rest::<X86_64>();
    fork_shares_everything_and_writes_stay_private::<X86_64>();
    mapping_in_a_child_is_not_seen_by_the_parent::<X86_64>();
    large_page_copied_as_small_pages::<X86_64>();
    copies_are_break_before_make::<X86_64>();
    unshare_tables_leaves_pages_shared::<X86_64>();
    full_copy_shares_only_kept_pages::<X86_64>();
}

/// Addresses spread over different tables at every level.
const ADDRS: [u64; 6] = [
    0x1000,
    0x2000,
    0x20_0000,
    0x4000_0000,
    0x80_0000_0000,
    0x7f00_0000_0000,
];

#[derive(Debug, Clone)]
enum Op {
    Map(usize, usize),
    /// Maps a write-or-execute page (change `node-jit`).
    MapWx(usize, usize),
    /// Fetches an instruction from a page.
    Fetch(usize, usize),
    Write(usize, usize, u64),
    Unmap(usize, usize),
    Fork(usize),
    Clone(usize),
    /// A full copy that shares no page (design D21 of
    /// `add-cell-hierarchy`).
    Copy(usize),
    Destroy(usize),
    /// Holds a reference to every page of a cell another one can reach,
    /// after `unshare_tables`, as a snapshot does.
    Hold(usize),
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0..4usize, 0..ADDRS.len()).prop_map(|(c, a)| Op::Map(c, a)),
        (0..4usize, 0..ADDRS.len()).prop_map(|(c, a)| Op::MapWx(c, a)),
        (0..4usize, 0..ADDRS.len()).prop_map(|(c, a)| Op::Fetch(c, a)),
        (0..4usize, 0..ADDRS.len(), 1..1000u64).prop_map(|(c, a, v)| Op::Write(c, a, v)),
        (0..4usize, 0..ADDRS.len()).prop_map(|(c, a)| Op::Unmap(c, a)),
        (0..4usize).prop_map(Op::Fork),
        (0..4usize).prop_map(Op::Clone),
        (0..4usize).prop_map(Op::Destroy),
        (0..4usize).prop_map(Op::Hold),
        (0..4usize).prop_map(Op::Copy),
    ]
}

/// Random sequences on up to four cells against a model of each cell's
/// memory: every read matches the model, a page held never changes, no
/// page is ever writable and executable, a write-or-execute page always
/// runs and an ordinary one never does (spec `virtual-memory`), and all
/// frames return at the end.
fn model_check<F: EntryFormat>(ops: &[Op]) {
    let mut w = World::<F>::new();
    let mut held: Vec<(u64, PageSize, u64)> = Vec::new();
    let first = w.table();
    let mut cells: Vec<(PageTable<F>, BTreeMap<u64, u64>)> = vec![(first, BTreeMap::new())];
    // Write-or-execute addresses, the same in every cell (forks keep them).
    let mut wx = std::collections::BTreeSet::new();
    for op in ops {
        let n = cells.len();
        let used = |a: usize| cells.iter().any(|cell| cell.1.contains_key(&ADDRS[a]));
        match *op {
            Op::Map(c, a) if c < n && !cells[c].1.contains_key(&ADDRS[a]) && !wx.contains(&a) => {
                let pt = PageTable::<F>::from_root(cells[c].0.root());
                w.map(&pt, ADDRS[a], PageSize::Small, RW);
                cells[c].1.insert(ADDRS[a], 0);
            }
            Op::MapWx(c, a) if c < n && !used(a) => {
                let pt = PageTable::<F>::from_root(cells[c].0.root());
                w.map(&pt, ADDRS[a], PageSize::Small, WX);
                cells[c].1.insert(ADDRS[a], 0);
                wx.insert(a);
            }
            Op::Fetch(c, a) if c < n && cells[c].1.contains_key(&ADDRS[a]) => {
                let pt = PageTable::<F>::from_root(cells[c].0.root());
                assert_eq!(w.fetch(&pt, ADDRS[a]), wx.contains(&a), "{op:?}");
            }
            Op::Write(c, a, v) if c < n && cells[c].1.contains_key(&ADDRS[a]) => {
                let pt = PageTable::<F>::from_root(cells[c].0.root());
                assert!(w.write(&pt, ADDRS[a], v));
                cells[c].1.insert(ADDRS[a], v);
            }
            Op::Unmap(c, a) if c < n && cells[c].1.contains_key(&ADDRS[a]) => {
                let pt = PageTable::<F>::from_root(cells[c].0.root());
                w.unmap(&pt, ADDRS[a]);
                cells[c].1.remove(&ADDRS[a]);
            }
            Op::Fork(c) if c < n && n < 4 => {
                let child = cells[c]
                    .0
                    .fork_lower_half(&mut w.mem, &mut w.frames)
                    .unwrap();
                let model = cells[c].1.clone();
                cells.push((child, model));
            }
            Op::Clone(c) if c < n && n < 4 => {
                let copy = cells[c]
                    .0
                    .clone_lower_half(&mut w.mem, &mut w.frames, 0..LOWER_HALF_END)
                    .unwrap();
                let model = cells[c].1.clone();
                cells.push((copy, model));
            }
            Op::Copy(c) if c < n && n < 4 => {
                let copy = cells[c]
                    .0
                    .copy_lower_half(&mut w.mem, &mut w.frames, &mut |_| false)
                    .unwrap();
                assert!(!copy.shares(&w.mem, &w.frames, &mut |_| false), "{op:?}");
                let model = cells[c].1.clone();
                cells.push((copy, model));
            }
            Op::Destroy(c) if c < n && n > 1 => {
                let (pt, _) = cells.remove(c);
                pt.destroy_lower_half(&mut w.mem, &mut w.frames);
            }
            Op::Hold(c) if c < n => {
                let pt = PageTable::<F>::from_root(cells[c].0.root());
                pt.unshare_tables(&mut w.mem, &mut w.frames).unwrap();
                for (_, page, size, _) in w.leaves(&pt) {
                    if w.frames.is_shared(page) {
                        w.frames.share(page);
                        held.push((page, size, w.mem.read_u64(page)));
                    }
                }
            }
            _ => {}
        }
        for &(page, _, v) in &held {
            assert_eq!(w.mem.read_u64(page), v, "held {page:#x} after {op:?}");
        }
        for (pt, model) in &cells {
            assert_never_wx(&w, pt);
            for &a in &ADDRS {
                match model.get(&a) {
                    Some(&v) => assert_eq!(w.read(pt, a), v, "{a:#x} after {op:?}"),
                    None => assert!(pt.translate(&w.mem, a).is_none(), "{a:#x} after {op:?}"),
                }
            }
        }
    }
    for (pt, _) in cells {
        pt.destroy_lower_half(&mut w.mem, &mut w.frames);
    }
    for (page, size, _) in held {
        w.frames.free(page, size);
    }
    assert_eq!(w.frames.live(), 0, "leaked frames");
}

proptest! {
    #[test]
    fn model_aarch64(ops in proptest::collection::vec(op(), 1..80)) {
        model_check::<Aarch64>(&ops);
    }

    #[test]
    fn model_x86_64(ops in proptest::collection::vec(op(), 1..80)) {
        model_check::<X86_64>(&ops);
    }
}

/// A cell sharing the kernel half (x86_64): a write fault or a mapping change
/// at a kernel address is never "resolved", and no kernel table changes.
#[test]
fn kernel_half_is_never_made_writable_or_private() {
    let mut w = World::<X86_64>::new();
    let kernel = w.table();
    let kernel_rw = PageFlags::WRITABLE.union(PageFlags::NO_EXECUTE);
    let read_only = PageFlags::empty();
    w.map(&kernel, 0xffff_8000_0000_0000, PageSize::Small, kernel_rw);
    w.map(&kernel, 0xffff_ffff_8000_0000, PageSize::Small, read_only);
    let cell = w.table();
    cell.share_upper_half(&mut w.mem, &kernel);
    let before = w.mem.clone();
    for virt in [
        0xffff_8000_0000_0000,
        0xffff_ffff_8000_0000,
        0xffff_8000_0001_0000,
    ] {
        assert_eq!(
            cell.resolve_write(
                &mut w.mem,
                &mut w.frames,
                &mut RecordingTlb::default(),
                virt
            ),
            Ok(false)
        );
        cell.unshare_path(&mut w.mem, &mut w.frames, virt).unwrap();
    }
    assert!(w.mem == before, "no kernel entry changed");
}
