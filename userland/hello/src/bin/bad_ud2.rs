//! `bad_ud2`: executes `ud2`, the x86 instruction defined specifically to
//! always raise an invalid-opcode exception, directly at Ring 3 (no
//! syscall involved). Proves the kernel's `#UD` fault-isolation path kills
//! only this process, not the kernel.
//!
//! If this process is ever observed printing its "UNEXPECTED" line, `#UD`
//! isolation is not actually working.

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
    write(b"bad_ud2: about to execute ud2 (invalid opcode)\n");
    // Safety: deliberately invalid; expected to fault (#UD), not execute.
    unsafe {
        asm!("ud2", options(nomem, nostack));
    }
    // Only reached if `ud2` somehow did not fault -- should be unreachable.
    write(b"bad_ud2: UNEXPECTED -- ud2 did not fault\n");
    // Safety: exit code 1, no pointers.
    unsafe {
        syscall(SYS_EXIT, 1, 0, 0);
    }
    loop {}
}
