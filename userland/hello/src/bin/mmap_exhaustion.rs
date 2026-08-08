//! Bounded physical-exhaustion test for transactional `MMAP` rollback and
//! post-`MUNMAP` frame reuse. Intended for the acceptance guest's 128 MiB RAM.

#![no_std]
#![no_main]

use core::arch::global_asm;

use hello_user::{syscall, write, SYS_EXIT, SYS_MMAP, SYS_MUNMAP, SYS_UPTIME_TICKS};

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

const USER_MMAP_BASE: u64 = 0x7000_1000_0000;
const CHUNK_LEN: u64 = 64 * 1024 * 1024;
const MAX_RANGES: usize = 12;

fn write_ticks(prefix: &[u8], delta: u64) {
    let mut decimal = [0u8; 20];
    write(prefix);
    write(hello_user::u64_to_decimal(delta, &mut decimal));
    write(b" ticks\n");
}

extern "C" fn rust_main() -> ! {
    let mut ranges = [0u64; MAX_RANGES];
    let mut count = 0usize;
    let mut saw_failure = false;

    write(b"mmap_exhaustion: allocating 64 MiB chunks until rejection\n");
    while count < MAX_RANGES {
        let ticks_before = unsafe { syscall(SYS_UPTIME_TICKS, 0, 0, 0) };
        let result = unsafe { syscall(SYS_MMAP, CHUNK_LEN, 1, 0) };
        let ticks_after = unsafe { syscall(SYS_UPTIME_TICKS, 0, 0, 0) };
        if result < 0 {
            saw_failure = true;
            break;
        }
        if ticks_after <= ticks_before {
            write(b"mmap_exhaustion: UNEXPECTED -- timer did not advance during large mmap\n");
            unsafe { syscall(SYS_EXIT, 1, 0, 0) };
            loop {}
        }
        write_ticks(
            b"mmap_exhaustion: timer advanced during 64 MiB mmap -- OK delta=",
            (ticks_after - ticks_before) as u64,
        );
        ranges[count] = result as u64;
        count += 1;
        write(b"mmap_exhaustion: retained one full chunk\n");
    }

    if !saw_failure {
        write(b"mmap_exhaustion: UNEXPECTED -- no rejection within bounded arena\n");
        unsafe { syscall(SYS_EXIT, 1, 0, 0) };
        loop {}
    }
    write(b"mmap_exhaustion: allocation rejected under bounded exhaustion -- OK\n");

    let expected_retry = USER_MMAP_BASE + count as u64 * CHUNK_LEN;
    let retry = unsafe { syscall(SYS_MMAP, 4096, 1, 0) };
    if retry < 0 || retry as u64 != expected_retry {
        write(b"mmap_exhaustion: UNEXPECTED -- failed request left pages or advanced cursor\n");
        unsafe { syscall(SYS_EXIT, 1, 0, 0) };
        loop {}
    }
    write(b"mmap_exhaustion: immediate retry reused failed range (rollback) -- OK\n");

    let retry_ptr = retry as u64 as *mut u8;
    let mut zeroed = true;
    let mut offset = 0usize;
    while offset < 4096 {
        if unsafe { retry_ptr.add(offset).read_volatile() } != 0 {
            zeroed = false;
            break;
        }
        offset += 1;
    }
    unsafe {
        retry_ptr.write_volatile(0xD3);
    }
    if !zeroed || unsafe { retry_ptr.read_volatile() } != 0xD3 {
        write(b"mmap_exhaustion: UNEXPECTED -- retry page zero/write/read check failed\n");
        unsafe { syscall(SYS_EXIT, 1, 0, 0) };
        loop {}
    }
    if unsafe { syscall(SYS_MUNMAP, retry as u64, 4096, 0) } != 0 {
        write(b"mmap_exhaustion: UNEXPECTED -- retry page cleanup failed\n");
        unsafe { syscall(SYS_EXIT, 1, 0, 0) };
        loop {}
    }

    let mut index = 0usize;
    while index < count {
        let ticks_before = unsafe { syscall(SYS_UPTIME_TICKS, 0, 0, 0) };
        if unsafe { syscall(SYS_MUNMAP, ranges[index], CHUNK_LEN, 0) } != 0 {
            write(b"mmap_exhaustion: UNEXPECTED -- retained chunk cleanup failed\n");
            unsafe { syscall(SYS_EXIT, 1, 0, 0) };
            loop {}
        }
        let ticks_after = unsafe { syscall(SYS_UPTIME_TICKS, 0, 0, 0) };
        if ticks_after <= ticks_before {
            write(b"mmap_exhaustion: UNEXPECTED -- timer did not advance during large munmap\n");
            unsafe { syscall(SYS_EXIT, 1, 0, 0) };
            loop {}
        }
        write_ticks(
            b"mmap_exhaustion: timer advanced during 64 MiB munmap -- OK delta=",
            (ticks_after - ticks_before) as u64,
        );
        index += 1;
    }
    write(b"mmap_exhaustion: retained frames released via MUNMAP -- OK\n");

    let reused = unsafe { syscall(SYS_MMAP, 4096, 1, 0) };
    if reused < 0 {
        write(b"mmap_exhaustion: UNEXPECTED -- mmap after release failed\n");
        unsafe { syscall(SYS_EXIT, 1, 0, 0) };
        loop {}
    }
    let reused_ptr = reused as u64 as *mut u8;
    if unsafe { reused_ptr.read_volatile() } != 0 {
        write(b"mmap_exhaustion: UNEXPECTED -- reused frame was not scrubbed\n");
        unsafe { syscall(SYS_EXIT, 1, 0, 0) };
        loop {}
    }
    unsafe {
        reused_ptr.write_volatile(0x4E);
    }
    if unsafe { reused_ptr.read_volatile() } != 0x4E
        || unsafe { syscall(SYS_MUNMAP, reused as u64, 4096, 0) } != 0
    {
        write(b"mmap_exhaustion: UNEXPECTED -- reused frame access/cleanup failed\n");
        unsafe { syscall(SYS_EXIT, 1, 0, 0) };
        loop {}
    }

    write(b"mmap_exhaustion: frame reuse remained zero-filled and writable -- OK\n");
    unsafe { syscall(SYS_EXIT, 0, 0, 0) };
    loop {}
}
