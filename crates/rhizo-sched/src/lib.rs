//! Scheduling of cells: a run queue per CPU, a sleep list (design decision
//! D1 of `add-scheduler`, D2 of `add-smp-scheduling`) and a waiting set
//! (design D3 of `add-sockets-and-poll`); and the cells running on several
//! CPUs at once ([`Gangs`], design D1 of `add-parallel-threads`); and which
//! idle CPU keeps a timer ([`IdleTimer`], design D1 of `add-tickless-idle`);
//! and how domains and cells share the CPUs by weight, within caps
//! ([`fair`], design D23 of `add-cell-hierarchy`).
//!
//! The scheduler never reads a clock and never asks which CPU runs it:
//! every operation that depends on time takes `now` in nanoseconds, and
//! every operation of a CPU takes its number, so its behavior is a pure
//! function of the calls made and is tested on the host, including the
//! interleavings of several CPUs.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use alloc::vec::Vec;

pub mod fair;
mod gangs;
mod idle;
pub use fair::{Fair, Share};
pub use gangs::Gangs;
pub use idle::IdleTimer;

/// Length of a time slice: how long a cell runs before it is preempted.
pub const SLICE_NS: u64 = 10_000_000;

/// What the scheduler knows an entry by: a key unique among its entries
/// (a cell's identifier), read once as the entry is added and kept next to
/// it, so finding an entry compares keys and never reads an entry (design
/// D4 of `host-ops-per-cell`).
pub trait Keyed {
    fn key(&self) -> u32;
}

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
///
/// `F` says how entries share the CPUs: `()`, the round robin above, costs
/// nothing; with [`Fair`] ([`Scheduler::fair`]) a CPU takes from the queue
/// it chose the entry owed the most CPU time by weight, and an entry whose
/// subtree used its CPU cap sleeps until the cap's period ends.
#[derive(Debug, Clone)]
pub struct Scheduler<T, F = ()> {
    /// Per CPU: entries in arrival order, with their arrival number and key.
    queues: Vec<VecDeque<(u64, u32, T)>>,
    /// Arrival number of the next entry made runnable.
    arrivals: u64,
    /// Sorted by wake time, with their key; equal times keep the order they
    /// were added in. A deque: the due ones leave from the front and a new
    /// deadline, most often the latest, joins at the back, neither moving
    /// the others (change `host-ops-per-cell`).
    sleeping: VecDeque<(u64, u32, T)>,
    /// Where to find a sleeper by its key without searching the sleep list
    /// (design D2 of `heap-caches-cell-index`): at the slot of the key's low
    /// bits, the wake time of the last entry put to sleep with such a key,
    /// whose place a binary search then finds. Written only by
    /// [`sleep`](Self::sleep), never as entries move, so a slot may be stale
    /// (the sleeper woke, or another key took the slot): a lookup checks the
    /// key it finds there, and searches the list when it is not the right
    /// one. At least as many slots as sleepers, a power of two.
    hints: Vec<u64>,
    /// By key (design D2 of `add-socket-wait-queues`), with their deadline
    /// if any.
    waiting: BTreeMap<u32, (Option<u64>, T)>,
    /// The deadlines of waiting entries, earliest first, with their key.
    deadlines: BTreeSet<(u64, u32)>,
    /// How entries share the CPUs (design D23 of `add-cell-hierarchy`).
    share: F,
}

impl<T: Keyed> Scheduler<T> {
    /// An empty scheduler for `cpus` CPUs (at least one), round robin.
    pub fn new(cpus: usize) -> Self {
        Self::with(cpus)
    }
}

impl<T: Keyed> Scheduler<T, Fair> {
    /// An empty scheduler for `cpus` CPUs (at least one) whose entries
    /// share the CPUs by weight.
    pub fn fair(cpus: usize) -> Self {
        Self::with(cpus)
    }
}

