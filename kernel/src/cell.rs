//! Running cells: spawn from static executables, schedule them on this CPU
//! until they exit, fault or are killed, then return every frame (specs
//! `cells`, `linux-abi` and `scheduling`).

use crate::arch::console::BootConsole;
use crate::arch::cpu::CpuTlb;
use crate::arch::irq;
use crate::arch::platform::Format;
use crate::arch::timer::{self, Counter};
use crate::arch::user::{self, Trap};
use crate::memory::{self, DirectMap};
use crate::sync::Global;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, Ordering};
use rhizo_abi::{Abi, Context, Outcome, Services};
use rhizo_cell::Cell;
use rhizo_cell::elf::{ElfError, Image};
use rhizo_cell::memory::Pager;
use rhizo_cell::space::{self, Frames};
use rhizo_fs::Fs;
use rhizo_hal::{Console, CycleCounter, Tlb};
use rhizo_mm::paging::{PageSize, PageTable};
use rhizo_mm::{FRAME_SIZE, LARGE_PAGE_SIZE};
use rhizo_net::Net;
use rhizo_sched::{SLICE_NS, Scheduler, TagPool};
use rhizo_snapshot::Snapshot;

/// How a cell ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    /// It called `exit` or `exit_group` with this status.
    Exit(i32),
    /// It raised an exception and was killed.
    Fault,
    /// The kernel ended it (`Host::kill`).
    Killed,
    /// It was suspended into a snapshot (`Host::suspend`).
    Suspended,
}

/// The kernel's frame allocator, as cells see it.
#[derive(Debug)]
struct KernelFrames;

/// Extra references per frame (design D1 of `add-fork`): index = frame
/// number, value = references beyond the first. Grown on demand, so cells
/// that never fork cost nothing.
static SHARED: Global<Vec<u16>> = Global::new(Vec::new());

/// The file system of a cell given no files, and its encoding: built once,
/// then shared copy-on-write by every such cell, so spawning or resuming one
/// builds no table.
static NO_FILES: Global<Option<(Fs, Vec<u8>)>> = Global::new(None);

fn with_no_files<R>(f: impl FnOnce(&Fs, &[u8]) -> R) -> R {
    NO_FILES.with(|slot| {
        let (fs, bytes) = slot.get_or_insert_with(|| {
            let fs = Fs::new();
            let mut bytes = Vec::new();
            fs.encode(&mut bytes);
            (fs, bytes)
        });
        f(fs, bytes)
    })
}

/// The file system a snapshot holds, shared with [`NO_FILES`] when it has
/// no files.
fn files_of(snapshot: &Snapshot) -> Fs {
    with_no_files(|fs, bytes| (snapshot.files_bytes() == bytes).then(|| fs.clone()))
        .unwrap_or_else(|| snapshot.files())
}

/// The sockets of every cell (spec `networking`): cells of this kernel talk
/// over its loopback network.
static NET: Global<Net> = Global::new(Net::new());

/// The files of a fork of a cell with `fs`: its sockets as design D4 of
/// `add-sockets-and-poll` says (listeners shared, connections reset).
fn fork_files(fs: &Fs) -> Fs {
    let mut child = fs.clone();
    if fs.sockets().next().is_some() {
        NET.with(|net| child.map_sockets(|h| net.fork(h).unwrap_or(h)));
    }
    child
}

/// Sockets open in the kernel.
#[cfg_attr(
    not(feature = "ktest"),
    allow(dead_code, reason = "used by in-kernel tests")
)]
pub fn open_sockets() -> usize {
    NET.with(|net| net.open())
}

/// Releases the sockets `fs` holds (its cell ended or was suspended).
fn release_sockets(fs: &Fs) {
    if fs.sockets().next().is_some() {
        NET.with(|net| fs.sockets().for_each(|h| net.release(h)));
    }
}

/// `fs` with its sockets replaced by indices into the records returned, for
/// a snapshot.
fn export_sockets(fs: &Fs) -> (Fs, Vec<rhizo_net::Record>) {
    let mut records = Vec::new();
    if fs.sockets().next().is_none() {
        return (fs.clone(), records);
    }
    let mut out = fs.clone();
    NET.with(|net| {
        out.map_sockets(|h| {
            let record = net.export(h).unwrap_or(rhizo_net::Record {
                family: rhizo_net::Family::Inet,
                ty: rhizo_net::Type::Stream,
                state: rhizo_net::Saved::Reset,
                local: None,
                opts: rhizo_net::Options::default(),
            });
            records.push(record);
            (records.len() - 1) as u32
        });
    });
    (out, records)
}

/// Makes the sockets of a resumed cell from its snapshot's records, and
/// points its descriptors and Unix socket names at them. A socket that
/// cannot be made at all is left out: its descriptor reads as reset.
fn import_sockets(fs: &mut Fs, records: &[rhizo_net::Record]) {
    if records.is_empty() {
        return;
    }
    let mut handles = Vec::with_capacity(records.len());
    let mut renamed = Vec::new();
    NET.with(|net| {
        for r in records {
            let reset = rhizo_net::Record {
                state: rhizo_net::Saved::Reset,
                local: None,
                ..r.clone()
            };
            let made = net.import(r).or_else(|_| net.import(&reset));
            let (h, name) = made.unwrap_or((u32::MAX, None));
            handles.push(h);
            renamed.extend(name);
        }
    });
    fs.map_sockets(|i| handles.get(i as usize).copied().unwrap_or(u32::MAX));
    // A name keeps working only if its socket came back with the cell.
    if fs.socket_names().next().is_some() {
        fs.map_socket_names(|n| renamed.iter().find(|r| r.0 == n).map_or(0, |r| r.1));
    }
}

impl Frames for KernelFrames {
    fn alloc(&mut self) -> Option<u64> {
        memory::alloc_for_cell(1)
    }

    fn alloc_large(&mut self) -> Option<u64> {
        memory::alloc_for_cell((LARGE_PAGE_SIZE / FRAME_SIZE) as usize)
    }

    fn free(&mut self, phys: u64, size: PageSize) {
        let index = (phys / FRAME_SIZE) as usize;
        let shared = SHARED.with(|s| match s.get_mut(index) {
            Some(extra) if *extra > 0 => {
                *extra -= 1;
                true
            }
            _ => false,
        });
        if !shared {
            let frames = (size.bytes() / FRAME_SIZE) as usize;
            memory::with_frames(|f| f.free_contiguous(phys, frames));
        }
    }

    fn share(&mut self, phys: u64) {
        let index = (phys / FRAME_SIZE) as usize;
        SHARED.with(|s| {
            if s.len() <= index {
                s.resize(index + 1, 0);
            }
            s[index] = s[index]
                .checked_add(1)
                .expect("more than 65535 cells share a frame");
        });
    }

    fn is_shared(&self, phys: u64) -> bool {
        SHARED.with(|s| s.get((phys / FRAME_SIZE) as usize).is_some_and(|&e| e > 0))
    }
}

