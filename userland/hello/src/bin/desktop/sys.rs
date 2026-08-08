//! Thin syscall wrappers for the desktop, built on `hello_user::syscall`
//! (see that crate for the ABI: `int 0x80`, RAX = number in / return value
//! out, RDI/RSI/RDX = args 1-3). Kept separate from `hello_user` itself
//! since these are desktop-specific conveniences (typed `DisplayInfo`,
//! fixed-size event decoding), not part of the shared test-binary ABI
//! surface.

use hello_user::{
    syscall, SYS_DISPLAY_INFO, SYS_DISPLAY_PRESENT, SYS_EXIT, SYS_INPUT_POLL, SYS_MMAP, SYS_MUNMAP,
    SYS_SPAWN, SYS_UPTIME_TICKS, SYS_YIELD,
};

pub const KEY_ESCAPE: u8 = 0x1B;

pub const PIXEL_FORMAT_BGR: u32 = 1;
pub const PIXEL_FORMAT_U8: u32 = 2;

#[derive(Clone, Copy)]
pub struct DisplayInfo {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub bytes_per_pixel: u32,
    pub pixel_format: u32,
}

impl DisplayInfo {
    /// Total backbuffer size for this mode, matching exactly what the
    /// kernel's `display::present` requires (`fb_info.byte_len`, which is
    /// `stride * height * bytes_per_pixel` for every pixel format this
    /// bootloader produces -- see `framebuffer_console.rs`'s own row-stride
    /// arithmetic on the kernel side).
    pub fn buffer_len(&self) -> u64 {
        (self.stride as u64) * (self.height as u64) * (self.bytes_per_pixel as u64)
    }
}

/// `SYS_DISPLAY_INFO`: `None` if no display is active (VGA-text boot) or
/// the syscall otherwise rejected the request.
pub fn display_info() -> Option<DisplayInfo> {
    let mut buf = [0u8; 20];
    // Safety: `buf` is a valid, appropriately sized stack buffer for the
    // duration of this call.
    let result = unsafe { syscall(SYS_DISPLAY_INFO, buf.as_mut_ptr() as u64, 20, 0) };
    if result < 0 {
        return None;
    }
    Some(DisplayInfo {
        width: u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]),
        height: u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]),
        stride: u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]),
        bytes_per_pixel: u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]),
        pixel_format: u32::from_le_bytes([buf[16], buf[17], buf[18], buf[19]]),
    })
}

/// `SYS_MMAP`: `None` on rejection (bad length, arena exhausted).
pub fn mmap(len: u64, writable: bool) -> Option<u64> {
    // Safety: no pointers involved on this side of the call -- `len` and
    // `writable` are plain integers the kernel validates itself.
    let result = unsafe { syscall(SYS_MMAP, len, writable as u64, 0) };
    if result < 0 {
        None
    } else {
        Some(result as u64)
    }
}

/// `SYS_MUNMAP`: release a page-aligned mapping range before normal exit.
pub fn munmap(ptr: u64, len: u64) -> bool {
    // Safety: plain integers; the kernel validates alignment, ownership,
    // bounds, and that the complete range is mapped before changing it.
    unsafe { syscall(SYS_MUNMAP, ptr, len, 0) == 0 }
}

/// `SYS_DISPLAY_PRESENT`: `true` if the frame was accepted and copied to
/// the real framebuffer.
///
/// # Safety
/// `ptr` must point to at least `len` readable bytes this process actually
/// owns (in practice, the exact buffer `mmap` returned, presented at
/// exactly its full length) -- the kernel independently validates this
/// too, but the caller is still asserting a true statement about its own
/// memory, not just hoping the kernel catches a lie.
pub unsafe fn display_present(ptr: u64, len: u64) -> bool {
    // Safety: forwarded from this function's own contract.
    unsafe { syscall(SYS_DISPLAY_PRESENT, ptr, len, 0) == 0 }
}

/// `SYS_INPUT_POLL`: fills `buf` with the next queued input event and
/// returns `true`, or returns `false` if the queue was empty (or the call
/// was otherwise rejected -- an 8-byte stack buffer is always valid, so in
/// practice this only ever means "no event").
pub fn input_poll(buf: &mut [u8; 8]) -> bool {
    // Safety: `buf` is a valid, exactly-sized stack buffer for the
    // duration of this call.
    let result = unsafe { syscall(SYS_INPUT_POLL, buf.as_mut_ptr() as u64, 8, 0) };
    result == 1
}

/// `SYS_UPTIME_TICKS`: raw timer tick count since boot (100 Hz -- see
/// `interrupts::TIMER_HZ` on the kernel side; this is the one place that
/// frequency is duplicated as an assumption rather than queried, since
/// there is no syscall exposing it and it has been a fixed constant since
/// Phase 1).
pub fn uptime_ticks() -> u64 {
    // Safety: no arguments.
    unsafe { syscall(SYS_UPTIME_TICKS, 0, 0, 0).max(0) as u64 }
}

pub fn yield_now() {
    // Safety: no arguments.
    unsafe {
        syscall(SYS_YIELD, 0, 0, 0);
    }
}

/// Launch a filesystem-backed Ring-3 process. This preview uses it only for
/// `/apps/tuwaiq-ai`; the kernel still validates and parses the path and ELF.
pub fn spawn(path: &[u8]) -> Option<u32> {
    let result = unsafe { syscall(SYS_SPAWN, path.as_ptr() as u64, path.len() as u64, 0) };
    u32::try_from(result).ok()
}

pub fn exit(code: i64) -> ! {
    // Safety: exit code only, no pointers; never returns.
    unsafe {
        syscall(SYS_EXIT, code as u64, 0, 0);
    }
    loop {}
}
