//! The threads of a cell, as plain data (spec `threads`, design D1–D3 of
//! `add-threads`): their registers, identifiers and what each waits for.
//!
//! The thread that runs (or ran last) is kept inline and the others in a
//! list, so a single-threaded cell allocates nothing. Time is passed in,
//! never read.

use crate::Registers;
use alloc::vec::Vec;

/// `-ETIMEDOUT`, the result of a futex wait that timed out.
const ETIMEDOUT: u64 = -110i64 as u64;

/// What a thread waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wait {
    /// Nothing: it can run.
    Ready,
    /// A sleep, until this monotonic time (nanoseconds).
    Sleep(u64),
    /// A futex wake on the word at `addr` with a bitset intersecting
    /// `bitset`, or the monotonic time `until`.
    Futex {
        addr: u64,
        bitset: u32,
        until: Option<u64>,
    },
}

impl Wait {
    fn deadline(self) -> Option<u64> {
        match self {
            Wait::Ready => None,
            Wait::Sleep(until) => Some(until),
            Wait::Futex { until, .. } => until,
        }
    }
}

/// One thread of a cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thread {
    /// Identifier, unique among the threads the cell ever had.
    pub tid: u32,
    /// Saved user registers (FP/SIMD state and thread pointer included).
    pub regs: Registers,
    /// Address of the 32-bit word cleared and woken when the thread exits
    /// (`set_tid_address`, `CLONE_CHILD_CLEARTID`); 0 for none.
    pub clear_tid: u64,
    /// What it waits for.
    pub wait: Wait,
}

impl Thread {
    /// A runnable thread with no clear-tid address.
    pub fn new(tid: u32, regs: Registers) -> Self {
        Self {
            tid,
            regs,
            clear_tid: 0,
            wait: Wait::Ready,
        }
    }

    /// Makes the thread runnable if its deadline is at or before `now`; a
    /// futex wait that times out returns `-ETIMEDOUT`.
    fn expire(&mut self, now: u64) {
        if self.wait.deadline().is_some_and(|t| t <= now) {
            if let Wait::Futex { .. } = self.wait {
                self.regs.gpr[0] = ETIMEDOUT;
            }
            self.wait = Wait::Ready;
        }
    }
}

/// The threads of a cell: never empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Threads {
    current: Thread,
    /// The other threads, in the order they run after the current one.
    others: Vec<Thread>,
    next_tid: u32,
}

impl Threads {
    /// A cell's first thread.
    pub fn new(first: Thread) -> Self {
        let next_tid = first.tid.saturating_add(1);
        Self {
            current: first,
            others: Vec::new(),
            next_tid,
        }
    }

    /// Rebuilds a table from its parts, the current thread first (a
    /// snapshot); `None` if `threads` is empty.
    pub fn from_parts(threads: Vec<Thread>, next_tid: u32) -> Option<Self> {
        let mut threads = threads.into_iter();
        let current = threads.next()?;
        Some(Self {
            current,
            others: threads.collect(),
            next_tid,
        })
    }

    /// The thread that runs, or ran last.
    pub fn current(&self) -> &Thread {
        &self.current
    }

    /// The thread that runs, or ran last, mutably.
    pub fn current_mut(&mut self) -> &mut Thread {
        &mut self.current
    }

    /// Every thread, the current one first, then in the order they run.
    pub fn iter(&self) -> impl Iterator<Item = &Thread> {
        core::iter::once(&self.current).chain(&self.others)
    }

    /// The thread `tid`, mutably.
    pub fn get_mut(&mut self, tid: u32) -> Option<&mut Thread> {
        core::iter::once(&mut self.current)
            .chain(&mut self.others)
            .find(|t| t.tid == tid)
    }

    /// Number of threads.
    pub fn len(&self) -> usize {
        1 + self.others.len()
    }

    /// Always false: a cell has at least one thread.
    pub fn is_empty(&self) -> bool {
        false
    }

    /// The identifier the next thread will get.
    pub fn next_tid(&self) -> u32 {
        self.next_tid
    }

    /// Adds a runnable thread with `regs`, after every other; returns its
    /// identifier, or `None` when identifiers are exhausted.
    pub fn spawn(&mut self, regs: Registers, clear_tid: u64) -> Option<u32> {
        let tid = self.next_tid;
        if tid > i32::MAX as u32 {
            return None; // identifiers are positive `pid_t` values
        }
        self.next_tid += 1;
        self.others.push(Thread {
            clear_tid,
            ..Thread::new(tid, regs)
        });
        Some(tid)
    }

    /// Removes the current thread and returns it, unless it is the last
    /// one. The thread after it becomes current (it may not be runnable).
    pub fn exit_current(&mut self) -> Option<Thread> {
        if self.others.is_empty() {
            return None;
        }
        let next = self.others.remove(0);
        Some(core::mem::replace(&mut self.current, next))
    }

