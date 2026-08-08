//! Hostile and positive tests for the mutable Phase 6 file ABI.

#![no_std]
#![no_main]

use core::arch::global_asm;

use hello_user::{
    syscall, SYS_CLOSE, SYS_EXIT, SYS_MKDIR, SYS_MMAP, SYS_MUNMAP, SYS_OPEN, SYS_PUT_FILE,
    SYS_READ, SYS_READDIR, SYS_REMOVE, SYS_STAT,
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
const DIR: &[u8] = b"/data/file-mutation-test/session";
const TEMP: &[u8] = b"/data/file-mutation-test/session/record.txt";
const PERSIST: &[u8] = b"/data/file-mutation-test/persist.txt";

extern "C" fn rust_main() -> ! {
    let page = call(SYS_MMAP, 4096, 1, 0);
    if page < 0 {
        fail(b"file-mutation: mmap setup");
    }
    let page = page as u64;
    let crossing = page + 4092;

    let valid_spec = buffer_spec(b"safe".as_ptr() as u64, 4);
    for bad in [NON_CANONICAL, KERNEL_ADDRESS, UNMAPPED_USER, 0] {
        if call(SYS_PUT_FILE, bad, 8, valid_spec.as_ptr() as u64) != -1
            || call(SYS_MKDIR, bad, 8, 0) != -1
            || call(SYS_REMOVE, bad, 8, 0) != -1
        {
            fail(b"file-mutation: hostile path pointer accepted");
        }
    }
    if call(
        SYS_PUT_FILE,
        PERSIST.as_ptr() as u64,
        PERSIST.len() as u64,
        crossing,
    ) != -1
    {
        fail(b"file-mutation: cross-page descriptor accepted");
    }
    for bad in [NON_CANONICAL, KERNEL_ADDRESS, UNMAPPED_USER, 0] {
        let spec = buffer_spec(bad, 4);
        if call(
            SYS_PUT_FILE,
            PERSIST.as_ptr() as u64,
            PERSIST.len() as u64,
            spec.as_ptr() as u64,
        ) != -1
        {
            fail(b"file-mutation: hostile data pointer accepted");
        }
    }
    let crossing_spec = buffer_spec(crossing, 8);
    if put_with_spec(PERSIST, &crossing_spec) != -1 {
        fail(b"file-mutation: cross-page data accepted");
    }
    let overflow_spec = buffer_spec(u64::MAX - 3, 8);
    let huge_spec = buffer_spec(b"x".as_ptr() as u64, 4097);
    let zero_bad_spec = buffer_spec(0, 0);
    if put_with_spec(PERSIST, &overflow_spec) != -1
        || put_with_spec(PERSIST, &huge_spec) != -1
        || put_with_spec(PERSIST, &zero_bad_spec) != -1
        || call(
            SYS_PUT_FILE,
            PERSIST.as_ptr() as u64,
            u64::MAX,
            valid_spec.as_ptr() as u64,
        ) != -1
    {
        fail(b"file-mutation: malformed range accepted");
    }
    let malformed = b"bad\0path";
    if call(
        SYS_MKDIR,
        malformed.as_ptr() as u64,
        malformed.len() as u64,
        0,
    ) != -1
    {
        fail(b"file-mutation: malformed path accepted");
    }
    if put(b"/apps/hello", b"overwrite") != -1
        || remove(b"/apps/hello") != -1
        || put(b"/data/file-mutation-test/../../apps/hello", b"escape") != -1
        || put(b"/data/peer/secret", b"cross-app") != -1
    {
        fail(b"file-mutation: namespace escape accepted");
    }

    if path_call(SYS_MKDIR, DIR) != 0 {
        fail(b"file-mutation: mkdir");
    }
    if put(TEMP, b"first") != 0 || put(TEMP, b"updated-record") != 0 {
        fail(b"file-mutation: create/update");
    }
    let (kind, size) = stat(TEMP);
    if kind != 1 || size != 14 {
        fail(b"file-mutation: file metadata");
    }
    let (kind, size) = stat(DIR);
    if kind != 2 || size != 1 {
        fail(b"file-mutation: directory metadata");
    }
    let mut listing = [0u8; 64];
    let list_spec = buffer_spec(listing.as_mut_ptr() as u64, listing.len() as u64);
    let listed = call(
        SYS_READDIR,
        DIR.as_ptr() as u64,
        DIR.len() as u64,
        list_spec.as_ptr() as u64,
    );
    if listed != 11 || &listing[..11] != b"record.txt\n" {
        fail(b"file-mutation: directory listing");
    }
    if read_exact(TEMP, b"updated-record") == false {
        fail(b"file-mutation: reopen updated file");
    }

    let bad_output_spec = buffer_spec(NON_CANONICAL, 16);
    if call(
        SYS_READDIR,
        DIR.as_ptr() as u64,
        DIR.len() as u64,
        bad_output_spec.as_ptr() as u64,
    ) != -1
        || call(SYS_READDIR, DIR.as_ptr() as u64, DIR.len() as u64, crossing) != -1
        || call(
            SYS_STAT,
            TEMP.as_ptr() as u64,
            TEMP.len() as u64,
            NON_CANONICAL,
        ) != -1
        || call(SYS_STAT, TEMP.as_ptr() as u64, TEMP.len() as u64, crossing) != -1
    {
        fail(b"file-mutation: hostile output pointer accepted");
    }

    if remove(DIR) != -1 || remove(TEMP) != 0 || path_call(SYS_STAT, TEMP) != -1 || remove(DIR) != 0
    {
        fail(b"file-mutation: atomic remove semantics");
    }

    if put(PERSIST, b"ring3-persist-v1") != 0 || put(PERSIST, b"ring3-persist-v2") != 0 {
        fail(b"file-mutation: persistent replacement");
    }
    if !read_exact(PERSIST, b"ring3-persist-v2") {
        fail(b"file-mutation: persistent reopen");
    }

    if call(SYS_MUNMAP, page, 4096, 0) != 0 {
        fail(b"file-mutation: mmap cleanup");
    }
    hello_user::write(
        b"file-mutation: PASS private-namespace create update stat readdir remove hostile-pointers persistent-data\n",
    );
    exit(0)
}

fn buffer_spec(pointer: u64, length: u64) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&pointer.to_le_bytes());
    bytes[8..].copy_from_slice(&length.to_le_bytes());
    bytes
}

