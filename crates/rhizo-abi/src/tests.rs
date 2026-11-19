//! Host tests: each call runs against a real page-table hierarchy in fake
//! physical memory, through the same entry point the kernel uses.

use super::*;
use rhizo_cell::testing::CountingFrames;
use rhizo_hal::testing::{FakePhysicalMemory, RecordingConsole, RecordingTlb};
use rhizo_mm::paging::{Aarch64, PageSize};

#[derive(Default)]
struct FakeServices {
    console: RecordingConsole,
    log: String,
    clock: u64,
}

impl Console for FakeServices {
    fn write_bytes(&mut self, bytes: &[u8]) {
        self.console.write_bytes(bytes);
    }
}

impl Services for FakeServices {
    fn log(&mut self, message: core::fmt::Arguments<'_>) {
        self.log.push_str(&format!("{message}\n"));
    }
    fn monotonic_ns(&mut self) -> u64 {
        self.clock
    }
    fn realtime_ns(&mut self) -> u64 {
        1_700_000_000_000_000_000 + self.clock
    }
    fn random(&mut self, out: &mut [u8]) {
        out.fill(0xa5);
    }
}

/// A cell with one writable page at `DATA`, a read-only page at `RO`, and
/// its heap starting at `HEAP`.
struct World {
    cell: Cell,
    abi: Abi,
    mem: FakePhysicalMemory,
    tlb: RecordingTlb,
    frames: CountingFrames,
    services: FakeServices,
    fs: rhizo_fs::Fs,
}

const DATA: u64 = 0x10_0000;
const RO: u64 = 0x11_0000;
const HEAP: u64 = 0x20_0000;

impl World {
    fn new(arch: Arch) -> Self {
        let mut mem = FakePhysicalMemory::new();
        let mut frames = CountingFrames::new(1024);
        let mut tlb = RecordingTlb::default();
        let pt = PageTable::<Aarch64>::new(&mut mem, &mut || frames.alloc()).unwrap();
        let rw = PageFlags::USER | PageFlags::WRITABLE | PageFlags::NO_EXECUTE;
        space::map_anonymous(&pt, &mut mem, &mut tlb, &mut frames, DATA, FRAME_SIZE, rw).unwrap();
        let ro = PageFlags::USER | PageFlags::NO_EXECUTE;
        space::map_anonymous(&pt, &mut mem, &mut tlb, &mut frames, RO, FRAME_SIZE, ro).unwrap();
        Self {
            cell: Cell::new(5, pt.root(), 0x40_0000, STACK_TOP - 64, HEAP),
            abi: Abi::new(arch),
            mem,
            tlb,
            frames,
            services: FakeServices::default(),
            fs: rhizo_fs::Fs::new(),
        }
    }

    fn pt(&self) -> PageTable<Aarch64> {
        PageTable::from_root(self.cell.root)
    }

    /// Calls `name` (AArch64 numbering unless the world is x86_64).
    fn call(&mut self, nr: u64, args: &[u64]) -> (i64, Outcome) {
        let regs = &mut self.cell.regs.gpr;
        match self.abi.arch {
            Arch::Aarch64 => {
                regs[8] = nr;
                regs[..args.len()].copy_from_slice(args);
            }
            Arch::X86_64 => {
                regs[0] = nr;
                for (&slot, &v) in [5, 4, 3, 9, 7, 8].iter().zip(args) {
                    regs[slot] = v;
                }
            }
        }
        let mut cx = Context {
            pt: self.pt(),
            mem: &mut self.mem,
            tlb: &mut self.tlb,
            frames: &mut self.frames,
            services: &mut self.services,
            fs: &mut self.fs,
            pager: &mut NoPager,
        };
        let out = self.abi.handle(&mut self.cell, &mut cx);
        (self.cell.regs.gpr[0] as i64, out)
    }

    fn poke(&mut self, addr: u64, data: &[u8]) {
        memory::copy_to(&self.pt(), &mut self.mem, addr, data).unwrap();
    }

    fn peek(&self, addr: u64, len: usize) -> Vec<u8> {
        let mut out = vec![0; len];
        memory::copy_from(&self.pt(), &self.mem, addr, &mut out).unwrap();
        out
    }
}

