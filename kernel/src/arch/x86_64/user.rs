//! Running cell code in ring 3: entry, return on `SYSCALL` or an exception,
//! and the CR3 switch (design decision D4). The entry code is in `cpu.rs`.

use super::cpu::{read_cr2, wrmsr};
use core::arch::asm;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use rhizo_cell::Registers;
use rhizo_core::exception::{self, ExceptionInfo};
use rhizo_hal::Console;

const FS_BASE: u32 = 0xC000_0100;

/// ELF machine of cell executables.
pub const MACHINE: rhizo_cell::elf::Machine = rhizo_cell::elf::Machine::X86_64;
/// System-call convention of cells.
pub const ABI: rhizo_abi::Arch = rhizo_abi::Arch::X86_64;
/// Whether a cell's root table must carry the kernel half: yes, CR3 holds a
/// single hierarchy for both halves.
pub const CELL_ROOT_HAS_KERNEL_HALF: bool = true;

unsafe extern "C" {
    fn rhizo_enter_cell(regs: *mut Registers, sysret: u64) -> u64;
    static rhizo_trap_error: u64;
}

/// Why a cell stopped running.
#[derive(Debug, Clone, Copy)]
pub enum Trap {
    /// `SYSCALL`: the system call is in the saved registers.
    Syscall,
    /// An interrupt (the end of its time slice): the cell can be resumed.
    Interrupt,
    /// An exception: the cell cannot continue.
    Fault(UserFault),
}

/// An exception raised by cell code.
#[derive(Debug, Clone, Copy)]
pub struct UserFault(ExceptionInfo);

impl UserFault {
    /// Prints `cell N killed: <report>`.
    pub fn report(&self, console: &mut impl Console, cell: u32) {
        exception::report_cell_exception(console, cell, &self.0);
    }
}

/// Root loaded in CR3 when a cell last ran, and whether it is at a system
/// call boundary (so `SYSRETQ` may return to it).
static USER_ROOT: AtomicU64 = AtomicU64::new(0);
static AT_SYSCALL: AtomicBool = AtomicBool::new(false);

/// Runs the cell whose root table is `root` from `regs` until it makes a
/// system call or faults; `regs` then holds its state at that point.
pub fn enter_user(root: u64, regs: &mut Registers) -> Trap {
    if USER_ROOT.swap(root, Ordering::Relaxed) != root {
        // SAFETY: a cell root shares the kernel half (`share_upper_half`), so
        // the kernel keeps running unchanged on it.
        unsafe { asm!("mov cr3, {}", in(reg) root, options(nostack, preserves_flags)) };
        AT_SYSCALL.store(false, Ordering::Relaxed);
    }
    // SYSRETQ needs a canonical lower-half RIP (else it faults in ring 0).
    let sysret = AT_SYSCALL.load(Ordering::Relaxed) && regs.pc < 0x0000_8000_0000_0000;
    // SAFETY: the FS base is the cell's thread pointer; the kernel does not
    // use FS.
    unsafe { wrmsr(FS_BASE, regs.tls) };
    // SAFETY: `regs` lives across the call; the entry code returns to ring 3
    // with user selectors and only the user flags, and comes back through
    // the SYSCALL entry or a ring-3 exception, which restore the kernel
    // stack and callee-saved registers.
    let vector = unsafe { rhizo_enter_cell(regs, sysret as u64) };
    AT_SYSCALL.store(vector == 256, Ordering::Relaxed);
    if vector == 256 {
        return Trap::Syscall;
    }
    let vector = vector as u8;
    if vector == super::irq::TIMER_VECTOR {
        super::irq::acknowledge();
        return Trap::Interrupt;
    }
    if vector == super::irq::SPURIOUS_VECTOR {
        return Trap::Interrupt;
    }
    Trap::Fault(UserFault(ExceptionInfo {
        vector,
        // SAFETY: written by the trap path just before returning.
        error_code: unsafe { core::ptr::addr_of!(rhizo_trap_error).read() },
        rip: regs.pc,
        fault_address: (vector == exception::PAGE_FAULT).then(read_cr2),
    }))
}

/// Switches CR3 back to the kernel's tables: call before freeing the tables
/// of the cell that last ran.
pub fn release_user() {
    let kernel = crate::memory::kernel_tables().root();
    USER_ROOT.store(kernel, Ordering::Relaxed);
    // SAFETY: the kernel tables map everything the kernel uses.
    unsafe { asm!("mov cr3, {}", in(reg) kernel, options(nostack, preserves_flags)) };
}

/// Nothing to do on x86_64: instruction fetch sees data writes.
pub fn sync_code(_virt: u64, _len: u64) {}
