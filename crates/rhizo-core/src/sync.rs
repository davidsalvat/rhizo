//! The lock behind every kernel global (spec `multi-core`): a spinlock that
//! knows which CPU holds it. Another CPU waits; the holding CPU asking again
//! is reentrance, a bug, and panics instead of deadlocking (as the
//! single-CPU kernel did).
//!
//! The kernel passes its CPU number, so this logic runs unchanged in host
//! tests with threads standing for CPUs.
//!
//! Each lock counts how often a CPU had to wait for it and how many times
//! it spun meanwhile (spec `profiling`): counters touched only on the
//! waiting path, so a lock that is free costs nothing more.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// A value shared between CPUs, reached only through [`SpinLock::with`].
#[derive(Debug)]
pub struct SpinLock<T> {
    value: UnsafeCell<T>,
    /// 0: free; else the holder's CPU number plus one.
    holder: AtomicU32,
    /// Acquisitions that found the lock held by another CPU.
    contended: AtomicU64,
    /// Loads of `holder` made while waiting.
    spins: AtomicU64,
}

/// How much a lock was fought over ([`SpinLock::contention`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Contention {
    /// Acquisitions that had to wait.
    pub waits: u64,
    /// Times the waiting CPUs looked at the lock again.
    pub spins: u64,
}

// SAFETY: `with` hands out the only reference to the value while `holder`
// is set, and `holder` is taken with acquire and released with release
// ordering, so accesses from different CPUs never overlap and each sees the
// previous holder's writes.
unsafe impl<T: Send> Sync for SpinLock<T> {}

impl<T> SpinLock<T> {
    /// A free lock around `value`.
    pub const fn new(value: T) -> Self {
        Self {
            value: UnsafeCell::new(value),
            holder: AtomicU32::new(0),
            contended: AtomicU64::new(0),
            spins: AtomicU64::new(0),
        }
    }

    /// Runs `f` with exclusive access to the value, on behalf of CPU `cpu`,
    /// waiting while another CPU holds it.
    ///
    /// # Panics
    /// If `cpu` already holds it (reentrance).
    pub fn with<R>(&self, cpu: usize, f: impl FnOnce(&mut T) -> R) -> R {
        f(&mut self.lock(cpu))
    }

    /// Exclusive access to the value, on behalf of CPU `cpu`, until the
    /// guard is dropped: for a holder that takes the lock only once it
    /// turns out to need it, then keeps it.
    ///
    /// # Panics
    /// If `cpu` already holds it (reentrance).
    pub fn lock(&self, cpu: usize) -> Guard<'_, T> {
        self.lock_waiting(cpu, || (), drop)
    }

    /// [`with`](Self::with), calling `wait` when the lock is found held by
    /// another CPU and `done` with its result once it is taken: the kernel
    /// measures a waiting CPU's time (spec `profiling`) without reading a
    /// clock when the lock is free.
    ///
    /// # Panics
    /// If `cpu` already holds it (reentrance).
    pub fn with_waiting<R, W>(
        &self,
        cpu: usize,
        wait: impl FnOnce() -> W,
        done: impl FnOnce(W),
        f: impl FnOnce(&mut T) -> R,
    ) -> R {
        f(&mut self.lock_waiting(cpu, wait, done))
    }

    /// [`lock`](Self::lock), with the hooks of
    /// [`with_waiting`](Self::with_waiting).
    ///
    /// # Panics
    /// If `cpu` already holds it (reentrance).
    pub fn lock_waiting<W>(
        &self,
        cpu: usize,
        wait: impl FnOnce() -> W,
        done: impl FnOnce(W),
    ) -> Guard<'_, T> {
        let me = cpu as u32 + 1;
        if self
            .holder
            .compare_exchange(0, me, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            let waiting = wait();
            while let Err(held) =
                self.holder
                    .compare_exchange_weak(0, me, Ordering::Acquire, Ordering::Relaxed)
            {
                self.wait_while_held(held, me);
            }
            done(waiting);
        }
        Guard(self)
    }

    /// The lock was found held by `held` (a CPU number plus one, 0 for a
    /// spurious failure): waits until it looks free, counting the wait.
    #[cold]
    fn wait_while_held(&self, held: u32, me: u32) {
        if held == me {
            panic!("reentrant access to a kernel global");
        }
        if held == 0 {
            return;
        }
        // Wait with plain loads until it looks free (no cache line
        // ping-pong from repeated writes).
        let mut spins = 0u64;
        while self.holder.load(Ordering::Relaxed) != 0 {
            spins += 1;
            core::hint::spin_loop();
        }
        self.contended.fetch_add(1, Ordering::Relaxed);
        self.spins.fetch_add(spins, Ordering::Relaxed);
    }

    /// How often CPUs waited for this lock, and spun meanwhile.
    pub fn contention(&self) -> Contention {
        Contention {
            waits: self.contended.load(Ordering::Relaxed),
            spins: self.spins.load(Ordering::Relaxed),
        }
    }

    /// The value, without locking: `&mut self` proves nobody else has it.
    pub fn get_mut(&mut self) -> &mut T {
        self.value.get_mut()
    }

    /// The value, the lock gone: owning it proves nobody else has it.
    pub fn into_inner(self) -> T {
        self.value.into_inner()
    }
}

