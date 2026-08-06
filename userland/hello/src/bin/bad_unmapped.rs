//! `bad_unmapped`: dereferences an address that lies within this process's
//! own permitted user region (`paging::USER_SPACE_BASE..+SIZE`) but was
//! never actually mapped -- distinct from `bad_kernel` (denied by the U/S
//! permission bit) and `bad_pointer` (denied by the syscall's own
//! validation): this one is a plain not-present page fault, the same
//! failure mode a real, buggy user program would hit.

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

/// Deep inside the permitted user region, but far past anything this tiny
/// program's own segments or stack ever reach -- see `task.rs`'s
/// `spawn_user_process` (stack) and this program's own segment sizes.
const UNMAPPED_USER_ADDR: u64 = 0x_7000_0000_0000 + 0x_2000_0000;

extern "C" fn rust_main() -> ! {
    write(b"bad_unmapped: about to read unmapped memory inside my own user region\n");
    let ptr = UNMAPPED_USER_ADDR as *const u64;
    // Safety: deliberately invalid -- the point of the test. Expected to
    // page-fault (not present), not to succeed.
    let _value = unsafe { core::ptr::read_volatile(ptr) };
    // Only reached if the read somehow did not fault -- should be
    // unreachable.
    write(b"bad_unmapped: UNEXPECTED -- unmapped read did not fault\n");
    // Safety: exit code 1, no pointers.
    unsafe {
        syscall(SYS_EXIT, 1, 0, 0);
    }
    loop {}
}
