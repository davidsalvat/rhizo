//! Per-cell quotas (spec `isolation`, design of `add-cell-quotas`): what
//! the host lets one cell use, and the accounting that enforces memory and
//! CPU time. Quotas are plain data in the [`Cell`](crate::Cell), so forks,
//! templates and snapshots carry them.

use core::cell::Cell as Counter;
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
/// for it costs one pointer to the meter.
#[derive(Debug)]
pub struct Meter {
    used: Counter<u64>,
    limit: Counter<u64>,
}

impl Meter {
    /// A meter with `used` frames counted and a limit of `limit` frames.
    pub const fn new(used: u64, limit: u64) -> Self {
        Self {
            used: Counter::new(used),
            limit: Counter::new(limit),
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
        self.limit.set(limit);
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
    #[inline(always)]
    fn take(&mut self, count: u64, alloc: impl FnOnce(&mut A) -> Option<u64>) -> Option<u64> {
        let used = self.meter.used().checked_add(count)?;
        if used > self.meter.limit() {
            return None;
        }
        let phys = alloc(&mut self.frames)?;
        self.meter.set_used(used);
        Some(phys)
    }
}

impl<A: Frames> Frames for Metered<'_, A> {
    #[inline(always)]
    fn alloc(&mut self) -> Option<u64> {
        self.take(1, A::alloc)
    }

    #[inline(always)]
    fn alloc_large(&mut self) -> Option<u64> {
        self.take(PageSize::Large.bytes() / FRAME_SIZE, A::alloc_large)
    }

    #[inline(always)]
    fn free(&mut self, phys: u64, size: PageSize) {
        self.frames.free(phys, size);
        let count = size.bytes() / FRAME_SIZE;
        self.meter.set_used(self.meter.used().saturating_sub(count));
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::CountingFrames;

    #[test]
    fn allocations_stop_at_the_limit_and_frees_make_room() {
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