// AArch64 call numbers.
const WRITE: u64 = 64;
const WRITEV: u64 = 66;
const EXIT_GROUP: u64 = 94;
const BRK: u64 = 214;
const MUNMAP: u64 = 215;
const MMAP: u64 = 222;

#[test]
fn write_to_stdout_reaches_the_console_and_returns_the_count() {
    let mut w = World::new(Arch::Aarch64);
    w.poke(DATA, b"hello from a cell\n");
    assert_eq!(w.call(WRITE, &[1, DATA, 18]), (18, Outcome::Resume));
    assert_eq!(w.services.console.text(), "hello from a cell\n");
    assert_eq!(w.call(WRITE, &[7, DATA, 1]).0, -EBADF);
}

#[test]
fn bad_pointers_return_efault_and_partial_writes_return_the_count() {
    let mut w = World::new(Arch::Aarch64);
    assert_eq!(w.call(WRITE, &[1, 0xdead_0000, 4]).0, -EFAULT);
    // Starts 4 bytes before the end of the page; the next page is unmapped.
    w.poke(DATA + FRAME_SIZE - 4, b"tail");
    assert_eq!(w.call(WRITE, &[1, DATA + FRAME_SIZE - 4, 300]).0, -EFAULT);
    w.poke(DATA + FRAME_SIZE - 300, &[b'x'; 300]);
    assert_eq!(w.call(WRITE, &[1, DATA + FRAME_SIZE - 300, 600]).0, 256);
    // Output into a read-only page faults too.
    assert_eq!(w.call(160 /* uname */, &[RO]).0, -EFAULT);
}

#[test]
fn writev_gathers_buffers() {
    let mut w = World::new(Arch::Aarch64);
    w.poke(DATA + 0x100, b"ab");
    w.poke(DATA + 0x200, b"cde");
    let mut iov = Vec::new();
    for (base, len) in [(DATA + 0x100, 2u64), (DATA + 0x200, 3)] {
        iov.extend_from_slice(&base.to_le_bytes());
        iov.extend_from_slice(&len.to_le_bytes());
    }
    w.poke(DATA, &iov);
    assert_eq!(w.call(WRITEV, &[2, DATA, 2]).0, 5);
    assert_eq!(w.services.console.text(), "abcde");
}

#[test]
fn exit_group_reports_the_low_byte() {
    let mut w = World::new(Arch::Aarch64);
    assert_eq!(w.call(EXIT_GROUP, &[0x1_2a]).1, Outcome::Exit(0x2a));
    assert_eq!(w.cell.exit_status, Some(0x2a));
}

#[test]
fn brk_grows_into_zeroed_memory_and_shrinks() {
    let mut w = World::new(Arch::Aarch64);
    assert_eq!(w.call(BRK, &[0]).0, HEAP as i64, "brk(0) returns the break");
    let top = HEAP + 3 * FRAME_SIZE + 10;
    assert_eq!(w.call(BRK, &[top]).0, top as i64);
    assert_eq!(w.peek(top - 16, 16), vec![0; 16]);
    w.poke(HEAP, b"heap");
    assert_eq!(w.call(BRK, &[HEAP + 1]).0, (HEAP + 1) as i64);
    assert!(w.pt().translate(&w.mem, HEAP + FRAME_SIZE).is_none());
    assert_eq!(w.peek(HEAP, 4), b"heap");
    assert_eq!(
        w.call(BRK, &[HEAP - FRAME_SIZE]).0,
        (HEAP + 1) as i64,
        "below the start fails"
    );
}

