/* Fixed-address, non-PIE layout for kernel/src/elf.rs's minimal ET_EXEC-only
 * loader -- no dynamic linking, no relocations, every address in the
 * produced ELF is already final.
 *
 * The base address MUST match `paging::USER_SPACE_BASE` in the kernel
 * (kernel/src/paging.rs) -- that's the one PML4 slot every process's
 * private address space reserves, and the loader rejects any PT_LOAD
 * segment (or entry point) outside
 * [USER_SPACE_BASE, USER_SPACE_BASE + USER_SPACE_SIZE).
 */

ENTRY(_start)

PHDRS
{
    text_seg PT_LOAD FLAGS(5); /* PF_R | PF_X */
    data_seg PT_LOAD FLAGS(6); /* PF_R | PF_W */
}

SECTIONS
{
    . = 0x700000000000;

    .text : {
        *(.text .text.*)
    } :text_seg

    .rodata : {
        *(.rodata .rodata.*)
    } :text_seg

    . = ALIGN(4096);

    .data : {
        *(.data .data.*)
    } :data_seg

    .bss : {
        *(.bss .bss.*)
    } :data_seg

    /DISCARD/ : {
        *(.comment)
        *(.eh_frame*)
        *(.note.*)
        *(.got*)
        *(.rela.*)
    }
}
