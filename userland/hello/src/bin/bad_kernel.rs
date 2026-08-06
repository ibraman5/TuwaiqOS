//! `bad_kernel`: dereferences a kernel-space address directly (no syscall
//! involved). This process's own page tables never map the kernel heap as
//! `USER_ACCESSIBLE` (see `paging::new_address_space`), so the CPU rejects
//! the access with a page fault -- proves user code cannot read kernel
//! memory even by touching it directly, not just "the syscall layer
//! happens not to expose it."

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

/// The kernel heap's fixed base (`memory::HEAP_START`) -- present in every
/// address space's page tables (see `paging::new_address_space`), but
/// never with `USER_ACCESSIBLE` set.
const KERNEL_HEAP_ADDR: u64 = 0x_4444_4444_0000;

extern "C" fn rust_main() -> ! {
    write(b"bad_kernel: about to read kernel memory directly (no syscall)\n");
    let kernel_ptr = KERNEL_HEAP_ADDR as *const u64;
    // Safety: deliberately invalid -- this is the point of the test. The
    // CPU's own U/S permission check is expected to fault before this
    // value is ever produced.
    let _value = unsafe { core::ptr::read_volatile(kernel_ptr) };
    // Only reached if the read somehow did not fault -- should be
    // unreachable.
    write(b"bad_kernel: UNEXPECTED -- kernel memory read did not fault\n");
    // Safety: exit code 1, no pointers.
    unsafe {
        syscall(SYS_EXIT, 1, 0, 0);
    }
    loop {}
}
