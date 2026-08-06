//! Physical frame allocation and virtual memory mapping (Phase 2).
//!
//! Before this module, the kernel heap was a static array baked into the
//! binary's `.bss` section specifically *to avoid* touching physical
//! memory or page tables at all -- the v0.5 `memory.rs` doc comment says
//! so directly. That was a reasonable way to get an early kernel running,
//! but it means the kernel could never map anything else: no user-process
//! memory, no MMIO, no growing the heap past a size fixed at compile time.
//!
//! This module gives the kernel real access to physical memory (via the
//! bootloader's `physical_memory_offset` mapping, opted into in `main.rs`),
//! a frame allocator over the usable regions the bootloader reports, and a
//! thin, error-returning wrapper around `x86_64::structures::paging::Mapper`
//! so callers get a `Result` instead of a panic when a mapping can't be
//! made.

use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use bootloader_api::info::{MemoryRegionKind, MemoryRegions};
use spin::Mutex;
use x86_64::structures::paging::mapper::{Translate, TranslateResult};
use x86_64::structures::paging::{
    FrameAllocator, FrameDeallocator, Mapper, OffsetPageTable, Page, PageTable, PageTableFlags,
    PhysFrame, Size4KiB,
};
use x86_64::{PhysAddr, VirtAddr};

/// Enable the CPU's no-execute page-protection feature (`EFER.NXE`).
///
/// Must run before any page is ever mapped with `PageTableFlags::NO_EXECUTE`
/// set -- until this bit is on, the CPU treats that flag as a reserved,
/// must-be-zero bit in every page-table entry, rather than the deny-execute
/// permission it's meant to be. Marking a page `NO_EXECUTE` while `NXE=0`
/// would not fail loudly; it would just silently *not* deny execution (or,
/// on stricter hardware, fault on a "reserved bit set" violation that has
/// nothing to do with the intended protection) -- either way, a security
/// property the rest of the kernel believes is enforced would not actually
/// be. Called once, at the very start of `kernel_main`, before any paging
/// setup happens at all -- see `main.rs`.
pub fn enable_nx() {
    use x86_64::registers::model_specific::{Efer, EferFlags};
    // Safety: setting NXE only makes future NO_EXECUTE-flagged page-table
    // entries actually deny execution instead of being ignored/reserved. It
    // cannot break any existing mapping: nothing has marked a page
    // NO_EXECUTE yet at this point in boot -- this runs before any page
    // table this kernel controls is even created.
    unsafe {
        Efer::update(|flags| flags.insert(EferFlags::NO_EXECUTE_ENABLE));
    }
    // Verify rather than assume: a feature bit that silently failed to
    // stick would make every later NO_EXECUTE mapping a no-op instead of
    // the enforced boundary the rest of this phase's isolation claims
    // depend on.
    assert!(
        Efer::read().contains(EferFlags::NO_EXECUTE_ENABLE),
        "enable_nx: EFER.NXE did not take effect -- CPU does not support (or rejected) the no-execute feature"
    );
    crate::serial_println!("paging: EFER.NXE enabled and verified");
}

/// Build an `OffsetPageTable` over the CPU's currently active level-4 page
/// table.
///
/// # Safety
/// The complete physical memory must already be mapped starting at
/// `physical_memory_offset` (see `BOOTLOADER_CONFIG` in `main.rs`), and
/// this must be called at most once for the lifetime of the returned
/// value: it hands out a `&'static mut` to the live page table, and a
/// second live `&mut` to the same table would be aliasing, which is
/// undefined behavior.
pub unsafe fn init(physical_memory_offset: VirtAddr) -> OffsetPageTable<'static> {
    let level_4_table = active_level_4_table(physical_memory_offset);
    OffsetPageTable::new(level_4_table, physical_memory_offset)
}

/// # Safety
/// Same requirement as `init`: the physical-memory mapping must be live,
/// and this must not be called more than once (see `init`).
unsafe fn active_level_4_table(physical_memory_offset: VirtAddr) -> &'static mut PageTable {
    use x86_64::registers::control::Cr3;

    let (level_4_frame, _) = Cr3::read();
    let phys = level_4_frame.start_address();
    let virt = physical_memory_offset + phys.as_u64();
    let page_table_ptr: *mut PageTable = virt.as_mut_ptr();

    &mut *page_table_ptr
}

