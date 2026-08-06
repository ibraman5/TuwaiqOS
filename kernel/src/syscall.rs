//! The real syscall ABI (Phase 4) -- what Milestone 1's `int 0x80` "the
//! demo is over, come back to Ring 0" gate has become now that user
//! processes are real and need to make more than one round trip.
//!
//! ## ABI
//!
//! `int 0x80`. `RAX` is the syscall number on entry and the return value on
//! exit. Arguments are `RDI`, `RSI`, `RDX` (up to three -- nothing here
//! needs more). Return values follow a simple POSIX-ish convention: `>= 0`
//! is success (a byte count, a pid, or plain `0`), `-1` is a generic
//! failure (invalid syscall number, invalid pointer/range, or a rejected
//! argument) -- there is no `errno`-style detail channel in this minimal
//! ABI, only success/failure.
//!
//! | # | name     | args                  | returns                    |
//! |---|----------|-----------------------|-----------------------------|
//! | 0 | EXIT     | `code: i32`           | never returns               |
//! | 1 | WRITE    | `ptr: *u8, len: usize`| bytes written, or `-1`      |
//! | 2 | YIELD    | --                    | `0`                         |
//! | 3 | GETPID   | --                    | this process's task id      |
//!
//! Any other number is rejected with `-1` -- logged, not a fault, and the
//! calling process keeps running (see `dispatch`'s `_` arm).
//!
//! ## Entry mechanism
//!
//! `int 0x80` is a normal (non-exception) interrupt gate at DPL=3 (see
//! `interrupts.rs`), so no error code is pushed and the only privilege
//! change is Ring 3 -> Ring 0. Unlike every other handler in this kernel,
//! this one is **not** `extern "x86-interrupt"`: that calling convention
//! only exposes the CPU-pushed frame (`InterruptStackFrame`), not general
//! purpose registers, and the ABI above needs to read `RAX`/`RDI`/`RSI`/
//! `RDX` and write a return value back into `RAX`. Instead, `syscall_entry`
//! is a hand-written naked stub (same technique as `task.rs`'s
//! `context_switch`): it saves all 15 general-purpose registers it might
//! plausibly need to preserve, calls into Rust with a pointer to them,
//! restores them (with `RAX` now holding the dispatch result), and
//! `iretq`s back to Ring 3 -- except for `EXIT`, which ends the task
//! instead (see `sys_exit`), abandoning that saved-register frame exactly
//! the way `usermode.rs`'s retired demo abandoned its own call chain.
//!
//! ## Pointer validation
//!
//! `WRITE` is the only syscall here that takes a pointer. Its length is
//! capped (`MAX_WRITE_LEN`), and the bytes are never dereferenced directly
//! under the caller's live CR3 -- `task::copy_from_current_user` walks the
//! calling process's own page tables first (`paging::translate_in_address_space`
//! under the hood) and only reads through the physical-memory-offset
//! mapping once every page in range is confirmed `PRESENT | USER_ACCESSIBLE`.
//! An invalid pointer or range is a clean `-1`, never a Ring 0 page fault
//! from kernel code trusting a user-supplied address.

use crate::task;

const SYS_EXIT: u64 = 0;
const SYS_WRITE: u64 = 1;
const SYS_YIELD: u64 = 2;
const SYS_GETPID: u64 = 3;

/// Upper bound on a single `WRITE`'s length -- generous for this ABI's
/// only real use (a handful of short diagnostic lines from `hello_user`),
/// and a firm cap on how much a single syscall can make the kernel copy on
/// a caller's behalf regardless of what `len` claims.
const MAX_WRITE_LEN: usize = 4096;

/// The 15 general-purpose registers `syscall_entry` saves, in the exact
/// order they land in memory (lowest address first) given the push order
/// in the asm below -- `r15` first (pushed last), `rax` last (pushed
/// first). `rdi`/`rsi`/`rdx` are read directly by `syscall_dispatch` from
/// the arguments the trap arrived with; `rax` is read for the syscall
/// number and overwritten with the return value before this frame is
/// popped back into real registers.
#[repr(C)]
struct SyscallFrame {
    r15: u64,
    r14: u64,
    r13: u64,
    r12: u64,
    r11: u64,
    r10: u64,
    r9: u64,
    r8: u64,
    rbp: u64,
    rdi: u64,
    rsi: u64,
    rdx: u64,
    rcx: u64,
    rbx: u64,
    rax: u64,
}

