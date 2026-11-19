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
