//! Access to a cell's memory through its page tables, with the permission
//! checks the kernel needs for system calls (`EFAULT` instead of a crash).

use crate::USER_END;
use rhizo_hal::PhysicalMemory;
use rhizo_mm::paging::{EntryFormat, PageFlags, PageTable};

/// A user address that is not mapped, or not with the needed permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fault(pub u64);

/// Physical address of user address `addr` and how many bytes from there
/// stay in the same page, if the page is mapped for the user (and writable
/// when `write`).
fn user_phys<F: EntryFormat>(
    pt: &PageTable<F>,
    mem: &impl PhysicalMemory,
    addr: u64,
    write: bool,
) -> Result<(u64, u64), Fault> {
    let t = pt.translate(mem, addr).ok_or(Fault(addr))?;
    if !t.flags.contains(PageFlags::USER) || (write && !t.flags.is_writable()) {
        return Err(Fault(addr));
    }
    Ok((t.phys, t.size.bytes() - (addr & (t.size.bytes() - 1))))
}

/// Calls `f(phys, offset, len)` for each page-contiguous piece of the user
/// range `addr..addr + len`, translating once per page. Stops at the first
/// address that is not accessible. A range reaching past [`USER_END`] is
/// refused before any piece, whatever the page table says: kernel memory is
/// never a cell's, even if a mapping were wrong (spec `isolation`).
fn for_each_piece<F: EntryFormat, M: PhysicalMemory>(
    pt: &PageTable<F>,
    mem: &mut M,
    addr: u64,
    len: usize,
    write: bool,
    mut f: impl FnMut(&mut M, u64, usize, usize),
) -> Result<(), Fault> {
    if len > 0
        && addr
            .checked_add(len as u64)
            .is_none_or(|end| end > USER_END)
    {
        return Err(Fault(addr.max(USER_END)));
    }
    let mut done = 0;
    while done < len {
        let at = addr + done as u64;
        let (phys, room) = user_phys(pt, mem, at, write)?;
        let n = (len - done).min(room as usize);
        f(mem, phys, done, n);
        done += n;
    }
    Ok(())
}

/// Copies `out.len()` bytes from cell address `addr`.
pub fn copy_from<F: EntryFormat>(
    pt: &PageTable<F>,
    mem: &impl PhysicalMemory,
    addr: u64,
    out: &mut [u8],
) -> Result<(), Fault> {
    // Shared access only: wrap so `for_each_piece` can take `&mut`.
    struct Ro<'a, M>(&'a M);
    impl<M: PhysicalMemory> PhysicalMemory for Ro<'_, M> {
        fn read_u64(&self, phys: u64) -> u64 {
            self.0.read_u64(phys)
        }
        fn write_u64(&mut self, _: u64, _: u64) {
            unreachable!("copy_from never writes")
        }
        fn read_bytes(&self, phys: u64, out: &mut [u8]) {
            self.0.read_bytes(phys, out);
        }
    }
    // One bulk read per page piece (a `memcpy` in the kernel).
    for_each_piece(
        pt,
        &mut Ro(mem),
        addr,
        out.len(),
        false,
        |m, phys, off, n| m.read_bytes(phys, &mut out[off..off + n]),
    )
}

/// Copies `data` to cell address `addr` (the pages must be user-writable).
/// A fault leaves memory unchanged.
pub fn copy_to<F: EntryFormat>(
    pt: &PageTable<F>,
    mem: &mut impl PhysicalMemory,
    addr: u64,
    data: &[u8],
) -> Result<(), Fault> {
    for_each_piece(pt, mem, addr, data.len(), true, |_, _, _, _| {})?;
    for_each_piece(pt, mem, addr, data.len(), true, |m, phys, off, n| {
        write_bytes(m, phys, &data[off..off + n]);
    })
}

/// Makes user addresses accessible on demand: pages of a lazily resumed cell
/// (H4) or copy-on-write pages (H5).
pub trait Pager<F: EntryFormat, M: PhysicalMemory> {
    /// Tries to make the page containing `addr` accessible (writable when
    /// `write`); returns whether it did.
    fn page_in(&mut self, pt: &PageTable<F>, mem: &mut M, addr: u64, write: bool) -> bool;

