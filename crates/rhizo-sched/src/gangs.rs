//! Entries running on several CPUs at once (design D1 and D5 of
//! `add-parallel-threads`): cells taken out of the run queues whose threads
//! more than one CPU runs. Like [`Scheduler`](crate::Scheduler), a pure
//! structure used under the host's lock; time is passed in.

use alloc::vec::Vec;

/// One entry and the CPUs that run it.
#[derive(Debug, Clone)]
struct Gang<T> {
    entry: T,
    /// CPUs running it (bit `n` for CPU `n`).
    cpus: u32,
    /// Runnable threads no CPU runs: what it offers to other CPUs.
    spare: u32,
    /// The earliest deadline of its waiting threads, if any.
    wake: Option<u64>,
}

/// The entries running on one or more CPUs that other CPUs may join, in
/// the order they started.
#[derive(Debug, Clone)]
pub struct Gangs<T> {
    gangs: Vec<Gang<T>>,
}

impl<T> Default for Gangs<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Gangs<T> {
    /// No entry.
    pub const fn new() -> Self {
        Self { gangs: Vec::new() }
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.gangs.len()
    }

    /// Whether there is no entry.
    pub fn is_empty(&self) -> bool {
        self.gangs.is_empty()
    }

    /// The earliest deadline any entry offers.
    pub fn next_wake(&self) -> Option<u64> {
        self.gangs.iter().filter_map(|g| g.wake).min()
    }
}

impl<T: Copy + PartialEq> Gangs<T> {
    fn find(&mut self, entry: T) -> Option<&mut Gang<T>> {
        self.gangs.iter_mut().find(|g| g.entry == entry)
    }

    /// `entry`, just taken out of the run queues, runs on `cpu` and offers
    /// nothing yet.
    pub fn start(&mut self, cpu: usize, entry: T) {
        self.gangs.push(Gang {
            entry,
            cpus: 1 << cpu,
            spare: 0,
            wake: None,
        });
    }

    /// Records what `entry` offers: `spare` runnable threads no CPU runs,
    /// and the earliest deadline `wake` of its waiting threads.
    pub fn offer(&mut self, entry: T, spare: u32, wake: Option<u64>) {
        if let Some(g) = self.find(entry) {
            g.spare = spare;
            g.wake = wake;
        }
    }

    /// The entry `cpu` (which runs none) joins at `now`: the first started
    /// that offers a thread or whose deadline has come. The CPU is counted
    /// in and takes one offered thread (or the deadline: another CPU does
    /// not join for it).
    pub fn join(&mut self, cpu: usize, now: u64) -> Option<T> {
        self.join_where(cpu, now, |_| true)
    }

    /// [`join`](Self::join), among the entries `may` allows `cpu` to run
    /// (exclusive CPU sets, design D17 of `add-cell-hierarchy`).
    pub fn join_where(&mut self, cpu: usize, now: u64, may: impl Fn(T) -> bool) -> Option<T> {
        let g = self
            .gangs
            .iter_mut()
            .find(|g| (g.spare > 0 || g.wake.is_some_and(|w| w <= now)) && may(g.entry))?;
        if g.spare > 0 {
            g.spare -= 1;
        } else {
            g.wake = None;
        }
        g.cpus |= 1 << cpu;
        Some(g.entry)
    }

    /// The CPUs running `entry` (bit `n` for CPU `n`).
    pub fn cpus(&self, entry: T) -> u32 {
        self.gangs
            .iter()
            .find(|g| g.entry == entry)
            .map_or(0, |g| g.cpus)
    }