#[test]
fn mmap_places_large_regions_on_large_pages_and_munmap_frees_them() {
    let mut w = World::new(Arch::Aarch64);
    let anon = MAP_ANONYMOUS | MAP_PRIVATE;
    let small = w.call(MMAP, &[0, 100, 3, anon, u64::MAX, 0]).0 as u64;
    assert_eq!(small, MMAP_BASE);
    let big = w.call(MMAP, &[0, 4 << 20, 3, anon, u64::MAX, 0]).0 as u64;
    assert!(big.is_multiple_of(LARGE_PAGE_SIZE) && big > small);
    assert_eq!(w.pt().translate(&w.mem, big).unwrap().size, PageSize::Large);
    assert_eq!(w.peek(big + (3 << 20), 8), vec![0; 8]);
    let before = w.frames.outstanding();
    assert_eq!(w.call(MUNMAP, &[big, 4 << 20]).0, 0);
    assert_eq!(w.frames.outstanding(), before - 2);
    assert!(w.pt().translate(&w.mem, big).is_none());

    assert_eq!(w.call(MMAP, &[0, 0, 3, anon, u64::MAX, 0]).0, -EINVAL);
    assert_eq!(
        w.call(MMAP, &[0, 4096, 3, MAP_PRIVATE, 3, 0]).0,
        -EINVAL,
        "files"
    );
    assert_eq!(
        w.call(MMAP, &[0, 4096, 6, anon, u64::MAX, 0]).0,
        -EACCES,
        "W^X"
    );
    assert_eq!(
        w.call(MMAP, &[0, u64::MAX - 10, 3, anon, u64::MAX, 0]).0,
        -ENOMEM
    );
    assert_eq!(w.call(MUNMAP, &[small + 1, 10]).0, -EINVAL);
}

#[test]
fn unknown_calls_return_enosys_and_are_reported_once() {
    let mut w = World::new(Arch::Aarch64);
    assert_eq!(w.call(999, &[]).0, -ENOSYS);
    assert_eq!(w.call(999, &[]).0, -ENOSYS);
    assert_eq!(w.call(1 << 40, &[]).0, -ENOSYS);
    assert_eq!(
        w.services.log,
        "cell 5: unimplemented syscall 999\ncell 5: unimplemented syscall 1099511627776\n"
    );
    assert_eq!(
        w.services.console.text(),
        "",
        "kernel messages are not cell output"
    );
}

#[test]
fn x86_64_numbers_and_registers() {
    let mut w = World::new(Arch::X86_64);
    w.poke(DATA, b"x86");
    assert_eq!(w.call(1, &[1, DATA, 3]).0, 3);
    assert_eq!(w.services.console.text(), "x86");
    assert_eq!(w.call(39, &[]).0, 5, "getpid");
    assert_eq!(w.call(158, &[0x1002, 0x7000]).0, 0, "arch_prctl(SET_FS)");
    assert_eq!(w.cell.regs.tls, 0x7000);
    assert_eq!(w.call(158, &[0x1003, DATA]).0, 0, "arch_prctl(GET_FS)");
    assert_eq!(w.peek(DATA, 8), 0x7000u64.to_le_bytes());
    assert_eq!(w.call(16, &[1, 0x5401, 0]).0, -ENOTTY, "ioctl");
    assert_eq!(w.call(231, &[3]).1, Outcome::Exit(3));
    // arch_prctl does not exist on AArch64 (167 is prctl there).
    let mut a = World::new(Arch::Aarch64);
    assert_eq!(a.call(158, &[0x1002, 0]).0, -ENOSYS);
}

#[test]
fn clocks_uname_random_and_signals() {
    let mut w = World::new(Arch::Aarch64);
    w.services.clock = 3_000_000_123;
    assert_eq!(w.call(113, &[1, DATA]).0, 0);
    assert_eq!(
        w.peek(DATA, 16),
        [3u64.to_le_bytes(), 123u64.to_le_bytes()].concat()
    );
    assert_eq!(w.call(113, &[99, DATA]).0, -EINVAL);

    assert_eq!(w.call(160, &[DATA]).0, 0);
    assert_eq!(&w.peek(DATA, 6), b"Linux\0");
    assert_eq!(&w.peek(DATA + 4 * 65, 8), b"aarch64\0");

    assert_eq!(w.call(278, &[DATA, 300, 0]).0, 300);
    assert_eq!(w.peek(DATA + 299, 2), vec![0xa5, 0]);

    w.poke(DATA, &[0xff; 32]);
    assert_eq!(w.call(134, &[2, 0, DATA, 8]).0, 0, "rt_sigaction");
    assert_eq!(w.peek(DATA, 32), vec![0; 32], "no previous handler");
    assert_eq!(w.call(135, &[0, 0, 0, 8]).0, 0, "rt_sigprocmask");
    assert_eq!(w.call(96, &[DATA]).0, 5, "set_tid_address returns the tid");
}