impl<T: Keyed> Scheduler<T, Option<Fair>> {
    /// An empty scheduler for `cpus` CPUs (at least one), round robin until
    /// a [`Fair`] is put in its [`shares`](Self::shares).
    pub fn switchable(cpus: usize) -> Self {
        Self::with(cpus)
    }
}

impl<T: Keyed, F: Share> Scheduler<T, F> {
    /// The weights and caps, to set when the tree changes.
    pub fn shares(&mut self) -> &mut F {
        &mut self.share
    }

    /// Charges `ns` of CPU time on `cpu`, ending at `now`, to the entry
    /// known by `key`, which just ran there: to call before it is put
    /// back. Entries of a subtree that used its cap sleep until the cap's
    /// period ends, the ones in queues at once and this one when it is put
    /// back.
    pub fn charge(&mut self, cpu: usize, key: u32, ns: u64, now: u64) {
        self.refresh();
        if self.share.charge(cpu, key, ns, now) {
            for node in self.share.spent() {
                self.hold(node);
            }
        }
    }

    /// Moves the queued entries under `node`'s used-up cap to sleep.
    #[cold]
    fn hold(&mut self, node: u32) {
        let mut held = Vec::new();
        for cpu in 0..self.queues.len() {
            let mut i = 0;
            while i < self.queues[cpu].len() {
                let key = self.queues[cpu][i].1;
                if self.share.under(key, node) {
                    let until = self.share.held(key).unwrap_or(0);
                    held.extend(self.queues[cpu].remove(i).map(|(_, _, e)| (e, until)));
                    self.share.dequeued(cpu, key);
                } else {
                    i += 1;
                }
            }
        }
        for (entry, until) in held {
            self.sleep(entry, until);
        }
    }

    /// Tells the share the queues again if its weights changed.
    #[inline]
    fn refresh(&mut self) {
        if self.share.dirty() {
            self.relearn();
        }
    }

    #[cold]
    fn relearn(&mut self) {
        let queued = self.queues.iter().enumerate();
        let entries = queued.flat_map(|(c, q)| q.iter().map(move |e| (c, e.0, e.1)));
        self.share.rebuild(entries);
    }
}

impl<T: Keyed, F: Share> Scheduler<T, F> {
    fn with(cpus: usize) -> Self {
        let cpus = cpus.max(1);
        Self {
            queues: (0..cpus).map(|_| VecDeque::new()).collect(),
            arrivals: 0,
            sleeping: VecDeque::new(),
            hints: Vec::new(),
            waiting: BTreeMap::new(),
            deadlines: BTreeSet::new(),
            share: F::new(cpus),
        }
    }

    /// Puts `entry`, known by `key`, at the end of `cpu`'s queue, with one
    /// test of how entries share: by weight, one that `arrives` (it did not
    /// just run) is not owed back what it did not use while idle, and one a
    /// cap holds sleeps until its period ends.
    fn push(&mut self, cpu: usize, key: u32, entry: T, arrives: bool) {
        if self.share.by_weight() {
            return self.push_by_weight(cpu, key, entry, arrives);
        }
        self.queue(cpu, key, entry);
    }

    /// [`push`](Self::push) while entries share by weight.
    #[inline(never)]
    fn push_by_weight(&mut self, cpu: usize, key: u32, entry: T, arrives: bool) {
        self.refresh();
        if arrives {
            self.share.arrive(cpu, key);
        }
        self.enqueue(cpu, key, entry);
    }

    /// Puts `entry` at the end of `cpu`'s queue, or to sleep until its cap's
    /// period ends (only while entries share by weight: the share learns
    /// what is queued).
    fn enqueue(&mut self, cpu: usize, key: u32, entry: T) {
        if let Some(until) = self.share.held(key) {
            return self.sleep(entry, until);
        }
        let arrival = self.arrivals;
        self.queue(cpu, key, entry);
        self.share.queued(cpu, key, arrival);
    }

    /// Puts `entry` at the end of `cpu`'s queue.
    fn queue(&mut self, cpu: usize, key: u32, entry: T) {
        self.queues[cpu].push_back((self.arrivals, key, entry));
        self.arrivals += 1;
    }

