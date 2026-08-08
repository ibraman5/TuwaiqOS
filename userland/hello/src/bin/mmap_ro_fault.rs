//! Controlled CPU-enforcement test for read-only anonymous mappings.

#![no_std]
#![no_main]

use core::arch::global_asm;

use hello_user::{syscall, write, SYS_EXIT, SYS_MMAP};

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
    let addr = unsafe { syscall(SYS_MMAP, 4096, 0, 0) };
    if addr < 0 {
        write(b"mmap_ro_fault: UNEXPECTED -- read-only mmap failed\n");
        unsafe { syscall(SYS_EXIT, 1, 0, 0) };
        loop {}
    }

    let ptr = addr as u64 as *mut u8;
    let zeroed = unsafe { ptr.read_volatile() == 0 && ptr.add(4095).read_volatile() == 0 };
    if !zeroed {
        write(b"mmap_ro_fault: UNEXPECTED -- mapping was not zero-filled\n");
        unsafe { syscall(SYS_EXIT, 1, 0, 0) };
        loop {}
    }

    write(b"mmap_ro_fault: read-only page readable and zero-filled -- OK\n");
    write(b"mmap_ro_fault: about to write read-only page; expect isolated user #PF\n");
    unsafe {
        ptr.write_volatile(0xA5);
    }
    write(b"mmap_ro_fault: UNEXPECTED -- write to read-only page returned\n");
    unsafe { syscall(SYS_EXIT, 1, 0, 0) };
    loop {}
}
