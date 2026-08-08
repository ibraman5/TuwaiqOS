//! `bad_pointer`: returning negative tests for every `WRITE` user-pointer
//! boundary: kernel-space, non-canonical, unmapped, and a mapped first page
//! whose requested range crosses into an unmapped second page.

#![no_std]
#![no_main]

use core::arch::global_asm;

use hello_user::{syscall, write, SYS_EXIT, SYS_MMAP, SYS_MUNMAP, SYS_WRITE};

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
const NON_CANONICAL_ADDR: u64 = 0x0001_0000_0000_0000;
const UNMAPPED_USER_ADDR: u64 = 0x7000_2000_0000;
const USER_SPACE_END: u64 = 0x7000_4000_0000;

extern "C" fn rust_main() -> ! {
    let mut ok = true;

    write(b"bad_pointer: WRITE with zero address\n");
    let result = unsafe { syscall(SYS_WRITE, 0, 1, 0) };
    if result < 0 {
        write(b"bad_pointer: zero address rejected -- OK\n");
    } else {
        write(b"bad_pointer: UNEXPECTED -- zero address was accepted\n");
        ok = false;
    }

    write(b"bad_pointer: about to WRITE with a kernel-address pointer\n");
    // Safety: deliberately invalid -- `KERNEL_HEAP_ADDR` was never mapped
    // into this process's address space. The kernel's `copy_from_current_user`
    // must reject this via a page-table walk before ever reading it.
    let result = unsafe { syscall(SYS_WRITE, KERNEL_HEAP_ADDR, 16, 0) };
    if result < 0 {
        write(b"bad_pointer: kernel rejected it cleanly, still running -- OK\n");
    } else {
        write(b"bad_pointer: UNEXPECTED -- invalid pointer did not report failure\n");
        ok = false;
    }

    write(b"bad_pointer: WRITE with a non-canonical pointer\n");
    let result = unsafe { syscall(SYS_WRITE, NON_CANONICAL_ADDR, 16, 0) };
    if result < 0 {
        write(b"bad_pointer: non-canonical pointer rejected without Ring-0 panic -- OK\n");
    } else {
        write(b"bad_pointer: UNEXPECTED -- non-canonical pointer was accepted\n");
        ok = false;
    }

    write(b"bad_pointer: WRITE from an unmapped in-range user address\n");
    let result = unsafe { syscall(SYS_WRITE, UNMAPPED_USER_ADDR, 16, 0) };
    if result < 0 {
        write(b"bad_pointer: unmapped user pointer rejected -- OK\n");
    } else {
        write(b"bad_pointer: UNEXPECTED -- unmapped user pointer was accepted\n");
        ok = false;
    }

    write(b"bad_pointer: WRITE crossing the user-address-space boundary\n");
    let result = unsafe { syscall(SYS_WRITE, USER_SPACE_END - 4, 8, 0) };
    if result < 0 {
        write(b"bad_pointer: user-boundary crossing rejected -- OK\n");
    } else {
        write(b"bad_pointer: UNEXPECTED -- user-boundary crossing was accepted\n");
        ok = false;
    }

    write(b"bad_pointer: WRITE with pointer-plus-length arithmetic overflow\n");
    let result = unsafe { syscall(SYS_WRITE, u64::MAX - 7, 16, 0) };
    if result < 0 {
        write(b"bad_pointer: overflowing range rejected -- OK\n");
    } else {
        write(b"bad_pointer: UNEXPECTED -- overflowing range was accepted\n");
        ok = false;
    }

    write(b"bad_pointer: WRITE with a huge length\n");
    let result = unsafe { syscall(SYS_WRITE, USER_SPACE_END - 1, u64::MAX, 0) };
    if result < 0 {
        write(b"bad_pointer: huge length rejected -- OK\n");
    } else {
        write(b"bad_pointer: UNEXPECTED -- huge length was accepted\n");
        ok = false;
    }

    write(b"bad_pointer: WRITE range crossing from mapped into unmapped page\n");
    let mapped = unsafe { syscall(SYS_MMAP, 4096, 1, 0) };
    if mapped < 0 {
        write(b"bad_pointer: UNEXPECTED -- scratch mmap failed\n");
        ok = false;
    } else {
        let cross_page = mapped as u64 + 4092;
        let result = unsafe { syscall(SYS_WRITE, cross_page, 8, 0) };
        if result < 0 {
            write(b"bad_pointer: mapped-to-unmapped cross-page range rejected -- OK\n");
        } else {
            write(b"bad_pointer: UNEXPECTED -- cross-page invalid range was accepted\n");
            ok = false;
        }
        let _ = unsafe { syscall(SYS_MUNMAP, mapped as u64, 4096, 0) };
    }
    // Safety: exit code reflects whether the rejection behaved as expected.
    unsafe {
        syscall(SYS_EXIT, if ok { 0 } else { 1 }, 0, 0);
    }
    loop {}
}