/// Console output of a cell: echoed to the console and/or copied into
/// `capture`.
struct KernelServices<'a> {
    capture: Option<&'a mut Vec<u8>>,
    echo: bool,
    random: u64,
}

impl Console for KernelServices<'_> {
    fn write_bytes(&mut self, bytes: &[u8]) {
        if self.echo {
            BootConsole.write_bytes(bytes);
        }
        if let Some(capture) = self.capture.as_deref_mut() {
            capture.extend_from_slice(bytes);
        }
    }
}

impl Services for KernelServices<'_> {
    fn log(&mut self, message: core::fmt::Arguments<'_>) {
        use core::fmt::Write;
        let _ = writeln!(rhizo_hal::ConsoleWriter(&mut BootConsole), "{message}");
    }

    fn monotonic_ns(&mut self) -> u64 {
        timer::cycles_to_ns(Counter.cycles()).unwrap_or(0)
    }

    fn realtime_ns(&mut self) -> u64 {
        // No real-time clock yet: the epoch is the boot.
        self.monotonic_ns()
    }

    fn can_store(&mut self, bytes: u64) -> bool {
        memory::cells_may_take(bytes)
    }

    fn random(&mut self, out: &mut [u8]) {
        // xorshift64*, seeded from the counter: not for cryptography (design
        // D6); deterministic sources for snapshots come in H4.
        for byte in out {
            self.random ^= self.random >> 12;
            self.random ^= self.random << 25;
            self.random ^= self.random >> 27;
            *byte = (self.random.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 56) as u8;
        }
    }
}

static NEXT_ID: AtomicU32 = AtomicU32::new(1);
/// The network card's address last reported on the console (0: none).
static WIRE_ADDR: AtomicU32 = AtomicU32::new(0);
/// Shortest time between two polls of the network card while cells run.
const WIRE_PERIOD_NS: u64 = 20_000;
static IRQ_READY: AtomicBool = AtomicBool::new(false);
/// Highest address-space tag of this CPU (set with `IRQ_READY`).
static MAX_TAG: AtomicU16 = AtomicU16::new(0);

/// Monotonic time in nanoseconds since boot (0 if the counter frequency is
/// unknown).
fn now_ns() -> u64 {
    timer::cycles_to_ns(Counter.cycles()).unwrap_or(0)
}

/// A live cell and what the kernel keeps for it.
#[derive(Debug)]
struct Live {
    cell: Cell,
    /// Address-space tag (ASID/PCID), 0 if none was free.
    tag: u16,
    /// Not run yet with this tag: its first entry drops the tag's old
    /// translations.
    fresh: bool,
    abi: Abi,
    output: Option<Vec<u8>>,
    echo: bool,
    random: u64,
    /// Pages still to restore, for a lazily resumed cell.
    lazy: Option<Lazy>,
    /// Its files and open descriptors (spec `files`).
    fs: Fs,
    /// Deadline of the blocked call it makes again when it runs (design D3
    /// of `add-sockets-and-poll`).
    deadline: Option<u64>,
}

/// The snapshot of a lazily resumed cell and which of its pages are still
/// only in the snapshot.
#[derive(Debug)]
struct Lazy {
    snapshot: Snapshot,
    pending: Vec<bool>,
    left: usize,
}

impl Lazy {
    fn new(snapshot: Snapshot) -> Self {
        let pages = snapshot.pages();
        Self {
            snapshot,
            pending: alloc::vec![true; pages],
            left: pages,
        }
    }

    /// Restores the page containing `addr` if it is still pending.
    fn page_in(&mut self, pt: &PageTable<Format>, addr: u64) -> bool {
        let Some(i) = self.snapshot.find(addr).filter(|&i| self.pending[i]) else {
            return false;
        };
        if self
            .snapshot
            .restore_page(i, pt, &mut DirectMap, &mut KernelFrames)
            .is_err()
        {
            return false;
        }
        let page = self.snapshot.page(i);
        if page.flags.is_executable()
            && let Some(t) = pt.translate(&DirectMap, page.virt)
        {
            user::sync_code(DirectMap::virt(t.phys), page.size.bytes());
        }
        self.pending[i] = false;
        self.left -= 1;
        true
    }

    /// Bytes of the pages still pending.
    fn pending_bytes(&self) -> u64 {
        (0..self.pending.len())
            .filter(|&i| self.pending[i])
            .map(|i| self.snapshot.page(i).size.bytes())
            .sum()
    }

    /// Drops pending pages inside `addr..addr + len` (the cell unmapped them).
    fn discard(&mut self, addr: u64, len: u64) {
        for i in 0..self.pending.len() {
            let page = self.snapshot.page(i);
            if self.pending[i] && page.virt >= addr && page.virt + page.size.bytes() <= addr + len {
                self.pending[i] = false;
                self.left -= 1;
            }
        }
    }

    /// Restores every page still pending.
    fn finish(&mut self, pt: &PageTable<Format>) {
        for i in 0..self.pending.len() {
            if self.pending[i] {
                self.page_in(pt, self.snapshot.page(i).virt);
            }
        }
    }
}

/// The pager of system calls: a lazily resumed cell's pending pages, and
/// copy-on-write pages shared after a fork.
struct CellPager<'a>(&'a mut Option<Lazy>);

impl Pager<Format, DirectMap> for CellPager<'_> {
    fn page_in(
        &mut self,
        pt: &PageTable<Format>,
        _: &mut DirectMap,
        addr: u64,
        write: bool,
    ) -> bool {
        if self.0.as_mut().is_some_and(|lazy| lazy.page_in(pt, addr)) {
            return true;
        }
        write && resolve_write(pt, addr)
    }

    fn discard(&mut self, addr: u64, len: u64) {
        if let Some(lazy) = self.0.as_mut() {
            lazy.discard(addr, len);
        }
    }
}

/// Resolves a write refused because the page is shared (copy-on-write).
fn resolve_write(pt: &PageTable<Format>, addr: u64) -> bool {
    let resolved = pt.resolve_write(&mut DirectMap, &mut KernelFrames, addr) == Ok(true);
    if resolved {
        CpuTlb.flush_page(addr);
    }
    resolved
}

/// How [`Host::resume`] restores memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resume {
    /// Every page before the cell runs.
    Eager,
    /// Each page the first time it is touched.
    Lazy,
}

/// A cell that ended, with its captured output.
#[derive(Debug)]
pub struct Finished {
    pub id: u32,
    pub ending: Ending,
    pub output: Vec<u8>,
}

/// The cells of this CPU: the scheduler holds the live ones (spec
/// `scheduling`); ended ones are kept until the caller looks at them.
#[derive(Debug)]
pub struct Host {
    sched: Scheduler<Live>,
    tags: TagPool,
    finished: Vec<Finished>,
    /// Frozen cells and their files: never run, only forked (spec `fork`,
    /// templates).
    templates: Vec<(Cell, Fs)>,
    /// Cell whose FP/SIMD state is in the registers (0: none). The kernel
    /// never uses them, so they are saved only when another cell runs.
    fp_owner: u32,
    /// [`Net::changes`] when waiting cells were last woken.
    net_seen: u64,
    /// When the network card is to be polled next, and what its last poll
    /// said: nanoseconds until it wants polling (design D4 of
    /// `add-virtio-net`; `None`: no card).
    wire_due: u64,
    wire_delay: Option<u64>,
    /// The card's address after its last poll.
    wire_addr: Option<[u8; 4]>,
}

