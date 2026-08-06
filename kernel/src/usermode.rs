//! Ring 3 foundation (Phase 4, first milestone).
//!
//! This module proves the kernel can genuinely drop the CPU to CPL=3 and
//! get back, and that a privileged instruction executed there traps safely
//! instead of corrupting anything. It deliberately does **not** attempt a
//! real process model: there is no ELF loader into user memory, no
//! per-process address space (the two demo payloads below run under the
//! same page tables as the kernel, in two pages explicitly marked
//! `USER_ACCESSIBLE`, nothing else), and no syscall ABI -- those are later
//! Phase 4 work. What exists here is the minimum needed to demonstrate the
//! privilege boundary itself is real:
//!
//! - `gdt.rs` supplies Ring 3 code/data GDT descriptors and a TSS whose
//!   RSP0 can be pointed at a specific kernel stack at runtime.
//! - `enter_ring3` (hand-written asm below) builds the `iretq` frame that
//!   drops CPL to 3 and jumps into one of the two payloads copied into a
//!   dedicated user-accessible code page.
//! - The "clean" payload runs a couple of harmless instructions and then
//!   executes `int 0x80`, a vector deliberately registered with DPL=3 (see
//!   `interrupts.rs`) as the one gate Ring 3 is allowed to call into Ring 0
//!   through. Its handler logs proof of where the trap came from (CS/RIP
//!   with CS's RPL bits intact) and ends the demo task.
//! - The "fault" payload executes `cli`, a privileged instruction, which
//!   the CPU rejects with a General Protection Fault purely as a hardware
//!   consequence of CPL=3 -- nothing in this module or `interrupts.rs`
//!   special-cases *which* instruction faults. The GPF handler recognizes
//!   the fault's origin was Ring 3 (`interrupts.rs`) and kills only the
//!   offending task rather than halting the whole kernel, exactly as it
//!   already halts unconditionally for a Ring-0-origin GPF.
//!
//! ## Why only one Ring-3-capable task at a time
//!
//! The TSS has exactly one RSP0 slot -- there is one answer to "which
//! kernel stack does the next Ring 3 -> Ring 0 trap land on" at any given
//! moment (see `gdt::set_kernel_stack`). A full process model swaps RSP0 on
//! every context switch, so each task's own trap always lands on that
//! task's own stack even under arbitrary preemption. That's real
//! scheduler-integration work, explicitly out of scope for this milestone.
//! Instead, each demo task sets RSP0 to its own kernel stack top once, at
//! its own start, and the shell command that drives this module (`shell.rs`
//! `usermode`) never has two such tasks in flight at once. This is safe and
//! correct for exactly the reason it's simple: no other task (shell, idle,
//! heartbeat) ever runs Ring 3 code, so RSP0's value is simply never
//! consulted while any of them is current.
//!
//! ## Known simplifications (deliberate, not oversights)
//!
//! - The user code page is mapped `WRITABLE` as well as executable, purely
//!   so the kernel-side `copy_nonoverlapping` below can populate it without
//!   an extra map-writable/copy/remap-read-only-and-exec dance. A real
//!   process loader would not do this. The user stack page similarly has
//!   no `NO_EXECUTE`, since wiring up `EFER.NXE` is unrelated hardening
//!   work, not part of this milestone.
//! - Both payloads and the user stack live in the *kernel's own* page
//!   tables, just with the `USER_ACCESSIBLE` bit set on exactly these two
//!   pages -- every other kernel mapping (heap, kernel stacks, kernel
//!   image) was mapped without that bit (see `memory.rs`), so it remains
//!   unreachable from Ring 3 by construction; a Ring 3 access to any of it
//!   would take a page fault, not a demonstration of isolation this
//!   milestone claims.

use core::sync::atomic::{AtomicBool, Ordering};

use x86_64::structures::paging::{Page, PageTableFlags, Size4KiB};
use x86_64::VirtAddr;

use crate::{gdt, paging, task};

/// Chosen the same way `memory.rs` picked `HEAP_START`: a memorable,
/// page-aligned address far from the bootloader's dynamic kernel/physical-
/// memory mappings and from the heap, so it can't alias either.
const USER_CODE_ADDR: u64 = 0x_5555_5555_0000;
const USER_STACK_ADDR: u64 = 0x_6666_6666_0000;
const USER_STACK_SIZE: u64 = 4096;

/// Offset within the code page of each payload -- both are a handful of
/// bytes, well clear of each other and of the page boundary.
const CLEAN_OFFSET: u64 = 0x000;
const FAULT_OFFSET: u64 = 0x100;

