//! Per-cell quotas (spec `isolation`, design of `add-cell-quotas`): what
//! the host lets one cell use, and the accounting that enforces memory and
//! CPU time. Quotas are plain data in the [`Cell`](crate::Cell), so forks,
//! templates and snapshots carry them.

use core::cell::Cell as Counter;
use core::sync::atomic::{AtomicU64, Ordering};
use rhizo_mm::FRAME_SIZE;
use rhizo_mm::cow::Frames;
use rhizo_mm::paging::PageSize;

/// Most descriptors any cell may have open (`rhizo_fs::FD_MAX`).
pub const FILES_MAX: u32 = 1024;

/// Sockets a cell may hold unless the host sets another quota
/// (`rhizo_fs::SOCKETS_DEFAULT`, finding S21): with the server ends of its
/// own connections, which the kernel makes at `connect`, a cell holds at
/// most 512 of the kernel's 4096 sockets.
pub const SOCKETS_DEFAULT: u32 = 256;

/// What a cell may use. Hitting a limit fails the call that needed more
/// (`ENOMEM`, `EMFILE`, `ENFILE`), except CPU time, which ends the cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quota {
    /// Bytes of memory the cell's pages and page tables may take (counted
    /// in whole frames).
    pub memory: u64,
    /// CPU time in nanoseconds, its system calls included and across
    /// suspend and resume, counted while the cell has this quota (a quota
    /// set later counts from then).
    pub cpu_ns: u64,
    /// Open descriptors (at most [`FILES_MAX`]).
    pub files: u32,
    /// Sockets held (open socket descriptions).
    pub sockets: u32,
}

impl Quota {
    /// No limit beyond the kernel's own: all of memory down to its reserve,
    /// unbounded CPU time, [`FILES_MAX`] descriptors, any number of sockets.
    pub const NONE: Self = Self {
        memory: u64::MAX,
        cpu_ns: u64::MAX,
        files: FILES_MAX,
        sockets: u32::MAX,
    };

    /// The memory limit in frames.
    pub fn frames(&self) -> u64 {
        self.memory / FRAME_SIZE
    }

    /// The descriptor limit, never above [`FILES_MAX`].
    pub fn files(&self) -> u32 {
        self.files.min(FILES_MAX)
    }
}

impl Quota {
    /// What a new cell may use: [`NONE`](Self::NONE), but at most
    /// [`SOCKETS_DEFAULT`] sockets, so no cell holds a large share of the
    /// kernel's.
    pub const DEFAULT: Self = Self {
        sockets: SOCKETS_DEFAULT,
        ..Self::NONE
    };
}

impl Default for Quota {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// One cell's memory meter: the frames counted against it and its limit
/// in frames (its memory quota). It lives with the cell, so taking frames
/// for it costs one pointer to the meter. Kernel memory held for the cell
/// other than its frames (file data and names, thread and socket records,
/// socket queues: design D4 of `add-cell-hierarchy`) is counted in bytes
/// and takes whole frames of the count.
///
/// While a meter with a limit lives, the part of its limit it does not
/// use is set aside for it in the kernel's [`RESERVE`].
#[derive(Debug)]
pub struct Meter {
    used: Counter<u64>,
    limit: Counter<u64>,
    /// Bytes of kernel memory counted besides frames (their whole frames
    /// are in `used`).
    heap: Counter<u64>,
}

/// Memory set aside for the cells of reserved domains (design D4 of
/// `add-cell-hierarchy`): what the limits of every live [`Meter`] leave
/// unused, and what the kernel holds for suspended cells. The kernel's
/// caches and cells without a limit may not take it.
pub static RESERVE: Reserve = Reserve::new();

/// The frames a meter with `used` and `limit` sets aside in the
/// [`Reserve`]: what its limit leaves, none without a limit.
#[inline(always)]
fn held(used: u64, limit: u64) -> u64 {
    if limit < Quota::NONE.frames() {
        limit.saturating_sub(used)
    } else {
        0
    }
}

impl Meter {
    /// A meter with `used` frames counted and a limit of `limit` frames.
    pub fn new(used: u64, limit: u64) -> Self {
        let held = held(used, limit);
        if held != 0 {
            RESERVE.hold(held);
        }
        Self {
            used: Counter::new(used),
            limit: Counter::new(limit),
            heap: Counter::new(0),
        }
    }