    /// Makes the current thread wait for `wait`.
    pub fn block(&mut self, wait: Wait) {
        self.current.wait = wait;
    }

    /// Wakes up to `n` threads waiting on the futex word at `addr` with a
    /// bitset intersecting `bitset`, in the order they run after the
    /// current one; their wait returns 0. Returns how many it woke.
    pub fn wake(&mut self, addr: u64, bitset: u32, n: u32) -> u32 {
        let mut woken = 0;
        for t in self
            .others
            .iter_mut()
            .chain(core::iter::once(&mut self.current))
        {
            if woken == n {
                break;
            }
            if let Wait::Futex {
                addr: a, bitset: b, ..
            } = t.wait
                && a == addr
                && b & bitset != 0
            {
                t.wait = Wait::Ready;
                t.regs.gpr[0] = 0;
                woken += 1;
            }
        }
        woken
    }

    /// Wakes the threads whose deadline is at or before `now`, then makes
    /// current the next runnable thread after the current one (the current
    /// one last). Returns whether a thread can run.
    pub fn next(&mut self, now: u64) -> bool {
        self.current.expire(now);
        let mut found = None;
        for (i, t) in self.others.iter_mut().enumerate() {
            t.expire(now);
            if found.is_none() && t.wait == Wait::Ready {
                found = Some(i);
            }
        }
        match found {
            Some(i) => {
                // The current thread goes last; the ones skipped keep their
                // order before it.
                let next = self.others.remove(i);
                let previous = core::mem::replace(&mut self.current, next);
                self.others.push(previous);
                true
            }
            None => self.current.wait == Wait::Ready,
        }
    }

    /// Whether a thread can run without waiting.
    pub fn is_runnable(&self) -> bool {
        self.iter().any(|t| t.wait == Wait::Ready)
    }

    /// The earliest deadline among waiting threads.
    pub fn wake_at(&self) -> Option<u64> {
        self.iter().filter_map(|t| t.wait.deadline()).min()
    }