    /// Takes, of the entries in the queues of CPUs `queues`, the one owed
    /// the most CPU time by weight, for `cpu` to run.
    fn take_by_weight(&mut self, cpu: usize, queues: core::ops::Range<usize>) -> Option<T> {
        self.refresh();
        let (from, arrival) = self.share.pick(cpu, queues)?;
        // A queue is in arrival order: entries join only at its end.
        // Most often the first: equal shares take turns.
        let queue = &mut self.queues[from];
        if queue.front().is_some_and(|e| e.0 == arrival) {
            return queue.pop_front().map(|(_, _, e)| e);
        }
        let at = queue.binary_search_by_key(&arrival, |e| e.0).ok()?;
        queue.remove(at).map(|(_, _, e)| e)
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
        self.push(cpu, entry.key(), entry, true);
        cpu
    }

    /// Makes `entry`, which just ran on `cpu`, runnable again: at the end of
    /// `cpu`'s queue, unless that queue would hold more than one entry more
    /// than the shortest; then at the end of the shortest. Returns the CPU
    /// whose queue it joined.
    pub fn requeue(&mut self, cpu: usize, entry: T) -> usize {
        let cpu = self.back_to(cpu);
        self.push(cpu, entry.key(), entry, false);
        cpu
    }

    /// [`requeue`](Self::requeue) after [`charge`](Self::charge): the slice
    /// `entry` just ran, whose length and end `slice` gives, asked only
    /// while entries share by weight. One test of how they share for both.
    pub fn requeue_charged(
        &mut self,
        cpu: usize,
        entry: T,
        slice: impl FnOnce() -> (u64, u64),
    ) -> usize {
        let key = entry.key();
        if self.share.by_weight() {
            let (ns, now) = slice();
            self.charge(cpu, key, ns, now);
            let to = self.back_to(cpu);
            self.enqueue(to, key, entry);
            return to;
        }
        let to = self.back_to(cpu);
        self.queue(to, key, entry);
        to
    }

    /// Where an entry that just ran on `cpu` goes back: `cpu`'s queue,
    /// unless it holds more entries than the shortest.
    fn back_to(&self, cpu: usize) -> usize {
        let shortest = self.shortest();
        if self.queues[cpu].len() > self.queues[shortest].len() {
            shortest
        } else {
            cpu
        }
    }

    /// Puts `entry` to sleep until `wake_at` (nanoseconds, same clock as
    /// `now`).
    pub fn sleep(&mut self, entry: T, wake_at: u64) {
        let key = entry.key();
        let at = self.sleeping.partition_point(|&(t, _, _)| t <= wake_at);
        self.sleeping.insert(at, (wake_at, key, entry));
        if self.sleeping.len() > self.hints.len() {
            self.grow_hints();
        }
        let slot = key as usize & (self.hints.len() - 1);
        self.hints[slot] = wake_at;
    }

    /// Doubles the hints' slots, filled again from the sleep list.
    #[cold]
    fn grow_hints(&mut self) {
        let slots = (2 * self.sleeping.len()).next_power_of_two();
        self.hints = alloc::vec![0; slots];
        for &(wake, key, _) in &self.sleeping {
            self.hints[key as usize & (slots - 1)] = wake;
        }
    }

    /// The place in the sleep list of the sleeper known by `key`, if its
    /// hint still tells it: the entries waking at the hinted time are
    /// contiguous, found by a binary search.
    fn hinted(&self, key: u32) -> Option<usize> {
        let slots = self.hints.len();
        let wake = *self.hints.get(key as usize & slots.checked_sub(1)?)?;
        let first = self.sleeping.partition_point(|&(t, _, _)| t < wake);
        let mut run = self.sleeping.range(first..).take_while(|e| e.0 == wake);
        run.position(|e| e.1 == key).map(|i| first + i)
    }

