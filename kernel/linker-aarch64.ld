/*
 * Rhizo kernel layout (AArch64, Linux arm64 Image boot protocol).
 *
 * The image is linked at KERNEL_BASE and loaded by the VMM at any 2 MiB-aligned
 * physical address. The entry code runs at that physical address with the MMU
 * off (PC-relative addressing only), builds stage-1 tables mapping the image
 * at KERNEL_BASE, and jumps there.
 */
ENTRY(_start)

KERNEL_BASE = 0xffffffff80000000;

PHDRS
{
    text   PT_LOAD FLAGS(5);
    rodata PT_LOAD FLAGS(4);
    data   PT_LOAD FLAGS(6);
}

SECTIONS
{
    . = KERNEL_BASE;
    __image_start = .;

    .text : {
        __text_start = .;
        KEEP(*(.text.head))
        *(.text .text.*)
        . = ALIGN(4K);
        __text_end = .;
    } :text

    .rodata : ALIGN(4K) {
        __rodata_start = .;
        *(.rodata .rodata.*)
        . = ALIGN(4K);
        __rodata_end = .;
    } :rodata

    .data : ALIGN(4K) {
        __data_start = .;
        *(.data .data.*)
        *(.got .got.*)
    } :data

    .bss : ALIGN(4K) {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(4K);
        __bss_end = .;
    } :data

    __image_end = .;
    __image_size = __image_end - __image_start;

    /DISCARD/ : {
        *(.eh_frame .eh_frame_hdr)
        *(.comment)
    }
}
