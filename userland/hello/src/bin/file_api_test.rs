//! Deterministic Phase 6 VFS/syscall boundary test.

#![no_std]
#![no_main]

use core::arch::global_asm;

use hello_user::{
    syscall, SYS_CHDIR, SYS_CLOSE, SYS_EXIT, SYS_GETCWD, SYS_MMAP, SYS_MUNMAP, SYS_OPEN, SYS_READ,
    SYS_SEEK, SYS_SPAWN,
};

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

const NON_CANONICAL: u64 = 0x0000_8000_0000_0000;
const KERNEL_ADDRESS: u64 = 0x4444_4444_0000;
const UNMAPPED_USER: u64 = 0x7000_2000_0000;

extern "C" fn rust_main() -> ! {
    let mut cwd = [0u8; 64];
    if call(SYS_GETCWD, cwd.as_mut_ptr() as u64, cwd.len() as u64, 0) != 12
        || &cwd[..12] != b"/phase6-test"
    {
        fail(b"file-api: initial per-process cwd");
    }

    for bad in [NON_CANONICAL, KERNEL_ADDRESS, UNMAPPED_USER, 0] {
        if call(SYS_GETCWD, bad, 64, 0) != -1 {
            fail(b"file-api: GETCWD accepted invalid pointer");
        }
    }

    if path_call(SYS_CHDIR, b".") != 0
        || path_call(SYS_CHDIR, b"..") != 0
        || path_call(SYS_CHDIR, b"/phase6-test") != 0
        || path_call(SYS_CHDIR, b"data.txt") != -1
        || path_call(SYS_CHDIR, b"missing") != -1
    {
        fail(b"file-api: CHDIR semantics");
    }

    for bad in [NON_CANONICAL, KERNEL_ADDRESS, UNMAPPED_USER, 0] {
        if call(SYS_OPEN, bad, 8, 0) != -1 {
            fail(b"file-api: OPEN accepted invalid pointer");
        }
    }
    if call(SYS_OPEN, b"data.txt".as_ptr() as u64, u64::MAX, 0) != -1 {
        fail(b"file-api: OPEN accepted huge length");
    }
    if call(SYS_OPEN, u64::MAX - 3, 8, 0) != -1
        || call(SYS_OPEN, b"data.txt".as_ptr() as u64, 0, 0) != -1
    {
        fail(b"file-api: OPEN accepted overflow/zero length");
    }
    let invalid_utf8 = [0xffu8];
    if call(SYS_OPEN, invalid_utf8.as_ptr() as u64, 1, 0) != -1
        || path_call(SYS_OPEN, b"data\0txt") != -1
    {
        fail(b"file-api: OPEN accepted malformed path");
    }
    let overlong = [b'x'; 121];
    if call(SYS_OPEN, overlong.as_ptr() as u64, overlong.len() as u64, 0) != -1 {
        fail(b"file-api: OPEN accepted overlong path");
    }

    let page = call(SYS_MMAP, 4096, 1, 0);
    if page < 0 {
        fail(b"file-api: mmap setup failed");
    }
    let crossing = page as u64 + 4092;
    if call(SYS_GETCWD, crossing, 64, 0) != -1 {
        fail(b"file-api: GETCWD accepted cross-page destination");
    }
    if call(SYS_OPEN, crossing, 8, 0) != -1 {
        fail(b"file-api: OPEN accepted cross-page path");
    }
    for bad in [NON_CANONICAL, KERNEL_ADDRESS, UNMAPPED_USER, 0] {
        if call(SYS_SPAWN, bad, 8, 0) != -1 {
            fail(b"file-api: SPAWN accepted invalid pointer");
        }
    }
    if call(SYS_SPAWN, crossing, 8, 0) != -1
        || call(SYS_SPAWN, u64::MAX - 3, 8, 0) != -1
        || call(SYS_SPAWN, b"missing".as_ptr() as u64, u64::MAX, 0) != -1
        || call(SYS_SPAWN, b"missing".as_ptr() as u64, 0, 0) != -1
        || path_call(SYS_SPAWN, b"missing") != -1
    {
        fail(b"file-api: SPAWN accepted invalid path");
    }

    let handle = path_call(SYS_OPEN, b"data.txt");
    if handle < 3 {
        fail(b"file-api: OPEN failed");
    }

    if call(SYS_READ, handle as u64, NON_CANONICAL, 0) != -1
        || call(SYS_READ, u64::MAX, page as u64, 0) != -1
        || call(SYS_READ, handle as u64, page as u64, 0) != 0
    {
        fail(b"file-api: zero-length READ validation");
    }

    for bad in [NON_CANONICAL, KERNEL_ADDRESS, UNMAPPED_USER, 0] {
        if call(SYS_READ, handle as u64, bad, 5) != -1 {
            fail(b"file-api: READ accepted invalid destination");
        }
    }
    let read_only = call(SYS_MMAP, 4096, 0, 0);
    if read_only < 0 || call(SYS_READ, handle as u64, read_only as u64, 5) != -1 {
        fail(b"file-api: READ accepted read-only destination");
    }
    if call(SYS_READ, handle as u64, crossing, 8) != -1
        || call(SYS_READ, handle as u64, page as u64, 4097) != -1
    {
        fail(b"file-api: READ accepted cross-page/huge destination");
    }
    let mut first = [0u8; 5];
    if call(SYS_READ, handle as u64, first.as_mut_ptr() as u64, 5) != 5 || &first != b"phase" {
        fail(b"file-api: invalid READ consumed data");
    }
    let mut rest = [0u8; 16];
    let count = call(
        SYS_READ,
        handle as u64,
        rest.as_mut_ptr() as u64,
        rest.len() as u64,
    );
    if count != 6 || &rest[..6] != b"6-data" {
        hello_user::write(b"file-api: sequential count=");
        let mut digits = [0u8; 20];
        hello_user::write(hello_user::u64_to_decimal(count.max(0) as u64, &mut digits));
        hello_user::write(b" bytes=");
        hello_user::write(&rest[..count.max(0) as usize]);
        hello_user::write(b"\n");
        fail(b"file-api: sequential READ mismatch");
    }
    if call(SYS_SEEK, handle as u64, 0, 0) != 0 {
        fail(b"file-api: SEEK rewind failed");
    }
    let mut rewound = [0u8; 5];
    if call(SYS_READ, handle as u64, rewound.as_mut_ptr() as u64, 5) != 5
        || &rewound != b"phase"
        || call(SYS_SEEK, handle as u64, 12, 0) != -1
        || call(SYS_SEEK, u64::MAX, 0, 0) != -1
        || call(SYS_SEEK, handle as u64, 11, 0) != 11
    {
        fail(b"file-api: SEEK bounds/handle semantics");
    }
    if call(
        SYS_READ,
        handle as u64,
        rest.as_mut_ptr() as u64,
        rest.len() as u64,
    ) != 0
    {
        fail(b"file-api: EOF was not stable");
    }
    if call(SYS_CLOSE, handle as u64, 0, 0) != 0 || call(SYS_CLOSE, handle as u64, 0, 0) != -1 {
        fail(b"file-api: CLOSE/double-close semantics");
    }

    let mut handles = [0i64; 16];
    for slot in &mut handles {
        *slot = path_call(SYS_OPEN, b"data.txt");
        if *slot < 3 {
            fail(b"file-api: bounded handle table filled early");
        }
    }
    if path_call(SYS_OPEN, b"data.txt") != -1 {
        fail(b"file-api: handle exhaustion not rejected");
    }
    for handle in handles {
        if call(SYS_CLOSE, handle as u64, 0, 0) != 0 {
            fail(b"file-api: handle cleanup failed");
        }
    }

    if call(SYS_MUNMAP, page as u64, 4096, 0) != 0 {
        fail(b"file-api: mmap cleanup failed");
    }
    if call(SYS_MUNMAP, read_only as u64, 4096, 0) != 0 {
        fail(b"file-api: read-only mmap cleanup failed");
    }
    if path_call(SYS_OPEN, b"data.txt") < 3 {
        fail(b"file-api: exit-cleanup handle setup failed");
    }

    hello_user::write(
        b"file-api: PASS cwd paths malformed-pointers cross-page atomic-read seek eof handles exit-cleanup\n",
    );
    exit(0)
}

fn path_call(number: u64, path: &[u8]) -> i64 {
    call(number, path.as_ptr() as u64, path.len() as u64, 0)
}

fn call(number: u64, a1: u64, a2: u64, a3: u64) -> i64 {
    unsafe { syscall(number, a1, a2, a3) }
}

fn fail(message: &[u8]) -> ! {
    hello_user::write(message);
    hello_user::write(b" FAIL\n");
    exit(1)
}

fn exit(code: i32) -> ! {
    unsafe {
        syscall(SYS_EXIT, code as u64, 0, 0);
    }
    loop {}
}