#[test]
fn yield_and_sleep_become_scheduling_outcomes() {
    let mut w = World::new(Arch::Aarch64);
    w.services.clock = 1_000;
    assert_eq!(w.call(124, &[]), (0, Outcome::Yield), "sched_yield");

    let ts = |sec: i64, nsec: i64| [sec.to_le_bytes(), nsec.to_le_bytes()].concat();
    w.poke(DATA, &ts(0, 20_000_000));
    assert_eq!(
        w.call(101, &[DATA, 0]),
        (0, Outcome::Sleep(20_001_000)),
        "nanosleep 20 ms"
    );
    w.poke(DATA, &ts(2, 0));
    assert_eq!(
        w.call(115, &[1, 1, DATA, 0]),
        (0, Outcome::Sleep(2_000_000_000)),
        "clock_nanosleep(MONOTONIC, TIMER_ABSTIME)"
    );
    // Realtime is monotonic + 1.7e18 in the fake: an absolute realtime
    // deadline 5 ns after "now" sleeps 5 ns.
    let deadline = 1_700_000_000_000_000_000i64 + 1_000 + 5;
    w.poke(
        DATA,
        &ts(deadline / 1_000_000_000, deadline % 1_000_000_000),
    );
    assert_eq!(w.call(115, &[0, 1, DATA, 0]).1, Outcome::Sleep(1_005));

    w.poke(DATA, &ts(0, 1_000_000_000));
    assert_eq!(
        w.call(101, &[DATA, 0]).0,
        -EINVAL,
        "nanoseconds out of range"
    );
    w.poke(DATA, &ts(-1, 0));
    assert_eq!(w.call(101, &[DATA, 0]).0, -EINVAL, "negative seconds");
    assert_eq!(w.call(101, &[0xdead_0000, 0]).0, -EFAULT);
    assert_eq!(w.call(115, &[2, 0, DATA, 0]).0, -EINVAL, "CPU-time clocks");
    // x86_64 numbers.
    let mut x = World::new(Arch::X86_64);
    assert_eq!(x.call(24, &[]).1, Outcome::Yield);
}

#[test]
fn libc_startup_calls() {
    let mut w = World::new(Arch::Aarch64);
    // mprotect: read-only, then W+X refused, unmapped range refused.
    assert_eq!(w.call(226, &[DATA, 4096, 1]).0, 0);
    assert!(!w.pt().translate(&w.mem, DATA).unwrap().flags.is_writable());
    assert_eq!(w.call(226, &[DATA, 4096, 7]).0, -EACCES);
    assert_eq!(w.call(226, &[0x7000_0000, 4096, 1]).0, -ENOMEM);
    assert_eq!(w.call(226, &[DATA + 1, 4096, 1]).0, -EINVAL);
    assert_eq!(w.call(226, &[DATA, 4096, 3]).0, 0, "writable again");
    // prlimit64(0, RLIMIT_STACK, NULL, &old)
    assert_eq!(w.call(261, &[0, 3, 0, DATA]).0, 0);
    assert_eq!(w.peek(DATA, 8), (8u64 << 20).to_le_bytes());
    assert_eq!(w.call(99, &[DATA, 24]).0, 0, "set_robust_list");
    assert_eq!(w.call(293, &[]).0, -ENOSYS, "rseq declined");
    assert_eq!(w.services.log, "", "declined calls are not reported");
    assert_eq!(w.call(78, &[0, DATA, DATA, 64]).0, -ENOENT, "readlinkat");
    assert_eq!(w.call(174, &[]).0, 0, "getuid");
    // fstat(1) on AArch64: mode at 16 is a character device.
    assert_eq!(w.call(80, &[1, DATA]).0, 0);
    assert_eq!(w.peek(DATA + 16, 4), (0o020_620u32).to_le_bytes());
    assert_eq!(w.call(80, &[5, DATA]).0, -EBADF);
    // newfstatat(1, "", &st, AT_EMPTY_PATH)
    w.poke(DATA + 0x800, &[0]);
    assert_eq!(w.call(79, &[1, DATA + 0x800, DATA, 0x1000]).0, 0);
    let mut x = World::new(Arch::X86_64);
    assert_eq!(x.call(5, &[2, DATA]).0, 0, "fstat on x86_64");
    assert_eq!(x.peek(DATA + 24, 4), (0o020_620u32).to_le_bytes());
}