fn put(path: &[u8], data: &[u8]) -> i64 {
    let spec = buffer_spec(data.as_ptr() as u64, data.len() as u64);
    put_with_spec(path, &spec)
}

fn put_with_spec(path: &[u8], spec: &[u8; 16]) -> i64 {
    call(
        SYS_PUT_FILE,
        path.as_ptr() as u64,
        path.len() as u64,
        spec.as_ptr() as u64,
    )
}

fn remove(path: &[u8]) -> i64 {
    path_call(SYS_REMOVE, path)
}

fn stat(path: &[u8]) -> (u64, u64) {
    let mut record = [0u8; 16];
    if call(
        SYS_STAT,
        path.as_ptr() as u64,
        path.len() as u64,
        record.as_mut_ptr() as u64,
    ) != 0
    {
        fail(b"file-mutation: stat failed");
    }
    (
        u64::from_le_bytes(record[..8].try_into().unwrap()),
        u64::from_le_bytes(record[8..].try_into().unwrap()),
    )
}

fn read_exact(path: &[u8], expected: &[u8]) -> bool {
    let handle = path_call(SYS_OPEN, path);
    if handle < 3 || expected.len() > 64 {
        return false;
    }
    let mut output = [0u8; 64];
    let count = call(
        SYS_READ,
        handle as u64,
        output.as_mut_ptr() as u64,
        output.len() as u64,
    );
    let closed = call(SYS_CLOSE, handle as u64, 0, 0) == 0;
    count == expected.len() as i64 && &output[..expected.len()] == expected && closed
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