    /// Forgets pages in `addr..addr + len` that are not in memory yet: the
    /// cell unmapped them, so they must not come back.
    fn discard(&mut self, _addr: u64, _len: u64) {}

    /// Whether some pages are not in memory yet (a lazily resumed cell's):
    /// a hole in the tables may then hold one of them.
    fn has_pending(&self) -> bool {
        false
    }

    /// The cell's write-or-execute ranges (design D5 of `node-jit`), whose
    /// missing pages [`page_in`](Self::page_in) backs; `None` where the cell
    /// may have none. Kept by the pager, which reaches them only on a fault,
    /// so copies of cells without such memory pay nothing for them.
    fn wx(&mut self) -> Option<&mut crate::space::WxRanges> {
        None
    }
}

/// No paging: what is not mapped stays a fault.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoPager;

impl<F: EntryFormat, M: PhysicalMemory> Pager<F, M> for NoPager {
    fn page_in(&mut self, _: &PageTable<F>, _: &mut M, _: u64, _: bool) -> bool {
        false
    }
}

/// Like [`copy_from`], paging in what is missing through `pager`.
pub fn read<F: EntryFormat, M: PhysicalMemory>(
    pt: &PageTable<F>,
    mem: &mut M,
    pager: &mut dyn Pager<F, M>,
    addr: u64,
    out: &mut [u8],
) -> Result<(), Fault> {
    // Each successful page-in makes progress; a pager that claims success
    // without mapping anything cannot loop forever.
    for _ in 0..=out.len() / 4096 + 2 {
        match copy_from(pt, mem, addr, out) {
            Err(Fault(at)) if pager.page_in(pt, mem, at, false) => {}
            done => return done,
        }
    }
    copy_from(pt, mem, addr, out)
}

/// Like [`copy_to`], paging in what is missing through `pager`.
pub fn write<F: EntryFormat, M: PhysicalMemory>(
    pt: &PageTable<F>,
    mem: &mut M,
    pager: &mut dyn Pager<F, M>,
    addr: u64,
    data: &[u8],
) -> Result<(), Fault> {
    for _ in 0..=data.len() / 4096 + 2 {
        match copy_to(pt, mem, addr, data) {
            Err(Fault(at)) if pager.page_in(pt, mem, at, true) => {}
            done => return done,
        }
    }
    copy_to(pt, mem, addr, data)
}

