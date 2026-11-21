//! Running cells: spawn from static executables, schedule them on every CPU
//! (the threads of one cell on several CPUs at once: [`Gang`]) until they
//! exit, fault or are killed, then return every frame (specs `cells`,
//! `linux-abi`, `scheduling`, `threads` and `multi-core`).

use crate::arch::console::BootConsole;
use crate::arch::cpu::CpuTlb;
use crate::arch::irq;
use crate::arch::platform::Format;
use crate::arch::timer::{self, Counter};
use crate::arch::user::{self, Trap, UserFault};
use crate::cost;
use crate::memory::{self, DirectMap};
use crate::prof::{self, Count, Ready};
use crate::sync::Global;
use alloc::boxed::Box;
use alloc::collections::BTreeSet;
use alloc::vec::Vec;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicU64, Ordering};
use rhizo_abi::{Abi, Context, ExecError, FilePages, Outcome, PageCache, Services};
use rhizo_cell::elf::{ElfError, Image};
use rhizo_cell::images::{self, Images};
use rhizo_cell::memory::Pager;
use rhizo_cell::space::{self, Frames};
use rhizo_cell::threads::Wait;
use rhizo_cell::{Cell, FpState, Registers};
use rhizo_core::random::{self, Batched, Generator};
use rhizo_core::smp::MAX_CPUS;
use rhizo_fs::Fs;
use rhizo_hal::{Console, CycleCounter, Tlb};
use rhizo_mm::paging::{PageSize, PageTable};
use rhizo_mm::{FRAME_SIZE, LARGE_PAGE_SIZE};
use rhizo_net::Net;
use rhizo_prof::Part;
use rhizo_sched::{Gangs, SLICE_NS, Scheduler, TagPool};
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

/// An empty file system for a new cell (shared until the cell writes).
pub fn no_files() -> Fs {
    with_no_files(|fs, _| fs.clone())
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

/// Set, under the sockets' lock, when a change woke waiters that nobody
/// drained yet: the scheduler takes the lock to drain them only then, not
/// at every take and give-back of a cell (design D2 of
/// `add-socket-wait-queues`).
static WOKEN: AtomicBool = AtomicBool::new(false);

/// The sockets as a system call reaches them (design D1 of
/// `split-sockets-lock`): locked the first time the call uses them, until
/// the call ends, with their change count then; a call that uses no socket
/// never waits for another CPU's socket calls.
#[derive(Default)]
struct CallNet(Option<(rhizo_core::sync::Guard<'static, Net>, u64)>);

impl rhizo_abi::Sockets for CallNet {
    fn net(&mut self) -> &mut Net {
        &mut self
            .0
            .get_or_insert_with(|| {
                let net = NET.lock();
                let changes = net.changes();
                (net, changes)
            })
            .0
    }
}

/// Runs `f` on the sockets, noting woken waiters for [`wake_woken`].
fn with_net<R>(f: impl FnOnce(&mut Net) -> R) -> R {
    NET.with(|net| {
        let r = f(net);
        if net.has_woken() {
            WOKEN.store(true, Ordering::Release);
        }
        r
    })
}

/// The files of a fork of a cell with `fs`: its sockets as design D4 of
/// `add-sockets-and-poll` says (listeners shared, connections reset).
fn fork_files(fs: &Fs) -> Fs {
    let mut child = fs.clone();
    if fs.sockets().next().is_some() {
        with_net(|net| child.map_sockets(|h| net.fork(h).unwrap_or(h)));
    }
    child
}

/// Sockets open in the kernel.
#[cfg_attr(
    not(feature = "ktest"),
    allow(dead_code, reason = "used by in-kernel tests")
)]
pub fn open_sockets() -> usize {
    with_net(|net| net.open())
}

/// Whether connections through the network card are still open or
/// closing (`smoltcp` keeps a closed connection until its peer has
/// acknowledged the close).
#[cfg_attr(
    not(feature = "ktest"),
    allow(dead_code, reason = "used by in-kernel tests")
)]
pub fn wire_busy() -> bool {
    with_net(|net| net.wire_busy())
}

/// Closes the connections parked for `snapshot`, which the host drops
/// without resuming it (spec `networking`, design D3 of
/// `keep-tcp-across-suspend`); resumed later, it finds them reset.
#[cfg_attr(
    not(feature = "ktest"),
    allow(dead_code, reason = "used by in-kernel tests")
)]
pub fn close_parked(snapshot: &Snapshot) {
    let records = snapshot.sockets();
    if !records.is_empty() {
        with_net(|net| records.iter().for_each(|r| net.unpark(r)));
    }
}

/// Releases the sockets `fs` of cell `id` holds (it ended or was
/// suspended), leaving their wait queues first.
fn release_sockets(fs: &Fs, id: u32) {
    if fs.sockets().next().is_some() {
        with_net(|net| {
            fs.sockets().for_each(|h| {
                net.forget(h, id);
                net.release(h);
            })
        });
    }
}

/// `fs` with its sockets replaced by indices into the records returned, for
/// a snapshot; its connections are parked in the kernel (design D1 of
/// `keep-tcp-across-suspend`).
fn export_sockets(fs: &Fs) -> (Fs, Vec<rhizo_net::Record>) {
    let mut records = Vec::new();
    if fs.sockets().next().is_none() {
        return (fs.clone(), records);
    }
    let mut out = fs.clone();
    with_net(|net| {
        out.map_sockets(|h| {
            let record = net.export(h).unwrap_or(rhizo_net::Record {
                family: rhizo_net::Family::Inet,
                ty: rhizo_net::Type::Stream,
                state: rhizo_net::Saved::Reset,
                local: None,
                opts: rhizo_net::Options::default(),
                parked: None,
            });
            records.push(record);
            (records.len() - 1) as u32
        });
    });
    (out, records)
}

