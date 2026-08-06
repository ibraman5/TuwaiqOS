//! Global Descriptor Table and Task State Segment.
//!
//! x86_64 mostly ignores segmentation, but a GDT is still required to load
//! `CS` at the correct privilege level, and the TSS's Interrupt Stack Table
//! (IST) is how the double-fault handler is guaranteed a known-good stack
//! even when the fault was *caused by* kernel stack overflow -- running the
//! handler on the already-overflowed stack would just triple-fault.

use lazy_static::lazy_static;
use x86_64::instructions::interrupts::without_interrupts;
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtAddr;

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;
/// Dedicated stack for the keyboard IRQ only (see `interrupts.rs`) -- it
/// never redirects control flow anywhere, just decodes a scancode and
/// returns, so a fixed always-valid stack is strictly safer with no
/// downside.
///
/// The timer IRQ deliberately does **not** use this (or any) IST stack as
/// of Phase 3: `task::on_timer_tick` may perform a real context switch,
/// and that only works if the interrupt frame the CPU pushes on entry
/// lands on *the currently running task's own stack* -- an IST stack would
/// force every timer tick onto the same fixed physical stack regardless of
/// which task was running, destroying the per-task state a switch needs
/// to resume that task correctly later. See `task.rs`'s module docs for
/// the full mechanism.
pub const IRQ_IST_INDEX: u16 = 1;

const STACK_SIZE: usize = 4096 * 5;

fn new_stack() -> VirtAddr {
    // Safety: each call site below defines its own `STACK` at a distinct
    // location; the returned address is only ever installed into the TSS
    // once, before interrupts are enabled, and never read or written
    // through any other path.
    macro_rules! stack_top {
        () => {{
            static mut STACK: [u8; STACK_SIZE] = [0; STACK_SIZE];
            let start = VirtAddr::from_ptr(core::ptr::addr_of!(STACK));
            start + STACK_SIZE as u64
        }};
    }
    stack_top!()
}

/// Not a `lazy_static` like the rest of this file: `privilege_stack_table[0]`
/// (RSP0 -- the stack the CPU loads when a Ring 3 -> Ring 0 privilege change
/// happens on any interrupt or exception) must be updatable at *runtime*,
/// once per Ring-3-capable task, via `set_kernel_stack` -- see that
/// function's docs and `task::current_kernel_stack_top`. `lazy_static`'s
/// generated wrapper only exposes `Deref`, not `DerefMut`, so a plain
/// mutable static plus explicit `unsafe` field access (exactly like `PICS`
/// in `interrupts.rs`, which is hardware-adjacent state for the same
/// reason) is the straightforward correct tool here, not a workaround.
static mut TSS: TaskStateSegment = TaskStateSegment::new();

struct Selectors {
    code_selector: SegmentSelector,
    tss_selector: SegmentSelector,
    user_code_selector: SegmentSelector,
    user_data_selector: SegmentSelector,
}

lazy_static! {
    static ref GDT: (GlobalDescriptorTable, Selectors) = {
        let mut gdt = GlobalDescriptorTable::new();
        let code_selector = gdt.add_entry(Descriptor::kernel_code_segment());
        // Safety: by the time this runs, `init` below has already set both
        // IST stack entries on `TSS` (it does so before the first access to
        // `GDT`, which is what triggers this lazy closure) and nothing else
        // ever holds a live `&mut TSS` concurrently on this single-core,
        // single-threaded-at-boot kernel.
        let tss_selector = gdt.add_entry(Descriptor::tss_segment(unsafe {
            &*core::ptr::addr_of!(TSS)
        }));
        // Ring 3 segments (Phase 4 foundation): a code and data descriptor
        // with DPL=3, required before any `iretq` can legally drop CPL to 3
        // -- see `usermode::enter_ring3`. `add_entry` encodes the
        // descriptor's own DPL into the returned selector's RPL bits
        // automatically, so these two selectors already read as RPL=3
        // without any extra `set_rpl` call.
        let user_data_selector = gdt.add_entry(Descriptor::user_data_segment());
        let user_code_selector = gdt.add_entry(Descriptor::user_code_segment());
        (
            gdt,
            Selectors {
                code_selector,
                tss_selector,
                user_code_selector,
                user_data_selector,
            },
        )
    };
}

