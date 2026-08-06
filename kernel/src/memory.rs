//! Kernel heap setup, now backed by real mapped pages (Phase 2).
//!
//! ## Stack vs heap
//!
//! - **Stack** — automatic storage for local variables and function call frames.
//!   Size is known at compile time and reclaimed when a function returns.
//! - **Heap** — dynamic storage requested at runtime through an **allocator**.
//!   A `Box`, `Vec`, or `String` lives on the heap until it is dropped.
//!
//! ## Why `no_std` needs `alloc`
//!
//! `#![no_std]` removes the standard library, including its OS-backed allocator.
//! The separate `alloc` crate still provides heap types, but **we** must supply
//! memory and implement [`GlobalAlloc`](core::alloc::GlobalAlloc) — see
//! [`crate::allocator`].
//!
//! ## From a static array to real paging
//!
//! Phase 1 (and all of v0.5) backed the heap with a static array baked into
//! `.bss`, specifically so the kernel never had to treat a raw physical
//! address from the bootloader's memory map as a dereferenceable pointer --
//! doing so would corrupt memory if the bootloader hadn't mapped that
//! physical range. That constraint is what this module now removes: with
//! `physical_memory_offset` mapped (see `BOOTLOADER_CONFIG` in `main.rs`),
//! `paging::init` gives the kernel a real `OffsetPageTable`, and the heap is
//! a genuine range of virtual pages backed by frames from
//! `paging::BootInfoFrameAllocator`, exactly like any other mapping the
//! kernel could now create (user-process memory, MMIO, and so on, in later
//! phases).
//!
//! If, for any reason, the bootloader did not provide a physical memory
//! offset (misconfiguration, or a future bootloader upgrade), heap setup
//! falls back to the old static array rather than failing to boot at all --
//! a degraded heap beats a kernel that can't start.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use bootloader_api::info::MemoryRegions;
use x86_64::structures::paging::{Page, PageTableFlags};
use x86_64::VirtAddr;

use crate::allocator;
use crate::paging;

/// Chosen to sit well clear of the bootloader's dynamic kernel/stack/
/// framebuffer/physical-memory mappings, which is the same address the
/// wider Rust OS-dev community's paging tutorials for this exact
/// `bootloader_api` version use for the same reason.
const HEAP_START: usize = 0x_4444_4444_0000;

/// 4 MiB: no longer constrained by what fits in a compile-time `.bss`
/// array, so this is larger than v0.5/Phase 1's 1 MiB static heap.
pub const HEAP_SIZE: usize = 4 * 1024 * 1024;

/// Backing storage for the fallback path only (see module docs). Sized to
/// match `HEAP_SIZE` so the fallback is a real equivalent-capacity heap,
/// not a silent downgrade.
#[repr(align(4096))]
struct HeapStorage([u8; HEAP_SIZE]);

static mut HEAP_STORAGE: HeapStorage = HeapStorage([0; HEAP_SIZE]);

/// Set up the kernel heap before the shell or any `Box`/`Vec`/`String` is used.
///
/// Takes the two specific `BootInfo` fields it needs (rather than the whole
/// struct by `'static` reference) so the caller's later, disjoint use of
/// `boot_info.framebuffer` is unaffected -- borrowing all of `BootInfo` for
/// `'static` here would otherwise conflict with that later mutable borrow.
pub fn init_heap(physical_memory_offset: Option<u64>, memory_regions: &'static MemoryRegions) {
    let Some(physical_memory_offset) = physical_memory_offset else {
        crate::serial_println!(
            "memory: bootloader did not provide physical_memory_offset -- falling back to the static-array heap"
        );
        fallback_static_heap();
        return;
    };

    // Safety: BOOTLOADER_CONFIG (main.rs) requests the physical memory
    // mapping, so `physical_memory_offset` is genuinely valid; this is the
    // only call to `paging::init` in the kernel.
    let mut mapper = unsafe { paging::init(VirtAddr::new(physical_memory_offset)) };
    // Safety: `memory_regions` is the bootloader's own report of which
    // physical ranges are free RAM.
    let mut frame_allocator = unsafe { paging::BootInfoFrameAllocator::init(memory_regions) };

    let page_range = {
        let heap_start = VirtAddr::new(HEAP_START as u64);
        let heap_end = heap_start + (HEAP_SIZE - 1) as u64;
        Page::range_inclusive(
            Page::containing_address(heap_start),
            Page::containing_address(heap_end),
        )
    };

    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
    for page in page_range {
        if let Err(reason) = paging::map_page(&mut mapper, &mut frame_allocator, page, flags) {
            crate::serial_println!(
                "memory: heap page mapping failed ({}) -- falling back to the static-array heap",
                reason
            );
            fallback_static_heap();
            return;
        }
    }

    paging::install(
        mapper,
        frame_allocator,
        VirtAddr::new(physical_memory_offset),
    );
    allocator::init(HEAP_START, HEAP_SIZE);
}

/// Degraded-but-working heap for when real paging can't be set up. Same
/// size as the paged heap so callers see no difference beyond `sysinfo`
/// reporting paging as inactive.
fn fallback_static_heap() {
    // Safety: this runs at most once, before any other code has read or
    // written HEAP_STORAGE, and only from this single-threaded boot path.
    let start = unsafe { core::ptr::addr_of_mut!(HEAP_STORAGE.0) as *mut u8 as usize };
    allocator::init(start, HEAP_SIZE);
}

/// Run a small allocation smoke test using `Box`, `Vec`, and `String`.
pub fn memtest() -> Result<(), &'static str> {
    if !allocator::is_initialized() {
        return Err("heap not initialized");
    }

    // Box — single value on the heap.
    let value = Box::new(42u64);
    if *value != 42 {
        return Err("Box value mismatch");
    }
    drop(value);

    // Vec — growable heap buffer.
    let mut numbers = Vec::new();
    for index in 0..128 {
        numbers.push(index);
    }
    if numbers.len() != 128 || numbers[64] != 64 {
        return Err("Vec push/read failed");
    }
    drop(numbers);

    // String — UTF-8 text on the heap.
    let mut label = String::from("TuwaiqOS");
    label.push(' ');
    label.push_str("heap");
    if label != "TuwaiqOS heap" {
        return Err("String content mismatch");
    }

    Ok(())
}