/// Makes the sockets of a resumed cell from its snapshot's records (its
/// parked connections come back), and points its descriptors and Unix
/// socket names at them. A socket that
/// cannot be made at all is left out: its descriptor reads as reset.
fn import_sockets(fs: &mut Fs, records: &[rhizo_net::Record]) {
    if records.is_empty() {
        return;
    }
    let mut handles = Vec::with_capacity(records.len());
    let mut renamed = Vec::new();
    with_net(|net| {
        for r in records {
            let reset = rhizo_net::Record {
                state: rhizo_net::Saved::Reset,
                local: None,
                parked: None,
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

/// Executables kept loaded (design D3 of `add-image-cache`).
const KEPT_IMAGES: usize = 4;

/// Executables loaded once and copied by every later spawn of them (spec
/// `cells`, design D1 of `add-image-cache`): each a hierarchy holding only
/// its segments, never run, whose tables and pages are only read once it is
/// here. Its frames are the kernel's, taken like a cell's (never from the
/// reserve) and given back before any cell is refused memory (D4).
static IMAGES: Global<Images<KEPT_IMAGES>> = Global::new(Images::new());

/// Most file pages kept (design D1 of `add-dynamic-linking`): 64 MiB.
const KEPT_PAGES: usize = 16384;

/// Pages of files mapped into cells, kept by content (spec `cells`, file
/// pages shared between cells): one read-only frame for every mapping of the
/// same bytes. Its frames are taken and given back like those of
/// [`IMAGES`].
static PAGES: Global<PageCache> = Global::new(PageCache::new(KEPT_PAGES));

/// The frames of mapped file pages, from [`PAGES`]; a page written now is
/// made visible to instruction fetch.
struct KernelPages;

impl FilePages<DirectMap, KernelFrames> for KernelPages {
    fn frame(
        &mut self,
        page: &rhizo_fs::Page,
        _: &mut DirectMap,
        _: &mut KernelFrames,
    ) -> Option<u64> {
        // Short of frames, what is kept goes first (outside the lock).
        let get = || PAGES.with(|pages| pages.get(page, &mut DirectMap, &mut ImageFrames));
        let (phys, fresh) = get().or_else(|| drop_images().then(get).flatten())?;
        if fresh {
            self.code(phys);
        }
        Some(phys)
    }

    fn code(&mut self, phys: u64) {
        user::sync_code(DirectMap::virt(phys), FRAME_SIZE);
    }
}

/// Drops every image kept loaded and every file page kept; returns whether
/// there was one. Pages that cells still share stay theirs.
#[cfg_attr(
    not(feature = "ktest"),
    allow(dead_code, reason = "also used by in-kernel tests")
)]
pub fn drop_images() -> bool {
    let any = IMAGES.with(|images| {
        let mut any = false;
        while let Some(root) = images.pop() {
            space::destroy(
                PageTable::<Format>::from_root(root),
                &mut DirectMap,
                &mut ImageFrames,
            );
            any = true;
        }
        any
    });
    PAGES.with(|pages| pages.clear(&mut ImageFrames)) || any
}

/// Whether cells may take `bytes` more, dropping the images kept loaded if
/// that is what it takes (design D4 of `add-image-cache`).
fn cells_may_take(bytes: u64) -> bool {
    memory::cells_may_take(bytes) || (drop_images() && memory::cells_may_take(bytes))
}

/// A space of its own holding the segments of the executable `data`, parsed
/// as `image` (design D2 of `add-image-cache`): a copy of the space kept
/// for it, loaded first if it is not kept yet.
fn image_space(data: &'static [u8], image: &Image) -> Result<PageTable<Format>, ElfError> {
    let key = (data.as_ptr() as u64, data.len() as u64);
    if let Some(pt) = IMAGES.with(|images| copy_image(images, key, image)) {
        return Ok(pt);
    }
    // Loaded outside the lock: loading may drop the images kept.
    let loaded = rhizo_cell::load::<Format>(image, &mut DirectMap, &mut KernelFrames)?;
    sync_code(&loaded, image);
    IMAGES
        .with(|images| {
            if images.get(key).is_some() {
                // Another CPU loaded it meanwhile.
                space::destroy(loaded, &mut DirectMap, &mut ImageFrames);
            } else if let Some(old) = images.insert(key, loaded.root()) {
                space::destroy(
                    PageTable::<Format>::from_root(old),
                    &mut DirectMap,
                    &mut ImageFrames,
                );
            }
            copy_image(images, key, image)
        })
        .ok_or(ElfError::OutOfMemory)
}

/// A copy of the space kept for `key`: its read-only pages shared, its
/// tables and writable pages the copy's own. Short of frames for that, the
/// kept space itself, taken out, with every other image dropped: a spawn
/// never needs more memory than a load. `None` if `key` is not kept.
fn copy_image(
    images: &mut Images<KEPT_IMAGES>,
    key: images::Key,
    image: &Image,
) -> Option<PageTable<Format>> {
    let kept = PageTable::<Format>::from_root(images.get(key)?);
    if let Ok(copy) = kept.clone_lower_half(&mut DirectMap, &mut ImageFrames, image.span()) {
        return Some(copy);
    }
    images.remove(key);
    while let Some(root) = images.pop() {
        space::destroy(
            PageTable::<Format>::from_root(root),
            &mut DirectMap,
            &mut ImageFrames,
        );
    }
    Some(kept)
}

/// The frames of [`KernelFrames`] for work under the [`IMAGES`] lock: an
/// allocation that fails does not try to drop images.
#[derive(Debug)]
struct ImageFrames;

impl Frames for ImageFrames {
    fn alloc(&mut self) -> Option<u64> {
        let _p = cost::scope(Part::Frames);
        memory::alloc_for_cell(1)
    }

    fn alloc_large(&mut self) -> Option<u64> {
        let _p = cost::scope(Part::Frames);
        memory::alloc_for_cell((LARGE_PAGE_SIZE / FRAME_SIZE) as usize)
    }

    fn free(&mut self, phys: u64, size: PageSize) {
        KernelFrames.free(phys, size);
    }

    fn share(&mut self, phys: u64) {
        KernelFrames.share(phys);
    }

    fn is_shared(&self, phys: u64) -> bool {
        KernelFrames.is_shared(phys)
    }

    fn try_share(&mut self, phys: u64) -> bool {
        KernelFrames.try_share(phys)
    }
}

impl Frames for KernelFrames {
    /// Short of frames, the images kept loaded go first (design D4 of
    /// `add-image-cache`).
    fn alloc(&mut self) -> Option<u64> {
        ImageFrames
            .alloc()
            .or_else(|| drop_images().then(|| ImageFrames.alloc()).flatten())
    }

    /// Without dropping images: whoever asks for 2 MiB falls back to
    /// 4 KiB pages, which may.
    fn alloc_large(&mut self) -> Option<u64> {
        ImageFrames.alloc_large()
    }

    fn free(&mut self, phys: u64, size: PageSize) {
        let _p = cost::scope(Part::Frames);
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
        let _p = cost::scope(Part::Frames);
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
        let _p = cost::scope(Part::Frames);
        SHARED.with(|s| s.get((phys / FRAME_SIZE) as usize).is_some_and(|&e| e > 0))
    }

    fn try_share(&mut self, phys: u64) -> bool {
        let _p = cost::scope(Part::Frames);
        let index = (phys / FRAME_SIZE) as usize;
        SHARED.with(|s| {
            if s.len() <= index {
                s.resize(index + 1, 0);
            }
            s[index].checked_add(1).map(|n| s[index] = n).is_some()
        })
    }
}

/// Most output kept for one cell when the host captures it (spec
/// `isolation`): a cell printing without end cannot fill the kernel heap.
const CAPTURE_MAX: usize = 1 << 20;

/// Console output of a cell: echoed to the console and/or copied into
/// `capture`.
struct KernelServices<'a> {
    capture: Option<&'a mut Vec<u8>>,
    echo: bool,
    /// The cell's generator, split off the kernel's on first use.
    random: &'a mut Option<Generator>,
    /// The calling thread's FP/SIMD state is in the CPU's registers.
    fp_in_cpu: bool,
}

impl Console for KernelServices<'_> {
    fn write_bytes(&mut self, bytes: &[u8]) {
        if self.echo {
            BootConsole.write_bytes(bytes);
        }
        // Captured output is kernel heap: bounded per cell, and never grown
        // by an allocation that could fail (and panic); the rest is dropped.
        if let Some(capture) = self.capture.as_deref_mut() {
            let n = bytes.len().min(CAPTURE_MAX.saturating_sub(capture.len()));
            if capture.try_reserve(n).is_ok() {
                capture.extend_from_slice(&bytes[..n]);
            }
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
        cells_may_take(bytes)
    }

    fn memory(&mut self) -> (u64, u64) {
        memory::with_frames(|f| (f.usable_frames() as u64, f.free_frames() as u64))
            .map_or((0, 0), |(total, free)| {
                (total * FRAME_SIZE, free * FRAME_SIZE)
            })
    }

    fn save_fp(&mut self, fp: &mut FpState) {
        if self.fp_in_cpu {
            user::fp_save(fp);
        }
    }

    fn random(&mut self, out: &mut [u8]) {
        self.random
            .get_or_insert_with(|| RANDOM.with(Batched::split))
            .fill(out);
    }
}

/// The 16 random bytes of a new cell's stack (`AT_RANDOM`), from the
/// kernel's generator.
fn start_random() -> [u8; 16] {
    let mut random = [0u8; 16];
    RANDOM.with(|g| g.fill(&mut random));
    random
}

/// The TLB of a hierarchy no CPU has used yet: nothing to flush (its tag's
/// old translations go on its first entry on each CPU).
struct NewSpace;

impl Tlb for NewSpace {
    fn flush_page(&mut self, _: u64) {}
}

static NEXT_ID: AtomicU32 = AtomicU32::new(1);
/// The kernel's generator, seeded from the hardware by [`init`]: each cell
/// splits its own off it (design D2 of `add-crypto-random`).
static RANDOM: Global<Batched> = Global::new(Batched::new(Generator::from_key([0; 32])));
/// The network card's address last reported on the console (0: none).
static WIRE_ADDR: AtomicU32 = AtomicU32::new(0);
/// Shortest time between two polls of the network card while cells run.
const WIRE_PERIOD_NS: u64 = 20_000;
/// Highest address-space tag (set by [`init`]).
static MAX_TAG: AtomicU16 = AtomicU16::new(0);

/// Sets up preemption and address-space tags on the boot CPU, then starts
/// the other CPUs, at most `smp` in all (spec `multi-core`), and turns KPTI
/// on where the CPU needs it, or always in the test build so every test
/// runs with it (spec `isolation`, design D1 of `add-kpti`); returns how
/// many CPUs are online. Call once, before any [`Host`].
pub fn init(smp: Option<u16>) -> usize {
    crate::heap::window::init();
    let (generator, strong) =
        random::seeded(&mut crate::arch::random::HardwareEntropy, Counter.cycles());
    RANDOM.with(|g| *g = Batched::new(generator));
    if !strong {
        use core::fmt::Write;
        let _ = writeln!(
            rhizo_hal::ConsoleWriter(&mut BootConsole),
            "rhizo: no hardware random source: getrandom is predictable"
        );
    }
    irq::init(SLICE_NS);
    MAX_TAG.store(user::init_tags(), Ordering::Relaxed);
    // The network card comes up once, at boot, rather than with the first
    // host: what it allocates is never charged to a cell or a test.
    let mut seed = [0; 8];
    // The parked records' key secret, from the kernel's generator (finding
    // S15: keys cannot be guessed without it).
    let mut secret = [0u8; 16];
    RANDOM.with(|g| {
        g.fill(&mut seed);
        g.fill(&mut secret);
    });
    with_net(|net| {
        // Parked connections re-attach only in this boot (design D2 of
        // `keep-tcp-across-suspend`).
        net.set_boot(Counter.cycles());
        let word = |i: usize| u64::from_le_bytes(secret[i..i + 8].try_into().unwrap_or_default());
        net.set_secret([word(0), word(8)]);
        crate::wire::init(net, now_ns(), u64::from_le_bytes(seed));
    });
    let cpus = crate::smp::start(smp);
    user::init_kpti(cpus);
    user::set_kpti(cfg!(feature = "ktest") || user::kpti_needed());
    cpus
}

/// Monotonic time in nanoseconds since boot (0 if the counter frequency is
/// unknown).
pub(crate) fn now_ns() -> u64 {
    timer::cycles_to_ns(Counter.cycles()).unwrap_or(0)
}

/// Who owns the CPU's FP/SIMD registers: thread `tid` of cell `cell` (0:
/// nobody; cell identifiers start at 1, and neither they nor thread
/// identifiers are reused, so a key never names another thread).
fn fp_key(cell: u32, tid: u32) -> u64 {
    ((cell as u64) << 32) | tid as u64
}

/// Held while page tables and frames that cells may share change: a fork's
/// copy-on-write tables are resolved, unmapped or freed by one CPU at a
/// time (design D6 of `add-smp-scheduling`). Cells that share nothing never
/// take it.
static SHARING: Global<()> = Global::new(());

/// Runs `f`, under [`SHARING`] when `live` may share frames with another
/// cell.
fn sharing<R>(shares: bool, f: impl FnOnce() -> R) -> R {
    if shares { SHARING.with(|()| f()) } else { f() }
}

/// The owner ([`fp_key`]) of each CPU's FP/SIMD registers, by CPU number.
static FP_OWNER: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

/// A live cell and what the kernel keeps for it.
#[derive(Debug)]
struct Live {
    cell: Cell,
    /// Address-space tag (ASID/PCID), 0 if none was free.
    tag: u16,
    /// CPUs whose TLB holds nothing stale for this cell (bit `n` for CPU
    /// `n`): none for a cell new to its tag, which drops the tag's old
    /// translations on its first entry on each CPU, and only the CPU that
    /// flushed after the cell's translations shrank, where flushes are not
    /// broadcast (design D5 of `add-smp-scheduling`).
    clean: u32,
    /// CPUs it ran on (bit `n` for CPU `n`).
    cpus: u32,
    /// Its memory may be shared with another cell, copy-on-write: it was
    /// forked, or is a fork.
    shares: bool,
    abi: Abi,
    output: Option<Vec<u8>>,
    echo: bool,
    /// Its generator: none until it first asks for random bytes, never in a
    /// snapshot, and never shared with a fork (design D2 of
    /// `add-crypto-random`).
    random: Option<Generator>,
    /// Pages still to restore, for a lazily resumed cell.
    lazy: Option<Lazy>,
    /// Its files and open descriptors (spec `files`).
    fs: Fs,
    /// Deadlines of its threads' calls waiting on descriptors, kept for
    /// when they make them again (design D3 of `add-sockets-and-poll`).
    deadlines: Vec<(u32, u64)>,
    /// A socket it waited on changed since its last system call, for the
    /// workloads' count of wake-ups that found nothing ([`prof`]).
    io_woken: bool,
    /// [`Host::wake`] came while no thread waited with a deadline: the next
    /// wait with one ends at once.
    host_wake: bool,
    /// Since when it can run, for the workloads' latencies ([`prof`]).
    ready: Ready,
    /// The thread chosen last while it ran on several CPUs: turns go on
    /// from there the next time (design D5 of `add-parallel-threads`).
    last: u32,
    /// Most CPUs that ran its threads at the same time (design D5 of
    /// `add-parallel-threads`).
    peak: u32,
}

impl Live {
    /// The deadline thread `tid`'s call keeps, if it waited with one.
    fn take_deadline(&mut self, tid: u32) -> Option<u64> {
        let i = self.deadlines.iter().position(|&(t, _)| t == tid)?;
        Some(self.deadlines.swap_remove(i).1)
    }
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
        // A 2 MiB page may have come back as 4 KiB pages: each its own.
        let mut at = page.virt;
        while page.flags.is_executable()
            && at < page.virt + page.size.bytes()
            && let Some(t) = pt.translate(&DirectMap, at)
        {
            user::sync_code(DirectMap::virt(t.phys), t.size.bytes());
            at += t.size.bytes();
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

/// The TLB as a cell sees it: [`CpuTlb`], noting that the cell's
/// translations shrank, and keeping the other CPUs that run the cell's
/// code out of it until the change is complete (design D7 of
/// `add-parallel-threads`).
struct CellTlb<'a> {
    shrank: bool,
    /// The gang this CPU runs, and the CPU's number.
    gang: Option<(&'a Gang, usize)>,
}

impl<'a> CellTlb<'a> {
    fn new(gang: Option<(&'a Gang, usize)>) -> Self {
        Self {
            shrank: false,
            gang,
        }
    }
}

impl Tlb for CellTlb<'_> {
    fn flush_page(&mut self, virt: u64) {
        if let Some((gang, cpu)) = self.gang {
            gang.hold_others(cpu);
        }
        CpuTlb.flush_page(virt);
        self.shrank = true;
    }
}

/// The pager of system calls: a lazily resumed cell's pending pages, and
/// copy-on-write pages shared after a fork (resolved through the TLB given).
struct CellPager<'a, 'g>(&'a mut Option<Lazy>, &'a mut CellTlb<'g>);

impl Pager<Format, DirectMap> for CellPager<'_, '_> {
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
        (write && resolve_write(pt, self.1, addr)) || grow_stack(pt, addr)
    }

    fn discard(&mut self, addr: u64, len: u64) {
        if let Some(lazy) = self.0.as_mut() {
            lazy.discard(addr, len);
        }
    }
}

/// Grows the stack of the cell with tables `pt` to cover `addr` (design D5
/// of `add-cpython`), if memory allows.
fn grow_stack(pt: &PageTable<Format>, addr: u64) -> bool {
    let _p = cost::scope(Part::Fault);
    cells_may_take(FRAME_SIZE)
        && space::grow_stack(pt, &mut DirectMap, &mut CpuTlb, &mut KernelFrames, addr)
}

/// Resolves a write refused because the page is shared (copy-on-write).
fn resolve_write(pt: &PageTable<Format>, tlb: &mut CellTlb<'_>, addr: u64) -> bool {
    let _p = cost::scope(Part::Cow);
    let resolved = pt.resolve_write(&mut DirectMap, &mut KernelFrames, tlb, addr) == Ok(true);
    if resolved {
        tlb.flush_page(addr);
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
    /// The CPUs it ran on (bit `n` for CPU `n`).
    #[cfg_attr(not(feature = "ktest"), allow(dead_code, reason = "in-kernel tests"))]
    pub cpus: u32,
    /// Most CPUs that ran its threads at the same time.
    #[cfg_attr(not(feature = "ktest"), allow(dead_code, reason = "in-kernel tests"))]
    pub peak: u32,
}

/// What the CPUs running a host's cells share (design D3 of
/// `add-smp-scheduling`), behind one lock taken only to take a cell, give
/// one back, or change the set of cells between runs. A cell taken out runs
/// on one CPU, which owns it until it gives it back.
#[derive(Debug)]
struct Shared {
    /// The live cells that are not running: one run queue per CPU, and the
    /// sleepers (spec `scheduling`).
    sched: Scheduler<Box<Live>>,
    tags: TagPool,
    /// Cells that ended since the host last looked ([`Host::collect`]).
    ended: Vec<Finished>,
    /// Cells taken out of `sched`, running on a CPU.
    running: usize,
    /// CPUs waiting for work in the current run (bit `n` for CPU `n`).
    idle: u32,
    /// The current run is over: CPUs give their cells back and stop.
    stopping: bool,
    /// Cells woken by a socket change while they were not in the waiting
    /// set (running, or about to be filed): their threads waiting on
    /// descriptors look again when they are next taken or given back
    /// (design D2 of `add-socket-wait-queues`).
    nudged: BTreeSet<u32>,
    /// Cells taken out whose threads run on several CPUs (design D1 of
    /// `add-parallel-threads`).
    gangs: Gangs<GangPtr>,
}

/// The cells of the machine: the live ones in the shared scheduler (or on
/// the CPU running them), ended ones kept until the caller looks at them.
/// [`run`](Self::run) uses every CPU; between runs no cell runs, so every
/// other operation sees all of a cell's threads stopped.
#[derive(Debug)]
pub struct Host {
    shared: Global<Shared>,
    finished: Vec<Finished>,
    /// Frozen cells and their files: never run, only forked (spec `fork`,
    /// templates).
    templates: Vec<(Cell, Fs)>,
    /// When the network card is to be polled next, and what its last poll
    /// said: nanoseconds until it wants polling (design D4 of
    /// `add-virtio-net`; `None`: no card). The boot CPU polls it.
    wire_due: u64,
    wire_delay: Option<u64>,
    /// The card's address after its last poll.
    wire_addr: Option<[u8; 4]>,
    /// Longest the boot CPU waits in [`run`](Self::run) before it looks at
    /// its stop condition again, even with nothing to run (a control
    /// kernel's poll of the serial line, design D4 of `add-control-plane`).
    look_every: Option<u64>,
}

/// What a CPU does next ([`take`]).
enum Take {
    /// Run this cell, picked at this time.
    Run(Box<Live>, u64),
    /// Run a thread of this running cell.
    Join(GangPtr),
    /// Wait for work until this time (or a wake-up, or for ever).
    Wait(Option<u64>),
    /// Cells ended that the boot CPU has not looked at: look first (its
    /// stop condition may hold now).
    Look,
    /// No cell can ever run again in this run.
    Done,
    /// The run is over.
    Stop,
}

/// How a cell comes back from its slice ([`give_back`]).
#[derive(Clone, Copy)]
enum Back {
    /// Preempted or yielding: runnable again.
    Requeue,
    /// Its threads block: runnable or asleep, as they say.
    File,
    /// It ended.
    End(Ending),
    /// Its thread made another: other CPUs may run them (design D2 of
    /// `add-parallel-threads`).
    Share,
}

/// Takes the next cell for `cpu`: the first of its queue or one stolen
/// (`oldest`: the one that has waited longest anywhere, for a single CPU
/// stepping through every cell).
fn take(shared: &Global<Shared>, cpu: usize, oldest: bool) -> Take {
    let _p = cost::scope(Part::Sched);
    let _timed = prof::Timed::new(Count::SchedCycles);
    shared.with(|s| {
        if s.stopping {
            s.idle &= !(1 << cpu);
            return Take::Stop;
        }
        let now = now_ns();
        wake_woken(s, cpu, || now);
        if prof::on() {
            // When each due cell joins a queue, for its latency.
            let q = if oldest { 0 } else { cpu };
            s.sched
                .wake_due(q, now, |live| live.ready = live.ready.due(now));
        }
        let next = if oldest {
            s.sched.next_oldest(now)
        } else {
            s.sched.next(cpu, now)
        };
        if let Some(mut live) = next {
            nudge(s, &mut live);
            live.ready.ran(now);
            live.ready = Ready::default();
            s.running += 1;
            s.idle &= !(1 << cpu);
            return Take::Run(live, now);
        }
        if !oldest && let Some(gang) = s.gangs.join(cpu, now) {
            s.idle &= !(1 << cpu);
            return Take::Join(gang);
        }
        let wake = match (s.sched.next_wake(), s.gangs.next_wake()) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
        .filter(|&t| t != u64::MAX);
        if s.running == 0 && wake.is_none() {
            // Only parked cells are left: nothing will wake them.
            return Take::Done;
        }
        if cpu == 0 && !s.ended.is_empty() {
            return Take::Look;
        }
        // Under the lock: a CPU giving a cell back sees this bit and wakes
        // this one, so no wake-up is lost between here and the wait.
        s.idle |= 1 << cpu;
        Take::Wait(wake)
    })
}

/// Wakes the cells whose sockets changed since the last call (design D2 of
/// `add-socket-wait-queues`): a waiting one joins `cpu`'s queue and makes
/// its calls again; any other is nudged ([`Shared::nudged`]). Called under
/// the host's lock, so a cell filed to wait cannot miss its wake-up.
fn wake_woken(s: &mut Shared, cpu: usize, now: impl FnOnce() -> u64) {
    // Set under the sockets' lock after a change woke waiters: clear, it
    // means nothing waits to be drained (a change after this load sets it
    // again, for the next drain, which comes before its cell is filed).
    if !WOKEN.swap(false, Ordering::Acquire) {
        return;
    }
    let now = now();
    // Lock order: the host's, then the sockets.
    NET.with(|net| {
        net.drain_woken(|id| {
            let waited = s.sched.wake(cpu, id, |live| {
                live.cell.threads.wake_io();
                live.ready = live.ready.net_change(now);
                live.io_woken = prof::on();
            });
            if !waited {
                s.nudged.insert(id);
            }
        })
    });
}

/// Lets the threads of `live` waiting on descriptors look again if a
/// socket they waited on changed while the cell was out of the waiting set.
fn nudge(s: &mut Shared, live: &mut Live) {
    if !s.nudged.is_empty() && s.nudged.remove(&live.cell.id) {
        live.cell.threads.wake_io();
    }
}

/// Waits for work: until `wake`, or a wake-up interrupt.
fn wait(wake: Option<u64>) {
    let _p = cost::scope(Part::Idle);
    match wake {
        // With several CPUs, look again at least every slice: what wakes a
        // cell may happen on another CPU (a safety net; wake-ups are sent).
        Some(at) if crate::smp::count() > 1 => idle_until(at.min(now_ns() + SLICE_NS)),
        Some(at) => idle_until(at),
        None => prof::idle(crate::smp::idle),
    }
    cost::after_idle();
}

/// Puts `live`, back from a slice on `cpu`, where `how` says, and wakes an
/// idle CPU that has something to do because of it: another runnable cell
/// to take, or the boot CPU to see a cell end or the last one stop.
fn give_back(shared: &Global<Shared>, cpu: usize, mut live: Box<Live>, how: Back) {
    let _p = cost::scope(Part::Sched);
    let _timed = prof::Timed::new(Count::SchedCycles);
    let ended = match how {
        Back::End(ending) => {
            shared.with(|s| s.nudged.remove(&live.cell.id));
            Some(end(live, ending))
        }
        Back::Requeue => {
            live.ready = ready_now();
            // Woken cells and its nudge wait for the next take.
            shared.with(|s| s.sched.requeue(cpu, live));
            None
        }
        Back::File | Back::Share => {
            shared.with(|s| {
                wake_woken(s, cpu, now_ns);
                file(s, Some(cpu), live)
            });
            None
        }
    };
    let was_end = ended.is_some();
    let wake = shared.with(|s| {
        s.running -= 1;
        if let Some((finished, tag)) = ended {
            if tag != 0 {
                s.tags.free(tag);
            }
            s.ended.push(finished);
        }
        let others = s.idle & !(1 << cpu);
        if s.sched.runnable() > 1 && others != 0 {
            others & others.wrapping_neg() // the lowest idle CPU
        } else if was_end || (s.running == 0 && s.sched.runnable() == 0) {
            others & 1 // the boot CPU checks whether the run is over
        } else {
            0
        }
    });
    for other in 0..crate::smp::count() {
        if wake & (1 << other) != 0 {
            crate::smp::wake(other);
        }
    }
}

/// Files a cell whose threads may all wait: runnable if one of its threads
/// can run (back on `cpu`'s queue, or on the shortest for a cell new to the
/// scheduler), else asleep until its earliest deadline, or parked for good
/// when none of its threads has one (design D2 of `add-threads`). A cell
/// waiting on descriptors waits for a socket it looked at to change (design
/// D2 of `add-socket-wait-queues`).
fn file(s: &mut Shared, cpu: Option<usize>, mut live: Box<Live>) {
    nudge(s, &mut live);
    if cpu.is_none() {
        // New to the scheduler (forked, resumed, woken by the host), it is
        // on no wait queue yet: its threads make their calls again.
        live.cell.threads.wake_io();
    }
    let sched = &mut s.sched;
    if live.cell.threads.is_runnable() {
        live.ready = ready_now();
        match cpu {
            Some(cpu) => sched.requeue(cpu, live),
            None => sched.place(live),
        };
    } else if live.cell.threads.waits_on_io() {
        // Woken by a socket change or its earliest deadline.
        let wake = live.cell.threads.wake_at();
        live.ready = Ready::event(wake.unwrap_or(0));
        sched.wait(live.cell.id, live, wake);
    } else {
        let wake = live.cell.threads.wake_at().unwrap_or(u64::MAX);
        live.ready = Ready::event(if wake == u64::MAX { 0 } else { wake });
        sched.sleep(live, wake);
    }
}

/// Runnable since now, for the workloads' latencies (nothing while they
/// are not measured: no clock read).
fn ready_now() -> Ready {
    if prof::on() {
        Ready::queued(now_ns())
    } else {
        Ready::default()
    }
}

/// Ends `live`: drops the CPU's hold on its address space and FP/SIMD
/// state, frees its memory, and returns what the host keeps of it and the
/// tag to free.
fn end(live: Box<Live>, ending: Ending) -> (Finished, u16) {
    let _p = cost::scope(Part::Teardown);
    let live = *live;
    if fp_owner() >> 32 == live.cell.id as u64 {
        set_fp_owner(0);
    }
    user::release_user(live.cell.root, live.tag);
    sharing(live.shares, || {
        space::destroy(
            PageTable::<Format>::from_root(live.cell.root),
            &mut DirectMap,
            &mut KernelFrames,
        )
    });
    release_sockets(&live.fs, live.cell.id);
    let finished = Finished {
        id: live.cell.id,
        ending,
        output: live.output.unwrap_or_default(),
        cpus: live.cpus,
        peak: live.peak,
    };
    (finished, live.tag)
}

/// What a secondary CPU runs during [`Host::run`]: cells from the shared
/// scheduler until the run stops, with its preemption tick on.
fn run_cpu(shared: u64) -> u64 {
    // SAFETY: `Host::run` passes the address of its `shared`, and waits for
    // this function to return before it returns itself, so the reference is
    // valid for the whole call; `Global` serializes access.
    let shared = unsafe { &*(shared as *const Global<Shared>) };
    let cpu = crate::arch::cpu::index();
    irq::start_tick();
    loop {
        match take(shared, cpu, false) {
            Take::Run(live, now) => run_slice(shared, cpu, live, now, true),
            Take::Join(gang) => join_gang(shared, cpu, gang),
            Take::Wait(wake) => wait(wake),
            Take::Done | Take::Look => wait(None),
            Take::Stop => break,
        }
    }
    irq::stop_tick();
    0
}

/// The CPU's FP/SIMD owner ([`fp_key`]).
fn fp_owner() -> u64 {
    FP_OWNER[crate::arch::cpu::index()].load(Ordering::Relaxed)
}

fn set_fp_owner(key: u64) {
    FP_OWNER[crate::arch::cpu::index()].store(key, Ordering::Relaxed);
}

/// The CPUs whose TLB holds nothing stale for a cell once this CPU has
/// flushed (all of them when flushes are broadcast, design D5).
fn flushed_on(cpu: usize) -> u32 {
    if user::TLB_FLUSH_IS_BROADCAST {
        u32::MAX
    } else {
        1 << cpu
    }
}

/// The CPUs whose TLB holds nothing stale for a cell once the kernel,
/// running on this CPU, flushed some of its translations: none with KPTI,
/// where the kernel's flushes cannot reach the cell's PCID (design D3 of
/// `add-kpti`), so each CPU flushes it on its next entry.
fn flushed_by_kernel_on(cpu: usize) -> u32 {
    if user::stale_everywhere() {
        0
    } else {
        flushed_on(cpu)
    }
}

/// Runs the system call of `live`'s current thread, whose FP/SIMD state is
/// in this CPU's registers when `fp_in_cpu`, through the TLB of `gang` when
/// other CPUs may run the cell's code. Returns what the thread does next
/// and whether the cell's translations shrank. Inlined: it is the hot path
/// of `run_slice`, as it was before it was shared with gangs.
#[inline(always)]
fn syscall(
    live: &mut Live,
    pt: PageTable<Format>,
    fp_in_cpu: bool,
    gang: Option<(&Gang, usize)>,
) -> (Outcome, bool) {
    let tid = live.cell.threads.current().tid;
    let deadline = live.take_deadline(tid);
    let mut services = KernelServices {
        capture: live.output.as_mut(),
        echo: live.echo,
        random: &mut live.random,
        fp_in_cpu,
    };
    let (mut tlb, mut pager_tlb) = (CellTlb::new(gang), CellTlb::new(gang));
    let t0 = prof::syscall_start();
    // Lock order: the cell's (when shared), `SHARING`, then the sockets.
    let (outcome, deadline) = sharing(live.shares, || {
        let _call = cost::syscall(&live.cell);
        let mut sockets = CallNet::default();
        let mut cx = Context {
            pt,
            mem: &mut DirectMap,
            tlb: &mut tlb,
            frames: &mut KernelFrames,
            services: &mut services,
            fs: &mut live.fs,
            net: &mut sockets,
            deadline,
            pager: &mut CellPager(&mut live.lazy, &mut pager_tlb),
            pages: &mut KernelPages,
        };
        let outcome = live.abi.handle(&mut live.cell, &mut cx);
        let deadline = cx.deadline;
        let waits = || matches!(live.cell.threads.current().wait, Wait::Io { .. });
        if let Some((net, changes)) = sockets.0.as_mut() {
            // A call that waits on descriptors joins the wait queues of the
            // sockets it looked at (design D1 of `add-socket-wait-queues`),
            // under the lock it looked at them with.
            if net.end_call(|| waits().then_some(live.cell.id)) {
                WOKEN.store(true, Ordering::Release);
            }
            if prof::on() && net.changes() != *changes {
                prof::net_changed(now_ns());
            }
        }
        if prof::on() && core::mem::take(&mut live.io_woken) && waits() {
            prof::add(Count::IoWasted, 1);
        }
        (outcome, deadline)
    });
    prof::syscall_end(t0);
    // Only a call that blocked can have started the wait the wake-up is
    // kept for.
    if matches!(outcome, Outcome::Block) && live.host_wake {
        live.keep_host_wake();
    }
    // A call waiting on descriptors keeps its deadline for when it is made
    // again (design D3 of `add-sockets-and-poll`).
    if let Wait::Io { until: Some(_) } = live.cell.threads.current().wait
        && let Some(at) = deadline
    {
        live.deadlines.push((tid, at));
    }
    (outcome, tlb.shrank || pager_tlb.shrank)
}

impl Live {
    /// A [`Host::wake`] came before any thread waited with a deadline: the
    /// first wait with one ends at once (design D5 of
    /// `add-socket-wait-queues`).
    #[cold]
    #[inline(never)]
    fn keep_host_wake(&mut self) {
        if self.cell.threads.wake_at().is_some() {
            self.host_wake = false;
            self.cell.threads.expire_timers();
        }
    }
}

/// Runs `live`'s threads on `cpu` until the cell ends, yields, has no
/// thread that can run, or is preempted, and gives it back. With `share`
/// (several CPUs, not stepping), a cell with several threads runs as a
/// [`Gang`] instead, which other CPUs may join.
fn run_slice(shared: &Global<Shared>, cpu: usize, mut live: Box<Live>, now: u64, share: bool) {
    if share && live.cell.threads.len() > 1 {
        start_gang(shared, cpu, live, false);
        return;
    }
    if !live.cell.threads.next(now) {
        give_back(shared, cpu, live, Back::File); // nothing due yet after all
        return;
    }
    live.cpus |= 1 << cpu;
    let pt = PageTable::<Format>::from_root(live.cell.root);
    // Ticks seen in this slice: the first may have been pending since
    // before the cell was picked, so a cell is preempted at the second,
    // and always makes progress (design D4).
    let mut ticks = 0;
    // Each thread that runs in the slice (design D2 of `add-threads`).
    let back = 'thread: loop {
        let key = fp_key(live.cell.id, live.cell.threads.current().tid);
        // FP/SIMD state moves only if this thread actually uses it
        // (design D1).
        user::fp_trap(fp_owner() != key);
        loop {
            let flush = live.clean & (1 << cpu) == 0;
            let trap = {
                let _p = cost::scope(Part::Cell);
                user::enter_user(live.cell.root, live.tag, flush, live.cell.regs_mut())
            };
            live.clean |= flushed_on(cpu);
            match trap {
                Trap::Syscall => {
                    let (outcome, shrank) = syscall(&mut live, pt, fp_owner() == key, None);
                    if shrank {
                        live.clean = flushed_by_kernel_on(cpu);
                    }
                    match outcome {
                        // A new thread: other CPUs may run it (design D2 of
                        // `add-parallel-threads`).
                        Outcome::Resume if share && live.cell.threads.len() > 1 => {
                            break 'thread Back::Share;
                        }
                        Outcome::Resume => continue,
                        Outcome::Yield => break 'thread Back::Requeue,
                        // Another thread of the cell runs at once, in this
                        // slice, without changing address space.
                        Outcome::Block if live.cell.threads.next(now_ns()) => continue 'thread,
                        Outcome::Block => break 'thread Back::File,
                        Outcome::Exit(status) => break 'thread Back::End(Ending::Exit(status)),
                    }
                }
                Trap::FpUnavailable => {
                    save_fp_owner(shared, &mut live);
                    user::fp_load(&live.cell.regs().fp);
                    user::fp_trap(false);
                    set_fp_owner(key);
                    continue;
                }
                Trap::Interrupt => {
                    if cost::timer_sampling() {
                        cost::sample(live.cell.id, live.cell.regs().pc);
                    }
                    prof::add(Count::Interrupts, 1);
                    ticks += 1;
                    if ticks < cost::ticks_per_slice() {
                        continue;
                    }
                    break 'thread Back::Requeue;
                }
                Trap::Sample => {
                    cost::sample(live.cell.id, live.cell.regs().pc);
                    continue;
                }
                Trap::Fault(fault)
                    if fault.missing_page().is_some_and(|addr| {
                        live.lazy.as_mut().is_some_and(|l| {
                            let _p = cost::scope(Part::Fault);
                            l.page_in(&pt, addr)
                        })
                    }) =>
                {
                    prof::add(Count::LazyFaults, 1);
                    if live.lazy.as_ref().is_some_and(|l| l.left == 0) {
                        live.lazy = None; // fully restored: drop the snapshot
                    }
                    continue;
                }
                Trap::Fault(fault)
                    if fault
                        .missing_page()
                        .is_some_and(|addr| grow_stack(&pt, addr)) =>
                {
                    continue;
                }
                Trap::Fault(fault)
                    if fault.write_denied().is_some_and(|addr| {
                        sharing(live.shares, || {
                            resolve_write(&pt, &mut CellTlb::new(None), addr)
                        })
                    }) =>
                {
                    prof::add(Count::CowFaults, 1);
                    live.clean = flushed_by_kernel_on(cpu);
                    continue;
                }
                Trap::Fault(fault) => {
                    let mut services = KernelServices {
                        capture: live.output.as_mut(),
                        echo: live.echo,
                        random: &mut live.random,
                        fp_in_cpu: false,
                    };
                    fault.report(&mut services, live.cell.id);
                    break 'thread Back::End(Ending::Fault);
                }
            }
        }
    };
    if let Back::Share = back {
        // Every thread's FP/SIMD state goes to memory: another CPU may run
        // it next.
        sync_fp(&mut live, true);
        start_gang(shared, cpu, live, true);
        return;
    }
    if crate::smp::count() > 1 && !matches!(back, Back::End(_)) {
        // Another CPU may take the cell next: its FP/SIMD state goes to
        // memory, and this CPU lets go of its tables (design D4).
        if fp_owner() >> 32 == live.cell.id as u64 {
            sync_fp(&mut live, true);
        }
        user::leave_user();
    }
    give_back(shared, cpu, live, back);
}

/// A cell whose threads run on several CPUs at once (design D3 of
/// `add-parallel-threads`): the cell behind its own lock, and what the CPUs
/// running it read without that lock.
struct Gang {
    /// The cell, its threads and where they run: locked around system
    /// calls, faults that change its tables, and choosing threads.
    cell: Global<GangCell>,
    id: u32,
    root: u64,
    tag: u16,
    /// [`Live::clean`], read as CPUs enter the cell's code.
    clean: AtomicU32,
    /// The cell ended: the CPUs running its threads leave (design D6).
    ending: AtomicBool,
    /// Its translations are shrinking: the other CPUs keep out of its code
    /// (design D7; x86_64 only).
    hold: AtomicBool,
}

/// What a gang's lock protects.
struct GangCell {
    live: Box<Live>,
    /// The thread each CPU runs (0: none).
    busy: [u32; MAX_CPUS],
    /// How the cell ended, once it has.
    ending: Option<Ending>,
    /// What it offers to other CPUs in [`Shared::gangs`]: runnable threads
    /// no CPU runs, and the earliest deadline of its waiting threads.
    offer: (u32, Option<u64>),
}

/// A running [`Gang`], as `Shared::gangs` records it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GangPtr(NonNull<Gang>);

// SAFETY: the pointer is a handle that CPUs pass under the host's lock; the
// gang behind it is shared through its own lock and atomics only.
unsafe impl Send for GangPtr {}

/// The cell whose code each CPU runs and a sequence number, odd while it
/// runs it (`id << 32 | sequence`; design D7, x86_64 only).
static IN_CELL: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

/// What a CPU does after a section under a gang's lock.
enum After {
    /// Its thread runs on.
    Resume,
    /// Its thread stopped and was put back: this one (whose registers the
    /// CPU now holds) runs next, or the CPU leaves.
    Switch(Option<u32>),
    /// It leaves the cell (its thread was put back, or the cell ended).
    Leave,
}

impl Gang {
    /// Runs `f` under the cell's lock, then offers what the cell has for
    /// other CPUs and lets them back into its code.
    fn locked<R>(
        &self,
        shared: &Global<Shared>,
        ptr: GangPtr,
        f: impl FnOnce(&mut GangCell) -> R,
    ) -> R {
        self.cell.with(|cell| {
            let result = f(cell);
            cell.offer(shared, ptr);
            if self.hold.load(Ordering::Relaxed) {
                self.hold.store(false, Ordering::Release);
            }
            result
        })
    }

    /// Keeps every other CPU out of the cell's code until the end of the
    /// section under the cell's lock (design D7): marks them stale,
    /// interrupts those that run its code, and waits until they left it.
    /// Nothing to do where TLB invalidations are broadcast.
    fn hold_others(&self, cpu: usize) {
        if user::TLB_FLUSH_IS_BROADCAST || self.hold.swap(true, Ordering::SeqCst) {
            return;
        }
        self.clean
            .store(flushed_by_kernel_on(cpu), Ordering::SeqCst);
        let mut seen = [0u64; MAX_CPUS];
        for (other, word) in seen.iter_mut().enumerate().take(crate::smp::count()) {
            let now = IN_CELL[other].load(Ordering::SeqCst);
            if other != cpu && now >> 32 == self.id as u64 && now & 1 == 1 {
                *word = now;
                crate::smp::wake(other);
            }
        }
        for (other, &word) in seen.iter().enumerate() {
            while word != 0 && IN_CELL[other].load(Ordering::Acquire) == word {
                core::hint::spin_loop();
            }
        }
    }

    /// Runs thread code of the cell on `cpu` from `regs`; `None` when the
    /// cell ended. Where flushes are per CPU, publishes that this CPU runs
    /// the cell's code, and stays out while another CPU holds it out
    /// (design D7: either this CPU sees the hold, or the holder sees it here
    /// and interrupts it).
    fn enter(&self, cpu: usize, regs: &mut Registers) -> Option<Trap> {
        // This CPU's word while it runs the cell's code, and after.
        let (mut inside, mut outside) = (0, 0);
        if !user::TLB_FLUSH_IS_BROADCAST {
            loop {
                let sequence = (IN_CELL[cpu].load(Ordering::Relaxed) as u32).wrapping_add(1) | 1;
                inside = (self.id as u64) << 32 | sequence as u64;
                outside = (self.id as u64) << 32 | sequence.wrapping_add(1) as u64;
                IN_CELL[cpu].store(inside, Ordering::SeqCst);
                if !self.hold.load(Ordering::SeqCst) {
                    break;
                }
                IN_CELL[cpu].store(outside, Ordering::SeqCst);
                self.cell.with(|_| ()); // until the holder's section ends
            }
        }
        let trap = if self.ending.load(Ordering::Acquire) {
            None
        } else {
            let flush = self.clean.load(Ordering::SeqCst) & (1 << cpu) == 0;
            if flush {
                self.clean.fetch_or(flushed_on(cpu), Ordering::SeqCst);
            }
            let _p = cost::scope(Part::Cell);
            Some(user::enter_user(self.root, self.tag, flush, regs))
        };
        if inside != 0 {
            IN_CELL[cpu].store(outside, Ordering::Release);
        }
        trap
    }
}

impl GangCell {
    /// Records how many CPUs run the cell's threads now, if that is the
    /// most so far: each holds its thread from the moment it is chosen until
    /// it is put back, so those stints overlap.
    fn count_busy(&mut self) {
        let busy = self.busy.iter().filter(|&&tid| tid != 0).count() as u32;
        self.live.peak = self.live.peak.max(busy);
    }

    /// Chooses the next thread for `cpu` (turns by identifier, skipping the
    /// threads other CPUs run; design D5) and copies its registers into
    /// `regs`, the CPU's; `None` if there is none.
    fn choose(&mut self, cpu: usize, now: u64, regs: &mut Registers) -> Option<u32> {
        if self.ending.is_some() {
            return None;
        }
        let busy = self.busy;
        let threads = &mut self.live.cell.threads;
        let tid = threads.pick(now, self.live.last, |t| busy.contains(&t))?;
        *regs = threads.get(tid)?.regs;
        self.busy[cpu] = tid;
        self.live.last = tid;
        self.count_busy();
        self.live.cpus |= 1 << cpu;
        Some(tid)
    }

    /// Puts back thread `tid`, which `cpu` stops running: its registers
    /// `regs` (unless the table holds them already), and its FP/SIMD state
    /// if this CPU's registers hold it, before another CPU can choose it.
    fn put_back(&mut self, cpu: usize, tid: u32, regs: Option<&Registers>) {
        self.busy[cpu] = 0;
        let key = fp_key(self.live.cell.id, tid);
        let thread = self.live.cell.threads.get_mut(tid);
        if let Some(thread) = thread {
            if let Some(regs) = regs {
                thread.regs = *regs;
            }
            if fp_owner() == key {
                user::fp_save(&mut thread.regs.fp);
            }
        }
        if fp_owner() == key {
            set_fp_owner(0);
        }
    }

    /// Ends the cell: records how, and interrupts the other CPUs running its
    /// threads so they leave (design D6).
    fn end(&mut self, gang: &Gang, cpu: usize, ending: Ending) {
        self.ending.get_or_insert(ending);
        gang.ending.store(true, Ordering::Release);
        self.busy[cpu] = 0;
        for (other, &tid) in self.busy.iter().enumerate() {
            if tid != 0 {
                crate::smp::wake(other);
            }
        }
    }

    /// The system call of thread `tid`, whose registers are `regs`, running
    /// on `cpu` (design D4: it becomes the current thread for the call).
    fn syscall(&mut self, gang: &Gang, cpu: usize, tid: u32, regs: &mut Registers) -> After {
        if self.ending.is_some() {
            return After::Leave; // nothing takes effect after the end
        }
        let threads = &mut self.live.cell.threads;
        threads.make_current(tid);
        threads.current_mut().regs = *regs;
        let pt = PageTable::<Format>::from_root(gang.root);
        let fp_in_cpu = fp_owner() == fp_key(gang.id, tid);
        let (outcome, _) = syscall(&mut self.live, pt, fp_in_cpu, Some((gang, cpu)));
        match outcome {
            Outcome::Resume => {
                *regs = *self.live.cell.regs();
                After::Resume
            }
            Outcome::Yield => {
                self.put_back(cpu, tid, None);
                After::Leave
            }
            Outcome::Block => {
                self.put_back(cpu, tid, None);
                After::Switch(self.choose(cpu, now_ns(), regs))
            }
            Outcome::Exit(status) => {
                self.end(gang, cpu, Ending::Exit(status));
                After::Leave
            }
        }
    }

    /// A fault of thread code on `cpu`: resolved (a page brought in, the
    /// stack grown, a copy-on-write page copied, or a page another CPU
    /// mapped meanwhile), or the cell ends. Returns whether the thread runs
    /// on.
    fn fault(&mut self, gang: &Gang, cpu: usize, fault: &UserFault) -> bool {
        if self.ending.is_some() {
            return false;
        }
        let live = &mut *self.live;
        let pt = PageTable::<Format>::from_root(gang.root);
        if let Some(addr) = fault.missing_page() {
            if live.lazy.as_mut().is_some_and(|l| {
                let _p = cost::scope(Part::Fault);
                l.page_in(&pt, addr)
            }) {
                if live.lazy.as_ref().is_some_and(|l| l.left == 0) {
                    live.lazy = None;
                }
                return true;
            }
            if grow_stack(&pt, addr) {
                return true;
            }
            if pt.translate(&DirectMap, addr).is_some() {
                CpuTlb.flush_page(addr & !(FRAME_SIZE - 1));
                return true;
            }
        }
        if let Some(addr) = fault.write_denied()
            && sharing(live.shares, || {
                resolve_write(&pt, &mut CellTlb::new(Some((gang, cpu))), addr)
            })
        {
            return true;
        }
        let mut services = KernelServices {
            capture: live.output.as_mut(),
            echo: live.echo,
            random: &mut live.random,
            fp_in_cpu: false,
        };
        fault.report(&mut services, live.cell.id);
        self.end(gang, cpu, Ending::Fault);
        false
    }

    /// Records in `Shared::gangs` what the cell offers other CPUs, if that
    /// changed, and wakes idle CPUs for it (design D5).
    fn offer(&mut self, shared: &Global<Shared>, ptr: GangPtr) {
        let offer = if self.ending.is_some() {
            (0, None)
        } else {
            let busy = &self.busy;
            let threads = &self.live.cell.threads;
            let spare = threads
                .iter()
                .filter(|t| t.wait == Wait::Ready && !busy.contains(&t.tid))
                .count() as u32;
            (spare, threads.wake_at())
        };
        if offer == self.offer {
            return;
        }
        let new_deadline = offer.1.is_some() && offer.1 != self.offer.1;
        self.offer = offer;
        let wake = shared.with(|s| {
            s.gangs.offer(ptr, offer.0, offer.1);
            // As many idle CPUs as threads offered; one for a new deadline,
            // so that it waits no longer than that.
            let wanted = if offer.0 > 0 {
                offer.0
            } else {
                new_deadline as u32
            };
            let (mut idle, mut wake) = (s.idle, 0u32);
            for _ in 0..wanted {
                let lowest = idle & idle.wrapping_neg();
                wake |= lowest;
                idle &= !lowest;
            }
            wake
        });
        for other in 0..crate::smp::count() {
            if wake & (1 << other) != 0 {
                crate::smp::wake(other);
            }
        }
    }
}

/// Turns `live`, taken by `cpu`, into a gang that other CPUs may join, and
/// runs its threads; `keep` keeps its current thread on this CPU (it just
/// made another).
fn start_gang(shared: &Global<Shared>, cpu: usize, live: Box<Live>, keep: bool) {
    let gang = Box::new(Gang {
        id: live.cell.id,
        root: live.cell.root,
        tag: live.tag,
        clean: AtomicU32::new(live.clean),
        ending: AtomicBool::new(false),
        hold: AtomicBool::new(false),
        cell: Global::new(GangCell {
            live,
            busy: [0; MAX_CPUS],
            ending: None,
            offer: (0, None),
        }),
    });
    let ptr = GangPtr(NonNull::from(Box::leak(gang)));
    shared.with(|s| s.gangs.start(cpu, ptr));
    // SAFETY: just made from a box; only the last CPU to leave frees it.
    let gang = unsafe { ptr.0.as_ref() };
    let mut regs = Registers::default();
    let first = gang.locked(shared, ptr, |cell| {
        if !keep {
            return cell.choose(cpu, now_ns(), &mut regs);
        }
        let current = cell.live.cell.threads.current();
        let tid = current.tid;
        regs = current.regs;
        cell.busy[cpu] = tid;
        cell.live.last = tid;
        cell.count_busy();
        Some(tid)
    });
    run_gang(shared, cpu, ptr, first, regs);
}

/// Runs a thread of `ptr`, which `cpu` just joined (design D5): the one it
/// offered, unless another CPU took it meanwhile.
fn join_gang(shared: &Global<Shared>, cpu: usize, ptr: GangPtr) {
    // SAFETY: `cpu` is counted in `Shared::gangs` for this gang (it just
    // joined), so the gang stays alive until it leaves.
    let gang = unsafe { ptr.0.as_ref() };
    let mut regs = Registers::default();
    let first = gang.locked(shared, ptr, |cell| {
        // `Gangs::join` took one offered thread (or the deadline).
        match &mut cell.offer {
            (spare @ 1.., _) => *spare -= 1,
            (_, wake) => *wake = None,
        }
        cell.choose(cpu, now_ns(), &mut regs)
    });
    run_gang(shared, cpu, ptr, first, regs);
}

/// Runs threads of the gang `ptr` on `cpu`, `first` (whose registers are
/// `regs`) first, until this CPU leaves the cell: its thread blocks and no
/// other is left for it, yields, or its slice ends, or the cell ends
/// (design D1 of `add-parallel-threads`).
fn run_gang(
    shared: &Global<Shared>,
    cpu: usize,
    ptr: GangPtr,
    first: Option<u32>,
    mut regs: Registers,
) {
    // SAFETY: `cpu` is counted in `Shared::gangs` for this gang until
    // `leave_gang` below, and only the last CPU to leave frees it.
    let gang = unsafe { ptr.0.as_ref() };
    // As in `run_slice`: the first tick may predate the slice.
    let mut ticks = 0;
    let mut next = first;
    while let Some(tid) = next.take() {
        let key = fp_key(gang.id, tid);
        user::fp_trap(fp_owner() != key);
        next = loop {
            let Some(trap) = gang.enter(cpu, &mut regs) else {
                break None;
            };
            match trap {
                Trap::Syscall => {
                    match gang.locked(shared, ptr, |cell| cell.syscall(gang, cpu, tid, &mut regs)) {
                        After::Resume => continue,
                        After::Switch(next) => break next,
                        After::Leave => break None,
                    }
                }
                Trap::FpUnavailable => {
                    user::fp_load(&regs.fp);
                    user::fp_trap(false);
                    set_fp_owner(key);
                }
                Trap::Sample => cost::sample(gang.id, regs.pc),
                Trap::Interrupt => {
                    if cost::timer_sampling() {
                        cost::sample(gang.id, regs.pc);
                    }
                    ticks += 1;
                    if ticks >= cost::ticks_per_slice() && !gang.ending.load(Ordering::Acquire) {
                        // The end of this CPU's slice: it looks at the run
                        // queues again (a waiting cell goes first).
                        gang.locked(shared, ptr, |cell| cell.put_back(cpu, tid, Some(&regs)));
                        break None;
                    }
                }
                Trap::Fault(fault) => {
                    if !gang.locked(shared, ptr, |cell| cell.fault(gang, cpu, &fault)) {
                        break None;
                    }
                }
            }
        };
    }
    leave_gang(shared, cpu, ptr);
}

/// `cpu` stops running the gang `ptr`; the last CPU to leave turns it back
/// into a cell and gives it back (filed as its threads say, or ended).
fn leave_gang(shared: &Global<Shared>, cpu: usize, ptr: GangPtr) {
    // SAFETY: as in `run_gang`, `cpu` has not left yet.
    let id = unsafe { ptr.0.as_ref() }.id;
    if fp_owner() >> 32 == id as u64 {
        set_fp_owner(0); // the cell ended: its FP/SIMD state is moot
    }
    user::leave_user();
    if !shared.with(|s| s.gangs.leave(cpu, ptr)) {
        return;
    }
    // SAFETY: `ptr` came from `Box::leak` in `start_gang`, and this was the
    // last CPU counted in `Shared::gangs` for it: removed from there under
    // the lock, no other CPU can reach it any more.
    let gang = unsafe { Box::from_raw(ptr.0.as_ptr()) };
    let Gang { cell, clean, .. } = *gang;
    let GangCell {
        mut live, ending, ..
    } = cell.into_inner();
    live.clean = clean.into_inner();
    give_back(shared, cpu, live, ending.map_or(Back::File, Back::End));
}

/// Saves the CPU's FP/SIMD registers into the thread that owns them,
/// which may be a thread of `running` or, with one CPU, of a cell waiting
/// its turn (with several, a cell's state leaves the CPU with the cell).
fn save_fp_owner(shared: &Global<Shared>, running: &mut Live) {
    let owner = fp_owner();
    let (cell, tid) = ((owner >> 32) as u32, owner as u32);
    if cell == running.cell.id {
        if let Some(thread) = running.cell.threads.get_mut(tid) {
            user::fp_save(&mut thread.regs.fp);
        }
    } else if cell != 0 {
        shared.with(|s| {
            if let Some(thread) = s
                .sched
                .find_mut(|l| l.cell.id == cell)
                .and_then(|l| l.cell.threads.get_mut(tid))
            {
                user::fp_save(&mut thread.regs.fp);
            }
        });
    }
}

/// Brings the FP/SIMD state of `live`'s threads up to date in their
/// registers, if one of them has it in this CPU; with `leaving`, the
/// CPU's copy stops belonging to it.
fn sync_fp(live: &mut Live, leaving: bool) {
    let owner = fp_owner();
    if owner >> 32 == live.cell.id as u64 {
        if let Some(thread) = live.cell.threads.get_mut(owner as u32) {
            user::fp_save(&mut thread.regs.fp);
        }
        if leaving {
            set_fp_owner(0);
        }
    }
}

impl Host {
    /// An empty host for every CPU online.
    pub fn new() -> Self {
        Self {
            shared: Global::new(Shared {
                sched: Scheduler::new(crate::smp::count()),
                tags: TagPool::new(MAX_TAG.load(Ordering::Relaxed)),
                ended: Vec::new(),
                running: 0,
                idle: 0,
                stopping: false,
                nudged: BTreeSet::new(),
                gangs: Gangs::new(),
            }),
            finished: Vec::new(),
            templates: Vec::new(),
            wire_due: 0,
            wire_delay: None,
            wire_addr: None,
            look_every: None,
        }
    }

    /// Makes [`run`](Self::run) look at its stop condition at least every
    /// `ns`, and never end by itself (`None`: it ends when no cell can run
    /// again).
    #[cfg_attr(
        not(feature = "control"),
        allow(dead_code, reason = "used by the control kernel")
    )]
    pub fn look_every(&mut self, ns: Option<u64>) {
        self.look_every = ns;
    }

    /// A host whose cells get no address-space tag: every switch flushes the
    /// TLB, as before tags (the `bench-cell` baseline for `cell.switch_ns`).
    #[cfg(feature = "bench-cell")]
    fn new_untagged() -> Self {
        let mut host = Self::new();
        host.shared.get_mut().tags = TagPool::new(0);
        host
    }

    /// The shared state, between runs only (no other CPU uses it then).
    fn state(&mut self) -> &mut Shared {
        self.shared.get_mut()
    }

    /// Moves the cells that ended into [`finished`](Self::finished) (other
    /// CPUs may be adding to them).
    fn collect(&mut self) {
        let ended = self.shared.with(|s| core::mem::take(&mut s.ended));
        self.finished.extend(ended);
    }

    /// A new live cell's record: its tag, nothing flushed yet.
    fn new_live(&mut self, cell: Cell, output: bool, echo: bool, fs: Fs) -> Box<Live> {
        let tag = self.state().tags.alloc();
        if tag.is_none() {
            prof::add(Count::Untagged, 1);
        }
        Box::new(Live {
            cell,
            tag: tag.unwrap_or(0),
            clean: 0,
            cpus: 0,
            shares: false,
            abi: Abi::new(user::ABI),
            output: output.then(Vec::new),
            echo,
            random: None,
            lazy: None,
            fs,
            deadlines: Vec::new(),
            io_woken: false,
            host_wake: false,
            ready: Ready::default(),
            last: 0,
            peak: 1,
        })
    }

    /// Creates a cell from the static executable `image` with `argv` and
    /// makes it runnable. Its output goes to the console when `echo` is set
    /// and is kept in [`Finished::output`] when `capture` is set. The image
    /// is loaded once and kept: later spawns of it copy that space (design
    /// D1 of `add-image-cache`).
    pub fn spawn(
        &mut self,
        image: &'static [u8],
        argv: &[&[u8]],
        capture: bool,
        echo: bool,
    ) -> Result<u32, ElfError> {
        self.spawn_with_files(image, argv, no_files(), capture, echo)
    }

    /// [`spawn`](Self::spawn) with `files` as the cell's file system (spec
    /// `files`, initial files).
    pub fn spawn_with_files(
        &mut self,
        image: &'static [u8],
        argv: &[&[u8]],
        files: Fs,
        capture: bool,
        echo: bool,
    ) -> Result<u32, ElfError> {
        self.spawn_with_env(image, argv, &[], files, capture, echo)
    }

    /// [`spawn_with_files`](Self::spawn_with_files) with `envp` as the
    /// cell's environment.
    pub fn spawn_with_env(
        &mut self,
        image: &'static [u8],
        argv: &[&[u8]],
        envp: &[&[u8]],
        files: Fs,
        capture: bool,
        echo: bool,
    ) -> Result<u32, ElfError> {
        let _p = cost::scope(Part::Spawn);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let random = start_random();
        let kernel = user::cell_kernel_half();
        let kernel_half = user::CELL_ROOT_HAS_KERNEL_HALF.then_some(&kernel);
        let parsed = Image::parse(image, user::MACHINE)?;
        let cell = rhizo_cell::start::<Format>(
            id,
            &parsed,
            user::MACHINE,
            image_space(image, &parsed)?,
            kernel_half,
            &mut DirectMap,
            &mut KernelFrames,
            argv,
            envp,
            random,
        )?;
        let live = self.new_live(cell, capture, echo, files);
        self.state().sched.place(live);
        Ok(id)
    }

    /// Creates a cell running the executable at `path` in `files`, its
    /// file system, with `argv` and `envp` (spec `cells`, dynamically
    /// linked executables): the executable and its interpreter are mapped
    /// from the files' pages, which every cell mapping the same bytes shares
    /// (design D1 of `add-dynamic-linking`). Output as for
    /// [`spawn`](Self::spawn).
    #[cfg_attr(
        not(any(feature = "ktest", feature = "bench-cell")),
        allow(dead_code, reason = "used by in-kernel tests and benchmarks")
    )]
    pub fn spawn_file(
        &mut self,
        files: Fs,
        path: &[u8],
        argv: &[&[u8]],
        envp: &[&[u8]],
        capture: bool,
        echo: bool,
    ) -> Result<u32, ExecError> {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let random = start_random();
        let kernel = user::cell_kernel_half();
        let kernel_half = user::CELL_ROOT_HAS_KERNEL_HALF.then_some(&kernel);
        let cell = rhizo_abi::exec::<Format, _, _>(
            id,
            user::MACHINE,
            &files,
            path,
            kernel_half,
            &mut DirectMap,
            &mut NewSpace,
            &mut KernelFrames,
            &mut KernelPages,
            argv,
            envp,
            random,
        )?;
        let live = self.new_live(cell, capture, echo, files);
        self.state().sched.place(live);
        Ok(id)
    }

    /// Suspends live cell `id` into a snapshot and frees all its memory
    /// (spec `snapshots`). Its captured output stays with the host.
    pub fn suspend(&mut self, id: u32) -> Option<Snapshot> {
        let _p = cost::scope(Part::Spawn);
        // The snapshot lives in the kernel heap, and pages still pending from
        // a lazy resume need frames first: refuse what would not fit
        // (design D2 of `add-memory-accounting`); the cell keeps running.
        let live = self.state().sched.find(|l| l.cell.id == id)?;
        let mut bytes = 0;
        PageTable::<Format>::from_root(live.cell.root)
            .for_each_lower_leaf(&DirectMap, &mut |_, _, size, _| bytes += size.bytes());
        if let Some(lazy) = &live.lazy {
            bytes += 2 * lazy.pending_bytes();
        }
        if !cells_may_take(bytes) {
            return None;
        }
        let mut live = self.state().sched.remove(|l| l.cell.id == id)?;
        let pt = PageTable::<Format>::from_root(live.cell.root);
        if let Some(lazy) = live.lazy.as_mut() {
            lazy.finish(&pt);
        }
        sync_fp(&mut live, true);
        let (files, sockets) = export_sockets(&live.fs);
        let snapshot = Snapshot::capture::<Format>(
            user::MACHINE,
            &live.cell,
            &files,
            &sockets,
            now_ns(),
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
        let _p = cost::scope(Part::Spawn);
        if mode == Resume::Eager {
            let bytes = (0..snapshot.pages())
                .map(|i| snapshot.page(i).size.bytes())
                .sum();
            if !cells_may_take(bytes) {
                return Err(snapshot);
            }
        }
        let Ok(pt) = PageTable::<Format>::new(&mut DirectMap, &mut || KernelFrames.alloc()) else {
            return Err(snapshot);
        };
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        if user::CELL_ROOT_HAS_KERNEL_HALF {
            pt.share_upper_half(&mut DirectMap, &user::cell_kernel_half());
        }
        let cell = snapshot.cell(id, pt.root(), now_ns());
        let mut fs = files_of(&snapshot);
        import_sockets(&mut fs, &snapshot.sockets());
        let mut lazy = Lazy::new(snapshot);
        if mode == Resume::Eager {
            lazy.finish(&pt);
        }
        let mut live = self.new_live(cell, capture, echo, fs);
        live.lazy = (lazy.left > 0).then_some(lazy);
        file(self.state(), None, live);
        Ok(id)
    }

    /// Page-table frames of the own half of live cell `id`.
    #[cfg_attr(
        not(feature = "bench-cell"),
        allow(dead_code, reason = "benchmark metric")
    )]
    pub fn table_frames(&mut self, id: u32) -> Option<usize> {
        let sched = &mut self.state().sched;
        let live = sched.remove(|l| l.cell.id == id)?;
        let frames = PageTable::<Format>::from_root(live.cell.root).lower_half_tables(&DirectMap);
        sched.place(live);
        Some(frames)
    }

    /// Runs one scheduling step on this CPU alone: the cell that has waited
    /// longest for one slice (until it ends, yields, sleeps or is
    /// preempted), or, if every cell sleeps, a wait for the first to wake.
    /// Returns `false` when no cell is left.
    #[cfg_attr(
        not(feature = "ktest"),
        allow(dead_code, reason = "used by in-kernel tests")
    )]
    pub fn step(&mut self) -> bool {
        self.poll_wire(now_ns());
        match take(&self.shared, 0, true) {
            Take::Run(live, now) => run_slice(&self.shared, 0, live, now, false),
            Take::Join(gang) => join_gang(&self.shared, 0, gang),
            Take::Wait(wake) => {
                self.wait_or_poll(wake);
            }
            Take::Done => {
                self.wait_or_poll(None);
            }
            Take::Stop | Take::Look => {}
        }
        self.collect();
        self.live() > 0
    }

    /// What live cell `id` has printed so far (when captured).
    #[cfg_attr(
        not(feature = "ktest"),
        allow(dead_code, reason = "used by in-kernel tests")
    )]
    pub fn output(&mut self, id: u32) -> Option<&[u8]> {
        self.state()
            .sched
            .find(|l| l.cell.id == id)?
            .output
            .as_deref()
    }

    /// Takes what live cell `id` has printed since the last call (when
    /// captured): its capture starts empty again, so a cell that keeps
    /// printing is bounded per call, not for its whole life.
    #[cfg_attr(
        not(feature = "control"),
        allow(dead_code, reason = "used by the control kernel")
    )]
    pub fn take_output(&mut self, id: u32) -> Option<Vec<u8>> {
        let live = self.state().sched.find_mut(|l| l.cell.id == id)?;
        live.output.as_mut().map(core::mem::take)
    }

    /// Pages of live cell `id` still waiting in its snapshot (lazy resume).
    #[cfg_attr(
        not(feature = "ktest"),
        allow(dead_code, reason = "used by in-kernel tests")
    )]
    pub fn pending_pages(&mut self, id: u32) -> Option<usize> {
        let live = self.state().sched.find(|l| l.cell.id == id)?;
        Some(live.lazy.as_ref().map_or(0, |l| l.left))
    }

    /// Runs cells on every CPU until none is left that can ever run or
    /// `stop` holds (checked on this CPU whenever it gives up a cell or a
    /// cell ends), then waits for the other CPUs to give back theirs.
    pub fn run(&mut self, mut stop: impl FnMut(&Self) -> bool) {
        if stop(self) {
            return;
        }
        let cpus = crate::smp::count();
        if cpus > 1 {
            let shared = &self.shared as *const Global<Shared> as u64;
            for cpu in 1..cpus {
                crate::smp::post(cpu, run_cpu, shared);
            }
        }
        loop {
            self.poll_wire(now_ns());
            match take(&self.shared, 0, false) {
                Take::Run(live, now) => run_slice(&self.shared, 0, live, now, cpus > 1),
                Take::Join(gang) => join_gang(&self.shared, 0, gang),
                Take::Wait(wake) => {
                    self.wait_or_poll(wake);
                }
                Take::Look => {}
                Take::Done => {
                    if !self.wait_or_poll(None) {
                        break;
                    }
                }
                Take::Stop => break,
            }
            self.collect();
            if stop(self) {
                break;
            }
        }
        if cpus > 1 {
            self.shared.with(|s| s.stopping = true);
            for cpu in 1..cpus {
                crate::smp::wake(cpu);
            }
            for cpu in 1..cpus {
                crate::smp::join(cpu);
            }
            let state = self.state();
            state.stopping = false;
            state.idle = 0;
            debug_assert!(state.gangs.is_empty(), "every CPU left every cell");
        }
        // Between runs every live cell is in the scheduler: a cell nudged
        // after it ended is forgotten.
        let state = self.state();
        if !state.nudged.is_empty() {
            let sched = &state.sched;
            state
                .nudged
                .retain(|&id| sched.find(|l| l.cell.id == id).is_some());
        }
        self.collect();
    }

    /// The boot CPU has nothing to run: cells waiting on descriptors may wait
    /// for the network card, so it polls the card now (a cell may just have
    /// opened a connection) and, if that changed nothing, waits until `wake`
    /// or until the card wants polling (design D4 of `add-virtio-net`).
    /// Returns `false` when nothing can ever wake a cell.
    fn wait_or_poll(&mut self, wake: Option<u64>) -> bool {
        let now = now_ns();
        let waiting = self.shared.with(|s| s.sched.waiting()) > 0;
        let mut wake = match (wake, self.look_every) {
            (Some(w), Some(every)) => Some(w.min(now.saturating_add(every))),
            (w, every) => w.or(every.map(|e| now.saturating_add(e))),
        };
        if waiting && self.wire_delay.is_some() {
            self.wire_due = 0;
            self.poll_wire(now);
            if WOKEN.load(Ordering::Acquire) {
                return true; // the next take wakes them
            }
            if let Some(delay) = self.wire_delay.filter(|&d| d != u64::MAX) {
                let soon = now.saturating_add(delay.max(1));
                wake = Some(wake.map_or(soon, |w| w.min(soon)));
            }
        }
        match wake {
            Some(at) => {
                wait(Some(at));
                true
            }
            None if self.shared.with(|s| s.running) > 0 => {
                wait(None);
                true
            }
            None => false,
        }
    }

    /// Polls the network card when it is due (every scheduling step at most
    /// once per [`WIRE_PERIOD_NS`]), and reports a new address.
    fn poll_wire(&mut self, now: u64) {
        if now < self.wire_due {
            return;
        }
        let _p = cost::scope(Part::Net);
        let (delay, addr) = with_net(|net| (crate::wire::poll(net, now), net.wire_addr()));
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

    /// Forks live cell `id` into `n` children that share its memory
    /// copy-on-write (spec `fork`); returns their identifiers. A child has
    /// the cell's state, a new identifier and tag, and its own output.
    #[cfg_attr(
        not(feature = "ktest"),
        allow(dead_code, reason = "used by in-kernel tests")
    )]
    pub fn fork(&mut self, id: u32, n: usize) -> Option<Vec<u32>> {
        let _p = cost::scope(Part::Spawn);
        let (mut parent, wake) = self.state().sched.remove_with_wake(|l| l.cell.id == id)?;
        let pt = PageTable::<Format>::from_root(parent.cell.root);
        if let Some(lazy) = parent.lazy.as_mut() {
            lazy.finish(&pt);
        }
        parent.lazy = None;
        sync_fp(&mut parent, false);
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
        // TLB still caches (on other CPUs, when it next runs there).
        user::flush_user(parent.cell.root, parent.tag);
        parent.clean = flushed_by_kernel_on(crate::arch::cpu::index());
        parent.shares = true;
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
            self.state().tags.free(live.tag);
        }
        release_sockets(&live.fs, live.cell.id);
        space::destroy(
            PageTable::<Format>::from_root(live.cell.root),
            &mut DirectMap,
            &mut KernelFrames,
        );
    }

    /// A new live cell sharing `cell`'s memory copy-on-write.
    fn child_of(&mut self, cell: &Cell, fs: &Fs, capture: bool, echo: bool) -> Option<Box<Live>> {
        let pt = PageTable::<Format>::from_root(cell.root);
        let child = pt.fork_lower_half(&mut DirectMap, &mut KernelFrames).ok()?;
        let cell = Cell {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            root: child.root(),
            ..cell.clone()
        };
        let mut live = self.new_live(cell, capture, echo, fork_files(fs));
        live.shares = true;
        Some(live)
    }

    #[cfg_attr(not(feature = "ktest"), allow(dead_code, reason = "used by `fork`"))]
    fn requeue(&mut self, live: Box<Live>, wake: Option<u64>) {
        let sched = &mut self.state().sched;
        match wake {
            Some(at) => sched.sleep(live, at),
            None => {
                sched.place(live);
            }
        }
    }

    /// Freezes live cell `id` into a template: it never runs again, and
    /// [`spawn_from_template`](Self::spawn_from_template) forks it.
    #[cfg_attr(
        not(any(feature = "ktest", feature = "bench-cell")),
        allow(dead_code, reason = "used by in-kernel tests and benchmarks")
    )]
    pub fn freeze(&mut self, id: u32) -> bool {
        let _p = cost::scope(Part::Spawn);
        let Some(mut live) = self.state().sched.remove(|l| l.cell.id == id) else {
            return false;
        };
        let pt = PageTable::<Format>::from_root(live.cell.root);
        if let Some(lazy) = live.lazy.as_mut() {
            lazy.finish(&pt);
        }
        sync_fp(&mut live, true);
        user::release_user(live.cell.root, live.tag);
        if live.tag != 0 {
            self.state().tags.free(live.tag);
        }
        let Live { cell, fs, .. } = *live;
        self.templates.push((cell, fs));
        true
    }

    /// Starts a new cell from template `id`: a fork of the frozen cell, with
    /// every deadline of its threads expired (spec `fork`, templates keep no
    /// time).
    #[cfg_attr(
        not(any(feature = "ktest", feature = "bench-cell")),
        allow(dead_code, reason = "used by in-kernel tests and benchmarks")
    )]
    pub fn spawn_from_template(&mut self, id: u32, capture: bool, echo: bool) -> Option<u32> {
        let _p = cost::scope(Part::Spawn);
        let index = self.templates.iter().position(|(c, _)| c.id == id)?;
        let (cell, fs) = &self.templates[index];
        let (cell, fs) = (cell.clone(), fs.clone());
        let mut child = self.child_of(&cell, &fs, capture, echo)?;
        child.cell.threads.expire_timers();
        let child_id = child.cell.id;
        file(self.state(), None, child);
        Some(child_id)
    }

    /// Whether live cell `id` sleeps: none of its threads can run, and one
    /// waits for a deadline, not for a descriptor (a host's safe point to
    /// suspend or freeze it, design D3 of `add-agent-workload`).
    #[cfg_attr(
        not(any(feature = "ktest", feature = "bench-cell")),
        allow(dead_code, reason = "used by in-kernel tests and benchmarks")
    )]
    pub fn asleep(&mut self, id: u32) -> bool {
        self.state()
            .sched
            .find(|l| l.cell.id == id)
            .is_some_and(|l| {
                let threads = &l.cell.threads;
                !threads.is_runnable() && !threads.waits_on_io() && threads.wake_at().is_some()
            })
    }

    /// Creates or replaces file `path` with `data` in live cell `id`'s
    /// files; returns whether it could (how a host hands a forked cell its
    /// part, design D3 of `add-agent-workload`).
    #[cfg_attr(
        not(any(feature = "ktest", feature = "bench-cell")),
        allow(dead_code, reason = "used by in-kernel tests and benchmarks")
    )]
    pub fn write_file(&mut self, id: u32, path: &[u8], data: &[u8]) -> bool {
        self.state()
            .sched
            .find_mut(|l| l.cell.id == id)
            .is_some_and(|l| l.fs.put_file(path, data).is_ok())
    }

    /// Writes `data` into live cell `id`'s standard input from the host, or
    /// closes the host's end of it when `data` is `None` (spec
    /// `control-plane`, design D3 of `add-interactive-chat`), then wakes
    /// its threads waiting on descriptors: they make their calls again and
    /// find the bytes, or the end of file. `None` if the cell is not live.
    #[cfg_attr(
        not(feature = "control"),
        allow(dead_code, reason = "used by the control kernel")
    )]
    pub fn input(&mut self, id: u32, data: Option<&[u8]>) -> Option<rhizo_fs::Result<()>> {
        let mut live = self.state().sched.remove(|l| l.cell.id == id)?;
        let result = match data {
            // The bytes wait in the kernel heap, as a cell's own pipe writes.
            Some(data) if !cells_may_take(data.len() as u64) => Err(rhizo_fs::Error::NoSpace),
            Some(data) => live.fs.push_input(data),
            None => live.fs.close_input(),
        };
        // Filed back from outside the scheduler, its threads waiting on
        // descriptors make their calls again.
        file(self.state(), None, live);
        Some(result)
    }

    /// Whether live cell `id` has printed something not taken yet and
    /// waits (none of its threads can run): the moment to send its output
    /// on (design D3 of `add-interactive-chat`).
    #[cfg_attr(
        not(feature = "control"),
        allow(dead_code, reason = "used by the control kernel")
    )]
    pub fn has_output(&self, id: u32) -> bool {
        self.shared.with(|s| {
            s.sched.find(|l| l.cell.id == id).is_some_and(|l| {
                !l.cell.threads.is_runnable() && l.output.as_ref().is_some_and(|o| !o.is_empty())
            })
        })
    }

    /// Ends the sleeps of live cell `id`'s threads and times out their timed
    /// waits, as an event from outside would (the benchmark host's stand-in
    /// for a user's message, design D1 of `add-reference-workloads`);
    /// returns whether the cell exists. A cell none of whose threads waits
    /// with a deadline yet (it is still on its way to its sleep) keeps the
    /// wake-up for its next one (design D5 of `add-socket-wait-queues`).
    #[cfg_attr(
        not(feature = "bench-cell"),
        allow(dead_code, reason = "used by the reference workloads")
    )]
    pub fn wake(&mut self, id: u32) -> bool {
        let Some(mut live) = self.state().sched.remove(|l| l.cell.id == id) else {
            return false;
        };
        live.host_wake = live.cell.threads.wake_at().is_none();
        live.cell.threads.expire_timers();
        file(self.state(), None, live);
        true
    }

    /// Ends live cell `id` if it exists; returns whether it did.
    #[cfg_attr(
        not(feature = "ktest"),
        allow(dead_code, reason = "used by in-kernel tests")
    )]
    pub fn kill(&mut self, id: u32) -> bool {
        match self.state().sched.remove(|l| l.cell.id == id) {
            Some(live) => {
                self.end(live, Ending::Killed);
                true
            }
            None => false,
        }
    }

    /// How cell `id` ended, if it has.
    #[cfg_attr(
        not(any(feature = "ktest", feature = "bench-cell")),
        allow(dead_code, reason = "used by in-kernel tests and benchmarks")
    )]
    pub fn finished(&self, id: u32) -> Option<&Finished> {
        self.finished.iter().find(|f| f.id == id)
    }

    /// Whether a cell ended (or was suspended) since the records were last
    /// taken.
    #[cfg_attr(
        not(feature = "control"),
        allow(dead_code, reason = "used by the control kernel")
    )]
    pub fn has_finished(&self) -> bool {
        !self.finished.is_empty()
    }

    /// Takes the record of every cell that ended so far.
    #[cfg_attr(
        not(feature = "bench-cell"),
        allow(dead_code, reason = "used by the reference workloads")
    )]
    pub fn take_finished(&mut self) -> Vec<Finished> {
        core::mem::take(&mut self.finished)
    }

    /// Number of live cells waiting on descriptors.
    #[cfg_attr(
        not(feature = "ktest"),
        allow(dead_code, reason = "used by in-kernel tests")
    )]
    pub fn waiting(&self) -> usize {
        self.shared.with(|s| s.sched.waiting())
    }

    /// Number of live cells.
    #[cfg_attr(
        not(feature = "ktest"),
        allow(dead_code, reason = "used by in-kernel tests")
    )]
    pub fn live(&self) -> usize {
        self.shared.with(|s| s.sched.len() + s.running)
    }

    /// Ends `live` between runs and keeps what remains of it.
    fn end(&mut self, live: Box<Live>, ending: Ending) {
        self.state().nudged.remove(&live.cell.id);
        let (finished, tag) = end(live, ending);
        if tag != 0 {
            self.state().tags.free(tag);
        }
        self.finished.push(finished);
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        while let Some(live) = self.state().sched.remove(|_| true) {
            self.end(live, Ending::Killed);
        }
        for (template, fs) in self.templates.drain(..) {
            space::destroy(
                PageTable::<Format>::from_root(template.root),
                &mut DirectMap,
                &mut KernelFrames,
            );
            release_sockets(&fs, template.id);
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
        prof::idle(|| irq::wait_ns(wake - now));
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
pub fn run(image: &'static [u8], argv: &[&[u8]]) -> Result<(Ending, Vec<u8>), ElfError> {
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
#[cfg_attr(
    feature = "profile",
    allow(unreachable_code, reason = "a profile build runs other measures")
)]
pub fn bench(console: &mut impl Console) {
    use core::fmt::Write;
    #[cfg(feature = "profile")]
    return profiled(console);
    if let Some(workload) = crate::wire::options().bench {
        crate::workloads::run(console, workload);
        return;
    }
    if crate::smp::count() > 1 {
        let (serial, parallel, wake, calls_serial, calls_parallel) = multi_core();
        let (one, four, shootdown) = parallel_threads();
        let _ = writeln!(
            rhizo_hal::ConsoleWriter(console),
            "rhizo-smp cpus={} serial_ns={serial} parallel_ns={parallel} ipi_wake_ns={wake} \
             threads1_ns={one} threads4_ns={four} shootdown_ns={shootdown} \
             syscalls_serial_ns={calls_serial} syscalls_parallel_ns={calls_parallel}",
            crate::smp::count()
        );
        return; // the other metrics are measured with one CPU
    }
    benchmarks(console);
    // Again in retired instructions and in core cycles where the PMU counts
    // them (spec `profiling`): the same code measures, so the figures
    // compare.
    let mut w = rhizo_hal::ConsoleWriter(console);
    let Some(pmu) = crate::arch::pmu::init() else {
        let _ = writeln!(w, "rhizo-pmu none");
        return;
    };
    let _ = writeln!(
        w,
        "rhizo-pmu {} counters={} instructions={}",
        pmu.kind,
        pmu.counters,
        if pmu.instructions { "yes" } else { "no" }
    );
    for (clock, name) in [
        (CLOCK_INSTRUCTIONS, "instructions"),
        (CLOCK_CYCLES, "cycles"),
    ] {
        if clock == CLOCK_INSTRUCTIONS && !pmu.instructions {
            continue;
        }
        let _ = writeln!(rhizo_hal::ConsoleWriter(console), "rhizo-pass {name}");
        CLOCK.store(clock, Ordering::Relaxed);
        benchmarks(console);
    }
    CLOCK.store(CLOCK_TIME, Ordering::Relaxed);
}

/// What the benchmarks' clock counts (spec `profiling`).
#[cfg(feature = "bench-cell")]
static CLOCK: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(CLOCK_TIME);
#[cfg(feature = "bench-cell")]
const CLOCK_TIME: u8 = 0;
#[cfg(feature = "bench-cell")]
const CLOCK_INSTRUCTIONS: u8 = 1;
#[cfg(feature = "bench-cell")]
const CLOCK_CYCLES: u8 = 2;

/// The benchmarks' clock: the counter, or the PMU's retired instructions or
/// core cycles.
#[cfg(feature = "bench-cell")]
fn bench_clock() -> u64 {
    match CLOCK.load(Ordering::Relaxed) {
        CLOCK_TIME => Counter.cycles(),
        clock => crate::arch::pmu::read().map_or(0, |c| {
            if clock == CLOCK_INSTRUCTIONS {
                c.instructions.unwrap_or(0)
            } else {
                c.cycles
            }
        }),
    }
}

/// [`bench_clock`] ticks in the unit the benchmarks print: nanoseconds, or
/// instructions or cycles as counted.
#[cfg(feature = "bench-cell")]
fn bench_ns(ticks: u64) -> Option<u64> {
    match CLOCK.load(Ordering::Relaxed) {
        CLOCK_TIME => timer::cycles_to_ns(ticks),
        _ => Some(ticks),
    }
}

/// The benchmarks of one CPU, each printed as a `rhizo-…` line.
#[cfg(feature = "bench-cell")]
fn benchmarks(console: &mut impl Console) {
    use core::fmt::Write;
    let (spawn_ns, run_ns, tables) = hello_runs();
    let tagged = switch_ns(Host::new());
    let untagged = switch_ns(Host::new_untagged());
    let _ = writeln!(
        rhizo_hal::ConsoleWriter(console),
        "rhizo-cell spawn_ns={spawn_ns} run_ns={run_ns} pt_frames={tables} switch_ns={tagged} \
         switch_untagged_ns={untagged}"
    );
    // KPTI's cost (design D5 of `add-kpti`): the same figures with it
    // forced on (`cell.syscall_ns` itself is on the `rhizo-ops` line).
    let default = user::kpti();
    user::set_kpti(true);
    let syscall_kpti = bench_ns(syscall_ticks()).unwrap_or(0);
    let switch_kpti = switch_ns(Host::new());
    let (_, run_kpti, _) = hello_runs();
    user::set_kpti(default);
    let _ = writeln!(
        rhizo_hal::ConsoleWriter(console),
        "rhizo-kpti default={} syscall_kpti_ns={syscall_kpti} switch_kpti_ns={switch_kpti} \
         run_kpti_ns={run_kpti}",
        default as u8
    );
    // The first pass only: forking 50 MiB and writing it needs most of
    // the memory, which the kernel heap grown by the first pass keeps.
    if CLOCK.load(Ordering::Relaxed) == CLOCK_TIME {
        let f = fork_and_templates();
        let _ = writeln!(
            rhizo_hal::ConsoleWriter(console),
            "rhizo-fork fork_ns={} fork_touch_ns={} fork_pt_frames={} spawn_from_template_ns={}",
            f[0],
            f[1],
            f[2],
            f[3]
        );
    }
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
    if let Some((cold, warm)) = cpython() {
        let _ = writeln!(
            rhizo_hal::ConsoleWriter(console),
            "rhizo-cpython cold_ns={cold} warm_ns={warm}"
        );
    }
    match agent_figures() {
        Ok([turn, model, suspend_resume, fork]) => {
            let _ = writeln!(
                rhizo_hal::ConsoleWriter(console),
                "rhizo-agent turn_ns={turn} model_ns={model} suspend_resume_ns={suspend_resume} \
                 fork_ns={fork}"
            );
        }
        Err(why) => {
            let _ = writeln!(
                rhizo_hal::ConsoleWriter(console),
                "rhizo-agent skipped: {why}"
            );
        }
    }
    let (idle, suspended) = density();
    let _ = writeln!(
        rhizo_hal::ConsoleWriter(console),
        "rhizo-density idle_bytes={idle} suspended_bytes={suspended}"
    );
    let (create, wake) = threads();
    let _ = writeln!(
        rhizo_hal::ConsoleWriter(console),
        "rhizo-threads create_ns={create} futex_wake_ns={wake}"
    );
    let (write_read, fork_files) = files();
    let _ = writeln!(
        rhizo_hal::ConsoleWriter(console),
        "rhizo-files write_read_ns={write_read} fork_ns={fork_files}"
    );
    let (pipe, loopback, tcp_resume) = net();
    let _ = writeln!(
        rhizo_hal::ConsoleWriter(console),
        "rhizo-net pipe_rtt_ns={pipe} loopback_rtt_ns={loopback} tcp_resume_ns={tcp_resume}"
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
    let _ = writeln!(
        rhizo_hal::ConsoleWriter(console),
        "rhizo-ops syscall_ns={}",
        bench_ns(syscall_ticks()).unwrap_or(0)
    );
    // Last: its files stay decoded in the kernel heap.
    if let Some((run, template)) = dynamic() {
        let _ = writeln!(
            rhizo_hal::ConsoleWriter(console),
            "rhizo-dynamic run_ns={run} template_ns={template}"
        );
    }
}

/// `bench-cell` with `profile`: the cost of a copy-on-write fault and the
/// breakdowns of the workloads (spec `profiling`), measured apart from the
/// other benchmarks, which run without the accounting hooks.
#[cfg(all(feature = "bench-cell", feature = "profile"))]
fn profiled(console: &mut impl Console) {
    use core::fmt::Write;
    if crate::smp::count() > 1 {
        parallel_breakdown(console);
        return;
    }
    let _ = writeln!(
        rhizo_hal::ConsoleWriter(console),
        "rhizo-ops cow_fault_ns={}",
        bench_ns(cow_fault_ticks()).unwrap_or(0)
    );
    breakdowns(console);
}

/// The dynamically linked `hello` (design D8 of `add-dynamic-linking`):
/// started from the cell's files and run to its exit, its interpreter
/// mapping `libc` and relocating, after a first run that keeps the file
/// pages; and forked from a template frozen once it printed `ready`, the
/// relocations done. Robust figures over 25 runs; `None` when the files
/// were not packed.
#[cfg(feature = "bench-cell")]
fn dynamic() -> Option<(u64, u64)> {
    const RUNS: usize = 25;
    const PATH: &[u8] = b"/usr/bin/hello";
    let files = dynamic_files()?;
    let ns = |t0: u64, t1: u64| timer::cycles_to_ns(t1 - t0).unwrap_or(0);
    let mut host = Host::new();
    let mut run = Vec::with_capacity(RUNS);
    for i in 0..=RUNS {
        host.finished.clear();
        let t0 = Counter.cycles();
        let id = host
            .spawn_file(files.clone(), PATH, &[b"hello"], &[], true, false)
            .expect("spawn");
        host.run(|h| h.finished(id).is_some());
        let t = ns(t0, Counter.cycles());
        assert_eq!(host.finished(id).map(|f| f.ending), Some(Ending::Exit(0)));
        if i > 0 {
            run.push(t);
        }
    }
    let template = host
        .spawn_file(files, PATH, &[b"hello", b"wait"], &[], true, false)
        .expect("spawn");
    while host.output(template) != Some(b"ready\n") {
        host.step();
    }
    assert!(host.freeze(template));
    let mut warm = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        host.finished.clear();
        let t0 = Counter.cycles();
        let id = host
            .spawn_from_template(template, true, false)
            .expect("fork");
        host.run(|h| h.finished(id).is_some());
        warm.push(ns(t0, Counter.cycles()));
        assert_eq!(host.finished(id).map(|f| f.ending), Some(Ending::Exit(0)));
    }
    Some((robust(&mut run), robust(&mut warm)))
}

/// A real program (`libc_warm`, static glibc) answering one request: started
/// cold from its executable (warm-up included), and forked from a template
/// frozen after its warm-up. Medians over 7 rounds.
#[cfg(feature = "bench-cell")]
fn workload() -> (u64, u64) {
    const ROUNDS: usize = 7;
    let image = test_image("libc_warm");
    let ns = |t0: u64, t1: u64| bench_ns(t1 - t0).unwrap_or(0);
    let mut host = Host::new();
    let (mut cold, mut warm) = (Vec::new(), Vec::new());
    for _ in 0..ROUNDS {
        let t0 = bench_clock();
        let id = host
            .spawn(image, &[b"libc_warm"], true, false)
            .expect("spawn");
        host.run(|h| h.finished(id).is_some());
        cold.push(ns(t0, bench_clock()));
    }
    let template = host
        .spawn(image, &[b"libc_warm"], true, false)
        .expect("spawn");
    while host.output(template) != Some(b"warm\n") {
        host.step();
    }
    assert!(host.freeze(template));
    for _ in 0..ROUNDS {
        let t0 = bench_clock();
        let id = host
            .spawn_from_template(template, true, false)
            .expect("fork");
        host.run(|h| h.finished(id).is_some());
        warm.push(ns(t0, bench_clock()));
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
    let ns = |t0: u64, t1: u64| bench_ns(t1 - t0).unwrap_or(0);
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
        let t0 = bench_clock();
        let child = host.spawn_from_template(big, true, false).expect("fork");
        let t1 = bench_clock();
        frames = (before - memory::free_bytes()) / FRAME_SIZE;
        host.run(|h| h.finished(child).is_some());
        let t2 = bench_clock();
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
        let t0 = bench_clock();
        let id = host
            .spawn_from_template(hello, true, false)
            .expect("spawn from template");
        let t1 = bench_clock();
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
    let ns = |t0: u64, t1: u64| bench_ns(t1 - t0).unwrap_or(0);
    let mut host = Host::new();
    let (mut cold, mut warm) = (Vec::new(), Vec::new());
    for _ in 0..ROUNDS {
        let t0 = bench_clock();
        let id = host
            .spawn_with_files(image, &argv, files(), true, false)
            .expect("spawn");
        host.run(|h| h.finished(id).is_some());
        cold.push(ns(t0, bench_clock()));
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
        let t0 = bench_clock();
        let id = host
            .spawn_from_template(template, true, false)
            .expect("fork");
        host.run(|h| h.finished(id).is_some());
        warm.push(ns(t0, bench_clock()));
        assert_eq!(host.finished(id).map(|f| f.ending), Some(Ending::Exit(0)));
    }
    cold.sort_unstable();
    warm.sort_unstable();
    Some((cold[ROUNDS / 2], warm[ROUNDS / 2]))
}

/// The files a CPython cell starts with (`cpython-files`, packed by `xtask`:
/// the standard library under `/usr/lib/python3.13` and `/app/agent_demo.py`;
/// design D2 of `add-cpython`), decoded once and then shared copy-on-write
/// by every CPython cell; `None` when CPython was not built.
#[cfg_attr(
    not(any(feature = "ktest", feature = "bench-cell")),
    allow(dead_code, reason = "used by in-kernel tests and benchmarks")
)]
pub fn cpython_files() -> Option<Fs> {
    static FILES: Global<Option<Fs>> = Global::new(None);
    let bytes = test_image("cpython-files");
    if bytes.is_empty() {
        return None;
    }
    Some(FILES.with(|files| {
        files
            .get_or_insert_with(|| Fs::decode(bytes).expect("xtask packs a valid file system"))
            .clone()
    }))
}