    /// Makes `entry`, known by `key` (unique among waiting entries), wait
    /// until [`wake`](Self::wake) names it or, if given, `deadline`
    /// (nanoseconds, same clock as `now`).
    pub fn wait(&mut self, key: u32, entry: T, deadline: Option<u64>) {
        if let Some(d) = deadline {
            self.deadlines.insert((d, key));
        }
        let old = self.waiting.insert(key, (deadline, entry));
        debug_assert!(old.is_none(), "key {key} already waits");
    }

    /// Makes the waiting entry known by `key` runnable on `cpu`'s queue,
    /// after `f` has seen it: something it may wait for happened. Returns
    /// whether such an entry waited.
    pub fn wake(&mut self, cpu: usize, key: u32, f: impl FnOnce(&mut T)) -> bool {
        let Some(mut entry) = self.take_waiting(key) else {
            return false;
        };
        f(&mut entry);
        self.push(cpu, key, entry, true);
        true
    }

    /// Removes the waiting entry known by `key`.
    fn take_waiting(&mut self, key: u32) -> Option<T> {
        let (deadline, entry) = self.waiting.remove(&key)?;
        if let Some(d) = deadline {
            self.deadlines.remove(&(d, key));
        }
        Some(entry)
    }

    /// Number of waiting entries.
    pub fn waiting(&self) -> usize {
        self.waiting.len()
    }

    /// Moves the sleepers and waiters whose time has come to the end of
    /// `cpu`'s queue (sleepers, then waiters, each in time order), after `f`
    /// has seen each: what
    /// [`next`](Self::next) does first, for a caller that wants to know
    /// when an entry became runnable.
    pub fn wake_due(&mut self, cpu: usize, now: u64, f: impl FnMut(&mut T)) {
        self.share.release(now);
        self.wake_sleepers(cpu, now, f);
    }

    /// [`wake_due`](Self::wake_due) but for the caps it lets run again.
    fn wake_sleepers(&mut self, cpu: usize, now: u64, mut f: impl FnMut(&mut T)) {
        // The due ones are at the front: no search, nothing else moves (an
        // entry a cap still holds goes back to sleep, later than `now`).
        while let Some((_, key, mut entry)) = self.sleeping.pop_front_if(|e| e.0 <= now) {
            f(&mut entry);
            self.push(cpu, key, entry, true);
        }
        while let Some(&(d, key)) = self.deadlines.first()
            && d <= now
        {
            if let Some(mut entry) = self.take_waiting(key) {
                f(&mut entry);
                self.push(cpu, key, entry, true);
            }
        }
    }

    /// The CPU whose queue's first entry has waited longest, skipping
    /// `except`.
    fn oldest(&self, except: Option<usize>) -> Option<usize> {
        (0..self.queues.len())
            .filter(|&c| Some(c) != except)
            .filter_map(|c| self.queues[c].front().map(|&(seq, _, _)| (seq, c)))
            .min()
            .map(|(_, c)| c)
    }

    /// Wakes the sleepers whose time has come (onto `cpu`'s queue) and
    /// returns the next entry for `cpu` to run: the first of its queue (by
    /// weight: the one owed the most CPU time), or else the one that has
    /// waited longest in another CPU's queue (by weight: the one there owed
    /// the most).
    pub fn next(&mut self, cpu: usize, now: u64) -> Option<T> {
        if self.share.by_weight() {
            return self.next_by_weight(cpu, now);
        }
        self.wake_sleepers(cpu, now, |_| {});
        let from = self.next_from(cpu)?;
        self.queues[from].pop_front().map(|(_, _, e)| e)
    }

    /// [`next`](Self::next) while entries share by weight.
    #[inline(never)]
    fn next_by_weight(&mut self, cpu: usize, now: u64) -> Option<T> {
        self.wake_due(cpu, now, |_| {});
        let from = self.next_from(cpu)?;
        self.take_by_weight(cpu, from..from + 1)
    }

