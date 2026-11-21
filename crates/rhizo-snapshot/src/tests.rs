use super::*;
use rhizo_cell::space;
use rhizo_cell::testing::CountingFrames;
use rhizo_hal::testing::{FakePhysicalMemory, RecordingTlb};
use rhizo_mm::paging::{Aarch64, X86_64};

const RW: PageFlags = PageFlags::USER
    .union(PageFlags::WRITABLE)
    .union(PageFlags::NO_EXECUTE);
const RX: PageFlags = PageFlags::USER;

/// The release of snapshots of cells that share no page.
fn unshared(phys: u64, _: PageSize) {
    unreachable!("{phys:#x} was never shared");
}

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
    c.quota = rhizo_cell::quota::Quota {
        memory: 32 << 20,
        cpu_ns: 7_000_000,
        files: 64,
        sockets: 3,
    };
    c.cpu_ns = 1234;
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
    let snap = Snapshot::capture::<F>(
        Machine::Aarch64,
        &original,
        &[],
        &WxRanges::default(),
        &Fs::new(),
        &[],
        1000,
        &mut mem,
        &mut frames,
        unshared,
    );
    assert_eq!(
        snap,
        Snapshot::capture::<F>(
            Machine::Aarch64,
            &original,
            &[],
            &WxRanges::default(),
            &Fs::new(),
            &[],
            1000,
            &mut mem,
            &mut frames,
            unshared
        ),
        "deterministic"
    );
    let snap = Snapshot::from_bytes(snap.export(&mem, crc32c), Machine::Aarch64, crc32c).unwrap();
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
        (
            restored.brk,
            restored.mmap_next,
            restored.quota,
            restored.cpu_ns
        ),
        (
            original.brk,
            original.mmap_next,
            original.quota,
            original.cpu_ns
        )
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
        &[],
        &WxRanges::default(),
        &Fs::new(),
        &[],
        1000,
        &mut mem,
        &mut frames,
        unshared,
    );
    assert_eq!(again, snap);
}

#[test]
fn round_trip_aarch64() {
    round_trip::<Aarch64>();
}

/// A write-or-execute page, writing and running.
const WX: PageFlags = space::WX_WRITING;
const WX_RUNNING: PageFlags = PageFlags::USER.union(PageFlags::WRITE_OR_EXEC);

/// Write-or-execute pages keep their state and contents, and the cell its
/// write-or-execute ranges, through a snapshot (spec `snapshots`, design D7
/// of `node-jit`).
fn wx_pages_and_ranges_travel<F: EntryFormat>() {
    let (mut mem, mut frames) = (FakePhysicalMemory::new(), CountingFrames::new(1024));
    let mut tlb = RecordingTlb::default();
    let c = cell::<F>(&mut mem, &mut frames);
    let pt = PageTable::<F>::from_root(c.root);
    for at in [0x70_0000, 0x70_1000] {
        space::map_anonymous(&pt, &mut mem, &mut tlb, &mut frames, at, 0x1000, WX).unwrap();
        rhizo_cell::memory::copy_to(&pt, &mut mem, at, &at.to_le_bytes()).unwrap();
    }
    assert_eq!(pt.resolve_exec(&mut mem, &mut frames, 0x70_1000), Ok(true));
    let mut wx = WxRanges::default();
    wx.add(0x70_0000, 0x80_0000).unwrap();
    wx.add(0x90_0000, 0x90_2000).unwrap();
    let snap = Snapshot::capture::<F>(
        Machine::Aarch64,
        &c,
        &[],
        &wx,
        &Fs::new(),
        &[],
        0,
        &mut mem,
        &mut frames,
        unshared,
    );
    let snap = Snapshot::from_bytes(snap.export(&mem, crc32c), Machine::Aarch64, crc32c).unwrap();
    assert_eq!(snap.wx(), wx);
    let pt = PageTable::<F>::new(&mut mem, &mut || frames.alloc()).unwrap();
    for i in 0..snap.pages() {
        snap.restore_page(i, &pt, &mut mem, &mut frames).unwrap();
    }
    for (at, flags) in [(0x70_0000, WX), (0x70_1000, WX_RUNNING)] {
        assert_eq!(pt.translate(&mem, at).unwrap().flags, flags, "{at:#x}");
        let mut word = [0; 8];
        rhizo_cell::memory::copy_from(&pt, &mem, at, &mut word).unwrap();
        assert_eq!(u64::from_le_bytes(word), at);
    }
}

#[test]
fn wx_pages_and_ranges_travel_aarch64() {
    wx_pages_and_ranges_travel::<Aarch64>();
}

#[test]
fn wx_pages_and_ranges_travel_x86_64() {
    wx_pages_and_ranges_travel::<X86_64>();
}

