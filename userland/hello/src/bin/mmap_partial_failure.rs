//! Deterministic post-mutation MMAP rollback acceptance test.

#![no_std]
#![no_main]

use core::arch::global_asm;

use hello_user::{syscall, write, SYS_EXIT, SYS_MMAP, SYS_MUNMAP};

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

const USER_MMAP_BASE: u64 = 0x7000_1000_0000;
const TEST_LEN: u64 = 64 * 1024;

extern "C" fn rust_main() -> ! {
    let failed = unsafe { syscall(SYS_MMAP, TEST_LEN, 1, 0) };
    if failed >= 0 {
        write(b"mmap_partial_failure: UNEXPECTED -- injected partial map succeeded\n");
        unsafe { syscall(SYS_EXIT, 1, 0, 0) };
        loop {}
    }
    write(b"mmap_partial_failure: injected partial map rejected -- OK\n");

    let retry = unsafe { syscall(SYS_MMAP, TEST_LEN, 1, 0) };
    if retry < 0 || retry as u64 != USER_MMAP_BASE {
        write(b"mmap_partial_failure: UNEXPECTED -- rollback advanced or blocked arena\n");
        unsafe { syscall(SYS_EXIT, 1, 0, 0) };
        loop {}
    }

    let bytes = unsafe { core::slice::from_raw_parts_mut(retry as *mut u8, TEST_LEN as usize) };
    if bytes.iter().any(|byte| *byte != 0) {
        write(b"mmap_partial_failure: UNEXPECTED -- retry was not zero-filled\n");
        unsafe { syscall(SYS_EXIT, 1, 0, 0) };
        loop {}
    }
    bytes[0] = 0xA5;
    bytes[bytes.len() - 1] = 0x5A;
    if bytes[0] != 0xA5 || bytes[bytes.len() - 1] != 0x5A {
        write(b"mmap_partial_failure: UNEXPECTED -- retry was not writable\n");
        unsafe { syscall(SYS_EXIT, 1, 0, 0) };
        loop {}
    }
    if unsafe { syscall(SYS_MUNMAP, retry as u64, TEST_LEN, 0) } != 0 {
        write(b"mmap_partial_failure: UNEXPECTED -- retry cleanup failed\n");
        unsafe { syscall(SYS_EXIT, 1, 0, 0) };
        loop {}
    }

    write(b"mmap_partial_failure: rollback, same-address retry, zero-fill, and cleanup -- OK\n");
    unsafe { syscall(SYS_EXIT, 0, 0, 0) };
    loop {}
}