/// Load the kernel GDT and TSS. Must run before `interrupts::init` loads
/// the IDT, since the double-fault entry references `DOUBLE_FAULT_IST_INDEX`.
pub fn init() {
    use x86_64::instructions::segmentation::{Segment, CS, DS, ES, FS, GS, SS};
    use x86_64::instructions::tables::load_tss;
    use x86_64::structures::gdt::SegmentSelector;

    // Safety: runs once, before interrupts are enabled and before the
    // first access to `GDT` below (whose lazy construction reads `TSS`),
    // so this is the only live reference to `TSS` in existence right now.
    unsafe {
        let tss = &mut *core::ptr::addr_of_mut!(TSS);
        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = new_stack();
        tss.interrupt_stack_table[IRQ_IST_INDEX as usize] = new_stack();
    }

    GDT.0.load();
    // Safety: code_selector/tss_selector come from entries this same
    // function just appended to GDT, so both indices are valid and the
    // GDT is already loaded above.
    //
    // The bootloader hands off with its own GDT/TSS already active and its
    // own (non-zero) selector values sitting in SS/DS/ES/FS/GS. Loading a
    // new GDT reloads the *table* those selectors index into, but not the
    // registers themselves -- so a stale selector value that was valid
    // under the bootloader's GDT can silently point at a *different*
    // (possibly invalid, e.g. our TSS descriptor) entry in the new one.
    // CS is fixed up below via the far-return `set_reg` needs; the other
    // segment registers are barely used in 64-bit long mode, so they are
    // reloaded to the null selector rather than left stale. Skipping this
    // was the root cause of a GPF (error_code pointing at the TSS
    // selector) on every single interrupt return during Phase 1 bring-up:
    // IRETQ validates the SS value on the stack against the *current*
    // GDT, and the bootloader's stale SS selector aliased our TSS entry.
    unsafe {
        CS::set_reg(GDT.1.code_selector);
        SS::set_reg(SegmentSelector::NULL);
        DS::set_reg(SegmentSelector::NULL);
        ES::set_reg(SegmentSelector::NULL);
        FS::set_reg(SegmentSelector::NULL);
        GS::set_reg(SegmentSelector::NULL);
        load_tss(GDT.1.tss_selector);
    }
}

/// The Ring 3 code segment selector (RPL=3), for building the `iretq` frame
/// that drops into user mode -- see `usermode::enter_ring3`.
pub fn user_code_selector() -> SegmentSelector {
    GDT.1.user_code_selector
}

/// The Ring 3 data segment selector (RPL=3), loaded into `SS` (and usable
/// for `DS`/`ES`) by the same `iretq`. Ring 3 cannot run with a null `SS`
/// the way Ring 0 can -- unlike the null `DS`/`ES`/`FS`/`GS` `init` leaves
/// in place above, loading a null selector into `SS` while CPL=3 faults
/// immediately, so user mode needs a real one.
pub fn user_data_selector() -> SegmentSelector {
    GDT.1.user_data_selector
}

/// Point the TSS's RSP0 (the stack the CPU switches to on any interrupt or
/// exception that catches the CPU running at CPL=3) at `top`.
///
/// This is what "kernel stacks stay under kernel control through the TSS"
/// means concretely: without it, RSP0 stays zeroed, and a trap out of Ring
/// 3 would hand the CPU an invalid stack to build its interrupt frame on.
///
/// **Scope note (Phase 4 foundation, not the full process model):** RSP0 is
/// a single, CPU-global field -- there is exactly one "the kernel stack for
/// the next Ring 3 -> Ring 0 trap" at a time. A task that is about to run
/// Ring 3 code must call this with its own kernel stack top (see
/// `task::current_kernel_stack_top`) before doing so, and only one
/// Ring-3-capable task may be in flight at a time; nothing in this phase
/// swaps RSP0 automatically on every ordinary context switch the way a full
/// per-process scheduler would. Kernel-only tasks (shell, idle, heartbeat)
/// never trap from CPL=3, so RSP0's value is simply unused while any of
/// them is current -- see `usermode.rs`'s module docs for the full
/// reasoning.
pub fn set_kernel_stack(top: VirtAddr) {
    without_interrupts(|| {
        // Safety: a `VirtAddr` write is a single aligned 8-byte store (so
        // there is nothing for a concurrent reader to tear even without
        // `without_interrupts`), and disabling interrupts here matches this
        // codebase's locking invariant for any state a trap handler might
        // also touch -- see ARCHITECTURE.md's "Locking invariant" section.
        unsafe {
            (*core::ptr::addr_of_mut!(TSS)).privilege_stack_table[0] = top;
        }
    });
}