/// A write-or-execute record writable and executable, or neither, and
/// ranges that break their rules are refused (spec `snapshots`).
#[test]
fn rejects_bad_wx_records_and_ranges() {
    let good = sample();
    let check = |bytes: Vec<u8>, err| {
        assert_eq!(
            Snapshot::from_bytes(sealed(bytes), Machine::Aarch64, crc32c),
            Err(err)
        )
    };
    let rec0 = HEADER + STATE + THREAD;
    for (flags, ok) in [
        (0b1011, false),
        (0b1110, false),
        (0b1111, true),
        (0b1010, true),
    ] {
        let mut b = good.clone();
        b[rec0 + 17] = flags; // the code page
        match ok {
            true => assert!(
                Snapshot::from_bytes(sealed(b), Machine::Aarch64, crc32c).is_ok(),
                "{flags:#b}"
            ),
            false => check(b, FormatError::BadPage { index: 0 }),
        }
    }
    // Ranges after the threads, counted in the header.
    let with = |ranges: &[(u64, u64)]| {
        let mut b = good[..rec0].to_vec();
        b[14..16].copy_from_slice(&(ranges.len() as u16).to_le_bytes());
        for &(s, e) in ranges {
            b.extend_from_slice(&s.to_le_bytes());
            b.extend_from_slice(&e.to_le_bytes());
        }
        b.extend_from_slice(&good[rec0..]);
        sealed(b)
    };
    let snap = Snapshot::from_bytes(with(&[(0x1000, 0x3000)]), Machine::Aarch64, crc32c).unwrap();
    assert_eq!(snap.wx().ranges(), &[(0x1000, 0x3000)]);
    assert_eq!(snap.find(0x60_0000), Some(1), "records after the ranges");
    for bad in [
        &[(0x1000, 0x1000)][..],
        &[(0x1001, 0x2000)],
        &[(0x3000, 0x4000), (0x1000, 0x2000)],
        &[(0x1000, 0x2000), (0x2000, 0x3000)],
        &[(0x1000, USER_END + 0x1000)],
    ] {
        check(with(bad), FormatError::BadRanges);
    }
    let many: Vec<_> = (0..=space::MAX_WX_RANGES as u64)
        .map(|i| (0x2000 * (i + 1), 0x2000 * (i + 1) + 0x1000))
        .collect();
    check(with(&many), FormatError::BadRanges);
    let mut cut = with(&[(0x1000, 0x3000)]);
    cut[14] = 200; // the records read as ranges
    assert!(Snapshot::from_bytes(sealed(cut), Machine::Aarch64, crc32c).is_err());
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
    let snap = Snapshot::capture::<Aarch64>(
        Machine::Aarch64,
        &c,
        &[],
        &WxRanges::default(),
        &fs,
        &[],
        0,
        &mut mem,
        &mut frames,
        unshared,
    );
    let snap = Snapshot::from_bytes(snap.export(&mem, crc32c), Machine::Aarch64, crc32c).unwrap();
    let back = snap.files();
    assert_eq!(back.getcwd(), b"/in");
    assert_eq!(back.stat(None, b"data.txt").unwrap().size, 5);
    // A files section that does not decode is refused (its last byte is
    // just before the sockets section: a length and an empty list, then
    // the checksum).
    let mut bytes = snap.export(&mem, crc32c);
    let last = bytes.len() - 13 - CHECKSUM;
    bytes[last] ^= 0xff;
    assert_eq!(
        Snapshot::from_bytes(sealed(bytes), Machine::Aarch64, crc32c),
        Err(FormatError::BadFiles)
    );
}

#[test]
fn socket_records_travel_one_per_descriptor() {
    use rhizo_net::{Family, Options, Record, Saved, Type};
    let (mut mem, mut frames) = (FakePhysicalMemory::new(), CountingFrames::new(1024));
    let c = cell::<Aarch64>(&mut mem, &mut frames);
    let listener = Record {
        family: Family::Inet,
        ty: Type::Stream,
        state: Saved::Listening(4),
        local: Some(rhizo_net::Addr::Inet {
            ip: [127, 0, 0, 1],
            port: 80,
        }),
        opts: Options::default(),
        parked: None,
    };
    let mut fs = Fs::new();
    let fd = fs.socket_fd(0, false, false).unwrap();
    let snap = Snapshot::capture::<Aarch64>(
        Machine::Aarch64,
        &c,
        &[],
        &WxRanges::default(),
        &fs,
        std::slice::from_ref(&listener),
        0,
        &mut mem,
        &mut frames,
        unshared,
    );
    let snap = Snapshot::from_bytes(snap.export(&mem, crc32c), Machine::Aarch64, crc32c).unwrap();
    assert_eq!(snap.sockets(), std::slice::from_ref(&listener));
    assert_eq!(snap.files().socket(fd), Ok(0));
    let mut refused = |fs: &Fs, records: &[Record]| {
        let snap = Snapshot::capture::<Aarch64>(
            Machine::Aarch64,
            &c,
            &[],
            &WxRanges::default(),
            fs,
            records,
            0,
            &mut mem,
            &mut frames,
            unshared,
        );
        Snapshot::from_bytes(snap.export(&mem, crc32c), Machine::Aarch64, crc32c)
    };
    assert_eq!(
        refused(&fs, &[]),
        Err(FormatError::BadSockets),
        "a descriptor without its record"
    );
    assert_eq!(
        refused(&Fs::new(), std::slice::from_ref(&listener)),
        Err(FormatError::BadSockets),
        "a record without its descriptor"
    );
    let mut twice = fs.clone();
    twice.socket_fd(0, false, false).unwrap();
    assert_eq!(
        refused(&twice, &[listener]),
        Err(FormatError::BadSockets),
        "one record, two holders"
    );
}

