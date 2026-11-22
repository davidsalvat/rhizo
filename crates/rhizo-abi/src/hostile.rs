//! Hostile host tests (spec `isolation`): the dispatcher driven the way an
//! attacker in a cell would, with any call number and any arguments, against
//! an address space that shares a kernel half (as x86_64 cells do) holding
//! kernel tables, data and code. Whatever the cell asks, the kernel must not
//! panic, must answer with a value or a Linux `errno`, and must leave every
//! byte of its own memory and tables as it was.

use super::*;
use proptest::prelude::*;
use rhizo_cell::testing::CountingFrames;
use rhizo_hal::testing::{FakePhysicalMemory, RecordingTlb};
use rhizo_mm::paging::{Aarch64, PageSize, X86_64};

/// Services that take whatever a cell gives and count it.
#[derive(Default)]
struct Quiet {
    printed: usize,
    logged: usize,
    clock: u64,
    /// Bytes cells may still take.
    room: u64,
}

impl Console for Quiet {
    fn write_bytes(&mut self, bytes: &[u8]) {
        self.printed += bytes.len();
    }
}

impl Services for Quiet {
    fn log(&mut self, _: fmt::Arguments<'_>) {
        self.logged += 1;
    }
    fn monotonic_ns(&mut self) -> u64 {
        self.clock += 1_000;
        self.clock
    }
    fn realtime_ns(&mut self) -> u64 {
        self.monotonic_ns()
    }
    fn random(&mut self, out: &mut [u8]) {
        out.fill(0x5a);
    }
    fn can_store(&mut self, bytes: u64) -> bool {
        let ok = bytes <= self.room;
        if ok {
            self.room -= bytes;
        }
        ok
    }
}

/// Kernel addresses: the start of the direct map (a 4 KiB page of kernel
/// data and a 2 MiB one) and the kernel image.
const KERNEL_DATA: u64 = 0xffff_8000_0000_0000;
const KERNEL_LARGE: u64 = 0xffff_8000_0020_0000;
const KERNEL_TEXT: u64 = 0xffff_ffff_8000_0000;
/// One writable and one read-only page of the cell, and its heap start.
const DATA: u64 = 0x10_0000;
const RO: u64 = 0x11_0000;
const HEAP: u64 = 0x20_0000;
/// What kernel data holds.
const SECRET: u64 = 0x5ec2_e75e_c2e7_5ec2;
/// The cell's file, open as descriptor 3.
const FILE_FD: u64 = 3;

/// Frames of the kernel, from a range of their own.
struct KernelFrames {
    next: u64,
    given: Vec<u64>,
}

impl KernelFrames {
    fn take(&mut self) -> Option<u64> {
        let frame = self.next;
        self.next += FRAME_SIZE;
        self.given.push(frame);
        Some(frame)
    }
}

/// A cell and the kernel around it.
struct Machine<F> {
    cell: Cell,
    abi: Abi,
    mem: FakePhysicalMemory,
    tlb: RecordingTlb,
    frames: CountingFrames,
    services: Quiet,
    fs: rhizo_fs::Fs,
    net: rhizo_net::Net,
    pages: PageCache,
    /// Keeps write-or-execute ranges; never backs a page (the frames
    /// here are counted).
    pager: Ranges,
    /// Every kernel frame: its tables, data, code and both ends of its
    /// 2 MiB page.
    kernel: Vec<u64>,
    format: core::marker::PhantomData<F>,
}