/// A physical frame allocator over the bootloader's usable memory regions.
///
/// This is a real, if simple, allocator: frames are bump-allocated from the
/// usable regions the bootloader reported, but freed frames go onto a
/// small pool and are reused before the bump cursor advances further --
/// deallocation genuinely returns memory to circulation rather than
/// leaking it forever.
pub struct BootInfoFrameAllocator {
    memory_regions: &'static MemoryRegions,
    next: usize,
    freed: Vec<PhysFrame<Size4KiB>>,
    allocated_count: usize,
}

impl BootInfoFrameAllocator {
    /// # Safety
    /// `memory_regions` must be the bootloader-provided map from `BootInfo`:
    /// every region marked `Usable` is handed out as free RAM, so the map
    /// must accurately reflect what's genuinely unused.
    pub unsafe fn init(memory_regions: &'static MemoryRegions) -> Self {
        Self {
            memory_regions,
            next: 0,
            freed: Vec::new(),
            allocated_count: 0,
        }
    }

    fn usable_frames(&self) -> impl Iterator<Item = PhysFrame> + '_ {
        self.memory_regions
            .iter()
            .filter(|region| region.kind == MemoryRegionKind::Usable)
            .flat_map(|region| (region.start..region.end).step_by(4096))
            .map(|addr| PhysFrame::containing_address(PhysAddr::new(addr)))
    }

    /// Total frames handed out over this allocator's lifetime (including
    /// ones later freed and re-handed-out) -- a monotonically increasing
    /// counter, distinct from currently-in-use frames. Because a reused
    /// frame increments this exactly like a fresh one, `frames_allocated()
    /// - frames_in_free_pool()` is *not* a valid "frames currently in use"
    /// metric whenever any reuse has happened -- see `frames_bumped()` for
    /// the one that actually is.
    pub fn frames_allocated(&self) -> usize {
        self.allocated_count
    }

    /// Frames that were freed and are waiting to be reused.
    pub fn frames_in_free_pool(&self) -> usize {
        self.freed.len()
    }

    /// How far the bump cursor over *fresh* memory has advanced --
    /// distinct from `frames_allocated()`, which also counts every reused
    /// frame. This only ever grows when `allocate_frame` finds the free
    /// list empty and has to hand out memory it has never given out
    /// before; a frame recycled through `deallocate_frame` and reused via
    /// the free list never touches it. This is the metric a leak test
    /// should compare before/after a batch of allocate+free cycles: if
    /// nothing leaked, every one of those frees left something in the free
    /// list for the next allocation to reuse, so this stays flat no matter
    /// how many cycles ran; if something leaked, the free list runs dry
    /// and this grows once per leaked frame (see `shell.rs`'s
    /// `spawnfail` command).
    pub fn frames_bumped(&self) -> usize {
        self.next
    }
}

// Safety: `allocate_frame` only ever returns frames from the bootloader's
// `Usable` regions (or previously-freed frames that came from the same
// source), and each physical frame is handed out at most once before
// being explicitly freed -- the bump cursor never repeats a frame, and
// the free-list only returns frames this same allocator previously gave
// out.
unsafe impl FrameAllocator<Size4KiB> for BootInfoFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        if let Some(frame) = self.freed.pop() {
            self.allocated_count += 1;
            return Some(frame);
        }

        let frame = self.usable_frames().nth(self.next);
        self.next += 1;
        if frame.is_some() {
            self.allocated_count += 1;
        }
        frame
    }
}

impl FrameDeallocator<Size4KiB> for BootInfoFrameAllocator {
    /// # Safety
    /// `frame` must have been obtained from this same allocator's
    /// `allocate_frame` and must no longer be mapped or in use anywhere.
    unsafe fn deallocate_frame(&mut self, frame: PhysFrame<Size4KiB>) {
        self.freed.push(frame);
    }
}

