//! Exception vectors, translation-table activation and TLB maintenance.

use super::console::BootConsole;
use super::power::Machine;
use crate::memory::DirectMap;
use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};
use rhizo_cell::Registers;
use rhizo_core::exception::{self, Aarch64Exception};
use rhizo_hal::{Console, ExitStatus, PhysicalMemory, Power, Tlb};

// Sixteen vectors of 0x80 bytes in a 2 KiB-aligned table. Exceptions taken
// to EL1 run on SP_EL1.
//
// - From the kernel (vectors 0-7, and 12-15 which never happen because cells
//   cannot run AArch32): record the index in x3 and report the exception.
// - From a cell (vectors 8-11): SP_EL1 points at the cell's `Registers`
//   (`enter_user` put it there), so the vector saves the user state in place,
//   restores the kernel stacks and returns from `rhizo_enter_cell` with the
//   vector index (design decision D3, option B).
//
// `Registers` layout: pc 0, sp 8, x0-x30 at 16 + 8n, tls 264, flags 272.
core::arch::global_asm!(
    r#"
    .macro RHIZO_VECTOR n
    .balign 0x80
    mov     x3, #\n
    b       rhizo_vector_common
    .endm

    .macro RHIZO_EL0_VECTOR n
    .balign 0x80
    stp     x0, x1, [sp, #16]
    mov     x0, #\n
    b       rhizo_el0_trap
    .endm

    .text
    .balign 2048
    .global rhizo_vectors
rhizo_vectors:
    .irp n, 0,1,2,3,4,5,6,7
    RHIZO_VECTOR \n
    .endr
    .irp n, 8,9,10,11
    RHIZO_EL0_VECTOR \n
    .endr
    .irp n, 12,13,14,15
    RHIZO_VECTOR \n
    .endr

rhizo_vector_common:
    mrs     x0, esr_el1
    mrs     x1, far_el1
    mrs     x2, elr_el1
    bl      rhizo_exception_handler
1:  wfe
    b       1b

rhizo_el0_trap:
    stp     x2, x3, [sp, #32]
    stp     x4, x5, [sp, #48]
    stp     x6, x7, [sp, #64]
    stp     x8, x9, [sp, #80]
    stp     x10, x11, [sp, #96]
    stp     x12, x13, [sp, #112]
    stp     x14, x15, [sp, #128]
    stp     x16, x17, [sp, #144]
    stp     x18, x19, [sp, #160]
    stp     x20, x21, [sp, #176]
    stp     x22, x23, [sp, #192]
    stp     x24, x25, [sp, #208]
    stp     x26, x27, [sp, #224]
    stp     x28, x29, [sp, #240]
    str     x30, [sp, #256]
    mrs     x1, elr_el1
    mrs     x2, sp_el0
    stp     x1, x2, [sp, #0]
    mrs     x1, tpidr_el0
    mrs     x2, spsr_el1
    stp     x1, x2, [sp, #264]
    ldr     x1, =rhizo_exception_stack_top
    mov     sp, x1
    msr     spsel, #0
    ldr     x1, =rhizo_kernel_sp
    ldr     x1, [x1]
    mov     sp, x1
    ldp     x19, x20, [sp, #16]
    ldp     x21, x22, [sp, #32]
    ldp     x23, x24, [sp, #48]
    ldp     x25, x26, [sp, #64]
    ldp     x27, x28, [sp, #80]
    ldp     x29, x30, [sp], #96
    ret

    // u64 rhizo_enter_cell(Registers *regs): runs the cell until it traps.
    .global rhizo_enter_cell
rhizo_enter_cell:
    stp     x29, x30, [sp, #-96]!
    stp     x19, x20, [sp, #16]
    stp     x21, x22, [sp, #32]
    stp     x23, x24, [sp, #48]
    stp     x25, x26, [sp, #64]
    stp     x27, x28, [sp, #80]
    ldr     x1, =rhizo_kernel_sp
    mov     x2, sp
    str     x2, [x1]
    msr     spsel, #1
    mov     sp, x0
    ldp     x1, x2, [sp, #0]
    msr     elr_el1, x1
    msr     sp_el0, x2
    ldp     x1, x2, [sp, #264]
    msr     tpidr_el0, x1
    and     x2, x2, #0xf0000000     // only NZCV: EL0t, AArch64, unmasked
    msr     spsr_el1, x2
    ldp     x0, x1, [sp, #16]
    ldp     x2, x3, [sp, #32]
    ldp     x4, x5, [sp, #48]
    ldp     x6, x7, [sp, #64]
    ldp     x8, x9, [sp, #80]
    ldp     x10, x11, [sp, #96]
    ldp     x12, x13, [sp, #112]
    ldp     x14, x15, [sp, #128]
    ldp     x16, x17, [sp, #144]
    ldp     x18, x19, [sp, #160]
    ldp     x20, x21, [sp, #176]
    ldp     x22, x23, [sp, #192]
    ldp     x24, x25, [sp, #208]
    ldp     x26, x27, [sp, #224]
    ldp     x28, x29, [sp, #240]
    ldr     x30, [sp, #256]
    eret

    .ltorg
    "#
);

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

/// Lower-half table loaded in `TTBR0_EL1`: the empty one, or a cell's.
static USER_ROOT: AtomicU64 = AtomicU64::new(0);
static EMPTY_ROOT: AtomicU64 = AtomicU64::new(0);

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

#[unsafe(no_mangle)]
extern "C" fn rhizo_exception_handler(esr: u64, far: u64, elr: u64, _vector: u64) -> ! {
    let guard = super::boot::layout::stack_guard();
    exception::report_aarch64(
        &mut BootConsole,
        &Aarch64Exception {
            esr,
            far,
            elr,
            stack_guard: (guard, guard + rhizo_mm::FRAME_SIZE),
        },
    );
    Machine.exit(ExitStatus::Failure)
}

/// Nothing to do: the entry code installs `VBAR_EL1` and both stacks.
pub fn init() {}

/// AArch64 with a 4 KiB granule always has 1 GiB blocks at level 1.
pub fn supports_1gib_pages() -> bool {
    true
}

/// Switches to the kernel tables rooted at `root` (`TTBR1_EL1`) and an empty
/// lower half (`TTBR0_EL1`, a fresh zeroed table from `alloc`).
///
/// # Safety
/// The new tables must map the currently executing code, the stack and all
/// kernel data at the same virtual addresses.
pub unsafe fn activate(root: u64, alloc: &mut impl FnMut() -> Option<u64>) {
    let empty = alloc().expect("a frame for the empty lower-half table");
    DirectMap.zero_frame(empty);
    EMPTY_ROOT.store(empty, Ordering::Relaxed);
    USER_ROOT.store(empty, Ordering::Relaxed);
    // SAFETY: guaranteed by the caller; the barriers make the table writes
    // visible to the walker before the switch and drop stale translations.
    unsafe {
        asm!(
            "dsb ishst",
            "msr ttbr0_el1, {low}",
            "msr ttbr1_el1, {high}",
            "isb",
            "tlbi vmalle1is",
            "dsb ish",
            "isb",
            low = in(reg) empty,
            high = in(reg) root,
            options(nostack)
        );
    }
}

/// TLB maintenance broadcast to every core (inner shareable).
#[derive(Debug)]
pub struct CpuTlb;

impl Tlb for CpuTlb {
    fn flush_page(&mut self, virt: u64) {
        // SAFETY: invalidating a translation (all ASIDs) only drops cached
        // entries; the barriers order it with the preceding table write.
        unsafe {
            asm!(
                "dsb ishst",
                "tlbi vaae1is, {page}",
                "dsb ish",
                "isb",
                page = in(reg) (virt >> 12) & 0xfff_ffff_ffff,
                options(nostack)
            );
        }
    }
}

/// Executes an undefined instruction (self-check `ktest-invalid-opcode`).
#[cfg(feature = "ktest-invalid-opcode")]
pub fn execute_invalid_instruction() {
    // SAFETY: deliberately undefined; the exception ends the run.
    unsafe { asm!("udf #0") };
}
