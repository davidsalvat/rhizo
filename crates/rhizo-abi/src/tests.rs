//! Host tests: each call runs against a real page-table hierarchy in fake
//! physical memory, through the same entry point the kernel uses.

use super::*;
use rhizo_cell::testing::CountingFrames;
use rhizo_cell::threads::Wait;
use rhizo_hal::testing::{FakePhysicalMemory, RecordingConsole, RecordingTlb};
use rhizo_mm::paging::{Aarch64, PageSize};

#[derive(Default)]
pub(crate) struct FakeServices {
    console: RecordingConsole,
    log: String,
    pub(crate) clock: u64,
    /// Bytes the cell may still store (`None`: unlimited).
    room: Option<u64>,
    /// Word 0 of the FP/SIMD state "in the CPU" (`save_fp`), if any.
    cpu_fp: Option<u64>,
    /// The memory cells share (`memory`).
    memory: u64,
    /// The egress refusals reported (`egress_denied`).
    pub(crate) denied: Vec<(u32, rhizo_net::Proto, [u8; 4], u16)>,
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
    fn can_store(&mut self, bytes: u64) -> bool {
        match &mut self.room {
            None => true,
            Some(room) if *room >= bytes => {
                *room -= bytes;
                true
            }
            Some(_) => false,
        }
    }
    fn memory(&mut self) -> u64 {
        self.memory
    }
    fn save_fp(&mut self, fp: &mut rhizo_cell::FpState) {
        if let Some(word) = self.cpu_fp {
            fp.0[0] = word;
        }
    }
    fn egress_denied(&mut self, cell: u32, proto: rhizo_net::Proto, ip: [u8; 4], port: u16) {
        self.denied.push((cell, proto, ip, port));
    }
}

/// A cell with one writable page at `DATA`, a read-only page at `RO`, and
/// its heap starting at `HEAP`.
pub(crate) struct World {
    pub(crate) cell: Cell,
    abi: Abi,
    mem: FakePhysicalMemory,
    tlb: RecordingTlb,
    frames: CountingFrames,
    pub(crate) services: FakeServices,
    pub(crate) fs: rhizo_fs::Fs,
    pub(crate) net: rhizo_net::Net,
    pub(crate) pages: PageCache,
    /// Keeps the cell's write-or-execute ranges, and backs their pages
    /// for system calls when told to, as the kernel's pager does.
    pub(crate) pager: Ranges,
    /// Deadline of the last blocked call (kept, as the kernel does).
    pub(crate) deadline: Option<u64>,
    /// The last call waits on descriptors: with this deadline.
    pub(crate) io: Option<Option<u64>>,
    /// Whether a call reached the sockets.
    pub(crate) used_net: bool,
}

/// The sockets, noting whether a call reached them.
struct Noting<'a>(&'a mut rhizo_net::Net, &'a mut bool);

impl Sockets for Noting<'_> {
    fn net(&mut self) -> &mut rhizo_net::Net {
        *self.1 = true;
        self.0
    }
}

pub(crate) const DATA: u64 = 0x10_0000;
const RO: u64 = 0x11_0000;
const HEAP: u64 = 0x20_0000;

impl World {
    pub(crate) fn new(arch: Arch) -> Self {
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
            net: rhizo_net::Net::new(),
            pages: PageCache::new(64),
            pager: Ranges::default(),
            deadline: None,
            io: None,
            used_net: false,
        }
    }

    fn pt(&self) -> PageTable<Aarch64> {
        PageTable::from_root(self.cell.root)
    }

    /// Calls `name` (AArch64 numbering unless the world is x86_64).
    pub(crate) fn call(&mut self, nr: u64, args: &[u64]) -> (i64, Outcome) {
        let regs = &mut self.cell.regs_mut().gpr;
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
            net: &mut Noting(&mut self.net, &mut self.used_net),
            deadline: self.deadline.take(),
            pager: &mut self.pager,
            pages: &mut self.pages,
        };
        let out = self.abi.handle(&mut self.cell, &mut cx);
        // A thread waiting on descriptors keeps its deadline and makes its
        // call again when it runs, as the kernel does when it wakes it.
        let thread = self.cell.threads.current_mut();
        self.io = match thread.wait {
            Wait::Io { until } => {
                thread.wait = Wait::Ready;
                self.deadline = until;
                Some(until)
            }
            _ => None,
        };
        (self.cell.regs().gpr[0] as i64, out)
    }

    /// The deadline the current thread sleeps until, if it sleeps; it is
    /// then made runnable again, as the kernel does at that time.
    fn slept(&mut self) -> Option<u64> {
        let thread = self.cell.threads.current_mut();
        match core::mem::replace(&mut thread.wait, Wait::Ready) {
            Wait::Sleep(until) => Some(until),
            _ => None,
        }
    }

    pub(crate) fn poke(&mut self, addr: u64, data: &[u8]) {
        memory::copy_to(&self.pt(), &mut self.mem, addr, data).unwrap();
    }

    pub(crate) fn peek(&self, addr: u64, len: usize) -> Vec<u8> {
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

/// A pager keeping write-or-execute ranges (design D5 of `node-jit`); with
/// `backing`, it backs their missing pages from frames of its own (far from
/// the world's, never given back), as the kernel's does from its allocator.
#[derive(Default)]
pub(crate) struct Ranges {
    pub(crate) wx: WxRanges,
    backing: bool,
    next: u64,
}

impl Frames for Ranges {
    fn alloc(&mut self) -> Option<u64> {
        self.next += FRAME_SIZE;
        Some((1 << 40) + self.next)
    }
    fn alloc_large(&mut self) -> Option<u64> {
        None
    }
    fn free(&mut self, _: u64, _: PageSize) {}
    fn share(&mut self, _: u64) {}
    fn is_shared(&self, _: u64) -> bool {
        false
    }
}

impl Pager<Aarch64, FakePhysicalMemory> for Ranges {
    fn page_in(
        &mut self,
        pt: &PageTable<Aarch64>,
        mem: &mut FakePhysicalMemory,
        addr: u64,
        _: bool,
    ) -> bool {
        if !self.backing || !self.wx.contains(addr) || pt.translate(mem, addr).is_some() {
            return false;
        }
        let page = addr & !(FRAME_SIZE - 1);
        let tlb = &mut RecordingTlb::default();
        space::map_anonymous(pt, mem, tlb, self, page, FRAME_SIZE, space::WX_WRITING).is_ok()
    }

    fn wx(&mut self) -> Option<&mut WxRanges> {
        Some(&mut self.wx)
    }
}
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
    // Files can be mapped now (`add-dynamic-linking`): descriptor 3 is not
    // open.
    assert_eq!(
        w.call(MMAP, &[0, 4096, 3, MAP_PRIVATE, 3, 0]).0,
        -EBADF,
        "files"
    );
    // Writable and executable anonymous memory is a write-or-execute range
    // now, without memory until touched (change `node-jit`).
    let before = w.frames.outstanding();
    let wx = w.call(MMAP, &[0, 4096, 6, anon, u64::MAX, 0]).0 as u64;
    assert!(wx >= MMAP_BASE, "W+X mapped");
    assert!(w.pt().translate(&w.mem, wx).is_none());
    assert_eq!(w.frames.outstanding(), before);
    assert_eq!(
        w.call(MMAP, &[0, u64::MAX - 10, 3, anon, u64::MAX, 0]).0,
        -ENOMEM
    );
    assert_eq!(w.call(MUNMAP, &[small + 1, 10]).0, -EINVAL);
}

