//! `bad_input`: exercises `SYS_INPUT_POLL`'s validation (Phase 5,
//! Milestone 5). Proves an undersized destination buffer and a
//! kernel-address destination are both rejected cleanly -- and, as a
//! positive control, the shell binds this process as foreground owner and
//! seeds one known `Z` event. Every invalid poll must leave it queued; the
//! final valid poll must return exactly that complete encoded record.

#![no_std]
#![no_main]

use core::arch::global_asm;

use hello_user::{syscall, write, SYS_EXIT, SYS_INPUT_POLL, SYS_MMAP, SYS_MUNMAP};

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
const NON_CANONICAL_ADDR: u64 = 0x0001_0000_0000_0000;

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

    write(b"bad_input: INPUT_POLL into a non-canonical destination\n");
    let noncanonical_result = unsafe { syscall(SYS_INPUT_POLL, NON_CANONICAL_ADDR, 8, 0) };
    if noncanonical_result < 0 {
        write(b"bad_input: non-canonical destination rejected, even if queue empty -- OK\n");
    } else {
        write(b"bad_input: UNEXPECTED -- non-canonical destination was accepted\n");
        ok = false;
    }

    write(b"bad_input: INPUT_POLL destination crossing into an unmapped page\n");
    let mapped = unsafe { syscall(SYS_MMAP, 4096, 1, 0) };
    if mapped < 0 {
        write(b"bad_input: UNEXPECTED -- scratch mmap failed\n");
        ok = false;
    } else {
        let cross_result = unsafe { syscall(SYS_INPUT_POLL, mapped as u64 + 4092, 8, 0) };
        if cross_result < 0 {
            write(b"bad_input: cross-page destination rejected before queue access -- OK\n");
        } else {
            write(b"bad_input: UNEXPECTED -- cross-page destination was accepted\n");
            ok = false;
        }
        let _ = unsafe { syscall(SYS_MUNMAP, mapped as u64, 4096, 0) };
    }

    write(b"bad_input: INPUT_POLL with a legitimate stack buffer\n");
    // Safety: `buf` is a valid, appropriately sized, writable stack buffer
    // for the duration of this call.
    let good_result = unsafe { syscall(SYS_INPUT_POLL, buf.as_mut_ptr() as u64, 8, 0) };
    let expected = [1, b'Z', 0, 0, 0, 0, 0, 0];
    if good_result == 1 && buf == expected {
        write(b"bad_input: seeded Z survived every invalid poll byte-for-byte -- OK\n");
    } else {
        write(b"bad_input: UNEXPECTED -- seeded event was missing, consumed, or corrupted\n");
        ok = false;
    }

    write(b"bad_input: invalid destination after queue became empty\n");
    let empty_invalid = unsafe { syscall(SYS_INPUT_POLL, NON_CANONICAL_ADDR, 8, 0) };
    if empty_invalid < 0 {
        write(b"bad_input: empty-queue invalid pointer still rejected -- OK\n");
    } else {
        write(b"bad_input: UNEXPECTED -- empty queue bypassed pointer validation\n");
        ok = false;
    }

    // Safety: exit code reflects whether every expectation above held.
    unsafe {
        syscall(SYS_EXIT, if ok { 0 } else { 1 }, 0, 0);
    }
    loop {}
}