// File calls (AArch64 numbers).
const OPENAT: u64 = 56;
const CLOSE: u64 = 57;
const READ: u64 = 63;
const LSEEK: u64 = 62;
const AT_FDCWD: u64 = -100i64 as u64;
const ENOTEMPTY: i64 = 39;
const EEXIST: i64 = 17;

/// Puts a NUL-terminated path at `DATA + 0x800` and returns its address.
fn name(w: &mut World, path: &str) -> u64 {
    let mut bytes = path.as_bytes().to_vec();
    bytes.push(0);
    w.poke(DATA + 0x800, &bytes);
    DATA + 0x800
}

#[test]
fn open_write_read_and_close_a_file() {
    let mut w = World::new(Arch::Aarch64);
    let p = name(&mut w, "/notes.txt");
    assert_eq!(w.call(OPENAT, &[AT_FDCWD, p, 0]).0, -ENOENT);
    let fd = w
        .call(OPENAT, &[AT_FDCWD, p, 0x42 /* O_CREAT|O_RDWR */, 0o644])
        .0;
    assert_eq!(fd, 3);
    assert_eq!(
        w.call(OPENAT, &[AT_FDCWD, p, 0xc2 /* |O_EXCL */]).0,
        -EEXIST
    );
    w.poke(DATA, b"some file text");
    assert_eq!(w.call(WRITE, &[3, DATA, 14]).0, 14);
    assert_eq!(w.call(LSEEK, &[3, 5, 0]).0, 5);
    assert_eq!(w.call(READ, &[3, DATA + 0x100, 100]).0, 9);
    assert_eq!(w.peek(DATA + 0x100, 9), b"file text");
    assert_eq!(w.call(READ, &[3, DATA + 0x100, 100]).0, 0, "end of file");
    assert_eq!(w.call(67 /* pread64 */, &[3, DATA + 0x100, 4, 0]).0, 4);
    assert_eq!(w.peek(DATA + 0x100, 4), b"some");
    assert_eq!(
        w.call(READ, &[3, RO, 4]).0,
        0,
        "at the end nothing is copied"
    );
    assert_eq!(w.call(LSEEK, &[3, 0, 0]).0, 0);
    assert_eq!(
        w.call(READ, &[3, RO, 4]).0,
        -EFAULT,
        "into a read-only page"
    );
    assert_eq!(w.call(LSEEK, &[1, 0, 0]).0, -29, "ESPIPE on the console");
    assert_eq!(w.call(READ, &[0, DATA, 4]).0, 0, "empty standard input");
    assert_eq!(w.call(CLOSE, &[3]).0, 0);
    assert_eq!(w.call(CLOSE, &[3]).0, -EBADF);
    assert_eq!(w.call(READ, &[3, DATA, 4]).0, -EBADF);
}

#[test]
fn large_reads_and_writes_cross_pages() {
    let mut w = World::new(Arch::Aarch64);
    let anon = MAP_ANONYMOUS | MAP_PRIVATE;
    let buf = w.call(MMAP, &[0, 3 * FRAME_SIZE, 3, anon, u64::MAX, 0]).0 as u64;
    let data: Vec<u8> = (0..3 * FRAME_SIZE as usize - 100)
        .map(|i| i as u8)
        .collect();
    w.poke(buf + 50, &data);
    let p = name(&mut w, "big");
    let fd = w.call(OPENAT, &[AT_FDCWD, p, 0x42]).0 as u64;
    assert_eq!(
        w.call(WRITE, &[fd, buf + 50, data.len() as u64]).0,
        data.len() as i64
    );
    w.poke(buf, &vec![0; 3 * FRAME_SIZE as usize]);
    assert_eq!(
        w.call(67, &[fd, buf + 7, data.len() as u64 + 99, 0]).0,
        data.len() as i64
    );
    assert_eq!(w.peek(buf + 7, data.len()), data);
}