impl Host {
    /// An empty host; sets up the timer interrupt the first time.
    pub fn new() -> Self {
        if !IRQ_READY.swap(true, Ordering::Relaxed) {
            irq::init(SLICE_NS);
            MAX_TAG.store(user::init_tags(), Ordering::Relaxed);
            NET.with(|net| crate::wire::init(net, now_ns(), Counter.cycles()));
        }
        Self {
            sched: Scheduler::new(),
            tags: TagPool::new(MAX_TAG.load(Ordering::Relaxed)),
            finished: Vec::new(),
            templates: Vec::new(),
            fp_owner: 0,
            net_seen: 0,
            wire_due: 0,
            wire_delay: None,
            wire_addr: None,
        }
    }

    /// A host whose cells get no address-space tag: every switch flushes the
    /// TLB, as before tags (the `bench-cell` baseline for `cell.switch_ns`).
    #[cfg(feature = "bench-cell")]
    fn new_untagged() -> Self {
        let mut host = Self::new();
        host.tags = TagPool::new(0);
        host
    }

    /// Creates a cell from the static executable `image` with `argv` and
    /// makes it runnable. Its output goes to the console when `echo` is set
    /// and is kept in [`Finished::output`] when `capture` is set.
    pub fn spawn(
        &mut self,
        image: &[u8],
        argv: &[&[u8]],
        capture: bool,
        echo: bool,
    ) -> Result<u32, ElfError> {
        let files = with_no_files(|fs, _| fs.clone());
        self.spawn_with_files(image, argv, files, capture, echo)
    }

    /// [`spawn`](Self::spawn) with `files` as the cell's file system (spec
    /// `files`, initial files).
    pub fn spawn_with_files(
        &mut self,
        image: &[u8],
        argv: &[&[u8]],
        files: Fs,
        capture: bool,
        echo: bool,
    ) -> Result<u32, ElfError> {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let mut services = KernelServices {
            capture: None,
            echo: false,
            random: Counter.cycles() | 1,
        };
        let mut random = [0u8; 16];
        services.random(&mut random);
        let kernel = memory::kernel_tables();
        let kernel_half = user::CELL_ROOT_HAS_KERNEL_HALF.then_some(&kernel);
        let cell = rhizo_cell::spawn::<Format>(
            id,
            image,
            user::MACHINE,
            kernel_half,
            &mut DirectMap,
            &mut KernelFrames,
            argv,
            &[],
            random,
        )?;
        sync_code(&PageTable::<Format>::from_root(cell.root), image);
        self.sched.add(Live {
            cell,
            tag: self.tags.alloc().unwrap_or(0),
            fresh: true,
            abi: Abi::new(user::ABI),
            output: capture.then(Vec::new),
            echo,
            random: services.random,
            lazy: None,
            fs: files,
            deadline: None,
        });
        Ok(id)
    }

    /// Brings `live`'s FP/SIMD state up to date in its registers, if it is in
    /// the CPU; with `leaving`, the CPU's copy stops belonging to it.
    fn sync_fp(&mut self, live: &mut Live, leaving: bool) {
        if self.fp_owner == live.cell.id {
            user::fp_save(&mut live.cell.regs.fp);
            if leaving {
                self.fp_owner = 0;
            }
        }
    }

    /// Suspends live cell `id` into a snapshot and frees all its memory
    /// (spec `snapshots`). Its captured output stays with the host.
    pub fn suspend(&mut self, id: u32) -> Option<Snapshot> {
        // The snapshot lives in the kernel heap, and pages still pending from
        // a lazy resume need frames first: refuse what would not fit
        // (design D2 of `add-memory-accounting`); the cell keeps running.
        let live = self.sched.find(|l| l.cell.id == id)?;
        let mut bytes = 0;
        PageTable::<Format>::from_root(live.cell.root)
            .for_each_lower_leaf(&DirectMap, &mut |_, _, size, _| bytes += size.bytes());
        if let Some(lazy) = &live.lazy {
            bytes += 2 * lazy.pending_bytes();
        }
        if !memory::cells_may_take(bytes) {
            return None;
        }
        let (mut live, wake) = self.sched.remove_with_wake(|l| l.cell.id == id)?;
        let pt = PageTable::<Format>::from_root(live.cell.root);
        if let Some(lazy) = live.lazy.as_mut() {
            lazy.finish(&pt);
        }
        self.sync_fp(&mut live, true);
        let remaining = wake.map_or(0, |w| w.saturating_sub(now_ns()).max(1));
        let (files, sockets) = export_sockets(&live.fs);
        let snapshot = Snapshot::capture::<Format>(
            user::MACHINE,
            &live.cell,
            &files,
            &sockets,
            remaining,
            &DirectMap,
        );
        self.end(live, Ending::Suspended);
        Some(snapshot)
    }

    /// Resumes `snapshot` as a new live cell (new identifier and tag) that
    /// continues exactly where the snapshot was taken. Gives the snapshot
    /// back if memory cannot hold the cell (eagerly: all its pages).
    pub fn resume(
        &mut self,
        snapshot: Snapshot,
        mode: Resume,
        capture: bool,
        echo: bool,
    ) -> Result<u32, Snapshot> {
        if mode == Resume::Eager {
            let bytes = (0..snapshot.pages())
                .map(|i| snapshot.page(i).size.bytes())
                .sum();
            if !memory::cells_may_take(bytes) {
                return Err(snapshot);
            }
        }
        let Ok(pt) = PageTable::<Format>::new(&mut DirectMap, &mut || KernelFrames.alloc()) else {
            return Err(snapshot);
        };
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        if user::CELL_ROOT_HAS_KERNEL_HALF {
            pt.share_upper_half(&mut DirectMap, &memory::kernel_tables());
        }
        let cell = snapshot.cell(id, pt.root());
        let sleep = snapshot.state().sleep_remaining_ns;
        let mut fs = files_of(&snapshot);
        import_sockets(&mut fs, &snapshot.sockets());
        let mut lazy = Lazy::new(snapshot);
        if mode == Resume::Eager {
            lazy.finish(&pt);
        }
        let live = Live {
            cell,
            tag: self.tags.alloc().unwrap_or(0),
            fresh: true,
            abi: Abi::new(user::ABI),
            output: capture.then(Vec::new),
            echo,
            random: Counter.cycles() | 1,
            lazy: (lazy.left > 0).then_some(lazy),
            fs,
            deadline: None,
        };
        match sleep {
            0 => self.sched.add(live),
            ns => self.sched.sleep(live, now_ns() + ns),
        }
        Ok(id)
    }

