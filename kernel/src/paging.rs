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

use alloc::vec::Vec;

use bootloader_api::info::{MemoryRegionKind, MemoryRegions};
use spin::Mutex;
use x86_64::structures::paging::{
    FrameAllocator, FrameDeallocator, Mapper, OffsetPageTable, Page, PageTable, PageTableFlags,
    PhysFrame, Size4KiB,
};
use x86_64::{PhysAddr, VirtAddr};

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
    /// counter, distinct from currently-in-use frames.
    pub fn frames_allocated(&self) -> usize {
        self.allocated_count
    }

    /// Frames that were freed and are waiting to be reused.
    pub fn frames_in_free_pool(&self) -> usize {
        self.freed.len()
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

/// Install the mapper and frame allocator built during heap setup as the
/// kernel-wide instances used by diagnostics and (from later phases) new
/// mappings outside the heap.
pub fn install(mapper: OffsetPageTable<'static>, frame_allocator: BootInfoFrameAllocator) {
    with_paging(|mapper_slot, frame_allocator_slot| {
        *mapper_slot = Some(mapper);
        *frame_allocator_slot = Some(frame_allocator);
    });
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
}

pub fn frame_stats() -> Option<FrameStats> {
    with_paging(|_, frame_allocator_slot| {
        frame_allocator_slot.as_ref().map(|allocator| FrameStats {
            allocated: allocator.frames_allocated(),
            free_in_pool: allocator.frames_in_free_pool(),
        })
    })
}
