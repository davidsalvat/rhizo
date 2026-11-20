//! The other CPUs (spec `multi-core`): their stacks, how they come online,
//! and what they do once there. The architecture starts them
//! (`arch::smp::start`) and enters [`secondary_main`] on each.
//!
//! Every CPU gets a window of [`STACK_WINDOW`] bytes at
//! `STACKS_BASE + index * STACK_WINDOW` in the kernel half, mapped page by
//! page with an unmapped guard page below each stack, so an overflow faults
//! instead of corrupting a neighbour. The boot CPU keeps the stacks in the
//! kernel image.

use crate::arch;
use crate::memory;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use rhizo_core::smp::MAX_CPUS;
use rhizo_mm::FRAME_SIZE;
use rhizo_mm::paging::PageFlags;

/// Start of the stack windows of the secondary CPUs.
const STACKS_BASE: u64 = 0xffff_ffff_c000_0000;
/// Virtual space per CPU: guard, kernel stack (64 KiB), guard, exception
/// stack (16 KiB), guard, double-fault stack (16 KiB, x86_64).
const STACK_WINDOW: u64 = 0x2_0000;
const KERNEL_STACK: u64 = 64 * 1024;
const SMALL_STACK: u64 = 16 * 1024;

/// Tops of the stacks of a secondary CPU.
#[derive(Debug, Clone, Copy)]
pub struct Stacks {
    /// The kernel stack.
    pub kernel: u64,
    /// The stack exceptions from cells and the kernel switch to (`SP_EL1`,
    /// x86_64 `RSP0`).
    pub exception: u64,
    /// The double-fault stack (x86_64 IST 1).
    pub double_fault: u64,
}

/// Where the stacks of secondary CPU `cpu` are.
pub fn stacks(cpu: usize) -> Stacks {
    assert!((1..MAX_CPUS).contains(&cpu));
    let kernel = STACKS_BASE + cpu as u64 * STACK_WINDOW + FRAME_SIZE + KERNEL_STACK;
    let exception = kernel + FRAME_SIZE + SMALL_STACK;
    Stacks {
        kernel,
        exception,
        double_fault: exception + FRAME_SIZE + SMALL_STACK,
    }
}

/// Maps the stacks of secondary CPU `cpu` (fresh frames, guard pages left
/// unmapped) and returns them.
///
/// # Panics
/// If memory for them is missing (at boot, a broken machine).
pub fn map_stacks(cpu: usize) -> Stacks {
    let stacks = stacks(cpu);
    for (top, len) in [
        (stacks.kernel, KERNEL_STACK),
        (stacks.exception, SMALL_STACK),
        (stacks.double_fault, SMALL_STACK),
    ] {
        for virt in (top - len..top).step_by(FRAME_SIZE as usize) {
            let phys = memory::with_frames(|f| f.alloc())
                .flatten()
                .expect("a frame for a CPU stack");
            memory::map_page(virt, phys, PageFlags::WRITABLE | PageFlags::NO_EXECUTE)
                .expect("map a CPU stack");
        }
    }
    stacks
}

/// The guard page below the kernel stack of CPU `cpu`.
#[cfg_attr(
    all(target_arch = "x86_64", not(feature = "ktest")),
    allow(dead_code, reason = "exception reports use it on AArch64")
)]
pub fn stack_guard(cpu: usize) -> u64 {
    match cpu {
        0 => arch::boot::layout::stack_guard(),
        _ => STACKS_BASE + cpu as u64 * STACK_WINDOW,
    }
}

/// CPUs online, the boot CPU included.
static ONLINE: AtomicUsize = AtomicUsize::new(1);

/// Work handed to one CPU: a function and its argument, then its result.
struct Mailbox {
    /// `fn(u64) -> u64` as an address; 0 when there is none.
    job: AtomicUsize,
    arg: AtomicU64,
    result: AtomicU64,
    /// The CPU reached [`secondary_main`].
    arrived: AtomicBool,
}

static MAIL: [Mailbox; MAX_CPUS] = [const {
    Mailbox {
        job: AtomicUsize::new(0),
        arg: AtomicU64::new(0),
        result: AtomicU64::new(0),
        arrived: AtomicBool::new(false),
    }
}; MAX_CPUS];

/// Starts the other CPUs, at most `limit` CPUs in all (`smp=`), and
/// returns how many are online.
pub fn start(limit: Option<u16>) -> usize {
    let online = arch::smp::start(limit);
    ONLINE.store(online, Ordering::Release);
    online
}

/// Number of CPUs online (1 until [`start`]).
pub fn count() -> usize {
    ONLINE.load(Ordering::Acquire)
}

/// Whether CPU `cpu`, just started, has reached [`secondary_main`].
pub fn arrived(cpu: usize) -> bool {
    MAIL[cpu].arrived.load(Ordering::Acquire)
}

/// Waits until CPU `cpu`, just started, reaches [`secondary_main`];
/// returns whether it did within `timeout_ns`.
pub fn wait_arrival(cpu: usize, timeout_ns: u64) -> bool {
    use rhizo_hal::CycleCounter;
    let now = || arch::timer::cycles_to_ns(arch::timer::Counter.cycles()).unwrap_or(0);
    let deadline = now() + timeout_ns;
    while !arrived(cpu) {
        if now() > deadline {
            return false;
        }
        core::hint::spin_loop();
    }
    true
}

/// Where a secondary CPU goes once its stacks, per-CPU data and interrupt
/// controller are set up: it reports itself, then idles until it is given
/// work ([`post`]), forever.
pub fn secondary_main(cpu: usize) -> ! {
    let mail = &MAIL[cpu];
    mail.arrived.store(true, Ordering::Release);
    loop {
        let job = mail.job.load(Ordering::Acquire);
        if job == 0 {
            arch::smp::idle();
            continue;
        }
        // SAFETY: `post` stores only addresses of `fn(u64) -> u64`.
        let f: fn(u64) -> u64 = unsafe { core::mem::transmute(job) };
        mail.result
            .store(f(mail.arg.load(Ordering::Relaxed)), Ordering::Relaxed);
        mail.job.store(0, Ordering::Release);
    }
}

/// Hands `f(arg)` to secondary CPU `cpu` and wakes it; [`join`] waits for
/// the result (the in-kernel tests use it to reach every CPU).
///
/// # Panics
/// If `cpu` is not an online secondary CPU, or it is busy.
#[cfg_attr(
    not(feature = "ktest"),
    allow(dead_code, reason = "used by in-kernel tests")
)]
pub fn post(cpu: usize, f: fn(u64) -> u64, arg: u64) {
    assert!((1..count()).contains(&cpu), "CPU {cpu} is not online");
    let mail = &MAIL[cpu];
    mail.arg.store(arg, Ordering::Relaxed);
    let posted = mail
        .job
        .compare_exchange(0, f as usize, Ordering::Release, Ordering::Relaxed);
    assert!(posted.is_ok(), "CPU {cpu} is busy");
    arch::smp::wake(cpu);
}

/// Waits until CPU `cpu` has run what [`post`] gave it; returns the result.
#[cfg_attr(
    not(feature = "ktest"),
    allow(dead_code, reason = "used by in-kernel tests")
)]
pub fn join(cpu: usize) -> u64 {
    let mail = &MAIL[cpu];
    while mail.job.load(Ordering::Acquire) != 0 {
        core::hint::spin_loop();
    }
    mail.result.load(Ordering::Relaxed)
}