    /// Page-table frames of the own half of live cell `id`.
    #[cfg_attr(
        not(feature = "bench-cell"),
        allow(dead_code, reason = "benchmark metric")
    )]
    pub fn table_frames(&mut self, id: u32) -> Option<usize> {
        let live = self.sched.remove(|l| l.cell.id == id)?;
        let frames = PageTable::<Format>::from_root(live.cell.root).lower_half_tables(&DirectMap);
        self.sched.add(live);
        Some(frames)
    }

    /// Runs one scheduling step: the next runnable cell for one slice (until
    /// it ends, yields, sleeps or is preempted), waiting first if every cell
    /// sleeps. Returns `false` when no cell is left.
    #[cfg_attr(
        not(feature = "ktest"),
        allow(dead_code, reason = "used by in-kernel tests")
    )]
    pub fn step(&mut self) -> bool {
        let mut stepped = false;
        self.run(|_| core::mem::replace(&mut stepped, true));
        self.live() > 0
    }

    /// What live cell `id` has printed so far (when captured).
    #[cfg_attr(
        not(feature = "ktest"),
        allow(dead_code, reason = "used by in-kernel tests")
    )]
    pub fn output(&self, id: u32) -> Option<&[u8]> {
        self.sched.find(|l| l.cell.id == id)?.output.as_deref()
    }

    /// Pages of live cell `id` still waiting in its snapshot (lazy resume).
    #[cfg_attr(
        not(feature = "ktest"),
        allow(dead_code, reason = "used by in-kernel tests")
    )]
    pub fn pending_pages(&self, id: u32) -> Option<usize> {
        let live = self.sched.find(|l| l.cell.id == id)?;
        Some(live.lazy.as_ref().map_or(0, |l| l.left))
    }

    /// Runs cells until none is left or `stop` holds (checked whenever a cell
    /// ends or gives up the CPU).
    pub fn run(&mut self, mut stop: impl FnMut(&Self) -> bool) {
        while !stop(self) {
            let now = now_ns();
            self.poll_wire(now);
            self.wake_on_changes();
            let Some(live) = self.sched.next(now) else {
                let mut wake = self.sched.next_wake();
                // Waiting cells may wait for the wire: look again when it
                // wants (design D4 of `add-virtio-net`).
                if self.sched.waiting() > 0
                    && let Some(delay) = self.wire_delay
                    && delay != u64::MAX
                {
                    let soon = now.saturating_add(delay.max(1));
                    wake = Some(wake.map_or(soon, |w| w.min(soon)));
                }
                match wake {
                    Some(wake) => idle_until(wake),
                    None => return,
                }
                continue;
            };
            self.run_slice(live);
        }
    }

    /// Polls the network card when it is due (every scheduling step at most
    /// once per [`WIRE_PERIOD_NS`]), and reports a new address.
    fn poll_wire(&mut self, now: u64) {
        if now < self.wire_due {
            return;
        }
        let (delay, addr) = NET.with(|net| (crate::wire::poll(net, now), net.wire_addr()));
        // As often as the wire needs, but not more than once per period,
        // and at least every millisecond for frames nobody waits for yet.
        self.wire_due = now + delay.unwrap_or(u64::MAX).clamp(WIRE_PERIOD_NS, 1_000_000);
        self.wire_delay = delay;
        self.wire_addr = addr;
        let packed = addr.map_or(0, u32::from_be_bytes);
        if WIRE_ADDR.swap(packed, Ordering::Relaxed) != packed
            && let Some(a) = addr
        {
            use core::fmt::Write;
            let _ = writeln!(
                rhizo_hal::ConsoleWriter(&mut BootConsole),
                "net: address {}.{}.{}.{}",
                a[0],
                a[1],
                a[2],
                a[3]
            );
        }
    }

    /// Polls the network card until it has an address, at most
    /// `timeout_ns`; returns the address.
    #[cfg_attr(
        not(feature = "ktest"),
        allow(dead_code, reason = "used by in-kernel tests and benchmarks")
    )]
    pub fn wire_address(&mut self, timeout_ns: u64) -> Option<[u8; 4]> {
        let end = now_ns().saturating_add(timeout_ns);
        loop {
            let now = now_ns();
            self.wire_due = 0;
            self.poll_wire(now);
            if self.wire_addr.is_some() || self.wire_delay.is_none() || now >= end {
                return self.wire_addr;
            }
            idle_until(now + WIRE_PERIOD_NS * 50);
        }
    }

    /// Makes every waiting cell runnable if a socket changed since they
    /// started waiting (design D3 of `add-sockets-and-poll`).
    fn wake_on_changes(&mut self) {
        let changes = NET.with(|net| net.changes());
        if changes != self.net_seen {
            self.net_seen = changes;
            self.sched.wake_waiting();
        }
    }

    /// Runs `live` until it ends, sleeps, yields or is preempted, and files it
    /// accordingly.
    fn run_slice(&mut self, mut live: Live) {
        // FP/SIMD state moves only if this cell actually uses it (design D1).
        user::fp_trap(self.fp_owner != live.cell.id);
        let pt = PageTable::<Format>::from_root(live.cell.root);
        // Ticks seen in this slice: the first may have been pending since
        // before the cell was picked, so a cell is preempted at the second,
        // and always makes progress (design D4).
        let mut ticks = 0;
        loop {
            match user::enter_user(
                live.cell.root,
                live.tag,
                core::mem::replace(&mut live.fresh, false),
                &mut live.cell.regs,
            ) {
                Trap::Syscall => {
                    let mut services = KernelServices {
                        capture: live.output.as_mut(),
                        echo: live.echo,
                        random: live.random,
                    };
                    let (outcome, deadline) = NET.with(|net| {
                        let mut cx = Context {
                            pt,
                            mem: &mut DirectMap,
                            tlb: &mut CpuTlb,
                            frames: &mut KernelFrames,
                            services: &mut services,
                            fs: &mut live.fs,
                            net,
                            deadline: live.deadline.take(),
                            pager: &mut CellPager(&mut live.lazy),
                        };
                        (live.abi.handle(&mut live.cell, &mut cx), cx.deadline)
                    });
                    live.random = services.random;
                    match outcome {
                        Outcome::Resume => continue,
                        Outcome::Yield => self.sched.add(live),
                        Outcome::Sleep(until) => self.sched.sleep(live, until),
                        Outcome::Block(until) => {
                            live.deadline = deadline;
                            self.sched.wait(live, until);
                        }
                        Outcome::Exit(status) => self.end(live, Ending::Exit(status)),
                    }
                }
                Trap::FpUnavailable => {
                    let owner = self.fp_owner;
                    if let Some(previous) = self.sched.find_mut(|l| l.cell.id == owner) {
                        user::fp_save(&mut previous.cell.regs.fp);
                    }
                    user::fp_load(&live.cell.regs.fp);
                    user::fp_trap(false);
                    self.fp_owner = live.cell.id;
                    continue;
                }
                Trap::Interrupt => {
                    ticks += 1;
                    if ticks < 2 {
                        continue;
                    }
                    self.sched.add(live);
                }
                Trap::Fault(fault)
                    if fault.missing_page().is_some_and(|addr| {
                        live.lazy.as_mut().is_some_and(|l| l.page_in(&pt, addr))
                    }) =>
                {
                    if live.lazy.as_ref().is_some_and(|l| l.left == 0) {
                        live.lazy = None; // fully restored: drop the snapshot
                    }
                    continue;
                }
                Trap::Fault(fault)
                    if fault
                        .write_denied()
                        .is_some_and(|addr| resolve_write(&pt, addr)) =>
                {
                    continue;
                }
                Trap::Fault(fault) => {
                    let mut services = KernelServices {
                        capture: live.output.as_mut(),
                        echo: live.echo,
                        random: 0,
                    };
                    fault.report(&mut services, live.cell.id);
                    self.end(live, Ending::Fault);
                }
            }
            return;
        }
    }

    /// Forks live cell `id` into `n` children that share its memory
    /// copy-on-write (spec `fork`); returns their identifiers. A child has
    /// the cell's state, a new identifier and tag, and its own output.
    #[cfg_attr(
        not(feature = "ktest"),
        allow(dead_code, reason = "used by in-kernel tests")
    )]
    pub fn fork(&mut self, id: u32, n: usize) -> Option<Vec<u32>> {
        let (mut parent, wake) = self.sched.remove_with_wake(|l| l.cell.id == id)?;
        let pt = PageTable::<Format>::from_root(parent.cell.root);
        if let Some(lazy) = parent.lazy.as_mut() {
            lazy.finish(&pt);
        }
        parent.lazy = None;
        self.sync_fp(&mut parent, false);
        let mut children = Vec::with_capacity(n);
        for _ in 0..n {
            let Some(child) = self.child_of(
                &parent.cell,
                &parent.fs,
                parent.output.is_some(),
                parent.echo,
            ) else {
                break;
            };
            children.push(child);
        }
        // The parent lost write access to everything it shares: drop what its
        // TLB still caches.
        user::flush_user(parent.cell.root, parent.tag);
        self.requeue(parent, wake);
        if children.len() < n {
            // All or nothing: the children made so far never ran.
            for child in &children {
                self.discard(child);
            }
            return None;
        }
        let ids = children.iter().map(|c| c.cell.id).collect();
        for child in children {
            self.requeue(child, wake);
        }
        Some(ids)
    }

    /// Frees the memory, tag and sockets of a cell that never ran, leaving
    /// no trace (no [`Finished`]).
    fn discard(&mut self, live: &Live) {
        if live.tag != 0 {
            self.tags.free(live.tag);
        }
        release_sockets(&live.fs);
        space::destroy(
            PageTable::<Format>::from_root(live.cell.root),
            &mut DirectMap,
            &mut KernelFrames,
        );
    }

    /// A new live cell sharing `cell`'s memory copy-on-write.
    fn child_of(&mut self, cell: &Cell, fs: &Fs, capture: bool, echo: bool) -> Option<Live> {
        let pt = PageTable::<Format>::from_root(cell.root);
        let child = pt.fork_lower_half(&mut DirectMap, &mut KernelFrames).ok()?;
        Some(Live {
            cell: Cell {
                id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
                root: child.root(),
                ..*cell
            },
            tag: self.tags.alloc().unwrap_or(0),
            fresh: true,
            abi: Abi::new(user::ABI),
            output: capture.then(Vec::new),
            echo,
            random: Counter.cycles() | 1,
            lazy: None,
            fs: fork_files(fs),
            deadline: None,
        })
    }

    #[cfg_attr(not(feature = "ktest"), allow(dead_code, reason = "used by `fork`"))]
    fn requeue(&mut self, live: Live, wake: Option<u64>) {
        match wake {
            Some(at) => self.sched.sleep(live, at),
            None => self.sched.add(live),
        }
    }

    /// Freezes live cell `id` into a template: it never runs again, and
    /// [`spawn_from_template`](Self::spawn_from_template) forks it.
    pub fn freeze(&mut self, id: u32) -> bool {
        let Some(mut live) = self.sched.remove(|l| l.cell.id == id) else {
            return false;
        };
        let pt = PageTable::<Format>::from_root(live.cell.root);
        if let Some(lazy) = live.lazy.as_mut() {
            lazy.finish(&pt);
        }
        self.sync_fp(&mut live, true);
        user::release_user(live.cell.root, live.tag);
        if live.tag != 0 {
            self.tags.free(live.tag);
        }
        self.templates.push((live.cell, live.fs));
        true
    }

    /// Starts a new cell from template `id`: a fork of the frozen cell.
    pub fn spawn_from_template(&mut self, id: u32, capture: bool, echo: bool) -> Option<u32> {
        let index = self.templates.iter().position(|(c, _)| c.id == id)?;
        let (cell, fs) = &self.templates[index];
        let (cell, fs) = (*cell, fs.clone());
        let child = self.child_of(&cell, &fs, capture, echo)?;
        let child_id = child.cell.id;
        self.sched.add(child);
        Some(child_id)
    }

    /// Ends live cell `id` if it exists; returns whether it did.
    #[cfg_attr(
        not(feature = "ktest"),
        allow(dead_code, reason = "used by in-kernel tests")
    )]
    pub fn kill(&mut self, id: u32) -> bool {
        match self.sched.remove(|l| l.cell.id == id) {
            Some(live) => {
                self.end(live, Ending::Killed);
                true
            }
            None => false,
        }
    }

    /// How cell `id` ended, if it has.
    pub fn finished(&self, id: u32) -> Option<&Finished> {
        self.finished.iter().find(|f| f.id == id)
    }

    /// Number of live cells waiting on descriptors.
    #[cfg_attr(
        not(feature = "ktest"),
        allow(dead_code, reason = "used by in-kernel tests")
    )]
    pub fn waiting(&self) -> usize {
        self.sched.waiting()
    }

    /// Number of live cells.
    #[cfg_attr(
        not(feature = "ktest"),
        allow(dead_code, reason = "used by in-kernel tests")
    )]
    pub fn live(&self) -> usize {
        self.sched.len()
    }

    fn end(&mut self, live: Live, ending: Ending) {
        if self.fp_owner == live.cell.id {
            self.fp_owner = 0;
        }
        user::release_user(live.cell.root, live.tag);
        if live.tag != 0 {
            self.tags.free(live.tag);
        }
        space::destroy(
            PageTable::<Format>::from_root(live.cell.root),
            &mut DirectMap,
            &mut KernelFrames,
        );
        release_sockets(&live.fs);
        self.finished.push(Finished {
            id: live.cell.id,
            ending,
            output: live.output.unwrap_or_default(),
        });
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        while let Some(live) = self.sched.remove(|_| true) {
            self.end(live, Ending::Killed);
        }
        for (template, fs) in self.templates.drain(..) {
            space::destroy(
                PageTable::<Format>::from_root(template.root),
                &mut DirectMap,
                &mut KernelFrames,
            );
            release_sockets(&fs);
        }
    }
}