#[test]
fn parked_connections_travel_as_their_key() {
    use rhizo_net::{Family, Options, Park, Record, Saved, Type};
    let (mut mem, mut frames) = (FakePhysicalMemory::new(), CountingFrames::new(1024));
    let c = cell::<Aarch64>(&mut mem, &mut frames);
    let parked = Record {
        family: Family::Unix,
        ty: Type::Stream,
        state: Saved::Reset,
        local: None,
        opts: Options {
            no_delay: true,
            ..Options::default()
        },
        parked: Some(Park {
            boot: 0x1234_5678_9abc_def0,
            key: 42,
        }),
    };
    let mut fs = Fs::new();
    let fd = fs.socket_fd(0, false, false).unwrap();
    let snap = Snapshot::capture::<Aarch64>(
        Machine::Aarch64,
        &c,
        &[],
        &WxRanges::default(),
        &fs,
        std::slice::from_ref(&parked),
        0,
        &mut mem,
        &mut frames,
        unshared,
    );
    let mut bytes = snap.export(&mem, crc32c);
    let back = Snapshot::from_bytes(bytes.clone(), Machine::Aarch64, crc32c).unwrap();
    assert_eq!(back.sockets(), std::slice::from_ref(&parked));
    assert_eq!(back.files().socket(fd), Ok(0));
    // The last byte of the section belongs to the record: cut, it is refused.
    bytes.remove(bytes.len() - CHECKSUM - 1);
    assert!(Snapshot::from_bytes(sealed(bytes), Machine::Aarch64, crc32c).is_err());
}

/// `bytes` (an exported snapshot, edited) with its checksum made right
/// again, so that what follows the header is what gets checked.
fn sealed(mut bytes: Vec<u8>) -> Vec<u8> {
    if let Some(end) = bytes.len().checked_sub(CHECKSUM) {
        let sum = crc32c(&bytes[..end]);
        bytes[end..].copy_from_slice(&sum.to_le_bytes());
    }
    bytes
}

fn sample() -> Vec<u8> {
    let (mut mem, mut frames) = (FakePhysicalMemory::new(), CountingFrames::new(1024));
    let c = cell::<Aarch64>(&mut mem, &mut frames);
    Snapshot::capture::<Aarch64>(
        Machine::Aarch64,
        &c,
        &[],
        &WxRanges::default(),
        &Fs::new(),
        &[],
        0,
        &mut mem,
        &mut frames,
        unshared,
    )
    .export(&mem, crc32c)
}

