//! `bad_mmap`: exercises `SYS_MMAP`'s validation (Phase 5, Milestone 2).
//! Proves zero-length and grossly-oversized requests are rejected cleanly,
//! then proves a legitimate small mmap actually works end to end: the
//! returned address is really mapped writable in *this* process (a direct
//! store through the raw pointer succeeds and reads back correctly, not
//! just "the syscall returned success"), and `SYS_MUNMAP` cleanly releases
//! it afterward.

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

/// Comfortably larger than `task::MAX_MMAP_LEN` (64 MiB) and than the whole
/// mmap arena (`USER_MMAP_LIMIT - USER_MMAP_BASE`, under 1 GiB) -- this
/// must be rejected however the kernel currently phrases its own bound.
const HUGE_LEN: u64 = 0x_1000_0000_0000;
const MAX_MMAP_LEN_PLUS_ONE: u64 = 64 * 1024 * 1024 + 1;

extern "C" fn rust_main() -> ! {
    let mut ok = true;

    write(b"bad_mmap: requesting a zero-length mapping\n");
    // Safety: deliberately invalid (len=0); a zero-length mapping is
    // meaningless and must be rejected before any address is chosen.
    let zero_result = unsafe { syscall(SYS_MMAP, 0, 1, 0) };
    if zero_result < 0 {
        write(b"bad_mmap: zero-length mmap rejected cleanly -- OK\n");
    } else {
        write(b"bad_mmap: UNEXPECTED -- zero-length mmap succeeded\n");
        ok = false;
    }

    write(b"bad_mmap: requesting a grossly oversized mapping\n");
    // Safety: deliberately invalid (far beyond MAX_MMAP_LEN / the arena
    // itself); must be rejected rather than exhausting kernel memory.
    let huge_result = unsafe { syscall(SYS_MMAP, HUGE_LEN, 1, 0) };
    if huge_result < 0 {
        write(b"bad_mmap: oversized mmap rejected cleanly -- OK\n");
    } else {
        write(b"bad_mmap: UNEXPECTED -- oversized mmap succeeded\n");
        ok = false;
    }

    write(b"bad_mmap: requesting one byte beyond the per-call bound\n");
    let bounded_result = unsafe { syscall(SYS_MMAP, MAX_MMAP_LEN_PLUS_ONE, 1, 0) };
    if bounded_result < 0 {
        write(b"bad_mmap: MAX_MMAP_LEN+1 rejected -- OK\n");
    } else {
        write(b"bad_mmap: UNEXPECTED -- over-bound mmap succeeded\n");
        ok = false;
    }

    write(b"bad_mmap: requesting a legitimate one-page writable mapping\n");
    // Safety: a small, sane request; the returned value (if non-negative)
    // is documented to be a valid, writable, USER_ACCESSIBLE address in
    // this process's own address space.
    let addr = unsafe { syscall(SYS_MMAP, 4096, 1, 0) };
    if addr < 0 {
        write(b"bad_mmap: UNEXPECTED -- legitimate small mmap was rejected\n");
        ok = false;
    } else {
        write(b"bad_mmap: legitimate mmap succeeded, verifying full-page zero-fill\n");
        let ptr = addr as u64 as *mut u8;
        // Safety: `addr` was just returned by a successful MMAP as a
        // writable page in this process's own address space; `ptr` points
        // at the first of at least 4096 such bytes.
        unsafe {
            let mut zeroed = true;
            let mut offset = 0usize;
            while offset < 4096 {
                if ptr.add(offset).read_volatile() != 0 {
                    zeroed = false;
                    ok = false;
                    break;
                }
                offset += 1;
            }
            if zeroed {
                write(b"bad_mmap: all 4096 fresh bytes were zero -- OK\n");
            } else {
                write(b"bad_mmap: UNEXPECTED -- fresh mapping exposed nonzero data\n");
            }
            ptr.write_volatile(0xAB);
            let readback = ptr.read_volatile();
            if readback == 0xAB {
                write(b"bad_mmap: write/read-back through the mapped page matched -- OK\n");
            } else {
                write(b"bad_mmap: UNEXPECTED -- read-back did not match what was written\n");
                ok = false;
            }
        }

        write(b"bad_mmap: releasing the mapping via munmap\n");
        // Safety: releasing exactly the range this process itself just
        // mapped.
        let munmap_result = unsafe { syscall(SYS_MUNMAP, addr as u64, 4096, 0) };
        if munmap_result == 0 {
            write(b"bad_mmap: munmap of an owned mapping succeeded -- OK\n");
        } else {
            write(b"bad_mmap: UNEXPECTED -- munmap of an owned mapping failed\n");
            ok = false;
        }
    }

    write(b"bad_mmap: requesting one byte to verify page rounding\n");
    let rounded = unsafe { syscall(SYS_MMAP, 1, 1, 0) };
    if rounded < 0 {
        write(b"bad_mmap: UNEXPECTED -- one-byte mmap failed\n");
        ok = false;
    } else {
        let last = (rounded as u64 + 4095) as *mut u8;
        unsafe {
            last.write_volatile(0x5A);
            if last.read_volatile() == 0x5A {
                write(b"bad_mmap: one-byte request supplied one full usable page -- OK\n");
            } else {
                write(b"bad_mmap: UNEXPECTED -- rounded page was not writable\n");
                ok = false;
            }
        }
        if unsafe { syscall(SYS_MUNMAP, rounded as u64, 4096, 0) } != 0 {
            write(b"bad_mmap: UNEXPECTED -- rounded mapping cleanup failed\n");
            ok = false;
        }
    }

    // Safety: exit code reflects whether every expectation above held.
    unsafe {
        syscall(SYS_EXIT, if ok { 0 } else { 1 }, 0, 0);
    }
    loop {}
}
