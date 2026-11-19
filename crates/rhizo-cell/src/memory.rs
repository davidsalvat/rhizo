//! Access to a cell's memory through its page tables, with the permission
//! checks the kernel needs for system calls (`EFAULT` instead of a crash).

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
/// address that is not accessible.
fn for_each_piece<F: EntryFormat, M: PhysicalMemory>(
    pt: &PageTable<F>,
    mem: &mut M,
    addr: u64,
    len: usize,
    write: bool,
    mut f: impl FnMut(&mut M, u64, usize, usize),
) -> Result<(), Fault> {
    let mut done = 0;
    while done < len {
        let at = addr.checked_add(done as u64).ok_or(Fault(addr))?;
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
    }
    for_each_piece(
        pt,
        &mut Ro(mem),
        addr,
        out.len(),
        false,
        |m, phys, off, n| {
            for (i, byte) in out[off..off + n].iter_mut().enumerate() {
                let p = phys + i as u64;
                *byte = (m.read_u64(p & !7) >> ((p & 7) * 8)) as u8;
            }
        },
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

/// Writes `data` at a physical address through 64-bit accesses (whole words
/// directly, partial words by read-modify-write).
pub fn write_bytes(mem: &mut impl PhysicalMemory, phys: u64, data: &[u8]) {
    let mut i = 0;
    while i < data.len() {
        let p = phys + i as u64;
        if p.is_multiple_of(8) && data.len() - i >= 8 {
            let word: [u8; 8] = data[i..i + 8].try_into().unwrap_or([0; 8]);
            mem.write_u64(p, u64::from_le_bytes(word));
            i += 8;
        } else {
            write_byte(mem, p, data[i]);
            i += 1;
        }
    }
}

/// Writes one byte at a physical address through 64-bit accesses.
fn write_byte(mem: &mut impl PhysicalMemory, phys: u64, byte: u8) {
    let word = mem.read_u64(phys & !7);
    let shift = (phys & 7) * 8;
    mem.write_u64(
        phys & !7,
        (word & !(0xff << shift)) | ((byte as u64) << shift),
    );
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
}
