//! Short-lived Ring-3 peer used to prove the desktop can coexist with a
//! separately scheduled userspace process without stealing its input.

#![no_std]
#![no_main]

use core::arch::global_asm;

use hello_user::{syscall, write, SYS_EXIT, SYS_UPTIME_TICKS, SYS_YIELD};

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
    write(b"desktop_peer: started alongside desktop\n");
    let start = unsafe { syscall(SYS_UPTIME_TICKS, 0, 0, 0) };
    if start < 0 {
        write(b"desktop_peer: UNEXPECTED -- uptime unavailable\n");
        unsafe { syscall(SYS_EXIT, 1, 0, 0) };
        loop {}
    }

    loop {
        let now = unsafe { syscall(SYS_UPTIME_TICKS, 0, 0, 0) };
        if now < 0 {
            write(b"desktop_peer: UNEXPECTED -- uptime failed while running\n");
            unsafe { syscall(SYS_EXIT, 1, 0, 0) };
            loop {}
        }
        if (now as u64).wrapping_sub(start as u64) >= 200 {
            break;
        }
        unsafe {
            syscall(SYS_YIELD, 0, 0, 0);
        }
    }

    write(b"desktop_peer: completed two-second concurrent run -- OK\n");
    unsafe { syscall(SYS_EXIT, 0, 0, 0) };
    loop {}
}
