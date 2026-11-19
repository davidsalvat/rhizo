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

/// CR3 value (root and PCID) when a cell last ran, whether it is at a
/// system call boundary (so `SYSRETQ` may return to it), and whether PCIDs
/// are on.
static USER_CR3: AtomicU64 = AtomicU64::new(0);
static AT_SYSCALL: AtomicBool = AtomicBool::new(false);
static PCID: AtomicBool = AtomicBool::new(false);
/// FS base last written (skipping the write when it does not change).
static LAST_FS: AtomicU64 = AtomicU64::new(u64::MAX);
/// CR3 bit 63 with PCIDs on: keep the translations of the new PCID.
const NO_FLUSH: u64 = 1 << 63;

/// Enables PCIDs when the CPU has them and returns the highest tag cells may
/// use (4095), or 0 without PCIDs (every switch flushes). Call once.
pub fn init_tags() -> u16 {
    if core::arch::x86_64::__cpuid(1).ecx & (1 << 17) == 0 {
        return 0;
    }
    // SAFETY: CR4.PCIDE may be set while CR3 holds PCID 0, as it does (the
    // kernel's tables are loaded with no PCID bits).
    unsafe {
        asm!(
            "mov {t}, cr4",
            "or {t}, {bit}",
            "mov cr4, {t}",
            t = out(reg) _,
            bit = const 1u64 << 17,
            options(nostack)
        );
    }
    PCID.store(true, Ordering::Relaxed);
    4095
}

fn load_cr3(value: u64) {
    // SAFETY: every value loaded is a cell root (sharing the kernel half) or
    // the kernel's own tables, so the kernel keeps running unchanged.
    unsafe { asm!("mov cr3, {}", in(reg) value, options(nostack, preserves_flags)) };
}

/// Runs the cell whose root table is `root`, tagged `tag` (PCID; 0 =
/// untagged), from `regs` until it makes a system call, is interrupted or
/// faults; `regs` then holds its state at that point.
pub fn enter_user(root: u64, tag: u16, regs: &mut Registers) -> Trap {
    let cr3 = root | tag as u64;
    if USER_CR3.swap(cr3, Ordering::Relaxed) != cr3 {
        let keep = PCID.load(Ordering::Relaxed) && tag != 0;
        load_cr3(if keep { cr3 | NO_FLUSH } else { cr3 });
        AT_SYSCALL.store(false, Ordering::Relaxed);
    }
    // SYSRETQ needs a canonical lower-half RIP (else it faults in ring 0).
    let sysret = AT_SYSCALL.load(Ordering::Relaxed) && regs.pc < 0x0000_8000_0000_0000;
    if LAST_FS.swap(regs.tls, Ordering::Relaxed) != regs.tls {
        // SAFETY: the FS base is the cell's thread pointer; the kernel does
        // not use FS.
        unsafe { wrmsr(FS_BASE, regs.tls) };
    }
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

/// Switches CR3 back to the kernel's tables and drops every translation of
/// the cell (`root`, `tag`): call before freeing its tables and its tag.
pub fn release_user(root: u64, tag: u16) {
    let pcid = PCID.load(Ordering::Relaxed);
    if pcid && tag != 0 {
        load_cr3(root | tag as u64); // without bit 63: flushes this PCID
    }
    let kernel = crate::memory::kernel_tables().root();
    USER_CR3.store(kernel, Ordering::Relaxed);
    load_cr3(if pcid { kernel | NO_FLUSH } else { kernel });
}

/// Nothing to do on x86_64: instruction fetch sees data writes.
pub fn sync_code(_virt: u64, _len: u64) {}
