//! `bad_munmap`: exercises `SYS_MUNMAP`'s validation (Phase 5, Milestone 2).
//! Proves the kernel rejects releasing memory this process never actually
//! obtained through `SYS_MMAP` -- an unaligned pointer, and a
//! plausible-looking but never-mapped address inside the mmap arena's usual
//! range -- rather than trusting the caller's claim about what it owns.

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

/// Deliberately not page-aligned -- must be rejected on that basis alone,
/// regardless of whether anything is mapped nearby.
const UNALIGNED_PTR: u64 = 0x_7000_1000_0001;

/// Page-aligned and inside the mmap arena's address range, but never
/// actually returned by a `SYS_MMAP` call in this process -- the arena is a
/// bump allocator (see `task::mmap_in_current_process`'s docs), so nothing
/// has claimed this address yet.
const UNOWNED_PTR: u64 = 0x_7000_1000_0000;
const NON_CANONICAL_PTR: u64 = 0x0001_0000_0000_0000;
const KERNEL_HEAP_ADDR: u64 = 0x_4444_4444_0000;
const OVERFLOW_LEN: u64 = 0u64.wrapping_sub(UNOWNED_PTR);
const USER_MMAP_LIMIT: u64 = 0x7000_3FF0_0000;

extern "C" fn rust_main() -> ! {
    let mut ok = true;

    write(b"bad_munmap: requesting zero address and zero length\n");
    let zero_ptr = unsafe { syscall(SYS_MUNMAP, 0, 4096, 0) };
    let zero_len = unsafe { syscall(SYS_MUNMAP, UNOWNED_PTR, 0, 0) };
    if zero_ptr < 0 && zero_len < 0 {
        write(b"bad_munmap: zero pointer/length rejected -- OK\n");
    } else {
        write(b"bad_munmap: UNEXPECTED -- zero pointer or length was accepted\n");
        ok = false;
    }

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

    write(b"bad_munmap: requesting an overflowing range\n");
    let overflow_result = unsafe { syscall(SYS_MUNMAP, UNOWNED_PTR, OVERFLOW_LEN, 0) };
    if overflow_result < 0 {
        write(b"bad_munmap: arithmetic-overflow range rejected -- OK\n");
    } else {
        write(b"bad_munmap: UNEXPECTED -- overflowing range was accepted\n");
        ok = false;
    }

    write(b"bad_munmap: requesting a non-canonical range\n");
    let noncanonical_result = unsafe { syscall(SYS_MUNMAP, NON_CANONICAL_PTR, 4096, 0) };
    if noncanonical_result < 0 {
        write(b"bad_munmap: non-canonical pointer rejected -- OK\n");
    } else {
        write(b"bad_munmap: UNEXPECTED -- non-canonical pointer was accepted\n");
        ok = false;
    }

    write(b"bad_munmap: requesting a kernel-space range\n");
    let kernel_result = unsafe { syscall(SYS_MUNMAP, KERNEL_HEAP_ADDR, 4096, 0) };
    if kernel_result < 0 {
        write(b"bad_munmap: kernel-space pointer rejected -- OK\n");
    } else {
        write(b"bad_munmap: UNEXPECTED -- kernel-space pointer was accepted\n");
        ok = false;
    }

    write(b"bad_munmap: requesting a range that crosses the mmap arena boundary\n");
    let boundary_result = unsafe { syscall(SYS_MUNMAP, USER_MMAP_LIMIT - 4096, 8192, 0) };
    if boundary_result < 0 {
        write(b"bad_munmap: mmap-arena boundary crossing rejected -- OK\n");
    } else {
        write(b"bad_munmap: UNEXPECTED -- mmap-arena boundary crossing was accepted\n");
        ok = false;
    }

    write(b"bad_munmap: constructing a two-page range with a hole in page two\n");
    let pair = unsafe { syscall(SYS_MMAP, 8192, 1, 0) };
    if pair < 0 {
        write(b"bad_munmap: UNEXPECTED -- two-page mmap failed\n");
        ok = false;
    } else {
        let first = pair as u64 as *mut u8;
        unsafe {
            first.write_volatile(0x6D);
        }
        let second_result = unsafe { syscall(SYS_MUNMAP, pair as u64 + 4096, 4096, 0) };
        if second_result != 0 {
            write(b"bad_munmap: UNEXPECTED -- could not create second-page hole\n");
            ok = false;
        } else {
            write(b"bad_munmap: unmapping full range containing hole (atomicity test)\n");
            let whole_result = unsafe { syscall(SYS_MUNMAP, pair as u64, 8192, 0) };
            if whole_result >= 0 {
                write(b"bad_munmap: UNEXPECTED -- partially mapped range was accepted\n");
                ok = false;
            } else {
                // If MUNMAP removed page one before discovering the hole,
                // this direct access faults and the success marker is never
                // printed. A surviving read/write proves the failed batch was
                // atomic from userspace's perspective.
                unsafe {
                    let before = first.read_volatile();
                    first.write_volatile(0xA7);
                    if before == 0x6D && first.read_volatile() == 0xA7 {
                        write(b"bad_munmap: failed range left first page intact -- OK\n");
                    } else {
                        write(b"bad_munmap: UNEXPECTED -- first page contents changed\n");
                        ok = false;
                    }
                }
            }
        }
        if unsafe { syscall(SYS_MUNMAP, pair as u64, 4096, 0) } != 0 {
            write(b"bad_munmap: UNEXPECTED -- first-page cleanup failed\n");
            ok = false;
        } else if unsafe { syscall(SYS_MUNMAP, pair as u64, 4096, 0) } < 0 {
            write(b"bad_munmap: double unmap rejected -- OK\n");
        } else {
            write(b"bad_munmap: UNEXPECTED -- double unmap was accepted\n");
            ok = false;
        }
    }

    // Safety: exit code reflects whether every expectation above held.
    unsafe {
        syscall(SYS_EXIT, if ok { 0 } else { 1 }, 0, 0);
    }
    loop {}
}
