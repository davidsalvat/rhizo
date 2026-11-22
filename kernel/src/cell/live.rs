//! A cell while it lives in a host ([`Live`]), and the pages a lazily
//! resumed cell has yet to bring in.

use super::*;

/// A live cell and what the kernel keeps for it.
#[derive(Debug)]
pub(super) struct Live {
    pub(super) cell: Cell,
    /// Address-space tag (ASID/PCID), 0 if none was free.
    pub(super) tag: u16,
    /// CPUs whose TLB holds nothing stale for this cell (bit `n` for CPU
    /// `n`): none for a cell new to its tag, which drops the tag's old
    /// translations on its first entry on each CPU, and only the CPU that
    /// flushed after the cell's translations shrank, where flushes are not
    /// broadcast (design D5 of `add-smp-scheduling`).
    pub(super) clean: u32,
    /// CPUs it ran on (bit `n` for CPU `n`).
    pub(super) cpus: u32,
    /// Its memory may be shared with another cell, copy-on-write: it was
    /// forked, or is a fork.
    pub(super) shares: bool,
    pub(super) abi: Abi,
    pub(super) output: Option<Vec<u8>>,
    pub(super) echo: bool,
    /// Its generator: none until it first asks for random bytes, never in a
    /// snapshot, and never shared with a fork (design D2 of
    /// `add-crypto-random`).
    pub(super) random: Option<Generator>,
    /// Pages it gets on demand: still to restore, and of its
    /// write-or-execute ranges.
    pub(super) demand: OnDemand,
    /// Frames counted against its memory quota.
    pub(super) frames: Meter,
    /// The counter at the start of the running slice, negated, then the
    /// slice's ticks until they are counted into `cell.cpu_ns` at its end
    /// (only for a cell with a CPU quota; design D2 of `add-cell-quotas`).
    pub(super) cpu_ticks: u64,
    /// Its files and open descriptors (spec `files`).
    pub(super) fs: Fs,
    /// Deadlines of its threads' calls waiting on descriptors, kept for
    /// when they make them again (design D3 of `add-sockets-and-poll`).
    pub(super) deadlines: Vec<(u32, u64)>,
    /// A socket it waited on changed since its last system call, for the
    /// workloads' count of wake-ups that found nothing ([`prof`]).
    pub(super) io_woken: bool,
    /// [`Host::wake`] came while no thread waited with a deadline: the next
    /// wait with one ends at once.
    pub(super) host_wake: bool,
    /// Since when it can run, for the workloads' latencies ([`prof`]).
    pub(super) ready: Ready,
    /// The thread chosen last while it ran on several CPUs: turns go on
    /// from there the next time (design D5 of `add-parallel-threads`).
    pub(super) last: u32,
    /// Most CPUs that ran its threads at the same time (design D5 of
    /// `add-parallel-threads`).
    pub(super) peak: u32,
    /// The host waits for it to pause ([`Host::run_to_pause`]): a call
    /// that blocks ends its slice, even when a thread could go on at once.
    pub(super) hold: bool,
}

impl Live {
    /// Counts the CPU ticks not yet counted into the cell's CPU time;
    /// returns whether it is still within its quota.
    pub(super) fn count_cpu(&mut self) -> bool {
        let ticks = core::mem::take(&mut self.cpu_ticks);
        self.cell
            .charge_cpu(timer::cycles_to_ns(ticks).unwrap_or(0))
    }

    /// The deadline thread `tid`'s call keeps, if it waited with one.
    pub(super) fn take_deadline(&mut self, tid: u32) -> Option<u64> {
        let i = self.deadlines.iter().position(|&(t, _)| t == tid)?;
        Some(self.deadlines.swap_remove(i).1)
    }