/// The files of the dynamically linked `hello` (`dynamic-files`, packed by
/// `xtask` with its interpreter and libraries; change
/// `add-dynamic-linking`), decoded once and then shared copy-on-write by
/// every cell given them; `None` when they were not packed.
#[cfg_attr(
    not(any(feature = "ktest", feature = "bench-cell")),
    allow(dead_code, reason = "used by in-kernel tests and benchmarks")
)]
pub fn dynamic_files() -> Option<Fs> {
    static FILES: Global<Option<Fs>> = Global::new(None);
    decoded(&FILES, "dynamic-files")
}

/// The files of the distribution's programs (`distro-files`: `ls`, `sort`,
/// `python3`, their interpreter, libraries and data), as
/// [`dynamic_files`]; packed natively only, and embedded in test kernels
/// only.
#[cfg_attr(
    not(feature = "ktest"),
    allow(dead_code, reason = "used by in-kernel tests")
)]
pub fn distro_files() -> Option<Fs> {
    static FILES: Global<Option<Fs>> = Global::new(None);
    decoded(&FILES, "distro-files")
}

/// The file system packed as test image `name`, decoded into `slot` the
/// first time; `None` when it was not packed.
#[cfg_attr(
    not(any(feature = "ktest", feature = "bench-cell")),
    allow(dead_code, reason = "used by in-kernel tests and benchmarks")
)]
fn decoded(slot: &Global<Option<Fs>>, name: &str) -> Option<Fs> {
    let bytes = test_image(name);
    if bytes.is_empty() {
        return None;
    }
    Some(slot.with(|files| {
        files
            .get_or_insert_with(|| Fs::decode(bytes).expect("xtask packs a valid file system"))
            .clone()
    }))
}