/// Map one page to a freshly allocated frame with the given flags.
///
/// Returns a descriptive `Err` instead of panicking on failure (out of
/// physical memory, or the page is already mapped) -- callers decide how
/// to react rather than the mapping code deciding for them.
pub fn map_page(
    mapper: &mut OffsetPageTable<'static>,
    frame_allocator: &mut BootInfoFrameAllocator,
    page: Page<Size4KiB>,
    flags: PageTableFlags,
) -> Result<(), &'static str> {
    let frame = frame_allocator
        .allocate_frame()
        .ok_or("out of physical memory frames")?;

    // Safety: `frame` was just allocated by `frame_allocator` and is not
    // mapped anywhere else (frames are only ever handed out once between
    // allocation and a matching deallocation), and `page` is the caller's
    // to map -- satisfying map_to's aliasing requirement.
    unsafe {
        mapper
            .map_to(page, frame, flags, frame_allocator)
            .map_err(|_| "page mapping failed: already mapped or invalid page table state")?
            .flush();
    }
    Ok(())
}

/// Global mapper + frame allocator, installed once by `memory::init_heap`.
/// A `Mutex` rather than a bare `static mut`: Phase 3's scheduler will make
/// these genuinely reachable from more than one execution context, and
/// this is the point past which that needs to already be safe.
static MAPPER: Mutex<Option<OffsetPageTable<'static>>> = Mutex::new(None);
static FRAME_ALLOCATOR: Mutex<Option<BootInfoFrameAllocator>> = Mutex::new(None);

/// The single sanctioned way to touch `MAPPER` and/or `FRAME_ALLOCATOR`,
/// mirroring `task::with_scheduler` and `keyboard::with_queue`: both locks
/// are plain `spin::Mutex`, and the 100 Hz timer ISR can preempt any task
/// mid-critical-section on this single-core kernel. Without disabling
/// interrupts for the duration of the lock(s), a tick landing while a task
/// holds either lock would have the ISR's own path (or a re-scheduled task)
/// spin on a lock its own preemption victim can never resume to release --
/// the same deadlock shape already fixed once in `task.rs` and again in
/// `allocator.rs`/`keyboard.rs`. Both locks are always acquired together
/// here, in the same order, so there's no separate lock-ordering hazard to
/// introduce by routing everything through one function.
fn with_paging<F, R>(f: F) -> R
where
    F: FnOnce(&mut Option<OffsetPageTable<'static>>, &mut Option<BootInfoFrameAllocator>) -> R,
{
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut mapper = MAPPER.lock();
        let mut frame_allocator = FRAME_ALLOCATOR.lock();
        f(&mut mapper, &mut frame_allocator)
    })
}

/// Set once by `install` below, read-only for the rest of the kernel's
/// life afterward -- plain relaxed atomics rather than another `Mutex`
/// deliberately: adding more locks to the interrupt-safety surface for
/// state that's written exactly once, during single-threaded boot, before
/// interrupts are even enabled, would be pure overhead. `0` means "not set
/// yet"; both a real physical-memory offset and a real PML4 physical
/// address are always non-zero, so it doubles as the "unset" sentinel.
static PHYS_MEM_OFFSET: AtomicU64 = AtomicU64::new(0);
static KERNEL_PML4_FRAME: AtomicU64 = AtomicU64::new(0);

/// Install the mapper and frame allocator built during heap setup as the
/// kernel-wide instances used by diagnostics and (from later phases) new
/// mappings outside the heap. Also records the physical-memory offset and
/// the kernel's own PML4 frame (read from CR3, which at this point in boot
/// -- before `task::init` creates anything -- is still exactly the one
/// page table this whole kernel has ever used): both are needed by
/// `new_address_space` to build every later process's private page tables.
pub fn install(
    mapper: OffsetPageTable<'static>,
    frame_allocator: BootInfoFrameAllocator,
    physical_memory_offset: VirtAddr,
) {
    let (kernel_pml4_frame, _) = x86_64::registers::control::Cr3::read();
    PHYS_MEM_OFFSET.store(physical_memory_offset.as_u64(), Ordering::Relaxed);
    KERNEL_PML4_FRAME.store(
        kernel_pml4_frame.start_address().as_u64(),
        Ordering::Relaxed,
    );

    with_paging(|mapper_slot, frame_allocator_slot| {
        *mapper_slot = Some(mapper);
        *frame_allocator_slot = Some(frame_allocator);
    });
}

