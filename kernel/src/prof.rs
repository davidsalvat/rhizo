//! Counters and latency histograms for the reference workloads (design D4
//! of `add-reference-workloads`): when a cell becomes runnable and when it
//! runs, how long system calls take, how often the CPUs wake from idle, and
//! a few counts that say where the time goes. Off (one relaxed load per
//! system call) unless a workload turns them on. Simple counters, meant to
//! give way to the permanent per-workload breakdown of F.1.

use crate::arch::timer::Counter;
use crate::sync::Global;
use alloc::boxed::Box;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use rhizo_core::smp::MAX_CPUS;
use rhizo_core::stats::Histogram;
use rhizo_hal::CycleCounter;

static ON: AtomicBool = AtomicBool::new(false);

/// Runnable to running, and event to running (nanoseconds).
static LATENCY: Global<Option<Box<[Histogram; 2]>>> = Global::new(None);
/// Time in the system-call handler, per CPU (counter cycles).
static SYSCALLS: [Global<Option<Box<Histogram>>>; MAX_CPUS] =
    [const { Global::new(None) }; MAX_CPUS];

/// Counts kept always (a relaxed add each, off any hot path) or while on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Count {
    /// A CPU came back from waiting for an interrupt (always counted).
    IdleWakeups,
    /// Cycles CPUs spent waiting for an interrupt (always counted).
    IdleCycles,
    /// Interrupts taken while cell code ran (preemption ticks, wake-ups).
    Interrupts,
    /// Cells made runnable because a socket they waited on changed.
    IoWoken,
    /// Of those, the ones whose first system call waited on descriptors
    /// again: a wake-up that found nothing.
    IoWasted,
    /// Socket changes made by system calls.
    NetChanges,
    /// Writes to copy-on-write pages resolved.
    CowFaults,
    /// Pages restored from a snapshot on first touch.
    LazyFaults,
    /// Cycles spent in scheduling decisions (taking and giving back cells).
    SchedCycles,
    /// Cells made with no address-space tag left: every switch to them
    /// flushes the TLB.
    Untagged,
}

const COUNTS: usize = 10;
static COUNTERS: [AtomicU64; COUNTS] = [const { AtomicU64::new(0) }; COUNTS];
/// When a system call last changed a socket (nanoseconds since boot).
static NET_CHANGED_AT: AtomicU64 = AtomicU64::new(0);
/// When counting started (nanoseconds since boot): a cell queued or woken
/// before then is not a latency of the window counted (cells made before a
/// workload's load starts all run at its start).
static STARTED_AT: AtomicU64 = AtomicU64::new(0);

/// Whether counting is on.
#[inline]
pub fn on() -> bool {
    ON.load(Ordering::Relaxed)
}

/// Clears every histogram and count, then counts until [`stop`].
#[cfg_attr(
    not(any(feature = "ktest", feature = "bench-cell")),
    allow(dead_code, reason = "used by in-kernel tests and benchmarks")
)]
pub fn start() {
    LATENCY.with(|l| match l {
        Some(h) => h.iter_mut().for_each(Histogram::clear),
        None => *l = Some(Box::new([Histogram::new(), Histogram::new()])),
    });
    for slot in &SYSCALLS {
        slot.with(|s| match s {
            Some(h) => h.clear(),
            None => *s = Some(Box::new(Histogram::new())),
        });
    }
    COUNTERS.iter().for_each(|c| c.store(0, Ordering::Relaxed));
    NET_CHANGED_AT.store(0, Ordering::Relaxed);
    let now = crate::arch::timer::cycles_to_ns(Counter.cycles()).unwrap_or(0);
    STARTED_AT.store(now, Ordering::Relaxed);
    ON.store(true, Ordering::Release);
}

/// Stops counting; returns what was counted since [`start`].
#[cfg_attr(
    not(any(feature = "ktest", feature = "bench-cell")),
    allow(dead_code, reason = "used by in-kernel tests and benchmarks")
)]
pub fn stop() -> Report {
    ON.store(false, Ordering::Release);
    let (sched, wake) = LATENCY.with(|l| {
        l.as_deref()
            .map_or_else(Default::default, |[s, w]| (s.clone(), w.clone()))
    });
    let mut syscalls = Histogram::new();
    for slot in &SYSCALLS {
        slot.with(|s| {
            if let Some(h) = s {
                syscalls.merge(h);
            }
        });
    }
    Report {
        sched,
        wake,
        syscall_cycles: syscalls,
        counts: core::array::from_fn(|i| COUNTERS[i].load(Ordering::Relaxed)),
    }
}