/// Waits without running anything until monotonic time `wake`.
fn idle_until(wake: u64) {
    let now = now_ns();
    if wake <= now {
        return;
    }
    if irq::available() {
        irq::wait_ns(wake - now);
    } else {
        while now_ns() < wake {
            core::hint::spin_loop();
        }
    }
}

/// Spawns and runs one cell to its end, echoing its output to the console;
/// returns how it ended and what it printed (including a fault report).
#[cfg_attr(
    not(feature = "ktest"),
    allow(dead_code, reason = "used by in-kernel tests")
)]
pub fn run(image: &[u8], argv: &[&[u8]]) -> Result<(Ending, Vec<u8>), ElfError> {
    let mut host = Host::new();
    let id = host.spawn(image, argv, true, true)?;
    host.run(|_| false);
    let done = host
        .finished
        .pop()
        .filter(|f| f.id == id)
        .expect("the cell ended");
    Ok((done.ending, done.output))
}

/// `bench-cell`: spawns and runs the hello cell repeatedly and prints the
/// median spawn time, the median time to run it to its exit (spawn
/// included), and its page-table frames.
#[cfg(feature = "bench-cell")]
pub fn bench(console: &mut impl Console) {
    use core::fmt::Write;
    const RUNS: usize = 101;
    let image = test_image("hello");
    let mut spawn_ns = Vec::with_capacity(RUNS);
    let mut run_ns = Vec::with_capacity(RUNS);
    let mut tables = 0;
    let mut host = Host::new();
    for _ in 0..RUNS {
        host.finished.clear();
        let t0 = Counter.cycles();
        let id = host
            .spawn(image, &[b"hello"], true, false)
            .expect("the hello cell spawns");
        let t1 = Counter.cycles();
        tables = host.table_frames(id).unwrap_or(0);
        host.run(|_| false);
        let t2 = Counter.cycles();
        let done = host.finished(id).expect("the hello cell ended");
        assert_eq!(done.ending, Ending::Exit(0), "the hello cell exits with 0");
        assert_eq!(done.output, b"hello from a cell\n");
        spawn_ns.push(timer::cycles_to_ns(t1 - t0).unwrap_or(0));
        run_ns.push(timer::cycles_to_ns(t2 - t0).unwrap_or(0));
    }
    let tagged = switch_ns(Host::new());
    let untagged = switch_ns(Host::new_untagged());
    let _ = writeln!(
        rhizo_hal::ConsoleWriter(console),
        "rhizo-cell spawn_ns={} run_ns={} pt_frames={tables} switch_ns={tagged} \
         switch_untagged_ns={untagged}",
        robust(&mut spawn_ns),
        robust(&mut run_ns)
    );
    let f = fork_and_templates();
    let _ = writeln!(
        rhizo_hal::ConsoleWriter(console),
        "rhizo-fork fork_ns={} fork_touch_ns={} fork_pt_frames={} spawn_from_template_ns={}",
        f[0],
        f[1],
        f[2],
        f[3]
    );
    let (cold, warm) = workload();
    let _ = writeln!(
        rhizo_hal::ConsoleWriter(console),
        "rhizo-workload cold_ns={cold} warm_ns={warm}"
    );
    // Before the others on purpose: the interpreter churns through memory,
    // and the figures after it must not depend on that (design D2 of
    // `reuse-freed-frames`).
    if let Some((cold, warm)) = python() {
        let _ = writeln!(
            rhizo_hal::ConsoleWriter(console),
            "rhizo-python cold_ns={cold} warm_ns={warm}"
        );
    }
    let (idle, suspended) = density();
    let _ = writeln!(
        rhizo_hal::ConsoleWriter(console),
        "rhizo-density idle_bytes={idle} suspended_bytes={suspended}"
    );
    let (write_read, fork_files) = files();
    let _ = writeln!(
        rhizo_hal::ConsoleWriter(console),
        "rhizo-files write_read_ns={write_read} fork_ns={fork_files}"
    );
    let (pipe, loopback) = net();
    let _ = writeln!(
        rhizo_hal::ConsoleWriter(console),
        "rhizo-net pipe_rtt_ns={pipe} loopback_rtt_ns={loopback}"
    );
    match wire_rtt() {
        Ok(rtt) => {
            let _ = writeln!(
                rhizo_hal::ConsoleWriter(console),
                "rhizo-wire tcp_rtt_ns={rtt}"
            );
        }
        Err(why) => {
            let _ = writeln!(
                rhizo_hal::ConsoleWriter(console),
                "rhizo-wire skipped: {why}"
            );
        }
    }
    let s = suspend_resume();
    let _ = writeln!(
        rhizo_hal::ConsoleWriter(console),
        "rhizo-snapshot suspend_ns={} resume_ns={} resume_eager_ns={} resume_touch_ns={} bytes={}",
        s[0],
        s[1],
        s[2],
        s[3],
        s[4]
    );
}

