//! `bad_input`: exercises `SYS_INPUT_POLL`'s validation (Phase 5,
//! Milestone 5). Proves an undersized destination buffer and a
//! kernel-address destination are both rejected cleanly -- and, as a
//! positive control, that a legitimately sized buffer on this process's
//! own stack is accepted (returning `0` or `1`, never `-1`) even with no
//! events queued.

#![no_std]
#![no_main]

use core::arch::global_asm;

use hello_user::{syscall, write, SYS_EXIT, SYS_INPUT_POLL};

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

const KERNEL_HEAP_ADDR: u64 = 0x_4444_4444_0000;

extern "C" fn rust_main() -> ! {
    let mut ok = true;
    let mut buf = [0u8; 8];

    write(b"bad_input: INPUT_POLL with an undersized destination buffer\n");
    // Safety: `buf.as_ptr()` is valid, but `len` (4) is deliberately less
    // than the encoded event size (8); must be rejected before any write.
    let short_result = unsafe { syscall(SYS_INPUT_POLL, buf.as_mut_ptr() as u64, 4, 0) };
    if short_result < 0 {
        write(b"bad_input: undersized buffer rejected cleanly -- OK\n");
    } else {
        write(b"bad_input: UNEXPECTED -- undersized buffer was accepted\n");
        ok = false;
    }

    write(b"bad_input: INPUT_POLL into a kernel-address destination\n");
    // Safety: deliberately invalid destination.
    let kernel_result = unsafe { syscall(SYS_INPUT_POLL, KERNEL_HEAP_ADDR, 8, 0) };
    if kernel_result < 0 {
        write(b"bad_input: kernel-address destination rejected -- OK\n");
    } else {
        write(b"bad_input: UNEXPECTED -- kernel-address destination was accepted\n");
        ok = false;
    }

    write(b"bad_input: INPUT_POLL with a legitimate stack buffer\n");
    // Safety: `buf` is a valid, appropriately sized, writable stack buffer
    // for the duration of this call.
    let good_result = unsafe { syscall(SYS_INPUT_POLL, buf.as_mut_ptr() as u64, 8, 0) };
    if good_result >= 0 {
        write(b"bad_input: legitimate poll accepted -- OK\n");
    } else {
        write(b"bad_input: UNEXPECTED -- legitimate poll was rejected\n");
        ok = false;
    }

    // Safety: exit code reflects whether every expectation above held.
    unsafe {
        syscall(SYS_EXIT, if ok { 0 } else { 1 }, 0, 0);
    }
    loop {}
}
