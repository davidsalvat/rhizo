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
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, Ordering};
use rhizo_abi::{Abi, Context, Outcome, Services};
use rhizo_cell::Cell;
use rhizo_cell::elf::{ElfError, Image};
use rhizo_cell::space::{self, Frames};
use rhizo_hal::{Console, CycleCounter};
use rhizo_mm::FRAME_SIZE;
use rhizo_mm::paging::{PageSize, PageTable};
use rhizo_sched::{SLICE_NS, Scheduler, TagPool};

/// How a cell ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    /// It called `exit` or `exit_group` with this status.
    Exit(i32),
    /// It raised an exception and was killed.
    Fault,
    /// The kernel ended it (`Host::kill`).
    Killed,
}

/// The kernel's frame allocator, as cells see it.
#[derive(Debug)]
struct KernelFrames;

impl Frames for KernelFrames {
    fn alloc(&mut self) -> Option<u64> {
        memory::with_frames(|f| f.alloc()).flatten()
    }

    fn alloc_large(&mut self) -> Option<u64> {
        memory::with_frames(|f| f.alloc_large()).flatten()
    }

    fn free(&mut self, phys: u64, size: PageSize) {
        let frames = (size.bytes() / FRAME_SIZE) as usize;
        memory::with_frames(|f| f.free_contiguous(phys, frames));
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
    abi: Abi,
    output: Option<Vec<u8>>,
    echo: bool,
    random: u64,
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
}

impl Host {
    /// An empty host; sets up the timer interrupt the first time.
    pub fn new() -> Self {
        if !IRQ_READY.swap(true, Ordering::Relaxed) {
            irq::init(SLICE_NS);
            MAX_TAG.store(user::init_tags(), Ordering::Relaxed);
        }
        Self {
            sched: Scheduler::new(),
            tags: TagPool::new(MAX_TAG.load(Ordering::Relaxed)),
            finished: Vec::new(),
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
            abi: Abi::new(user::ABI),
            output: capture.then(Vec::new),
            echo,
            random: services.random,
        });
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

    /// Runs cells until none is left or `stop` holds (checked whenever a cell
    /// ends or gives up the CPU).
    pub fn run(&mut self, mut stop: impl FnMut(&Self) -> bool) {
        while !stop(self) {
            let now = now_ns();
            let Some(mut live) = self.sched.next(now) else {
                match self.sched.next_wake() {
                    Some(wake) => idle_until(wake),
                    None => return,
                }
                continue;
            };
            self.run_slice(&mut live);
        }
    }

    /// Runs `live` until it ends, sleeps, yields or is preempted, and files it
    /// accordingly.
    fn run_slice(&mut self, live: &mut Live) {
        let pt = PageTable::<Format>::from_root(live.cell.root);
        loop {
            match user::enter_user(live.cell.root, live.tag, &mut live.cell.regs) {
                Trap::Syscall => {
                    let mut services = KernelServices {
                        capture: live.output.as_mut(),
                        echo: live.echo,
                        random: live.random,
                    };
                    let mut cx = Context {
                        pt,
                        mem: &mut DirectMap,
                        tlb: &mut CpuTlb,
                        frames: &mut KernelFrames,
                        services: &mut services,
                    };
                    let outcome = live.abi.handle(&mut live.cell, &mut cx);
                    live.random = services.random;
                    match outcome {
                        Outcome::Resume => continue,
                        Outcome::Yield => self.sched.add(take(live)),
                        Outcome::Sleep(until) => self.sched.sleep(take(live), until),
                        Outcome::Exit(status) => self.end(take(live), Ending::Exit(status)),
                    }
                }
                Trap::Interrupt => self.sched.add(take(live)),
                Trap::Fault(fault) => {
                    let mut services = KernelServices {
                        capture: live.output.as_mut(),
                        echo: live.echo,
                        random: 0,
                    };
                    fault.report(&mut services, live.cell.id);
                    self.end(take(live), Ending::Fault);
                }
            }
            return;
        }
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

    /// Number of live cells.
    #[cfg_attr(
        not(feature = "ktest"),
        allow(dead_code, reason = "used by in-kernel tests")
    )]
    pub fn live(&self) -> usize {
        self.sched.len()
    }

    fn end(&mut self, live: Live, ending: Ending) {
        user::release_user(live.cell.root, live.tag);
        if live.tag != 0 {
            self.tags.free(live.tag);
        }
        space::destroy(
            PageTable::<Format>::from_root(live.cell.root),
            &mut DirectMap,
            &mut KernelFrames,
        );
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
    }
}

/// Moves a live cell out of a `&mut` slot (the slot is not used again).
fn take(live: &mut Live) -> Live {
    Live {
        cell: live.cell,
        tag: live.tag,
        abi: core::mem::replace(&mut live.abi, Abi::new(user::ABI)),
        output: live.output.take(),
        echo: live.echo,
        random: live.random,
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
    spawn_ns.sort_unstable();
    run_ns.sort_unstable();
    let tagged = switch_ns(Host::new());
    let untagged = switch_ns(Host::new_untagged());
    let _ = writeln!(
        rhizo_hal::ConsoleWriter(console),
        "rhizo-cell spawn_ns={} run_ns={} pt_frames={tables} switch_ns={tagged} \
         switch_untagged_ns={untagged}",
        spawn_ns[RUNS / 2],
        run_ns[RUNS / 2]
    );
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