    /// The queue `cpu` takes from: its own, or the one whose first entry
    /// has waited longest.
    fn next_from(&self, cpu: usize) -> Option<usize> {
        if self.queues[cpu].is_empty() {
            self.oldest(Some(cpu))
        } else {
            Some(cpu)
        }
    }

    /// Wakes the sleepers whose time has come (onto CPU 0's queue) and
    /// returns the entry that has waited longest in any queue (by weight:
    /// the one owed the most CPU time in all of them).
    pub fn next_oldest(&mut self, now: u64) -> Option<T> {
        if self.share.by_weight() {
            self.wake_due(0, now, |_| {});
            self.oldest(None)?;
            return self.take_by_weight(0, 0..self.queues.len());
        }
        self.wake_sleepers(0, now, |_| {});
        let from = self.oldest(None)?;
        self.queues[from].pop_front().map(|(_, _, e)| e)
    }

    /// Earliest wake-up time among sleepers and waiters with a deadline.
    pub fn next_wake(&self) -> Option<u64> {
        let waiter = self.deadlines.first().map(|&(d, _)| d);
        match (self.sleeping.front().map(|&(t, _, _)| t), waiter) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Number of runnable entries, in every queue.
    pub fn runnable(&self) -> usize {
        self.queues.iter().map(VecDeque::len).sum()
    }

    /// Removes the entry known by `key` (runnable, waiting or sleeping),
    /// with its wake time if it was sleeping (a waiting entry is returned
    /// as runnable: what it waits for is checked again when it runs).
    pub fn remove(&mut self, key: u32) -> Option<(T, Option<u64>)> {
        if let Some(i) = self.hinted(key) {
            let (wake, _, entry) = self.sleeping.remove(i)?;
            return Some((entry, Some(wake)));
        }
        for cpu in 0..self.queues.len() {
            if let Some(i) = self.queues[cpu].iter().position(|e| e.1 == key) {
                self.share.dequeued(cpu, key);
                return self.queues[cpu].remove(i).map(|(_, _, e)| (e, None));
            }
        }
        if let Some(entry) = self.take_waiting(key) {
            return Some((entry, None));
        }
        let i = self.sleeping.iter().position(|e| e.1 == key)?;
        let (wake, _, entry) = self.sleeping.remove(i)?;
        Some((entry, Some(wake)))
    }

    /// Removes some entry, if there is one (to empty the scheduler).
    pub fn pop(&mut self) -> Option<T> {
        if let Some(&key) = self.waiting.keys().next() {
            return self.take_waiting(key);
        }
        let cpu = self.queues.iter().position(|q| !q.is_empty());
        let queued = cpu.and_then(|c| {
            let e = self.queues[c].pop_front()?;
            self.share.dequeued(c, e.1);
            Some(e)
        });
        queued
            .or_else(|| self.sleeping.pop_back())
            .map(|(_, _, e)| e)
    }

    /// The entry known by `key` (runnable, waiting or sleeping).
    pub fn find(&self, key: u32) -> Option<&T> {
        if let Some(i) = self.hinted(key) {
            return self.sleeping.get(i).map(|(_, _, e)| e);
        }
        if let Some((_, e)) = self.waiting.get(&key) {
            return Some(e);
        }
        self.queues
            .iter()
            .flatten()
            .chain(self.sleeping.iter())
            .find(|e| e.1 == key)
            .map(|(_, _, e)| e)
    }

    /// The entry known by `key` (runnable, waiting or sleeping), mutably.
    pub fn find_mut(&mut self, key: u32) -> Option<&mut T> {
        if let Some(i) = self.hinted(key) {
            return self.sleeping.get_mut(i).map(|(_, _, e)| e);
        }
        if let Some((_, e)) = self.waiting.get_mut(&key) {
            return Some(e);
        }
        self.queues
            .iter_mut()
            .flatten()
            .chain(self.sleeping.iter_mut())
            .find(|e| e.1 == key)
            .map(|(_, _, e)| e)
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

    impl Keyed for usize {
        fn key(&self) -> u32 {
            *self as u32
        }
    }

    impl Keyed for u32 {
        fn key(&self) -> u32 {
            *self
        }
    }

    impl Keyed for i32 {
        fn key(&self) -> u32 {
            *self as u32
        }
    }

    impl Keyed for char {
        fn key(&self) -> u32 {
            *self as u32
        }
    }

    impl<T: Keyed> Scheduler<T> {
        /// The entry known by `key`, and its wake time if it sleeps, by
        /// searching every place: what the hints must agree with.
        fn find_linear(&self, key: u32) -> Option<(&T, Option<u64>)> {
            if let Some((_, e)) = self.waiting.get(&key) {
                return Some((e, None));
            }
            let queued = self.queues.iter().flatten().map(|e| (e, None));
            let asleep = self.sleeping.iter().map(|e| (e, Some(e.0)));
            queued
                .chain(asleep)
                .find(|(e, _)| e.1 == key)
                .map(|((_, _, e), wake)| (e, wake))
        }
    }

    #[test]
    fn sleepers_are_found_by_key_through_stale_and_shared_slots() {
        let mut s = Scheduler::new(2);
        // Keys 1 and 9 share a slot once there are 8 slots.
        for key in [1u32, 2, 3, 9] {
            s.sleep(key, 100 + key as u64);
        }
        assert_eq!(s.hints.len(), 8);
        assert_eq!(s.find(1), Some(&1), "its slot holds 9's time");
        assert_eq!(s.find(9), Some(&9));
        // Woken: its slot still tells where it slept.
        assert_eq!(s.next(0, 102), Some(1));
        assert_eq!(s.next(0, 102), Some(2));
        s.requeue(1, 2);
        assert_eq!(s.find(2), Some(&2), "runnable now, found in a queue");
        assert_eq!(s.remove(2), Some((2, None)));
        s.sleep(2, 103); // the same time as 3
        assert_eq!(s.remove(2), Some((2, Some(103))));
        assert_eq!(s.remove(3), Some((3, Some(103))));
        assert_eq!(s.remove(2), None);
        assert_eq!(s.find(17), None, "a key no entry has");
        // Parked for good: many entries at the same time.
        for key in 20..40 {
            s.sleep(key, u64::MAX);
        }
        assert_eq!(s.remove(33), Some((33, Some(u64::MAX))));
        assert_eq!(s.len(), 1 + 19, "9 and the parked ones");
    }

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
    fn remove_finds_runnable_and_sleeping_entries_by_key() {
        let mut s = Scheduler::new(1);
        s.place(1);
        s.sleep(2, 10);
        assert_eq!(s.len(), 2);
        assert_eq!(s.find(2), Some(&2));
        // The key is the one the entry had when it was added.
        *s.find_mut(1).unwrap() = 3;
        assert_eq!(s.find(1), Some(&3));
        assert_eq!(s.find(3), None);
        *s.find_mut(1).unwrap() = 1;
        assert_eq!(s.remove(2), Some((2, Some(10))));
        assert_eq!(s.remove(9), None);
        assert_eq!(s.remove(1), Some((1, None)));
        assert!(s.is_empty());
    }

    #[test]
    fn pop_empties_every_place() {
        let mut s = Scheduler::new(2);
        s.place(1);
        s.place(2);
        s.sleep(3, 10);
        s.wait(4, 4, Some(5));
        let mut seen: Vec<i32> = core::iter::from_fn(|| s.pop()).collect();
        seen.sort_unstable();
        assert_eq!(seen, [1, 2, 3, 4]);
        assert!(s.is_empty());
        assert_eq!(s.next_wake(), None);
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
        /// The host takes this entry out by its key, if no CPU runs it, and
        /// puts it back where it was (asleep, or runnable).
        Host(usize),
    }

    fn op(cpus: usize) -> impl Strategy<Value = Op> {
        prop_oneof![
            (0..cpus).prop_map(Op::Take),
            (0..cpus).prop_map(Op::Preempt),
            (0..cpus, 1u64..20).prop_map(|(c, d)| Op::Sleep(c, d)),
            (1u64..10).prop_map(Op::Tick),
            (0usize..12).prop_map(Op::Host),
        ]
    }

    proptest! {
        /// Several CPUs taking, preempting and putting to sleep entries, and
        /// the host taking entries out by key and putting them back, in
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
                    Op::Host(e) if e < entries => {
                        let taken = s.remove(e as u32);
                        if running.contains(&Some(e)) {
                            prop_assert!(taken.is_none(), "{} is running", e);
                        } else {
                            let (got, wake_at) = taken.expect("not running: in the scheduler");
                            prop_assert_eq!(got, e);
                            match wake_at {
                                Some(t) => s.sleep(e, t),
                                None => {
                                    s.place(e);
                                }
                            }
                        }
                    }
                    _ => {}
                }
                // Every entry is in exactly one place.
                let mut seen = vec![0; entries];
                for e in running.iter().flatten() {
                    seen[*e] += 1;
                }
                for (e, n) in seen.iter_mut().enumerate() {
                    if s.find(e as u32).is_some() {
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
        s.wait(1, 'a', None);
        s.wait(2, 'b', Some(100));
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
        assert_eq!(s.find(1), Some(&'a'), "by the key it waits with");
        assert!(!s.wake(0, 2, |_| {}), "no longer waits");
        assert!(s.wake(0, 1, |_| {}));
        assert_eq!(s.waiting(), 0);
        assert_eq!(s.next(0, 0), Some('a'));
    }

    #[test]
    fn a_removed_waiter_comes_back_runnable() {
        let mut s = Scheduler::new(1);
        s.wait(7, 1, Some(10));
        *s.find_mut(7).unwrap() = 2;
        assert_eq!(s.remove(7), Some((2, None)));
        assert!(s.is_empty());
        assert_eq!(s.next_wake(), None, "its deadline went with it");
    }

    #[test]
    fn due_entries_are_shown_as_they_become_runnable() {
        let mut s = Scheduler::new(2);
        s.sleep(1, 50);
        s.sleep(2, 70);
        s.wait(3, 3, Some(60));
        s.wait(4, 4, None);
        let mut seen = Vec::new();
        s.wake_due(1, 65, |&mut e| seen.push(e));
        assert_eq!(seen, [1, 3], "the sleeper, then the waiter, both due");
        assert_eq!((s.runnable(), s.waiting()), (2, 1));
        assert_eq!(s.next(1, 65), Some(1), "on the waking CPU's queue");
        assert_eq!(s.next(1, 65), Some(3));
        assert_eq!(s.next(1, 65), None);
    }

    #[test]
    fn woken_waiters_join_the_waking_cpu() {
        let mut s = Scheduler::new(2);
        s.wait(1, 'w', None);
        s.wake(1, 1, |_| {});
        assert_eq!(s.next(1, 0), Some('w'));
    }

    #[test]
    fn only_the_named_waiters_wake() {
        let mut s = Scheduler::new(1);
        for key in 1..=4 {
            s.wait(key, key, None);
        }
        let mut seen = Vec::new();
        for key in [3, 1, 9] {
            s.wake(0, key, |&mut e| seen.push(e));
        }
        assert_eq!(seen, [3, 1], "a key that does not wait wakes nobody");
        assert_eq!(s.waiting(), 2);
        assert_eq!(s.next(0, 0), Some(3), "in the order they were woken");
        assert_eq!(s.next(0, 0), Some(1));
        assert_eq!(s.next(0, 0), None);
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

    /// What happens to the scheduler in [`hints_agree_with_a_search`].
    #[derive(Debug, Clone)]
    enum Move {
        Place(u32),
        Sleep(u32, u64),
        Wait(u32, Option<u64>),
        Next(usize, u64),
        Wake(usize, u32),
        Remove(u32),
        Change(u32),
    }

    fn a_move() -> impl Strategy<Value = Move> {
        // Few keys, spread so that several share each slot.
        let key = (0u32..12, 0u32..3).prop_map(|(k, m)| k + 64 * m);
        prop_oneof![
            key.clone().prop_map(Move::Place),
            (key.clone(), 0u64..40).prop_map(|(k, t)| Move::Sleep(k, t)),
            (key.clone(), proptest::option::of(0u64..40)).prop_map(|(k, d)| Move::Wait(k, d)),
            (0usize..2, 0u64..40).prop_map(|(c, t)| Move::Next(c, t)),
            (0usize..2, key.clone()).prop_map(|(c, k)| Move::Wake(c, k)),
            key.clone().prop_map(Move::Remove),
            key.prop_map(Move::Change),
        ]
    }

    proptest! {
        /// Whatever entries come and go, finding, changing and removing by
        /// key give what a search of every place gives.
        #[test]
        fn hints_agree_with_a_search(moves in proptest::collection::vec(a_move(), 1..200)) {
            // Entries are (key, version): the key is what the scheduler
            // keeps, the version what a change made.
            #[derive(Debug, Clone, Copy, PartialEq)]
            struct E(u32, u32);
            impl Keyed for E {
                fn key(&self) -> u32 {
                    self.0
                }
            }
            let mut s = Scheduler::new(2);
            let mut held = std::collections::BTreeSet::new();
            let mut now = 0;
            for m in moves {
                match m {
                    Move::Place(k) if held.insert(k) => { s.place(E(k, 0)); }
                    Move::Sleep(k, t) if held.insert(k) => s.sleep(E(k, 0), now + t),
                    Move::Wait(k, d) if held.insert(k) => s.wait(k, E(k, 0), d.map(|d| now + d)),
                    Move::Next(cpu, t) => {
                        now += t;
                        // Run and give back at once, or leave it out.
                        if let Some(e) = s.next(cpu, now) {
                            if e.0 % 2 == 0 { s.requeue(cpu, e); } else { held.remove(&e.0); }
                        }
                    }
                    Move::Wake(cpu, k) => { s.wake(cpu, k, |_| {}); }
                    Move::Remove(k) => {
                        let want = s.find_linear(k).map(|(&e, w)| (e, w));
                        prop_assert_eq!(s.remove(k), want);
                        held.remove(&k);
                    }
                    Move::Change(k) => {
                        if let Some(e) = s.find_mut(k) {
                            e.1 += 1;
                        }
                    }
                    _ => {}
                }
                for k in (0..12).flat_map(|k| [k, k + 64, k + 128]) {
                    let want = s.find_linear(k).map(|(e, _)| e);
                    prop_assert_eq!(s.find(k), want, "key {}", k);
                    prop_assert_eq!(want.is_some(), held.contains(&k), "key {}", k);
                }
            }
        }

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
                s.wait(id as u32, id, deadline);
            }
            let mut seen = vec![false; ops.len()];
            for now in 0..=50u64 {
                let woken = now as usize % ops.len();
                if ops[woken].1 && now % 7 == 0 {
                    s.wake(0, woken as u32, |_| {});
                    while let Some(id) = s.next(0, now) {
                        prop_assert!(!seen[id]);
                        prop_assert!(id == woken || ops[id].0.is_some_and(|d| d <= now));
                        seen[id] = true;
                    }
                }
                while let Some(id) = s.next(0, now) {
                    prop_assert!(!seen[id], "entry {} ran twice", id);
                    prop_assert!(ops[id].0.is_some_and(|d| d <= now), "entry {} woke early", id);
                    seen[id] = true;
                }
            }
            for id in 0..ops.len() {
                s.wake(0, id as u32, |_| {});
            }
            while let Some(id) = s.next(0, 50) {
                prop_assert!(!seen[id]);
                prop_assert!(ops[id].0.is_none());
                seen[id] = true;
            }
            prop_assert!(seen.iter().all(|&s| s));
        }
    }
}
