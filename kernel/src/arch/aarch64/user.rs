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
    /// An interrupt (the end of its time slice): the cell can be resumed.
    Interrupt,
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

/// Enables 16-bit ASIDs when the CPU has them and returns the highest tag
/// cells may use (65535, or 255 with 8-bit ASIDs). Call once.
pub fn init_tags() -> u16 {
    let mmfr0: u64;
    // SAFETY: reading an ID register has no side effects.
    unsafe { asm!("mrs {}, id_aa64mmfr0_el1", out(reg) mmfr0, options(nomem, nostack)) };
    if (mmfr0 >> 4) & 0xf != 0b0010 {
        return 255;
    }
    // SAFETY: TCR_EL1.AS selects 16-bit ASIDs; no cell translation is cached
    // yet (only ASID 0 was ever used, and it is flushed with everything).
    unsafe {
        asm!(
            "mrs {t}, tcr_el1",
            "orr {t}, {t}, #(1 << 36)",
            "msr tcr_el1, {t}",
            "isb",
            "tlbi vmalle1",
            "dsb nsh",
            "isb",
            t = out(reg) _,
            options(nostack)
        );
    }
    u16::MAX
}

/// Loads `root` with ASID `tag` in `TTBR0_EL1`. A tagged switch needs no
/// TLB maintenance (design D6); tag 0 is shared by the empty table and
/// untagged cells, so switching to it drops what ASID 0 had cached.
fn set_user_root(root: u64, tag: u16) {
    let ttbr = root | ((tag as u64) << 48);
    if USER_ROOT.swap(ttbr, Ordering::Relaxed) == ttbr {
        return;
    }
    // SAFETY: TTBR0 maps only the lower half, which the kernel never uses;
    // cell pages are non-global, so their translations are tagged.
    unsafe {
        asm!("dsb ishst", "msr ttbr0_el1, {}", "isb", in(reg) ttbr, options(nostack));
        if tag == 0 {
            asm!("tlbi aside1, xzr", "dsb ish", "isb", options(nostack));
        }
    }
}

/// Runs the cell whose lower-half table is `root`, tagged `tag` (0 =
/// untagged), from `regs` until it makes a system call, is interrupted or
/// faults; `regs` then holds its state at that point.
pub fn enter_user(root: u64, tag: u16, regs: &mut Registers) -> Trap {
    set_user_root(root, tag);
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
    if vector == 9 {
        super::irq::acknowledge();
        return Trap::Interrupt;
    }
    Trap::Fault(UserFault(Aarch64Exception {
        esr,
        far,
        elr: regs.pc,
        stack_guard: (0, 0),
    }))
}

/// Points `TTBR0_EL1` back at the empty table and drops every translation
/// of the cell tagged `tag`: call before freeing its tables and its tag.
pub fn release_user(_root: u64, tag: u16) {
    set_user_root(EMPTY_ROOT.load(Ordering::Relaxed), 0);
    if tag != 0 {
        // SAFETY: invalidating one ASID only drops cached translations.
        unsafe {
            asm!(
                "dsb ishst",
                "tlbi aside1is, {}",
                "dsb ish",
                "isb",
                in(reg) (tag as u64) << 48,
                options(nostack)
            );
        }
    }
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