/// What a dynamic loader does with a library (spec `linux-abi`, file
/// mappings): map it whole read-only, map its data over part of it with
/// `MAP_FIXED`, its `.bss` anonymously, and make the gap inaccessible.
/// File pages are shared by every mapping of the same bytes; writes stay
/// private.
#[test]
fn files_map_privately_and_share_their_pages() {
    let mut w = World::new(Arch::Aarch64);
    let mut lib = vec![b'A'; 4096];
    lib.extend([b'B'; 4096]);
    lib.extend([b'C'; 100]);
    w.fs.put_file(b"/lib/libx.so", &lib).unwrap();
    let read = rhizo_fs::OpenFlags {
        read: true,
        ..Default::default()
    };
    let fd = u64::from(w.fs.open(None, b"/lib/libx.so", read, 0).unwrap());
    let held = |w: &World| w.frames.outstanding() - w.pt().lower_half_tables(&w.mem);
    let before = held(&w);
    let private = MAP_PRIVATE;
    let base = w.call(MMAP, &[0, 3 * 4096, 1, private, fd, 0]).0 as u64;
    assert_eq!(base, MMAP_BASE);
    assert_eq!(w.peek(base, 2), b"AA");
    assert_eq!(w.peek(base + 4096, 2), b"BB");
    assert_eq!(w.peek(base + 8192 + 98, 4), b"CC\0\0", "zero past the end");
    let phys = |w: &World, at| w.pt().translate(&w.mem, at).unwrap();
    assert!(!phys(&w, base).flags.is_writable());

    // A second mapping of the same file (another cell loading it) shares.
    let again = w.call(MMAP, &[0, 4096, 5, private, fd, 4096]).0 as u64;
    assert_eq!(again, base + 3 * 4096);
    assert_eq!(phys(&w, again).phys, phys(&w, base + 4096).phys);
    assert!(phys(&w, again).flags.is_executable());

    // Data over the middle page: the same frame, copy-on-write.
    let data = w.call(MMAP, &[base + 4096, 4096, 3, private | MAP_FIXED, fd, 4096]);
    assert_eq!(data.0 as u64, base + 4096);
    let t = phys(&w, base + 4096);
    assert_eq!(t.phys, phys(&w, again).phys);
    assert!(t.flags.contains(PageFlags::COPY_ON_WRITE));
    assert_eq!(
        w.pt()
            .resolve_write(&mut w.mem, &mut w.frames, &mut w.tlb, base + 4096),
        Ok(true)
    );
    w.poke(base + 4096, b"mine");
    assert_eq!(w.peek(again, 4), b"BBBB", "the file's page is unchanged");
    // `.bss` over the last page, and an inaccessible gap.
    let bss = w.call(
        MMAP,
        &[
            base + 8192,
            4096,
            3,
            MAP_ANONYMOUS | private | MAP_FIXED,
            u64::MAX,
            0,
        ],
    );
    assert_eq!(bss.0 as u64, base + 8192);
    assert_eq!(w.peek(base + 8192, 2), b"\0\0");
    assert_eq!(w.call(MPROTECT, &[base, 4096, 0]).0, 0);
    assert!(w.pt().translate(&w.mem, base).is_none(), "nothing kept");
    assert_eq!(w.call(MPROTECT, &[base, 4096, 3]).0, 0, "reserved");
    assert_eq!(w.peek(base, 2), b"\0\0");

    // What cannot be mapped.
    assert_eq!(w.call(MMAP, &[0, 4096, 1, private, fd, 100]).0, -EINVAL);
    assert_eq!(w.call(MMAP, &[0, 4096, 6, private, fd, 0]).0, -EACCES);
    assert_eq!(w.call(MMAP, &[0, 4096, 1, 1, fd, 0]).0, -ENODEV, "shared");
    assert_eq!(w.call(MMAP, &[0, 4096, 1, private, 99, 0]).0, -EBADF);
    assert_eq!(
        w.call(MMAP, &[0, 4096, 1, private, 0, 0]).0,
        -ENODEV,
        "console"
    );
    let write_only = rhizo_fs::OpenFlags {
        write: true,
        ..Default::default()
    };
    let wo = u64::from(w.fs.open(None, b"/lib/libx.so", write_only, 1).unwrap());
    assert_eq!(w.call(MMAP, &[0, 4096, 1, private, wo, 0]).0, -EACCES);
    assert_eq!(
        w.call(MMAP, &[0, 4096, 1, private | MAP_FIXED, fd, 0]).0,
        -EPERM
    );
    let past = USER_END - 4096;
    assert_eq!(
        w.call(MMAP, &[past, 8192, 1, private | MAP_FIXED, fd, 0]).0,
        -EINVAL
    );

    // The file written: a new mapping has its new bytes, the old ones keep
    // theirs.
    let rw = rhizo_fs::OpenFlags {
        read: true,
        write: true,
        ..Default::default()
    };
    let wfd = w.fs.open(None, b"/lib/libx.so", rw, 2).unwrap();
    w.fs.write(wfd, b"new", Some(4096)).unwrap();
    let fresh = w.call(MMAP, &[0, 4096, 1, private, fd, 4096]).0 as u64;
    assert_eq!(fresh, base + 4 * 4096);
    assert_eq!(w.peek(fresh, 4), b"newB");
    assert_eq!(w.peek(again, 4), b"BBBB");

    // Unmapped and dropped from the cache: every frame is back.
    assert_eq!(w.call(MUNMAP, &[base, 5 * 4096]).0, 0);
    assert!(w.pages.trim(&mut w.frames));
    assert_eq!(held(&w), before);
}

/// The calls coreutils ask before they start (spec `linux-abi`).
#[test]
fn sysinfo_affinity_and_advice() {
    let mut w = World::new(Arch::Aarch64);
    w.services.clock = 5_000_000_000;
    assert_eq!(w.call(179, &[DATA]).0, 0);
    let info = w.peek(DATA, 112);
    assert_eq!(info[..8], 5u64.to_le_bytes(), "uptime");
    assert_eq!(info[104..108], 1u32.to_le_bytes(), "mem_unit");
    assert_eq!(w.call(179, &[RO]).0, -EFAULT);
    assert_eq!(w.call(123, &[0, 128, DATA]).0, 8);
    assert_eq!(w.peek(DATA, 8), 1u64.to_le_bytes(), "one CPU");
    assert_eq!(w.call(123, &[0, 4, DATA]).0, -EINVAL);
    assert_eq!(w.call(223, &[1, 0, 0, 2]).0, 0);
    assert_eq!(w.call(223, &[99, 0, 0, 2]).0, -EBADF);
}

/// Hostile (finding S24): `sysinfo` shows the cell only itself. Its memory
/// is its own limit and use as the kernel reports them (whatever other
/// cells hold), and its processes are its own threads.
#[test]
fn sysinfo_shows_only_the_cell_itself() {
    let mut w = World::new(Arch::Aarch64);
    w.services.memory = 1 << 30;
    // 10 MiB used of a 64 MiB quota.
    w.frames.metered = Some((10 << 8, 64 << 8));
    for _ in 0..2 {
        w.cell
            .threads
            .spawn(rhizo_cell::Registers::default(), 0)
            .unwrap();
    }
    assert_eq!(w.call(179, &[DATA]).0, 0);
    let info = w.peek(DATA, 112);
    let word = |at: usize| u64::from_le_bytes(info[at..at + 8].try_into().unwrap());
    assert_eq!(word(32), 64 << 20, "totalram: the cell's limit");
    assert_eq!(word(40), 54 << 20, "freeram: what it leaves of it");
    assert_eq!(info[80..82], 3u16.to_le_bytes(), "procs: its threads");
    assert_eq!(info[8..32], [0; 24], "no load");
    // Past its limit (memory it inherited from a fork), nothing is free.
    w.frames.metered = Some((9 << 8, 8 << 8));
    assert_eq!(w.call(179, &[DATA]).0, 0);
    assert_eq!(w.peek(DATA + 32, 8), (8u64 << 20).to_le_bytes());
    assert_eq!(w.peek(DATA + 40, 8), [0; 8]);
    // Without a quota: all the memory cells share, less its own use.
    w.frames.metered = None;
    assert_eq!(w.call(179, &[DATA]).0, 0);
    assert_eq!(w.peek(DATA + 32, 8), (1u64 << 30).to_le_bytes());
}

/// Spec `linux-abi`, "Memory of a limited cell" (task 1.7 of
/// `add-cell-hierarchy`): a cell with a 64 MiB limit that took 10 MiB reads
/// 64 MiB total and 54 MiB free, while another domain takes frames from the
/// same allocator and gives them back.
#[test]
fn sysinfo_per_cell() {
    use rhizo_mm::cow::Frames;
    let mut w = World::new(Arch::X86_64);
    w.services.memory = 1 << 30;
    w.frames.metered = Some((10 << 8, 64 << 8));
    let read = |w: &mut World| {
        assert_eq!(w.call(99, &[DATA]).0, 0);
        let info = w.peek(DATA, 112);
        let word = |at: usize| u64::from_le_bytes(info[at..at + 8].try_into().unwrap());
        (word(32), word(40))
    };
    assert_eq!(read(&mut w), (64 << 20, 54 << 20));
    let taken: Vec<u64> = (0..4096).map(|_| w.frames.alloc().unwrap()).collect();
    assert_eq!(
        read(&mut w),
        (64 << 20, 54 << 20),
        "while another domain holds 16 MiB"
    );
    for phys in taken {
        w.frames.free(phys, rhizo_mm::paging::PageSize::Small);
    }
    assert_eq!(read(&mut w), (64 << 20, 54 << 20), "once it gave them back");
}