/// Writes `data` at a physical address.
pub fn write_bytes(mem: &mut impl PhysicalMemory, phys: u64, data: &[u8]) {
    mem.write_bytes(phys, data);
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhizo_hal::testing::FakePhysicalMemory;
    use rhizo_mm::paging::{Aarch64, PageSize};

    fn setup() -> (PageTable<Aarch64>, FakePhysicalMemory) {
        let mut mem = FakePhysicalMemory::new();
        let mut next = 0x100_0000;
        let mut alloc = || {
            next += 0x1000;
            Some(next)
        };
        let pt = PageTable::new(&mut mem, &mut alloc).unwrap();
        let rw = PageFlags::USER | PageFlags::WRITABLE | PageFlags::NO_EXECUTE;
        pt.map(&mut mem, &mut alloc, 0x1000, 0x50_0000, PageSize::Small, rw)
            .unwrap();
        pt.map(
            &mut mem,
            &mut alloc,
            0x2000,
            0x50_1000,
            PageSize::Small,
            PageFlags::USER,
        )
        .unwrap();
        pt.map(
            &mut mem,
            &mut alloc,
            0x3000,
            0x50_2000,
            PageSize::Small,
            PageFlags::WRITABLE | PageFlags::NO_EXECUTE,
        )
        .unwrap();
        (pt, mem)
    }

    #[test]
    fn round_trip_across_a_page_boundary() {
        let (pt, mut mem) = setup();
        copy_to(&pt, &mut mem, 0x1ffc, b"rhizo").unwrap_err(); // 0x2000 is read-only
        copy_to(&pt, &mut mem, 0x1ff9, b"rhizo").unwrap();
        let mut back = [0u8; 5];
        copy_from(&pt, &mem, 0x1ff9, &mut back).unwrap();
        assert_eq!(&back, b"rhizo");
    }

    /// Maps a zeroed user page on demand, as a lazily resumed cell would.
    struct MapOnDemand(u64);

    impl Pager<Aarch64, FakePhysicalMemory> for MapOnDemand {
        fn page_in(
            &mut self,
            pt: &PageTable<Aarch64>,
            mem: &mut FakePhysicalMemory,
            addr: u64,
            _: bool,
        ) -> bool {
            self.0 += 0x1000;
            let rw = PageFlags::USER | PageFlags::WRITABLE | PageFlags::NO_EXECUTE;
            let mut next = 0x70_0000 + self.0 * 2;
            let mut alloc = || {
                next += 0x1000;
                Some(next)
            };
            pt.map(
                mem,
                &mut alloc,
                addr & !0xfff,
                0x60_0000 + self.0,
                PageSize::Small,
                rw,
            )
            .is_ok()
        }
    }

    #[test]
    fn the_pager_brings_in_missing_pages() {
        let (pt, mut mem) = setup();
        let mut pager = MapOnDemand(0);
        write(&pt, &mut mem, &mut pager, 0x8ffc, b"across").unwrap();
        let mut back = [0u8; 6];
        read(&pt, &mut mem, &mut pager, 0x8ffc, &mut back).unwrap();
        assert_eq!(&back, b"across");
        assert_eq!(pager.0, 0x2000, "two pages brought in, once each");
        assert_eq!(
            read(&pt, &mut mem, &mut NoPager, 0xa000, &mut back),
            Err(Fault(0xa000))
        );
    }

    #[test]
    fn faults_report_the_first_bad_address() {
        let (pt, mut mem) = setup();
        let mut buf = [0u8; 4];
        assert_eq!(copy_from(&pt, &mem, 0x8000, &mut buf), Err(Fault(0x8000)));
        assert_eq!(
            copy_from(&pt, &mem, 0x3000, &mut buf),
            Err(Fault(0x3000)),
            "kernel-only page"
        );
        assert_eq!(
            copy_to(&pt, &mut mem, 0x2000, b"x"),
            Err(Fault(0x2000)),
            "read-only page"
        );
        // A failed copy leaves memory unchanged.
        copy_to(&pt, &mut mem, 0x1ffe, b"abcd").unwrap_err();
        let mut two = [0u8; 2];
        copy_from(&pt, &mem, 0x1ffe, &mut two).unwrap();
        assert_eq!(two, [0, 0]);
    }

    /// Kernel memory mapped as if it were the cell's (a wrong user entry, as
    /// a bug in a mapping call could leave): still never copied.
    fn kernel_mapped_for_the_user() -> (PageTable<Aarch64>, FakePhysicalMemory) {
        let (pt, mut mem) = setup();
        let mut next = 0x200_0000;
        let mut alloc = || {
            next += 0x1000;
            Some(next)
        };
        let rw = PageFlags::USER | PageFlags::WRITABLE | PageFlags::NO_EXECUTE;
        for (virt, phys) in [
            (0xffff_8000_0000_0000, 0x90_0000),
            (USER_END, 0x91_0000),
            (USER_END - 0x1000, 0x92_0000),
        ] {
            pt.map(&mut mem, &mut alloc, virt, phys, PageSize::Small, rw)
                .unwrap();
        }
        (pt, mem)
    }

    proptest::proptest! {
        #[test]
        fn ranges_outside_the_user_half_are_refused(
            addr in proptest::prop_oneof![
                USER_END - 0x2000..USER_END + 0x2000,
                0xffff_8000_0000_0000u64..0xffff_8000_0000_2000,
                proptest::prelude::any::<u64>(),
            ],
            len in 1usize..0x3000,
        ) {
            let (pt, mut mem) = kernel_mapped_for_the_user();
            let mut buf = vec![0x77u8; len];
            let past = addr.checked_add(len as u64).is_none_or(|end| end > USER_END);
            let read = copy_from(&pt, &mem, addr, &mut buf);
            let wrote = copy_to(&pt, &mut mem, addr, &buf);
            if past {
                proptest::prop_assert_eq!(read, Err(Fault(addr.max(USER_END))));
                proptest::prop_assert_eq!(wrote, Err(Fault(addr.max(USER_END))));
            }
            for phys in [0x90_0000, 0x91_0000] {
                proptest::prop_assert_eq!(mem.read_u64(phys), 0, "kernel memory untouched");
            }
        }
    }
}
