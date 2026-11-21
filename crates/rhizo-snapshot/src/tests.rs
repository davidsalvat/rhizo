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
    let snap = Snapshot::capture::<F>(Machine::Aarch64, &original, &Fs::new(), &[], 1000, &mem);
    assert_eq!(
        snap,
        Snapshot::capture::<F>(Machine::Aarch64, &original, &Fs::new(), &[], 1000, &mem),
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
        &[],
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
    let snap = Snapshot::capture::<Aarch64>(Machine::Aarch64, &c, &fs, &[], 0, &mem);
    let snap = Snapshot::from_bytes(snap.into_bytes(), Machine::Aarch64).unwrap();
    let back = snap.files();
    assert_eq!(back.getcwd(), b"/in");
    assert_eq!(back.stat(None, b"data.txt").unwrap().size, 5);
    // A files section that does not decode is refused (its last byte is
    // just before the sockets section: a length and an empty list).
    let mut bytes = snap.into_bytes();
    let last = bytes.len() - 13;
    bytes[last] ^= 0xff;
    assert_eq!(
        Snapshot::from_bytes(bytes, Machine::Aarch64),
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
        &fs,
        std::slice::from_ref(&listener),
        0,
        &mem,
    );
    let snap = Snapshot::from_bytes(snap.into_bytes(), Machine::Aarch64).unwrap();
    assert_eq!(snap.sockets(), std::slice::from_ref(&listener));
    assert_eq!(snap.files().socket(fd), Ok(0));
    let refused = |fs: &Fs, records: &[Record]| {
        let snap = Snapshot::capture::<Aarch64>(Machine::Aarch64, &c, fs, records, 0, &mem);
        Snapshot::from_bytes(snap.into_bytes(), Machine::Aarch64)
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
        &fs,
        std::slice::from_ref(&parked),
        0,
        &mem,
    );
    let mut bytes = snap.into_bytes();
    let back = Snapshot::from_bytes(bytes.clone(), Machine::Aarch64).unwrap();
    assert_eq!(back.sockets(), std::slice::from_ref(&parked));
    assert_eq!(back.files().socket(fd), Ok(0));
    // The last byte of the section belongs to the record: cut, it is refused.
    bytes.pop();
    assert!(Snapshot::from_bytes(bytes, Machine::Aarch64).is_err());
}

fn sample() -> Vec<u8> {
    let (mut mem, mut frames) = (FakePhysicalMemory::new(), CountingFrames::new(1024));
    let c = cell::<Aarch64>(&mut mem, &mut frames);
    Snapshot::capture::<Aarch64>(Machine::Aarch64, &c, &Fs::new(), &[], 0, &mem).into_bytes()
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
    let good =
        Snapshot::capture::<X86_64>(Machine::X86_64, &c, &Fs::new(), &[], 0, &mem).into_bytes();
    assert!(Snapshot::from_bytes(good.clone(), Machine::X86_64).is_ok());
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
            Snapshot::from_bytes(b, Machine::X86_64),
            Err(FormatError::BadState)
        );
    }
    // MXCSR_MASK (bytes 28-31), as a real FXSAVE writes it, is accepted.
    let mut b = good;
    b[mxcsr..mxcsr + 8].copy_from_slice(&(0xffbf_u64 << 32 | 0x1f80).to_le_bytes());
    assert!(Snapshot::from_bytes(b, Machine::X86_64).is_ok());
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
    let snap = Snapshot::capture::<X86_64>(Machine::Aarch64, &original, &Fs::new(), &[], 0, &mem);
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
    let good =
        Snapshot::capture::<Aarch64>(Machine::Aarch64, &c, &Fs::new(), &[], 0, &mem).into_bytes();
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
        for (at, flags) in [(0x40_0000, RX), (0x60_0000, RW), (0x60_1000, RW)] {
            space::map_anonymous(&pt, &mut mem, &mut tlb, &mut frames, at, 0x1000, flags).unwrap();
        }
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
        Snapshot::capture::<X86_64>(Machine::X86_64, &c, &fs, &records, 0, &mem).into_bytes()
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
    /// records (where a byte changes the most), and sometimes a cut.
    fn tampered() -> impl Strategy<Value = Vec<u8>> {
        let good = rich();
        let head = HEADER + STATE + 2 * THREAD + 3 * RECORD;
        let at = prop_oneof![3 => 0..head, 1 => 0..good.len()];
        (
            prop::collection::vec((at, any::<u8>()), 1..4),
            prop::option::weighted(0.1, any::<usize>()),
        )
            .prop_map(move |(edits, keep)| {
                let mut bytes = good.clone();
                for (at, b) in edits {
                    bytes[at] = b;
                }
                if let Some(k) = keep {
                    bytes.truncate(k % (bytes.len() + 1));
                }
                bytes
            })
    }

    proptest! {
        #[test]
        fn tampered_snapshots_are_refused_or_contained(bytes in tampered()) {
            if let Ok(snap) = Snapshot::from_bytes(bytes, Machine::X86_64) {
                restore_safely(&snap);
            }
        }

        #[test]
        fn random_bytes_are_refused_or_contained(
            mut bytes in prop::collection::vec(any::<u8>(), 0..2048),
            pages in 0u32..4,
            threads in 1u32..3,
        ) {
            // A valid header, so the rest is what gets tested.
            if bytes.len() >= HEADER {
                bytes[..8].copy_from_slice(MAGIC);
                bytes[8..12].copy_from_slice(&VERSION.to_le_bytes());
                bytes[12..14].copy_from_slice(&(Machine::X86_64 as u16).to_le_bytes());
                bytes[16..20].copy_from_slice(&pages.to_le_bytes());
                bytes[20..24].copy_from_slice(&threads.to_le_bytes());
            }
            if let Ok(snap) = Snapshot::from_bytes(bytes, Machine::X86_64) {
                restore_safely(&snap);
            }
        }
    }
}
