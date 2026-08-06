//! `hello`: the well-behaved Ring 3 test ELF. Calls `write`, `getpid`, and
//! `yield`, then exits cleanly -- the "does a real ELF process, loaded and
//! run at CPL=3, complete a normal syscall-driven lifecycle" proof.

#![no_std]
#![no_main]

use core::arch::global_asm;

use hello_user::{syscall, u64_to_decimal, write, SYS_EXIT, SYS_GETPID, SYS_YIELD};

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

extern "C" fn rust_main() -> ! {
    write(b"hello: Ring 3 ELF process alive\n");

    // Safety: no arguments to validate; GETPID always succeeds for a
    // process asking about itself.
    let pid = unsafe { syscall(SYS_GETPID, 0, 0, 0) };
    write(b"hello: my pid is ");
    let mut buf = [0u8; 20];
    write(u64_to_decimal(pid.max(0) as u64, &mut buf));
    write(b"\n");

    // Safety: no arguments.
    unsafe {
        syscall(SYS_YIELD, 0, 0, 0);
    }
    write(b"hello: resumed after yield, exiting cleanly\n");

    // Safety: exit code 0, no pointers; never returns.
    unsafe {
        syscall(SYS_EXIT, 0, 0, 0);
    }
    loop {}
}