/// A real program (`libc_warm`, static glibc) answering one request: started
/// cold from its executable (warm-up included), and forked from a template
/// frozen after its warm-up. Medians over 7 rounds.
#[cfg(feature = "bench-cell")]
fn workload() -> (u64, u64) {
    const ROUNDS: usize = 7;
    let image = test_image("libc_warm");
    let ns = |t0: u64, t1: u64| timer::cycles_to_ns(t1 - t0).unwrap_or(0);
    let mut host = Host::new();
    let (mut cold, mut warm) = (Vec::new(), Vec::new());
    for _ in 0..ROUNDS {
        let t0 = Counter.cycles();
        let id = host
            .spawn(image, &[b"libc_warm"], true, false)
            .expect("spawn");
        host.run(|h| h.finished(id).is_some());
        cold.push(ns(t0, Counter.cycles()));
    }
    let template = host
        .spawn(image, &[b"libc_warm"], true, false)
        .expect("spawn");
    while host.output(template) != Some(b"warm\n") {
        host.step();
    }
    assert!(host.freeze(template));
    for _ in 0..ROUNDS {
        let t0 = Counter.cycles();
        let id = host
            .spawn_from_template(template, true, false)
            .expect("fork");
        host.run(|h| h.finished(id).is_some());
        warm.push(ns(t0, Counter.cycles()));
        assert_eq!(host.finished(id).map(|f| f.ending), Some(Ending::Exit(0)));
    }
    cold.sort_unstable();
    warm.sort_unstable();
    (cold[ROUNDS / 2], warm[ROUNDS / 2])
}