// Safety: this is the syscall gate's IDT entry point (registered in
// `interrupts.rs` at DPL=3), reached only via `int 0x80` from Ring 3. The
// prologue saves all 15 GPRs (120 bytes -- keeping RSP 16-byte aligned
// right before `call {dispatch}`, matching the SysV requirement, given
// RSP0 itself is 16-byte aligned -- see `task.rs`'s `aligned_top`) before
// touching anything, and the epilogue restores every one of them from
// exactly the same memory, in reverse, before `iretq`. `EXIT`'s path
// through `syscall_dispatch` never returns here at all -- see the module
// docs.
core::arch::global_asm!(
    r#"
.global syscall_entry
syscall_entry:
    push rax
    push rbx
    push rcx
    push rdx
    push rsi
    push rdi
    push rbp
    push r8
    push r9
    push r10
    push r11
    push r12
    push r13
    push r14
    push r15
    mov rdi, rsp
    call {dispatch}
    pop r15
    pop r14
    pop r13
    pop r12
    pop r11
    pop r10
    pop r9
    pop r8
    pop rbp
    pop rdi
    pop rsi
    pop rdx
    pop rcx
    pop rbx
    pop rax
    iretq
"#,
    dispatch = sym syscall_dispatch,
);

extern "C" {
    pub fn syscall_entry();
}

extern "C" fn syscall_dispatch(frame: *mut SyscallFrame) {
    // Safety: `frame` was constructed by `syscall_entry`'s own prologue,
    // immediately before this call, pointing at a live, exclusively-owned
    // region of the current task's own kernel stack -- valid for exactly
    // the duration of this call, and nothing else touches it concurrently
    // (single-core kernel, interrupts disabled for the whole interrupt-gate
    // duration).
    let frame = unsafe { &mut *frame };
    let result = dispatch(frame.rax, frame.rdi, frame.rsi, frame.rdx);
    frame.rax = result as u64;
}

/// `_a3` is unused by every syscall implemented so far -- kept in the
/// dispatch signature (matching the ABI's documented three-argument shape)
/// rather than dropped, so adding a syscall that needs it later doesn't
/// require threading a new parameter through here.
fn dispatch(num: u64, a1: u64, a2: u64, _a3: u64) -> i64 {
    match num {
        SYS_EXIT => sys_exit(a1 as i32),
        SYS_WRITE => sys_write(a1, a2),
        SYS_YIELD => sys_yield(),
        SYS_GETPID => sys_getpid(),
        _ => {
            // Exactly the "unknown syscall numbers must fail safely"
            // requirement: logged for visibility, a plain error return,
            // the calling process is not touched otherwise and keeps
            // running normally afterward.
            crate::serial_println!("syscall: rejected unknown syscall number {}", num);
            -1
        }
    }
}

fn sys_exit(code: i32) -> ! {
    crate::serial_println!(
        "syscall: EXIT(code={}) from task {:?} -- terminating",
        code,
        task::current_task_id()
    );
    task::exit_with_code(code)
}

fn sys_write(ptr: u64, len: u64) -> i64 {
    if len as usize > MAX_WRITE_LEN {
        crate::serial_println!(
            "syscall: WRITE rejected -- len {} exceeds max {}",
            len,
            MAX_WRITE_LEN
        );
        return -1;
    }

    match task::copy_from_current_user(ptr, len as usize) {
        Some(bytes) => {
            // This syscall's contract is "write these bytes," not "write
            // valid UTF-8" -- invalid sequences are replaced rather than
            // rejected, purely so the fallback still prints *something*
            // readable on serial rather than requiring a byte-for-byte
            // faithful (but harder to eyeball) hex dump.
            let text = core::str::from_utf8(&bytes).unwrap_or("<non-utf8 write payload>");
            crate::serial_print!("{}", text);
            bytes.len() as i64
        }
        None => {
            crate::serial_println!(
                "syscall: WRITE rejected -- invalid user pointer/range (ptr={:#x}, len={})",
                ptr,
                len
            );
            -1
        }
    }
}

fn sys_yield() -> i64 {
    task::yield_now();
    0
}

fn sys_getpid() -> i64 {
    task::current_task_id().map(i64::from).unwrap_or(-1)
}