/// Exclusive access to a [`SpinLock`]'s value; dropping it releases the
/// lock.
#[derive(Debug)]
pub struct Guard<'a, T>(&'a SpinLock<T>);

impl<T> core::ops::Deref for Guard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: the guard exists only while `holder` is ours, so no other
        // reference to the value exists.
        unsafe { &*self.0.value.get() }
    }
}

impl<T> core::ops::DerefMut for Guard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: as in `deref`, and `&mut self` makes this the only
        // reference through the guard.
        unsafe { &mut *self.0.value.get() }
    }
}

impl<T> Drop for Guard<'_, T> {
    fn drop(&mut self) {
        self.0.holder.store(0, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn cpus_take_turns_and_lose_no_update() {
        const CPUS: usize = 4;
        const ROUNDS: u64 = 20_000;
        let lock = Arc::new(SpinLock::new((0u64, 0u64)));
        let threads: Vec<_> = (0..CPUS)
            .map(|cpu| {
                let lock = Arc::clone(&lock);
                std::thread::spawn(move || {
                    for _ in 0..ROUNDS {
                        lock.with(cpu, |(a, b)| {
                            // Two plain updates that must stay equal.
                            *a += 1;
                            std::hint::black_box(&*a);
                            *b += 1;
                        });
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let total = CPUS as u64 * ROUNDS;
        assert_eq!(lock.with(0, |v| *v), (total, total));
    }

    #[test]
    #[should_panic(expected = "reentrant access")]
    fn reentrance_on_the_same_cpu_panics() {
        let lock = SpinLock::new(0);
        lock.with(3, |_| lock.with(3, |_| ()));
    }

    #[test]
    fn exclusive_owner_needs_no_lock() {
        let mut lock = SpinLock::new(1);
        *lock.get_mut() += 1;
        assert_eq!(lock.with(0, |v| *v), 2);
    }

    #[test]
    fn waits_are_counted_and_reported() {
        let lock = Arc::new(SpinLock::new(0u64));
        assert_eq!(lock.contention(), Contention::default());
        let (tx, rx) = std::sync::mpsc::channel();
        let other = {
            let lock = Arc::clone(&lock);
            std::thread::spawn(move || {
                // CPU 1 takes it, tells CPU 0, and holds it a while.
                lock.with(1, |v| {
                    tx.send(()).unwrap();
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    *v += 1;
                });
            })
        };
        rx.recv().unwrap();
        let (mut waited, mut done) = (0, 0);
        lock.with_waiting(0, || waited += 1, |()| done += 1, |v| *v += 1);
        other.join().unwrap();
        assert_eq!((waited, done), (1, 1), "one wait, measured once");
        let c = lock.contention();
        assert_eq!(c.waits, 1);
        assert!(c.spins > 0);
        // A free lock does not wait.
        lock.with_waiting(0, || waited += 1, |()| done += 1, |_| ());
        assert_eq!((waited, done), (1, 1));
        assert_eq!(lock.contention().waits, 1);
        assert_eq!(lock.with(0, |v| *v), 2);
    }

    #[test]
    fn the_owner_takes_the_value_out() {
        let lock = SpinLock::new(vec![1]);
        lock.with(2, |v| v.push(2));
        assert_eq!(lock.into_inner(), [1, 2]);
    }

    #[test]
    fn released_after_use() {
        let lock = SpinLock::new(1);
        assert_eq!(lock.with(0, |v| *v + 1), 2);
        assert_eq!(lock.with(1, |v| *v), 1, "another CPU gets it");
    }

    #[test]
    fn a_guard_holds_the_lock_until_dropped() {
        let lock = SpinLock::new(1);
        let mut guard = lock.lock(0);
        *guard += 1;
        assert_eq!(lock.holder.load(Ordering::Relaxed), 1, "held by CPU 0");
        drop(guard);
        assert_eq!(lock.with(1, |v| *v), 2, "released, with the change");
    }

    #[test]
    #[should_panic(expected = "reentrant access")]
    fn a_guard_is_reentrance_too() {
        let lock = SpinLock::new(0);
        let _held = lock.lock(1);
        lock.with(1, |_| ());
    }
}