/// The physical-memory offset established at boot (see `paging::init`) --
/// needed anywhere the kernel must turn a physical address into a
/// dereferenceable one, such as building an `OffsetPageTable` over a
/// process's own PML4 frame rather than the currently active one.
pub fn physical_memory_offset() -> Option<VirtAddr> {
    match PHYS_MEM_OFFSET.load(Ordering::Relaxed) {
        0 => None,
        raw => Some(VirtAddr::new(raw)),
    }
}

/// The kernel's own PML4 frame -- the address space every kernel-only task
/// (shell, idle, heartbeat) runs under, and what the scheduler loads into
/// CR3 whenever the current task isn't a user process (see `task.rs`).
pub fn kernel_pml4_frame() -> Option<PhysFrame> {
    match KERNEL_PML4_FRAME.load(Ordering::Relaxed) {
        0 => None,
        raw => Some(PhysFrame::containing_address(PhysAddr::new(raw))),
    }
}

/// Whether `install` has run. Diagnostics use this to report "paging not
/// active" instead of silently showing zeroes if heap init fell back to
/// the static array (see `memory::init_heap`).
pub fn is_active() -> bool {
    with_paging(|mapper_slot, _| mapper_slot.is_some())
}

/// Frame allocator statistics for `sysinfo`/`monitor`.
pub struct FrameStats {
    pub allocated: usize,
    pub free_in_pool: usize,
    /// See `BootInfoFrameAllocator::frames_bumped` -- the correct metric
    /// for "did anything just leak," immune to `allocated`'s
    /// double-counting of reused frames.
    pub bumped: usize,
}

pub fn frame_stats() -> Option<FrameStats> {
    with_paging(|_, frame_allocator_slot| {
        frame_allocator_slot.as_ref().map(|allocator| FrameStats {
            allocated: allocator.frames_allocated(),
            free_in_pool: allocator.frames_in_free_pool(),
            bumped: allocator.frames_bumped(),
        })
    })
}

/// A 9-bit PML4 index (bits 47:39) for `addr`.
const fn pml4_index(addr: u64) -> usize {
    ((addr >> 39) & 0x1FF) as usize
}

/// Base of the per-process private user address range. Every user
/// process's ELF segments and stack live somewhere in
/// `[USER_SPACE_BASE, USER_SPACE_BASE + USER_SPACE_SIZE)`. Chosen so the
/// whole range sits inside a single PML4 entry's 512 GiB span (comfortably
/// -- `USER_SPACE_SIZE` is 1 GiB), far from the bootloader's dynamic
/// kernel/physical-memory mappings and the heap (`0x_4444_4444_0000`).
pub const USER_SPACE_BASE: u64 = 0x_7000_0000_0000;
pub const USER_SPACE_SIZE: u64 = 0x_4000_0000;

/// The one PML4 slot every process's private page-table subtree lives
/// under. `new_address_space` leaves exactly this index empty when it
/// clones the kernel's other 511 entries, and `map_in_address_space`
/// refuses to map anything outside it -- together, that's what makes two
/// processes' private memory genuinely private from each other even when
/// both use the identical virtual address (see `new_address_space`'s docs).
const USER_REGION_PML4_INDEX: usize = pml4_index(USER_SPACE_BASE);

/// A process's private page-table root plus every physical frame it owns
/// (its own page-table subtree and every mapped leaf page), so
/// `free_address_space` can give it all back without walking the tree.
pub struct AddressSpace {
    pml4_frame: PhysFrame,
    owned_frames: Vec<PhysFrame<Size4KiB>>,
}

impl AddressSpace {
    pub fn pml4_frame(&self) -> PhysFrame {
        self.pml4_frame
    }

    /// Number of physical frames this address space currently owns --
    /// diagnostic use (`sysinfo`/`monitor`/isolation-test evidence), not
    /// load-bearing for correctness.
    pub fn frame_count(&self) -> usize {
        self.owned_frames.len()
    }
}

