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
    /// No large frame is free (fragmented memory).
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
                .resolve_write(
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

    fn read(&self, pt: &PageTable<F>, virt: u64) -> u64 {
        self.mem
            .read_u64(pt.translate(&self.mem, virt).unwrap().phys & !7)
    }
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

/// With no 2 MiB block free, a write to a shared large page copies it into
/// small pages: the writer sees all of its data and its write, the other
/// sharer sees neither change, and every frame comes back.
fn large_page_copied_into_small_pages_without_large_frames<F: EntryFormat>() {
    let mut w = World::<F>::new();
    let parent = w.table();
    let base = 0x4000_0000_0000;
    w.map(&parent, base, PageSize::Large, RW);
    for i in 0..512 {
        w.write(&parent, base + i * FRAME_SIZE + 8, i);
    }
    let child = parent.fork_lower_half(&mut w.mem, &mut w.frames).unwrap();
    w.frames.no_large = true;
    let mut tlb = RecordingTlb::default();
    assert_eq!(
        child.resolve_write(&mut w.mem, &mut w.frames, &mut tlb, base + 0x5008),
        Ok(true)
    );
    assert!(
        tlb.flushed.contains(&(base + 0x5008)),
        "the entry is flushed"
    );
    let t = child.translate(&w.mem, base + 0x5008).unwrap();
    assert_eq!(t.size, PageSize::Small);
    assert!(t.flags.is_writable());
    w.write(&child, base + 0x5008, 99);
    for i in 0..512 {
        let expected = if i == 5 { 99 } else { i };
        assert_eq!(w.read(&child, base + i * FRAME_SIZE + 8), expected);
        assert_eq!(
            w.read(&parent, base + i * FRAME_SIZE + 8),
            i,
            "parent unchanged"
        );
    }
    assert_eq!(
        parent.translate(&w.mem, base).unwrap().size,
        PageSize::Large
    );
    for pt in [parent, child] {
        pt.destroy_lower_half(&mut w.mem, &mut w.frames);
    }
    assert_eq!(w.frames.live(), 0, "every frame returned once");
}

#[test]
fn fork_aarch64() {
    fork_shares_everything_and_writes_stay_private::<Aarch64>();
    mapping_in_a_child_is_not_seen_by_the_parent::<Aarch64>();
    large_page_copied_into_small_pages_without_large_frames::<Aarch64>();
}

#[test]
fn fork_x86_64() {
    fork_shares_everything_and_writes_stay_private::<X86_64>();
    mapping_in_a_child_is_not_seen_by_the_parent::<X86_64>();
    large_page_copied_into_small_pages_without_large_frames::<X86_64>();
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
    Write(usize, usize, u64),
    Unmap(usize, usize),
    Fork(usize),
    Destroy(usize),
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0..4usize, 0..ADDRS.len()).prop_map(|(c, a)| Op::Map(c, a)),
        (0..4usize, 0..ADDRS.len(), 1..1000u64).prop_map(|(c, a, v)| Op::Write(c, a, v)),
        (0..4usize, 0..ADDRS.len()).prop_map(|(c, a)| Op::Unmap(c, a)),
        (0..4usize).prop_map(Op::Fork),
        (0..4usize).prop_map(Op::Destroy),
    ]
}

/// Random sequences on up to four cells against a model of each cell's
/// memory: every read matches the model, and all frames return at the end.
fn model_check<F: EntryFormat>(ops: &[Op]) {
    let mut w = World::<F>::new();
    let first = w.table();
    let mut cells: Vec<(PageTable<F>, BTreeMap<u64, u64>)> = vec![(first, BTreeMap::new())];
    for op in ops {
        let n = cells.len();
        match *op {
            Op::Map(c, a) if c < n && !cells[c].1.contains_key(&ADDRS[a]) => {
                let pt = PageTable::<F>::from_root(cells[c].0.root());
                w.map(&pt, ADDRS[a], PageSize::Small, RW);
                cells[c].1.insert(ADDRS[a], 0);
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
            Op::Destroy(c) if c < n && n > 1 => {
                let (pt, _) = cells.remove(c);
                pt.destroy_lower_half(&mut w.mem, &mut w.frames);
            }
            _ => {}
        }
        for (pt, model) in &cells {
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
