//! `bad_munmap`: exercises `SYS_MUNMAP`'s validation (Phase 5, Milestone 2).
//! Proves the kernel rejects releasing memory this process never actually
//! obtained through `SYS_MMAP` -- an unaligned pointer, and a
//! plausible-looking but never-mapped address inside the mmap arena's usual
//! range -- rather than trusting the caller's claim about what it owns.

#![no_std]
#![no_main]

use core::arch::global_asm;

use hello_user::{syscall, write, SYS_EXIT, SYS_MUNMAP};

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

/// Deliberately not page-aligned -- must be rejected on that basis alone,
/// regardless of whether anything is mapped nearby.
const UNALIGNED_PTR: u64 = 0x_7000_1000_0001;

/// Page-aligned and inside the mmap arena's address range, but never
/// actually returned by a `SYS_MMAP` call in this process -- the arena is a
/// bump allocator (see `task::mmap_in_current_process`'s docs), so nothing
/// has claimed this address yet.
const UNOWNED_PTR: u64 = 0x_7000_1000_0000;

extern "C" fn rust_main() -> ! {
    let mut ok = true;

    write(b"bad_munmap: requesting munmap of an unaligned pointer\n");
    // Safety: deliberately invalid input to a syscall documented to reject
    // it; no memory is actually touched on either success or failure path.
    let unaligned_result = unsafe { syscall(SYS_MUNMAP, UNALIGNED_PTR, 4096, 0) };
    if unaligned_result < 0 {
        write(b"bad_munmap: unaligned pointer rejected cleanly -- OK\n");
    } else {
        write(b"bad_munmap: UNEXPECTED -- unaligned pointer was accepted\n");
        ok = false;
    }

    write(b"bad_munmap: requesting munmap of an address never mmap'd by this process\n");
    // Safety: same reasoning -- deliberately invalid, rejected before any
    // unmap logic runs.
    let unowned_result = unsafe { syscall(SYS_MUNMAP, UNOWNED_PTR, 4096, 0) };
    if unowned_result < 0 {
        write(b"bad_munmap: never-mapped address rejected cleanly -- OK\n");
    } else {
        write(b"bad_munmap: UNEXPECTED -- never-mapped address was accepted\n");
        ok = false;
    }

    // Safety: exit code reflects whether every expectation above held.
    unsafe {
        syscall(SYS_EXIT, if ok { 0 } else { 1 }, 0, 0);
    }
    loop {}
}