/// Build an `OffsetPageTable` over an arbitrary (not necessarily currently
/// active) PML4 frame, via the same physical-memory-offset technique
/// `paging::init` uses for the CPU's *active* table.
///
/// # Safety
/// `frame` must be a valid, live level-4 page table for the duration the
/// returned mapper is used, and nothing else may hold a live `&mut`
/// reference to it concurrently (single-threaded kernel, so this reduces to
/// "the caller doesn't stash a second one").
unsafe fn mapper_for(frame: PhysFrame, phys_offset: VirtAddr) -> OffsetPageTable<'static> {
    let virt = phys_offset + frame.start_address().as_u64();
    let table_ptr: *mut PageTable = virt.as_mut_ptr();
    // Safety: forwarded from this function's own contract.
    let table: &'static mut PageTable = unsafe { &mut *table_ptr };
    // Safety: `table` is a valid level-4 table per this function's contract,
    // and `phys_offset` is the same offset mapping used to reach it.
    unsafe { OffsetPageTable::new(table, phys_offset) }
}

/// Build a fresh, private address space for a user process.
///
/// Every kernel mapping (heap, kernel image, the physical-memory identity
/// window) is copied in verbatim -- the *same* physical subtree pointers
/// and the *same* flags the kernel's own table already has, which is what
/// "kernel mappings remain available to Ring 0" and "kernel pages must not
/// become USER_ACCESSIBLE" both mean concretely: Ring 0 code (interrupt
/// handlers, syscalls) keeps working correctly no matter which process's
/// CR3 happens to be loaded, and copying an entry cannot change its
/// `USER_ACCESSIBLE` bit, so those pages stay exactly as supervisor-only as
/// they always were. The one exception is `USER_REGION_PML4_INDEX`, left
/// completely empty (not merely unmapped -- the slot itself is absent) so
/// `map_in_address_space` builds a subtree there that belongs to nothing
/// else: this is the actual mechanism behind "one process cannot read or
/// write another's private memory," not a policy this module has to
/// enforce after the fact -- there is no shared page-table entry through
/// which it could happen.
pub fn new_address_space() -> Result<AddressSpace, &'static str> {
    let phys_offset = physical_memory_offset().ok_or("physical memory offset not set")?;

    with_paging(|mapper_slot, frame_allocator_slot| {
        let mapper = mapper_slot.as_mut().ok_or("paging not active")?;
        let frame_allocator = frame_allocator_slot
            .as_mut()
            .ok_or("frame allocator not active")?;

        let pml4_frame = frame_allocator
            .allocate_frame()
            .ok_or("out of physical memory frames")?;

        // Safety: `pml4_frame` was just allocated and is not yet referenced
        // by any page table or CR3, so this is an exclusive access; the
        // physical-memory offset mapping covers all usable RAM.
        let new_table: &mut PageTable = unsafe {
            let virt = phys_offset + pml4_frame.start_address().as_u64();
            &mut *virt.as_mut_ptr()
        };
        new_table.zero();

        let kernel_table = mapper.level_4_table();
        for i in 0..512 {
            if i == USER_REGION_PML4_INDEX {
                continue;
            }
            new_table[i] = kernel_table[i].clone();
        }

        Ok(AddressSpace {
            pml4_frame,
            owned_frames: vec![pml4_frame],
        })
    })
}

/// Thin `FrameAllocator` wrapper that records every frame it hands out into
/// `owned`, so a single `map_to` call -- which may itself allocate any
/// number of new P3/P2/P1 tables internally, opaquely, before it ever
/// reaches the leaf mapping -- still leaves `AddressSpace::owned_frames`
/// with a complete, exact record of everything to free later.
struct TrackingFrameAllocator<'a> {
    inner: &'a mut BootInfoFrameAllocator,
    owned: &'a mut Vec<PhysFrame<Size4KiB>>,
}

