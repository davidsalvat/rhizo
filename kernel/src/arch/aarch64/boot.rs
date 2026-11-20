//! AArch64 entry: Linux arm64 Image header, EL2 → EL1, stage-1 page tables,
//! MMU on, jump to the higher half (design decisions D2 and D4).
//!
//! The VMM enters `_start` at the physical load address with `x0` holding the
//! physical address of the device tree, the MMU and data cache off, at EL2 or
//! EL1. Until the MMU is on, only PC-relative addressing is used.
//!
//! Stage-1 tables (all in `.bss`, zeroed first):
//! - `TTBR0`: identity map of physical 0–4 GiB (1 GiB normal-memory blocks),
//!   needed only while switching on the MMU.
//! - `TTBR1`: direct map of 0–4 GiB at `0xffff_8000_0000_0000` (normal memory),
//!   a device-memory window of 0–4 GiB at [`super::MMIO_BASE`] (for the UART
//!   before the kernel builds its own tables), and the image at
//!   `0xffff_ffff_8000_0000` (2 MiB blocks starting at the load address).

use core::sync::atomic::{AtomicU64, Ordering};

/// Size of the kernel stack (the guard page below it is not included).
pub const KERNEL_STACK_SIZE: usize = 64 * 1024;
/// Size of the exception stack (`SP_EL1`).
pub const EXCEPTION_STACK_SIZE: usize = 16 * 1024;

