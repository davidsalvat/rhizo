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
}

#[cfg(feature = "profile")]
mod on {
    use crate::arch::timer::Counter;
    use core::sync::atomic::{AtomicU16, Ordering};
    use rhizo_core::smp::MAX_CPUS;
    use rhizo_core::sync::SpinLock;
    use rhizo_hal::CycleCounter;
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
}