/// CPython answering one tool call (`cells/python/agent_demo.py`, design D3
/// of `add-cpython`): started cold from its executable (interpreter start,
/// imports, warm-up) with the standard library in its files, and forked from
/// a template frozen after the warm-up. Medians over 7 rounds; `None` when
/// CPython was not built.
#[cfg(feature = "bench-cell")]
fn cpython() -> Option<(u64, u64)> {
    const ROUNDS: usize = 7;
    const ARGV: [&[u8]; 2] = [b"/usr/bin/python3", b"/app/agent_demo.py"];
    let image = test_image("cpython");
    let files = cpython_files()?;
    let ns = |t0: u64, t1: u64| bench_ns(t1 - t0).unwrap_or(0);
    let mut host = Host::new();
    let (mut cold, mut warm) = (Vec::new(), Vec::new());
    for _ in 0..ROUNDS {
        let t0 = bench_clock();
        let id = host
            .spawn_with_files(image, &ARGV, files.clone(), true, false)
            .expect("spawn");
        host.run(|h| h.finished(id).is_some());
        cold.push(ns(t0, bench_clock()));
        assert_eq!(host.finished(id).map(|f| f.ending), Some(Ending::Exit(0)));
    }
    let template = host
        .spawn_with_files(image, &ARGV, files, true, false)
        .expect("spawn");
    while host.output(template) != Some(b"warm\n") {
        host.step();
    }
    assert!(host.freeze(template));
    for _ in 0..ROUNDS {
        let t0 = bench_clock();
        let id = host
            .spawn_from_template(template, true, false)
            .expect("fork");
        host.run(|h| h.finished(id).is_some());
        warm.push(ns(t0, bench_clock()));
        assert_eq!(host.finished(id).map(|f| f.ending), Some(Ending::Exit(0)));
    }
    cold.sort_unstable();
    warm.sort_unstable();
    Some((cold[ROUNDS / 2], warm[ROUNDS / 2]))
}