core::arch::global_asm!(
    r#"
    .section .text.head, "ax"
    .global _start
_start:
    b       1f                      // code0: branch over the header
    .long   0                       // code1
    .quad   0                       // text_offset
    .quad   __image_size            // image_size (includes .bss)
    .quad   0xa                     // flags: little endian, 4 KiB pages, anywhere
    .quad   0, 0, 0                 // reserved
    .ascii  "ARM\x64"               // magic
    .long   0                       // PE header offset (none)

1:  mov     x19, x0                 // device tree (physical)
    isb
    mrs     x20, cntvct_el0         // entry timestamp

    // Leave EL2 for EL1 if the VMM entered at EL2.
    mrs     x0, CurrentEL
    lsr     x0, x0, #2
    cmp     x0, #2
    b.ne    2f
    mov     x0, #(1 << 31)          // HCR_EL2.RW: EL1 runs AArch64
    msr     hcr_el2, x0
    mov     x0, #3                  // CNTHCTL_EL2: EL1 may use the counters
    msr     cnthctl_el2, x0
    msr     cntvoff_el2, xzr
    mov     x0, #0x3c5              // SPSR: EL1h, interrupts masked
    msr     spsr_el2, x0
    adr     x0, 2f
    msr     elr_el2, x0
    eret
2:  mrs     x0, CurrentEL
    lsr     x0, x0, #2
    cmp     x0, #1
    b.ne    9f                      // not EL1: unsupported, stop
    msr     daifset, #0xf

    // The image must be 2 MiB aligned (boot protocol, and the 2 MiB blocks).
    adrp    x21, __image_start
    add     x21, x21, :lo12:__image_start
    tst     x21, #0x1fffff
    b.ne    9f

    // Zero .bss (physical addresses, PC-relative).
    adrp    x0, __bss_start
    add     x0, x0, :lo12:__bss_start
    adrp    x1, __bss_end
    add     x1, x1, :lo12:__bss_end
3:  cmp     x0, x1
    b.hs    4f
    stp     xzr, xzr, [x0], #16
    b       3b
4:
    // l1_ram[i] = i GiB, normal memory block (AF | inner shareable | attr 0).
    adrp    x0, rhizo_l1_ram
    add     x0, x0, :lo12:rhizo_l1_ram
    mov     x2, #0
    mov     x3, #0x701
5:  lsl     x4, x2, #30
    orr     x4, x4, x3
    str     x4, [x0, x2, lsl #3]
    add     x2, x2, #1
    cmp     x2, #4
    b.lo    5b

    // l1_mmio[i] = i GiB, device block (AF | attr 1 | PXN | UXN).
    adrp    x0, rhizo_l1_mmio
    add     x0, x0, :lo12:rhizo_l1_mmio
    mov     x2, #0
    ldr     x3, =0x0060000000000405
6:  lsl     x4, x2, #30
    orr     x4, x4, x3
    str     x4, [x0, x2, lsl #3]
    add     x2, x2, #1
    cmp     x2, #4
    b.lo    6b

    // l2_kernel[j] = load address + j * 2 MiB, normal block, EL0 execute-never.
    adrp    x0, rhizo_l2_kernel
    add     x0, x0, :lo12:rhizo_l2_kernel
    mov     x2, #0
    ldr     x3, =0x0040000000000701
7:  lsl     x4, x2, #21
    add     x4, x4, x21
    orr     x4, x4, x3
    str     x4, [x0, x2, lsl #3]
    add     x2, x2, #1
    cmp     x2, #16
    b.lo    7b

    // TTBR0 root: [0] -> l1_ram (identity).
    adrp    x0, rhizo_l0_low
    add     x0, x0, :lo12:rhizo_l0_low
    adrp    x1, rhizo_l1_ram
    add     x1, x1, :lo12:rhizo_l1_ram
    orr     x1, x1, #3
    str     x1, [x0]

    // TTBR1 root: [256] -> l1_ram (direct map), [384] -> l1_mmio,
    // [511] -> l1_kernel, whose [510] -> l2_kernel.
    adrp    x0, rhizo_l0_high
    add     x0, x0, :lo12:rhizo_l0_high
    str     x1, [x0, #(256 * 8)]
    adrp    x1, rhizo_l1_mmio
    add     x1, x1, :lo12:rhizo_l1_mmio
    orr     x1, x1, #3
    str     x1, [x0, #(384 * 8)]
    adrp    x1, rhizo_l1_kernel
    add     x1, x1, :lo12:rhizo_l1_kernel
    orr     x2, x1, #3
    str     x2, [x0, #(511 * 8)]
    adrp    x2, rhizo_l2_kernel
    add     x2, x2, :lo12:rhizo_l2_kernel
    orr     x2, x2, #3
    str     x2, [x1, #(510 * 8)]

    // Everything above was written with the MMU and caches off. Invalidate
    // .bss (tables, stacks, data) to the point of coherency so no stale
    // cache line hides it once caches are on. Line size from CTR_EL0.DminLine.
    mrs     x3, ctr_el0
    ubfx    x3, x3, #16, #4
    mov     x4, #4
    lsl     x4, x4, x3
    adrp    x0, __bss_start
    add     x0, x0, :lo12:__bss_start
    adrp    x1, __bss_end
    add     x1, x1, :lo12:__bss_end
10: dc      ivac, x0
    add     x0, x0, x4
    cmp     x0, x1
    b.lo    10b
    dsb     sy

    // MAIR: attr 0 normal write-back, attr 1 device nGnRE.
    ldr     x0, =0x04ff
    msr     mair_el1, x0
    // TCR: 48-bit VA both halves, 4 KiB granules, write-back inner shareable
    // walks; IPS from the CPU's physical address range.
    ldr     x0, =0xb5103510
    mrs     x1, id_aa64mmfr0_el1
    and     x1, x1, #7
    bfi     x0, x1, #32, #3
    msr     tcr_el1, x0
    adrp    x0, rhizo_l0_low
    add     x0, x0, :lo12:rhizo_l0_low
    msr     ttbr0_el1, x0
    adrp    x0, rhizo_l0_high
    add     x0, x0, :lo12:rhizo_l0_high
    msr     ttbr1_el1, x0
    isb
    tlbi    vmalle1
    dsb     nsh
    isb
    // SCTLR: MMU, data and instruction caches on; no alignment checks.
    mrs     x0, sctlr_el1
    mov     x1, #((1 << 0) | (1 << 2) | (1 << 12))
    orr     x0, x0, x1
    bic     x0, x0, #(1 << 1)
    msr     sctlr_el1, x0
    isb

    ldr     x0, =rhizo_high_entry
    br      x0

9:  wfe
    b       9b

    .ltorg

    .text
rhizo_high_entry:
    // Exception stack on SP_EL1; the kernel itself runs on SP_EL0, so a
    // kernel stack overflow can still be reported (design decision D5).
    msr     spsel, #1
    ldr     x0, =rhizo_exception_stack_top
    mov     sp, x0
    msr     spsel, #0
    ldr     x1, =rhizo_percpu       // CPU 0's per-CPU data (index 0)
    str     x0, [x1, #16]
    msr     tpidr_el1, x1
    ldr     x0, =rhizo_kernel_stack_top
    mov     sp, x0
    ldr     x0, =rhizo_vectors
    msr     vbar_el1, x0
    isb
    ldr     x0, =rhizo_image_phys
    str     x21, [x0]
    mov     x0, x19
    mov     x1, x20
    bl      kernel_main
8:  wfe
    b       8b

    .ltorg

    .section .bss.boot_tables, "aw", @nobits
    .balign 4096
    .global rhizo_l0_low
rhizo_l0_low:     .skip 4096
rhizo_l0_high:    .skip 4096
rhizo_l1_ram:     .skip 4096
rhizo_l1_mmio:    .skip 4096
rhizo_l1_kernel:  .skip 4096
rhizo_l2_kernel:  .skip 4096

    .section .bss.kstack, "aw", @nobits
    .balign 4096
    .global rhizo_kernel_stack_guard
rhizo_kernel_stack_guard:
    .skip 4096
    .skip {kernel_stack}
    .global rhizo_kernel_stack_top
rhizo_kernel_stack_top:
    .balign 16
    .skip {exception_stack}
    .global rhizo_exception_stack_top
rhizo_exception_stack_top:

    .section .bss.boot_data, "aw", @nobits
    .balign 8
    .global rhizo_image_phys
rhizo_image_phys:
    .skip 8
    .text
    "#,
    kernel_stack = const KERNEL_STACK_SIZE,
    exception_stack = const EXCEPTION_STACK_SIZE,
);

/// Boundaries of the kernel image, from the linker script.
pub mod layout {
    unsafe extern "C" {
        static __image_start: u8;
        static __image_end: u8;
        static __text_start: u8;
        static __text_end: u8;
        static __rodata_start: u8;
        static __rodata_end: u8;
        static __data_start: u8;
        static __bss_end: u8;
        static rhizo_kernel_stack_guard: u8;
        static rhizo_image_phys: u64;
    }

    macro_rules! symbol_address {
        ($name:ident) => {
            // Only the address of the linker symbol is taken; nothing is read.
            core::ptr::addr_of!($name) as u64
        };
    }

    /// Physical address the image was loaded at.
    fn image_phys() -> u64 {
        // SAFETY: written once by the entry code before any Rust code runs.
        unsafe { core::ptr::addr_of!(rhizo_image_phys).read() }
    }

    /// Physical address of a kernel-image virtual address.
    pub fn virt_to_phys(virt: u64) -> u64 {
        virt - symbol_address!(__image_start) + image_phys()
    }

    /// Physical range `[start, end)` occupied by the whole image (with .bss).
    pub fn kernel_phys() -> (u64, u64) {
        let size = symbol_address!(__image_end) - symbol_address!(__image_start);
        (image_phys(), image_phys() + size)
    }

    /// Virtual range of kernel code.
    pub fn text() -> (u64, u64) {
        (symbol_address!(__text_start), symbol_address!(__text_end))
    }

    /// Virtual range of read-only data.
    pub fn rodata() -> (u64, u64) {
        (
            symbol_address!(__rodata_start),
            symbol_address!(__rodata_end),
        )
    }

    /// Virtual range of writable data and `.bss` (includes the stacks).
    pub fn data() -> (u64, u64) {
        (symbol_address!(__data_start), symbol_address!(__bss_end))
    }

    /// Virtual address of the guard page below the kernel stack.
    pub fn stack_guard() -> u64 {
        symbol_address!(rhizo_kernel_stack_guard)
    }
}

static ENTRY_CYCLES: AtomicU64 = AtomicU64::new(0);
static BOOT_ARG: AtomicU64 = AtomicU64::new(0);

/// Records the values the entry code passed to `kernel_main`.
pub fn record(boot_arg: u64, entry_cycles: u64) {
    BOOT_ARG.store(boot_arg, Ordering::Relaxed);
    ENTRY_CYCLES.store(entry_cycles, Ordering::Relaxed);
}

#[cfg_attr(
    not(feature = "ktest"),
    allow(dead_code, reason = "used by in-kernel tests only")
)]
/// The counter value read at the very first instruction of the kernel.
pub fn entry_cycles() -> u64 {
    ENTRY_CYCLES.load(Ordering::Relaxed)
}

/// What the boot protocol passed: the physical address of the device tree.
pub fn boot_arg() -> u64 {
    BOOT_ARG.load(Ordering::Relaxed)
}
