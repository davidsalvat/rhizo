//! Scheduling of cells: a run queue per CPU, a sleep list (design decision
//! D1 of `add-scheduler`, D2 of `add-smp-scheduling`) and a waiting set
//! (design D3 of `add-sockets-and-poll`); and the cells running on several
//! CPUs at once ([`Gangs`], design D1 of `add-parallel-threads`).
//!
//! The scheduler never reads a clock and never asks which CPU runs it:
//! every operation that depends on time takes `now` in nanoseconds, and
//! every operation of a CPU takes its number, so its behavior is a pure
//! function of the calls made and is tested on the host, including the
//! interleavings of several CPUs.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::collections::VecDeque;
use alloc::vec::Vec;

mod gangs;
pub use gangs::Gangs;

/// Length of a time slice: how long a cell runs before it is preempted.
pub const SLICE_NS: u64 = 10_000_000;

/// Runnable entries in one queue per CPU, and sleeping ones by wake time.
///
/// Every runnable entry carries the order in which it became runnable, so
/// the scheduler knows which one has waited longest across all queues:
/// - a CPU takes the first entry of its own queue, or, when its queue is
///   empty, the entry that has waited longest in the others (stealing);
/// - [`next_oldest`](Self::next_oldest) takes the one that has waited
///   longest of all, which is plain round robin over every queue (one CPU
///   stepping through every cell, as with a single queue).
///
/// With one CPU there is one queue and both are the round robin of a single
/// run queue.
#[derive(Debug, Clone)]
pub struct Scheduler<T> {
    /// Per CPU: entries in arrival order, with their arrival number.
    queues: Vec<VecDeque<(u64, T)>>,
    /// Arrival number of the next entry made runnable.
    arrivals: u64,
    /// Sorted by wake time; equal times keep the order they were added in.
    sleeping: Vec<(u64, T)>,
    /// In the order they started waiting, with their deadline if any.
    waiting: Vec<(Option<u64>, T)>,
}

impl<T> Scheduler<T> {
    /// An empty scheduler for `cpus` CPUs (at least one).
    pub fn new(cpus: usize) -> Self {
        Self {
            queues: (0..cpus.max(1)).map(|_| VecDeque::new()).collect(),
            arrivals: 0,
            sleeping: Vec::new(),
            waiting: Vec::new(),
        }
    }

    fn push(&mut self, cpu: usize, entry: T) {
        self.queues[cpu].push_back((self.arrivals, entry));
        self.arrivals += 1;
    }

    /// The CPU with the shortest queue (the lowest number on ties).
    fn shortest(&self) -> usize {
        (0..self.queues.len())
            .min_by_key(|&c| self.queues[c].len())
            .unwrap_or(0)
    }

    /// Makes a new entry runnable on the CPU with the shortest queue;
    /// returns that CPU.
    pub fn place(&mut self, entry: T) -> usize {
        let cpu = self.shortest();
        self.push(cpu, entry);
        cpu
    }

    /// Makes `entry`, which just ran on `cpu`, runnable again: at the end of
    /// `cpu`'s queue, unless that queue would hold more than one entry more
    /// than the shortest; then at the end of the shortest. Returns the CPU
    /// whose queue it joined.
    pub fn requeue(&mut self, cpu: usize, entry: T) -> usize {
        let shortest = self.shortest();
        let cpu = if self.queues[cpu].len() > self.queues[shortest].len() {
            shortest
        } else {
            cpu
        };
        self.push(cpu, entry);
        cpu
    }

    /// Puts `entry` to sleep until `wake_at` (nanoseconds, same clock as
    /// `now`).
    pub fn sleep(&mut self, entry: T, wake_at: u64) {
        let at = self.sleeping.partition_point(|&(t, _)| t <= wake_at);
        self.sleeping.insert(at, (wake_at, entry));
    }

    /// Makes `entry` wait until [`wake_waiting`](Self::wake_waiting) or, if
    /// given, `deadline` (nanoseconds, same clock as `now`).
    pub fn wait(&mut self, entry: T, deadline: Option<u64>) {
        self.waiting.push((deadline, entry));
    }

    /// Makes every waiting entry runnable on `cpu`'s queue, in the order
    /// they started waiting, after `f` has seen it: something they may wait
    /// for happened.
    pub fn wake_waiting(&mut self, cpu: usize, mut f: impl FnMut(&mut T)) {
        for (_, mut entry) in core::mem::take(&mut self.waiting) {
            f(&mut entry);
            self.push(cpu, entry);
        }
    }

