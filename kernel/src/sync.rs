//! Global state for a single-CPU kernel with interrupts disabled.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

/// A global value with exclusive access through [`Global::with`].
///
/// Rhizo currently runs on one CPU with interrupts disabled, so the only way
/// to observe the value twice at once is reentrance (for example, the
/// allocator being called while it is already running). That is a bug, and
/// `with` panics instead of handing out a second `&mut`.
pub struct Global<T> {
    value: UnsafeCell<T>,
    busy: AtomicBool,
}

// SAFETY: access is serialized by `busy`; on a single CPU without interrupts
// there is no concurrent access, and reentrance is detected.
unsafe impl<T: Send> Sync for Global<T> {}

impl<T> Global<T> {
    /// Creates a new global.
    pub const fn new(value: T) -> Self {
        Self {
            value: UnsafeCell::new(value),
            busy: AtomicBool::new(false),
        }
    }

    /// Runs `f` with exclusive access to the value.
    ///
    /// # Panics
    /// If called again from inside `f`.
    pub fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        assert!(
            !self.busy.swap(true, Ordering::Acquire),
            "reentrant access to a kernel global"
        );
        // SAFETY: `busy` guarantees this is the only live reference.
        let result = f(unsafe { &mut *self.value.get() });
        self.busy.store(false, Ordering::Release);
        result
    }
}
