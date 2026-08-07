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

const PIXEL_FORMAT_RGB: u32 = 0;
const PIXEL_FORMAT_BGR: u32 = 1;
const PIXEL_FORMAT_U8: u32 = 2;
const PIXEL_FORMAT_UNKNOWN: u32 = 3;

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

    let ok = crate::task::with_current_address_space(|space| {
        let Some(fb_buffer) = crate::framebuffer_console::raw_buffer_mut() else {
            return false;
        };
        paging::read_bytes_from_address_space_into(
            space,
            x86_64::VirtAddr::new(user_ptr),
            fb_buffer,
        )
    });

    match ok {
        Some(true) => Ok(()),
        Some(false) => Err("invalid source buffer"),
        None => Err("current task is not a user process"),
    }
}