/// Adds `n` to a count (only while on, except the idle counts).
#[inline]
pub fn add(count: Count, n: u64) {
    if on() || matches!(count, Count::IdleWakeups | Count::IdleCycles) {
        COUNTERS[count as usize].fetch_add(n, Ordering::Relaxed);
    }
}

/// The value of a count kept always.
#[cfg_attr(
    not(any(feature = "ktest", feature = "bench-cell")),
    allow(dead_code, reason = "used by in-kernel tests and benchmarks")
)]
pub fn get(count: Count) -> u64 {
    COUNTERS[count as usize].load(Ordering::Relaxed)
}

/// Runs `wait` (a wait for an interrupt) and counts the wake-up.
pub fn idle(wait: impl FnOnce()) {
    let t0 = Counter.cycles();
    wait();
    add(Count::IdleCycles, Counter.cycles().saturating_sub(t0));
    add(Count::IdleWakeups, 1);
}

/// The start of a system call (cycles; 0 while off).
#[inline]
pub fn syscall_start() -> u64 {
    if on() { Counter.cycles() } else { 0 }
}

/// The end of a system call started at `t0` on this CPU.
#[inline]
pub fn syscall_end(t0: u64) {
    if t0 != 0 {
        let t = Counter.cycles().saturating_sub(t0);
        SYSCALLS[crate::arch::cpu::index()].with(|s| {
            if let Some(h) = s {
                h.record(t);
            }
        });
    }
}

/// A system call changed a socket at `now` (nanoseconds).
pub fn net_changed(now: u64) {
    NET_CHANGED_AT.store(now, Ordering::Relaxed);
    add(Count::NetChanges, 1);
}

/// When a cell's event happened (its deadline passed, a socket it waits
/// on changed) and when it joined a run queue, in nanoseconds since boot
/// (0: not known).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Ready {
    event: u64,
    queued: u64,
}

impl Ready {
    /// Runnable since `now` (preempted, yielding, new), with no event.
    pub fn queued(now: u64) -> Self {
        Self {
            event: 0,
            queued: now,
        }
    }

    /// Not runnable until an event at `at` (its deadline; 0: none).
    pub fn event(at: u64) -> Self {
        Self {
            event: at,
            queued: 0,
        }
    }

    /// Its deadline passed: it joins a run queue at `now`.
    pub fn due(self, now: u64) -> Self {
        Self {
            queued: now,
            ..self
        }
    }

    /// The last socket change woke it (or its deadline, if earlier): it
    /// joins a run queue at `now`.
    pub fn net_change(self, now: u64) -> Self {
        add(Count::IoWoken, 1);
        let changed = NET_CHANGED_AT.load(Ordering::Relaxed);
        let event = match self.event {
            0 => changed,
            d if changed == 0 => d,
            d => d.min(changed),
        };
        Self { event, queued: now }
    }

    /// The cell runs at `now`: records how long it waited for a CPU, and
    /// since its event.
    pub fn ran(self, now: u64) {
        if !on() {
            return;
        }
        let start = STARTED_AT.load(Ordering::Relaxed);
        LATENCY.with(|l| {
            if let Some([sched, wake]) = l.as_deref_mut() {
                if self.queued >= start.max(1) && self.queued <= now {
                    sched.record(now - self.queued);
                }
                if self.event >= start.max(1) && self.event <= now {
                    wake.record(now - self.event);
                }
            }
        });
    }
}

/// What was counted between [`start`] and [`stop`].
#[derive(Debug, Default)]
#[cfg_attr(
    not(any(feature = "ktest", feature = "bench-cell")),
    allow(dead_code, reason = "used by in-kernel tests and benchmarks")
)]
pub struct Report {
    /// Joined a run queue to running, in nanoseconds.
    pub sched: Histogram,
    /// Event (deadline, socket change) to running, in nanoseconds.
    pub wake: Histogram,
    /// Time in the system-call handler, in counter cycles.
    pub syscall_cycles: Histogram,
    /// Every [`Count`], by its number.
    pub counts: [u64; COUNTS],
}

#[cfg_attr(
    not(any(feature = "ktest", feature = "bench-cell")),
    allow(dead_code, reason = "used by in-kernel tests and benchmarks")
)]
impl Report {
    /// A count.
    pub fn count(&self, c: Count) -> u64 {
        self.counts[c as usize]
    }
}

/// Adds the cycles from its creation to its drop to a count, while on.
pub struct Timed(Count, u64);

impl Timed {
    /// Starts timing for `count`.
    #[inline]
    pub fn new(count: Count) -> Self {
        Self(count, if on() { Counter.cycles() } else { 0 })
    }
}

impl Drop for Timed {
    #[inline]
    fn drop(&mut self) {
        if self.1 != 0 {
            add(self.0, Counter.cycles().saturating_sub(self.1));
        }
    }
}
