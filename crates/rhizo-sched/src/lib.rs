//! Scheduling of cells: a round-robin run queue and a sleep list (design
//! decision D1 of `add-scheduler`).
//!
//! The scheduler never reads a clock: every operation that depends on time
//! takes `now` in nanoseconds, so its behavior is a pure function of the
//! calls made and is tested on the host.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::collections::VecDeque;
use alloc::vec::Vec;

/// Length of a time slice: how long a cell runs before it is preempted.
pub const SLICE_NS: u64 = 10_000_000;

/// Runnable entries in round-robin order, and sleeping ones by wake time.
#[derive(Debug, Clone)]
pub struct Scheduler<T> {
    runnable: VecDeque<T>,
    /// Sorted by wake time; equal times keep the order they were added in.
    sleeping: Vec<(u64, T)>,
}

impl<T> Default for Scheduler<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Scheduler<T> {
    /// An empty scheduler.
    pub const fn new() -> Self {
        Self {
            runnable: VecDeque::new(),
            sleeping: Vec::new(),
        }
    }

    /// Makes `entry` runnable, after every entry already waiting to run.
    pub fn add(&mut self, entry: T) {
        self.runnable.push_back(entry);
    }

    /// Puts `entry` to sleep until `wake_at` (nanoseconds, same clock as
    /// `now`).
    pub fn sleep(&mut self, entry: T, wake_at: u64) {
        let at = self.sleeping.partition_point(|&(t, _)| t <= wake_at);
        self.sleeping.insert(at, (wake_at, entry));
    }

    /// Wakes the sleepers whose time has come and returns the next entry to
    /// run, if any.
    pub fn next(&mut self, now: u64) -> Option<T> {
        let due = self.sleeping.partition_point(|&(t, _)| t <= now);
        self.runnable
            .extend(self.sleeping.drain(..due).map(|(_, entry)| entry));
        self.runnable.pop_front()
    }

    /// Earliest wake-up time among sleepers.
    pub fn next_wake(&self) -> Option<u64> {
        self.sleeping.first().map(|&(t, _)| t)
    }

    /// Removes and returns the first entry (runnable or sleeping) matching
    /// `pred`.
    pub fn remove(&mut self, mut pred: impl FnMut(&T) -> bool) -> Option<T> {
        if let Some(i) = self.runnable.iter().position(&mut pred) {
            return self.runnable.remove(i);
        }
        let i = self.sleeping.iter().position(|(_, e)| pred(e))?;
        Some(self.sleeping.remove(i).1)
    }

    /// The first entry (runnable or sleeping) matching `pred`.
    pub fn find(&self, mut pred: impl FnMut(&T) -> bool) -> Option<&T> {
        self.runnable
            .iter()
            .find(|e| pred(e))
            .or_else(|| self.sleeping.iter().map(|(_, e)| e).find(|e| pred(e)))
    }

    /// Number of entries, runnable and sleeping.
    pub fn len(&self) -> usize {
        self.runnable.len() + self.sleeping.len()
    }

    /// Whether there is no entry at all.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Address-space tags (ASIDs, PCIDs) for live cells: `1..=max`, tag 0 being
/// the kernel's. A tag is reused only after `free` (design decision D6).
#[derive(Debug, Clone)]
pub struct TagPool {
    /// One bit per tag; set = in use.
    used: Vec<u64>,
    max: u16,
    /// Next-fit hint, so a freed tag is not handed out again at once.
    next: u16,
}

impl TagPool {
    /// Tags `1..=max` (`max` = 0: no tags, every cell gets 0).
    pub fn new(max: u16) -> Self {
        Self {
            used: alloc::vec![0; max as usize / 64 + 1],
            max,
            next: 1,
        }
    }

    /// A free tag, or `None` when all are in use (the caller then runs the
    /// cell untagged, flushing on every switch).
    pub fn alloc(&mut self) -> Option<u16> {
        if self.max == 0 {
            return None;
        }
        let tags = (self.next..=self.max).chain(1..self.next);
        for tag in tags {
            let (word, bit) = (tag as usize / 64, tag % 64);
            if self.used[word] & (1 << bit) == 0 {
                self.used[word] |= 1 << bit;
                self.next = if tag == self.max { 1 } else { tag + 1 };
                return Some(tag);
            }
        }
        None
    }