impl<F: EntryFormat> Machine<F> {
    /// A cell of `arch`; its root shares the kernel half when `shared` (as
    /// on x86_64).
    fn new(arch: Arch, shared: bool) -> Self {
        let mut mem = FakePhysicalMemory::new();
        let mut k = KernelFrames {
            next: 0x1000_0000_0000,
            given: Vec::new(),
        };
        let kernel = PageTable::<F>::new(&mut mem, &mut || k.take()).unwrap();
        let rw = PageFlags::WRITABLE | PageFlags::NO_EXECUTE;
        let data = k.take().unwrap();
        let text = k.take().unwrap();
        let large = 0x1000_4000_0000;
        let mut take = || k.take();
        kernel
            .map(&mut mem, &mut take, KERNEL_DATA, data, PageSize::Small, rw)
            .unwrap();
        kernel
            .map(
                &mut mem,
                &mut take,
                KERNEL_LARGE,
                large,
                PageSize::Large,
                rw,
            )
            .unwrap();
        kernel
            .map(
                &mut mem,
                &mut take,
                KERNEL_TEXT,
                text,
                PageSize::Small,
                PageFlags::empty(),
            )
            .unwrap();
        let mut kernel_frames = k.given;
        kernel_frames.extend([large, large + LARGE_PAGE_SIZE - FRAME_SIZE]);
        for &frame in &kernel_frames {
            for i in 0..FRAME_SIZE / 8 {
                if mem.read_u64(frame + i * 8) == 0 {
                    mem.write_u64(frame + i * 8, SECRET);
                }
            }
        }

        let mut frames = CountingFrames::new(1 << 20);
        frames.limit = 192;
        let mut tlb = RecordingTlb::default();
        let pt = PageTable::<F>::new(&mut mem, &mut || frames.alloc()).unwrap();
        if shared {
            pt.share_upper_half(&mut mem, &kernel);
        }
        let user = PageFlags::USER | PageFlags::NO_EXECUTE;
        let user_rw = user | PageFlags::WRITABLE;
        space::map_anonymous(
            &pt,
            &mut mem,
            &mut tlb,
            &mut frames,
            DATA,
            FRAME_SIZE,
            user_rw,
        )
        .unwrap();
        space::map_anonymous(&pt, &mut mem, &mut tlb, &mut frames, RO, FRAME_SIZE, user).unwrap();
        let mut fs = rhizo_fs::Fs::new();
        fs.put_file(b"/f", &[7; 5000]).unwrap();
        let how = rhizo_fs::OpenFlags {
            read: true,
            write: true,
            create: false,
            exclusive: false,
            truncate: false,
            append: false,
            directory: false,
        };
        assert_eq!(fs.open(None, b"/f", how, 2).unwrap() as u64, FILE_FD);
        Self {
            cell: Cell {
                domain: 9, // a root's domain is its identifier
                ..Cell::new(9, pt.root(), 0x40_0000, STACK_TOP - 64, HEAP)
            },
            abi: Abi::new(arch),
            mem,
            tlb,
            frames,
            services: Quiet {
                room: 1 << 20,
                ..Quiet::default()
            },
            fs,
            net: rhizo_net::Net::new(),
            pages: PageCache::new(16),
            pager: Ranges::default(),
            kernel: kernel_frames,
            format: core::marker::PhantomData,
        }
    }

    /// Every word of every kernel frame.
    fn kernel_memory(&self) -> Vec<u64> {
        self.kernel
            .iter()
            .flat_map(|&f| (0..FRAME_SIZE / 8).map(move |i| f + i * 8))
            .map(|at| self.mem.read_u64(at))
            .collect()
    }

    /// Runs call `nr` with `args`; returns how it ended and the result
    /// register.
    fn call(&mut self, nr: u64, args: [u64; 6]) -> (Outcome, i64) {
        let regs = &mut self.cell.regs_mut().gpr;
        match self.abi.arch {
            Arch::Aarch64 => {
                regs[8] = nr;
                regs[..6].copy_from_slice(&args);
            }
            Arch::X86_64 => {
                regs[0] = nr;
                for (&slot, v) in [5, 4, 3, 9, 7, 8].iter().zip(args) {
                    regs[slot] = v;
                }
            }
        }
        let mut cx = Context {
            pt: PageTable::<F>::from_root(self.cell.root),
            mem: &mut self.mem,
            tlb: &mut self.tlb,
            frames: &mut self.frames,
            services: &mut self.services,
            fs: &mut self.fs,
            net: &mut self.net,
            deadline: None,
            pager: &mut self.pager,
            pages: &mut self.pages,
        };
        let outcome = self.abi.handle(&mut self.cell, &mut cx);
        // A call waiting on descriptors runs again as the next one.
        if let rhizo_cell::threads::Wait::Io { .. } = self.cell.threads.current().wait {
            self.cell.threads.current_mut().wait = rhizo_cell::threads::Wait::Ready;
        }
        (outcome, self.cell.regs().gpr[0] as i64)
    }