    /// Number of waiting entries.
    pub fn waiting(&self) -> usize {
        self.waiting.len()
    }

    /// Moves the sleepers and waiters whose time has come to the end of
    /// `cpu`'s queue (sleepers in wake-time order, then waiters in the order
    /// they started waiting).
    fn wake(&mut self, cpu: usize, now: u64) {
        let due = self.sleeping.partition_point(|&(t, _)| t <= now);
        let Self {
            queues,
            arrivals,
            sleeping,
            waiting,
        } = self;
        for (_, entry) in sleeping.drain(..due) {
            queues[cpu].push_back((*arrivals, entry));
            *arrivals += 1;
        }
        if waiting.iter().any(|&(d, _)| d.is_some_and(|d| d <= now)) {
            let (due, rest): (Vec<_>, Vec<_>) = core::mem::take(waiting)
                .into_iter()
                .partition(|&(d, _)| d.is_some_and(|d| d <= now));
            *waiting = rest;
            for (_, entry) in due {
                queues[cpu].push_back((*arrivals, entry));
                *arrivals += 1;
            }
        }
    }

    /// The CPU whose queue's first entry has waited longest, skipping
    /// `except`.
    fn oldest(&self, except: Option<usize>) -> Option<usize> {
        (0..self.queues.len())
            .filter(|&c| Some(c) != except)
            .filter_map(|c| self.queues[c].front().map(|&(seq, _)| (seq, c)))
            .min()
            .map(|(_, c)| c)
    }

    /// Wakes the sleepers whose time has come (onto `cpu`'s queue) and
    /// returns the next entry for `cpu` to run: the first of its queue, or
    /// else the one that has waited longest in another CPU's queue.
    pub fn next(&mut self, cpu: usize, now: u64) -> Option<T> {
        self.wake(cpu, now);
        let from = if self.queues[cpu].is_empty() {
            self.oldest(Some(cpu))?
        } else {
            cpu
        };
        self.queues[from].pop_front().map(|(_, e)| e)
    }

    /// Wakes the sleepers whose time has come (onto CPU 0's queue) and
    /// returns the entry that has waited longest in any queue.
    pub fn next_oldest(&mut self, now: u64) -> Option<T> {
        self.wake(0, now);
        let from = self.oldest(None)?;
        self.queues[from].pop_front().map(|(_, e)| e)
    }