    /// Ends every sleep and times out every timed futex wait (a template is
    /// instantiated at an arbitrary time: its deadlines mean nothing).
    pub fn expire_timers(&mut self) {
        for t in core::iter::once(&mut self.current).chain(&mut self.others) {
            t.expire(u64::MAX);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn regs(mark: u64) -> Registers {
        let mut r = Registers::default();
        r.gpr[0] = mark;
        r
    }

    fn futex(addr: u64, bitset: u32, until: Option<u64>) -> Wait {
        Wait::Futex {
            addr,
            bitset,
            until,
        }
    }

    /// Three threads: 7 (current), 8, 9.
    fn three() -> Threads {
        let mut t = Threads::new(Thread::new(7, regs(0)));
        assert_eq!(t.spawn(regs(1), 0), Some(8));
        assert_eq!(t.spawn(regs(2), 0x1000), Some(9));
        t
    }

    fn order(t: &Threads) -> Vec<u32> {
        t.iter().map(|t| t.tid).collect()
    }

    #[test]
    fn threads_take_turns_in_creation_order() {
        let mut t = three();
        let mut ran = Vec::new();
        for _ in 0..6 {
            assert!(t.next(0));
            ran.push(t.current().tid);
        }
        assert_eq!(ran, [8, 9, 7, 8, 9, 7]);
        assert_eq!(t.len(), 3);
    }

    #[test]
    fn a_single_thread_keeps_running_and_allocates_nothing() {
        let mut t = Threads::new(Thread::new(5, regs(0)));
        assert!(t.next(0));
        assert_eq!(t.current().tid, 5);
        assert_eq!(t.others.capacity(), 0);
        t.block(Wait::Sleep(100));
        assert!(!t.next(99), "asleep until 100");
        assert_eq!(t.wake_at(), Some(100));
        assert!(t.next(100));
        assert_eq!(t.wake_at(), None);
    }

    #[test]
    fn blocked_threads_are_skipped_and_woken_by_a_matching_wake() {
        let mut t = three();
        t.block(futex(0x2000, u32::MAX, None)); // 7 waits
        assert!(t.next(0));
        assert_eq!(t.current().tid, 8);
        t.block(futex(0x2000, 0b01, None)); // 8 waits, bitset 1
        assert!(t.next(0));
        assert_eq!(t.current().tid, 9);
        t.block(futex(0x3000, u32::MAX, None)); // 9 waits elsewhere
        assert!(!t.next(0), "everyone waits");
        assert!(!t.is_runnable());
        assert_eq!(t.wake_at(), None, "no deadline: parked");

        assert_eq!(t.wake(0x2000, 0b10, 5), 1, "only 7's bitset matches");
        assert!(t.next(0));
        assert_eq!(t.current().tid, 7);
        assert_eq!(t.current().regs.gpr[0], 0, "the wait returns 0");
        assert_eq!(t.wake(0x2000, u32::MAX, 5), 1, "8 is the last waiter");
        assert_eq!(t.wake(0x2000, u32::MAX, 5), 0);
        assert_eq!(t.wake(0x3000, u32::MAX, 0), 0, "a wake of 0 wakes none");
    }

    #[test]
    fn wakes_go_in_turn_order_up_to_n() {
        let mut t = three();
        assert!(t.next(0)); // 8 current; order 8, 9, 7
        for _ in 0..3 {
            t.block(futex(0x40, 1, None));
            t.next(0);
        }
        assert!(!t.is_runnable());
        assert_eq!(order(&t), [7, 8, 9]);
        assert_eq!(t.wake(0x40, 1, 2), 2);
        let woken: Vec<u32> = t
            .iter()
            .filter(|t| t.wait == Wait::Ready)
            .map(|t| t.tid)
            .collect();
        assert_eq!(woken, [8, 9], "the threads after the current one first");
    }

    #[test]
    fn timeouts_return_etimedout_and_sleeps_return_as_set() {
        let mut t = three();
        t.current_mut().regs.gpr[0] = 0;
        t.block(Wait::Sleep(50)); // 7
        t.next(0);
        t.block(futex(0x10, 1, Some(30))); // 8
        t.next(0);
        t.block(futex(0x10, 1, Some(80))); // 9
        assert!(!t.next(10));
        assert_eq!(t.wake_at(), Some(30));
        assert!(t.next(30));
        assert_eq!(t.current().tid, 8);
        assert_eq!(t.current().regs.gpr[0], ETIMEDOUT);
        assert_eq!(t.wake_at(), Some(50));
        t.block(Wait::Sleep(1000));
        assert!(t.next(60));
        assert_eq!(t.current().tid, 7);
        assert_eq!(t.current().regs.gpr[0], 0, "a sleep's result stays");
    }

    #[test]
    fn exit_removes_the_current_thread_but_never_the_last() {
        let mut t = three();
        let gone = t.exit_current().expect("not the last");
        assert_eq!(gone.tid, 7);
        assert_eq!(order(&t), [8, 9]);
        assert_eq!(t.exit_current().map(|g| g.tid), Some(8));
        assert_eq!(t.exit_current(), None, "the last thread stays");
        assert_eq!(t.current().tid, 9);
        assert_eq!(t.current().clear_tid, 0x1000);
        assert_eq!(t.spawn(regs(3), 0), Some(10), "identifiers are not reused");
        assert_eq!(t.get_mut(10).map(|t| t.regs.gpr[0]), Some(3));
        assert!(t.get_mut(7).is_none());
    }

    #[test]
    fn templates_expire_every_deadline_but_untimed_waits() {
        let mut t = three();
        t.block(Wait::Sleep(u64::MAX - 1));
        t.next(0);
        t.block(futex(0x10, 1, Some(1 << 60)));
        t.next(0);
        t.block(futex(0x10, 1, None));
        t.expire_timers();
        let waits: Vec<Wait> = t.iter().map(|t| t.wait).collect();
        assert_eq!(waits, [futex(0x10, 1, None), Wait::Ready, Wait::Ready]);
        assert_eq!(t.get_mut(8).unwrap().regs.gpr[0], ETIMEDOUT);
    }

    #[test]
    fn identifiers_stay_positive() {
        let mut t = Threads::new(Thread::new(i32::MAX as u32, regs(0)));
        assert_eq!(t.spawn(regs(0), 0), None);
        let copy = Threads::from_parts(t.iter().cloned().collect(), t.next_tid()).unwrap();
        assert_eq!(copy, t, "a table rebuilt from its parts is the same");
        assert!(Threads::from_parts(Vec::new(), 1).is_none());
    }

    proptest! {
        /// Whatever threads block and wake, every runnable thread runs
        /// within one turn of all the others, and no blocked thread runs.
        #[test]
        fn runnable_threads_all_run_and_blocked_ones_never(
            blocked in proptest::collection::vec(any::<bool>(), 1..8)
        ) {
            let mut t = Threads::new(Thread::new(1, regs(0)));
            for _ in 1..blocked.len() {
                t.spawn(regs(0), 0);
            }
            for (i, &b) in blocked.iter().enumerate() {
                if b {
                    t.get_mut(1 + i as u32).unwrap().wait = futex(0x10, 1, None);
                }
            }
            let ready = blocked.iter().filter(|&&b| !b).count();
            let mut seen = std::collections::BTreeSet::new();
            for _ in 0..ready {
                prop_assert!(t.next(0));
                prop_assert_eq!(t.current().wait, Wait::Ready);
                seen.insert(t.current().tid);
            }
            prop_assert_eq!(seen.len(), ready, "each runnable thread ran once");
            prop_assert_eq!(t.next(0), ready > 0);
        }
    }
}