/// Medians over several rounds: forking a 50 MiB cell (frozen as a template,
/// so the child's writes copy everything), the fork plus the child writing
/// every page until it exits, the page-table frames a fork allocates, and
/// spawning `hello` from a template.
#[cfg(feature = "bench-cell")]
fn fork_and_templates() -> [u64; 4] {
    const ROUNDS: usize = 7;
    let ns = |t0: u64, t1: u64| timer::cycles_to_ns(t1 - t0).unwrap_or(0);
    let mut host = Host::new();
    let big = host
        .spawn(
            test_image("ballast"),
            &[b"ballast", b"write", b"50"],
            true,
            false,
        )
        .expect("spawn ballast");
    while host.output(big) != Some(b"filled\n") {
        host.step();
    }
    assert!(host.freeze(big));
    let (mut fork, mut touch) = (Vec::new(), Vec::new());
    let mut frames = 0;
    for _ in 0..ROUNDS {
        let before = memory::free_bytes();
        let t0 = Counter.cycles();
        let child = host.spawn_from_template(big, true, false).expect("fork");
        let t1 = Counter.cycles();
        frames = (before - memory::free_bytes()) / FRAME_SIZE;
        host.run(|h| h.finished(child).is_some());
        let t2 = Counter.cycles();
        assert_eq!(
            host.finished(child).map(|f| f.ending),
            Some(Ending::Exit(0))
        );
        fork.push(ns(t0, t1));
        touch.push(ns(t0, t2));
    }
    drop(host);
    let mut host = Host::new();
    let hello = host
        .spawn(test_image("hello"), &[b"hello"], true, false)
        .expect("spawn");
    assert!(host.freeze(hello));
    let mut template = Vec::new();
    for _ in 0..101 {
        let t0 = Counter.cycles();
        let id = host
            .spawn_from_template(hello, true, false)
            .expect("spawn from template");
        let t1 = Counter.cycles();
        host.run(|h| h.finished(id).is_some());
        template.push(ns(t0, t1));
    }
    let median = |v: &mut Vec<u64>| {
        v.sort_unstable();
        v[v.len() / 2]
    };
    [
        median(&mut fork),
        median(&mut touch),
        frames,
        robust(&mut template),
    ]
}

/// MicroPython answering one request (`cells/python/rhizo_demo.py`): started
/// cold from its executable (interpreter start, imports, warm-up), and
/// forked from a template frozen after the warm-up. Medians over 7 rounds;
/// `None` when MicroPython was not built.
#[cfg(feature = "bench-cell")]
fn python() -> Option<(u64, u64)> {
    const ROUNDS: usize = 7;
    const COMMAND: &[u8] =
        b"import sys; sys.path.insert(0, 'lib'); import rhizo_demo; rhizo_demo.main()";
    let image = test_image("micropython");
    if image.is_empty() {
        return None;
    }
    let files = || {
        let mut fs = Fs::new();
        fs.put_file(
            b"/lib/rhizo_demo.py",
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../cells/python/rhizo_demo.py"
            )),
        )
        .expect("module");
        fs
    };
    let argv: [&[u8]; 3] = [b"micropython", b"-c", COMMAND];
    let ns = |t0: u64, t1: u64| timer::cycles_to_ns(t1 - t0).unwrap_or(0);
    let mut host = Host::new();
    let (mut cold, mut warm) = (Vec::new(), Vec::new());
    for _ in 0..ROUNDS {
        let t0 = Counter.cycles();
        let id = host
            .spawn_with_files(image, &argv, files(), true, false)
            .expect("spawn");
        host.run(|h| h.finished(id).is_some());
        cold.push(ns(t0, Counter.cycles()));
        assert_eq!(host.finished(id).map(|f| f.ending), Some(Ending::Exit(0)));
    }
    let template = host
        .spawn_with_files(image, &argv, files(), true, false)
        .expect("spawn");
    while host.output(template) != Some(b"warm\n") {
        host.step();
    }
    assert!(host.freeze(template));
    for _ in 0..ROUNDS {
        let t0 = Counter.cycles();
        let id = host
            .spawn_from_template(template, true, false)
            .expect("fork");
        host.run(|h| h.finished(id).is_some());
        warm.push(ns(t0, Counter.cycles()));
        assert_eq!(host.finished(id).map(|f| f.ending), Some(Ending::Exit(0)));
    }
    cold.sort_unstable();
    warm.sort_unstable();
    Some((cold[ROUNDS / 2], warm[ROUNDS / 2]))
}

/// Memory per idle cell, from 1000 forks of a template of `idle` each run
/// into its sleep (the second of two identical rounds, so the heap has
/// grown its tables already), and the snapshot size of one idle cell
/// (design D3 of `add-memory-accounting`).
#[cfg(feature = "bench-cell")]
fn density() -> (u64, u64) {
    const N: usize = 1000;
    let image = test_image("idle");
    let round = || {
        let mut host = Host::new();
        let template = host.spawn(image, &[b"idle"], true, false).expect("spawn");
        while host.output(template) != Some(b"idle\n") {
            host.step();
        }
        assert!(host.freeze(template));
        let before = memory::free_bytes();
        let ids: Vec<u32> = (0..N)
            .map(|_| {
                host.spawn_from_template(template, false, false)
                    .expect("fork")
            })
            .collect();
        for _ in 0..N {
            host.step(); // each child runs into its sleep
        }
        let per_cell = (before - memory::free_bytes()) / N as u64;
        for id in ids {
            host.kill(id);
        }
        per_cell
    };
    round();
    let idle = round();
    let mut host = Host::new();
    let id = host.spawn(image, &[b"idle"], true, false).expect("spawn");
    while host.output(id) != Some(b"idle\n") {
        host.step();
    }
    let suspended = host.suspend(id).map_or(0, |s| s.as_bytes().len() as u64);
    (idle, suspended)
}

/// Rounds of a repeated measure (design D4 of `add-cell-files`).
#[cfg(feature = "bench-cell")]
const ROUNDS: usize = 5;

/// The minimum over [`ROUNDS`] consecutive rounds of each round's median:
/// the operation's cost, not the noise of its slowest moment.
#[cfg(feature = "bench-cell")]
fn robust(samples: &mut [u64]) -> u64 {
    let per = (samples.len() / ROUNDS).max(1);
    samples
        .chunks_mut(per)
        .take(ROUNDS)
        .map(|round| {
            round.sort_unstable();
            round[round.len() / 2]
        })
        .min()
        .unwrap_or(0)
}

/// Robust figures (see [`robust`]): `filer` creating, writing and reading
/// back 1 MiB of file data, spawn to exit; and forking a cell (frozen as a
/// template) that holds a 16 MiB file.
#[cfg(feature = "bench-cell")]
fn files() -> (u64, u64) {
    const RUNS: usize = 5 * ROUNDS;
    let image = test_image("filer");
    let ns = |t0: u64, t1: u64| timer::cycles_to_ns(t1 - t0).unwrap_or(0);
    let mut host = Host::new();
    let mut write_read = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        host.finished.clear();
        let t0 = Counter.cycles();
        let id = host.spawn(image, &[b"filer"], true, false).expect("spawn");
        host.run(|h| h.finished(id).is_some());
        write_read.push(ns(t0, Counter.cycles()));
        let done = host.finished(id).expect("ended");
        assert_eq!(done.ending, Ending::Exit(0), "filer exits with 0");
        assert_eq!(done.output, b"files ok\n");
    }
    let holder = host
        .spawn(image, &[b"filer", b"16"], true, false)
        .expect("spawn");
    while host.output(holder) != Some(b"files ok\n") {
        host.step();
    }
    assert!(host.freeze(holder));
    let mut fork = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        let t0 = Counter.cycles();
        let child = host.spawn_from_template(holder, true, false).expect("fork");
        fork.push(ns(t0, Counter.cycles()));
        assert!(host.kill(child));
    }
    (robust(&mut write_read), robust(&mut fork))
}

