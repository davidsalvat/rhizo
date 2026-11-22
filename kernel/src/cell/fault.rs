//! Page faults of cell code: copy-on-write, lazy resume, the stack,
//! and pages that are written or executed in turn (W^X).

use super::*;

/// The TLB as a cell sees it: [`CpuTlb`], noting that the cell's
/// translations shrank, and keeping the other CPUs that run the cell's
/// code out of it until the change is complete (design D7 of
/// `add-parallel-threads`).
pub(super) struct CellTlb<'a> {
    pub(super) shrank: bool,
    /// The gang this CPU runs, and the CPU's number.
    gang: Option<(&'a Gang, usize)>,
}

impl<'a> CellTlb<'a> {
    pub(super) fn new(gang: Option<(&'a Gang, usize)>) -> Self {
        Self {
            shrank: false,
            gang,
        }
    }
}

impl Tlb for CellTlb<'_> {
    fn flush_page(&mut self, virt: u64) {
        if let Some((gang, cpu)) = self.gang {
            gang.hold_others(cpu);
        }
        CpuTlb.flush_page(virt);
        self.shrank = true;
    }
}

/// The pager of system calls: a lazily resumed cell's pending pages, and
/// copy-on-write pages shared after a fork (resolved through the TLB given).
pub(super) struct CellPager<'a, 'g>(
    pub(super) &'a mut OnDemand,
    pub(super) &'a mut CellTlb<'g>,
    pub(super) &'a Meter,
);

impl Pager<Format, DirectMap> for CellPager<'_, '_> {
    fn page_in(
        &mut self,
        pt: &PageTable<Format>,
        _: &mut DirectMap,
        addr: u64,
        write: bool,
    ) -> bool {
        if self
            .0
            .lazy
            .as_mut()
            .is_some_and(|lazy| lazy.page_in(pt, &mut cell_frames(self.2), addr))
        {
            return true;
        }
        (write && resolve_write(pt, self.1, addr))
            || grow_stack(pt, &mut cell_frames(self.2), addr)
            || wx_page_in(&self.0.wx, self.1, self.2, pt, addr, write)
    }

    fn discard(&mut self, addr: u64, len: u64) {
        if let Some(lazy) = self.0.lazy.as_mut() {
            lazy.discard(addr, len);
        }
    }

    fn has_pending(&self) -> bool {
        self.0.lazy.is_some()
    }

    fn wx(&mut self) -> Option<&mut WxRanges> {
        Some(&mut self.0.wx)
    }
}

/// A system call's access to a page of write-or-execute memory (design D5
/// of `node-jit`): a write to a running page makes it writing, and a page
/// of the ranges not mapped yet is backed. Out of line and cold: a cell
/// without such memory never comes here but on a real fault.
#[cold]
#[inline(never)]
fn wx_page_in(
    wx: &WxRanges,
    tlb: &mut CellTlb<'_>,
    meter: &Meter,
    pt: &PageTable<Format>,
    addr: u64,
    write: bool,
) -> bool {
    if write && resolve_write_wx(pt, tlb, addr) {
        return true;
    }
    let (mem, frames) = (&mut DirectMap, &mut cell_frames(meter));
    may_take(meter, FRAME_SIZE) && space::back_wx(pt, mem, &mut CpuTlb, frames, wx, addr)
}

/// The instruction at `pc` in the cell with tables `pt`, if it is mapped
/// (for the faults the kernel answers by emulating the instruction).
pub(super) fn instruction(pt: &PageTable<Format>, pc: u64) -> Option<u32> {
    let mut word = [0; 4];
    rhizo_cell::memory::copy_from(pt, &DirectMap, pc, &mut word).ok()?;
    Some(u32::from_le_bytes(word))
}

/// Grows the stack of the cell with tables `pt` to cover `addr` (design D5
/// of `add-cpython`), if memory allows.
pub(super) fn grow_stack(pt: &PageTable<Format>, frames: &mut impl Frames, addr: u64) -> bool {
    let _p = cost::scope(Part::Fault);
    let room = frames
        .metered()
        .is_some_and(|(used, limit)| limit < Quota::NONE.frames() && used < limit);
    (room || cells_may_take(FRAME_SIZE))
        && space::grow_stack(pt, &mut DirectMap, &mut CpuTlb, frames, addr)
}