#[test]
fn rejects_what_it_cannot_resume() {
    let good = sample();
    let refused =
        |bytes: Vec<u8>| Snapshot::from_bytes(bytes, Machine::Aarch64, crc32c).unwrap_err();
    let check = |bytes: Vec<u8>, err| assert_eq!(refused(sealed(bytes)), err);
    check(good[..30].to_vec(), FormatError::Truncated);
    // Cut or damaged after the header, a file fails its checksum.
    assert_eq!(
        refused(good[..good.len() - 1].to_vec()),
        FormatError::Checksum
    );
    let mut b = good.clone();
    b[HEADER + STATE + 6 * 8] ^= 1;
    assert_eq!(refused(b), FormatError::Checksum);
    check(good[..good.len() - 1].to_vec(), FormatError::Truncated);
    let mut b = good.clone();
    b[0] = b'X';
    check(b, FormatError::BadMagic);
    let mut b = good.clone();
    b[8] = (VERSION + 1) as u8; // a later format
    check(b, FormatError::Version(VERSION + 1));
    assert_eq!(
        Snapshot::from_bytes(good.clone(), Machine::X86_64, crc32c),
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
    let mut b = good.clone();
    b[rec0..rec0 + 8].copy_from_slice(&0xffff_8000_0000_0000u64.to_le_bytes());
    check(b, FormatError::BadPage { index: 0 }); // kernel address
    // Registers that would fault in the kernel when entered (spec
    // `isolation`): a kernel or non-canonical program counter.
    for pc in [USER_END, 0x0000_8000_0000_0000, 0xffff_8000_0000_0000] {
        let mut b = good.clone();
        let at = HEADER + STATE + 6 * 8; // the first thread's program counter
        b[at..at + 8].copy_from_slice(&pc.to_le_bytes());
        check(b, FormatError::BadState);
    }
}

/// On x86_64 the thread pointer goes to the FS base and the FP image to
/// `FXRSTOR`: a non-canonical base or reserved `MXCSR` bits would fault in
/// the kernel, so such a snapshot is refused.
#[test]
fn x86_64_state_that_would_fault_in_the_kernel_is_refused() {
    let (mut mem, mut frames) = (FakePhysicalMemory::new(), CountingFrames::new(1024));
    let c = cell::<X86_64>(&mut mem, &mut frames);
    let good = Snapshot::capture::<X86_64>(
        Machine::X86_64,
        &c,
        &[],
        &WxRanges::default(),
        &Fs::new(),
        &[],
        0,
        &mut mem,
        &mut frames,
        unshared,
    )
    .export(&mem, crc32c);
    assert!(Snapshot::from_bytes(good.clone(), Machine::X86_64, crc32c).is_ok());
    // The first thread's words: tls is word 39, the FP image from word 41.
    let tls = HEADER + STATE + 39 * 8;
    let mxcsr = HEADER + STATE + (41 + 3) * 8;
    for (at, value) in [
        (tls, 0x8000_0000_0000_0000u64),
        (tls, USER_END),
        (mxcsr, 1 << 16),
    ] {
        let mut b = good.clone();
        b[at..at + 8].copy_from_slice(&value.to_le_bytes());
        assert_eq!(
            Snapshot::from_bytes(sealed(b), Machine::X86_64, crc32c),
            Err(FormatError::BadState)
        );
    }
    // MXCSR_MASK (bytes 28-31), as a real FXSAVE writes it, is accepted.
    let mut b = good;
    b[mxcsr..mxcsr + 8].copy_from_slice(&(0xffbf_u64 << 32 | 0x1f80).to_le_bytes());
    assert!(Snapshot::from_bytes(sealed(b), Machine::X86_64, crc32c).is_ok());
}

/// Frames without any 2 MiB run free.
struct Fragmented(CountingFrames);

impl Frames for Fragmented {
    fn alloc(&mut self) -> Option<u64> {
        self.0.alloc()
    }
    fn alloc_large(&mut self) -> Option<u64> {
        None
    }
    fn free(&mut self, phys: u64, size: PageSize) {
        self.0.free(phys, size);
    }
    fn share(&mut self, phys: u64) {
        self.0.share(phys);
    }
    fn is_shared(&self, phys: u64) -> bool {
        self.0.is_shared(phys)
    }
}

#[test]
fn a_large_page_comes_back_in_small_pages_without_a_large_frame() {
    let (mut mem, mut frames) = (FakePhysicalMemory::new(), CountingFrames::new(4096));
    let original = cell::<X86_64>(&mut mem, &mut frames);
    let snap = Snapshot::capture::<X86_64>(
        Machine::Aarch64,
        &original,
        &[],
        &WxRanges::default(),
        &Fs::new(),
        &[],
        0,
        &mut mem,
        &mut frames,
        unshared,
    );
    assert_eq!(snap.page(2).size, PageSize::Large);
    let mut frames = Fragmented(frames);
    let pt = PageTable::<X86_64>::new(&mut mem, &mut || frames.alloc()).unwrap();
    snap.restore_page(2, &pt, &mut mem, &mut frames).unwrap();
    let big = 0x4000_0000_0000;
    for at in [big, big + 0x1000, big + 0x1f_f000] {
        let t = pt.translate(&mem, at).unwrap();
        assert_eq!((t.size, t.flags), (PageSize::Small, RW), "{at:#x}");
    }
    let mut buf = [0u8; 8];
    rhizo_cell::memory::copy_from(&pt, &mem, 0x4000_001f_fff8, &mut buf).unwrap();
    assert_eq!(&buf, b"big page");
}

#[test]
fn restore_failure_returns_the_frame() {
    let snap = Snapshot::from_bytes(sample(), Machine::Aarch64, crc32c).unwrap();
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
    let good = Snapshot::capture::<Aarch64>(
        Machine::Aarch64,
        &c,
        &[],
        &WxRanges::default(),
        &Fs::new(),
        &[],
        0,
        &mut mem,
        &mut frames,
        unshared,
    )
    .export(&mem, crc32c);
    assert!(Snapshot::from_bytes(good.clone(), Machine::Aarch64, crc32c).is_ok());
    let word = |index: usize, i: usize| HEADER + STATE + index * THREAD + i * 8;
    let with = |at: usize, v: u64| {
        let mut b = good.clone();
        b[at..at + 8].copy_from_slice(&v.to_le_bytes());
        Snapshot::from_bytes(sealed(b), Machine::Aarch64, crc32c)
    };
    let bad = |index| Err(FormatError::BadThread { index });
    assert_eq!(with(word(1, 0), 11), bad(1), "repeated identifier");
    assert_eq!(with(word(0, 0), 0), bad(0), "identifier 0");
    assert_eq!(
        with(word(2, 0), 12),
        bad(2),
        "not below the next identifier"
    );
    assert_eq!(with(word(0, 2), 4), bad(0), "unknown wait");
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
        Snapshot::from_bytes(sealed(none), Machine::Aarch64, crc32c),
        bad(0),
        "no thread"
    );
    let mut many = good;
    many[20..24].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(
        Snapshot::from_bytes(sealed(many), Machine::Aarch64, crc32c),
        Err(FormatError::Truncated)
    );
}

/// Hostile snapshots (spec `isolation`, snapshots from outside): bytes
/// that `from_bytes` accepts, however they were made, restore into a cell
/// that reaches nothing beyond its own memory, with files and sockets whose
/// operations work.
mod hostile {
    use super::*;
    use proptest::prelude::*;
    use rhizo_net::{Addr, Family, Options, Record, Saved, Type};

