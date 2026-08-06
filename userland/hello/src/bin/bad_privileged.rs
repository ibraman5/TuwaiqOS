//! `bad_privileged`: executes `cli`, a privileged instruction, directly at
//! Ring 3 (no syscall involved at all). The CPU itself rejects this with a
//! General Protection Fault purely as a consequence of CPL=3 -- proves the
//! kernel's fault-isolation path kills only this process, not the kernel.
//!
//! If this process is ever observed printing its "UNEXPECTED" line, Ring 3
//! privilege separation is not actually working.

#![no_std]
#![no_main]

use core::arch::{asm, global_asm};

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

extern "C" fn rust_main() -> ! {
    write(b"bad_privileged: about to execute a privileged instruction (cli)\n");
    // Safety: deliberately privileged; expected to fault (#GP) rather than
    // execute, since this process runs at CPL=3.
    unsafe {
        asm!("cli", options(nomem, nostack));
    }
    // Only reached if `cli` somehow did not fault -- should be unreachable.
    write(b"bad_privileged: UNEXPECTED -- cli did not fault\n");
    // Safety: exit code 1, no pointers.
    unsafe {
        syscall(SYS_EXIT, 1, 0, 0);
    }
    loop {}
}