/// Robust figures (see [`robust`]) of the time per round trip: `piper`
/// writing one byte into a pipe and reading it back, and two `loopback`
/// cells exchanging a 64-byte message over TCP on 127.0.0.1 (design D7 of
/// `add-sockets-and-poll`).
#[cfg(feature = "bench-cell")]
fn net() -> (u64, u64) {
    const RUNS: usize = 3 * ROUNDS;
    const PIPE_TRIPS: u64 = 2000;
    const NET_TRIPS: u64 = 200;
    let ns = |t0: u64, t1: u64| timer::cycles_to_ns(t1 - t0).unwrap_or(0);
    let mut host = Host::new();
    let mut pipe = Vec::with_capacity(RUNS);
    let trips = b"2000";
    for _ in 0..RUNS {
        let t0 = Counter.cycles();
        let id = host
            .spawn(test_image("piper"), &[b"piper", trips], true, false)
            .expect("spawn piper");
        host.run(|h| h.finished(id).is_some());
        pipe.push(ns(t0, Counter.cycles()) / PIPE_TRIPS);
        assert_eq!(host.finished(id).map(|f| f.ending), Some(Ending::Exit(0)));
    }
    let image = test_image("loopback");
    let mut loopback = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        host.finished.clear();
        let server = host
            .spawn(
                image,
                &[b"loopback", b"serve", b"7400", b"200"],
                true,
                false,
            )
            .expect("spawn server");
        host.step(); // listening, waiting in `accept`
        let t0 = Counter.cycles();
        let client = host
            .spawn(image, &[b"loopback", b"ping", b"7400", b"200"], true, false)
            .expect("spawn client");
        host.run(|h| h.finished(server).is_some() && h.finished(client).is_some());
        loopback.push(ns(t0, Counter.cycles()) / NET_TRIPS);
        for id in [server, client] {
            assert_eq!(host.finished(id).map(|f| f.ending), Some(Ending::Exit(0)));
        }
    }
    (robust(&mut pipe), robust(&mut loopback))
}

/// A robust figure (see [`robust`]) of the round trip of a 64-byte message
/// between a `loopback` cell and the host's echo server, through the
/// network card (`echo=` on the command line).
#[cfg(feature = "bench-cell")]
fn wire_rtt() -> Result<u64, &'static str> {
    const RUNS: usize = 3 * ROUNDS;
    const TRIPS: u64 = 100;
    let (ip, port) = crate::wire::options()
        .echo
        .ok_or("no network card and echo server on this VMM")?;
    let mut host = Host::new();
    host.wire_address(10_000_000_000)
        .ok_or("the network card got no address")?;
    let ip = alloc::format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
    let port = alloc::format!("{port}");
    let argv: [&[u8]; 5] = [b"loopback", b"ping", port.as_bytes(), b"100", ip.as_bytes()];
    let image = test_image("loopback");
    let mut samples = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        host.finished.clear();
        let t0 = Counter.cycles();
        let id = host.spawn(image, &argv, true, false).expect("spawn");
        host.run(|h| h.finished(id).is_some());
        if host.finished(id).map(|f| f.ending) != Some(Ending::Exit(0)) {
            return Err("the echo server did not answer");
        }
        samples.push(timer::cycles_to_ns(Counter.cycles() - t0).unwrap_or(0) / TRIPS);
    }
    Ok(robust(&mut samples))
}

/// Medians over 7 rounds, for the `ballast` cell (8 MiB written): suspend,
/// lazy resume until runnable, eager resume, lazy resume plus touching every
/// page to the cell's exit; and the snapshot size.
#[cfg(feature = "bench-cell")]
fn suspend_resume() -> [u64; 5] {
    const ROUNDS: usize = 7;
    let image = test_image("ballast");
    let ns = |t0: u64, t1: u64| timer::cycles_to_ns(t1 - t0).unwrap_or(0);
    let mut samples: [Vec<u64>; 4] = Default::default();
    let mut bytes = 0;
    let mut host = Host::new();
    for _ in 0..ROUNDS {
        for (eager, slot) in [(false, 1), (true, 2)] {
            let id = host
                .spawn(image, &[b"ballast"], true, false)
                .expect("spawn ballast");
            while host.output(id) != Some(b"filled\n") {
                host.step();
            }
            let t0 = Counter.cycles();
            let snapshot = host.suspend(id).expect("live");
            let t1 = Counter.cycles();
            bytes = snapshot.as_bytes().len();
            let mode = if eager { Resume::Eager } else { Resume::Lazy };
            let t2 = Counter.cycles();
            let id = host.resume(snapshot, mode, true, false).expect("resume");
            let t3 = Counter.cycles();
            host.run(|h| h.finished(id).is_some());
            let t4 = Counter.cycles();
            assert_eq!(host.finished(id).map(|f| f.ending), Some(Ending::Exit(0)));
            samples[0].push(ns(t0, t1));
            samples[slot].push(ns(t2, t3));
            if !eager {
                samples[3].push(ns(t2, t4));
            }
        }
    }
    let mut out = [0u64; 5];
    for (o, s) in out.iter_mut().zip(&mut samples) {
        s.sort_unstable();
        *o = s[s.len() / 2];
    }
    out[4] = bytes as u64;
    out
}

/// Median over 5 rounds of the time per switch between two cells that
/// yield to each other 2000 times each (system call in, scheduler, system
/// call out to the other cell).
#[cfg(feature = "bench-cell")]
fn switch_ns(mut host: Host) -> u64 {
    const YIELDS: u64 = 2000;
    let image = test_image("yielder");
    let mut rounds = [0u64; 5];
    for round in &mut rounds {
        let a = host
            .spawn(image, &[b"yielder"], false, false)
            .expect("spawn yielder");
        let b = host
            .spawn(image, &[b"yielder"], false, false)
            .expect("spawn yielder");
        let t0 = Counter.cycles();
        host.run(|h| h.finished(a).is_some() && h.finished(b).is_some());
        let t1 = Counter.cycles();
        *round = timer::cycles_to_ns(t1 - t0).unwrap_or(0) / (2 * YIELDS);
    }
    rounds.sort_unstable();
    rounds[2]
}

/// Makes the executable pages just written through the direct map visible to
/// instruction fetch (needed on AArch64; nothing to do on x86_64).
fn sync_code(pt: &PageTable<Format>, image: &[u8]) {
    let Ok(image) = Image::parse(image, user::MACHINE) else {
        return;
    };
    for s in image.segments().filter(|s| s.flags.is_executable()) {
        let first = s.vaddr & !(FRAME_SIZE - 1);
        for page in (first..s.vaddr + s.mem_size).step_by(FRAME_SIZE as usize) {
            if let Some(t) = pt.translate(&DirectMap, page) {
                user::sync_code(DirectMap::virt(t.phys), FRAME_SIZE);
            }
        }
    }
}

include!(concat!(env!("OUT_DIR"), "/cells.rs"));
