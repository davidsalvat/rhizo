//! Exception vectors, translation-table activation and TLB maintenance.

use super::console::BootConsole;
use super::power::Machine;
use crate::memory::DirectMap;
use core::arch::asm;
use rhizo_core::exception::{self, Aarch64Exception};
use rhizo_hal::{ExitStatus, PhysicalMemory, Power, Tlb};

// Sixteen vectors of 0x80 bytes in a 2 KiB-aligned table. Each records its
// index in x3 and joins a common path that reports the exception. Exceptions
// taken to EL1 run on SP_EL1, the dedicated exception stack.
core::arch::global_asm!(
    r#"
    .macro RHIZO_VECTOR n
    .balign 0x80
    mov     x3, #\n
    b       rhizo_vector_common
    .endm

    .text
    .balign 2048
    .global rhizo_vectors
rhizo_vectors:
    .irp n, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15
    RHIZO_VECTOR \n
    .endr

rhizo_vector_common:
    mrs     x0, esr_el1
    mrs     x1, far_el1
    mrs     x2, elr_el1
    bl      rhizo_exception_handler
1:  wfe
    b       1b
    "#
);

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
