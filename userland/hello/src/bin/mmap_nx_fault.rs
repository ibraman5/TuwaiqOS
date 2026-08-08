//! Controlled CPU-enforcement test for NX anonymous mappings.

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
    let addr = unsafe { syscall(SYS_MMAP, 4096, 1, 0) };
    if addr < 0 {
        write(b"mmap_nx_fault: UNEXPECTED -- writable mmap failed\n");
        unsafe { syscall(SYS_EXIT, 1, 0, 0) };
        loop {}
    }

    let ptr = addr as u64 as *mut u8;
    // x86-64 `ret`: if execution were permitted this immediately returns to
    // the caller. Anonymous mappings are unconditionally NX, so instruction
    // fetch at `ptr` must instead produce an isolated userspace #PF.
    unsafe {
        ptr.write_volatile(0xC3);
    }
    write(b"mmap_nx_fault: writable data byte installed -- OK\n");
    write(b"mmap_nx_fault: about to execute NX page; expect isolated user #PF\n");
    let entry: extern "C" fn() = unsafe { core::mem::transmute(addr as u64) };
    entry();
    write(b"mmap_nx_fault: UNEXPECTED -- execution from NX page returned\n");
    unsafe { syscall(SYS_EXIT, 1, 0, 0) };
    loop {}
}