/// One run of the CPython agent (`cells/python/agent_loop.py`, change
/// `add-agent-workload`).
#[cfg(any(feature = "ktest", feature = "bench-cell"))]
#[derive(Debug)]
pub struct AgentRun {
    /// Everything the agent printed: its turns, then each branch in order.
    pub transcript: Vec<u8>,
    /// Each suspend plus resume between turns.
    pub suspend_resume_ns: Vec<u64>,
    /// From freezing the agent at its decision point until every branch
    /// forked from it runs (has printed its first line).
    pub fork_ns: u64,
}

/// Branches the agent forks into at its decision point.
#[cfg(any(feature = "ktest", feature = "bench-cell"))]
const AGENT_BRANCHES: u8 = 3;

/// How the agent reports ([`agent`]).
#[cfg(any(feature = "ktest", feature = "bench-cell"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code, reason = "each build uses its own: tests or benchmarks")]
pub enum AgentOutput {
    /// Its transcript only.
    Transcript,
    /// Its transcript and its turn times.
    Timed,
}

/// Runs the CPython agent (model `model`) for `turns` turns against the
/// host's model server (`llm=` on the command line), suspending and
/// resuming it in its pause after every turn (unless `mode` is `None`);
/// then, where it prints a line ending in `fork` and sleeps, freezes it as
/// a template and starts one cell per branch from it, each told its branch
/// in `/branch` (design D3 of `add-agent-workload`), and runs them to their
/// end. An error says why the agent could not run or finish.
#[cfg(any(feature = "ktest", feature = "bench-cell"))]
pub fn agent(
    host: &mut Host,
    turns: usize,
    mode: Option<Resume>,
    model: &[u8],
    output: AgentOutput,
) -> Result<AgentRun, alloc::string::String> {
    use alloc::format;
    let files = cpython_files().ok_or("CPython not built")?;
    let (ip, port) = crate::wire::options()
        .llm
        .ok_or("no network card and model server on this VMM")?;
    host.wire_address(10_000_000_000)
        .ok_or("the network card got no address")?;
    let url = format!("http://{}.{}.{}.{}:{port}", ip[0], ip[1], ip[2], ip[3]);
    let turns_arg = format!("{turns}");
    let mut argv: Vec<&[u8]> = alloc::vec![
        b"/usr/bin/python3",
        b"/app/agent_loop.py",
        url.as_bytes(),
        model,
        b"cell",
        turns_arg.as_bytes(),
    ];
    match output {
        AgentOutput::Transcript => {}
        AgentOutput::Timed => argv.push(b"timed"),
    }
    let ns = |t0: u64, t1: u64| timer::cycles_to_ns(t1 - t0).unwrap_or(0);
    // Under emulation CPython is slow; a model that never answers must still
    // end the run.
    let limit_s = 600;
    let end = now_ns().saturating_add(limit_s * 1_000_000_000);
    let said = |host: &Host, id: u32| match host.finished(id) {
        Some(f) => format!(
            "it ended ({:?}) after: {}",
            f.ending,
            alloc::string::String::from_utf8_lossy(&f.output)
                .trim()
                .replace('\n', " | ")
        ),
        None => format!("not done in {limit_s} s"),
    };
    let mut id = host
        .spawn_with_files(test_image("cpython"), &argv, files, true, false)
        .map_err(|e| format!("cannot spawn CPython: {e:?}"))?;
    let mut run = AgentRun {
        transcript: Vec::new(),
        suspend_resume_ns: Vec::new(),
        fork_ns: 0,
    };
    // Its turns: after each, it prints the turn's line and pauses briefly;
    // the host suspends and resumes it once the line is out (a pause of a
    // millisecond can pass unseen on a loaded host, so the host does not
    // wait for it). At its decision point it waits, asleep, for `/branch`.
    let mut seen = 0;
    loop {
        let printed = |host: &mut Host, id: u32| {
            host.output(id)
                .is_some_and(|o| o.len() > seen && o.ends_with(b"\n"))
        };
        let turn_done = |host: &mut Host, id: u32| {
            host.output(id).is_some_and(|o| {
                let body = &o[..o.len() - 1];
                let start = body.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
                body[start..].starts_with(b"turn ")
            })
        };
        while !(printed(host, id) && (host.asleep(id) || turn_done(host, id))) {
            if host.finished(id).is_some() || now_ns() > end {
                return Err(said(host, id));
            }
            host.step();
        }
        if host.output(id).is_some_and(|o| o.ends_with(b"fork\n")) {
            break;
        }
        let Some(mode) = mode else {
            seen = host.output(id).map_or(0, <[u8]>::len);
            continue;
        };
        let t0 = Counter.cycles();
        let snapshot = host.suspend(id).ok_or("cannot suspend the agent")?;
        let resumed = host.resume(snapshot, mode, true, false);
        let t2 = Counter.cycles();
        run.suspend_resume_ns.push(ns(t0, t2));
        let resumed = resumed.map_err(|_| "cannot resume the agent")?;
        run.transcript
            .extend_from_slice(&host.finished(id).expect("suspended").output);
        id = resumed;
        seen = 0;
    }
    run.transcript
        .extend_from_slice(host.output(id).unwrap_or_default());
    // The decision point: one cell per branch, from the frozen agent.
    let t0 = Counter.cycles();
    if !host.freeze(id) {
        return Err("cannot freeze the agent".into());
    }
    let mut branches = Vec::new();
    for b in 0..AGENT_BRANCHES {
        let child = host
            .spawn_from_template(id, true, false)
            .ok_or("cannot fork the agent")?;
        if !host.write_file(child, b"/branch", &[b'0' + b]) {
            return Err("cannot give a branch its number".into());
        }
        branches.push(child);
    }
    let running = |host: &mut Host, child: u32| {
        let out = match host.finished(child) {
            Some(f) => Some(f.output.as_slice()),
            None => host.output(child),
        };
        out.is_some_and(|o| o.contains(&b'\n'))
    };
    while !branches.iter().all(|&c| running(host, c)) {
        if now_ns() > end {
            return Err("the branches do not run".into());
        }
        host.step();
    }
    run.fork_ns = ns(t0, Counter.cycles());
    host.run(|h| branches.iter().all(|&c| h.finished(c).is_some()) || now_ns() > end);
    for &child in &branches {
        match host.finished(child) {
            Some(f) if f.ending == Ending::Exit(0) => {
                run.transcript.extend_from_slice(&f.output);
            }
            _ => {
                let why = said(host, child);
                host.kill(child);
                return Err(format!("branch {child}: {why}"));
            }
        }
    }
    Ok(run)
}