    /// Kernel data, mapped in the kernel half every x86_64 cell shares.
    const KERNEL_DATA: u64 = 0xffff_8000_0000_0000;
    const SECRET: u64 = 0x5ec2_e75e_c2e7_5ec2;

    /// A snapshot holding something of every kind but 2 MiB pages (which
    /// make each case slow): two threads, code and data pages, files with a
    /// pipe, and two sockets.
    fn rich() -> Vec<u8> {
        let (mut mem, mut frames) = (FakePhysicalMemory::new(), CountingFrames::new(1024));
        let mut tlb = RecordingTlb::default();
        let pt = PageTable::<X86_64>::new(&mut mem, &mut || frames.alloc()).unwrap();
        for (at, flags) in [
            (0x40_0000, RX),
            (0x60_0000, RW),
            (0x60_1000, RW),
            (0x70_0000, space::WX_WRITING),
            (0x70_1000, space::WX_WRITING),
        ] {
            space::map_anonymous(&pt, &mut mem, &mut tlb, &mut frames, at, 0x1000, flags).unwrap();
        }
        assert_eq!(pt.resolve_exec(&mut mem, &mut frames, 0x70_1000), Ok(true));
        let mut wx = WxRanges::default();
        wx.add(0x70_0000, 0x80_0000).unwrap();
        rhizo_cell::memory::copy_to(&pt, &mut mem, 0x60_0ffc, b"data").unwrap();
        let mut c = Cell::new(9, pt.root(), 0x40_0123, 0x7fff_0000, 0x61_0000);
        let regs = *c.regs();
        assert_eq!(c.threads.spawn(regs, 0x60_0ff4), Some(10));
        c.threads.block(Wait::Futex {
            addr: 0x60_0ff8,
            bitset: u32::MAX,
            until: Some(2000),
        });
        let mut fs = Fs::new();
        fs.put_file(b"/d/f", &[7; 5000]).unwrap();
        let (_, w) = fs.pipe(false, false).unwrap();
        fs.write_pipe(w, b"queued").unwrap();
        fs.socket_fd(0, false, false).unwrap();
        fs.socket_fd(1, true, false).unwrap();
        let records = [
            Record {
                family: Family::Inet,
                ty: Type::Stream,
                state: Saved::Listening(4),
                local: Some(Addr::Inet {
                    ip: [127, 0, 0, 1],
                    port: 80,
                }),
                opts: Options::default(),
                parked: None,
            },
            Record {
                family: Family::Unix,
                ty: Type::Stream,
                state: Saved::Open,
                local: Some(Addr::Unix {
                    name: 3,
                    path: b"/s".to_vec(),
                }),
                opts: Options::default(),
                parked: None,
            },
        ];
        Snapshot::capture::<X86_64>(
            Machine::X86_64,
            &c,
            &[],
            &wx,
            &fs,
            &records,
            0,
            &mut mem,
            &mut frames,
            unshared,
        )
        .export(&mem, crc32c)
    }

    /// Restores everything `snap` holds as the kernel does on resume, then
    /// checks that kernel memory and the kernel half are unchanged, that
    /// every mapping and register stays in the cell's half, that its files
    /// answer, and that every frame comes back when the cell is gone.
    fn restore_safely(snap: &Snapshot) {
        let (mut mem, mut frames) = (FakePhysicalMemory::new(), CountingFrames::new(1 << 16));
        let mut next = 0x1000_0000_0000u64;
        let mut ktake = || {
            next += 0x1000;
            Some(next)
        };
        let kernel = PageTable::<X86_64>::new(&mut mem, &mut ktake).unwrap();
        let data = ktake().unwrap();
        mem.write_u64(data, SECRET);
        kernel
            .map(&mut mem, &mut ktake, KERNEL_DATA, data, PageSize::Small, RW)
            .unwrap();
        let pt = PageTable::<X86_64>::new(&mut mem, &mut || frames.alloc()).unwrap();
        pt.share_upper_half(&mut mem, &kernel);
        for i in 0..snap.pages() {
            if snap.restore_page(i, &pt, &mut mem, &mut frames).is_err() {
                break;
            }
        }
        let cell = snap.cell(5, pt.root(), 1_000);
        let wx = snap.wx();
        assert!(wx.ranges().len() <= space::MAX_WX_RANGES);
        assert!(
            wx.ranges().iter().all(|&(_, end)| end <= USER_END),
            "{wx:?}"
        );
        for t in cell.threads.iter() {
            assert!(t.regs.pc < USER_END && t.regs.tls < USER_END, "{t:?}");
        }
        pt.for_each_lower_leaf(&mem, &mut |virt, _, size, flags| {
            assert!(virt + size.bytes() <= USER_END, "{virt:#x}");
            assert!(flags.contains(PageFlags::USER), "{virt:#x}");
            assert!(!(flags.is_writable() && flags.is_executable()), "W^X");
        });
        for addr in [0, 0x40_0000, 0x60_0fff, USER_END - 1, u64::MAX] {
            if let Some(i) = snap.find(addr) {
                let p = snap.page(i);
                assert!(p.virt <= addr && addr < p.virt + p.size.bytes());
            }
        }
        let mut files = snap.files();
        let sockets = snap.sockets();
        for h in files.sockets().collect::<Vec<_>>() {
            assert!((h as usize) < sockets.len(), "socket {h}");
        }
        let mut net = rhizo_net::Net::new();
        for r in &sockets {
            let _ = net.import(r);
        }
        // Every descriptor answers (no operation panics).
        let mut buf = [0u8; 64];
        for fd in 0..32 {
            let _ = files.read(fd, &mut buf, None);
            let _ = files.write(fd, b"x", None);
            let _ = files.read_pipe(fd, 64, &mut |_| true);
            let _ = files.fstat(fd);
            let _ = files.read_dir(fd, |_| true);
            let _ = files.poll(fd, &|_| 0);
            let _ = files.close(fd);
        }
        let _ = files.getcwd();
        assert_eq!(mem.read_u64(data), SECRET, "kernel data unchanged");
        assert_eq!(pt.translate(&mem, KERNEL_DATA).unwrap().phys, data);
        space::destroy(pt, &mut mem, &mut frames);
        assert_eq!(frames.outstanding(), 0, "every frame back");
    }