    /// Ends its threads' sleeps and times out their timed waits, waits on
    /// descriptors included: their calls, made again, find their deadline
    /// passed (design D1 of `fast-node-template`). Those deadlines are kept
    /// here, not in the cell, so a template's are gone: each of its threads
    /// waiting on descriptors with one gets a passed one.
    pub(super) fn expire_timers(&mut self) {
        for kept in &mut self.deadlines {
            kept.1 = 0;
        }
        for t in self.cell.threads.iter() {
            if let Wait::Io { until: Some(_) } = t.wait
                && !self.deadlines.iter().any(|&(tid, _)| tid == t.tid)
            {
                self.deadlines.push((t.tid, 0));
            }
        }
        self.cell.threads.expire_timers();
    }
}

impl Keyed for Box<Live> {
    fn key(&self) -> u32 {
        self.cell.id
    }
}

/// The pages a live cell gets on demand: those of a lazy resume still only
/// in the snapshot, and those of its write-or-execute ranges (design D5 of
/// `node-jit`), together so the pager of system calls reaches both through
/// one pointer.
#[derive(Debug, Default)]
pub(super) struct OnDemand {
    pub(super) lazy: Option<Lazy>,
    pub(super) wx: WxRanges,
}

/// The snapshot of a lazily resumed cell and which of its pages are still
/// only in the snapshot.
#[derive(Debug)]
pub(super) struct Lazy {
    snapshot: Snapshot,
    pending: Vec<bool>,
    pub(super) left: usize,
}

impl Lazy {
    pub(super) fn new(snapshot: Snapshot) -> Self {
        let pages = snapshot.pages();
        Self {
            snapshot,
            pending: alloc::vec![true; pages],
            left: pages,
        }
    }

    /// The snapshot it restores from.
    pub(super) fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    /// The snapshot it restores from, to change where none of its pages
    /// is restored yet.
    pub(super) fn snapshot_mut(&mut self) -> &mut Snapshot {
        &mut self.snapshot
    }

    /// The snapshot back, nothing restored.
    pub(super) fn into_snapshot(self) -> Snapshot {
        self.snapshot
    }

    /// Restores the page containing `addr` if it is still pending.
    pub(super) fn page_in(
        &mut self,
        pt: &PageTable<Format>,
        frames: &mut impl Frames,
        addr: u64,
    ) -> bool {
        let Some(i) = self.snapshot.find(addr).filter(|&i| self.pending[i]) else {
            return false;
        };
        if self
            .snapshot
            .restore_page(i, pt, &mut DirectMap, frames)
            .is_err()
        {
            return false;
        }
        let page = self.snapshot.page(i);
        // A 2 MiB page may have come back as 4 KiB pages: each its own.
        let mut at = page.virt;
        while page.flags.is_executable()
            && at < page.virt + page.size.bytes()
            && let Some(t) = pt.translate(&DirectMap, at)
        {
            user::sync_code(DirectMap::virt(t.phys), t.size.bytes());
            at += t.size.bytes();
        }
        self.pending[i] = false;
        self.left -= 1;
        true
    }

    /// Bytes of the pages still pending that need frames of their own (not
    /// those it shares).
    pub(super) fn pending_bytes(&self) -> u64 {
        (0..self.pending.len())
            .filter(|&i| self.pending[i])
            .map(|i| self.snapshot.page(i))
            .filter(|p| matches!(p.data, Data::Bytes(_)))
            .map(|p| p.size.bytes())
            .sum()
    }

    /// Drops pending pages inside `addr..addr + len` (the cell unmapped them).
    pub(super) fn discard(&mut self, addr: u64, len: u64) {
        for i in 0..self.pending.len() {
            let page = self.snapshot.page(i);
            if self.pending[i] && page.virt >= addr && page.virt + page.size.bytes() <= addr + len {
                self.pending[i] = false;
                self.left -= 1;
            }
        }
    }

    /// Restores every page still pending, counting them in `meter` (beyond
    /// the quota if need be: they were the cell's already).
    pub(super) fn finish(&mut self, pt: &PageTable<Format>, meter: &Meter) {
        let limit = meter.limit();
        meter.set_limit(u64::MAX);
        for i in 0..self.pending.len() {
            if self.pending[i] {
                let virt = self.snapshot.page(i).virt;
                self.page_in(pt, &mut cell_frames(meter), virt);
            }
        }
        meter.set_limit(limit);
    }
}
