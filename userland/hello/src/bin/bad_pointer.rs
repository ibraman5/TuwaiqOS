//! `bad_pointer`: calls `WRITE` with a pointer into kernel address space
//! instead of its own memory. Proves the kernel validates the pointer
//! *before* dereferencing it -- a clean `-1`, not a Ring 0 page fault from
//! the kernel blindly trusting a user-supplied address.

#![no_std]
#![no_main]

use core::arch::global_asm;

use hello_user::{syscall, write, SYS_EXIT, SYS_WRITE};

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

/// The kernel heap's fixed base (`memory::HEAP_START`) -- real kernel
/// memory, never part of this process's own mapped user region.
const KERNEL_HEAP_ADDR: u64 = 0x_4444_4444_0000;

extern "C" fn rust_main() -> ! {
    write(b"bad_pointer: about to WRITE with a kernel-address pointer\n");
    // Safety: deliberately invalid -- `KERNEL_HEAP_ADDR` was never mapped
    // into this process's address space. The kernel's `copy_from_current_user`
    // must reject this via a page-table walk before ever reading it.
    let result = unsafe { syscall(SYS_WRITE, KERNEL_HEAP_ADDR, 16, 0) };
    if result < 0 {
        write(b"bad_pointer: kernel rejected it cleanly, still running -- OK\n");
    } else {
        write(b"bad_pointer: UNEXPECTED -- invalid pointer did not report failure\n");
    }
    // Safety: exit code reflects whether the rejection behaved as expected.
    unsafe {
        syscall(SYS_EXIT, if result < 0 { 0 } else { 1 }, 0, 0);
    }
    loop {}
}