/// The CPython agent's figures (design D4 of `add-agent-workload`), over 3
/// pairs of runs of 8 turns: medians of a turn as the agent times it and of
/// the part of it spent on the model's answer (the first turn, which
/// connects and runs cold, left out) from runs that are not suspended, as
/// the Linux process is not; of a suspend plus eager resume after every
/// turn of the other runs; and of a fork into the branches until they run,
/// from both.
#[cfg(feature = "bench-cell")]
fn agent_figures() -> Result<[u64; 4], alloc::string::String> {
    const ROUNDS: usize = 3;
    const TURNS: usize = 8;
    let mut samples: [Vec<u64>; 4] = Default::default();
    for _ in 0..ROUNDS {
        let mut host = Host::new();
        let suspended = agent(
            &mut host,
            TURNS,
            Some(Resume::Eager),
            b"fake",
            AgentOutput::Timed,
        )?;
        samples[2].extend(suspended.suspend_resume_ns);
        samples[3].push(suspended.fork_ns);
        drop(host);
        let mut host = Host::new();
        let run = agent(&mut host, TURNS, None, b"fake", AgentOutput::Timed)?;
        let text = alloc::string::String::from_utf8_lossy(&run.transcript);
        for (key, slot) in [("agent turn_ns=", 0), ("agent model_ns=", 1)] {
            let times = text
                .lines()
                .find_map(|l| l.strip_prefix(key))
                .ok_or("the agent printed no turn times")?;
            samples[slot].extend(
                times
                    .split(',')
                    .skip(1)
                    .filter_map(|t| t.parse::<u64>().ok()),
            );
        }
        samples[3].push(run.fork_ns);
    }
    let mut out = [0; 4];
    for (o, s) in out.iter_mut().zip(&mut samples) {
        s.sort_unstable();
        *o = s.get(s.len() / 2).copied().unwrap_or(0);
    }
    Ok(out)
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

/// What the `threads` cell measures in its `bench` mode (design D10 of
/// `add-threads`): creating and joining a thread, and one futex hand-off
/// between two threads.
#[cfg(feature = "bench-cell")]
fn threads() -> (u64, u64) {
    let mut host = Host::new();
    let id = host
        .spawn(test_image("threads"), &[b"threads", b"bench"], true, false)
        .expect("spawn threads");
    host.run(|h| h.finished(id).is_some());
    let done = host.finished(id).expect("ended");
    assert_eq!(
        done.ending,
        Ending::Exit(0),
        "the threads cell exits with 0"
    );
    let text = core::str::from_utf8(&done.output).unwrap_or("");
    let field = |key: &str| -> u64 {
        text.split_whitespace()
            .find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    };
    (field("create_ns"), field("futex_wake_ns"))
}

/// With several CPUs (design D8 of `add-smp-scheduling`): the time for
/// four CPU-bound cells per CPU (`spin 5`) to run to their end on this CPU
/// alone (steps) and on every CPU (a run), medians over 3 rounds; and the
/// round trip to an idle CPU (wake it, run nothing, see it done), robust
/// figure over 105.
#[cfg(feature = "bench-cell")]
fn multi_core() -> (u64, u64, u64, u64, u64) {
    const ROUNDS: usize = 3;
    let ns = |t0: u64, t1: u64| bench_ns(t1 - t0).unwrap_or(0);
    let batch = |argv: &[&[u8]], parallel: bool| {
        let image = test_image(core::str::from_utf8(argv[0]).unwrap_or("spin"));
        let mut host = Host::new();
        let ids: Vec<u32> = (0..4 * crate::smp::count())
            .map(|_| host.spawn(image, argv, false, false).expect("spawn"))
            .collect();
        let t0 = bench_clock();
        if parallel {
            host.run(|_| false);
        } else {
            while host.step() {}
        }
        let t1 = bench_clock();
        for id in ids {
            assert_eq!(host.finished(id).map(|f| f.ending), Some(Ending::Exit(0)));
        }
        ns(t0, t1)
    };
    let median = |argv: &[&[u8]], parallel: bool| {
        let mut t: Vec<u64> = (0..ROUNDS).map(|_| batch(argv, parallel)).collect();
        t.sort_unstable();
        t[ROUNDS / 2]
    };
    let spin: [&[u8]; 2] = [b"spin", b"5"];
    let (serial, parallel) = (median(&spin, false), median(&spin, true));
    // System calls of different cells (design D3 of `split-sockets-lock`):
    // each `piper` writes and reads a byte through its pipe 20,000 times.
    let piper: [&[u8]; 2] = [b"piper", b"20000"];
    let (calls_serial, calls_parallel) = (median(&piper, false), median(&piper, true));
    fn nothing(x: u64) -> u64 {
        x
    }
    let mut wake: Vec<u64> = (0..105)
        .map(|_| {
            let t0 = bench_clock();
            crate::smp::post(1, nothing, 0);
            crate::smp::join(1);
            ns(t0, bench_clock())
        })
        .collect();
    (
        serial,
        parallel,
        robust(&mut wake),
        calls_serial,
        calls_parallel,
    )
}

/// With several CPUs (design D9 of `add-parallel-threads`): `parsum` with 1
/// and with 4 threads, alone on every CPU (the time it reports, medians
/// over 3 rounds), and the median `munmap` of `shootdown bench` while its
/// other threads run on the other CPUs.
#[cfg(feature = "bench-cell")]
fn parallel_threads() -> (u64, u64, u64) {
    const ROUNDS: usize = 3;
    // Both print the figure last: `ns <n>`, `shootdown_ns=<n>`.
    let run = |argv: &[&[u8]]| -> u64 {
        let name = core::str::from_utf8(argv[0]).unwrap_or("");
        let mut host = Host::new();
        let id = host
            .spawn(test_image(name), argv, true, false)
            .expect("spawn");
        host.run(|h| h.finished(id).is_some());
        let done = host.finished(id).expect("ended");
        assert_eq!(done.ending, Ending::Exit(0), "{name}");
        let text = core::str::from_utf8(&done.output).unwrap_or("");
        let last = text.trim_end().rsplit([' ', '=']).next();
        last.and_then(|v| v.parse().ok()).unwrap_or(0)
    };
    let median = |threads: &[u8]| {
        let mut ns: Vec<u64> = (0..ROUNDS)
            .map(|_| run(&[b"parsum", threads, b"24", b"time"]))
            .collect();
        ns.sort_unstable();
        ns[ROUNDS / 2]
    };
    let (one, four) = (median(b"1"), median(b"4"));
    let shootdown = run(&[b"shootdown", b"bench", b"200"]);
    (one, four, shootdown)
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
    let ns = |t0: u64, t1: u64| bench_ns(t1 - t0).unwrap_or(0);
    let mut host = Host::new();
    let mut write_read = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        host.finished.clear();
        let t0 = bench_clock();
        let id = host.spawn(image, &[b"filer"], true, false).expect("spawn");
        host.run(|h| h.finished(id).is_some());
        write_read.push(ns(t0, bench_clock()));
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
        let t0 = bench_clock();
        let child = host.spawn_from_template(holder, true, false).expect("fork");
        fork.push(ns(t0, bench_clock()));
        assert!(host.kill(child));
    }
    (robust(&mut write_read), robust(&mut fork))
}

