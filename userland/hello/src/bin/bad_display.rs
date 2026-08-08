//! `bad_display`: exercises `SYS_DISPLAY_INFO`/`SYS_DISPLAY_PRESENT`'s
//! validation (Phase 5, Milestone 3). Proves the kernel rejects a
//! kernel-address destination for `DISPLAY_INFO`, and rejects
//! `DISPLAY_PRESENT` buffers that are the wrong size or point at memory
//! this process never actually mapped -- never a Ring 0 fault from blindly
//! trusting a Ring 3 pointer or length.

#![no_std]
#![no_main]

use core::arch::global_asm;

use hello_user::{
    syscall, write, SYS_DISPLAY_INFO, SYS_DISPLAY_PRESENT, SYS_EXIT, SYS_MMAP, SYS_MUNMAP,
};

global_asm!(
    r#"
.global _start
_start:
    call {main}
1:
    jmp 1b
"#,
    main = sym rust_main,
);

/// The kernel heap's fixed base -- real kernel memory, never part of this
/// process's own mapped user region (same constant `bad_pointer` uses).
const KERNEL_HEAP_ADDR: u64 = 0x_4444_4444_0000;
const NON_CANONICAL_ADDR: u64 = 0x0001_0000_0000_0000;

/// Deliberately not page-aligned / never mapped -- a plausible-looking
/// pointer this process does not actually own.
const UNMAPPED_PTR: u64 = 0x_7000_2000_0000;

fn read_u32(bytes: &[u8; 20], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

extern "C" fn rust_main() -> ! {
    let mut ok = true;
    let mut info = [0u8; 20];

    write(b"bad_display: querying the real display byte length\n");
    let query_result = unsafe {
        syscall(
            SYS_DISPLAY_INFO,
            info.as_mut_ptr() as u64,
            info.len() as u64,
            0,
        )
    };
    let exact_len = if query_result == 0 {
        let height = u64::from(read_u32(&info, 4));
        let stride = u64::from(read_u32(&info, 8));
        let bytes_per_pixel = u64::from(read_u32(&info, 12));
        height
            .checked_mul(stride)
            .and_then(|pixels| pixels.checked_mul(bytes_per_pixel))
            .unwrap_or(0)
    } else {
        0
    };
    if exact_len == 0 {
        write(b"bad_display: UNEXPECTED -- could not obtain valid display dimensions\n");
        ok = false;
    } else {
        write(b"bad_display: exact framebuffer length obtained -- OK\n");
    }

    write(b"bad_display: DISPLAY_INFO into a kernel-address destination\n");
    // Safety: deliberately invalid destination; must be rejected by the
    // same USER_ACCESSIBLE page-walk validation `SYS_WRITE` uses, before
    // any byte is written.
    let info_result = unsafe { syscall(SYS_DISPLAY_INFO, KERNEL_HEAP_ADDR, 20, 0) };
    if info_result < 0 {
        write(b"bad_display: kernel-address DISPLAY_INFO destination rejected -- OK\n");
    } else {
        write(b"bad_display: UNEXPECTED -- kernel-address destination was accepted\n");
        ok = false;
    }

    write(b"bad_display: DISPLAY_INFO into a non-canonical destination\n");
    let info_result = unsafe { syscall(SYS_DISPLAY_INFO, NON_CANONICAL_ADDR, 20, 0) };
    if info_result < 0 {
        write(b"bad_display: non-canonical DISPLAY_INFO destination rejected -- OK\n");
    } else {
        write(b"bad_display: UNEXPECTED -- non-canonical destination was accepted\n");
        ok = false;
    }

    write(b"bad_display: DISPLAY_PRESENT with an unmapped source buffer\n");
    // Safety: deliberately invalid source; must be rejected before any
    // framebuffer byte is touched.
    let unmapped_result = unsafe { syscall(SYS_DISPLAY_PRESENT, UNMAPPED_PTR, exact_len, 0) };
    if unmapped_result < 0 {
        write(b"bad_display: unmapped DISPLAY_PRESENT source rejected -- OK\n");
    } else {
        write(b"bad_display: UNEXPECTED -- unmapped source was accepted\n");
        ok = false;
    }

    write(b"bad_display: DISPLAY_PRESENT with a non-canonical source\n");
    let noncanonical_result =
        unsafe { syscall(SYS_DISPLAY_PRESENT, NON_CANONICAL_ADDR, exact_len, 0) };
    if noncanonical_result < 0 {
        write(b"bad_display: non-canonical DISPLAY_PRESENT source rejected -- OK\n");
    } else {
        write(b"bad_display: UNEXPECTED -- non-canonical source was accepted\n");
        ok = false;
    }

    write(b"bad_display: DISPLAY_PRESENT with a real but wrong-sized buffer\n");
    // Safety: a genuinely mapped, genuinely writable page -- but its
    // length (4096) will not match the real framebuffer's byte length for
    // any display mode this kernel boots with, so DISPLAY_PRESENT's exact
    // size-match check must reject it regardless of the pointer being
    // otherwise perfectly valid.
    let mapped = unsafe { syscall(SYS_MMAP, 4096, 1, 0) };
    if mapped >= 0 {
        let wrong_size_result = unsafe { syscall(SYS_DISPLAY_PRESENT, mapped as u64, 4096, 0) };
        if wrong_size_result < 0 {
            write(b"bad_display: wrong-sized DISPLAY_PRESENT buffer rejected -- OK\n");
        } else {
            write(b"bad_display: UNEXPECTED -- wrong-sized buffer was accepted\n");
            ok = false;
        }

        write(b"bad_display: DISPLAY_INFO range crossing into an unmapped page\n");
        let cross_info_result = unsafe { syscall(SYS_DISPLAY_INFO, mapped as u64 + 4090, 20, 0) };
        if cross_info_result < 0 {
            write(b"bad_display: cross-page DISPLAY_INFO destination rejected -- OK\n");
        } else {
            write(b"bad_display: UNEXPECTED -- cross-page destination was accepted\n");
            ok = false;
        }

        // Make the first page visibly destructive if an implementation starts
        // copying before discovering the unmapped second page. Acceptance
        // compares the framebuffer before/after this call; the syscall itself
        // must return failure and the screen must remain byte-identical.
        unsafe {
            core::ptr::write_bytes(mapped as u64 as *mut u8, 0xA5, 4096);
        }
        write(b"bad_display: partially mapped exact-size source (atomicity test)\n");
        let partial_result = unsafe { syscall(SYS_DISPLAY_PRESENT, mapped as u64, exact_len, 0) };
        if partial_result < 0 {
            write(b"bad_display: partial source rejected; verify framebuffer unchanged -- OK\n");
        } else {
            write(b"bad_display: UNEXPECTED -- partially mapped source was accepted\n");
            ok = false;
        }

        let _ = unsafe { syscall(SYS_MUNMAP, mapped as u64, 4096, 0) };
    } else {
        write(b"bad_display: could not mmap a scratch buffer to test with\n");
        ok = false;
    }

    // Safety: exit code reflects whether every expectation above held.
    unsafe {
        syscall(SYS_EXIT, if ok { 0 } else { 1 }, 0, 0);
    }
    loop {}
}