    fn pt_translate(&self, virt: u64) -> Option<rhizo_mm::paging::Translation> {
        PageTable::<F>::from_root(self.cell.root).translate(&self.mem, virt)
    }

    /// Ends the cell: every frame it was given comes back.
    fn end(mut self) {
        let pt = PageTable::<F>::from_root(self.cell.root);
        space::destroy(pt, &mut self.mem, &mut self.frames);
        self.pages.trim(&mut self.frames);
        assert_eq!(
            self.frames.outstanding(),
            0,
            "every frame of the cell is back"
        );
    }
}

/// Any argument a hostile cell might pass: random words, edge values, kernel
/// and non-canonical addresses, its own pages, negative descriptors and huge
/// lengths.
fn arg() -> impl Strategy<Value = u64> {
    prop_oneof![
        any::<u64>(),
        0u64..=4096,
        0u64..8,
        (-4096i64..0).prop_map(|v| v as u64),
        0xffff_8000_0000_0000u64..=u64::MAX,
        0x0000_8000_0000_0000u64..0xffff_8000_0000_0000,
        0u64..0x0000_8000_0000_0000,
        DATA..DATA + FRAME_SIZE,
        (1u32..64).prop_map(|s| 1u64 << s),
        prop::sample::select(vec![
            0,
            u64::MAX,
            KERNEL_DATA,
            KERNEL_LARGE,
            KERNEL_TEXT,
            KERNEL_DATA + FRAME_SIZE - 8,
            0x0000_7fff_ffff_f000,
            0x0000_8000_0000_0000,
            0x8000_0000_0000_0000,
            MMAP_BASE,
            DATA,
            DATA + FRAME_SIZE - 1,
            RO,
            FILE_FD,
            AT_FDCWD,
            LARGE_PAGE_SIZE,
        ]),
    ]
}

/// Any call number: mostly implemented ones, some never assigned.
fn number() -> impl Strategy<Value = u64> {
    prop_oneof![8 => 0u64..460, 1 => any::<u64>(), 1 => 1024u64..100_000]
}

fn calls() -> impl Strategy<Value = Vec<(u64, [u64; 6])>> {
    prop::collection::vec(
        (number(), [arg(), arg(), arg(), arg(), arg(), arg()]),
        1..40,
    )
}

/// Runs `calls` in a fresh machine and checks the kernel's promises: every
/// result is a value or an errno, kernel memory is as before (a change would
/// stay, so it is compared once at the end), the kernel's messages are
/// bounded, and every frame comes back.
fn survive<F: EntryFormat>(arch: Arch, shared: bool, calls: &[(u64, [u64; 6])]) {
    survive_in(Machine::<F>::new(arch, shared), calls);
}

/// [`survive`] in machine `m`.
fn survive_in<F: EntryFormat>(mut m: Machine<F>, calls: &[(u64, [u64; 6])]) {
    let kernel = m.kernel_memory();
    let mut numbers = Vec::new();
    for &(nr, args) in calls {
        let (outcome, result) = m.call(nr, args);
        numbers.push(nr);
        if outcome == Outcome::Resume {
            assert!(
                result >= -4095,
                "call {nr} {args:x?}: {result} is neither a value nor an errno"
            );
        }
        if let Outcome::Exit(_) = outcome {
            break;
        }
    }
    assert!(m.kernel_memory() == kernel, "kernel memory changed");
    // Never writable and executable (spec `virtual-memory`, `node-jit`).
    let pt = PageTable::<F>::from_root(m.cell.root);
    pt.for_each_lower_leaf(&m.mem, &mut |virt, _, _, f| {
        assert!(!(f.is_writable() && f.is_executable()), "W+X at {virt:#x}");
    });
    numbers.sort_unstable();
    numbers.dedup();
    assert!(
        m.services.logged <= numbers.len(),
        "{} kernel messages for {} distinct calls",
        m.services.logged,
        numbers.len()
    );
    m.end();
}

