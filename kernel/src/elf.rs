//! Minimal, hand-rolled ELF64 loader for user processes (Phase 4).
//!
//! ## Supported subset -- documented exactly, not implied
//!
//! - **Class**: `ELFCLASS64` only (32-bit ELF is rejected).
//! - **Data encoding**: `ELFDATA2LSB` (little-endian) only -- the only
//!   encoding x86_64 uses.
//! - **Type**: `ET_EXEC` only. Position-independent executables (`ET_DYN`)
//!   and relocatable objects (`ET_REL`) are rejected -- this loader does no
//!   relocation processing at all, so every address in the file must
//!   already be the real, final virtual address.
//! - **Machine**: `EM_X86_64` only.
//! - **Segments**: only `PT_LOAD` is processed. `PT_INTERP` or `PT_DYNAMIC`
//!   (dynamic linking) cause the whole ELF to be rejected, since this
//!   loader has no dynamic linker behind it. Any other segment type
//!   (`PT_NOTE`, `PT_GNU_STACK`, `PT_PHDR`, ...) is silently skipped -- it
//!   describes something this minimal loader doesn't need to act on, not
//!   something it needs to reject.
//! - **Sections**: not read at all. A real linker's section headers carry
//!   debug info and symbol tables; nothing this loader does needs them,
//!   only the program headers, which describe what actually gets mapped at
//!   runtime.
//! - **Address range**: every `PT_LOAD` segment must fall entirely within
//!   `paging::USER_SPACE_BASE .. +USER_SPACE_SIZE` -- seting `p_vaddr`
//!   anywhere else (e.g. inside kernel address space) is rejected outright,
//!   both here and again, independently, by `paging::map_in_address_space`
//!   itself (see its docs).
//!
//! Every offset and size taken from the file is validated with checked
//! arithmetic before use -- a truncated file, an overflowing
//! offset+size pair, or a segment claiming a range outside the file's own
//! bytes is a rejected load, never a read past the buffer.

use x86_64::structures::paging::{Page, PageTableFlags, Size4KiB};
use x86_64::VirtAddr;

use crate::paging::{self, AddressSpace};

const EI_CLASS: usize = 4;
const EI_DATA: usize = 5;
const EI_VERSION: usize = 6;

const ELFCLASS64: u8 = 2;
const ELFDATA2LSB: u8 = 1;
const EV_CURRENT: u8 = 1;

const ET_EXEC: u16 = 2;
const EM_X86_64: u16 = 62;

const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PT_INTERP: u32 = 3;

const PF_X: u32 = 1;
const PF_W: u32 = 2;
const PF_R: u32 = 4;

const EHDR_SIZE: usize = 64;
const PHDR_SIZE: usize = 56;

/// What a successful load produced -- just enough for `task.rs` to build
/// the initial Ring 3 entry.
pub struct LoadedElf {
    pub entry_point: VirtAddr,
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, &'static str> {
    let slice = bytes
        .get(offset..offset + 2)
        .ok_or("truncated ELF header")?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, &'static str> {
    let slice = bytes
        .get(offset..offset + 4)
        .ok_or("truncated ELF header")?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, &'static str> {
    let slice = bytes
        .get(offset..offset + 8)
        .ok_or("truncated ELF header")?;
    Ok(u64::from_le_bytes(slice.try_into().unwrap()))
}

struct ProgramHeader {
    p_type: u32,
    p_flags: u32,
    p_offset: u64,
    p_vaddr: u64,
    p_filesz: u64,
    p_memsz: u64,
}

fn parse_program_header(bytes: &[u8], offset: usize) -> Result<ProgramHeader, &'static str> {
    if offset
        .checked_add(PHDR_SIZE)
        .ok_or("program header offset overflow")?
        > bytes.len()
    {
        return Err("program header out of bounds");
    }
    Ok(ProgramHeader {
        p_type: read_u32(bytes, offset)?,
        p_flags: read_u32(bytes, offset + 4)?,
        p_offset: read_u64(bytes, offset + 8)?,
        p_vaddr: read_u64(bytes, offset + 16)?,
        p_filesz: read_u64(bytes, offset + 32)?,
        p_memsz: read_u64(bytes, offset + 40)?,
    })
}