/// Robust figures (see [`robust`]) of the time per round trip: `piper`
/// writing one byte into a pipe and reading it back, and two `loopback`
/// cells exchanging a 64-byte message over TCP on 127.0.0.1 (design D7 of
/// `add-sockets-and-poll`); and of the time to resume a `loopback twice`
/// client suspended with its connection established until it has made one
/// more round trip and exited (design D5 of `keep-tcp-across-suspend`).
#[cfg(feature = "bench-cell")]
fn net() -> (u64, u64, u64) {
    const RUNS: usize = 3 * ROUNDS;
    const PIPE_TRIPS: u64 = 2000;
    const NET_TRIPS: u64 = 200;
    let ns = |t0: u64, t1: u64| bench_ns(t1 - t0).unwrap_or(0);
    let mut host = Host::new();
    let mut pipe = Vec::with_capacity(RUNS);
    let trips = b"2000";
    for _ in 0..RUNS {
        let t0 = bench_clock();
        let id = host
            .spawn(test_image("piper"), &[b"piper", trips], true, false)
            .expect("spawn piper");
        host.run(|h| h.finished(id).is_some());
        pipe.push(ns(t0, bench_clock()) / PIPE_TRIPS);
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
        let t0 = bench_clock();
        let client = host
            .spawn(image, &[b"loopback", b"ping", b"7400", b"200"], true, false)
            .expect("spawn client");
        host.run(|h| h.finished(server).is_some() && h.finished(client).is_some());
        loopback.push(ns(t0, bench_clock()) / NET_TRIPS);
        for id in [server, client] {
            assert_eq!(host.finished(id).map(|f| f.ending), Some(Ending::Exit(0)));
        }
    }
    let mut resume = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        host.finished.clear();
        let server = host
            .spawn(image, &[b"loopback", b"serve", b"7401", b"2"], true, false)
            .expect("spawn server");
        host.step(); // listening, waiting in `accept`
        let client = host
            .spawn(image, &[b"loopback", b"twice", b"7401"], true, false)
            .expect("spawn client");
        while host.output(client) != Some(b"once\n") {
            host.step();
        }
        let snapshot = host.suspend(client).expect("live");
        let t0 = bench_clock();
        let client = host
            .resume(snapshot, Resume::Lazy, true, false)
            .expect("resume");
        host.run(|h| h.finished(client).is_some());
        resume.push(ns(t0, bench_clock()));
        host.run(|h| h.finished(server).is_some());
        for id in [server, client] {
            assert_eq!(host.finished(id).map(|f| f.ending), Some(Ending::Exit(0)));
        }
    }
    (
        robust(&mut pipe),
        robust(&mut loopback),
        robust(&mut resume),
    )
}