    /// Returns `tag` to the pool.
    ///
    /// # Panics
    /// If `tag` is not in use.
    pub fn free(&mut self, tag: u16) {
        let (word, bit) = (tag as usize / 64, tag % 64);
        assert!(
            tag != 0 && tag <= self.max && self.used[word] & (1 << bit) != 0,
            "freeing tag {tag} that is not in use"
        );
        self.used[word] &= !(1 << bit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn round_robin_in_arrival_order() {
        let mut s = Scheduler::new();
        for id in 1..=3 {
            s.add(id);
        }
        let mut order = Vec::new();
        for _ in 0..6 {
            let id = s.next(0).unwrap();
            order.push(id);
            s.add(id); // preempted: back of the queue
        }
        assert_eq!(order, [1, 2, 3, 1, 2, 3]);
    }

    #[test]
    fn sleepers_wake_at_their_time_in_time_order() {
        let mut s = Scheduler::new();
        s.sleep('b', 200);
        s.sleep('a', 100);
        s.sleep('c', 200);
        assert_eq!(s.next_wake(), Some(100));
        assert_eq!(s.next(99), None, "nobody is due yet");
        assert_eq!(s.next(100), Some('a'));
        assert_eq!(s.next(150), None);
        assert_eq!(s.next(500), Some('b'), "equal times keep their order");
        assert_eq!(s.next(500), Some('c'));
        assert_eq!(s.next_wake(), None);
    }

    #[test]
    fn woken_sleepers_queue_behind_runnable_entries() {
        let mut s = Scheduler::new();
        s.add(1);
        s.sleep(2, 10);
        s.add(3);
        assert_eq!(s.next(10), Some(1));
        assert_eq!(s.next(10), Some(3));
        assert_eq!(s.next(10), Some(2));
    }

    #[test]
    fn remove_finds_runnable_and_sleeping_entries() {
        let mut s = Scheduler::new();
        s.add(1);
        s.sleep(2, 10);
        assert_eq!(s.len(), 2);
        assert_eq!(s.find(|&e| e == 2), Some(&2));
        assert_eq!(s.remove(|&e| e == 2), Some(2));
        assert_eq!(s.remove(|&e| e == 9), None);
        assert_eq!(s.remove(|&e| e == 1), Some(1));
        assert!(s.is_empty());
    }

    #[test]
    fn tags_are_unique_until_freed_and_reused_late() {
        let mut pool = TagPool::new(3);
        assert_eq!(
            [pool.alloc(), pool.alloc(), pool.alloc()],
            [Some(1), Some(2), Some(3)]
        );
        assert_eq!(pool.alloc(), None, "all in use");
        pool.free(2);
        assert_eq!(pool.alloc(), Some(2));
        pool.free(1);
        pool.free(3);
        assert_eq!(
            pool.alloc(),
            Some(3),
            "next fit: the search goes on after the last tag handed out"
        );
        assert_eq!(TagPool::new(0).alloc(), None, "no tags at all");
    }

    #[test]
    #[should_panic(expected = "not in use")]
    fn freeing_a_free_tag_panics() {
        TagPool::new(4).free(2);
    }

    proptest! {
        /// Whatever the mix of adds, sleeps and steps, every entry comes out
        /// exactly once, never before its wake time.
        #[test]
        fn every_entry_runs_once_and_never_early(
            ops in proptest::collection::vec((any::<bool>(), 0u64..50), 1..60)
        ) {
            let mut s = Scheduler::new();
            let mut wake = Vec::new();
            for (id, &(sleeps, t)) in ops.iter().enumerate() {
                if sleeps { s.sleep(id, t); } else { s.add(id); }
                wake.push(if sleeps { t } else { 0 });
            }
            let mut seen = vec![false; ops.len()];
            for now in 0..=50 {
                while let Some(id) = s.next(now) {
                    prop_assert!(!seen[id], "entry {} ran twice", id);
                    prop_assert!(wake[id] <= now, "entry {} woke early", id);
                    seen[id] = true;
                }
            }
            prop_assert!(seen.iter().all(|&s| s));
        }
    }
}
