/*
 * Rhizo kernel layout (x86_64, PVH direct boot).
 *
 * The hypervisor loads the ELF segments at their physical addresses and
 * enters `pvh_start32` (advertised in the PVH ELF note) in 32-bit protected
 * mode with paging disabled. The kernel is linked and identity-mapped at 1 MiB.
 */
ENTRY(pvh_start32)

PHDRS
{
    text   PT_LOAD FLAGS(5);   /* R X */
    rodata PT_LOAD FLAGS(4);   /* R   */
    data   PT_LOAD FLAGS(6);   /* R W */
    note   PT_NOTE FLAGS(4);
}

SECTIONS
{
    . = 1M;

    .text : ALIGN(4K) {
        KEEP(*(.text.boot))
        *(.text .text.*)
    } :text

    .rodata : ALIGN(4K) {
        *(.rodata .rodata.*)
    } :rodata

    .note : ALIGN(4) {
        KEEP(*(.note.Xen))
    } :rodata :note

    .data : ALIGN(4K) {
        *(.data .data.*)
        *(.got .got.*)
    } :data

    .bss : ALIGN(4K) {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(8);
        __bss_end = .;
    } :data

    /DISCARD/ : {
        *(.eh_frame .eh_frame_hdr)
        *(.comment)
    }
}
