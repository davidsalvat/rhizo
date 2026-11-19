/*
 * Rhizo kernel layout (x86_64, PVH direct boot, higher-half kernel).
 *
 * Physical load address (LMA): 1 MiB. The hypervisor loads segments at their
 * physical addresses and enters `pvh_start32` in 32-bit mode, paging off.
 *
 * - `.boot` (trampoline code, its GDT and stage-1 page tables) is linked at
 *   its physical address so it can run before paging is enabled.
 * - Everything else is linked at KERNEL_BASE + physical address and loaded at
 *   the physical address (AT()).
 */
ENTRY(pvh_start32)

KERNEL_BASE = 0xffffffff80000000;

PHDRS
{
    boot   PT_LOAD FLAGS(7);   /* R W X, only used during boot */
    text   PT_LOAD FLAGS(5);   /* R X */
    rodata PT_LOAD FLAGS(4);   /* R   */
    data   PT_LOAD FLAGS(6);   /* R W */
    note   PT_NOTE FLAGS(4);
}

SECTIONS
{
    . = 1M;
    __kernel_phys_start = .;

    .boot : ALIGN(4K) {
        KEEP(*(.text.boot))
        *(.rodata.boot)
        *(.data.boot)
    } :boot

    .boot.bss (NOLOAD) : ALIGN(4K) {
        __boot_bss_start = .;
        *(.bss.boot)
        . = ALIGN(8);
        __boot_bss_end = .;
    } :boot

    . = ALIGN(4K);
    . += KERNEL_BASE;

    .text : AT(ADDR(.text) - KERNEL_BASE) ALIGN(4K) {
        __text_start = .;
        *(.text .text.*)
        . = ALIGN(4K);
        __text_end = .;
    } :text

    .rodata : AT(ADDR(.rodata) - KERNEL_BASE) ALIGN(4K) {
        __rodata_start = .;
        *(.rodata .rodata.*)
    } :rodata

    .note : AT(ADDR(.note) - KERNEL_BASE) ALIGN(4) {
        KEEP(*(.note.Xen))
        . = ALIGN(4K);
        __rodata_end = .;
    } :rodata :note

    .data : AT(ADDR(.data) - KERNEL_BASE) ALIGN(4K) {
        __data_start = .;
        *(.data .data.*)
        *(.got .got.*)
    } :data

    .bss : AT(ADDR(.bss) - KERNEL_BASE) ALIGN(4K) {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(4K);
        __bss_end = .;
    } :data

    __kernel_phys_end = . - KERNEL_BASE;
    __bss_phys_start = __bss_start - KERNEL_BASE;
    __bss_phys_end = __bss_end - KERNEL_BASE;

    /DISCARD/ : {
        *(.eh_frame .eh_frame_hdr)
        *(.comment)
    }
}