    /// Up to three edits, most in the header, state, threads and page
    /// records (where a byte changes the most), and sometimes a cut; most
    /// with the checksum made right again, as a hostile file has it, so
    /// that the edits reach the checks behind it (spec `snapshots`, files).
    fn tampered() -> impl Strategy<Value = Vec<u8>> {
        let good = rich();
        let head = HEADER + STATE + 2 * THREAD + RANGE + 5 * RECORD;
        let at = prop_oneof![3 => 0..head, 1 => 0..good.len()];
        (
            prop::collection::vec((at, any::<u8>()), 1..4),
            prop::option::weighted(0.1, any::<usize>()),
            prop::bool::weighted(0.9),
        )
            .prop_map(move |(edits, keep, seal)| {
                let mut bytes = good.clone();
                for (at, b) in edits {
                    bytes[at] = b;
                }
                if let Some(k) = keep {
                    bytes.truncate(k % (bytes.len() + 1));
                }
                if seal { sealed(bytes) } else { bytes }
            })
    }

    proptest! {
        #[test]
        fn tampered_snapshots_are_refused_or_contained(bytes in tampered()) {
            if let Ok(snap) = Snapshot::from_bytes(bytes, Machine::X86_64, crc32c) {
                restore_safely(&snap);
            }
        }

        #[test]
        fn random_bytes_are_refused_or_contained(
            mut bytes in prop::collection::vec(any::<u8>(), 0..2048),
            pages in 0u32..4,
            threads in 1u32..3,
        ) {
            // A valid header and checksum, so the rest is what gets tested.
            if bytes.len() >= HEADER {
                bytes[..8].copy_from_slice(MAGIC);
                bytes[8..12].copy_from_slice(&VERSION.to_le_bytes());
                bytes[12..14].copy_from_slice(&(Machine::X86_64 as u16).to_le_bytes());
                bytes[16..20].copy_from_slice(&pages.to_le_bytes());
                bytes[20..24].copy_from_slice(&threads.to_le_bytes());
            }
            if let Ok(snap) = Snapshot::from_bytes(sealed(bytes), Machine::X86_64, crc32c) {
                restore_safely(&snap);
            }
        }
    }
}

