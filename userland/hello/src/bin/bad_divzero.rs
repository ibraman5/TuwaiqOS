//! `bad_divzero`: executes a raw `div` instruction with a zero divisor
//! directly at Ring 3 (no syscall involved). Uses inline asm rather than
//! Rust's `/` operator deliberately -- Rust's own integer division panics
//! in software on a zero divisor (in every build profile, not just debug),
//! which would just call this crate's panic handler and never reach real
//! hardware; a raw `div` is what actually raises the CPU's `#DE` exception.
//! Proves the kernel's `#DE` fault-isolation path kills only this process,
//! not the kernel.
//!
//! If this process is ever observed printing its "UNEXPECTED" line, `#DE`
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
    write(b"bad_divzero: about to divide by zero (raw div instruction)\n");
    // Safety: deliberately divides EDX:EAX by zero; expected to fault
    // (#DE), not produce a value. EDX is zeroed first so this is a
    // straightforward zero-divisor case, not also an overflow case.
    unsafe {
        asm!(
            "xor edx, edx",
            "xor eax, eax",
            "xor ecx, ecx",
            "div ecx",
            options(nomem, nostack)
        );
    }
    // Only reached if `div` somehow did not fault -- should be unreachable.
    write(b"bad_divzero: UNEXPECTED -- div by zero did not fault\n");
    // Safety: exit code 1, no pointers.
    unsafe {
        syscall(SYS_EXIT, 1, 0, 0);
    }
    loop {}
}
