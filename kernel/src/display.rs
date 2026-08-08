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
///   USER_ACCESSIBLE` (checked arithmetic throughout, via
///   `paging::read_bytes_from_address_space_into` -- the same validated,
///   chunk-at-a-time primitive `WRITE`'s pointer validation is built on,
///   just copying straight into the destination instead of an intermediate
///   `Vec` for this call's much larger, once-per-frame payload).
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

    // `RDTSC` is diagnostic only: it measures the copy cost without adding a
    // timer interrupt, lock, allocation, or scheduler-policy dependency.
    let started = unsafe { core::arch::x86_64::_rdtsc() };
    let ok = crate::task::with_current_address_space(|space| {
        // Full-range preflight happens before even borrowing the mutable
        // framebuffer slice. `read_bytes_from_address_space_into` repeats the
        // check defensively before its first copy, so a later unmapped or
        // supervisor page can never produce a partially presented frame.
        if paging::validate_user_range(space, user_ptr, user_len, false).is_err() {
            return false;
        }
        let Some(fb_buffer) = crate::framebuffer_console::raw_buffer_mut() else {
            return false;
        };
        paging::read_bytes_from_address_space_into(space, user_addr, fb_buffer)
    });

    match ok {
        Some(true) => {
            let cycles = unsafe { core::arch::x86_64::_rdtsc() }.saturating_sub(started);
            PRESENT_COUNT.fetch_add(1, Ordering::Relaxed);
            PRESENT_TOTAL_CYCLES.fetch_add(cycles, Ordering::Relaxed);
            update_max(&PRESENT_MAX_CYCLES, cycles);
            Ok(())
        }
        Some(false) => Err("invalid source buffer"),
        None => Err("current task is not a user process"),
    }
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