/// Spec `linux-abi`, "Identifiers per domain": `getpid` is the cell's
/// number in its domain, not the kernel's identifier, and calls naming a
/// process take that number.
#[test]
fn identifiers_come_from_the_domain() {
    let mut w = World::new(Arch::Aarch64);
    w.cell.id = 4711;
    w.cell.pid = 3;
    assert_eq!(w.call(172, &[]).0, 3, "getpid");
    assert_eq!(w.call(121, &[3, DATA]).0, 0, "sched_getparam of itself");
    assert_eq!(
        w.call(121, &[4711, DATA]).0,
        -ESRCH,
        "the kernel's identifier names nothing"
    );
}

/// The calls Node.js makes at start-up and under load (spec `linux-abi`,
/// change `add-node-workload`), with the AArch64 numbers and the x86_64
/// ones.
#[test]
fn node_start_up_calls() {
    for (arch, n) in [
        (Arch::Aarch64, [169, 114, 90, 167, 121, 120, 425]),
        (Arch::X86_64, [96, 229, 125, 157, 143, 145, 425]),
    ] {
        let [
            gettimeofday,
            getres,
            capget,
            prctl,
            getparam,
            getscheduler,
            io_uring,
        ] = n;
        let mut w = World::new(arch);
        w.services.clock = 2_500_000_000;
        // gettimeofday: seconds and microseconds of the realtime clock; an
        // empty time zone.
        w.poke(DATA + 16, &[0xff; 8]);
        assert_eq!(w.call(gettimeofday, &[DATA, DATA + 16]).0, 0);
        let tv = w.peek(DATA, 16);
        assert_eq!(tv[..8], 1_700_000_002u64.to_le_bytes());
        assert_eq!(tv[8..], 500_000u64.to_le_bytes());
        assert_eq!(w.peek(DATA + 16, 8), [0; 8]);
        assert_eq!(w.call(gettimeofday, &[0, 0]).0, 0);
        assert_eq!(w.call(gettimeofday, &[RO, 0]).0, -EFAULT);
        // clock_getres: one nanosecond on every clock clock_gettime reads.
        assert_eq!(w.call(getres, &[1, DATA]).0, 0);
        assert_eq!(
            w.peek(DATA, 16),
            [0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0]
        );
        assert_eq!(w.call(getres, &[0, 0]).0, 0);
        assert_eq!(w.call(getres, &[3, DATA]).0, -EINVAL);
        // capget: no capabilities; an unknown version is answered with 3.
        w.poke(DATA, &0x2008_0522u32.to_le_bytes());
        w.poke(DATA + 16, &[0xff; 24]);
        assert_eq!(w.call(capget, &[DATA, DATA + 16]).0, 0);
        assert_eq!(w.peek(DATA + 16, 24), [0; 24]);
        w.poke(DATA, &7u32.to_le_bytes());
        assert_eq!(w.call(capget, &[DATA, DATA + 16]).0, -EINVAL);
        assert_eq!(w.peek(DATA, 4), 0x2008_0522u32.to_le_bytes());
        assert_eq!(w.call(capget, &[RO + 4096 * 64, 0]).0, -EFAULT);
        // prctl: naming a thread is accepted, other options are not known.
        assert_eq!(w.call(prctl, &[15, DATA]).0, 0);
        assert_eq!(w.call(prctl, &[16, DATA]).0, 0);
        assert_eq!(w.call(prctl, &[0x5961_6d61, 0]).0, -EINVAL);
        // The normal policy, priority 0, for the caller or a thread of it.
        w.poke(DATA, &[0xff; 4]);
        assert_eq!(w.call(getparam, &[0, DATA]).0, 0);
        assert_eq!(w.peek(DATA, 4), [0; 4]);
        assert_eq!(w.call(getscheduler, &[5]).0, 0);
        assert_eq!(w.call(getscheduler, &[77]).0, -ESRCH);
        assert_eq!(w.call(getparam, &[-1i64 as u64, DATA]).0, -EINVAL);
        // io_uring is declined on purpose (libuv falls back), unreported.
        assert_eq!(w.call(io_uring, &[256, DATA]).0, -ENOSYS);
        assert_eq!(w.services.log, "", "{arch:?}");
    }
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
    assert_eq!(w.cell.regs().tls, 0x7000);
    assert_eq!(w.call(158, &[0x1003, DATA]).0, 0, "arch_prctl(GET_FS)");
    assert_eq!(w.peek(DATA, 8), 0x7000u64.to_le_bytes());
    assert_eq!(w.call(16, &[1, 0x5401, 0]).0, -ENOTTY, "ioctl");
    assert_eq!(w.call(231, &[3]).1, Outcome::Exit(3));
    // arch_prctl does not exist on AArch64 (167 is prctl there).
    let mut a = World::new(Arch::Aarch64);
    assert_eq!(a.call(158, &[0x1002, 0]).0, -ENOSYS);
}

#[test]
fn call_numbers_names_and_socket_calls_for_reports() {
    let mut w = World::new(Arch::X86_64);
    w.cell.regs_mut().gpr[0] = 41;
    assert_eq!(Arch::X86_64.call_number(&w.cell), 41);
    assert_eq!(
        format!("{:?}", Arch::X86_64.call_name(41).unwrap()),
        "Socket"
    );
    assert_eq!(
        format!("{:?}", Arch::Aarch64.call_name(63).unwrap()),
        "Read"
    );
    assert!(Arch::X86_64.call_name(9999).is_none());
    assert!(Arch::X86_64.is_socket_call(41));
    assert!(Arch::Aarch64.is_socket_call(206), "sendto");
    assert!(
        !Arch::Aarch64.is_socket_call(63),
        "read is not only for sockets"
    );
    assert!(!Arch::X86_64.is_socket_call(9999));
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
        (0, Outcome::Block),
        "nanosleep 20 ms"
    );
    assert_eq!(w.slept(), Some(20_001_000), "nanosleep 20 ms");
    w.poke(DATA, &ts(2, 0));
    assert_eq!(
        w.call(115, &[1, 1, DATA, 0]),
        (0, Outcome::Block),
        "clock_nanosleep(MONOTONIC, TIMER_ABSTIME)"
    );
    assert_eq!(
        w.slept(),
        Some(2_000_000_000),
        "clock_nanosleep(MONOTONIC, TIMER_ABSTIME)"
    );
    // Realtime is monotonic + 1.7e18 in the fake: an absolute realtime
    // deadline 5 ns after "now" sleeps 5 ns.
    let deadline = 1_700_000_000_000_000_000i64 + 1_000 + 5;
    w.poke(
        DATA,
        &ts(deadline / 1_000_000_000, deadline % 1_000_000_000),
    );
    assert_eq!(w.call(115, &[0, 1, DATA, 0]), (0, Outcome::Block));
    assert_eq!(w.slept(), Some(1_005));

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
fn rlimits_report_the_cells_quotas() {
    let mut w = World::new(Arch::Aarch64);
    let limit = |w: &mut World, resource: u64| {
        assert_eq!(w.call(163, &[resource, DATA]).0, 0, "getrlimit");
        u64::from_le_bytes(w.peek(DATA, 8).try_into().unwrap())
    };
    assert_eq!(limit(&mut w, 0), u64::MAX, "no quota: unlimited");
    assert_eq!(limit(&mut w, 7), 1024);
    w.cell.quota = rhizo_cell::quota::Quota {
        memory: 64 << 20,
        cpu_ns: 1_500_000_000,
        files: 32,
        sockets: 4,
    };
    assert_eq!(limit(&mut w, 0), 2, "CPU seconds, rounded up");
    assert_eq!(limit(&mut w, 7), 32);
    assert_eq!(limit(&mut w, 9), 64 << 20);
}