// Safety: both payloads are plain, position-independent instruction bytes
// (no relocations, no data references) living in the kernel's own .text --
// they are never executed at their kernel-image address, only read as raw
// bytes and copied into the user-accessible code page by `ensure_mapped`
// below. `enter_ring3` is genuinely executed from Ring 0: it only builds an
// `iretq` frame and jumps to whatever `entry` address the caller (this
// module's demo entry points) passes, which is always inside that same
// user page.
core::arch::global_asm!(
    r#"
.global __usermode_clean_start
.global __usermode_clean_end
__usermode_clean_start:
    mov rax, 0xC0FFEE
    inc rax
    int 0x80
1:
    jmp 1b
__usermode_clean_end:

.global __usermode_fault_start
.global __usermode_fault_end
__usermode_fault_start:
    cli
2:
    jmp 2b
__usermode_fault_end:

.global enter_ring3
enter_ring3:
    // SysV args: rdi=entry, rsi=user_stack_top, rdx=code_selector, rcx=data_selector.
    // iretq pops RIP, CS, RFLAGS, RSP, SS in that order, so they are
    // pushed here in reverse.
    push rcx
    push rsi
    push 0x202
    push rdx
    push rdi
    iretq
"#
);

extern "C" {
    static __usermode_clean_start: u8;
    static __usermode_clean_end: u8;
    static __usermode_fault_start: u8;
    static __usermode_fault_end: u8;

    /// Never returns: ends in `iretq`, not `ret`. See the module docs for
    /// why the only way execution continues in Ring 0 afterward is through
    /// the `int 0x80` handler or a fault handler, not this call returning.
    fn enter_ring3(entry: u64, user_stack_top: u64, code_selector: u64, data_selector: u64) -> !;
}

static MAPPED: AtomicBool = AtomicBool::new(false);

/// Map the user code + stack pages and copy both payloads in, exactly once
/// -- safe to call from every demo entry point unconditionally.
fn ensure_mapped() {
    if MAPPED.swap(true, Ordering::AcqRel) {
        return;
    }

    let code_page: Page<Size4KiB> = Page::containing_address(VirtAddr::new(USER_CODE_ADDR));
    let code_flags =
        PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;
    paging::map_user_page(code_page, code_flags).expect("usermode: failed to map user code page");

    let stack_page: Page<Size4KiB> =
        Page::containing_address(VirtAddr::new(USER_STACK_ADDR - USER_STACK_SIZE));
    let stack_flags =
        PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;
    paging::map_user_page(stack_page, stack_flags)
        .expect("usermode: failed to map user stack page");

    // Safety: `code_page` was just mapped PRESENT|WRITABLE above, exclusively
    // for this purpose (nothing else has ever pointed a mapping at
    // USER_CODE_ADDR); the two payload symbols are `static` data inside the
    // kernel's own image, always readable from Ring 0. The two copies land
    // at CLEAN_OFFSET/FAULT_OFFSET, well within one page and clear of each
    // other by construction (each payload is a handful of bytes).
    unsafe {
        let clean_start = &__usermode_clean_start as *const u8;
        let clean_end = &__usermode_clean_end as *const u8;
        let clean_len = clean_end as usize - clean_start as usize;
        core::ptr::copy_nonoverlapping(
            clean_start,
            (USER_CODE_ADDR + CLEAN_OFFSET) as *mut u8,
            clean_len,
        );

        let fault_start = &__usermode_fault_start as *const u8;
        let fault_end = &__usermode_fault_end as *const u8;
        let fault_len = fault_end as usize - fault_start as usize;
        core::ptr::copy_nonoverlapping(
            fault_start,
            (USER_CODE_ADDR + FAULT_OFFSET) as *mut u8,
            fault_len,
        );
    }

    crate::serial_println!(
        "usermode: mapped user code page {:#x} and stack page (top {:#x}), payloads copied",
        USER_CODE_ADDR,
        USER_STACK_ADDR
    );
}

/// Register the current task as the Ring 3 trap landing pad (see the
/// module docs on why this must be the only Ring-3-capable task running),
/// then drop CPL to 3 at `entry_offset` within the user code page. Never
/// returns -- the task's lifetime ends via `interrupts.rs`'s `int 0x80`
/// handler or GPF recovery path calling `task::exit()`.
fn run_demo(entry_offset: u64) -> ! {
    let stack_top =
        task::current_kernel_stack_top().expect("usermode demo must run as its own task");
    gdt::set_kernel_stack(VirtAddr::new(stack_top));

    ensure_mapped();

    crate::serial_println!(
        "usermode: entering Ring 3 at {:#x} (user stack top {:#x})",
        USER_CODE_ADDR + entry_offset,
        USER_STACK_ADDR
    );

    // Safety: both pages are mapped and populated by `ensure_mapped` above,
    // `USER_STACK_ADDR` is the top of a freshly mapped, otherwise-unused
    // page, and the selectors come from `gdt`'s own GDT entries.
    unsafe {
        enter_ring3(
            USER_CODE_ADDR + entry_offset,
            USER_STACK_ADDR,
            gdt::user_code_selector().0 as u64,
            gdt::user_data_selector().0 as u64,
        )
    }
}

/// Task entry point: enters Ring 3, runs a couple of instructions, then
/// traps back to Ring 0 via `int 0x80` -- the "does Ring 3 work, and can it
/// get back" half of this milestone.
pub fn demo_clean_entry() {
    run_demo(CLEAN_OFFSET)
}

/// Task entry point: enters Ring 3 and immediately executes `cli`, a
/// privileged instruction -- the "does a privileged operation trap safely
/// instead of corrupting the kernel" half of this milestone.
pub fn demo_fault_entry() {
    run_demo(FAULT_OFFSET)
}