#[test]
fn directories_names_and_metadata() {
    let mut w = World::new(Arch::Aarch64);
    let d = name(&mut w, "/work");
    assert_eq!(w.call(34 /* mkdirat */, &[AT_FDCWD, d, 0o755]).0, 0);
    assert_eq!(w.call(49 /* chdir */, &[d]).0, 0);
    assert_eq!(w.call(17 /* getcwd */, &[DATA, 64]).0, 6);
    assert_eq!(w.peek(DATA, 6), b"/work\0");
    assert_eq!(w.call(17, &[DATA, 3]).0, -34, "ERANGE");
    let f = name(&mut w, "a");
    let fd = w.call(OPENAT, &[AT_FDCWD, f, 0x42]).0 as u64;
    w.poke(DATA, b"12345");
    w.call(WRITE, &[fd, DATA, 5]);
    // fstat and newfstatat: regular file of 5 bytes (AArch64 layout).
    assert_eq!(w.call(80, &[fd, DATA]).0, 0);
    assert_eq!(w.peek(DATA + 16, 4), (0o100_644u32).to_le_bytes());
    assert_eq!(w.peek(DATA + 48, 8), 5u64.to_le_bytes());
    let full = name(&mut w, "/work/a");
    assert_eq!(w.call(79, &[AT_FDCWD, full, DATA, 0]).0, 0);
    assert_eq!(w.peek(DATA + 48, 8), 5u64.to_le_bytes());
    // faccessat: exists, but cannot be executed.
    assert_eq!(w.call(48, &[AT_FDCWD, full, 4]).0, 0);
    assert_eq!(w.call(48, &[AT_FDCWD, full, 1]).0, -EACCES);
    // getdents64 lists ".", ".." and "a".
    let dot = name(&mut w, ".");
    let dir = w.call(OPENAT, &[AT_FDCWD, dot, 0x4000 /* O_DIRECTORY */]).0 as u64;
    let n = w.call(61, &[dir, DATA, 512]).0;
    assert_eq!(n, 72, "three 24-byte records");
    let rec = w.peek(DATA, n as usize);
    assert_eq!(&rec[48 + 19..48 + 21], b"a\0");
    assert_eq!(rec[48 + 18], 8, "DT_REG");
    assert_eq!(w.call(61, &[dir, DATA, 512]).0, 0, "end of directory");
    assert_eq!(w.call(61, &[fd, DATA, 512]).0, -20, "ENOTDIR");
    // rename, then rmdir refuses a non-empty directory.
    let b = name(&mut w, "/work/b");
    let a = DATA + 0x900;
    w.poke(a, b"/work/a\0");
    assert_eq!(w.call(38 /* renameat */, &[AT_FDCWD, a, AT_FDCWD, b]).0, 0);
    assert_eq!(w.call(79, &[AT_FDCWD, a, DATA, 0]).0, -ENOENT);
    let root = name(&mut w, "/work");
    assert_eq!(
        w.call(35 /* unlinkat */, &[AT_FDCWD, root, 0x200]).0,
        -ENOTEMPTY
    );
    let b = name(&mut w, "b");
    assert_eq!(w.call(35, &[AT_FDCWD, b, 0]).0, 0);
    let up = name(&mut w, "/");
    assert_eq!(w.call(49, &[up]).0, 0);
    let root = name(&mut w, "work");
    assert_eq!(w.call(35, &[AT_FDCWD, root, 0x200]).0, 0);
    assert_eq!(
        w.call(276 /* renameat2 */, &[AT_FDCWD, up, AT_FDCWD, up, 1])
            .0,
        -EINVAL
    );
}

