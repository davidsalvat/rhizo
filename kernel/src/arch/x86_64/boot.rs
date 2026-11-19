//! PVH entry point: from 32-bit protected mode to Rust in 64-bit long mode.
//!
//! The hypervisor enters `pvh_start32` with:
//! - 32-bit protected mode, flat segments, paging disabled, interrupts off;
//! - `ebx` holding the physical address of the `hvm_start_info` structure.
//!
//! The trampoline:
//! 1. checks the `hvm_start_info` magic and halts forever if it is wrong;
//! 2. records the TSC as early as possible (for the boot-time report);
//! 3. zeroes `.bss` (page tables, stack and Rust statics live there);
//! 4. identity-maps the first 1 GiB with 2 MiB pages;
//! 5. enables PAE, long mode and paging, loads a 64-bit GDT;
//! 6. sets up the stack and calls `kernel_main`.

/// Magic value at offset 0 of `hvm_start_info` ("xEn3" with the high bit set).
pub const PVH_START_INFO_MAGIC: u32 = 0x336e_c578;

/// Size of the boot stack.
const BOOT_STACK_SIZE: usize = 64 * 1024;

core::arch::global_asm!(
    r#"
    /* ---- PVH ELF note: tells the hypervisor where the 32-bit entry is ---- */
    .section .note.Xen, "a", @note
    .balign 4
    .long 4                     /* name size: "Xen\0" */
    .long 4                     /* descriptor size */
    .long 18                    /* XEN_ELFNOTE_PHYS32_ENTRY */
    .asciz "Xen"
    .balign 4
    .long pvh_start32
    .balign 4

    /* ---- 32-bit entry ---- */
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

    /* 3. Zero .bss (edi = start, ecx = length, al = 0). */
    movl $__bss_start, %edi
    movl $__bss_end, %ecx
    subl %edi, %ecx
    xorl %eax, %eax
    rep stosb

    movl %esi, rhizo_boot_tsc
    movl %ebp, rhizo_boot_tsc + 4
    movl %ebx, rhizo_boot_start_info

    /* 4. Page tables: PML4[0] -> PDPT, PDPT[0] -> PD, PD[i] -> 2 MiB page i. */
    movl $rhizo_boot_pdpt, %eax
    orl $0x3, %eax              /* present | writable */
    movl %eax, rhizo_boot_pml4
    movl $rhizo_boot_pd, %eax
    orl $0x3, %eax
    movl %eax, rhizo_boot_pdpt
    xorl %ecx, %ecx
2:
    movl %ecx, %eax
    shll $21, %eax
    orl $0x83, %eax             /* present | writable | huge (2 MiB) */
    movl %eax, rhizo_boot_pd(,%ecx,8)
    incl %ecx
    cmpl $512, %ecx
    jne 2b

    /* 5. PAE, then CR3, then EFER.LME, then paging. */
    movl %cr4, %eax
    orl $(1 << 5), %eax
    movl %eax, %cr4

    movl $rhizo_boot_pml4, %eax
    movl %eax, %cr3

    movl $0xC0000080, %ecx
    rdmsr
    orl $(1 << 8), %eax
    wrmsr

    movl %cr0, %eax
    orl $0x80000001, %eax       /* PG | PE */
    movl %eax, %cr0

    lgdt rhizo_boot_gdt_ptr
    ljmp $0x08, $3f

    /* Wrong boot protocol: stop without running any kernel code. */
9:
    cli
    hlt
    jmp 9b

    /* ---- 64-bit ---- */
    .code64
3:
    movw $0x10, %ax
    movw %ax, %ds
    movw %ax, %es
    movw %ax, %ss
    movw %ax, %fs
    movw %ax, %gs

    /* 6. Stack (16-byte aligned) and Rust. */
    movabsq $rhizo_boot_stack_top, %rsp
    xorl %ebp, %ebp
    call kernel_main
4:
    cli
    hlt
    jmp 4b

    /* ---- GDT: null, 64-bit code (0x08), data (0x10) ---- */
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

    /* ---- Zeroed at boot: page tables, stack, boot data ---- */
    .section .bss.boot, "aw", @nobits
    .balign 4096
rhizo_boot_pml4:
    .skip 4096
rhizo_boot_pdpt:
    .skip 4096
rhizo_boot_pd:
    .skip 4096
    .balign 16
    .skip {stack_size}
rhizo_boot_stack_top:

    .balign 8
    .global rhizo_boot_tsc
rhizo_boot_tsc:
    .skip 8
    .global rhizo_boot_start_info
rhizo_boot_start_info:
    .skip 4

    .text
    "#,
    magic = const PVH_START_INFO_MAGIC,
    stack_size = const BOOT_STACK_SIZE,
    options(att_syntax)
);

unsafe extern "C" {
    static rhizo_boot_tsc: u64;
    #[cfg_attr(
        not(feature = "ktest"),
        allow(dead_code, reason = "read by tests now, by memory setup in H1")
    )]
    static rhizo_boot_start_info: u32;
}

/// The TSC value read at the very first instruction of the kernel.
pub fn entry_cycles() -> u64 {
    // SAFETY: written exactly once by the boot trampoline before any Rust code
    // runs, and never written again, so reading it cannot race.
    unsafe { core::ptr::addr_of!(rhizo_boot_tsc).read() }
}

/// Physical address of the `hvm_start_info` structure passed by the hypervisor.
#[cfg_attr(
    not(feature = "ktest"),
    allow(dead_code, reason = "read by tests now, by memory setup in H1")
)]
pub fn start_info_addr() -> u32 {
    // SAFETY: written exactly once by the boot trampoline before any Rust code
    // runs, and never written again.
    unsafe { core::ptr::addr_of!(rhizo_boot_start_info).read() }
}
