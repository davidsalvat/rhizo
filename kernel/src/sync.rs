//! Global state shared by the CPUs.

use rhizo_core::sync::SpinLock;

/// A global value with exclusive access through [`Global::with`].
///
/// The kernel runs with interrupts masked, so a CPU only waits for another
/// CPU here; the same CPU asking again is reentrance (for example, the
/// allocator being called while it is already running). That is a bug, and
/// `with` panics instead of deadlocking (`rhizo_core::sync`).
#[derive(Debug)]
pub struct Global<T>(SpinLock<T>);

impl<T> Global<T> {
    /// Creates a new global.
    pub const fn new(value: T) -> Self {
        Self(SpinLock::new(value))
    }

    /// Runs `f` with exclusive access to the value, waiting while another
    /// CPU has it.
    ///
    /// # Panics
    /// If called again from inside `f` on the same CPU.
    pub fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        // Profile builds account the time a CPU waits here (spec
        // `profiling`); the hook runs only when the lock is held elsewhere.
        #[cfg(feature = "profile")]
        return self.0.with_waiting(
            crate::arch::cpu::index(),
            || crate::cost::scope(rhizo_prof::Part::Lock),
            drop,
            f,
        );
        #[cfg(not(feature = "profile"))]
        self.0.with(crate::arch::cpu::index(), f)
    }

    /// How often CPUs waited for it.
    #[cfg_attr(
        not(feature = "bench-cell"),
        allow(dead_code, reason = "benchmark metric")
    )]
    #[cfg(feature = "profile")]
    pub fn contention(&self) -> rhizo_core::sync::Contention {
        self.0.contention()
    }

    /// The value, without locking: `&mut self` proves nobody else has it.
    #[cfg(any(feature = "ktest", feature = "bench-cell"))]
    pub fn get_mut(&mut self) -> &mut T {
        self.0.get_mut()
    }

    /// The value, the lock gone: owning the global proves nobody else has
    /// it.
    #[cfg(any(feature = "ktest", feature = "bench-cell"))]
    pub fn into_inner(self) -> T {
        self.0.into_inner()
    }
}
