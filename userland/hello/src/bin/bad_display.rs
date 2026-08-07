//! `bad_display`: exercises `SYS_DISPLAY_INFO`/`SYS_DISPLAY_PRESENT`'s
//! validation (Phase 5, Milestone 3). Proves the kernel rejects a
//! kernel-address destination for `DISPLAY_INFO`, and rejects
//! `DISPLAY_PRESENT` buffers that are the wrong size or point at memory
//! this process never actually mapped -- never a Ring 0 fault from blindly
//! trusting a Ring 3 pointer or length.

#![no_std]
#![no_main]

use core::arch::global_asm;

use hello_user::{syscall, write, SYS_DISPLAY_INFO, SYS_DISPLAY_PRESENT, SYS_EXIT, SYS_MMAP};

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

/// Deliberately not page-aligned / never mapped -- a plausible-looking
/// pointer this process does not actually own.
const UNMAPPED_PTR: u64 = 0x_7000_2000_0000;

extern "C" fn rust_main() -> ! {
    let mut ok = true;

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

    write(b"bad_display: DISPLAY_PRESENT with an unmapped source buffer\n");
    // Safety: deliberately invalid source; must be rejected before any
    // framebuffer byte is touched.
    let unmapped_result = unsafe { syscall(SYS_DISPLAY_PRESENT, UNMAPPED_PTR, 8_294_400, 0) };
    if unmapped_result < 0 {
        write(b"bad_display: unmapped DISPLAY_PRESENT source rejected -- OK\n");
    } else {
        write(b"bad_display: UNEXPECTED -- unmapped source was accepted\n");
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
