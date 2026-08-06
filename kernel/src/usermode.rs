//! Low-level Ring 3 entry primitive.
//!
//! This module used to also own a pair of demo payloads that proved the
//! privilege boundary itself was real (Phase 4's first milestone -- see
//! git history and `ARCHITECTURE.md`'s "Ring 3 foundation" section for that
//! evidence). That demo is gone now, superseded by the real thing: `task.rs`
//! spawns genuine user processes loaded from real ELF64 files (`elf.rs`),
//! each with its own private address space (`paging::AddressSpace`) and
//! its own syscall-driven way back into Ring 0 (`syscall.rs`). What's left
//! here is only the one truly low-level, address-space-agnostic mechanism
//! every one of those processes still needs at the exact moment it first
//! starts running: the `iretq` that actually drops CPL to 3.

core::arch::global_asm!(
    r#"
.global enter_ring3
enter_ring3:
    // SysV args: rdi=entry, rsi=user_stack_top, rdx=code_selector, rcx=data_selector.
    // iretq pops RIP, CS, RFLAGS, RSP, SS in that order, so they are
    // pushed here in reverse. RFLAGS = 0x202: IF=1 (interrupts stay on in
    // Ring 3) and IOPL=00, so I/O port instructions fault from Ring 3 like
    // every other privileged instruction.
    push rcx
    push rsi
    push 0x202
    push rdx
    push rdi
    iretq
"#
);

extern "C" {
    /// Build the five-word `iretq` frame by hand and drop CPL to 3 at
    /// `entry`, with `SS:RSP` set to `data_selector:user_stack_top` and
    /// `CS:RIP` to `code_selector:entry`.
    ///
    /// Never returns: ends in `iretq`, not `ret`. The caller's kernel-mode
    /// call stack below this point is abandoned -- the only way execution
    /// continues in Ring 0 afterward is a fresh trap (syscall or fault)
    /// landing on whatever the TSS's RSP0 currently points to (see
    /// `gdt::set_kernel_stack`, kept current by `task.rs` on every
    /// scheduler switch), not this call frame resuming.
    ///
    /// # Safety
    /// `entry` and `user_stack_top` must be addresses mapped `PRESENT |
    /// USER_ACCESSIBLE` (with `user_stack_top` additionally `WRITABLE`) in
    /// whichever address space is the currently loaded CR3 at the moment
    /// this runs -- typically the calling process's own, switched in by
    /// the scheduler just before resuming it (see `task.rs`).
    /// `code_selector`/`data_selector` must be `gdt::user_code_selector()`/
    /// `gdt::user_data_selector()`'s raw values.
    pub fn enter_ring3(
        entry: u64,
        user_stack_top: u64,
        code_selector: u64,
        data_selector: u64,
    ) -> !;
}