proptest! {
    #[test]
    fn x86_64_cells_cannot_hurt_the_kernel(calls in calls()) {
        survive::<X86_64>(Arch::X86_64, true, &calls);
    }

    #[test]
    fn aarch64_cells_cannot_hurt_the_kernel(calls in calls()) {
        survive::<Aarch64>(Arch::Aarch64, false, &calls);
    }

    /// Every call that takes a pointer answers `EFAULT` for one outside the
    /// cell: in the kernel half, non-canonical, the last page of the lower
    /// half, or simply unmapped.
    #[test]
    fn pointers_outside_the_cell_give_efault(
        p in prop_oneof![
            0xffff_8000_0000_0000u64..=u64::MAX,
            USER_END..0xffff_8000_0000_0000,
            prop::sample::select(vec![KERNEL_DATA, KERNEL_LARGE, KERNEL_TEXT, USER_END]),
            0x1000u64..0x8_0000,
        ],
        n in 1u64..=1 << 20,
    ) {
        for (arch, shared) in [(Arch::X86_64, true), (Arch::Aarch64, false)] {
            let mut m = Machine::<X86_64>::new(arch, shared);
            let kernel = m.kernel_memory();
            for (a64, x86, args) in pointer_calls(p, n) {
                let nr = if arch == Arch::Aarch64 { a64 } else { x86 };
                let (_, result) = m.call(nr, args);
                prop_assert_eq!(result, -EFAULT, "call {} {:x?}", nr, args);
            }
            prop_assert!(m.kernel_memory() == kernel, "kernel memory changed");
            m.end();
        }
    }
}

/// (AArch64 number, x86_64 number, arguments) of every call that reads or
/// writes user memory, with `p` as its pointer and `n` as its length.
fn pointer_calls(p: u64, n: u64) -> Vec<(u64, u64, [u64; 6])> {
    let cwd = AT_FDCWD;
    vec![
        (64, 1, [1, p, n, 0, 0, 0]),                 // write (console)
        (64, 1, [FILE_FD, p, n, 0, 0, 0]),           // write (file)
        (63, 0, [FILE_FD, p, n, 0, 0, 0]),           // read
        (67, 17, [FILE_FD, p, n, 0, 0, 0]),          // pread64
        (68, 18, [FILE_FD, p, n, 0, 0, 0]),          // pwrite64
        (66, 20, [1, p, 1, 0, 0, 0]),                // writev
        (160, 63, [p, 0, 0, 0, 0, 0]),               // uname
        (113, 228, [1, p, 0, 0, 0, 0]),              // clock_gettime
        (278, 318, [p, n, 0, 0, 0, 0]),              // getrandom
        (80, 5, [1, p, 0, 0, 0, 0]),                 // fstat
        (17, 79, [p, 4096, 0, 0, 0, 0]),             // getcwd
        (56, 257, [cwd, p, 0, 0, 0, 0]),             // openat
        (56, 257, [cwd, p, 0x42, 0, 0, 0]),          // openat (O_CREAT)
        (34, 258, [cwd, p, 0o755, 0, 0, 0]),         // mkdirat
        (35, 263, [cwd, p, 0, 0, 0, 0]),             // unlinkat
        (49, 80, [p, 0, 0, 0, 0, 0]),                // chdir
        (48, 269, [cwd, p, 0, 0, 0, 0]),             // faccessat
        (79, 262, [cwd, p, DATA, 0, 0, 0]),          // newfstatat (path)
        (79, 262, [FILE_FD, DATA, p, 0x1000, 0, 0]), // newfstatat (result)
        (38, 264, [cwd, p, cwd, DATA, 0, 0]),        // renameat
        (101, 35, [p, 0, 0, 0, 0, 0]),               // nanosleep
        (115, 230, [1, 0, p, 0, 0, 0]),              // clock_nanosleep
        (72, 270, [0, 0, 0, 0, p, 0]),               // pselect6
        (134, 13, [2, 0, p, 8, 0, 0]),               // rt_sigaction
        (135, 14, [0, 0, p, 8, 0, 0]),               // rt_sigprocmask
        (261, 302, [0, 3, 0, p, 0, 0]),              // prlimit64
        (163, 97, [3, p, 0, 0, 0, 0]),               // getrlimit
    ]
}