/// A robust figure (see [`robust`]) of the round trip of a 64-byte message
/// between a `loopback` cell and the host's echo server, through the
/// network card (`echo=` on the command line).
#[cfg(feature = "bench-cell")]
fn wire_rtt() -> Result<u64, alloc::string::String> {
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
        let t0 = bench_clock();
        let id = host.spawn(image, &argv, true, false).expect("spawn");
        // A connection that never answers must not stop the benchmarks.
        let end = now_ns().saturating_add(10_000_000_000);
        host.run(|h| h.finished(id).is_some() || now_ns() > end);
        if host.finished(id).is_none() {
            let said = alloc::string::String::from_utf8_lossy(host.output(id).unwrap_or_default())
                .into_owned();
            host.kill(id);
            return Err(alloc::format!(
                "no answer from the echo server in 10 s: {}",
                said.trim().replace('\n', " | ")
            ));
        }
        match host.finished(id) {
            Some(f) if f.ending == Ending::Exit(0) => {}
            f => {
                let said =
                    f.map(|f| alloc::string::String::from_utf8_lossy(&f.output).into_owned());
                return Err(alloc::format!(
                    "the echo server did not answer: {}",
                    said.unwrap_or_default().trim().replace('\n', " | ")
                ));
            }
        }
        samples.push(bench_ns(bench_clock() - t0).unwrap_or(0) / TRIPS);
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
    let ns = |t0: u64, t1: u64| bench_ns(t1 - t0).unwrap_or(0);
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
            let t0 = bench_clock();
            let snapshot = host.suspend(id).expect("live");
            let t1 = bench_clock();
            bytes = snapshot.as_bytes().len();
            let mode = if eager { Resume::Eager } else { Resume::Lazy };
            let t2 = bench_clock();
            let id = host.resume(snapshot, mode, true, false).expect("resume");
            let t3 = bench_clock();
            host.run(|h| h.finished(id).is_some());
            let t4 = bench_clock();
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

/// Spawns and runs the hello cell 101 times: robust spawn and spawn-to-end
/// times (see [`robust`]), and its page-table frames.
#[cfg(feature = "bench-cell")]
fn hello_runs() -> (u64, u64, usize) {
    const RUNS: usize = 101;
    let image = test_image("hello");
    let mut spawn_ns = Vec::with_capacity(RUNS);
    let mut run_ns = Vec::with_capacity(RUNS);
    let mut tables = 0;
    let mut host = Host::new();
    for _ in 0..RUNS {
        host.finished.clear();
        let t0 = bench_clock();
        let id = host
            .spawn(image, &[b"hello"], true, false)
            .expect("the hello cell spawns");
        let t1 = bench_clock();
        tables = host.table_frames(id).unwrap_or(0);
        host.run(|_| false);
        let t2 = bench_clock();
        let done = host.finished(id).expect("the hello cell ended");
        assert_eq!(done.ending, Ending::Exit(0), "the hello cell exits with 0");
        assert_eq!(done.output, b"hello from a cell\n");
        spawn_ns.push(bench_ns(t1 - t0).unwrap_or(0));
        run_ns.push(bench_ns(t2 - t0).unwrap_or(0));
    }
    (robust(&mut spawn_ns), robust(&mut run_ns), tables)
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
        let t0 = bench_clock();
        host.run(|h| h.finished(a).is_some() && h.finished(b).is_some());
        let t1 = bench_clock();
        *round = bench_ns(t1 - t0).unwrap_or(0) / (2 * YIELDS);
    }
    rounds.sort_unstable();
    rounds[2]
}

/// Counter ticks of one `getpid` round trip (cell to kernel and back):
/// `yielder getpid 20000` run to its exit, minus `yielder getpid 0` (robust
/// over rounds, see [`robust`]).
#[cfg(feature = "bench-cell")]
fn syscall_ticks() -> u64 {
    const CALLS: u64 = 20_000;
    let mut host = Host::new();
    let mut run = |n: &[u8]| {
        let argv: [&[u8]; 3] = [b"yielder", b"getpid", n];
        let id = host
            .spawn(test_image("yielder"), &argv, false, false)
            .expect("spawn yielder");
        let t0 = bench_clock();
        host.run(|h| h.finished(id).is_some());
        bench_clock() - t0
    };
    let mut samples: Vec<u64> = (0..3 * ROUNDS)
        .map(|_| {
            let empty = run(b"0");
            run(b"20000").saturating_sub(empty)
        })
        .collect();
    robust(&mut samples) / CALLS
}

/// A frozen `ballast` cell holding `mib` MiB written, on `host`.
#[cfg(all(feature = "bench-cell", feature = "profile"))]
fn ballast_template(host: &mut Host, mib: &[u8]) -> u32 {
    let big = host
        .spawn(
            test_image("ballast"),
            &[b"ballast", b"write", mib],
            true,
            false,
        )
        .expect("spawn ballast");
    while host.output(big) != Some(b"filled\n") {
        host.step();
    }
    assert!(host.freeze(big));
    big
}

/// Counter ticks the kernel spends resolving one copy-on-write fault of a
/// 4 KiB page (frame, copy, entry and TLB flush; the trap itself is the
/// system call's round trip): that part accounted alone while a fork of a
/// cell holding 1 MiB (small pages) writes every page.
#[cfg(all(feature = "bench-cell", feature = "profile"))]
fn cow_fault_ticks() -> u64 {
    let mut host = Host::new();
    let big = ballast_template(&mut host, b"1");
    let mut samples: Vec<u64> = (0..ROUNDS)
        .map(|_| {
            let child = host.spawn_from_template(big, true, false).expect("fork");
            cost::start(1 << Part::Cow as u16);
            host.run(|h| h.finished(child).is_some());
            cost::stop(|a, _| a.inclusive(Part::Cow) / a.count(Part::Cow).max(1))
        })
        .collect();
    samples.sort_unstable();
    samples[ROUNDS / 2]
}

/// Rounds of each workload in a `sampling` build (one otherwise): more
/// samples.
#[cfg(all(feature = "bench-cell", feature = "profile"))]
const SAMPLED_ROUNDS: usize = if cfg!(feature = "sampling") { 5 } else { 1 };

/// Room for samples on the CPU that takes them (spec `profiling`).
#[cfg(all(feature = "bench-cell", feature = "sampling"))]
const SAMPLE_ROOM: usize = 1 << 16;

/// Runs `f` with every part accounted and prints its breakdown (spec
/// `profiling`): the parts, the scopes entered, and the system calls that
/// took most time. `per_trap` is the cost of one entry into cell code and
/// back, in counter ticks. A `sampling` build also samples where the cells
/// (running `image`) are and prints the samples, folded.
#[cfg(all(feature = "bench-cell", feature = "profile"))]
fn breakdown(
    console: &mut impl Console,
    workload: &str,
    #[cfg_attr(not(feature = "sampling"), allow(unused_variables))] image: &str,
    per_trap: u64,
    f: impl FnOnce(),
) {
    use core::fmt::Write;
    cost::start(cost::ALL);
    #[cfg(feature = "sampling")]
    let (sampler, period) = cost::start_sampling(SAMPLE_ROOM);
    f();
    #[cfg(feature = "sampling")]
    cost::stop_sampling(|samples| {
        let mut w = rhizo_hal::ConsoleWriter(&mut *console);
        let _ = writeln!(
            w,
            "rhizo-samples {workload} image={image} sampler={sampler} period={period} lost={}",
            samples.lost()
        );
        let _ = rhizo_prof::samples::write(&mut w, workload, samples.sorted());
    });
    cost::stop(|account, calls| {
        let ns = |t: u64| timer::cycles_to_ns(t).unwrap_or(0);
        let mut w = rhizo_hal::ConsoleWriter(console);
        let _ = rhizo_prof::Breakdown {
            workload,
            account,
            per_trap,
        }
        .write(&mut w, ns);
        let _ = write!(w, "rhizo-breakdown-calls {workload}");
        let (top, n) = calls.top::<8>();
        for &(nr, count, ticks) in &top[..n] {
            let _ = match user::ABI.call_name(nr) {
                Some(name) => write!(w, " {name:?}={count}/{}", ns(ticks)),
                None => write!(w, " {nr}={count}/{}", ns(ticks)),
            };
        }
        let _ = writeln!(w);
    });
}

/// The breakdowns of the workloads (spec `profiling`): the cell benchmark
/// loop (`hello` spawned and run 101 times), a fork of a 50 MiB cell
/// writing every page, and CPython answering one request from cold.
#[cfg(all(feature = "bench-cell", feature = "profile"))]
fn breakdowns(console: &mut impl Console) {
    // One trap's cost: cell time per entry of a cell doing nothing but
    // system calls.
    let per_trap = {
        let mut host = Host::new();
        let argv: [&[u8]; 3] = [b"yielder", b"getpid", b"20000"];
        let id = host
            .spawn(test_image("yielder"), &argv, false, false)
            .expect("spawn yielder");
        cost::start(cost::ALL);
        host.run(|h| h.finished(id).is_some());
        cost::stop(|a, _| a.time(Part::Cell) / a.count(Part::Cell).max(1))
    };
    let mut host = Host::new();
    breakdown(console, "cell_loop", "hello", per_trap, || {
        for _ in 0..101 {
            let id = host
                .spawn(test_image("hello"), &[b"hello"], true, false)
                .expect("spawn");
            host.run(|h| h.finished(id).is_some());
        }
    });
    let big = ballast_template(&mut host, b"50");
    breakdown(console, "fork_touch", "ballast", per_trap, || {
        for _ in 0..SAMPLED_ROUNDS {
            let id = host.spawn_from_template(big, true, false).expect("fork");
            host.run(|h| h.finished(id).is_some());
        }
    });
    if let Some(files) = cpython_files() {
        const ARGV: [&[u8]; 2] = [b"/usr/bin/python3", b"/app/agent_demo.py"];
        breakdown(console, "cpython_cold", "cpython", per_trap, || {
            for _ in 0..SAMPLED_ROUNDS {
                let id = host
                    .spawn_with_files(test_image("cpython"), &ARGV, files.clone(), true, false)
                    .expect("spawn");
                host.run(|h| h.finished(id).is_some());
            }
        });
    }
}

/// With several CPUs: the breakdown of sixteen CPU-bound cells on every CPU
/// (time waiting on locks included), and how often each kernel lock made a
/// CPU wait meanwhile.
#[cfg(all(feature = "bench-cell", feature = "profile"))]
fn parallel_breakdown(console: &mut impl Console) {
    use core::fmt::Write;
    let locks = |host: &Host| {
        [
            ("frames", memory::contention()),
            ("heap", crate::heap::contention()),
            ("refs", SHARED.contention()),
            ("sharing", SHARING.contention()),
            ("net", NET.contention()),
            ("sched", host.shared.contention()),
        ]
    };
    let mut host = Host::new();
    let ids: Vec<u32> = (0..4 * crate::smp::count())
        .map(|_| {
            host.spawn(test_image("spin"), &[b"spin", b"5"], false, false)
                .expect("spawn")
        })
        .collect();
    let before = locks(&host);
    breakdown(console, "parallel", "spin", 0, || host.run(|_| false));
    let after = locks(&host);
    for id in ids {
        assert_eq!(host.finished(id).map(|f| f.ending), Some(Ending::Exit(0)));
    }
    let mut w = rhizo_hal::ConsoleWriter(console);
    let _ = write!(w, "rhizo-locks");
    for ((name, a), (_, b)) in after.iter().zip(&before) {
        let _ = write!(w, " {name}={}/{}", a.waits - b.waits, a.spins - b.spins);
    }
    let _ = writeln!(w);
}

/// Makes the executable pages just written through the direct map visible to
/// instruction fetch (needed on AArch64; nothing to do on x86_64). Once per
/// loaded image: its copies share these pages, never written again (design
/// D5 of `add-image-cache`).
fn sync_code(pt: &PageTable<Format>, image: &Image) {
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