/// Resolves a write refused because the page is shared (copy-on-write).
///
/// Not metered (design D1 of `add-cell-quotas`): a copy replaces the cell's
/// reference to a shared frame or table by its own, so the meter would
/// charge one frame and credit one back.
pub(super) fn resolve_write(pt: &PageTable<Format>, tlb: &mut CellTlb<'_>, addr: u64) -> bool {
    let _p = cost::scope(Part::Cow);
    let resolved = pt.resolve_write(&mut DirectMap, &mut KernelFrames, tlb, addr) == Ok(true);
    if resolved {
        tlb.flush_page(addr);
    }
    resolved
}

/// Makes the running write-or-execute page holding `addr` writing for a
/// write (design D3 of `node-jit`), copying a frame still shared, its old
/// translation flushed through `tlb`. Not metered, as [`resolve_write`].
#[cold]
#[inline(never)]
pub(super) fn resolve_write_wx(pt: &PageTable<Format>, tlb: &mut CellTlb<'_>, addr: u64) -> bool {
    let _p = cost::scope(Part::Fault);
    let resolved = pt.resolve_write_wx(&mut DirectMap, &mut KernelFrames, tlb, addr) == Ok(true);
    if resolved {
        tlb.flush_page(addr);
    }
    resolved
}

/// Backs the page of `live`'s write-or-execute ranges holding `addr` with
/// a zeroed page, writing, if it is not mapped and memory and the cell's
/// quota allow (design D5 of `node-jit`).
pub(super) fn back_wx(live: &Live, pt: &PageTable<Format>, addr: u64) -> bool {
    let _p = cost::scope(Part::Fault);
    let (mem, frames) = (&mut DirectMap, &mut cell_frames(&live.frames));
    let wx = &live.demand.wx;
    wx.contains(addr)
        && may_take(&live.frames, FRAME_SIZE)
        && sharing(live.shares, || {
            space::back_wx(pt, mem, &mut CpuTlb, frames, wx, addr)
        })
}

/// A fault of `live`'s code on `cpu`, run alone, resolved for its
/// write-or-execute memory (design D3 and D5 of `node-jit`): a page of a
/// range backed, a writing page made running, or a running page made
/// writing (unless the instruction writes the page it runs from). Only
/// faults no other path resolved come here: the cell loop is unchanged.
#[cold]
#[inline(never)]
pub(super) fn wx_fault(
    live: &mut Live,
    pt: &PageTable<Format>,
    cpu: usize,
    fault: &UserFault,
) -> bool {
    if fault
        .missing_page()
        .is_some_and(|addr| back_wx(live, pt, addr))
    {
        return true;
    }
    let pc = live.cell.regs().pc;
    let resolved = fault.exec_denied().is_some_and(|addr| {
        sharing(live.shares, || {
            resolve_exec(pt, &mut CellTlb::new(None), addr)
        })
    }) || fault.write_denied().is_some_and(|addr| {
        !writes_own_code(pt, pc, addr)
            && sharing(live.shares, || {
                resolve_write_wx(pt, &mut CellTlb::new(None), addr)
            })
    });
    if resolved {
        live.clean = flushed_by_kernel_on(cpu);
    }
    resolved
}

/// Makes the write-or-execute page holding `addr`, writing, running for an
/// instruction fetch (design D3 of `node-jit`): read-only and executable,
/// its old translation flushed through `tlb` from every CPU that may hold
/// it, then (on AArch64) its contents made visible to instruction fetches.
/// Returns `false` for any other page.
pub(super) fn resolve_exec(pt: &PageTable<Format>, tlb: &mut CellTlb<'_>, addr: u64) -> bool {
    let _p = cost::scope(Part::Fault);
    if pt.resolve_exec(&mut DirectMap, &mut KernelFrames, addr) != Ok(true) {
        return false;
    }
    tlb.flush_page(addr);
    if let Some(t) = pt.translate(&DirectMap, addr) {
        let size = t.size.bytes();
        user::sync_code(DirectMap::virt(t.phys & !(size - 1)), size);
    }
    prof::add(Count::WxFlips, 1);
    true
}

/// Whether a write fault at `addr` from the instruction at `pc` writes the
/// running write-or-execute page it runs from: no change of state lets it
/// complete, so it is a fault (spec `linux-abi`, design D3 of `node-jit`).
pub(super) fn writes_own_code(pt: &PageTable<Format>, pc: u64, addr: u64) -> bool {
    pc >> 12 == addr >> 12
        && pt
            .translate(&DirectMap, addr)
            .is_some_and(|t| t.flags.contains(PageFlags::WRITE_OR_EXEC) && t.flags.is_executable())
}