// Safety: delegates entirely to `inner`'s own already-`unsafe impl`
// guarantee (each frame handed out at most once until freed); recording the
// frame in `owned` afterward doesn't affect that.
unsafe impl FrameAllocator<Size4KiB> for TrackingFrameAllocator<'_> {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        let frame = self.inner.allocate_frame()?;
        self.owned.push(frame);
        Some(frame)
    }
}

/// Map one page into `space`'s private address space. Safe to call whether
/// or not `space` is the currently active CR3 -- correctness for a newly
/// mapped page comes from the CR3 reload the scheduler performs on every
/// switch into a process (`task.rs`), not from the `invlpg` `MapperFlush`
/// issues here (which, for a page under a CR3 that isn't loaded, is
/// architecturally harmless: at worst it evicts an unrelated stale TLB
/// entry).
///
/// Refuses to map anything outside `USER_SPACE_BASE..+USER_SPACE_SIZE` --
/// see `new_address_space`'s docs on why that boundary is what actually
/// keeps processes' private memory private, not just a policy check.
pub fn map_in_address_space(
    space: &mut AddressSpace,
    page: Page<Size4KiB>,
    flags: PageTableFlags,
) -> Result<(), &'static str> {
    if pml4_index(page.start_address().as_u64()) != USER_REGION_PML4_INDEX {
        return Err("refusing to map outside the process's private address-space region");
    }
    let phys_offset = physical_memory_offset().ok_or("physical memory offset not set")?;

    with_paging(|_, frame_allocator_slot| {
        let frame_allocator = frame_allocator_slot
            .as_mut()
            .ok_or("frame allocator not active")?;

        // Safety: `space.pml4_frame` was allocated by `new_address_space`
        // and is exclusively owned by `space`.
        let mut mapper = unsafe { mapper_for(space.pml4_frame, phys_offset) };
        let mut tracking = TrackingFrameAllocator {
            inner: frame_allocator,
            owned: &mut space.owned_frames,
        };

        let frame = tracking
            .allocate_frame()
            .ok_or("out of physical memory frames")?;

        // Safety: `frame` was just allocated exclusively for this mapping,
        // and `page` falls within `space`'s own private region (checked
        // above), never aliased by another address space's mappings.
        unsafe {
            mapper
                .map_to(page, frame, flags, &mut tracking)
                .map_err(|_| "page mapping failed: already mapped or invalid page table state")?
                .flush();
        }
        Ok(())
    })
}

/// Resolve `addr` within `space`'s own page tables -- regardless of whether
/// `space` is the currently active CR3 -- returning the mapped physical
/// address and the leaf entry's flags, or `None` if unmapped.
///
/// This is the primitive user-pointer validation (`syscall.rs`) is built
/// on: a Ring 3 pointer's numeric value is never trusted on its own, it
/// must first resolve to a real mapping in the *calling* process's own
/// tables, with the permissions the operation actually needs.
pub fn translate_in_address_space(
    space: &AddressSpace,
    addr: VirtAddr,
) -> Option<(PhysAddr, PageTableFlags)> {
    let phys_offset = physical_memory_offset()?;
    // Safety: `space.pml4_frame` is a valid level-4 table for as long as
    // `space` exists, which outlives this call.
    let mapper = unsafe { mapper_for(space.pml4_frame, phys_offset) };
    match mapper.translate(addr) {
        TranslateResult::Mapped {
            frame,
            offset,
            flags,
        } => Some((frame.start_address() + offset, flags)),
        TranslateResult::NotMapped | TranslateResult::InvalidFrameAddress(_) => None,
    }
}

/// Change the flags of an already-mapped page in `space`. Used by the ELF
/// loader (`elf.rs`) to map a segment `WRITABLE` just long enough to copy
/// its bytes in, then drop `WRITABLE` (or add `NO_EXECUTE`) to reach its
/// real, final permissions -- "user executable code: readable/executable,
/// not writable *after loading*" is implemented as exactly this two-step
/// sequence, not a claim.
pub fn update_flags_in_address_space(
    space: &AddressSpace,
    page: Page<Size4KiB>,
    flags: PageTableFlags,
) -> Result<(), &'static str> {
    let phys_offset = physical_memory_offset().ok_or("physical memory offset not set")?;
    // Safety: `space.pml4_frame` is a valid level-4 table for as long as
    // `space` exists.
    let mut mapper = unsafe { mapper_for(space.pml4_frame, phys_offset) };
    // Safety: `page` was previously mapped by `map_in_address_space` within
    // this same `space`; narrowing or changing its flags here cannot make
    // any *other* mapping unsound, since this table's user-region subtree
    // is exclusively owned by `space`.
    unsafe {
        mapper
            .update_flags(page, flags)
            .map_err(|_| "flag update failed: page not mapped")?
            .flush();
    }
    Ok(())
}

