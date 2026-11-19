//! PVH entry point: from 32-bit protected mode to Rust in the higher half.
//!
//! The hypervisor enters `pvh_start32` with 32-bit protected mode, flat
//! segments, paging disabled, interrupts off, and `ebx` holding the physical
//! address of `hvm_start_info`.
//!
//! The trampoline (stage 1 of design decision D2):
//! 1. checks the `hvm_start_info` magic and halts forever if it is wrong;
//! 2. records the TSC as early as possible;
//! 3. zeroes `.boot.bss` and the kernel `.bss` (by physical address);
//! 4. builds stage-1 page tables mapping physical 0–4 GiB as identity and as
//!    the direct map, and the first 1 GiB at the kernel base, with 2 MiB pages;
//! 5. enables PAE, long mode, no-execute (`EFER.NXE`), paging and
//!    supervisor write protection (`CR0.WP`), loads a 64-bit GDT;
//! 6. switches to the higher-half kernel stack and calls
//!    `kernel_main(start_info_phys, entry_tsc)`.

/// Magic value at offset 0 of `hvm_start_info`.
pub const PVH_START_INFO_MAGIC: u32 = rhizo_mm::memmap::PVH_MAGIC;

/// Size of the kernel stack (the guard page below it is not included).
pub const KERNEL_STACK_SIZE: usize = 64 * 1024;

core::arch::global_asm!(
    r#"
    /* ---- PVH ELF note: physical address of the 32-bit entry ---- */
    .section .note.Xen, "a", @note
    .balign 4
    .long 4                     /* name size: "Xen\0" */
    .long 4                     /* descriptor size */
    .long 18                    /* XEN_ELFNOTE_PHYS32_ENTRY */
    .asciz "Xen"
    .balign 4
    .long pvh_start32
    .balign 4

    /* ---- 32-bit entry (linked and run at its physical address) ---- */
    .section .text.boot, "ax"
    .code32
    .global pvh_start32
pvh_start32:
    cli
    cld

    /* 1. Validate the boot protocol. */
    cmpl ${magic}, (%ebx)
    jne 9f

    /* 2. Early timestamp, kept in esi:ebp until .bss is cleared. */
    rdtsc
    movl %eax, %esi
    movl %edx, %ebp

    /* 3. Zero .boot.bss and the kernel .bss (physical addresses). */
    xorl %eax, %eax
    movl $__boot_bss_start, %edi
    movl $__boot_bss_end, %ecx
    subl %edi, %ecx
    rep stosb
    movl $__bss_phys_start, %edi
    movl $__bss_phys_end, %ecx
    subl %edi, %ecx
    rep stosb

    movl %esi, rhizo_boot_tsc
    movl %ebp, rhizo_boot_tsc + 4
    movl %ebx, rhizo_boot_start_info

    /* 4. Stage-1 tables. PD0..PD3 map 0-4 GiB with 2 MiB pages. */
    xorl %ecx, %ecx
2:
    movl %ecx, %eax
    shll $21, %eax
    orl $0x83, %eax             /* present | writable | huge */
    movl %eax, rhizo_boot_pd(,%ecx,8)
    incl %ecx
    cmpl $2048, %ecx
    jne 2b

    /* PDPT_DM[i] -> PD_i (i = 0..3): physical 0-4 GiB. */
    movl $rhizo_boot_pd, %eax
    orl $0x3, %eax
    xorl %ecx, %ecx
3:
    movl %eax, rhizo_boot_pdpt_dm(,%ecx,8)
    addl $4096, %eax
    incl %ecx
    cmpl $4, %ecx
    jne 3b

    /* PDPT_K[510] -> PD_0: kernel base maps physical 0-1 GiB. */
    movl $rhizo_boot_pd, %eax
    orl $0x3, %eax
    movl %eax, rhizo_boot_pdpt_k + 510 * 8

    /* PML4[0] (identity) and PML4[256] (direct map) -> PDPT_DM,
       PML4[511] (kernel) -> PDPT_K. */
    movl $rhizo_boot_pdpt_dm, %eax
    orl $0x3, %eax
    movl %eax, rhizo_boot_pml4
    movl %eax, rhizo_boot_pml4 + 256 * 8
    movl $rhizo_boot_pdpt_k, %eax
    orl $0x3, %eax
    movl %eax, rhizo_boot_pml4 + 511 * 8

    /* 5. PAE, CR3, EFER.LME | EFER.NXE, CR0.PG | CR0.WP | CR0.PE. */
    movl %cr4, %eax
    orl $(1 << 5), %eax
    movl %eax, %cr4

    movl $rhizo_boot_pml4, %eax
    movl %eax, %cr3

    movl $0xC0000080, %ecx
    rdmsr
    orl $((1 << 8) | (1 << 11)), %eax
    wrmsr

    movl %cr0, %eax
    orl $0x80010001, %eax
    movl %eax, %cr0

    lgdt rhizo_boot_gdt_ptr
    ljmp $0x08, $4f

    /* Wrong boot protocol: stop without running any kernel code. */
9:
    cli
    hlt
    jmp 9b

    /* ---- 64-bit, still at the physical address ---- */
    .code64
4:
    movw $0x10, %ax
    movw %ax, %ds
    movw %ax, %es
    movw %ax, %ss
    movw %ax, %fs
    movw %ax, %gs

    /* 6. Arguments, higher-half stack, then jump to the higher half. */
    movl rhizo_boot_start_info, %edi
    movq rhizo_boot_tsc, %rsi
    movabsq $rhizo_kernel_stack_top, %rsp
    xorl %ebp, %ebp
    movabsq $kernel_main, %rax
    call *%rax
5:
    cli
    hlt
    jmp 5b

    /* ---- Boot GDT: null, 64-bit code (0x08), data (0x10) ---- */
    .section .rodata.boot, "a"
    .balign 16
rhizo_boot_gdt:
    .quad 0
    .quad 0x00af9a000000ffff
    .quad 0x00cf92000000ffff
rhizo_boot_gdt_end:
rhizo_boot_gdt_ptr:
    .word rhizo_boot_gdt_end - rhizo_boot_gdt - 1
    .long rhizo_boot_gdt

    /* ---- Stage-1 tables and boot data (physical, zeroed at entry) ---- */
    .section .bss.boot, "aw", @nobits
    .balign 4096
rhizo_boot_pml4:
    .skip 4096
rhizo_boot_pdpt_dm:
    .skip 4096
rhizo_boot_pdpt_k:
    .skip 4096
rhizo_boot_pd:
    .skip 4 * 4096
    .balign 8
rhizo_boot_tsc:
    .skip 8
rhizo_boot_start_info:
    .skip 4

    /* ---- Kernel stack (higher half) with an unmapped guard page ---- */
    .section .bss.kstack, "aw", @nobits
    .balign 4096
    .global rhizo_kernel_stack_guard
rhizo_kernel_stack_guard:
    .skip 4096
    .global rhizo_kernel_stack_bottom
rhizo_kernel_stack_bottom:
    .skip {stack_size}
    .global rhizo_kernel_stack_top
rhizo_kernel_stack_top:

    .text
    "#,
    magic = const PVH_START_INFO_MAGIC,
    stack_size = const KERNEL_STACK_SIZE,
    options(att_syntax)
);