thread_local! {
    /// Frames the snapshots of [`shared`] gave back.
    static RELEASED: std::cell::RefCell<Vec<(u64, PageSize)>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn release(phys: u64, size: PageSize) {
    RELEASED.with(|r| r.borrow_mut().push((phys, size)));
}

/// A fork of a template that wrote one page: its snapshot refers to the
/// two pages it still shares, copies the third, keeps them as they were
/// when the template writes them, and exports them in full.
fn shared<F: EntryFormat>() {
    let (mut mem, mut frames) = (FakePhysicalMemory::new(), CountingFrames::new(1024));
    let template = cell::<F>(&mut mem, &mut frames);
    let tpt = PageTable::<F>::from_root(template.root);
    let fork = tpt.fork_lower_half(&mut mem, &mut frames).unwrap();
    /// Writes like a cell, resolving copy-on-write first.
    fn write<F: EntryFormat>(
        mem: &mut FakePhysicalMemory,
        frames: &mut CountingFrames,
        pt: &PageTable<F>,
        at: u64,
        data: &[u8],
    ) {
        let resolved = pt.resolve_write(mem, frames, &mut RecordingTlb::default(), at);
        assert_eq!(resolved, Ok(true));
        rhizo_cell::memory::copy_to(pt, mem, at, data).unwrap();
    }
    write(&mut mem, &mut frames, &fork, 0x60_0ffc, b"kid!");
    let child = Cell {
        root: fork.root(),
        ..template
    };
    let snap = Snapshot::capture::<F>(
        Machine::Aarch64,
        &child,
        &[],
        &WxRanges::default(),
        &Fs::new(),
        &[],
        0,
        &mut mem,
        &mut frames,
        release,
    );
    let kinds: Vec<bool> = (0..3)
        .map(|i| matches!(snap.page(i).data, Data::Frame(_)))
        .collect();
    assert_eq!(kinds, [true, false, true], "code and 2 MiB page shared");
    assert_eq!(snap.shared_bytes(), 0x1000 + (2 << 20));
    assert!(
        snap.heap_bytes() < 3 * 0x1000,
        "{} bytes",
        snap.heap_bytes()
    );
    space::destroy(fork, &mut mem, &mut frames);
    // The template writes a page the snapshot refers to: it gets a copy.
    write(&mut mem, &mut frames, &tpt, 0x4000_001f_fff8, b"template");

    let check = |snap: &Snapshot, mem: &mut FakePhysicalMemory, frames: &mut CountingFrames| {
        let pt = PageTable::<F>::new(mem, &mut || frames.alloc()).unwrap();
        for i in 0..snap.pages() {
            snap.restore_page(i, &pt, mem, frames).unwrap();
        }
        let mut buf = [0u8; 8];
        rhizo_cell::memory::copy_from(&pt, mem, 0x4000_001f_fff8, &mut buf).unwrap();
        assert_eq!(&buf, b"big page");
        rhizo_cell::memory::copy_from(&pt, mem, 0x60_0ffc, &mut buf[..4]).unwrap();
        assert_eq!(&buf[..4], b"kid!");
        assert_eq!(pt.translate(mem, 0x40_0000).unwrap().flags, RX);
        pt
    };
    let resumed = check(&snap, &mut mem, &mut frames);
    let big = resumed.translate(&mem, 0x4000_0000_0000).unwrap();
    assert_eq!(snap.page(2).data, Data::Frame(big.phys), "shared again");
    assert!(!big.flags.is_writable(), "copy-on-write");
    write(
        &mut mem,
        &mut frames,
        &resumed,
        0x4000_0000_0000,
        b"resumed!",
    );
    let back = Snapshot::from_bytes(snap.export(&mem, crc32c), Machine::Aarch64, crc32c).unwrap();
    assert_eq!(back.shared_bytes(), 0, "an export stands alone");
    assert!(
        RELEASED.with(|r| r.borrow().is_empty()),
        "the export keeps them"
    );
    drop(snap);
    let imported = check(&back, &mut mem, &mut frames);
    let released = RELEASED.with(|r| r.take());
    assert_eq!(released.len(), 2, "dropped, it gave both references back");
    for (phys, size) in released {
        frames.free(phys, size);
    }
    for pt in [resumed, imported, tpt] {
        space::destroy(pt, &mut mem, &mut frames);
    }
    assert_eq!(frames.outstanding(), 0, "every frame back");
}

#[test]
fn shared_pages_are_referred_to_aarch64() {
    shared::<Aarch64>();
}

#[test]
fn shared_pages_are_referred_to_x86_64() {
    shared::<X86_64>();
}

#[test]
fn records_that_refer_to_frames_are_refused() {
    // A file whose record claims a physical address (`REFERS`, change
    // `add-shared-snapshots`), its checksum right: no frame is ever taken.
    let mut b = sample();
    b[HEADER + STATE + THREAD + REFERS] = 1;
    assert_eq!(
        Snapshot::from_bytes(sealed(b), Machine::Aarch64, crc32c),
        Err(FormatError::BadPage { index: 0 })
    );
}

/// A thread waiting on descriptors, and one woken that has yet to make its
/// call again, keep the time their call had left (design D5 of
/// `snapshots-on-disk`): resumed later, the call ends that much later, not
/// a whole timeout later.
#[test]
fn waits_on_descriptors_keep_their_time_left() {
    let (mut mem, mut frames) = (FakePhysicalMemory::new(), CountingFrames::new(1024));
    let mut c = cell::<X86_64>(&mut mem, &mut frames);
    let regs = *c.regs();
    let first = c.threads.current().tid;
    let woken = c.threads.spawn(regs, 0).unwrap();
    let forever = c.threads.spawn(regs, 0).unwrap();
    c.threads.get_mut(first).unwrap().wait = Wait::Io { until: Some(1500) };
    c.threads.get_mut(forever).unwrap().wait = Wait::Io { until: None };
    let kept = [(woken, 1800), (first, 1500)];
    let snap = Snapshot::capture::<X86_64>(
        Machine::X86_64,
        &c,
        &kept,
        &WxRanges::default(),
        &Fs::new(),
        &[],
        1000,
        &mut mem,
        &mut frames,
        unshared,
    );
    let snap = Snapshot::from_bytes(snap.export(&mem, crc32c), Machine::X86_64, crc32c).unwrap();
    let threads = snap.threads(5000);
    let wait = |tid| threads.get(tid).unwrap().wait;
    assert_eq!(wait(first), Wait::Io { until: Some(5500) });
    assert_eq!(wait(woken), Wait::Ready);
    assert_eq!(wait(forever), Wait::Io { until: None });
    let mut deadlines = snap.deadlines(5000);
    deadlines.sort();
    assert_eq!(deadlines, [(first, 5500), (woken, 5800)]);
}

#[test]
fn the_checksum_is_crc32c() {
    // The check value of CRC-32C (RFC 3720, appendix B.4).
    assert_eq!(crc32c(b"123456789"), 0xe306_9283);
    assert_eq!(crc32c(b""), 0);
    // Eight bytes at a time, the same as one at a time (RFC 3720, B.4: 32
    // bytes of zeros, of ones, and counting up).
    assert_eq!(crc32c(&[0; 32]), 0x8a91_36aa);
    assert_eq!(crc32c(&[0xff; 32]), 0x62a8_ab43);
    let up: Vec<u8> = (0..32).collect();
    assert_eq!(crc32c(&up), 0x46dd_794e);
}

/// CRC-32C one bit at a time, straight from its definition.
fn crc32c_bitwise(bytes: &[u8]) -> u32 {
    !bytes.iter().fold(!0u32, |c, &b| {
        (0..8).fold(c ^ u32::from(b), |c, _| {
            (c >> 1) ^ (0x82f6_3b78 & (c & 1).wrapping_neg())
        })
    })
}

proptest::proptest! {
    /// The table form equals the definition for any contents, length and
    /// alignment: the reference the kernel's CRC instructions are tested
    /// against (change `fast-mem-primitives`).
    #[test]
    fn the_table_checksum_equals_the_definition(
        bytes in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..600),
        skip in 0usize..8,
    ) {
        let bytes = &bytes[skip.min(bytes.len())..];
        proptest::prop_assert_eq!(crc32c(bytes), crc32c_bitwise(bytes));
    }
}

