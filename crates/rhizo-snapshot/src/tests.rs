use super::*;
use rhizo_cell::space;
use rhizo_cell::testing::CountingFrames;
use rhizo_hal::testing::{FakePhysicalMemory, RecordingTlb};
use rhizo_mm::paging::{Aarch64, X86_64};

const RW: PageFlags = PageFlags::USER
    .union(PageFlags::WRITABLE)
    .union(PageFlags::NO_EXECUTE);
const RX: PageFlags = PageFlags::USER;

/// A cell with a code page, a written data page and a 2 MiB region.
fn cell<F: EntryFormat>(mem: &mut FakePhysicalMemory, frames: &mut CountingFrames) -> Cell {
    let mut tlb = RecordingTlb::default();
    let pt = PageTable::<F>::new(mem, &mut || frames.alloc()).unwrap();
    space::map_anonymous(&pt, mem, &mut tlb, frames, 0x40_0000, 0x1000, RX).unwrap();
    space::map_anonymous(&pt, mem, &mut tlb, frames, 0x60_0000, 0x1000, RW).unwrap();
    space::map_anonymous(&pt, mem, &mut tlb, frames, 0x4000_0000_0000, 2 << 20, RW).unwrap();
    rhizo_cell::memory::copy_to(&pt, mem, 0x60_0ffc, b"data").unwrap();
    rhizo_cell::memory::copy_to(&pt, mem, 0x4000_001f_fff8, b"big page").unwrap();
    let mut c = Cell::new(9, pt.root(), 0x40_0123, 0x7fff_0000, 0x61_0000);
    c.regs.gpr[30] = 0xfeed;
    c.regs.tls = 0x1234;
    c.regs.fp.0[5] = 0x4009_21fb_5444_2d18; // pi, in V2 / XMM
    c.regs.fp.0[65] = 0x10; // FPSR
    c.mmap_next = 0x4000_0020_0000;
    c
}

fn round_trip<F: EntryFormat>() {
    let (mut mem, mut frames) = (FakePhysicalMemory::new(), CountingFrames::new(1024));
    let original = cell::<F>(&mut mem, &mut frames);
    let snap = Snapshot::capture::<F>(Machine::Aarch64, &original, 77, &mem);
    assert_eq!(
        snap,
        Snapshot::capture::<F>(Machine::Aarch64, &original, 77, &mem),
        "deterministic"
    );
    let snap = Snapshot::from_bytes(snap.into_bytes(), Machine::Aarch64).unwrap();
    assert_eq!(snap.pages(), 3);
    assert_eq!(snap.state().sleep_remaining_ns, 77);
    assert_eq!(snap.page(2).size, PageSize::Large);
    assert_eq!(snap.find(0x60_0fff), Some(1));
    assert_eq!(snap.find(0x60_1000), None);

    let pt = PageTable::<F>::new(&mut mem, &mut || frames.alloc()).unwrap();
    for i in 0..snap.pages() {
        snap.restore_page(i, &pt, &mut mem, &mut frames).unwrap();
    }
    let restored = snap.cell(10, pt.root());
    assert_eq!(restored.regs, original.regs);
    assert_eq!(
        (restored.brk, restored.mmap_next),
        (original.brk, original.mmap_next)
    );
    let mut buf = [0u8; 8];
    rhizo_cell::memory::copy_from(&pt, &mem, 0x4000_001f_fff8, &mut buf).unwrap();
    assert_eq!(&buf, b"big page");
    rhizo_cell::memory::copy_from(&pt, &mem, 0x60_0ffc, &mut buf[..4]).unwrap();
    assert_eq!(&buf[..4], b"data");
    assert_eq!(pt.translate(&mem, 0x40_0000).unwrap().flags, RX);
    // The restored cell captures to the same bytes (same state, same memory).
    let again = Snapshot::capture::<F>(Machine::Aarch64, &Cell { id: 9, ..restored }, 77, &mem);
    assert_eq!(again.as_bytes(), snap.as_bytes());
}

#[test]
fn round_trip_aarch64() {
    round_trip::<Aarch64>();
}

#[test]
fn round_trip_x86_64() {
    round_trip::<X86_64>();
}

fn sample() -> Vec<u8> {
    let (mut mem, mut frames) = (FakePhysicalMemory::new(), CountingFrames::new(1024));
    let c = cell::<Aarch64>(&mut mem, &mut frames);
    Snapshot::capture::<Aarch64>(Machine::Aarch64, &c, 0, &mem).into_bytes()
}

#[test]
fn rejects_what_it_cannot_resume() {
    let good = sample();
    let check =
        |bytes: Vec<u8>, err| assert_eq!(Snapshot::from_bytes(bytes, Machine::Aarch64), Err(err));
    check(good[..30].to_vec(), FormatError::Truncated);
    check(good[..good.len() - 1].to_vec(), FormatError::Truncated);
    let mut b = good.clone();
    b[0] = b'X';
    check(b, FormatError::BadMagic);
    let mut b = good.clone();
    b[8] = (VERSION + 1) as u8; // a later format
    check(b, FormatError::Version(VERSION + 1));
    assert_eq!(
        Snapshot::from_bytes(good.clone(), Machine::X86_64),
        Err(FormatError::Machine {
            expected: 62,
            found: 183
        })
    );
    let rec0 = HEADER + STATE;
    let mut b = good.clone();
    b[rec0 + 17] = 0b011; // writable and executable
    check(b, FormatError::BadPage { index: 0 });
    let mut b = good.clone();
    b[rec0 + RECORD..rec0 + RECORD + 8].copy_from_slice(&0x40_0000u64.to_le_bytes());
    check(b, FormatError::BadPage { index: 1 }); // overlaps page 0
    let mut b = good.clone();
    b[rec0 + 2 * RECORD + 16] = 1;
    b[rec0 + 2 * RECORD..rec0 + 2 * RECORD + 8].copy_from_slice(&0x4000_0000_1000u64.to_le_bytes());
    check(b, FormatError::BadPage { index: 2 }); // misaligned 2 MiB page
    let mut b = good;
    b[rec0..rec0 + 8].copy_from_slice(&0xffff_8000_0000_0000u64.to_le_bytes());
    check(b, FormatError::BadPage { index: 0 }); // kernel address
}

#[test]
fn restore_failure_returns_the_frame() {
    let snap = Snapshot::from_bytes(sample(), Machine::Aarch64).unwrap();
    let (mut mem, mut frames) = (FakePhysicalMemory::new(), CountingFrames::new(1024));
    let pt = PageTable::<Aarch64>::new(&mut mem, &mut || frames.alloc()).unwrap();
    snap.restore_page(0, &pt, &mut mem, &mut frames).unwrap();
    let before = frames.outstanding();
    assert_eq!(
        snap.restore_page(0, &pt, &mut mem, &mut frames),
        Err(MapError::AlreadyMapped(0x40_0000))
    );
    assert_eq!(frames.outstanding(), before);
}