/// Boundaries of the kernel image, from the linker script.
pub mod layout {
    unsafe extern "C" {
        static __kernel_phys_start: u8;
        static __kernel_phys_end: u8;
        static __text_start: u8;
        static __text_end: u8;
        static __rodata_start: u8;
        static __rodata_end: u8;
        static __data_start: u8;
        static __bss_end: u8;
        static rhizo_kernel_stack_guard: u8;
    }

    macro_rules! symbol_address {
        ($name:ident) => {
            // Only the address of the linker symbol is taken; nothing is read.
            core::ptr::addr_of!($name) as u64
        };
    }

    /// Physical range `[start, end)` occupied by the whole kernel image.
    pub fn kernel_phys() -> (u64, u64) {
        (
            symbol_address!(__kernel_phys_start),
            symbol_address!(__kernel_phys_end),
        )
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

    /// Virtual range of writable data and `.bss` (includes the kernel stack).
    pub fn data() -> (u64, u64) {
        (symbol_address!(__data_start), symbol_address!(__bss_end))
    }

    /// Virtual address of the guard page below the kernel stack.
    pub fn stack_guard() -> u64 {
        symbol_address!(rhizo_kernel_stack_guard)
    }
}

use core::sync::atomic::{AtomicU64, Ordering};

static ENTRY_TSC: AtomicU64 = AtomicU64::new(0);
static START_INFO: AtomicU64 = AtomicU64::new(0);

/// Records the values the trampoline passed to `kernel_main`.
pub fn record(start_info: u64, entry_tsc: u64) {
    START_INFO.store(start_info, Ordering::Relaxed);
    ENTRY_TSC.store(entry_tsc, Ordering::Relaxed);
}

#[cfg_attr(
    not(feature = "ktest"),
    allow(dead_code, reason = "used by in-kernel tests; cells use it from H2")
)]
/// The TSC value read at the very first instruction of the kernel.
pub fn entry_cycles() -> u64 {
    ENTRY_TSC.load(Ordering::Relaxed)
}

#[cfg_attr(
    not(feature = "ktest"),
    allow(dead_code, reason = "used by in-kernel tests; cells use it from H2")
)]
/// Physical address of the `hvm_start_info` structure passed by the hypervisor.
pub fn start_info_addr() -> u64 {
    START_INFO.load(Ordering::Relaxed)
}
