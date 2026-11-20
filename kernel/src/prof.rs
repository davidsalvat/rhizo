//! Cost accounting (spec `profiling`): while it is on, each CPU charges the
//! counter ticks between two hooks to the part of the kernel it was in
//! (`rhizo_prof::Account`), and system calls to their number. Accounting can
//! be on for some parts only: the other parts' hooks then do nothing, so they
//! cost the parts measured nothing.
//!
//! The hooks exist only in `profile` builds; elsewhere they are empty and
//! compile to nothing, so accounting costs the kernel nothing when it is
//! not built in (design D1 of `add-cost-accounting`).
//!
//! Accounts are behind plain spinlocks, not `Global`: `Global` itself calls
//! the lock-wait hook, and only the CPU that owns an account takes its lock
//! while accounting runs.

#[cfg(feature = "profile")]
pub use on::*;

#[cfg(not(feature = "profile"))]
pub use off::*;

#[cfg(not(feature = "profile"))]
mod off {
    use rhizo_prof::Part;

    /// An open part (nothing without `profile`).
    #[derive(Debug)]
    pub struct Scope;

    /// Enters `part` (nothing without `profile`).
    #[inline(always)]
    pub fn scope(_: Part) -> Scope {
        Scope
    }

    /// A system call being handled (nothing without `profile`).
    #[derive(Debug)]
    pub struct Call;

    /// Enters a system call's part (nothing without `profile`).
    #[inline(always)]
    pub fn syscall(_: &rhizo_cell::Cell) -> Call {
        Call
    }

    /// Records a sample (nothing without `profile`).
    #[inline(always)]
    pub fn sample(_: u32, _: u64) {}

    /// Whether timer interrupts are samples (never without `profile`).
    #[inline(always)]
    pub fn timer_sampling() -> bool {
        false
    }

    /// Timer interrupts in a time slice, the first one included.
    #[inline(always)]
    pub fn ticks_per_slice() -> u32 {
        2
    }

    /// After a CPU idled (nothing without `profile`).
    #[inline(always)]
    pub fn after_idle() {}
}

#[cfg(feature = "profile")]
mod on {
    use crate::arch::timer::Counter;
    use core::sync::atomic::{AtomicU8, AtomicU16, AtomicU32, Ordering};
    use rhizo_core::smp::MAX_CPUS;
    use rhizo_core::sync::SpinLock;
    use rhizo_hal::CycleCounter;
    use rhizo_prof::samples::{Sample, Samples};
    use rhizo_prof::{Account, Calls, Part};

    /// The parts accounted (bit `n` for part `n`).
    static ON: AtomicU16 = AtomicU16::new(0);

    /// Every part.
    pub const ALL: u16 = (1 << rhizo_prof::PARTS) - 1;

    static ACCOUNTS: [SpinLock<Account>; MAX_CPUS] =
        [const { SpinLock::new(Account::new()) }; MAX_CPUS];
    static CALLS: [SpinLock<Calls>; MAX_CPUS] = [const { SpinLock::new(Calls::new()) }; MAX_CPUS];

    fn cpu() -> usize {
        crate::arch::cpu::index()
    }

    fn now() -> u64 {
        Counter.cycles()
    }

    /// Whether `part` is accounted.
    #[inline]
    fn on(part: Part) -> bool {
        ON.load(Ordering::Relaxed) & (1 << part as u16) != 0
    }

    /// An open part, closed when dropped.
    #[must_use]
    #[derive(Debug)]
    pub struct Scope(bool);

    /// Enters `part` on this CPU until the returned scope is dropped.
    #[inline]
    pub fn scope(part: Part) -> Scope {
        if !on(part) {
            return Scope(false);
        }
        let cpu = cpu();
        ACCOUNTS[cpu].with(cpu, |a| a.enter(part, now()));
        Scope(true)
    }

    /// Leaves the innermost part; returns how long it was open.
    fn leave() -> u64 {
        let cpu = cpu();
        ACCOUNTS[cpu].with(cpu, |a| a.leave(now()))
    }

    impl Drop for Scope {
        #[inline]
        fn drop(&mut self) {
            if self.0 {
                leave();
            }
        }
    }

    /// A system call being handled ([`syscall`]).
    #[must_use]
    #[derive(Debug)]
    pub struct Call(Option<u64>);

    /// Enters the part of the system call saved in `cell`'s registers
    /// (socket calls are network time) until the returned value is dropped,
    /// and records the call's time under its number.
    #[inline]
    pub fn syscall(cell: &rhizo_cell::Cell) -> Call {
        if ON.load(Ordering::Relaxed) == 0 {
            return Call(None);
        }
        let abi = crate::arch::user::ABI;
        let nr = abi.call_number(cell);
        let part = if abi.is_socket_call(nr) {
            Part::Net
        } else {
            Part::Syscall
        };
        if !on(part) {
            return Call(None);
        }
        core::mem::forget(scope(part)); // left when the call is dropped
        Call(Some(nr))
    }

    impl Drop for Call {
        #[inline]
        fn drop(&mut self) {
            if let Some(nr) = self.0 {
                let ticks = leave();
                let cpu = cpu();
                CALLS[cpu].with(cpu, |c| c.add(nr, ticks));
            }
        }
    }

    /// Clears every CPU's account and accounts `parts` (bit `n` for part
    /// `n`; [`ALL`] for a breakdown).
    pub fn start(parts: u16) {
        let (me, t) = (cpu(), now());
        for (account, calls) in ACCOUNTS.iter().zip(&CALLS).take(crate::smp::count()) {
            account.with(me, |a| a.start(t));
            calls.with(me, Calls::clear);
        }
        ON.store(parts, Ordering::Relaxed);
    }

