//! Cost accounting of the kernel (spec `profiling`): where the time of a
//! workload goes, split into parts (cell code, system calls, page faults,
//! copy-on-write, the frame allocator, copies, the network, the scheduler,
//! lock waits, cell operations, teardown, the rest), and per system call.
//!
//! An [`Account`] is one CPU's: the kernel opens and closes scopes on it as
//! it moves between parts, passing the time each time (in counter ticks),
//! and every tick between two calls goes to the innermost open part. So the
//! parts are exclusive: they never count the same tick twice and add up to
//! the time accounted. Nothing here reads a clock, so the logic is tested on
//! the host.

#![cfg_attr(not(test), no_std)]

use core::fmt::{self, Write};

/// Number of [`Part`]s.
pub const PARTS: usize = 13;

/// Where a CPU's time goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Part {
    /// Cell code, with the kernel's entry and exit path around it.
    Cell,
    /// Handling a system call other than a socket call.
    Syscall,
    /// A page fault: a lazily resumed page coming back, or the stack growing.
    Fault,
    /// Resolving a copy-on-write fault.
    Cow,
    /// The frame allocator and frame reference counts.
    Frames,
    /// Copying or zeroing memory through the direct map.
    Copy,
    /// Socket calls and polling the network card.
    Net,
    /// Picking a cell to run and putting it back.
    Sched,
    /// Waiting for work (halted).
    Idle,
    /// Waiting for a lock another CPU holds.
    Lock,
    /// Cell operations: spawn, fork, suspend, resume.
    Spawn,
    /// Ending a cell: its address space and frames freed.
    Teardown,
    /// Everything else in the kernel.
    Other,
}

impl Part {
    /// Every part, in report order.
    pub const ALL: [Part; PARTS] = [
        Part::Cell,
        Part::Syscall,
        Part::Fault,
        Part::Cow,
        Part::Frames,
        Part::Copy,
        Part::Net,
        Part::Sched,
        Part::Idle,
        Part::Lock,
        Part::Spawn,
        Part::Teardown,
        Part::Other,
    ];

    /// Name in reports and metric names.
    pub fn name(self) -> &'static str {
        match self {
            Part::Cell => "cell",
            Part::Syscall => "syscall",
            Part::Fault => "fault",
            Part::Cow => "cow",
            Part::Frames => "frames",
            Part::Copy => "copy",
            Part::Net => "net",
            Part::Sched => "sched",
            Part::Idle => "idle",
            Part::Lock => "lock",
            Part::Spawn => "spawn",
            Part::Teardown => "teardown",
            Part::Other => "other",
        }
    }
}

/// Deepest nesting of scopes kept (deeper ones go to their parent).
pub const DEPTH: usize = 8;

/// One CPU's account: time per part, exclusive and inclusive, and entries.
#[derive(Debug, Clone)]
pub struct Account {
    /// Ticks spent in each part and not in a part nested inside it.
    time: [u64; PARTS],
    /// Ticks from entering each part to leaving it, outermost scope only.
    inclusive: [u64; PARTS],
    /// Scopes entered, per part.
    count: [u64; PARTS],
    /// Scopes of each part open now.
    open: [u8; PARTS],
    /// Open scopes: the part each one interrupted, and when it was entered.
    stack: [(Part, u64); DEPTH],
    depth: usize,
    /// Scopes entered beyond [`DEPTH`] and not left yet.
    beyond: usize,
    current: Part,
    since: u64,
}

impl Default for Account {
    fn default() -> Self {
        Self::new()
    }
}

impl Account {
    /// An empty account, in [`Part::Other`] since tick 0.
    pub const fn new() -> Self {
        Self {
            time: [0; PARTS],
            inclusive: [0; PARTS],
            count: [0; PARTS],
            open: [0; PARTS],
            stack: [(Part::Other, 0); DEPTH],
            depth: 0,
            beyond: 0,
            current: Part::Other,
            since: 0,
        }
    }

    /// Clears the account: from `now` on, time goes to [`Part::Other`].
    pub fn start(&mut self, now: u64) {
        *self = Self::new();
        self.since = now;
    }

    /// Gives the ticks up to `now` to the current part.
    pub fn charge(&mut self, now: u64) {
        self.time[self.current as usize] += now.saturating_sub(self.since);
        self.since = now;
    }

    /// Enters `part` at `now`.
    pub fn enter(&mut self, part: Part, now: u64) {
        if self.depth == DEPTH {
            self.beyond += 1;
            return;
        }
        self.charge(now);
        self.stack[self.depth] = (self.current, now);
        self.depth += 1;
        self.current = part;
        self.count[part as usize] += 1;
        self.open[part as usize] += 1;
    }

    /// Leaves the innermost part at `now`; returns how long it was open
    /// (0 for a scope beyond [`DEPTH`] or when none is open).
    pub fn leave(&mut self, now: u64) -> u64 {
        if self.beyond > 0 {
            self.beyond -= 1;
            return 0;
        }
        if self.depth == 0 {
            return 0; // opened before the account was started
        }
        self.charge(now);
        self.depth -= 1;
        let (outer, at) = self.stack[self.depth];
        let part = self.current as usize;
        let open = now.saturating_sub(at);
        self.open[part] -= 1;
        if self.open[part] == 0 {
            self.inclusive[part] += open;
        }
        self.current = outer;
        open
    }