#[test]
fn libc_startup_calls() {
    let mut w = World::new(Arch::Aarch64);
    // mprotect: read-only, then W+X (a write-or-execute page, writing,
    // since `node-jit`), unmapped range refused.
    assert_eq!(w.call(226, &[DATA, 4096, 1]).0, 0);
    assert!(!w.pt().translate(&w.mem, DATA).unwrap().flags.is_writable());
    assert_eq!(w.call(226, &[DATA, 4096, 7]).0, 0);
    let flags = w.pt().translate(&w.mem, DATA).unwrap().flags;
    assert_eq!(flags, space::WX_WRITING);
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
    // statx (`add-dynamic-linking`): the same, in its own layout.
    assert_eq!(w.call(291, &[AT_FDCWD, full, 0, 0x7ff, DATA]).0, 0);
    assert_eq!(w.peek(DATA + 28, 2), (0o100_644u16).to_le_bytes());
    assert_eq!(w.peek(DATA + 40, 8), 5u64.to_le_bytes());
    // faccessat: exists, but cannot be executed.
    assert_eq!(w.call(48, &[AT_FDCWD, full, 4]).0, 0);
    assert_eq!(w.call(48, &[AT_FDCWD, full, 1]).0, -EACCES);
    let missing = name(&mut w, "/work/none");
    assert_eq!(w.call(291, &[AT_FDCWD, missing, 0, 0x7ff, DATA]).0, -2);
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
fn ioctl_sets_non_blocking_and_close_on_exec() {
    const IOCTL: u64 = 16;
    const FCNTL: u64 = 72;
    let mut w = World::new(Arch::X86_64);
    let p = name(&mut w, "/x");
    let fd = w
        .call(2 /* open */, &[p, 0x42 /* O_CREAT|O_RDWR */, 0o644])
        .0 as u64;
    w.poke(DATA, &1u32.to_le_bytes());
    assert_eq!(w.call(IOCTL, &[fd, 0x5421 /* FIONBIO */, DATA]).0, 0);
    assert_eq!(w.call(FCNTL, &[fd, 3 /* F_GETFL */]).0, 0x802, "O_NONBLOCK");
    w.poke(DATA, &0u32.to_le_bytes());
    assert_eq!(w.call(IOCTL, &[fd, 0x5421, DATA]).0, 0);
    assert_eq!(w.call(FCNTL, &[fd, 3]).0, 2, "blocking again");
    assert_eq!(w.call(IOCTL, &[fd, 0x5421, 0xdead_0000]).0, -EFAULT);
    assert_eq!(w.call(IOCTL, &[fd, 0x5451 /* FIOCLEX */, 0]).0, 0);
    assert_eq!(w.call(FCNTL, &[fd, 1 /* F_GETFD */]).0, 1);
    assert_eq!(w.call(IOCTL, &[fd, 0x5450 /* FIONCLEX */, 0]).0, 0);
    assert_eq!(w.call(FCNTL, &[fd, 1]).0, 0);
    assert_eq!(w.call(IOCTL, &[77, 0x5451, 0]).0, -EBADF);
    assert_eq!(w.call(IOCTL, &[77, 0x5421, DATA]).0, -EBADF);
    assert_eq!(w.call(IOCTL, &[fd, 0x5401 /* TCGETS */, DATA]).0, -ENOTTY);
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

#[test]
fn file_writes_stop_when_the_kernel_refuses_memory() {
    let mut w = World::new(Arch::Aarch64);
    let p = name(&mut w, "/f");
    let fd = w.call(OPENAT, &[AT_FDCWD, p, 0x42]).0 as u64;
    w.services.room = Some(4096);
    w.poke(DATA, &[1; 4000]);
    assert_eq!(w.call(WRITE, &[fd, DATA, 4000]).0, 4000);
    assert_eq!(w.call(WRITE, &[fd, DATA, 4000]).0, -28, "ENOSPC");
    w.services.room = Some(100);
    assert_eq!(w.call(WRITE, &[fd, DATA, 4000]).0, -28);
    assert_eq!(w.call(80, &[fd, DATA + 0x800]).0, 0);
    assert_eq!(
        w.peek(DATA + 0x800 + 48, 8),
        4000u64.to_le_bytes(),
        "nothing more written"
    );
}

/// Design D4 of `add-cell-hierarchy`: file blocks and names, sockets and
/// thread records count against the cell's memory while it holds them.
#[test]
fn files_sockets_and_threads_are_metered() {
    let mut w = World::new(Arch::Aarch64);
    let block = rhizo_fs::BLOCK as u64;
    w.frames.kernel_limit = 2 * block;
    let p = name(&mut w, "/f");
    let fd = w.call(OPENAT, &[AT_FDCWD, p, 0x42]).0 as u64;
    let named = rhizo_fs::NAME_COST + 1;
    assert_eq!(w.frames.kernel, named, "only the name is counted");
    w.poke(DATA, &[1; 4000]);
    assert_eq!(w.call(WRITE, &[fd, DATA, 4000]).0, 4000);
    assert_eq!(w.frames.kernel, named + block);
    assert_eq!(w.call(WRITE, &[fd, DATA, 96]).0, 96, "same block");
    assert_eq!(w.call(WRITE, &[fd, DATA, 4000]).0, -28, "ENOSPC");
    assert_eq!(w.frames.kernel, named + block, "a refusal counts nothing");
    assert_eq!(w.call(CLOSE, &[fd]).0, 0);
    let p = name(&mut w, "/f");
    assert_eq!(w.call(35 /* unlinkat */, &[AT_FDCWD, p, 0]).0, 0);
    assert_eq!(w.frames.kernel, 0, "everything given back");
    assert_eq!(w.call(198 /* socket */, &[2, 1, 0]).0, -ENOMEM, "too big");
    w.frames.kernel_limit = u64::MAX;
    let s = w.call(198, &[2, 1, 0]).0 as u64;
    assert_eq!(w.frames.kernel, rhizo_net::SOCKET_BYTES);
    assert_eq!(w.call(CLOSE, &[s]).0, 0);
    assert_eq!(w.frames.kernel, 0);
    assert!(w.call(CLONE, &[PTHREAD_FLAGS, 0, 0, 0, 0]).0 > 0);
    assert_eq!(
        w.frames.kernel,
        size_of::<rhizo_cell::threads::Thread>() as u64
    );
}

#[test]
fn select_without_descriptors_sleeps() {
    let mut w = World::new(Arch::X86_64);
    w.services.clock = 500;
    let tv = [0i64.to_le_bytes(), 1000i64.to_le_bytes()].concat(); // 1 ms
    w.poke(DATA, &tv);
    assert_eq!(
        w.call(23, &[0, 0, 0, 0, DATA]),
        (0, Outcome::Block),
        "select"
    );
    assert_eq!(w.slept(), Some(1_000_500), "select");
    let ts = [0i64.to_le_bytes(), 2000i64.to_le_bytes()].concat(); // 2 us
    w.poke(DATA, &ts);
    assert_eq!(
        w.call(270, &[0, 0, 0, 0, DATA, 0]),
        (0, Outcome::Block),
        "pselect6"
    );
    assert_eq!(w.slept(), Some(2_500), "pselect6");
    // With descriptors it waits on them (spec change `add-sockets-and-poll`):
    // standard input reads as empty, so it is ready at once.
    w.poke(DATA + 16, &1u64.to_le_bytes());
    assert_eq!(
        w.call(23, &[1, DATA + 16, 0, 0, DATA]),
        (1, Outcome::Resume),
        "descriptors"
    );
    assert_eq!(w.call(23, &[0, 0, 0, 0, 0]).0, -EINVAL, "no timeout");
    w.poke(
        DATA,
        &[0i64.to_le_bytes(), 1_000_000i64.to_le_bytes()].concat(),
    );
    assert_eq!(
        w.call(23, &[0, 0, 0, 0, DATA]).0,
        -EINVAL,
        "microseconds out of range"
    );
    let mut a = World::new(Arch::Aarch64);
    a.poke(DATA, &ts);
    assert_eq!(
        a.call(72, &[0, 0, 0, 0, DATA, 0]),
        (0, Outcome::Block),
        "aarch64 pselect6"
    );
    assert_eq!(a.slept(), Some(2_000), "aarch64 pselect6");
}

// --- Threads (spec `threads`) ----------------------------------------------

// AArch64 numbers.
const CLONE: u64 = 220;
const CLONE3: u64 = 435;
const EXIT: u64 = 93;
const FUTEX: u64 = 98;
const GETTID: u64 = 178;
const GETPID: u64 = 172;
const SET_TID_ADDRESS: u64 = 96;
const MPROTECT: u64 = 226;
/// The flags glibc's `pthread_create` passes.
const PTHREAD_FLAGS: u64 =
    0x100 | 0x200 | 0x400 | 0x800 | 0x1_0000 | 0x4_0000 | 0x8_0000 | 0x10_0000 | 0x20_0000;
const CLONE_CHILD_SETTID: u64 = 0x100_0000;
const FUTEX_WAIT: u64 = 0;
const FUTEX_WAKE: u64 = 1;
const FUTEX_WAIT_BITSET: u64 = 9;
const FUTEX_WAKE_BITSET: u64 = 10;
const PRIVATE: u64 = 128;
const CLOCK_REALTIME: u64 = 256;
const EAGAIN_: i64 = 11;
const ETIMEDOUT_: i64 = 110;

impl World {
    /// Makes thread `tid` the current one, as the kernel's scheduling would.
    fn switch_to(&mut self, tid: u32) {
        for _ in 0..self.cell.threads.len() {
            if self.cell.threads.current().tid == tid {
                return;
            }
            assert!(
                self.cell.threads.next(self.services.clock),
                "nothing can run"
            );
        }
        assert_eq!(
            self.cell.threads.current().tid,
            tid,
            "thread {tid} cannot run"
        );
    }

    fn thread(&mut self, tid: u32) -> &rhizo_cell::threads::Thread {
        self.cell.threads.get_mut(tid).expect("thread exists")
    }
}

#[test]
fn clone_creates_a_thread_like_pthread_create() {
    let mut w = World::new(Arch::Aarch64);
    w.cell.regs_mut().pc = 0x40_1234;
    w.cell.regs_mut().gpr[19] = 0x1919;
    w.cell.regs_mut().fp.0[5] = 55;
    w.services.cpu_fp = Some(77); // the caller's FP state is in the CPU
    let (ptid, ctid) = (DATA + 0x100, DATA + 0x104);
    // AArch64: clone(flags, stack, parent_tid, tls, child_tid)
    let (tid, out) = w.call(
        CLONE,
        &[
            PTHREAD_FLAGS | CLONE_CHILD_SETTID,
            0x60_0000,
            ptid,
            0x7777,
            ctid,
        ],
    );
    assert_eq!(
        (tid, out),
        (6, Outcome::Resume),
        "the caller gets the new id"
    );
    assert_eq!(w.cell.threads.len(), 2);
    assert_eq!(w.peek(ptid, 4), 6u32.to_le_bytes(), "CLONE_PARENT_SETTID");
    assert_eq!(w.peek(ctid, 4), 6u32.to_le_bytes(), "CLONE_CHILD_SETTID");
    let child = w.thread(6).clone();
    assert_eq!(child.regs.gpr[0], 0, "the child sees 0");
    assert_eq!((child.regs.sp, child.regs.tls), (0x60_0000, 0x7777));
    assert_eq!((child.regs.pc, child.regs.gpr[19]), (0x40_1234, 0x1919));
    assert_eq!(
        (child.regs.fp.0[0], child.regs.fp.0[5]),
        (77, 55),
        "FP copied"
    );
    assert_eq!(child.clear_tid, ctid, "CLONE_CHILD_CLEARTID");
    assert_eq!(w.cell.regs().tls, 0, "the caller keeps its thread pointer");

    // x86_64: clone(flags, stack, parent_tid, child_tid, tls)
    let mut x = World::new(Arch::X86_64);
    assert_eq!(
        x.call(56, &[PTHREAD_FLAGS, 0x60_0000, ptid, ctid, 0x8888])
            .0,
        6
    );
    let child = x.thread(6).clone();
    assert_eq!((child.regs.tls, child.clear_tid), (0x8888, ctid));
}

#[test]
fn clones_that_are_not_threads_are_refused() {
    let mut w = World::new(Arch::Aarch64);
    const SIGCHLD: u64 = 17;
    // glibc's fork(): a new process.
    assert_eq!(
        w.call(CLONE, &[0x120_0000 | SIGCHLD, 0, 0, 0, 0]).0,
        -ENOSYS
    );
    assert_eq!(
        w.call(CLONE, &[0x100 | 0x800, 0, 0, 0, 0]).0,
        -ENOSYS,
        "no CLONE_THREAD"
    );
    assert_eq!(
        w.call(CLONE, &[PTHREAD_FLAGS | 0x2000_0000, 0, 0, 0, 0]).0,
        -EINVAL,
        "CLONE_NEWUTS"
    );
    // A kernel-half or non-canonical thread pointer would fault in the
    // kernel when loaded into the FS base.
    for tls in [0xffff_8000_0000_0000, 0x0000_8000_0000_0000, u64::MAX] {
        assert_eq!(
            w.call(CLONE, &[PTHREAD_FLAGS, 0, 0, tls, 0]).0,
            -EPERM,
            "tls {tls:#x}"
        );
    }
    assert_eq!(w.cell.threads.len(), 1);
    assert_eq!(w.services.log, "", "refusals are not missing calls");
    w.services.room = Some(100);
    assert_eq!(
        w.call(CLONE, &[PTHREAD_FLAGS, 0, 0, 0, 0]).0,
        -EAGAIN_,
        "no room"
    );
    assert_eq!(w.cell.threads.len(), 1);
}

#[test]
fn clone3_reads_its_argument_block() {
    let mut w = World::new(Arch::Aarch64);
    let args = |flags: u64, stack: u64, size: u64, tls: u64, set_tid: u64| {
        // flags, pidfd, child_tid, parent_tid, exit_signal, stack,
        // stack_size, tls, set_tid (glibc fills pidfd without CLONE_PIDFD)
        [
            flags,
            DATA + 0x200,
            DATA + 0x200,
            DATA + 0x204,
            0,
            stack,
            size,
            tls,
            set_tid,
        ]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect::<Vec<u8>>()
    };
    w.poke(DATA, &args(PTHREAD_FLAGS, 0x50_0000, 0x1_0000, 0x99, 0));
    assert_eq!(w.call(CLONE3, &[DATA, 64]).0, 6);
    let child = w.thread(6).clone();
    assert_eq!(child.regs.sp, 0x51_0000, "stack plus its size");
    assert_eq!((child.regs.tls, child.clear_tid), (0x99, DATA + 0x200));
    assert_eq!(w.peek(DATA + 0x204, 4), 6u32.to_le_bytes(), "parent_tid");

    assert_eq!(
        w.call(CLONE3, &[DATA, 63]).0,
        -EINVAL,
        "smaller than version 0"
    );
    assert_eq!(w.call(CLONE3, &[0xdead_0000, 64]).0, -EFAULT);
    w.poke(DATA, &args(PTHREAD_FLAGS, 0x50_0000, 0x1_0000, 0, 1));
    assert_eq!(
        w.call(CLONE3, &[DATA, 72]).0,
        -EINVAL,
        "set_tid is not supported"
    );
    assert_eq!(
        w.call(CLONE3, &[DATA, 64]).0,
        7,
        "ignored beyond the size given"
    );
    w.poke(DATA, &args(PTHREAD_FLAGS | 17, 0, 0, 0, 0));
    assert_eq!(
        w.call(CLONE3, &[DATA, 64]).0,
        -EINVAL,
        "a signal in the flags"
    );
    w.poke(DATA, &args(0x100_0000, 0, 0, 0, 0));
    assert_eq!(w.call(CLONE3, &[DATA, 64]).0, -ENOSYS, "a process");
    assert_eq!(w.cell.threads.len(), 3);
}

#[test]
fn identifiers_per_thread() {
    let mut w = World::new(Arch::Aarch64);
    assert_eq!(
        w.call(GETTID, &[]).0,
        5,
        "the first thread is the cell's id"
    );
    assert_eq!(w.call(CLONE, &[PTHREAD_FLAGS, 0, 0, 0, 0]).0, 6);
    w.switch_to(6);
    assert_eq!(w.call(GETTID, &[]).0, 6);
    assert_eq!(w.call(GETPID, &[]).0, 5, "one process id for all threads");
    assert_eq!(w.call(SET_TID_ADDRESS, &[DATA + 8]).0, 6);
    assert_eq!(w.thread(6).clear_tid, DATA + 8);
    assert_eq!(w.thread(5).clear_tid, 0, "per thread");
}

#[test]
fn thread_exit_clears_its_tid_and_wakes_the_joiner() {
    let mut w = World::new(Arch::Aarch64);
    let tid_word = DATA + 0x40;
    w.poke(tid_word, &6u32.to_le_bytes());
    assert_eq!(w.call(CLONE, &[PTHREAD_FLAGS, 0, 0, 0, tid_word]).0, 6);
    // pthread_join: wait while the word holds the thread's id.
    assert_eq!(
        w.call(
            FUTEX,
            &[
                tid_word,
                FUTEX_WAIT_BITSET | CLOCK_REALTIME,
                6,
                0,
                0,
                u32::MAX as u64
            ]
        )
        .1,
        Outcome::Block
    );
    assert!(w.cell.threads.next(0));
    assert_eq!(w.cell.threads.current().tid, 6);
    assert_eq!(w.call(EXIT, &[9]).1, Outcome::Block, "only the thread ends");
    assert_eq!(w.cell.threads.len(), 1);
    assert_eq!(w.peek(tid_word, 4), [0; 4], "the tid word is cleared");
    assert!(w.cell.threads.next(0), "the joiner was woken");
    assert_eq!(w.cell.regs().gpr[0], 0, "its wait returns 0");
    assert_eq!(w.call(EXIT, &[4]).1, Outcome::Exit(4), "the last thread");
    assert_eq!(w.cell.exit_status, Some(4));

    let mut w = World::new(Arch::Aarch64);
    w.call(CLONE, &[PTHREAD_FLAGS, 0, 0, 0, 0]);
    assert_eq!(w.call(EXIT_GROUP, &[2]).1, Outcome::Exit(2), "every thread");
}

#[test]
fn a_pipe_write_wakes_the_threads_of_the_cell_waiting_on_descriptors() {
    const PIPE2: u64 = 59;
    const READ: u64 = 63;
    let mut w = World::new(Arch::Aarch64);
    let fds = DATA + 0x200;
    assert_eq!(w.call(PIPE2, &[fds, 0]).0, 0);
    let fd = |at: u64| u64::from(u32::from_le_bytes(w.peek(at, 4).try_into().unwrap()));
    let (r, wr) = (fd(fds), fd(fds + 4));
    let main = w.cell.threads.current().tid;
    assert_eq!(w.call(CLONE, &[PTHREAD_FLAGS, 0, 0, 0, 0]).0, 6);
    w.switch_to(6);
    assert_eq!(w.call(READ, &[r, DATA, 1]).1, Outcome::Block);
    // The world made it runnable again: it waits, as the kernel keeps it.
    w.cell.threads.current_mut().wait = Wait::Io { until: None };
    w.switch_to(main);
    w.poke(DATA + 0x300, b"x");
    assert_eq!(w.call(WRITE, &[1, DATA + 0x300, 1]).0, 1);
    assert_ne!(w.thread(6).wait, Wait::Ready, "the console is no pipe");
    assert_eq!(w.call(WRITE, &[wr, DATA + 0x300, 1]).0, 1);
    assert_eq!(w.thread(6).wait, Wait::Ready, "it reads again");
}

#[test]
fn futex_waits_block_and_wakes_wake() {
    let mut w = World::new(Arch::Aarch64);
    let word = DATA + 0x80;
    w.poke(word, &1u32.to_le_bytes());
    assert_eq!(w.call(CLONE, &[PTHREAD_FLAGS, 0, 0, 0, 0]).0, 6);
    assert_eq!(
        w.call(FUTEX, &[word, FUTEX_WAIT | PRIVATE, 0, 0]).0,
        -EAGAIN_,
        "changed"
    );
    assert_eq!(
        w.call(FUTEX, &[word, FUTEX_WAIT | PRIVATE, 1, 0]).1,
        Outcome::Block
    );
    assert_eq!(
        w.cell.threads.current().wait,
        Wait::Futex {
            addr: word,
            bitset: u32::MAX,
            until: None
        }
    );
    assert!(w.cell.threads.next(0));
    assert_eq!(w.cell.threads.current().tid, 6);
    assert_eq!(
        w.call(FUTEX, &[word + 4, FUTEX_WAKE, 1]).0,
        0,
        "another word"
    );
    assert_eq!(w.call(FUTEX, &[word, FUTEX_WAKE | PRIVATE, 1]).0, 1);
    assert_eq!(w.thread(5).wait, Wait::Ready);
    assert_eq!(w.thread(5).regs.gpr[0], 0, "the wait returns 0");
    assert_eq!(w.call(FUTEX, &[word, FUTEX_WAKE, 1]).0, 0, "nobody left");

    // Bitsets: only intersecting waiters wake.
    let mut w = World::new(Arch::Aarch64);
    w.poke(word, &0u32.to_le_bytes());
    for _ in 0..2 {
        w.call(CLONE, &[PTHREAD_FLAGS, 0, 0, 0, 0]);
    }
    for (tid, bits) in [(6, 1), (7, 2)] {
        w.switch_to(tid);
        assert_eq!(
            w.call(FUTEX, &[word, FUTEX_WAIT_BITSET, 0, 0, 0, bits]).1,
            Outcome::Block
        );
    }
    w.switch_to(5);
    assert_eq!(
        w.call(FUTEX, &[word, FUTEX_WAKE_BITSET, i32::MAX as u64, 0, 0, 2])
            .0,
        1
    );
    assert_eq!(w.thread(7).wait, Wait::Ready);
    assert_ne!(w.thread(6).wait, Wait::Ready);
    assert_eq!(
        w.call(FUTEX, &[word, FUTEX_WAKE, 0]).0,
        1,
        "asked for none, wakes one"
    );
}

#[test]
fn futex_timeouts_and_errors() {
    let mut w = World::new(Arch::Aarch64);
    w.services.clock = 1_000;
    let word = DATA + 0x80;
    let ts = DATA + 0x90;
    w.poke(word, &3u32.to_le_bytes());
    w.poke(
        ts,
        &[0i64.to_le_bytes(), 20_000_000i64.to_le_bytes()].concat(),
    );
    // FUTEX_WAIT: relative.
    assert_eq!(w.call(FUTEX, &[word, FUTEX_WAIT, 3, ts]).1, Outcome::Block);
    let until = |w: &World| match w.cell.threads.current().wait {
        Wait::Futex { until, .. } => until,
        _ => None,
    };
    assert_eq!(until(&w), Some(20_001_000));
    assert!(!w.cell.threads.next(20_000_999), "not yet");
    assert!(w.cell.threads.next(20_001_000));
    assert_eq!(w.cell.regs().gpr[0] as i64, -ETIMEDOUT_);
    // FUTEX_WAIT_BITSET: absolute, monotonic or real time.
    w.poke(ts, &[1i64.to_le_bytes(), 0i64.to_le_bytes()].concat());
    w.call(FUTEX, &[word, FUTEX_WAIT_BITSET, 3, ts, 0, 1]);
    assert_eq!(until(&w), Some(1_000_000_000));
    w.cell.threads.current_mut().wait = Wait::Ready;
    let real = 1_700_000_000_000_000_000i64 + 1_000 + 7; // 7 ns from now
    w.poke(
        ts,
        &[
            (real / 1_000_000_000).to_le_bytes(),
            (real % 1_000_000_000).to_le_bytes(),
        ]
        .concat(),
    );
    w.call(
        FUTEX,
        &[word, FUTEX_WAIT_BITSET | CLOCK_REALTIME, 3, ts, 0, 1],
    );
    assert_eq!(until(&w), Some(1_007));
    w.cell.threads.current_mut().wait = Wait::Ready;
    w.poke(ts, &[0i64.to_le_bytes(), 999i64.to_le_bytes()].concat());
    assert_eq!(
        w.call(FUTEX, &[word, FUTEX_WAIT_BITSET, 3, ts, 0, 1]).0,
        -ETIMEDOUT_,
        "a deadline already passed"
    );
    assert_eq!(
        w.call(FUTEX, &[word, FUTEX_WAIT_BITSET, 4, ts, 0, 1]).0,
        -EAGAIN_,
        "value first"
    );

    for (args, err, what) in [
        ([word + 2, FUTEX_WAIT, 3, 0, 0, 0], -EINVAL, "misaligned"),
        (
            [word + 2, FUTEX_WAKE, 1, 0, 0, 0],
            -EINVAL,
            "misaligned wake",
        ),
        (
            [word, FUTEX_WAIT_BITSET, 3, 0, 0, 0],
            -EINVAL,
            "empty bitset",
        ),
        (
            [word, FUTEX_WAKE_BITSET, 1, 0, 0, 0],
            -EINVAL,
            "empty wake bitset",
        ),
        (
            [0xdead_0000, FUTEX_WAIT, 3, 0, 0, 0],
            -EFAULT,
            "unmapped word",
        ),
        (
            [word, FUTEX_WAIT, 3, 0xdead_0000, 0, 0],
            -EFAULT,
            "unmapped timeout",
        ),
        (
            [word, FUTEX_WAIT | CLOCK_REALTIME, 3, 0, 0, 0],
            -ENOSYS,
            "real time on WAIT",
        ),
        ([word, 3, 1, 0, 0, 0], -ENOSYS, "FUTEX_REQUEUE"),
        ([word, 5, 1, 0, 0, 0], -ENOSYS, "FUTEX_WAKE_OP"),
    ] {
        assert_eq!(w.call(FUTEX, &args).0, err, "{what}");
    }
    w.poke(
        ts,
        &[0i64.to_le_bytes(), 1_000_000_000i64.to_le_bytes()].concat(),
    );
    assert_eq!(
        w.call(FUTEX, &[word, FUTEX_WAIT, 3, ts]).0,
        -EINVAL,
        "bad timeout"
    );
    assert_eq!(w.cell.threads.current().wait, Wait::Ready, "never blocked");
}

#[test]
fn reserved_mappings_are_backed_by_mprotect() {
    let mut w = World::new(Arch::Aarch64);
    // A thread stack as glibc makes it: reserve, then open all but the
    // guard page at the bottom.
    let base = w.call(MMAP, &[0, 1 << 20, 0, 0x22]).0 as u64;
    assert_eq!(base, MMAP_BASE);
    assert!(
        w.pt().translate(&w.mem, base).is_none(),
        "reserved, not backed"
    );
    assert_eq!(w.call(MPROTECT, &[base + 4096, (1 << 20) - 4096, 3]).0, 0);
    assert!(
        w.pt().translate(&w.mem, base).is_none(),
        "the guard page stays"
    );
    let top = base + (1 << 20) - 8;
    assert_eq!(w.peek(top, 8), [0; 8]);
    w.poke(top, b"stacked!");
    assert_eq!(w.peek(top, 8), *b"stacked!");
    let t = w.pt().translate(&w.mem, base + 4096).unwrap();
    assert!(t.flags.is_writable() && !t.flags.is_executable());
    // Opening it again changes nothing; beyond the cursor is not reserved.
    assert_eq!(w.call(MPROTECT, &[base + 4096, 4096, 3]).0, 0);
    assert_eq!(w.call(MPROTECT, &[base + (1 << 20), 4096, 3]).0, -ENOMEM);

    // A large reservation opens with 2 MiB pages where it can.
    let big = w.call(MMAP, &[0, 8 << 20, 0, 0x22]).0 as u64;
    assert_eq!(w.call(MPROTECT, &[big + 4096, (8 << 20) - 4096, 3]).0, 0);
    let size = |w: &World, at: u64| w.pt().translate(&w.mem, at).unwrap().size;
    assert_eq!(size(&w, big + 4096), PageSize::Small);
    assert_eq!(size(&w, big + (2 << 20)), PageSize::Large);
    let before = w.frames.outstanding();
    assert_eq!(w.call(MUNMAP, &[big, 8 << 20]).0, 0);
    assert!(w.frames.outstanding() < before, "frames returned");
}

#[test]
fn time_returns_and_stores_epoch_seconds() {
    let mut w = World::new(Arch::X86_64);
    w.services.clock = 2_500_000_000;
    let expected = 1_700_000_002;
    assert_eq!(w.call(201, &[0]), (expected, Outcome::Resume));
    assert_eq!(w.call(201, &[DATA]), (expected, Outcome::Resume));
    assert_eq!(w.peek(DATA, 8), (expected as u64).to_le_bytes());
    assert_eq!(w.call(201, &[RO]).0, -EFAULT);
}

#[test]
fn time_does_not_exist_on_aarch64() {
    let mut w = World::new(Arch::Aarch64);
    // 201 is `listen` there: standard input is no socket.
    assert_eq!(
        w.call(201, &[0]).0,
        -errno::ENOTSOCK,
        "201 is not `time` on AArch64"
    );
}

/// Writable and executable anonymous memory (spec `linux-abi`, design D5 of
/// `node-jit`): a range without memory, whose pages a system call backs on
/// first touch, writing; `mprotect` turns reserved addresses into ranges
/// and mapped pages into write-or-execute pages; other protections,
/// `munmap` and `MAP_FIXED` take their part out; file mappings still refuse
/// both rights.
#[test]
fn wx_memory_comes_on_first_touch() {
    let mut w = World::new(Arch::Aarch64);
    let anon = MAP_ANONYMOUS | MAP_PRIVATE;
    let before = w.frames.outstanding();
    // A JIT's code range: reserved, then made writable and executable.
    let code = w.call(MMAP, &[0, 512 << 20, 0, anon, u64::MAX, 0]).0 as u64;
    assert_eq!(w.call(226, &[code, 512 << 20, 7]).0, 0);
    assert_eq!(w.frames.outstanding(), before, "no memory yet");
    assert_eq!(w.pager.wx.ranges(), &[(code, code + (512 << 20))]);
    // A system call writing there gets a zeroed page, writing.
    w.pager.backing = true;
    let at = code + (100 << 20) + 8;
    assert_eq!(w.call(261, &[0, 3, 0, at]).0, 0, "prlimit64 into it");
    assert_eq!(w.peek(at, 8), (8u64 << 20).to_le_bytes());
    assert_eq!(w.peek(at - 8, 8), [0; 8]);
    let t = w.pt().translate(&w.mem, at).unwrap();
    assert_eq!((t.size, t.flags), (PageSize::Small, space::WX_WRITING));
    assert!(
        w.pt().translate(&w.mem, at + FRAME_SIZE).is_none(),
        "one page"
    );
    // And a read from a pipe (pieces copied by the pipe's own path).
    assert_eq!(w.call(59, &[DATA, 0]).0, 0, "pipe2");
    let fds = w.peek(DATA, 8);
    let (rd, wr) = (u64::from(fds[0]), u64::from(fds[4]));
    w.poke(DATA + 64, b"jit!");
    assert_eq!(w.call(64, &[wr, DATA + 64, 4]).0, 4);
    let piped = code + (150 << 20) + 100;
    assert_eq!(w.call(63, &[rd, piped, 4]).0, 4, "read into it");
    assert_eq!(w.peek(piped, 4), b"jit!");
    // Another protection takes its part out (and backs it, as reserved
    // addresses are); so does munmap.
    let ro = code + (200 << 20);
    assert_eq!(w.call(226, &[ro, FRAME_SIZE, 1]).0, 0);
    let t = w.pt().translate(&w.mem, ro).unwrap();
    assert!(!t.flags.is_writable() && !t.flags.contains(PageFlags::WRITE_OR_EXEC));
    assert_eq!(w.call(MUNMAP, &[code, 1 << 20]).0, 0);
    let end = code + (512 << 20);
    assert_eq!(
        w.pager.wx.ranges(),
        &[(code + (1 << 20), ro), (ro + FRAME_SIZE, end)]
    );
    // A mapped page made writable and executable keeps its frame.
    let frame = w.pt().translate(&w.mem, DATA).unwrap().phys;
    w.poke(DATA, b"code");
    assert_eq!(w.call(226, &[DATA, FRAME_SIZE, 7]).0, 0);
    let t = w.pt().translate(&w.mem, DATA).unwrap();
    assert_eq!((t.phys, t.flags), (frame, space::WX_WRITING));
    assert_eq!(w.peek(DATA, 4), b"code");
    // Both rights over a hole that is not reserved, or over part of a
    // 2 MiB page: refused, nothing changed.
    let big = w.call(MMAP, &[0, 2 << 20, 3, anon, u64::MAX, 0]).0 as u64;
    let ranges = w.pager.wx.clone();
    assert_eq!(w.call(226, &[0x7000_0000, FRAME_SIZE, 7]).0, -ENOMEM);
    assert_eq!(w.call(226, &[big, FRAME_SIZE, 7]).0, -EINVAL);
    assert_eq!(w.pager.wx, ranges);
    // MAP_FIXED over a range replaces it.
    let fixed = code + (300 << 20);
    let got = w.call(MMAP, &[fixed, FRAME_SIZE, 3, anon | MAP_FIXED, u64::MAX, 0]);
    assert_eq!(got.0 as u64, fixed);
    assert!(!w.pager.wx.contains(fixed) && w.pager.wx.contains(fixed + FRAME_SIZE));
    // Files: still never both.
    w.fs.put_file(b"/f", &[1; 4096]).unwrap();
    let read = rhizo_fs::OpenFlags {
        read: true,
        ..Default::default()
    };
    let fd = u64::from(w.fs.open(None, b"/f", read, 0).unwrap());
    assert_eq!(w.call(MMAP, &[0, 4096, 7, MAP_PRIVATE, fd, 0]).0, -EACCES);
}

/// At most 64 write-or-execute ranges (spec `linux-abi`): the call that
/// would need one more, a new range or a split, fails with `ENOMEM` and
/// changes nothing.
#[test]
fn wx_ranges_are_bounded() {
    let mut w = World::new(Arch::Aarch64);
    let anon = MAP_ANONYMOUS | MAP_PRIVATE;
    let mut first = 0;
    for i in 0..space::MAX_WX_RANGES {
        let at = w.call(MMAP, &[0, 3 * FRAME_SIZE, 7, anon, u64::MAX, 0]).0 as u64;
        if i == 0 {
            first = at;
        }
        // A reserved gap keeps the next range apart.
        assert!(w.call(MMAP, &[0, FRAME_SIZE, 0, anon, u64::MAX, 0]).0 > 0);
    }
    let full = w.pager.wx.clone();
    assert_eq!(full.ranges().len(), space::MAX_WX_RANGES);
    let more = w.call(MMAP, &[0, FRAME_SIZE, 7, anon, u64::MAX, 0]).0;
    assert_eq!(more, -ENOMEM);
    let middle = first + FRAME_SIZE;
    assert_eq!(w.call(MUNMAP, &[middle, FRAME_SIZE]).0, -ENOMEM, "a split");
    assert_eq!(w.call(226, &[middle, FRAME_SIZE, 1]).0, -ENOMEM, "a split");
    assert_eq!(w.pager.wx, full);
    assert_eq!(w.call(MUNMAP, &[first, FRAME_SIZE]).0, 0, "a trim");
    assert_eq!(w.pager.wx.ranges().len(), space::MAX_WX_RANGES);
    assert_eq!(w.pager.wx.ranges()[0], (middle, first + 3 * FRAME_SIZE));
}

const MADVISE: u64 = 233;
const MADV_WIPEONFORK: u64 = 18;
const MADV_KEEPONFORK: u64 = 19;

/// `MADV_WIPEONFORK` marks mapped memory and `MADV_KEEPONFORK` unmarks it
/// (design D22 of `add-cell-hierarchy`); `munmap` and `MAP_FIXED` take
/// their part out, `mprotect` keeps it. Addresses not mapped are refused
/// with `ENOMEM`, an unaligned start or part of a 2 MiB page with
/// `EINVAL`, and other advice changes nothing.
#[test]
fn wipeonfork_marks_and_keeponfork_unmarks() {
    let mut w = World::new(Arch::Aarch64);
    let anon = MAP_ANONYMOUS | MAP_PRIVATE;
    let at = w.call(MMAP, &[0, 4 * FRAME_SIZE, 3, anon, u64::MAX, 0]).0 as u64;
    assert_eq!(w.call(MADVISE, &[at, 4 * FRAME_SIZE, 4]).0, 0, "DONTNEED");
    assert_eq!(w.pager.wx.wiped(), &[]);
    assert_eq!(w.call(MADVISE, &[at, 100, MADV_WIPEONFORK]).0, 0);
    assert_eq!(w.pager.wx.wiped(), &[(at, at + FRAME_SIZE)], "whole pages");
    let end = at + 4 * FRAME_SIZE;
    assert_eq!(w.call(MADVISE, &[at, 4 * FRAME_SIZE, MADV_WIPEONFORK]).0, 0);
    assert_eq!(w.pager.wx.wiped(), &[(at, end)]);
    let second = at + FRAME_SIZE;
    assert_eq!(w.call(MADVISE, &[second, FRAME_SIZE, MADV_KEEPONFORK]).0, 0);
    assert_eq!(
        w.pager.wx.wiped(),
        &[(at, second), (second + FRAME_SIZE, end)]
    );
    assert_eq!(w.call(226, &[at, FRAME_SIZE, 1]).0, 0, "mprotect keeps it");
    assert_eq!(w.pager.wx.wiped()[0], (at, second));
    assert_eq!(w.call(MUNMAP, &[end - FRAME_SIZE, FRAME_SIZE]).0, 0);
    let fixed = at + 2 * FRAME_SIZE;
    let got = w.call(MMAP, &[fixed, FRAME_SIZE, 3, anon | MAP_FIXED, u64::MAX, 0]);
    assert_eq!(got.0 as u64, fixed);
    assert_eq!(w.pager.wx.wiped(), &[(at, second)]);
    assert!(w.pager.wx.ranges().is_empty());
    // Refused, nothing changed.
    let marks = w.pager.wx.clone();
    let wipe = MADV_WIPEONFORK;
    assert_eq!(w.call(MADVISE, &[end - FRAME_SIZE, 10, wipe]).0, -ENOMEM);
    assert_eq!(w.call(MADVISE, &[at + 8, 10, wipe]).0, -EINVAL);
    let big = w.call(MMAP, &[0, 2 << 20, 3, anon, u64::MAX, 0]).0 as u64;
    assert_eq!(w.call(MADVISE, &[big, FRAME_SIZE, wipe]).0, -EINVAL);
    assert_eq!(w.pager.wx, marks);
    assert_eq!(w.call(MADVISE, &[big, 2 << 20, wipe]).0, 0, "a whole page");
    assert_eq!(w.call(MADVISE, &[at, end - at, MADV_KEEPONFORK]).0, -ENOMEM);
    assert_eq!(w.call(MADVISE, &[at, second - at, MADV_KEEPONFORK]).0, 0);
    assert_eq!(w.pager.wx.wiped(), &[(big, big + (2 << 20))]);
}

/// A copy of a cell (here its fork) reads zero in its wipe ranges, with
/// each page's permissions, while the source keeps its contents and pages
/// outside the ranges stay shared (design D22 of `add-cell-hierarchy`).
#[test]
fn wipeonfork_pages_read_zero_in_a_copy() {
    let mut w = World::new(Arch::Aarch64);
    w.poke(DATA, &1u64.to_le_bytes());
    assert_eq!(w.call(MADVISE, &[DATA, FRAME_SIZE, MADV_WIPEONFORK]).0, 0);
    assert_eq!(w.call(MADVISE, &[RO, FRAME_SIZE, MADV_WIPEONFORK]).0, 0);
    let anon = MAP_ANONYMOUS | MAP_PRIVATE;
    let kept = w.call(MMAP, &[0, FRAME_SIZE, 3, anon, u64::MAX, 0]).0 as u64;
    w.poke(kept, b"kept");
    let ro_before = w.pt().translate(&w.mem, RO).unwrap();
    let child = w.pt().fork_lower_half(&mut w.mem, &mut w.frames).unwrap();
    let wiped = w.pager.wx.wiped().to_vec();
    let mut tlb = RecordingTlb::default();
    space::wipe(&child, &mut w.mem, &mut tlb, &mut w.frames, &wiped).unwrap();
    let read = |w: &World, pt: &PageTable<Aarch64>, at: u64| {
        let mut out = [0u8; 8];
        memory::copy_from(pt, &w.mem, at, &mut out).unwrap();
        out
    };
    assert_eq!(read(&w, &child, DATA), [0; 8], "wiped in the copy");
    assert_eq!(read(&w, &w.pt(), DATA), 1u64.to_le_bytes(), "the source's");
    let ro = child.translate(&w.mem, RO).unwrap();
    assert_ne!(ro.phys, ro_before.phys, "a frame of its own");
    assert!(!ro.flags.is_writable(), "still read only");
    let data = child.translate(&w.mem, DATA).unwrap();
    assert!(
        data.flags.is_writable() && data.flags.contains(PageFlags::NO_EXECUTE),
        "{:?}",
        data.flags
    );
    assert_eq!(&read(&w, &child, kept)[..4], b"kept", "shared as before");
    assert_eq!(
        child.translate(&w.mem, kept).unwrap().phys,
        w.pt().translate(&w.mem, kept).unwrap().phys
    );
}

/// A cell reads its copy generation in `/run/rhizo/generation`, 8 bytes
/// little endian, as it is when opened; the file is read only (design D22
/// of `add-cell-hierarchy`).
#[test]
fn copy_generation_reads_in_run_rhizo() {
    const EROFS: i64 = 30;
    let mut w = World::new(Arch::Aarch64);
    let p = name(&mut w, "/run/rhizo/generation");
    let fd = w.call(OPENAT, &[AT_FDCWD, p, 0x8_0000 /* O_CLOEXEC */]).0;
    assert_eq!(fd, 3);
    assert_eq!(w.call(READ, &[3, DATA, 100]).0, 8);
    assert_eq!(w.peek(DATA, 8), 0u64.to_le_bytes());
    assert_eq!(w.call(READ, &[3, DATA, 100]).0, 0);
    w.cell.generation = 3;
    assert_eq!(w.call(LSEEK, &[3, 0, 0]).0, 0);
    assert_eq!(w.call(READ, &[3, DATA, 8]).0, 8);
    assert_eq!(w.peek(DATA, 8), 0u64.to_le_bytes(), "as when opened");
    assert_eq!(w.call(WRITE, &[3, DATA, 8]).0, -EBADF);
    assert_eq!(w.call(CLOSE, &[3]).0, 0);
    assert_eq!(w.call(OPENAT, &[AT_FDCWD, p, 0]).0, 3);
    assert_eq!(w.call(READ, &[3, DATA, 8]).0, 8);
    assert_eq!(w.peek(DATA, 8), 3u64.to_le_bytes());
    assert_eq!(w.call(OPENAT, &[AT_FDCWD, p, 1]).0, -EROFS);
    assert_eq!(w.call(OPENAT, &[AT_FDCWD, p, 0x42]).0, -EROFS);
}