/// Walk `[start, start+len)` in `space`'s own mapped memory one page-chunk
/// at a time, handing `f` a kernel-writable pointer (via the
/// physical-memory-offset mapping) and a length for each chunk. Every byte
/// touched must already be mapped `WRITABLE` in `space` -- this is the
/// shared primitive behind `write_bytes_in_address_space` and
/// `zero_bytes_in_address_space`, used by the ELF loader to populate a
/// freshly mapped segment regardless of whether `space` is the currently
/// active CR3.
fn for_each_mapped_chunk(
    space: &AddressSpace,
    start: VirtAddr,
    len: u64,
    mut f: impl FnMut(*mut u8, usize),
) -> Result<(), &'static str> {
    let phys_offset = physical_memory_offset().ok_or("physical memory offset not set")?;
    let mut written = 0u64;
    while written < len {
        let addr = start + written;
        let (phys, flags) = translate_in_address_space(space, addr)
            .ok_or("destination page not mapped in this address space")?;
        if !flags.contains(PageTableFlags::WRITABLE) {
            return Err("destination page not writable");
        }
        let page_offset = addr.as_u64() % 4096;
        let chunk_len = (4096 - page_offset).min(len - written);
        let dst_ptr: *mut u8 = (phys_offset + phys.as_u64()).as_mut_ptr();
        // Safety: `phys` was just resolved from a `PRESENT | WRITABLE`
        // mapping in `space`'s own tables (checked above), and the
        // physical-memory offset mapping covers all usable RAM -- `dst_ptr`
        // is valid and writable for exactly `chunk_len` bytes starting
        // there, which is bounded to stay within this one 4 KiB frame.
        f(dst_ptr, chunk_len as usize);
        written += chunk_len;
    }
    Ok(())
}

/// Copy `data` into `space`'s own mapped memory at `dest`. Every byte
/// touched must already be mapped `WRITABLE` in `space` (see
/// `map_in_address_space`).
pub fn write_bytes_in_address_space(
    space: &AddressSpace,
    dest: VirtAddr,
    data: &[u8],
) -> Result<(), &'static str> {
    let mut src_offset = 0usize;
    for_each_mapped_chunk(space, dest, data.len() as u64, |dst_ptr, chunk_len| {
        // Safety: see `for_each_mapped_chunk`'s docs; `data[src_offset..]`
        // has at least `chunk_len` bytes remaining by construction (the
        // chunk walk never exceeds `data.len()` total).
        unsafe {
            core::ptr::copy_nonoverlapping(
                data[src_offset..src_offset + chunk_len].as_ptr(),
                dst_ptr,
                chunk_len,
            );
        }
        src_offset += chunk_len;
    })
}

/// Zero `len` bytes of `space`'s own mapped memory starting at `dest` --
/// used for a segment's BSS tail (`p_memsz > p_filesz`) and, just as
/// importantly, to guarantee a freshly allocated physical frame never
/// exposes a previous owner's leftover contents to a new process (frames
/// handed back to the allocator by `free_address_space` are not zeroed on
/// free, only reused ones would otherwise leak data on the *next*
/// allocation).
pub fn zero_bytes_in_address_space(
    space: &AddressSpace,
    dest: VirtAddr,
    len: u64,
) -> Result<(), &'static str> {
    for_each_mapped_chunk(space, dest, len, |dst_ptr, chunk_len| {
        // Safety: see `for_each_mapped_chunk`'s docs.
        unsafe {
            core::ptr::write_bytes(dst_ptr, 0, chunk_len);
        }
    })
}

