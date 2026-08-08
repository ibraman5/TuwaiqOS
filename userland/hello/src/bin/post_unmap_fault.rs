//! Controlled CPU-enforcement test for access after successful `MUNMAP`.

#![no_std]
#![no_main]

use core::arch::global_asm;

use hello_user::{syscall, write, SYS_EXIT, SYS_MMAP, SYS_MUNMAP};

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
    let addr = unsafe { syscall(SYS_MMAP, 4096, 1, 0) };
    if addr < 0 {
        write(b"post_unmap_fault: UNEXPECTED -- mmap failed\n");
        unsafe { syscall(SYS_EXIT, 1, 0, 0) };
        loop {}
    }
    let ptr = addr as u64 as *mut u8;
    unsafe {
        ptr.write_volatile(0x5C);
    }
    if unsafe { syscall(SYS_MUNMAP, addr as u64, 4096, 0) } != 0 {
        write(b"post_unmap_fault: UNEXPECTED -- munmap failed\n");
        unsafe { syscall(SYS_EXIT, 1, 0, 0) };
        loop {}
    }

    write(b"post_unmap_fault: munmap succeeded -- OK\n");
    write(b"post_unmap_fault: about to read released page; expect isolated user #PF\n");
    let _ = unsafe { ptr.read_volatile() };
    write(b"post_unmap_fault: UNEXPECTED -- post-unmap read returned\n");
    unsafe { syscall(SYS_EXIT, 1, 0, 0) };
    loop {}
}