// x86_64 call numbers.
const X_MMAP: u64 = 9;
const X_MPROTECT: u64 = 10;
const X_MUNMAP: u64 = 11;
const X_ARCH_PRCTL: u64 = 158;

#[test]
fn mprotect_cannot_open_kernel_pages_to_a_cell() {
    let mut m = Machine::<X86_64>::new(Arch::X86_64, true);
    let kernel = m.kernel_memory();
    for (addr, len) in [
        (KERNEL_DATA, FRAME_SIZE),
        (KERNEL_LARGE, LARGE_PAGE_SIZE),
        (KERNEL_TEXT, FRAME_SIZE),
        (DATA, KERNEL_DATA - DATA + FRAME_SIZE),
    ] {
        for prot in [1, 3, 5] {
            let (_, r) = m.call(X_MPROTECT, [addr, len, prot, 0, 0, 0]);
            assert!(r < 0, "mprotect({addr:#x}, {len:#x}, {prot}) = {r}");
        }
    }
    assert!(m.kernel_memory() == kernel, "kernel pages unchanged");
    m.end();
}

#[test]
fn munmap_cannot_remove_kernel_pages() {
    let mut m = Machine::<X86_64>::new(Arch::X86_64, true);
    let kernel = m.kernel_memory();
    for (addr, len) in [
        (KERNEL_DATA, FRAME_SIZE),
        (0, u64::MAX),
        (DATA, u64::MAX - DATA),
    ] {
        assert_eq!(m.call(X_MUNMAP, [addr, len, 0, 0, 0, 0]).1, -EINVAL);
    }
    assert!(m.kernel_memory() == kernel, "kernel pages unchanged");
    m.end();
}

/// The kernel runs a call to its end with no preemption: a range of the
/// whole user half, nearly all of it empty, must not take a step per page
/// (2^35 steps would stop every cell for hours).
#[test]
fn huge_empty_ranges_take_few_steps() {
    let mut m = Machine::<X86_64>::new(Arch::X86_64, true);
    let start = std::time::Instant::now();
    assert_eq!(m.call(X_MPROTECT, [0, USER_END, 1, 0, 0, 0]).1, -ENOMEM);
    assert_eq!(
        m.call(X_MPROTECT, [DATA, USER_END - DATA, 1, 0, 0, 0]).1,
        -ENOMEM
    );
    assert_eq!(
        m.call(X_MUNMAP, [MMAP_BASE, USER_END - MMAP_BASE, 0, 0, 0, 0])
            .1,
        0
    );
    assert_eq!(m.call(X_MUNMAP, [0, USER_END, 0, 0, 0, 0]).1, 0);
    assert!(m.pt_translate(DATA).is_none(), "everything unmapped");
    // Writable and executable over terabytes: refused, or a range at once.
    assert_eq!(m.call(X_MPROTECT, [0, USER_END, 7, 0, 0, 0]).1, -ENOMEM);
    let anon = 0x22; // MAP_PRIVATE | MAP_ANONYMOUS
    let huge = 1 << 45;
    let at = m.call(X_MMAP, [0, huge, 0, anon, u64::MAX, 0]).1 as u64;
    assert_eq!(m.call(X_MPROTECT, [at, huge, 7, 0, 0, 0]).1, 0);
    assert_eq!(m.call(X_MUNMAP, [at, huge, 0, 0, 0, 0]).1, 0);
    assert!(start.elapsed().as_secs() < 10, "{:?}", start.elapsed());
    m.end();
}

