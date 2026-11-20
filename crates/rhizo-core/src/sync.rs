//! The lock behind every kernel global (spec `multi-core`): a spinlock that
//! knows which CPU holds it. Another CPU waits; the holding CPU asking again
//! is reentrance, a bug, and panics instead of deadlocking (as the
//! single-CPU kernel did).
//!
//! The kernel passes its CPU number, so this logic runs unchanged in host
//! tests with threads standing for CPUs.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU32, Ordering};

/// A value shared between CPUs, reached only through [`SpinLock::with`].
#[derive(Debug)]
pub struct SpinLock<T> {
    value: UnsafeCell<T>,
    /// 0: free; else the holder's CPU number plus one.
    holder: AtomicU32,
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
        }
    }

    /// Runs `f` with exclusive access to the value, on behalf of CPU `cpu`,
    /// waiting while another CPU holds it.
    ///
    /// # Panics
    /// If `cpu` already holds it (reentrance).
    pub fn with<R>(&self, cpu: usize, f: impl FnOnce(&mut T) -> R) -> R {
        let me = cpu as u32 + 1;
        loop {
            match self
                .holder
                .compare_exchange_weak(0, me, Ordering::Acquire, Ordering::Relaxed)
            {
                Ok(_) => break,
                Err(held) if held == me => panic!("reentrant access to a kernel global"),
                Err(_) => {
                    // Wait with plain loads until it looks free (no cache
                    // line ping-pong from repeated writes).
                    while self.holder.load(Ordering::Relaxed) != 0 {
                        core::hint::spin_loop();
                    }
                }
            }
        }
        // SAFETY: `holder` is ours, so no other reference to the value exists.
        let result = f(unsafe { &mut *self.value.get() });
        self.holder.store(0, Ordering::Release);
        result
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
    fn released_after_use() {
        let lock = SpinLock::new(1);
        assert_eq!(lock.with(0, |v| *v + 1), 2);
        assert_eq!(lock.with(1, |v| *v), 1, "another CPU gets it");
    }
}
