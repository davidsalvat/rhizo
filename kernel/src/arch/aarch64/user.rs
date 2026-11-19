//! Running cell code at EL0: entry, return on `SVC` or an exception, and
//! the lower-half table switch (design decision D3). The vectors and the
//! entry code are in `cpu.rs`.

use super::cpu::{EMPTY_ROOT, USER_ROOT};
use core::arch::asm;
use core::sync::atomic::Ordering;
use rhizo_cell::Registers;
use rhizo_core::exception::{self, Aarch64Exception};
use rhizo_hal::Console;

/// ELF machine of cell executables.
pub const MACHINE: rhizo_cell::elf::Machine = rhizo_cell::elf::Machine::Aarch64;
/// System-call convention of cells.
pub const ABI: rhizo_abi::Arch = rhizo_abi::Arch::Aarch64;
/// Whether a cell's root table must carry the kernel half: no, the kernel
/// lives in `TTBR1_EL1` and the cell's table goes in `TTBR0_EL1`.
pub const CELL_ROOT_HAS_KERNEL_HALF: bool = false;

unsafe extern "C" {
    fn rhizo_enter_cell(regs: *mut Registers) -> u64;
}

/// Why a cell stopped running.
#[derive(Debug, Clone, Copy)]
pub enum Trap {
    /// `SVC #0`: the system call is in the saved registers.
    Syscall,
    /// Any other exception: the cell cannot continue.
    Fault(UserFault),
}

/// An exception raised by cell code.
#[derive(Debug, Clone, Copy)]
pub struct UserFault(Aarch64Exception);

impl UserFault {
    /// Prints `cell N killed: <report>`.
    pub fn report(&self, console: &mut impl Console, cell: u32) {
        exception::report_cell_aarch64(console, cell, &self.0);
    }
}

fn set_user_root(root: u64) {
    if USER_ROOT.swap(root, Ordering::Relaxed) != root {
        // SAFETY: TTBR0 maps only the lower half, which the kernel never
        // uses; cell pages are non-global (ASID 0), so dropping ASID 0 removes
        // every stale cell translation and keeps the kernel's global ones.
        unsafe {
            asm!(
                "dsb ishst",
                "msr ttbr0_el1, {root}",
                "isb",
                "tlbi aside1, xzr",
                "dsb ish",
                "isb",
                root = in(reg) root,
                options(nostack)
            );
        }
    }
}

/// Runs the cell whose lower-half table is `root` from `regs` until it makes
/// a system call or faults; `regs` then holds its state at that point.
pub fn enter_user(root: u64, regs: &mut Registers) -> Trap {
    set_user_root(root);
    // SAFETY: `regs` is a valid, 16-byte aligned frame that lives across the
    // call; the entry code drops to EL0 with only the NZCV flags of the
    // saved state, and the EL0 vectors restore the kernel stacks and
    // callee-saved registers before returning here.
    let vector = unsafe { rhizo_enter_cell(regs) };
    let (esr, far): (u64, u64);
    // SAFETY: reading the syndrome registers of the exception just taken.
    unsafe {
        asm!("mrs {}, esr_el1", "mrs {}, far_el1", out(reg) esr, out(reg) far, options(nomem, nostack));
    }
    if vector == 8 && (esr >> 26) & 0x3f == 0x15 {
        return Trap::Syscall;
    }
    Trap::Fault(UserFault(Aarch64Exception {
        esr,
        far,
        elr: regs.pc,
        stack_guard: (0, 0),
    }))
}

/// Points `TTBR0_EL1` back at the empty table: call before freeing the
/// tables of the cell that last ran.
pub fn release_user() {
    set_user_root(EMPTY_ROOT.load(Ordering::Relaxed));
}

/// Makes code written through the data side (a loaded cell image at the
/// direct-map address `virt`) visible to instruction fetch.
pub fn sync_code(virt: u64, len: u64) {
    let ctr: u64;
    // SAFETY: reading the cache type register has no side effects.
    unsafe { asm!("mrs {}, ctr_el0", out(reg) ctr, options(nomem, nostack)) };
    let line = 4 << ((ctr >> 16) & 0xf); // DminLine: log2 of words
    let mut at = virt & !(line - 1);
    while at < virt + len {
        // SAFETY: cleaning a cache line of mapped memory has no other effect.
        unsafe { asm!("dc cvau, {}", in(reg) at, options(nostack)) };
        at += line;
    }
    // SAFETY: invalidating the instruction cache only drops cached lines.
    unsafe { asm!("dsb ish", "ic ialluis", "dsb ish", "isb", options(nostack)) };
}
