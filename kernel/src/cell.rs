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
use rhizo_abi::{
    Abi, Context, ExecError, FilePages, Outcome, PageCache, Services, Share, own_copy,
};
use rhizo_cell::elf::{ElfError, Image};
use rhizo_cell::images::{self, Images};
use rhizo_cell::memory::Pager;
use rhizo_cell::quota::{Meter, Metered, Quota};
use rhizo_cell::space::{self, Frames, WxRanges};
use rhizo_cell::threads::Wait;
use rhizo_cell::{Cell, FpState, Registers};
use rhizo_core::random::{self, Batched, Generator};
use rhizo_core::smp::MAX_CPUS;
use rhizo_fs::Fs;
use rhizo_hal::{Console, CycleCounter, Tlb};
use rhizo_mm::paging::{PageFlags, PageSize, PageTable};
use rhizo_mm::{FRAME_SIZE, LARGE_PAGE_SIZE};
use rhizo_net::Net;
use rhizo_prof::Part;
use rhizo_sched::{Fair, Gangs, IdleTimer, Keyed, SLICE_NS, Scheduler, TagPool};
use rhizo_snapshot::{Data, Snapshot};

#[cfg(any(feature = "ktest", feature = "bench-cell"))]
mod agent;
#[cfg(feature = "bench-cell")]
mod bench;
mod fault;
mod files;
mod fp;
mod frames;
mod gang;
mod host;
mod image;
mod live;
mod ops;
mod pages;
#[cfg(any(feature = "ktest", feature = "bench-cell"))]
mod pause;
mod publish;
mod sched;
mod services;
mod shares;
mod slice;
mod snapshot;
mod sockets;
mod transfer;
mod tree;

#[cfg(any(feature = "ktest", feature = "bench-cell"))]
pub use agent::*;
#[cfg(feature = "bench-cell")]
pub use bench::*;
use fault::*;
pub use files::*;
use fp::*;
use frames::*;
use gang::*;
pub use host::*;
pub use image::*;
use live::*;
pub use pages::*;
#[cfg(any(feature = "ktest", feature = "bench-cell"))]
pub use pause::*;
pub use publish::*;
use sched::*;
use services::*;
use slice::*;
pub use snapshot::*;
pub use sockets::*;

/// How long a snapshot transfer through the network card may go without
/// moving a byte.
const TRANSFER_STALL_NS: u64 = 10_000_000_000;

/// Why a snapshot transfer through the network card failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transfer {
    /// The sockets said so (no card or address: `NetUnreach`).
    Net(rhizo_net::Error),
    /// The host closed before sending every byte.
    Short,
    /// No byte moved for [`TRANSFER_STALL_NS`].
    Stalled,
}

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
    /// It used up its CPU time quota (spec `isolation`).
    OutOfCpu,
}

static NEXT_ID: AtomicU32 = AtomicU32::new(1);
/// The kernel's generator, seeded from the hardware by [`init`]: each cell
/// splits its own off it (design D2 of `add-crypto-random`).
static RANDOM: Global<Batched> = Global::new(Batched::new(Generator::from_key([0; 32])));
/// Whether [`RANDOM`] was seeded from the hardware.
static RANDOM_STRONG: AtomicBool = AtomicBool::new(false);

/// Fills `out` from the kernel's generator (after [`init`]); `false` if it
/// was not seeded from the hardware, so `out` is predictable.
#[cfg(feature = "control")]
pub fn random_bytes(out: &mut [u8]) -> bool {
    RANDOM.with(|g| g.fill(out));
    RANDOM_STRONG.load(Ordering::Relaxed)
}
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
    RANDOM_STRONG.store(strong, Ordering::Relaxed);
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

/// Waits about `ns` nanoseconds on this CPU's timer, as an idle CPU waits,
/// and returns how long the wait lasted by the counter (spec `multi-core`,
/// the same time on every CPU). A secondary CPU stops its tick again
/// afterwards, as it idles between runs. Runs on any CPU through
/// `smp::post`.
#[cfg(any(feature = "ktest", feature = "bench-cell"))]
pub fn timed_wait(ns: u64) -> u64 {
    let t0 = Counter.cycles();
    irq::wait_ns(ns);
    let took = timer::cycles_to_ns(Counter.cycles() - t0).unwrap_or(0);
    if crate::arch::cpu::index() != 0 {
        irq::stop_tick();
    }
    took
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

include!(concat!(env!("OUT_DIR"), "/cells.rs"));