#[test]
fn descriptors_dup_and_fcntl() {
    let mut w = World::new(Arch::Aarch64);
    let p = name(&mut w, "/f");
    let fd = w.call(OPENAT, &[AT_FDCWD, p, 0x42]).0 as u64;
    w.poke(DATA, b"abcdef");
    w.call(WRITE, &[fd, DATA, 6]);
    w.call(LSEEK, &[fd, 0, 0]);
    let copy = w.call(23 /* dup */, &[fd]).0 as u64;
    assert_eq!(copy, 4);
    assert_eq!(w.call(READ, &[fd, DATA, 2]).0, 2);
    assert_eq!(w.call(READ, &[copy, DATA, 2]).0, 2);
    assert_eq!(w.peek(DATA, 2), b"cd", "the offset is shared");
    assert_eq!(w.call(24 /* dup3 */, &[fd, fd, 0]).0, -EINVAL);
    assert_eq!(w.call(24, &[fd, 1, 0x8_0000]).0, 1, "dup3 over stdout");
    assert_eq!(w.call(25 /* fcntl */, &[1, 1 /* F_GETFD */]).0, 1);
    assert_eq!(w.call(WRITE, &[1, DATA, 2]).0, 2);
    assert_eq!(w.services.console.text(), "", "stdout now goes to the file");
    assert_eq!(w.call(25, &[fd, 3 /* F_GETFL */]).0, 2, "O_RDWR");
    assert_eq!(w.call(25, &[fd, 4 /* F_SETFL */, 0x400]).0, 0);
    assert_eq!(w.call(25, &[fd, 3]).0, 0x402);
    assert_eq!(w.call(25, &[fd, 0 /* F_DUPFD */, 10]).0, 10);
    assert_eq!(w.call(25, &[fd, 99]).0, -EINVAL);
    assert_eq!(w.call(25, &[77, 3]).0, -EBADF);
    assert_eq!(w.call(46 /* ftruncate */, &[fd, 2]).0, 0);
    assert_eq!(w.call(80, &[fd, DATA]).0, 0);
    assert_eq!(w.peek(DATA + 48, 8), 2u64.to_le_bytes());
    assert_eq!(w.call(46, &[0, 2]).0, -EINVAL, "not a file");
    assert_eq!(w.call(82 /* fsync */, &[fd]).0, 0);
    assert_eq!(w.call(82, &[77]).0, -EBADF);
}

#[test]
fn x86_64_path_calls() {
    let mut w = World::new(Arch::X86_64);
    let p = name(&mut w, "/x");
    let fd = w
        .call(2 /* open */, &[p, 0x41 /* O_CREAT|O_WRONLY */, 0o644])
        .0;
    assert_eq!(fd, 3);
    w.poke(DATA, b"xyz");
    assert_eq!(w.call(1, &[3, DATA, 3]).0, 3);
    assert_eq!(w.call(0 /* read */, &[3, DATA, 3]).0, -EBADF, "write-only");
    assert_eq!(w.call(4 /* stat */, &[p, DATA]).0, 0);
    assert_eq!(w.peek(DATA + 24, 4), (0o100_644u32).to_le_bytes());
    assert_eq!(w.peek(DATA + 48, 8), 3u64.to_le_bytes());
    assert_eq!(w.call(33 /* dup2 */, &[3, 3]).0, 3);
    assert_eq!(w.call(33, &[9, 3]).0, -EBADF);
    let d = name(&mut w, "/d");
    assert_eq!(w.call(83 /* mkdir */, &[d, 0o755]).0, 0);
    assert_eq!(w.call(83, &[d, 0o755]).0, -EEXIST);
    let dir = w.call(2, &[d, 0x1_0000 /* O_DIRECTORY */]).0;
    assert_eq!(dir, 4);
    assert_eq!(
        w.call(1, &[4, DATA, 3]).0,
        -EBADF,
        "a directory is read-only"
    );
    assert_eq!(w.call(84 /* rmdir */, &[d]).0, 0);
    let (from, to) = (DATA + 0xa00, DATA + 0x900);
    w.poke(from, b"/x\0");
    w.poke(to, b"/y\0");
    assert_eq!(w.call(82 /* rename */, &[from, to]).0, 0);
    assert_eq!(w.call(21 /* access */, &[to, 0]).0, 0);
    assert_eq!(w.call(87 /* unlink */, &[to]).0, 0);
    assert_eq!(w.call(21, &[to, 0]).0, -ENOENT);
    // A path that never ends faults.
    assert_eq!(w.call(2, &[0xdead_0000, 0]).0, -EFAULT);
}