    /// Earliest wake-up time among sleepers and waiters with a deadline.
    pub fn next_wake(&self) -> Option<u64> {
        let waiter = self.waiting.iter().filter_map(|&(d, _)| d).min();
        match (self.sleeping.first().map(|&(t, _)| t), waiter) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Number of runnable entries, in every queue.
    pub fn runnable(&self) -> usize {
        self.queues.iter().map(VecDeque::len).sum()
    }

    /// Removes and returns the first entry (runnable or sleeping) matching
    /// `pred`.
    pub fn remove(&mut self, pred: impl FnMut(&T) -> bool) -> Option<T> {
        self.remove_with_wake(pred).map(|(entry, _)| entry)
    }

    /// Like [`remove`](Self::remove), also returning the wake time if the
    /// entry was sleeping (a waiting entry is returned as runnable: what it
    /// waits for is checked again when it runs).
    pub fn remove_with_wake(
        &mut self,
        mut pred: impl FnMut(&T) -> bool,
    ) -> Option<(T, Option<u64>)> {
        for queue in &mut self.queues {
            if let Some(i) = queue.iter().position(|(_, e)| pred(e)) {
                return queue.remove(i).map(|(_, e)| (e, None));
            }
        }
        if let Some(i) = self.waiting.iter().position(|(_, e)| pred(e)) {
            return Some((self.waiting.remove(i).1, None));
        }
        let i = self.sleeping.iter().position(|(_, e)| pred(e))?;
        let (wake, entry) = self.sleeping.remove(i);
        Some((entry, Some(wake)))
    }

    /// The first entry (runnable, waiting or sleeping) matching `pred`.
    pub fn find(&self, mut pred: impl FnMut(&T) -> bool) -> Option<&T> {
        self.queues
            .iter()
            .flatten()
            .map(|(_, e)| e)
            .chain(self.waiting.iter().map(|(_, e)| e))
            .chain(self.sleeping.iter().map(|(_, e)| e))
            .find(|e| pred(e))
    }

    /// The first entry (runnable, waiting or sleeping) matching `pred`,
    /// mutably.
    pub fn find_mut(&mut self, mut pred: impl FnMut(&T) -> bool) -> Option<&mut T> {
        self.queues
            .iter_mut()
            .flatten()
            .map(|(_, e)| e)
            .chain(self.waiting.iter_mut().map(|(_, e)| e))
            .chain(self.sleeping.iter_mut().map(|(_, e)| e))
            .find(|e| pred(e))
    }

    /// Number of entries, runnable, waiting and sleeping.
    pub fn len(&self) -> usize {
        self.runnable() + self.waiting.len() + self.sleeping.len()
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
        let mut s = Scheduler::new(1);
        for id in 1..=3 {
            s.place(id);
        }
        let mut order = Vec::new();
        for _ in 0..6 {
            let id = s.next(0, 0).unwrap();
            order.push(id);
            s.requeue(0, id); // preempted: back of the queue
        }
        assert_eq!(order, [1, 2, 3, 1, 2, 3]);
    }

    #[test]
    fn sleepers_wake_at_their_time_in_time_order() {
        let mut s = Scheduler::new(1);
        s.sleep('b', 200);
        s.sleep('a', 100);
        s.sleep('c', 200);
        assert_eq!(s.next_wake(), Some(100));
        assert_eq!(s.next(0, 99), None, "nobody is due yet");
        assert_eq!(s.next(0, 100), Some('a'));
        assert_eq!(s.next(0, 150), None);
        assert_eq!(s.next(0, 500), Some('b'), "equal times keep their order");
        assert_eq!(s.next(0, 500), Some('c'));
        assert_eq!(s.next_wake(), None);
    }

    #[test]
    fn woken_sleepers_queue_behind_runnable_entries() {
        let mut s = Scheduler::new(1);
        s.place(1);
        s.sleep(2, 10);
        s.place(3);
        assert_eq!(s.next(0, 10), Some(1));
        assert_eq!(s.next(0, 10), Some(3));
        assert_eq!(s.next(0, 10), Some(2));
    }

    #[test]
    fn remove_finds_runnable_and_sleeping_entries() {
        let mut s = Scheduler::new(1);
        s.place(1);
        s.sleep(2, 10);
        assert_eq!(s.len(), 2);
        assert_eq!(s.find(|&e| e == 2), Some(&2));
        *s.find_mut(|&e| e == 1).unwrap() = 3;
        assert_eq!(s.find(|&e| e == 3), Some(&3));
        *s.find_mut(|&e| e == 3).unwrap() = 1;
        assert_eq!(s.remove_with_wake(|&e| e == 2), Some((2, Some(10))));
        assert_eq!(s.remove(|&e| e == 9), None);
        assert_eq!(s.remove(|&e| e == 1), Some(1));
        assert!(s.is_empty());
    }

    #[test]
    fn new_entries_spread_and_cpus_keep_their_own() {
        let mut s = Scheduler::new(3);
        assert_eq!(
            [s.place('a'), s.place('b'), s.place('c'), s.place('d')],
            [0, 1, 2, 0]
        );
        assert_eq!(s.next(1, 0), Some('b'));
        assert_eq!(s.requeue(1, 'b'), 1, "back to its CPU");
        assert_eq!(s.next(0, 0), Some('a'));
        assert_eq!(s.next(0, 0), Some('d'));
        assert_eq!(
            s.next(0, 0),
            Some('c'),
            "empty: steals the oldest of the others"
        );
        assert_eq!(s.next(0, 0), Some('b'));
        assert_eq!(s.next(0, 0), None);
    }

    #[test]
    fn requeue_keeps_queues_within_one_entry() {
        let mut s = Scheduler::new(2);
        for e in 0..4 {
            s.requeue(0, e); // all ran on CPU 0
        }
        // CPU 0 keeps an entry while it holds no more than CPU 1, else the
        // entry goes to CPU 1.
        assert_eq!(s.next(1, 0), Some(1), "CPU 1 got the second one");
        assert_eq!(s.next(1, 0), Some(3));
        assert_eq!(s.next(1, 0), Some(0), "then steals");
    }

    #[test]
    fn next_oldest_is_round_robin_over_every_queue() {
        let mut s = Scheduler::new(4);
        for id in 1..=5 {
            s.place(id);
        }
        let mut order = Vec::new();
        for _ in 0..10 {
            let id = s.next_oldest(0).unwrap();
            order.push(id);
            s.requeue(0, id);
        }
        assert_eq!(order, [1, 2, 3, 4, 5, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn woken_sleepers_join_the_cpu_that_finds_them() {
        let mut s = Scheduler::new(2);
        s.sleep('z', 10);
        s.place('a');
        assert_eq!(s.next(1, 10), Some('z'), "woken onto CPU 1's queue");
        assert_eq!(s.next(1, 10), Some('a'));
    }

    /// One step of a simulated CPU in the interleaving tests.
    #[derive(Debug, Clone, Copy)]
    enum Op {
        /// The CPU takes its next entry, if it has none running.
        Take(usize),
        /// The CPU's running entry is preempted (back in a queue).
        Preempt(usize),
        /// The CPU's running entry sleeps for this long.
        Sleep(usize, u64),
        /// Time passes.
        Tick(u64),
    }

    fn op(cpus: usize) -> impl Strategy<Value = Op> {
        prop_oneof![
            (0..cpus).prop_map(Op::Take),
            (0..cpus).prop_map(Op::Preempt),
            (0..cpus, 1u64..20).prop_map(|(c, d)| Op::Sleep(c, d)),
            (1u64..10).prop_map(Op::Tick),
        ]
    }

    proptest! {
        /// Several CPUs taking, preempting and putting to sleep entries in
        /// any interleaving: no entry is lost or held twice, none runs
        /// before its wake time, and a CPU finds nothing only when no queue
        /// holds anything and no sleeper is due.
        #[test]
        fn interleaved_cpus_lose_nothing_and_never_idle_with_work(
            cpus in 1usize..5,
            entries in 1usize..12,
            ops in proptest::collection::vec(op(4), 1..300),
        ) {
            let mut s = Scheduler::new(cpus);
            for e in 0..entries {
                s.place(e);
            }
            let mut running: Vec<Option<usize>> = vec![None; cpus];
            let mut wake = vec![0u64; entries];
            let mut now = 0;
            for op in ops {
                match op {
                    Op::Take(c) if c < cpus && running[c].is_none() => {
                        let due = s.next_wake().is_some_and(|t| t <= now);
                        let work = s.runnable() > 0 || due;
                        running[c] = s.next(c, now);
                        prop_assert_eq!(running[c].is_some(), work);
                        if let Some(e) = running[c] {
                            prop_assert!(wake[e] <= now, "{} woke early", e);
                        }
                    }
                    Op::Preempt(c) if c < cpus => {
                        if let Some(e) = running[c].take() {
                            s.requeue(c, e);
                        }
                    }
                    Op::Sleep(c, d) if c < cpus => {
                        if let Some(e) = running[c].take() {
                            wake[e] = now + d;
                            s.sleep(e, now + d);
                        }
                    }
                    Op::Tick(d) => now += d,
                    _ => {}
                }
                // Every entry is in exactly one place.
                let mut seen = vec![0; entries];
                for e in running.iter().flatten() {
                    seen[*e] += 1;
                }
                for (e, n) in seen.iter_mut().enumerate() {
                    if s.find(|&x| x == e).is_some() {
                        *n += 1;
                    }
                }
                prop_assert!(seen.iter().all(|&n| n == 1), "{:?}", seen);
                prop_assert_eq!(s.len() + running.iter().flatten().count(), entries);
            }
        }

        /// Rounds in which every CPU, in any order, takes an entry and puts
        /// it back: every entry runs at least once in any `entries`
        /// consecutive rounds (nobody starves), and the queues never differ
        /// by more than a couple of entries.
        #[test]
        fn rounds_of_cpus_starve_nobody(
            cpus in 1usize..5,
            entries in 1usize..12,
            orders in proptest::collection::vec(
                Just((0..4).collect::<Vec<usize>>()).prop_shuffle(), 30),
        ) {
            let mut s = Scheduler::new(cpus);
            for e in 0..entries {
                s.place(e);
            }
            let mut last_run = vec![0usize; entries];
            for (round, order) in orders.iter().enumerate() {
                let mut taken = Vec::new();
                for &c in order.iter().filter(|&&c| c < cpus) {
                    if let Some(e) = s.next(c, 0) {
                        last_run[e] = round;
                        taken.push((c, e));
                    }
                }
                for (c, e) in taken {
                    s.requeue(c, e);
                }
                for (e, &last) in last_run.iter().enumerate() {
                    prop_assert!(round - last < entries.max(1),
                        "entry {} waited since round {}", e, last);
                }
                let lens: Vec<usize> = (0..cpus).map(|c| s.queues[c].len()).collect();
                let (min, max) = (lens.iter().min().unwrap(), lens.iter().max().unwrap());
                prop_assert!(max - min <= 2, "unbalanced queues {:?}", lens);
            }
        }

        /// With one CPU, `next` and `next_oldest` are the same round robin.
        #[test]
        fn one_cpu_next_is_next_oldest(
            ops in proptest::collection::vec((any::<bool>(), 0u64..50), 1..40)
        ) {
            let (mut a, mut b) = (Scheduler::new(1), Scheduler::new(1));
            for (id, &(sleeps, t)) in ops.iter().enumerate() {
                if sleeps { a.sleep(id, t); b.sleep(id, t); } else { a.place(id); b.place(id); }
            }
            for now in 0..=50 {
                loop {
                    let (x, y) = (a.next(0, now), b.next_oldest(now));
                    prop_assert_eq!(x, y);
                    match x {
                        Some(id) if id % 3 == 0 => { a.requeue(0, id + 100); b.requeue(0, id + 100); }
                        Some(_) => {}
                        None => break,
                    }
                }
            }
        }
    }

    #[test]
    fn waiters_wake_together_or_at_their_deadline() {
        let mut s = Scheduler::new(1);
        s.wait('a', None);
        s.wait('b', Some(100));
        s.sleep('c', 50);
        assert_eq!(s.len(), 3);
        assert_eq!(s.waiting(), 2);
        assert_eq!(s.next_wake(), Some(50), "the earlier of sleeper and waiter");
        assert_eq!(s.next(0, 60), Some('c'));
        assert_eq!(s.next_wake(), Some(100));
        assert_eq!(s.next(0, 99), None, "a deadline is never early");
        assert_eq!(s.next(0, 100), Some('b'));
        assert_eq!(s.next(0, 1_000), None, "no deadline: waits for a wake");
        assert_eq!(s.next_wake(), None);
        assert_eq!(s.find(|&e| e == 'a'), Some(&'a'));
        s.wake_waiting(0, |_| {});
        assert_eq!(s.waiting(), 0);
        assert_eq!(s.next(0, 0), Some('a'));
    }

    #[test]
    fn a_removed_waiter_comes_back_runnable() {
        let mut s = Scheduler::new(1);
        s.wait(1, Some(10));
        *s.find_mut(|&e| e == 1).unwrap() = 2;
        assert_eq!(s.remove_with_wake(|&e| e == 2), Some((2, None)));
        assert!(s.is_empty());
    }

    #[test]
    fn woken_waiters_join_the_waking_cpu() {
        let mut s = Scheduler::new(2);
        s.wait('w', None);
        s.wake_waiting(1, |_| {});
        assert_eq!(s.next(1, 0), Some('w'));
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
            let mut s = Scheduler::new(1);
            let mut wake = Vec::new();
            for (id, &(sleeps, t)) in ops.iter().enumerate() {
                if sleeps { s.sleep(id, t); } else { s.place(id); }
                wake.push(if sleeps { t } else { 0 });
            }
            let mut seen = vec![false; ops.len()];
            for now in 0..=50 {
                while let Some(id) = s.next(0, now) {
                    prop_assert!(!seen[id], "entry {} ran twice", id);
                    prop_assert!(wake[id] <= now, "entry {} woke early", id);
                    seen[id] = true;
                }
            }
            prop_assert!(seen.iter().all(|&s| s));
        }

        /// Waiters with deadlines never come out early, and every waiter
        /// comes out exactly once, whether by deadline or by a wake.
        #[test]
        fn waiters_are_never_early_or_lost(
            ops in proptest::collection::vec((proptest::option::of(0u64..50), any::<bool>()), 1..60)
        ) {
            let mut s = Scheduler::new(1);
            for (id, &(deadline, _)) in ops.iter().enumerate() {
                s.wait(id, deadline);
            }
            let mut seen = vec![false; ops.len()];
            for now in 0..=50u64 {
                if ops[now as usize % ops.len()].1 && now % 7 == 0 {
                    s.wake_waiting(0, |_| {});
                    while let Some(id) = s.next(0, now) {
                        prop_assert!(!seen[id]);
                        seen[id] = true;
                    }
                }
                while let Some(id) = s.next(0, now) {
                    prop_assert!(!seen[id], "entry {} ran twice", id);
                    prop_assert!(ops[id].0.is_some_and(|d| d <= now), "entry {} woke early", id);
                    seen[id] = true;
                }
            }
            s.wake_waiting(0, |_| {});
            while let Some(id) = s.next(0, 50) {
                prop_assert!(!seen[id]);
                prop_assert!(ops[id].0.is_none());
                seen[id] = true;
            }
            prop_assert!(seen.iter().all(|&s| s));
        }
    }
}
