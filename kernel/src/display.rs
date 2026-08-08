//! Kernel display abstraction (Phase 5).
//!
//! Turns the raw framebuffer `framebuffer_console.rs` already owns (and
//! still uses for the boot banner and text shell) into the clean interface
//! a Ring 3 desktop process actually needs: a fixed-layout description of
//! the display (`DisplayInfo`) and one controlled operation
//! (`present`) that copies a *validated* userspace pixel buffer into the
//! real, kernel-owned framebuffer memory.
//!
//! Userspace never receives a pointer to the real framebuffer, unrestricted
//! or otherwise -- it renders into its own memory (mapped via `SYS_MMAP`)
//! and submits the finished frame through `SYS_DISPLAY_PRESENT`, which is
//! the only path that ever writes into the real screen memory on a
//! process's behalf. This mirrors "userspace renders into its own buffer,
//! then a controlled syscall presents it," not "hand userspace the
//! framebuffer and hope."

use crate::paging;
use core::sync::atomic::{AtomicU64, Ordering};

const PIXEL_FORMAT_RGB: u32 = 0;
const PIXEL_FORMAT_BGR: u32 = 1;
const PIXEL_FORMAT_U8: u32 = 2;
const PIXEL_FORMAT_UNKNOWN: u32 = 3;
static PRESENT_COUNT: AtomicU64 = AtomicU64::new(0);
static PRESENT_TOTAL_CYCLES: AtomicU64 = AtomicU64::new(0);
static PRESENT_MAX_CYCLES: AtomicU64 = AtomicU64::new(0);
const PRESENT_COPY_CHUNK: usize = 64 * 1024;

/// Fixed 20-byte wire format for `SYS_DISPLAY_INFO` -- five little-endian
/// `u32`s, in this exact order. `stride` is in *pixels*, not bytes (matches
/// `bootloader_api::info::FrameBufferInfo::stride`, which can exceed
/// `width` for row-padding reasons); a renderer computes a byte offset as
/// `(y * stride + x) * bytes_per_pixel`.
pub struct DisplayInfo {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub bytes_per_pixel: u32,
    pub pixel_format: u32,
}

impl DisplayInfo {
    pub fn to_le_bytes(&self) -> [u8; 20] {
        let mut out = [0u8; 20];
        out[0..4].copy_from_slice(&self.width.to_le_bytes());
        out[4..8].copy_from_slice(&self.height.to_le_bytes());
        out[8..12].copy_from_slice(&self.stride.to_le_bytes());
        out[12..16].copy_from_slice(&self.bytes_per_pixel.to_le_bytes());
        out[16..20].copy_from_slice(&self.pixel_format.to_le_bytes());
        out
    }
}

/// Current display info, or `None` if no framebuffer is active (VGA-text
/// fallback boot -- see `main.rs`; a desktop process cannot run in that
/// configuration, which `DISPLAY_INFO`/`DISPLAY_PRESENT` both report as a
/// clean failure rather than a fault).
pub fn info() -> Option<DisplayInfo> {
    let fb = crate::framebuffer_console::raw_info()?;
    let pixel_format = match fb.pixel_format {
        bootloader_api::info::PixelFormat::Rgb => PIXEL_FORMAT_RGB,
        bootloader_api::info::PixelFormat::Bgr => PIXEL_FORMAT_BGR,
        bootloader_api::info::PixelFormat::U8 => PIXEL_FORMAT_U8,
        _ => PIXEL_FORMAT_UNKNOWN,
    };
    Some(DisplayInfo {
        width: fb.width as u32,
        height: fb.height as u32,
        stride: fb.stride as u32,
        bytes_per_pixel: fb.bytes_per_pixel as u32,
        pixel_format,
    })
}

/// Deterministic checksum of the complete physical framebuffer, used by the
/// Phase 5 acceptance harness to prove rejected `DISPLAY_PRESENT` calls make
/// zero destination changes. This is 64-bit FNV-1a: compact and stable, not a
/// cryptographic integrity primitive.
pub fn framebuffer_checksum() -> Option<u64> {
    const FNV_OFFSET_BASIS: u64 = 0xCBF2_9CE4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01B3;

    let buffer = crate::framebuffer_console::raw_buffer_mut()?;
    let mut hash = FNV_OFFSET_BASIS;
    for byte in buffer.iter() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    Some(hash)
}