    /// Frames counted.
    #[inline]
    pub fn used(&self) -> u64 {
        self.used.get()
    }

    /// Sets the frames counted.
    #[inline]
    pub fn set_used(&self, used: u64) {
        // Without a limit (most cells), one comparison.
        let limit = self.limit.get();
        if limit < Quota::NONE.frames() {
            let was = self.used.get();
            RESERVE.shift(limit.saturating_sub(was), limit.saturating_sub(used));
        }
        self.used.set(used);
    }

    /// The limit in frames.
    #[inline]
    pub fn limit(&self) -> u64 {
        self.limit.get()
    }

    /// Sets the limit in frames.
    #[inline]
    pub fn set_limit(&self, limit: u64) {
        self.set(self.used.get(), limit);
    }

    #[inline(always)]
    fn set(&self, used: u64, limit: u64) {
        RESERVE.shift(held(self.used.get(), self.limit.get()), held(used, limit));
        self.used.set(used);
        self.limit.set(limit);
    }

    /// Counts `bytes` of kernel memory besides frames; `false`, counting
    /// nothing, when that would take the count past the limit.
    pub fn charge(&self, bytes: u64) -> bool {
        let heap = self.heap.get();
        let Some(more) = heap.checked_add(bytes) else {
            return false;
        };
        let frames = more.div_ceil(FRAME_SIZE) - heap.div_ceil(FRAME_SIZE);
        match self.used().checked_add(frames) {
            Some(used) if used <= self.limit() => {
                self.heap.set(more);
                self.set_used(used);
                true
            }
            _ => false,
        }
    }

    /// Gives back `bytes` counted by [`charge`](Self::charge), never more
    /// than were (what a cell inherited was never counted for it).
    pub fn credit(&self, bytes: u64) {
        let heap = self.heap.get();
        let less = heap.saturating_sub(bytes);
        let frames = heap.div_ceil(FRAME_SIZE) - less.div_ceil(FRAME_SIZE);
        self.heap.set(less);
        self.set_used(self.used().saturating_sub(frames));
    }
}

impl Drop for Meter {
    fn drop(&mut self) {
        RESERVE.shift(held(self.used(), self.limit()), 0);
    }
}

/// Frames set aside ([`RESERVE`]).
#[derive(Debug, Default)]
pub struct Reserve(AtomicU64);

impl Reserve {
    pub const fn new() -> Self {
        Self(AtomicU64::new(0))
    }

    /// Frames set aside.
    #[inline]
    pub fn frames(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }

    /// Sets `frames` aside for a holder that is not a meter (a suspended
    /// cell's reservation), until [`release`](Self::release).
    pub fn hold(&self, frames: u64) {
        self.shift(0, frames);
    }

    /// Gives back `frames` set aside by [`hold`](Self::hold).
    pub fn release(&self, frames: u64) {
        self.shift(frames, 0);
    }

    #[inline(always)]
    fn shift(&self, from: u64, to: u64) {
        if to > from {
            self.0.fetch_add(to - from, Ordering::Relaxed);
        } else if from > to {
            self.0.fetch_sub(from - to, Ordering::Relaxed);
        }
    }
}

/// Frames taken for one cell, counted against its [`Meter`]: an allocation
/// that would take the count past the limit fails, as when memory runs
/// out. Every frame the cell gives back is credited, whoever took it first,
/// so a cell holds at most its quota plus what it inherited from its parent
/// or template (counted against them).
#[derive(Debug)]
pub struct Metered<'a, A> {
    /// Where frames come from.
    pub frames: A,
    /// The cell's meter (shared by every `Metered` of the cell).
    pub meter: &'a Meter,
}

impl<A: Frames> Metered<'_, A> {
    /// Counts `count` frames, then takes them: a meter with a limit has
    /// them out of what is set aside for it before they are asked for.
    #[inline(always)]
    fn take(&mut self, count: u64, alloc: impl FnOnce(&mut A) -> Option<u64>) -> Option<u64> {
        let was = self.meter.used();
        let used = was.checked_add(count)?;
        if used > self.meter.limit() {
            return None;
        }
        self.meter.set_used(used);
        let phys = alloc(&mut self.frames);
        if phys.is_none() {
            self.meter.set_used(was);
        }
        phys
    }
}

