//! Which idle CPU programs its timer (design D1 of `add-tickless-idle`).
//!
//! An idle CPU has no periodic tick. Of all idle CPUs, one at most holds a
//! timer for the earliest deadline of the cells that are not running; the
//! others wait for a wake-up interrupt only. A CPU that takes work while a
//! deadline is not covered by an idle CPU's timer names an idle CPU to wake,
//! which then takes the deadline over. Like [`Scheduler`](crate::Scheduler),
//! it reads no clock: the caller passes the earliest deadline.

/// The idle CPU whose timer covers the earliest deadline, and that
/// deadline.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IdleTimer {
    armed: Option<(usize, u64)>,
}

impl IdleTimer {
    /// No CPU holds a timer (between runs).
    pub const fn new() -> Self {
        Self { armed: None }
    }

    /// `cpu` looks for work (it was woken, or never waited): a timer it held
    /// covers nothing any more.
    pub fn leave(&mut self, cpu: usize) {
        if self.armed.is_some_and(|(c, _)| c == cpu) {
            self.armed = None;
        }
    }

    /// `cpu` goes idle while `next` is the earliest deadline: the time its
    /// timer must fire at, when no other CPU's timer covers `next` yet (it
    /// then holds it), else `None` (it waits for a wake-up only).
    pub fn idle(&mut self, cpu: usize, next: Option<u64>) -> Option<u64> {
        let at = next?;
        if self.covers(at) {
            return None;
        }
        self.armed = Some((cpu, at));
        Some(at)
    }

    /// A CPU takes work while `next` is the earliest deadline and `idle`
    /// (bit `n` for CPU `n`) the other CPUs that wait: the idle CPU to wake
    /// so that it holds a timer for `next`, when none covers it. That CPU
    /// counts as holding it until it looks for work. The CPU whose timer is
    /// later is chosen first, so that no CPU keeps a timer nobody needs;
    /// else the lowest idle one.
    pub fn cover(&mut self, next: Option<u64>, idle: u32) -> Option<usize> {
        let at = next?;
        if idle == 0 || self.covers(at) {
            return None;
        }
        let cpu = match self.armed {
            Some((c, _)) if idle & (1 << c) != 0 => c,
            _ => idle.trailing_zeros() as usize,
        };
        self.armed = Some((cpu, at));
        Some(cpu)
    }

    /// The CPU holding a timer and when it fires.
    pub fn armed(&self) -> Option<(usize, u64)> {
        self.armed
    }

    fn covers(&self, at: u64) -> bool {
        self.armed.is_some_and(|(_, a)| a <= at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn one_idle_cpu_holds_the_timer() {
        let mut t = IdleTimer::new();
        assert_eq!(t.idle(0, Some(100)), Some(100));
        assert_eq!(t.idle(1, Some(100)), None, "CPU 0 covers it");
        assert_eq!(t.idle(2, None), None, "no deadline: no timer");
        assert_eq!(t.armed(), Some((0, 100)));
    }

    #[test]
    fn an_earlier_deadline_moves_the_timer() {
        let mut t = IdleTimer::new();
        t.idle(0, Some(100));
        assert_eq!(t.idle(1, Some(50)), Some(50));
        assert_eq!(t.armed(), Some((1, 50)));
    }

    #[test]
    fn the_holder_taking_work_hands_the_deadline_over() {
        let mut t = IdleTimer::new();
        t.idle(0, Some(100));
        assert_eq!(t.idle(1, Some(100)), None);
        t.leave(0); // CPU 0 woken for work
        assert_eq!(t.cover(Some(100), 0b10), Some(1));
        t.leave(1); // CPU 1 woken: looks again, finds nothing
        assert_eq!(t.idle(1, Some(100)), Some(100));
    }

    #[test]
    fn a_later_timer_is_woken_first() {
        let mut t = IdleTimer::new();
        t.idle(2, Some(100));
        // A busy CPU filed a cell due at 50 and keeps running.
        assert_eq!(t.cover(Some(50), 0b110), Some(2));
        assert_eq!(t.cover(Some(50), 0b110), None, "CPU 2 is on its way");
    }

    #[test]
    fn nothing_to_cover() {
        let mut t = IdleTimer::new();
        assert_eq!(t.cover(None, 0b11), None);
        assert_eq!(t.cover(Some(5), 0), None, "nobody idles");
        t.leave(3); // holds nothing: no effect
        assert_eq!(t.armed(), None);
    }

    /// What one CPU does in the model.
    #[derive(Debug, Clone, Copy, PartialEq)]
    enum Cpu {
        Busy,
        /// Waiting, with its timer (if any).
        Idle(Option<u64>),
        /// Idle, and a wake-up was sent to it.
        Woken,
    }

    /// A CPU of the model looks for work: it finds some (`work`), or goes
    /// idle; the earliest deadline is `next`.
    fn take(t: &mut IdleTimer, cpus: &mut [Cpu], cpu: usize, work: bool, next: Option<u64>) {
        t.leave(cpu);
        if work {
            cpus[cpu] = Cpu::Busy;
            let idle = cpus
                .iter()
                .enumerate()
                .filter(|(_, c)| matches!(c, Cpu::Idle(_)))
                .fold(0u32, |m, (i, _)| m | 1 << i);
            if let Some(other) = t.cover(next, idle) {
                cpus[other] = Cpu::Woken;
            }
        } else {
            cpus[cpu] = Cpu::Idle(t.idle(cpu, next));
        }
    }

    proptest! {
        /// Whatever CPUs do, while one idles and a deadline exists, an idle
        /// CPU's timer fires no later than the deadline, or an idle CPU was
        /// sent a wake-up (it will look again): no deadline is left to the
        /// end of a busy CPU's slice.
        #[test]
        fn an_idle_cpu_always_covers_the_earliest_deadline(
            ops in proptest::collection::vec((0usize..4, any::<bool>(), proptest::option::of(0u64..100)), 1..200)
        ) {
            let mut t = IdleTimer::new();
            let mut cpus = [Cpu::Busy; 4];
            let mut next: Option<u64> = None;
            for (cpu, work, deadline) in ops {
                // A woken or busy CPU looks for work; an idle CPU looks only
                // when its timer fires (its deadline is then consumed).
                match cpus[cpu] {
                    Cpu::Idle(None) => continue,
                    Cpu::Idle(Some(at)) => {
                        if next == Some(at) {
                            next = deadline.map(|d| d.max(at));
                        }
                    }
                    Cpu::Busy => {
                        // It may have filed a cell with a new deadline.
                        if let Some(d) = deadline {
                            next = Some(next.map_or(d, |n| n.min(d)));
                        }
                    }
                    Cpu::Woken => {}
                }
                take(&mut t, &mut cpus, cpu, work, next);
                let idle = cpus.iter().any(|c| matches!(c, Cpu::Idle(_)));
                if let (true, Some(n)) = (idle, next) {
                    let covered = cpus.iter().any(|c| match c {
                        Cpu::Idle(Some(at)) => *at <= n,
                        Cpu::Woken => true,
                        _ => false,
                    });
                    prop_assert!(covered, "deadline {n} uncovered: {cpus:?} {t:?}");
                }
            }
        }
    }
}