#[test]
fn the_thread_pointer_stays_in_the_cell() {
    const ARCH_SET_FS: u64 = 0x1002;
    const EPERM: i64 = 1;
    let mut m = Machine::<X86_64>::new(Arch::X86_64, true);
    for bad in [
        0x0000_8000_0000_0000,
        0x8000_0000_0000_0000,
        KERNEL_DATA,
        u64::MAX,
    ] {
        assert_eq!(
            m.call(X_ARCH_PRCTL, [ARCH_SET_FS, bad, 0, 0, 0, 0]).1,
            -EPERM
        );
    }
    assert_eq!(m.cell.regs().tls, 0, "unchanged");
    assert_eq!(m.call(X_ARCH_PRCTL, [ARCH_SET_FS, DATA, 0, 0, 0, 0]).1, 0);
    assert_eq!(m.cell.regs().tls, DATA);
    m.end();
}

#[test]
fn unknown_calls_are_reported_once_whatever_their_number() {
    let mut m = Machine::<X86_64>::new(Arch::X86_64, true);
    for nr in [5000, 5000, u64::MAX, u64::MAX, 1 << 40, 1023, 1023] {
        assert_eq!(m.call(nr, [0; 6]).1, -ENOSYS);
    }
    assert!(m.services.logged <= 4, "{} messages", m.services.logged);
    m.end();
}

/// A file system holding something of every kind the encoding knows:
/// directories, a file with a hole and a partial last block, a second name
/// for it, a pipe holding bytes, an event counter, an `epoll` instance
/// watching them, a Unix socket name, and a working directory below the
/// root.
fn rich_fs() -> rhizo_fs::Fs {
    use rhizo_fs::{EpollOp, OpenFlags};
    let mut fs = rhizo_fs::Fs::new();
    fs.mkdir(None, b"/d").unwrap();
    fs.mkdir(None, b"/d/e").unwrap();
    fs.put_file(b"/d/f", &[7; 5000]).unwrap();
    let rw = OpenFlags {
        read: true,
        write: true,
        ..OpenFlags::default()
    };
    let f = fs.open(None, b"/d/f", rw, 2).unwrap();
    fs.write(f, b"far", Some(5 * 4096)).unwrap();
    let (r, w) = fs.pipe(false, false).unwrap();
    fs.write_pipe(w, b"queued").unwrap();
    let ev = fs.eventfd(3, false, true, false).unwrap();
    let ep = fs.epoll_create(false).unwrap();
    fs.epoll_ctl(ep, EpollOp::Add, r, 1, 9).unwrap();
    fs.epoll_ctl(ep, EpollOp::Add, ev, 1, 10).unwrap();
    fs.bind_name(None, b"/d/sock", 77).unwrap();
    fs.dup(f, 20, true, true).unwrap();
    fs.chdir(b"/d/e").unwrap();
    fs
}

/// Calls on descriptors and paths: any of them, with descriptors and
/// small values most of the time.
fn file_calls() -> impl Strategy<Value = Vec<(u64, [u64; 6])>> {
    // x86_64: read, write, close, fstat, poll, lseek, mmap, pread64,
    // pwrite64, readv, writev, pipe, dup, dup2, fcntl, ftruncate,
    // getdents64, getcwd, chdir, epoll_wait, epoll_ctl, openat, mkdirat,
    // unlinkat, renameat, eventfd2, newfstatat, ioctl, connect, bind.
    let nr = prop::sample::select(vec![
        0u64, 1, 3, 5, 7, 8, 9, 17, 18, 19, 20, 22, 32, 33, 72, 77, 217, 79, 80, 232, 233, 257,
        258, 263, 264, 290, 262, 16, 42, 49,
    ]);
    let small = || prop_oneof![4 => (0u64..24).boxed(), 1 => arg().boxed()].boxed();
    prop::collection::vec(
        (
            nr,
            [
                small(),
                arg().boxed(),
                small(),
                small(),
                small(),
                arg().boxed(),
            ],
        ),
        1..30,
    )
}

/// `bytes` with `edits` (position modulo the length, new byte) made and
/// cut to `keep` bytes when given.
fn mutated(mut bytes: Vec<u8>, edits: &[(usize, u8)], keep: Option<usize>) -> Vec<u8> {
    for &(at, b) in edits {
        let n = bytes.len();
        bytes[at % n] = b;
    }
    if let Some(k) = keep {
        bytes.truncate(k % (bytes.len() + 1));
    }
    bytes
}