    /// Ticks spent in `part`, nested parts excluded.
    pub fn time(&self, part: Part) -> u64 {
        self.time[part as usize]
    }

    /// Ticks from entering `part` to leaving it (outermost scopes only).
    pub fn inclusive(&self, part: Part) -> u64 {
        self.inclusive[part as usize]
    }

    /// Scopes of `part` entered.
    pub fn count(&self, part: Part) -> u64 {
        self.count[part as usize]
    }

    /// Ticks accounted in all.
    pub fn total(&self) -> u64 {
        self.time.iter().sum()
    }

    /// Adds `other`'s figures to this account's (several CPUs).
    pub fn merge(&mut self, other: &Account) {
        for i in 0..PARTS {
            self.time[i] += other.time[i];
            self.inclusive[i] += other.inclusive[i];
            self.count[i] += other.count[i];
        }
    }
}

/// Call numbers with a slot of their own in [`Calls`]; higher ones share
/// the last slot.
pub const CALL_SLOTS: usize = 512;

/// Per system call number: calls made and ticks spent handling them.
#[derive(Debug, Clone)]
pub struct Calls {
    slots: [(u64, u64); CALL_SLOTS],
}

impl Default for Calls {
    fn default() -> Self {
        Self::new()
    }
}

impl Calls {
    /// No call yet.
    pub const fn new() -> Self {
        Self {
            slots: [(0, 0); CALL_SLOTS],
        }
    }

    /// Forgets every call.
    pub fn clear(&mut self) {
        self.slots.fill((0, 0));
    }

    /// Records one call `nr` that took `ticks`.
    pub fn add(&mut self, nr: u64, ticks: u64) {
        let slot = &mut self.slots[(nr as usize).min(CALL_SLOTS - 1)];
        slot.0 += 1;
        slot.1 += ticks;
    }

    /// Adds `other`'s figures to these.
    pub fn merge(&mut self, other: &Calls) {
        for (a, b) in self.slots.iter_mut().zip(&other.slots) {
            a.0 += b.0;
            a.1 += b.1;
        }
    }

    /// The `N` calls that took most time, as `(nr, calls, ticks)`, most
    /// first (fewer when fewer were made).
    pub fn top<const N: usize>(&self) -> ([(u64, u64, u64); N], usize) {
        let mut out = [(0, 0, 0); N];
        let mut len = 0;
        for (nr, &(calls, ticks)) in self.slots.iter().enumerate() {
            if calls == 0 {
                continue;
            }
            let mut i = if len < N {
                len += 1;
                len - 1
            } else if N > 0 && ticks > out[N - 1].2 {
                N - 1 // replaces the smallest kept
            } else {
                continue;
            };
            out[i] = (nr as u64, calls, ticks);
            while i > 0 && out[i - 1].2 < out[i].2 {
                out.swap(i - 1, i);
                i -= 1;
            }
        }
        (out, len)
    }
}

/// A workload's breakdown, from the merged account of every CPU, in the
/// line `xtask` reads:
///
/// `rhizo-breakdown <workload> total_ns=… user_ns=… entry_exit_ns=…
/// syscall_ns=… … other_ns=…`, then `rhizo-breakdown-counts <workload>
/// traps=… syscall=… …` (scopes entered per part).
///
/// Cell time is split into the cell's own code (`user`) and the kernel's
/// entry and exit path (`entry_exit`): `per_trap` ticks for each entry into
/// the cell, as measured on a cell doing nothing but system calls.
#[derive(Debug)]
pub struct Breakdown<'a> {
    pub workload: &'a str,
    pub account: &'a Account,
    /// Ticks of one entry and exit of cell code.
    pub per_trap: u64,
}

impl Breakdown<'_> {
    /// Ticks of the kernel's entry and exit path.
    pub fn entry_exit(&self) -> u64 {
        let cell = self.account.time(Part::Cell);
        (self.account.count(Part::Cell) * self.per_trap).min(cell)
    }

    /// Writes both lines, converting ticks with `ns`.
    pub fn write(&self, w: &mut impl Write, ns: impl Fn(u64) -> u64) -> fmt::Result {
        let a = self.account;
        let entry_exit = self.entry_exit();
        write!(
            w,
            "rhizo-breakdown {} total_ns={} user_ns={} entry_exit_ns={}",
            self.workload,
            ns(a.total()),
            ns(a.time(Part::Cell) - entry_exit),
            ns(entry_exit)
        )?;
        for part in &Part::ALL[1..] {
            write!(w, " {}_ns={}", part.name(), ns(a.time(*part)))?;
        }
        write!(
            w,
            "\nrhizo-breakdown-counts {} traps={}",
            self.workload,
            a.count(Part::Cell)
        )?;
        for part in &Part::ALL[1..] {
            write!(w, " {}={}", part.name(), a.count(*part))?;
        }
        writeln!(w)
    }
}

#[cfg(test)]
mod tests;
