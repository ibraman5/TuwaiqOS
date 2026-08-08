//! Hostile preview provider used to prove Ring-3 crash isolation.

#![no_std]
#![no_main]

use core::arch::{asm, global_asm};

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
    hello_user::write(b"ai-preview: fault provider entering isolated failure\n");
    unsafe { asm!("ud2", options(noreturn)) }
}
