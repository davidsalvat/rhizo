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
    let r = c.regs_mut();
    r.gpr[30] = 0xfeed;
    r.tls = 0x1234;
    r.fp.0[5] = 0x4009_21fb_5444_2d18; // pi, in V2 / XMM
    r.fp.0[65] = 0x10; // FPSR
    c.mmap_next = 0x4000_0020_0000;
    c
}

/// `cell` with three threads: the first sleeping until 1077, a second
/// waiting on a futex until 2000, a third waiting on one without timeout;
/// the third is current (it ran last), so the order is third, first, second.
fn threaded<F: EntryFormat>(mem: &mut FakePhysicalMemory, frames: &mut CountingFrames) -> Cell {
    let mut c = cell::<F>(mem, frames);
    c.threads.current_mut().clear_tid = 0x60_0ff0;
    let mut regs = *c.regs();
    regs.tls = 0x5555;
    regs.gpr[0] = 0;
    assert_eq!(c.threads.spawn(regs, 0x60_0ff4), Some(10));
    regs.fp.0[0] = 3;
    assert_eq!(c.threads.spawn(regs, 0), Some(11));
    c.threads.block(Wait::Sleep(1077));
    assert!(c.threads.next(1000));
    c.threads.block(Wait::Futex {
        addr: 0x60_0ff4,
        bitset: 6,
        until: Some(2000),
    });
    assert!(c.threads.next(1000));
    c.threads.block(Wait::Futex {
        addr: 0x60_0ff8,
        bitset: u32::MAX,
        until: None,
    });
    assert!(!c.threads.next(1000));
    c
}

fn round_trip<F: EntryFormat>() {
    let (mut mem, mut frames) = (FakePhysicalMemory::new(), CountingFrames::new(1024));
    let original = threaded::<F>(&mut mem, &mut frames);
    let snap = Snapshot::capture::<F>(Machine::Aarch64, &original, &Fs::new(), 1000, &mem);
    assert_eq!(
        snap,
        Snapshot::capture::<F>(Machine::Aarch64, &original, &Fs::new(), 1000, &mem),
        "deterministic"
    );
    let snap = Snapshot::from_bytes(snap.into_bytes(), Machine::Aarch64).unwrap();
    assert_eq!(snap.pages(), 3);
    assert_eq!(snap.page(2).size, PageSize::Large);
    assert_eq!(snap.find(0x60_0fff), Some(1));
    assert_eq!(snap.find(0x60_1000), None);

    let pt = PageTable::<F>::new(&mut mem, &mut || frames.alloc()).unwrap();
    for i in 0..snap.pages() {
        snap.restore_page(i, &pt, &mut mem, &mut frames).unwrap();
    }
    let restored = snap.cell(10, pt.root(), 1000);
    assert_eq!(
        restored.threads, original.threads,
        "same threads, same order"
    );
    // Deadlines are time left: resumed later, they move with the clock.
    let later = snap.threads(5000);
    let waits: Vec<Wait> = later.iter().map(|t| t.wait).collect();
    assert_eq!(
        waits[..2],
        [
            Wait::Futex {
                addr: 0x60_0ff8,
                bitset: u32::MAX,
                until: None
            },
            Wait::Sleep(5077),
        ]
    );
    assert_eq!(later.next_tid(), 12);
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
    let again = Snapshot::capture::<F>(
        Machine::Aarch64,
        &Cell { id: 9, ..restored },
        &Fs::new(),
        1000,
        &mem,
    );
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

#[test]
fn files_travel_with_the_snapshot() {
    let (mut mem, mut frames) = (FakePhysicalMemory::new(), CountingFrames::new(1024));
    let c = cell::<Aarch64>(&mut mem, &mut frames);
    let mut fs = Fs::new();
    fs.put_file(b"/in/data.txt", b"input").unwrap();
    fs.chdir(b"/in").unwrap();
    let snap = Snapshot::capture::<Aarch64>(Machine::Aarch64, &c, &fs, 0, &mem);
    let snap = Snapshot::from_bytes(snap.into_bytes(), Machine::Aarch64).unwrap();
    let back = snap.files();
    assert_eq!(back.getcwd(), b"/in");
    assert_eq!(back.stat(None, b"data.txt").unwrap().size, 5);
    // A files section that does not decode is refused.
    let mut bytes = snap.into_bytes();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    assert_eq!(
        Snapshot::from_bytes(bytes, Machine::Aarch64),
        Err(FormatError::BadFiles)
    );
}

fn sample() -> Vec<u8> {
    let (mut mem, mut frames) = (FakePhysicalMemory::new(), CountingFrames::new(1024));
    let c = cell::<Aarch64>(&mut mem, &mut frames);
    Snapshot::capture::<Aarch64>(Machine::Aarch64, &c, &Fs::new(), 0, &mem).into_bytes()
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
    let rec0 = HEADER + STATE + THREAD;
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

#[test]
fn rejects_malformed_threads() {
    let (mut mem, mut frames) = (FakePhysicalMemory::new(), CountingFrames::new(1024));
    let c = threaded::<Aarch64>(&mut mem, &mut frames);
    let good = Snapshot::capture::<Aarch64>(Machine::Aarch64, &c, &Fs::new(), 0, &mem).into_bytes();
    assert!(Snapshot::from_bytes(good.clone(), Machine::Aarch64).is_ok());
    let word = |index: usize, i: usize| HEADER + STATE + index * THREAD + i * 8;
    let with = |at: usize, v: u64| {
        let mut b = good.clone();
        b[at..at + 8].copy_from_slice(&v.to_le_bytes());
        Snapshot::from_bytes(b, Machine::Aarch64)
    };
    let bad = |index| Err(FormatError::BadThread { index });
    assert_eq!(with(word(1, 0), 11), bad(1), "repeated identifier");
    assert_eq!(with(word(0, 0), 0), bad(0), "identifier 0");
    assert_eq!(
        with(word(2, 0), 12),
        bad(2),
        "not below the next identifier"
    );
    assert_eq!(with(word(0, 2), 3), bad(0), "unknown wait");
    assert_eq!(with(word(0, 3), 0x60_0ff9), bad(0), "misaligned futex");
    assert_eq!(
        with(word(0, 3), 0xffff_8000_0000_0000),
        bad(0),
        "kernel futex"
    );
    assert_eq!(with(word(0, 4), 0), bad(0), "empty bitset");
    assert_eq!(
        with(word(2, 2), 0),
        bad(2),
        "a runnable thread with a futex"
    );
    assert_eq!(with(word(1, 5), u64::MAX), bad(1), "a sleep without one");
    let mut none = good.clone();
    none[20..24].copy_from_slice(&0u32.to_le_bytes());
    assert_eq!(
        Snapshot::from_bytes(none, Machine::Aarch64),
        bad(0),
        "no thread"
    );
    let mut many = good;
    many[20..24].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(
        Snapshot::from_bytes(many, Machine::Aarch64),
        Err(FormatError::Truncated)
    );
}