impl<A: Frames> Frames for Metered<'_, A> {
    /// A meter with a limit takes frames its limit set aside: the source
    /// serves them even from a reserve of its own (finding S30 in
    /// `docs/security.md`).
    #[inline(always)]
    fn alloc(&mut self) -> Option<u64> {
        let set_aside = self.meter.limit() < Quota::NONE.frames();
        self.take(1, |f| match set_aside {
            true => f.alloc_set_aside(),
            false => f.alloc(),
        })
    }

    #[inline(always)]
    fn alloc_large(&mut self) -> Option<u64> {
        let set_aside = self.meter.limit() < Quota::NONE.frames();
        self.take(PageSize::Large.bytes() / FRAME_SIZE, |f| match set_aside {
            true => f.alloc_large_set_aside(),
            false => f.alloc_large(),
        })
    }

    /// A reference to a frame the kernel keeps (a page of its page cache)
    /// was never counted for the cell, so giving it back credits nothing:
    /// a cell mapping and unmapping such a page lowered its count every
    /// time, below what it held (finding S30 in `docs/security.md`).
    #[inline(always)]
    fn free(&mut self, phys: u64, size: PageSize) {
        if self.frames.release(phys, size) {
            let count = size.bytes() / FRAME_SIZE;
            self.meter.set_used(self.meter.used().saturating_sub(count));
        }
    }

    #[inline(always)]
    fn share(&mut self, phys: u64) {
        self.frames.share(phys);
    }

    #[inline(always)]
    fn is_shared(&self, phys: u64) -> bool {
        self.frames.is_shared(phys)
    }

    fn metered(&self) -> Option<(u64, u64)> {
        Some((self.meter.used(), self.meter.limit()))
    }

    fn charge(&mut self, bytes: u64) -> bool {
        self.meter.charge(bytes)
    }

    fn credit(&mut self, bytes: u64) {
        self.meter.credit(bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::CountingFrames;

    /// Held by the tests that make meters with a limit: they share the
    /// [`RESERVE`], which one of them checks.
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
        SERIAL.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Finding S30: a reference to a frame the kernel keeps (a page of
    /// its cache that the cell mapped) credits nothing when given back,
    /// however often; the cell's own frames and shared ones do.
    #[test]
    fn references_to_kept_frames_credit_nothing() {
        let _serial = serial();
        let used = Meter::new(0, 8);
        let mut m = Metered {
            frames: CountingFrames::new(64),
            meter: &used,
        };
        let own = m.alloc().expect("within quota");
        let kept = m.frames.alloc().expect("the kernel's");
        m.frames.kept.push(kept);
        for _ in 0..3 {
            m.share(kept); // mapped by the cell
            m.free(kept, PageSize::Small); // and unmapped
        }
        assert_eq!(used.used(), 1, "only its own frame is counted");
        let shared = m.alloc().expect("within quota");
        m.share(shared); // a fork's copy holds it too
        m.free(shared, PageSize::Small);
        assert_eq!(used.used(), 1, "a shared frame of its own credits");
        m.free(own, PageSize::Small);
        assert_eq!(used.used(), 0);
    }

    #[test]
    fn allocations_stop_at_the_limit_and_frees_make_room() {
        let _serial = serial();
        let frames = CountingFrames::new(64);
        let used = Meter::new(0, 3);
        let mut m = Metered {
            frames,
            meter: &used,
        };
        let a: Vec<u64> = (0..3).map(|_| m.alloc().expect("within quota")).collect();
        assert_eq!(m.alloc(), None, "past the quota");
        assert_eq!(m.alloc_large(), None, "a large page does not fit either");
        m.free(a[0], PageSize::Small);
        assert_eq!(used.used(), 2);
        assert!(m.alloc().is_some(), "a freed frame makes room");
        assert_eq!(
            m.frames.outstanding(),
            3,
            "nothing allocated past the quota"
        );
    }

    #[test]
    fn large_pages_count_512_frames() {
        let _serial = serial();
        let frames = CountingFrames::new(64);
        let used = Meter::new(0, 600);
        let mut m = Metered {
            frames,
            meter: &used,
        };
        let large = m.alloc_large().expect("fits");
        assert_eq!(used.used(), 512);
        assert_eq!(m.alloc_large(), None);
        m.free(large, PageSize::Large);
        assert_eq!(used.used(), 0);
    }

    #[test]
    fn credit_never_goes_below_zero() {
        let _serial = serial();
        // A child giving back a page it inherited was never charged for it.
        let mut frames = CountingFrames::new(64);
        let inherited = frames.alloc().unwrap();
        let used = Meter::new(0, 1);
        let mut m = Metered {
            frames,
            meter: &used,
        };
        m.free(inherited, PageSize::Small);
        assert_eq!(used.used(), 0);
        assert!(m.alloc().is_some());
        assert_eq!(m.alloc(), None);
    }

    #[test]
    fn kernel_memory_counts_in_whole_frames_with_the_pages() {
        let _serial = serial();
        let used = Meter::new(0, 3);
        let mut m = Metered {
            frames: CountingFrames::new(64),
            meter: &used,
        };
        assert!(m.charge(100), "a name takes a frame of the count");
        assert!(m.charge(FRAME_SIZE - 100), "and fills it");
        assert_eq!(used.used(), 1);
        let page = m.alloc().expect("within quota");
        assert!(m.charge(1));
        assert_eq!(used.used(), 3);
        assert!(!m.charge(FRAME_SIZE), "past the quota: nothing counted");
        assert_eq!(m.alloc(), None, "pages and kernel memory share it");
        m.credit(1);
        m.free(page, PageSize::Small);
        assert_eq!(used.used(), 1);
        m.credit(10 * FRAME_SIZE);
        assert_eq!(used.used(), 0, "never more given back than counted");
        used.set_used(2); // pages it inherited
        m.credit(FRAME_SIZE);
        assert_eq!(used.used(), 2, "kernel memory never credits pages");
    }

    #[test]
    fn the_reserve_holds_what_limits_leave() {
        let _serial = serial();
        let unlimited = Meter::new(5, Quota::NONE.frames());
        assert_eq!(RESERVE.frames(), 0, "no limit: nothing set aside");
        {
            let used = Meter::new(1, 4);
            assert_eq!(RESERVE.frames(), 3);
            let mut m = Metered {
                frames: CountingFrames::new(64),
                meter: &used,
            };
            let page = m.alloc().expect("within quota");
            assert!(m.charge(1));
            assert_eq!(RESERVE.frames(), 1, "taken out of what was set aside");
            m.free(page, PageSize::Small);
            assert_eq!(RESERVE.frames(), 2);
            used.set_limit(Quota::NONE.frames());
            assert_eq!(RESERVE.frames(), 0, "a limit removed sets nothing aside");
            used.set_limit(3);
            assert_eq!(RESERVE.frames(), 1);
            let mut frames = CountingFrames::new(64);
            frames.limit = 0;
            let mut empty = Metered {
                frames,
                meter: &used,
            };
            assert_eq!(empty.alloc(), None);
            assert_eq!(RESERVE.frames(), 1, "a failed allocation gives it back");
        }
        assert_eq!(RESERVE.frames(), 0, "a meter that goes gives it all back");
        drop(unlimited);
        RESERVE.hold(7);
        assert_eq!(RESERVE.frames(), 7);
        RESERVE.release(7);
        assert_eq!(RESERVE.frames(), 0);
    }

    #[test]
    fn cpu_time_runs_out_at_the_quota() {
        let mut cell = crate::Cell::new(1, 0, 0, 0, 0);
        assert!(cell.charge_cpu(u64::MAX), "no quota: never runs out");
        cell.cpu_ns = 0;
        cell.quota.cpu_ns = 100;
        assert!(cell.charge_cpu(60));
        assert!(!cell.charge_cpu(40));
        assert_eq!(cell.cpu_ns, 100);
    }

    #[test]
    fn no_quota_is_the_kernels_limits() {
        let q = Quota::NONE;
        assert_eq!(q.files(), FILES_MAX);
        assert_eq!(Quota { files: 5000, ..q }.files(), FILES_MAX);
        assert_eq!(Quota { memory: 8191, ..q }.frames(), 1);
    }

    #[test]
    fn a_new_cell_holds_a_bounded_share_of_sockets() {
        let cell = crate::Cell::new(1, 0, 0, 0, 0);
        assert_eq!(cell.quota, Quota::default());
        assert_eq!(
            cell.quota,
            Quota {
                sockets: SOCKETS_DEFAULT,
                ..Quota::NONE
            }
        );
    }
}
