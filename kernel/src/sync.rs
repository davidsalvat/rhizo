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
        self.0.with(crate::arch::cpu::index(), f)
    }

    /// Exclusive access to the value until the guard is dropped (taken by
    /// a holder only once it turns out to need it).
    ///
    /// # Panics
    /// If this CPU holds it already.
    pub fn lock(&self) -> rhizo_core::sync::Guard<'_, T> {
        self.0.lock(crate::arch::cpu::index())
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