/// Read `len` bytes from `space`'s own mapped memory starting at `src`.
///
/// This is the other half of user-pointer validation alongside
/// `translate_in_address_space` (which this is built on): `syscall.rs`'s
/// `WRITE` syscall calls this instead of ever dereferencing a Ring 3
/// pointer directly under the live CR3, precisely so a bad `ptr`/`len`
/// pair from user code becomes a clean `None` here -- never a Ring 0 page
/// fault from kernel code blindly trusting a user-supplied address (the
/// classic "unvalidated user pointer" kernel vulnerability class).
///
/// Every page touched must be `PRESENT | USER_ACCESSIBLE` (not necessarily
/// `WRITABLE` -- reading a process's own read-only code/rodata through
/// this path is legitimate). Returns `None`, without reading anything, if
/// any page in range is unmapped, not user-accessible, or if `src + len`
/// overflows.
pub fn read_bytes_from_address_space(
    space: &AddressSpace,
    src: VirtAddr,
    len: usize,
) -> Option<Vec<u8>> {
    let phys_offset = physical_memory_offset()?;
    src.as_u64().checked_add(len as u64)?;

    let mut out = Vec::with_capacity(len);
    let mut read = 0u64;
    while read < len as u64 {
        let addr = src + read;
        let (phys, flags) = translate_in_address_space(space, addr)?;
        if !flags.contains(PageTableFlags::USER_ACCESSIBLE) {
            return None;
        }
        let page_offset = addr.as_u64() % 4096;
        let chunk_len = (4096 - page_offset).min(len as u64 - read);
        let src_ptr: *const u8 = (phys_offset + phys.as_u64()).as_ptr();
        // Safety: `phys` was just resolved from a `PRESENT | USER_ACCESSIBLE`
        // mapping in `space`'s own tables (checked above), and the
        // physical-memory offset mapping covers all usable RAM -- valid for
        // exactly `chunk_len` bytes, bounded to stay within this one frame.
        unsafe {
            out.extend_from_slice(core::slice::from_raw_parts(src_ptr, chunk_len as usize));
        }
        read += chunk_len;
    }
    Some(out)
}

/// Load `frame` as the active CR3. Called on *every* scheduler switch
/// (`task.rs`), not just when the address space actually changes: reloading
/// CR3 to its current value is just a slightly wasteful full TLB flush,
/// which is a better trade than trusting a separately maintained "currently
/// loaded" cache to never drift out of sync with reality.
///
/// # Safety
/// `frame` must be a valid, fully populated PML4 -- the kernel's own root
/// (`kernel_pml4_frame`) or one built by `new_address_space` -- that stays
/// valid for as long as it remains loaded. In particular the caller must
/// not have freed it (`free_address_space`'s own safety contract exists
/// precisely to sequence this correctly: switch away first, free second).
pub unsafe fn switch_to(frame: PhysFrame) {
    use x86_64::registers::control::{Cr3, Cr3Flags};
    // Safety: forwarded from this function's own contract.
    unsafe {
        Cr3::write(frame, Cr3Flags::empty());
    }
}

/// Return every frame `space` owns -- its private page-table subtree and
/// all leaf (code/data/stack) pages -- to the global frame allocator.
///
/// # Safety
/// The caller must guarantee `space`'s PML4 frame is **not** the currently
/// loaded CR3. On this single-core kernel that means: the scheduler must
/// have already switched to a different address space before this runs.
/// Freeing frames CR3 still references would let them be handed out again
/// while still live in the active page tables -- silent corruption the
/// instant the new owner and the stale mapping collide.
pub unsafe fn free_address_space(space: AddressSpace) {
    with_paging(|_, frame_allocator_slot| {
        if let Some(frame_allocator) = frame_allocator_slot.as_mut() {
            for frame in space.owned_frames {
                // Safety: every frame here was allocated by this same
                // global allocator specifically for `space` (see
                // `new_address_space` / `map_in_address_space`'s
                // `TrackingFrameAllocator`), and this function's own
                // contract guarantees it is no longer live in any loaded
                // CR3.
                unsafe {
                    frame_allocator.deallocate_frame(frame);
                }
            }
        }
    });
}
