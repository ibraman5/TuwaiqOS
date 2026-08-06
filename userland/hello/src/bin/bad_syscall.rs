//! `bad_syscall`: calls a syscall number the kernel does not implement.
//! Proves the kernel rejects it safely (a plain `-1`, logged, nothing more)
//! and lets this process keep running and exit normally afterward.

#![no_std]
#![no_main]

use core::arch::global_asm;

use hello_user::{syscall, write, SYS_EXIT};

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

const INVALID_SYSCALL_NUMBER: u64 = 999;

extern "C" fn rust_main() -> ! {
    write(b"bad_syscall: about to call an unimplemented syscall number\n");
    // Safety: deliberately invalid syscall number; the kernel's dispatch
    // `_` arm handles this without touching any argument as a pointer.
    let result = unsafe { syscall(INVALID_SYSCALL_NUMBER, 0, 0, 0) };
    if result < 0 {
        write(b"bad_syscall: kernel rejected it cleanly, still running -- OK\n");
    } else {
        write(b"bad_syscall: UNEXPECTED -- invalid syscall did not report failure\n");
    }
    // Safety: exit code reflects whether the rejection behaved as expected.
    unsafe {
        syscall(SYS_EXIT, if result < 0 { 0 } else { 1 }, 0, 0);
    }
    loop {}
}