proptest! {
    /// A file system decoded from tampered bytes (as a snapshot from
    /// outside holds them) is refused or works: no call on it panics, and
    /// the kernel survives as with any cell.
    #[test]
    fn decoded_files_cannot_hurt_the_kernel(
        edits in prop::collection::vec((any::<usize>(), any::<u8>()), 0..4),
        keep in prop::option::weighted(0.1, any::<usize>()),
        calls in file_calls(),
    ) {
        let mut bytes = Vec::new();
        rich_fs().encode(&mut bytes);
        let bytes = mutated(bytes, &edits, keep);
        if let Ok(fs) = rhizo_fs::Fs::decode(&bytes) {
            let mut m = Machine::<X86_64>::new(Arch::X86_64, true);
            m.fs = fs;
            survive_in(m, &calls);
        }
    }

    /// Random bytes never decode into a file system that breaks.
    #[test]
    fn random_bytes_are_refused_or_work(
        bytes in prop::collection::vec(any::<u8>(), 0..256),
        calls in file_calls(),
    ) {
        if let Ok(fs) = rhizo_fs::Fs::decode(&bytes) {
            let mut m = Machine::<Aarch64>::new(Arch::Aarch64, false);
            m.fs = fs;
            survive_in(m, &calls);
        }
    }
}

/// Finding S16: buffers whose lengths add up past 2^64 (each one at most
/// `SSIZE_MAX`, as Linux allows) made `sendmsg` and `recvmsg` overflow,
/// which panics a kernel built with overflow checks.
#[test]
fn buffer_lengths_that_overflow_their_sum_are_harmless() {
    const X_SENDMSG: u64 = 46;
    const X_RECVMSG: u64 = 47;
    const X_SOCKETPAIR: u64 = 53;
    let mut m = Machine::<X86_64>::new(Arch::X86_64, true);
    assert_eq!(m.call(X_SOCKETPAIR, [1, 1, 0, DATA, 0, 0]).1, 0);
    let mut fds = [0u8; 8];
    let pt = PageTable::<X86_64>::from_root(m.cell.root);
    memory::copy_from(&pt, &m.mem, DATA, &mut fds).unwrap();
    let (a, b) = (
        u64::from(u32::from_le_bytes(fds[..4].try_into().unwrap())),
        u64::from(u32::from_le_bytes(fds[4..].try_into().unwrap())),
    );
    // Three buffers of SSIZE_MAX bytes each, all at the cell's page, and a
    // `msghdr` pointing at them.
    let (iov, msg) = (DATA + 64, DATA + 256);
    let mut list = Vec::new();
    for _ in 0..3 {
        list.extend_from_slice(&(DATA + 1024).to_le_bytes());
        list.extend_from_slice(&(i64::MAX as u64).to_le_bytes());
    }
    let mut hdr = [0u8; 56];
    hdr[16..24].copy_from_slice(&iov.to_le_bytes());
    hdr[24..32].copy_from_slice(&3u64.to_le_bytes());
    memory::copy_to(&pt, &mut m.mem, iov, &list).unwrap();
    memory::copy_to(&pt, &mut m.mem, msg, &hdr).unwrap();
    let (_, sent) = m.call(X_SENDMSG, [a, msg, 0, 0, 0, 0]);
    assert!(sent == -EFAULT || sent > 0, "sendmsg: {sent}");
    let (_, got) = m.call(X_RECVMSG, [b, msg, 0x40, 0, 0, 0]);
    assert!(
        got == -EAGAIN || got == -EFAULT || got >= 0,
        "recvmsg: {got}"
    );
    m.end();
}

/// A pager that keeps write-or-execute ranges and pages nothing in.
#[derive(Default)]
struct Ranges(WxRanges);

impl<F: EntryFormat> Pager<F, FakePhysicalMemory> for Ranges {
    fn page_in(&mut self, _: &PageTable<F>, _: &mut FakePhysicalMemory, _: u64, _: bool) -> bool {
        false
    }

    fn wx(&mut self) -> Option<&mut WxRanges> {
        Some(&mut self.0)
    }
}