    /// `cpu` stops running `entry`. Returns `true` when it was the last
    /// CPU: the entry is forgotten, and the caller owns it alone.
    pub fn leave(&mut self, cpu: usize, entry: T) -> bool {
        let Some(i) = self.gangs.iter().position(|g| g.entry == entry) else {
            return false;
        };
        let g = &mut self.gangs[i];
        g.cpus &= !(1 << cpu);
        if g.cpus != 0 {
            return false;
        }
        self.gangs.remove(i);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn idle_cpus_join_what_is_offered_and_the_last_leaver_owns_it() {
        let mut g = Gangs::new();
        g.start(0, 'a');
        g.start(1, 'b');
        assert_eq!(g.join(2, 0), None, "nothing offered");
        g.offer('b', 2, None);
        g.offer('a', 1, None);
        assert_eq!(g.join(2, 0), Some('a'), "the first started first");
        assert_eq!(g.join(3, 0), Some('b'));
        assert_eq!(g.cpus('a'), 0b101);
        assert_eq!(g.cpus('b'), 0b1010);
        assert!(!g.leave(0, 'a'));
        assert!(g.leave(2, 'a'), "the last CPU gets it back");
        assert_eq!(g.cpus('a'), 0);
        assert_eq!(g.len(), 1);
        assert_eq!(g.join(0, 0), Some('b'), "one thread still offered");
        assert_eq!(g.join(2, 0), None);
    }

    #[test]
    fn exclusive_sets_keep_cpus_out_of_gangs_they_may_not_run() {
        let mut g = Gangs::new();
        g.start(2, 'a'); // a domain with a set: CPUs 2 and 3
        g.start(0, 'b');
        g.offer('a', 2, None);
        g.offer('b', 1, None);
        let set = |e: char| e != 'a';
        assert_eq!(
            g.join_where(1, 0, set),
            Some('b'),
            "past 'a', offered first"
        );
        assert_eq!(g.join_where(1, 0, set), None, "only 'a' offers now");
        assert_eq!(g.join_where(3, 0, |e| e == 'a'), Some('a'));
        assert_eq!(g.cpus('a'), 0b1100);
    }

    #[test]
    fn deadlines_bring_one_cpu_each() {
        let mut g = Gangs::new();
        g.start(0, 'a');
        g.offer('a', 0, Some(100));
        assert_eq!(g.next_wake(), Some(100));
        assert_eq!(g.join(1, 99), None, "not yet");
        assert_eq!(g.join(1, 100), Some('a'));
        assert_eq!(g.join(2, 100), None, "the first CPU takes the deadline");
        assert_eq!(g.next_wake(), None);
    }

    /// A step of a simulated CPU.
    #[derive(Debug, Clone, Copy)]
    enum Op {
        /// The CPU, running nothing, starts a new entry.
        Start(usize),
        /// The CPU, running nothing, joins one at time `now`.
        Join(usize, u64),
        /// The CPU's entry offers this much.
        Offer(usize, u32, Option<u64>),
        /// The CPU leaves its entry.
        Leave(usize),
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            (0..4usize).prop_map(Op::Start),
            (0..4usize, 0..20u64).prop_map(|(c, t)| Op::Join(c, t)),
            (0..4usize, 0..3u32, proptest::option::of(0..20u64))
                .prop_map(|(c, s, w)| Op::Offer(c, s, w)),
            (0..4usize).prop_map(Op::Leave),
        ]
    }

    proptest! {
        /// Any interleaving of CPUs starting, joining, offering and
        /// leaving: a CPU runs at most one entry, joins only an entry that
        /// offers a thread or a due deadline (and takes it), and exactly
        /// the last CPU to leave an entry gets it back, once.
        #[test]
        fn interleaved_cpus_agree_with_a_model(ops in proptest::collection::vec(op(), 1..300)) {
            let mut g = Gangs::new();
            // Model: the entry each CPU runs; per entry, its offer.
            let mut runs: [Option<u32>; 4] = [None; 4];
            let mut offers: Vec<(u32, u32, Option<u64>)> = Vec::new();
            let mut next = 0u32;
            let mut returned = Vec::new();
            for op in ops {
                match op {
                    Op::Start(c) if runs[c].is_none() => {
                        g.start(c, next);
                        offers.push((next, 0, None));
                        runs[c] = Some(next);
                        next += 1;
                    }
                    Op::Join(c, now) if runs[c].is_none() => {
                        let expect = offers
                            .iter()
                            .position(|&(_, s, w)| s > 0 || w.is_some_and(|w| w <= now));
                        let got = g.join(c, now);
                        prop_assert_eq!(got, expect.map(|i| offers[i].0));
                        if let Some(i) = expect {
                            if offers[i].1 > 0 { offers[i].1 -= 1 } else { offers[i].2 = None }
                            runs[c] = got;
                        }
                    }
                    Op::Offer(c, s, w) => {
                        if let Some(e) = runs[c] {
                            g.offer(e, s, w);
                            let o = offers.iter_mut().find(|o| o.0 == e).unwrap();
                            (o.1, o.2) = (s, w);
                        }
                    }
                    Op::Leave(c) => {
                        if let Some(e) = runs[c].take() {
                            let last = runs.iter().all(|r| *r != Some(e));
                            prop_assert_eq!(g.leave(c, e), last);
                            if last {
                                prop_assert!(!returned.contains(&e), "returned twice");
                                returned.push(e);
                                offers.retain(|o| o.0 != e);
                            }
                        }
                    }
                    _ => {}
                }
                prop_assert_eq!(g.len(), offers.len());
                for &(e, _, _) in &offers {
                    let mask = (0..4).filter(|&c| runs[c] == Some(e)).fold(0, |m, c| m | 1 << c);
                    prop_assert_eq!(g.cpus(e), mask);
                    prop_assert!(mask != 0, "a live entry has a CPU");
                }
                prop_assert_eq!(g.next_wake(), offers.iter().filter_map(|o| o.2).min());
            }
        }
    }
}