    /// Turns accounting off and gives `f` the accounts and calls of every
    /// CPU, merged.
    pub fn stop<R>(f: impl FnOnce(&Account, &Calls) -> R) -> R {
        ON.store(0, Ordering::Relaxed);
        let (me, t) = (cpu(), now());
        let mut total = Account::new();
        for account in &ACCOUNTS[..crate::smp::count()] {
            account.with(me, |a| {
                a.charge(t);
                total.merge(a);
            });
        }
        CALLS[0].with(me, |first| {
            for calls in &CALLS[1..crate::smp::count()] {
                calls.with(me, |c| first.merge(c));
            }
            f(&total, first)
        })
    }

    /// How samples are taken (spec `profiling`, sampling).
    const OFF: u8 = 0;
    const BY_PMU: u8 = 1;
    const BY_TIMER: u8 = 2;
    static SAMPLER: AtomicU8 = AtomicU8::new(OFF);
    /// Cycles of cell code between two samples taken by the PMU.
    #[cfg(feature = "sampling")]
    pub const PMU_PERIOD: u64 = 100_000;
    /// Nanoseconds between two samples taken by the timer.
    #[cfg(feature = "sampling")]
    pub const TIMER_PERIOD_NS: u64 = 100_000;
    /// Timer interrupts in a time slice (the first may predate the slice).
    static TICKS: AtomicU32 = AtomicU32::new(2);
    static SAMPLES: [SpinLock<Option<Samples>>; MAX_CPUS] =
        [const { SpinLock::new(None) }; MAX_CPUS];

    /// Gives this CPU room for `capacity` samples and starts its PMU's
    /// sampling; returns whether the PMU can (1) or not (0).
    #[cfg(feature = "sampling")]
    fn start_here(capacity: u64) -> u64 {
        let cpu = cpu();
        SAMPLES[cpu].with(cpu, |s| {
            *s = Some(Samples::with_capacity(capacity as usize))
        });
        use crate::arch::pmu;
        (pmu::init().is_some() && pmu::start_sampling(PMU_PERIOD)) as u64
    }

    #[cfg(feature = "sampling")]
    fn stop_here(_: u64) -> u64 {
        crate::arch::pmu::stop_sampling();
        0
    }

    /// Starts sampling where cell code is, on every CPU, with room for
    /// `capacity` samples on each: with the PMU's overflow interrupt where
    /// the boot CPU has one, else with the timer, ticking every
    /// [`TIMER_PERIOD_NS`] meanwhile. Returns how (`pmu` or `timer`) and the
    /// period (cycles or ns). Call between runs (the other CPUs idle).
    #[cfg(feature = "sampling")]
    pub fn start_sampling(capacity: usize) -> (&'static str, u64) {
        let cpus = crate::smp::count();
        if start_here(capacity as u64) == 1 {
            for other in 1..cpus {
                crate::smp::post(other, start_here, capacity as u64);
                crate::smp::join(other);
            }
            SAMPLER.store(BY_PMU, Ordering::Relaxed);
            return ("pmu", PMU_PERIOD);
        }
        for other in 1..cpus {
            // Room only: their ticks start with their next run.
            crate::smp::post(other, start_here, capacity as u64);
            crate::smp::join(other);
            crate::smp::post(other, stop_here, 0);
            crate::smp::join(other);
        }
        let ticks = (rhizo_sched::SLICE_NS / TIMER_PERIOD_NS) as u32 + 1;
        TICKS.store(ticks, Ordering::Relaxed);
        SAMPLER.store(BY_TIMER, Ordering::Relaxed);
        crate::arch::irq::set_tick_ns(TIMER_PERIOD_NS);
        ("timer", TIMER_PERIOD_NS)
    }

    /// Stops sampling and gives `f` the samples of every CPU.
    #[cfg(feature = "sampling")]
    pub fn stop_sampling<R>(f: impl FnOnce(&mut Samples) -> R) -> R {
        let cpus = crate::smp::count();
        match SAMPLER.swap(OFF, Ordering::Relaxed) {
            BY_PMU => {
                stop_here(0);
                for other in 1..cpus {
                    crate::smp::post(other, stop_here, 0);
                    crate::smp::join(other);
                }
            }
            BY_TIMER => {
                crate::arch::irq::set_tick_ns(rhizo_sched::SLICE_NS);
                TICKS.store(2, Ordering::Relaxed);
            }
            _ => {}
        }
        let me = cpu();
        let mut all = Samples::with_capacity(
            SAMPLES[..cpus]
                .iter()
                .map(|s| s.with(me, |s| s.as_ref().map_or(0, Samples::len)))
                .sum(),
        );
        for s in &SAMPLES[..cpus] {
            s.with(me, |s| {
                if let Some(s) = s.take() {
                    all.append(&s);
                }
            });
        }
        f(&mut all)
    }

    /// Records that cell `cell` was at `pc` when a sample was taken.
    pub fn sample(cell: u32, pc: u64) {
        let cpu = cpu();
        SAMPLES[cpu].with(cpu, |s| {
            if let Some(s) = s {
                s.push(Sample { cell, pc });
            }
        });
    }

    /// Whether timer interrupts are samples.
    #[inline]
    pub fn timer_sampling() -> bool {
        SAMPLER.load(Ordering::Relaxed) == BY_TIMER
    }

    /// Timer interrupts in a time slice, the first one included.
    #[inline]
    pub fn ticks_per_slice() -> u32 {
        TICKS.load(Ordering::Relaxed)
    }

    /// After a CPU idled: an overflow taken while it waited (and lost) does
    /// not stop the PMU's sampling.
    #[inline]
    pub fn after_idle() {
        if SAMPLER.load(Ordering::Relaxed) == BY_PMU {
            crate::arch::pmu::rearm();
        }
    }
}