/// Confirm `[addr, addr+len)` (checked, no wraparound) falls entirely
/// within the process user-address range. This is the loader's own gate,
/// independent of (and in addition to) the identical check
/// `paging::map_in_address_space` performs -- rejecting here produces a
/// clean load error instead of a mapping failure partway through.
fn in_user_range(addr: u64, len: u64) -> bool {
    let Some(end) = addr.checked_add(len) else {
        return false;
    };
    addr >= paging::USER_SPACE_BASE && end <= paging::USER_SPACE_BASE + paging::USER_SPACE_SIZE
}

fn segment_page_flags(p_flags: u32) -> PageTableFlags {
    let mut flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
    if p_flags & PF_W != 0 {
        flags |= PageTableFlags::WRITABLE;
    }
    if p_flags & PF_X == 0 {
        // Only meaningful once EFER.NXE is enabled -- see `paging::enable_nx`,
        // called unconditionally at the very start of `kernel_main`.
        flags |= PageTableFlags::NO_EXECUTE;
    }
    flags
}

/// Validate and load `bytes` as an ELF64 executable into `space`, mapping
/// every `PT_LOAD` segment with the permissions its `p_flags` describe.
///
/// Segments are always mapped `WRITABLE` first so their bytes can be
/// copied in (and their BSS tail zeroed) through the kernel-side
/// physical-memory-offset path (`paging::write_bytes_in_address_space`),
/// then narrowed to their real, final permissions with
/// `paging::update_flags_in_address_space` -- so a segment without `PF_W`
/// (ordinarily: code and rodata) genuinely ends up non-writable once this
/// function returns, not just "writable but the loader promises not to
/// write again."
pub fn load(space: &mut AddressSpace, bytes: &[u8]) -> Result<LoadedElf, &'static str> {
    if bytes.len() < EHDR_SIZE {
        return Err("ELF: file too small for a header");
    }
    if bytes[0..4] != [0x7f, b'E', b'L', b'F'] {
        return Err("ELF: bad magic");
    }
    if bytes[EI_CLASS] != ELFCLASS64 {
        return Err("ELF: not a 64-bit object (ELFCLASS64 required)");
    }
    if bytes[EI_DATA] != ELFDATA2LSB {
        return Err("ELF: not little-endian (ELFDATA2LSB required)");
    }
    if bytes[EI_VERSION] != EV_CURRENT {
        return Err("ELF: unsupported e_ident version");
    }

    let e_type = read_u16(bytes, 16)?;
    let e_machine = read_u16(bytes, 18)?;
    let e_version = read_u32(bytes, 20)?;
    let e_entry = read_u64(bytes, 24)?;
    let e_phoff = read_u64(bytes, 32)?;
    let e_phentsize = read_u16(bytes, 54)?;
    let e_phnum = read_u16(bytes, 56)?;

    if e_type != ET_EXEC {
        return Err("ELF: unsupported e_type (only ET_EXEC is supported -- no relocations/PIE)");
    }
    if e_machine != EM_X86_64 {
        return Err("ELF: unsupported e_machine (only EM_X86_64 is supported)");
    }
    if e_version != EV_CURRENT as u32 {
        return Err("ELF: unsupported e_version");
    }
    if e_phentsize as usize != PHDR_SIZE {
        return Err(
            "ELF: unsupported e_phentsize (only the standard 56-byte Elf64_Phdr is supported)",
        );
    }
    if e_phnum == 0 {
        return Err("ELF: no program headers (nothing to load)");
    }

    let ph_table_len = (e_phentsize as u64)
        .checked_mul(e_phnum as u64)
        .ok_or("ELF: program header table size overflow")?;
    let ph_table_end = e_phoff
        .checked_add(ph_table_len)
        .ok_or("ELF: program header table offset overflow")?;
    if ph_table_end > bytes.len() as u64 {
        return Err("ELF: program header table out of bounds");
    }

    if !in_user_range(e_entry, 1) {
        return Err("ELF: entry point outside the permitted user address range");
    }

    // Two passes: first validate and reject the *entire* file if anything
    // is malformed or unsupported, before mapping a single page -- a
    // partially loaded process is not a safe thing to ever hand control to.
    let mut headers = alloc::vec::Vec::with_capacity(e_phnum as usize);
    for i in 0..e_phnum as usize {
        let offset = e_phoff as usize + i * PHDR_SIZE;
        let ph = parse_program_header(bytes, offset)?;

        if ph.p_type == PT_DYNAMIC || ph.p_type == PT_INTERP {
            return Err("ELF: dynamic linking is not supported by this loader");
        }
        if ph.p_type != PT_LOAD {
            continue;
        }
        if ph.p_filesz > ph.p_memsz {
            return Err("ELF: segment p_filesz exceeds p_memsz");
        }
        let file_end = ph
            .p_offset
            .checked_add(ph.p_filesz)
            .ok_or("ELF: segment file range overflow")?;
        if file_end > bytes.len() as u64 {
            return Err("ELF: segment file range out of bounds");
        }
        if !in_user_range(ph.p_vaddr, ph.p_memsz) {
            return Err("ELF: segment virtual address range outside the permitted user region");
        }
        if ph.p_flags & PF_R == 0 {
            return Err("ELF: segment without PF_R is not supported (nothing this loader maps is ever fully inaccessible)");
        }

        headers.push(ph);
    }

    if headers.is_empty() {
        return Err("ELF: no PT_LOAD segments (nothing to load)");
    }

    // Second pass: map, populate, and lock down permissions.
    for ph in &headers {
        let seg_start = ph.p_vaddr;
        let seg_end = seg_start + ph.p_memsz; // already range-checked above
        let page_start = seg_start & !0xFFF;
        let page_end = (seg_end + 0xFFF) & !0xFFF;

        let final_flags = segment_page_flags(ph.p_flags);
        let staging_flags =
            PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;

        let mut page_addr = page_start;
        while page_addr < page_end {
            let page: Page<Size4KiB> = Page::containing_address(VirtAddr::new(page_addr));
            paging::map_in_address_space(space, page, staging_flags)?;
            page_addr += 4096;
        }

        // Zero the *entire* freshly mapped range first, not just the
        // segment's own BSS tail: `page_start` can sit before `seg_start`
        // and `page_end` after `seg_end` (p_vaddr/memsz need not be
        // page-aligned), and those margin bytes belong to a physical frame
        // this process just received from the allocator -- possibly reused
        // from a previous process's freed address space (see
        // `paging::zero_bytes_in_address_space`'s docs). Leaving them
        // unzeroed would let a new process read whatever a prior owner left
        // there.
        paging::zero_bytes_in_address_space(
            space,
            VirtAddr::new(page_start),
            page_end - page_start,
        )?;

        let file_bytes = &bytes[ph.p_offset as usize..(ph.p_offset + ph.p_filesz) as usize];
        paging::write_bytes_in_address_space(space, VirtAddr::new(seg_start), file_bytes)?;

        if final_flags != staging_flags {
            let mut page_addr = page_start;
            while page_addr < page_end {
                let page: Page<Size4KiB> = Page::containing_address(VirtAddr::new(page_addr));
                paging::update_flags_in_address_space(space, page, final_flags)?;
                page_addr += 4096;
            }
        }
    }

    Ok(LoadedElf {
        entry_point: VirtAddr::new(e_entry),
    })
}