#[test]
fn the_checksum_function_passed_in_is_the_one_used() {
    let sealed_by_other: Crc32c = |b| !crc32c(b);
    let bytes = Snapshot::from_bytes(sample(), Machine::Aarch64, crc32c)
        .unwrap()
        .export(&FakePhysicalMemory::new(), sealed_by_other);
    assert!(Snapshot::from_bytes(bytes.clone(), Machine::Aarch64, sealed_by_other).is_ok());
    assert_eq!(
        Snapshot::from_bytes(bytes, Machine::Aarch64, crc32c).err(),
        Some(FormatError::Checksum)
    );
}

#[test]
fn quotas_from_outside_are_capped() {
    use rhizo_cell::quota::Quota;
    // `sample` has 32 MiB, 7 ms of CPU, 64 descriptors and 3 sockets.
    let mut snap = Snapshot::from_bytes(sample(), Machine::Aarch64, crc32c).unwrap();
    snap.cap_quota(Quota::DEFAULT);
    assert_eq!(snap.state().quota.sockets, 3, "lower ones stay");
    snap.cap_quota(Quota {
        memory: 1 << 20,
        cpu_ns: 5,
        files: 8,
        sockets: 2,
    });
    let q = snap.state().quota;
    assert_eq!((q.memory, q.cpu_ns, q.files, q.sockets), (1 << 20, 5, 8, 2));
    assert_eq!(snap.state().cpu_ns, 1234, "the time used stays");
}

/// A snapshot in the kernel keeps the cell's copy generation, whether it
/// may hold a secret, and its wipe ranges (design D22 of
/// `add-cell-hierarchy`); its exported bytes do not hold them yet (format
/// version 11 will), so one made from bytes starts with none.
#[test]
fn copy_state_kept_in_the_kernel() {
    let mut mem = FakePhysicalMemory::new();
    let mut frames = CountingFrames::new(4096);
    let mut c = cell::<Aarch64>(&mut mem, &mut frames);
    c.generation = 4;
    c.exposed = true;
    let mut wx = WxRanges::default();
    wx.add(0x90_0000, 0x90_2000).unwrap();
    wx.wipe(0x60_0000, 0x60_1000).unwrap();
    let snap = Snapshot::capture::<Aarch64>(
        Machine::Aarch64,
        &c,
        &[],
        &wx,
        &Fs::new(),
        &[],
        0,
        &mut mem,
        &mut frames,
        unshared,
    );
    assert_eq!(snap.wx(), wx);
    let restored = snap.cell(9, 0, 0);
    assert_eq!((restored.generation, restored.exposed), (4, true));
    // Its wiped page is held as zeros, the rest as they were.
    let pt = PageTable::<Aarch64>::new(&mut mem, &mut || frames.alloc()).unwrap();
    for i in 0..snap.pages() {
        snap.restore_page(i, &pt, &mut mem, &mut frames).unwrap();
    }
    let mut buf = [0xffu8; 4];
    rhizo_cell::memory::copy_from(&pt, &mem, 0x60_0ffc, &mut buf).unwrap();
    assert_eq!(buf, [0; 4], "wiped");
    rhizo_cell::memory::copy_from(&pt, &mem, 0x4000_001f_fff8, &mut buf).unwrap();
    assert_eq!(&buf, b"big ");
    let loaded = Snapshot::from_bytes(snap.export(&mem, crc32c), Machine::Aarch64, crc32c).unwrap();
    assert_eq!(loaded, snap, "the same bytes");
    assert_eq!(loaded.wx().ranges(), wx.ranges());
    assert!(loaded.wx().wiped().is_empty());
    let restored = loaded.cell(9, 0, 0);
    assert_eq!((restored.generation, restored.exposed), (0, false));
}