/// `SYS_DISPLAY_PRESENT`'s implementation: copy the *currently running*
/// process's own validated buffer at `user_ptr` into the real framebuffer.
///
/// Validation, all before a single byte is copied:
/// - a display must actually be active (`info()` succeeds);
/// - `user_len` must exactly equal the real framebuffer's byte length
///   (`FrameBufferInfo::byte_len`) -- not "at least," exactly, so a
///   mismatched buffer is always rejected outright rather than silently
///   truncated or read out of bounds;
/// - every page of `[user_ptr, user_ptr+user_len)` in the calling
///   process's own address space must be mapped `PRESENT |
///   USER_ACCESSIBLE`, using the same checked full-range validator as the
///   other pointer-bearing syscalls.
///
/// Once preflight succeeds, the caller's mapping is immutable for this
/// single-threaded process. The copy proceeds in bounded chunks with an IRQ
/// delivery window between chunks; a preempted task resumes under its own CR3
/// before copying the next chunk. The syscall layer permits only the foreground
/// process to present, so another process cannot interleave a competing frame.
///
/// A process's own `SYS_MMAP`'d buffer is never the real framebuffer and
/// is never touched by any *other* process's address space (Phase 4's
/// per-process isolation applies here unchanged), so there is no path from
/// this syscall to reading or corrupting anything outside the caller's own
/// memory and the one destination it's explicitly allowed to write:
/// the screen.
pub fn present(user_ptr: u64, user_len: usize) -> Result<(), &'static str> {
    let fb_info = crate::framebuffer_console::raw_info().ok_or("display not active")?;
    if user_len != fb_info.byte_len {
        return Err("buffer size does not match display dimensions");
    }
    let user_addr = paging::checked_user_virt_addr(user_ptr)?;

    // `RDTSC` is diagnostic only. Account active validation/copy critical
    // sections, excluding time another task may run in an inter-chunk window.
    let validation_started = unsafe { core::arch::x86_64::_rdtsc() };
    if !crate::task::validate_current_user_range(user_ptr, user_len, false) {
        return Err("invalid source buffer");
    }
    let validation_cycles =
        unsafe { core::arch::x86_64::_rdtsc() }.saturating_sub(validation_started);

    let fb_buffer = crate::framebuffer_console::raw_buffer_mut().ok_or("display not active")?;
    let destination = fb_buffer.as_mut_ptr();
    // End the slice borrow before opening any interrupt window. The raw
    // framebuffer pointer is stable for the machine's lifetime.
    let _ = fb_buffer;
    // Safety: the complete source was just validated PRESENT and
    // USER_ACCESSIBLE in the caller's address space. The source and the
    // disjoint kernel framebuffer are valid for exactly `user_len` bytes.
    let (copy_total_cycles, copy_max_cycles) =
        unsafe { copy_framebuffer_bytes_batched(user_addr.as_ptr::<u8>(), destination, user_len) };

    let total_cycles = validation_cycles.saturating_add(copy_total_cycles);
    let max_cycles = validation_cycles.max(copy_max_cycles);
    PRESENT_COUNT.fetch_add(1, Ordering::Relaxed);
    PRESENT_TOTAL_CYCLES.fetch_add(total_cycles, Ordering::Relaxed);
    update_max(&PRESENT_MAX_CYCLES, max_cycles);
    Ok(())
}

/// Copy one already-validated, non-overlapping frame in bounded critical
/// sections. Each chunk uses the x86 string engine; an `sti; nop; cli` window
/// between chunks lets a pending timer/input IRQ run without extending any
/// single interrupt-disabled copy across the full frame.
///
/// # Safety
///
/// `source` and `destination` must be valid for `len` bytes and must not
/// overlap. The caller establishes those conditions immediately above.
unsafe fn copy_framebuffer_bytes_batched(
    source: *const u8,
    destination: *mut u8,
    len: usize,
) -> (u64, u64) {
    let mut copied = 0usize;
    let mut total_cycles = 0u64;
    let mut max_cycles = 0u64;
    while copied < len {
        let chunk_len = PRESENT_COPY_CHUNK.min(len - copied);
        let qwords = chunk_len / core::mem::size_of::<u64>();
        let tail = chunk_len % core::mem::size_of::<u64>();
        let started = unsafe { core::arch::x86_64::_rdtsc() };
        // Safety: the caller validated the whole frame and this chunk is
        // bounded within it. CLD makes forward progress explicit.
        unsafe {
            core::arch::asm!(
                "cld",
                "rep movsq",
                "mov rcx, rdx",
                "rep movsb",
                inout("rsi") source.add(copied) => _,
                inout("rdi") destination.add(copied) => _,
                inout("rcx") qwords => _,
                in("rdx") tail,
                options(nostack),
            );
        }
        let cycles = unsafe { core::arch::x86_64::_rdtsc() }.saturating_sub(started);
        total_cycles = total_cycles.saturating_add(cycles);
        max_cycles = max_cycles.max(cycles);
        copied += chunk_len;

        if copied < len {
            // Safety: DISPLAY_PRESENT enters through an interrupt gate with IF
            // clear and holds no scheduler/display lock here. If a pending IRQ
            // preempts us after STI, the scheduler restores this task's CR3
            // before the NOP/CLI continuation executes.
            unsafe { core::arch::asm!("sti", "nop", "cli", options(nostack)) };
        }
    }
    (total_cycles, max_cycles)
}

#[derive(Clone, Copy)]
pub struct PresentTelemetry {
    pub count: u64,
    pub total_cycles: u64,
    pub max_cycles: u64,
}

pub fn present_telemetry() -> PresentTelemetry {
    PresentTelemetry {
        count: PRESENT_COUNT.load(Ordering::Relaxed),
        total_cycles: PRESENT_TOTAL_CYCLES.load(Ordering::Relaxed),
        max_cycles: PRESENT_MAX_CYCLES.load(Ordering::Relaxed),
    }
}

fn update_max(target: &AtomicU64, value: u64) {
    let mut current = target.load(Ordering::Relaxed);
    while value > current {
        match target.compare_exchange_weak(current, value, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(observed) => current = observed,
        }
    }
}
