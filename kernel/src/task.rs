//! Preemptive task scheduler (Phase 3).
//!
//! Replaces the v0.5/Phase-1-2 decorative two-row task table with real
//! execution: each task has its own kernel stack and a saved CPU context,
//! `context_switch` (hand-written assembly, see below) actually transfers
//! control between them, and the timer interrupt drives preemption --
//! `ps`/`taskinfo`/`kill` now report and act on genuine scheduler state.
//!
//! ## How the context switch works
//!
//! `context_switch(old_rsp: *mut u64, new_rsp: u64)` looks like an ordinary
//! `extern "C"` function call from the Rust side. That's deliberate: the
//! System V calling convention already specifies which registers a normal
//! function call must preserve (`rbx`, `rbp`, `r12`-`r15`, and the stack
//! pointer itself) and which it's free to clobber (everything else,
//! including the floating-point/SSE registers -- SysV has no callee-saved
//! XMM registers at all). `context_switch` only needs to save/restore
//! exactly the callee-saved set to be a *correct* function call from the
//! compiler's point of view; it doesn't need to know or care what the
//! caller was doing with any other register, because the compiler already
//! assumes a normal call might clobber those.
//!
//! The trick is what happens between the push and the pop: it switches
//! `rsp` to a *different* task's stack in the middle, so the `ret` at the
//! end returns not to the caller, but to wherever *that* task last called
//! `context_switch` from (or, for a brand new task, to a small trampoline
//! that starts it running for the first time). Each suspended task's own
//! call chain -- including, critically, the interrupt frame the CPU pushed
//! when the timer fired -- sits dormant on that task's own stack until the
//! scheduler switches back to it, at which point it unwinds completely
//! normally: back up through `schedule`, back up through the timer ISR,
//! and out through a perfectly ordinary `iretq`.
//!
//! This is why the timer interrupt (unlike keyboard) can no longer use a
//! dedicated IST stack (see `gdt.rs`): IST would force every timer
//! interrupt onto the *same* physical stack regardless of which task was
//! running, destroying the very thing that makes resuming a task later
//! possible.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use spin::Mutex;
use x86_64::structures::paging::{Page, PageTableFlags, PhysFrame, Size4KiB};
use x86_64::VirtAddr;

use crate::{elf, gdt, paging};

/// Per-task kernel stack size. Generous relative to what `idle`/`heartbeat`
/// actually need, since it must also absorb however many Rust call frames
/// are active (scheduler + interrupt handler) at the moment a task is
/// preempted.
const STACK_SIZE: usize = 32 * 1024;

/// Bytes needed for the fake initial stack frame `spawn` builds: six
/// callee-saved registers plus a return address, matching exactly what
/// `context_switch`'s epilogue pops before its `ret`.
const INITIAL_FRAME_SIZE: usize = 7 * 8;

/// A task is preempted after this many timer ticks (100 Hz -- see
/// `interrupts.rs` -- so 5 ticks is a 50 ms time slice).
const TIME_SLICE_TICKS: u64 = 5;

/// Lifecycle state of a kernel task.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TaskState {
    Ready,
    Running,
    Blocked,
    Terminated,
}

impl TaskState {
    fn label(self) -> &'static str {
        match self {
            TaskState::Ready => "Ready",
            TaskState::Running => "Running",
            TaskState::Blocked => "Blocked",
            TaskState::Terminated => "Terminated",
        }
    }
}

/// Whether a task runs kernel code at Ring 0 only, or is a genuine user
/// process with its own private address space and Ring 3 execution -- see
/// `Tcb::process`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Privilege {
    Kernel,
    User,
}

impl Privilege {
    fn label(self) -> &'static str {
        match self {
            Privilege::Kernel => "kernel",
            Privilege::User => "user",
        }
    }
}

/// Lightweight snapshot returned by `list`/`info` -- deliberately does not
/// expose a task's stack, saved context, or address space, only what
/// `ps`/`taskinfo` need.
#[derive(Clone)]
pub struct Task {
    pub id: u32,
    pub name: String,
    pub state: TaskState,
    pub privilege: Privilege,
    /// `Some` once a user process has exited (via the `exit` syscall or
    /// fault-isolation recovery -- see `syscall.rs` / `interrupts.rs`).
    /// Always `None` for kernel-only tasks.
    pub exit_code: Option<i32>,
}

/// The real task control block. Not exposed outside this module: callers
/// get `Task` snapshots instead (see `list`/`info`).
struct Tcb {
    id: u32,
    name: String,
    state: TaskState,
    /// `None` only for the boot task (id 1, "shell"): it runs on the stack
    /// the bootloader handed the kernel, which this module doesn't own and
    /// must not free. Never read directly -- its only job is to keep the
    /// allocation (and therefore `saved_rsp`, which points into it) alive
    /// for as long as this `Tcb` exists; dropping a `Tcb` frees it.
    #[allow(dead_code)]
    stack: Option<Box<[u8; STACK_SIZE]>>,
    /// Saved stack pointer. Meaningless while this task is `Running` (the
    /// real value lives in the CPU's `rsp` register); valid for every other
    /// state.
    saved_rsp: u64,
    /// Absolute tick count (see `interrupts::ticks`) at which a `Blocked`
    /// task should become `Ready` again. Zero means "not sleeping".
    wake_at_tick: u64,
    /// Top of this task's own kernel stack (16-byte aligned), or 0 for the
    /// boot task (id 1, "shell"), which has no stack this module allocated
    /// (see `stack` above). This is what `schedule()` now writes into the
    /// TSS's RSP0 on *every* switch (see `gdt::set_kernel_stack`) -- the
    /// mechanism that lets any number of user processes safely interleave
    /// under preemption, each trapping from Ring 3 onto its own kernel
    /// stack rather than a single shared one.
    kernel_stack_top: u64,
    /// Present only for genuine user processes -- Ring 3 privilege, a
    /// private address space, and (once set) an exit status. `None` for
    /// kernel-only tasks (shell, idle, heartbeat), which run entirely at
    /// Ring 0 under the shared kernel address space.
    process: Option<ProcessState>,
}

/// A user process's private state: its own address space and, once it has
/// run, an exit status. See `spawn_user_process`.
struct ProcessState {
    /// `None` only in the brief window between a process being marked
    /// `Terminated` (by itself, via `exit`, or by `kill`) and the point at
    /// which `Scheduler::prepare_switch`/`kill` actually reclaims it --
    /// see `schedule()`'s and `kill()`'s docs on why that reclamation is
    /// sequenced the way it is (CR3 must move off an address space before
    /// its frames can be safely freed).
    address_space: Option<paging::AddressSpace>,
    entry_point: u64,
    user_stack_top: u64,
    exit_code: Option<i32>,
}

struct Scheduler {
    tasks: Vec<Box<Tcb>>,
    current: usize,
}

/// Everything `schedule()` needs to actually perform a switch, computed
/// while `SCHEDULER`'s lock is held (`Scheduler::prepare_switch`) so the
/// lock can be released before any of it happens -- `context_switch` may
/// not return to that call frame for an arbitrarily long time.
struct SwitchPlan {
    old_rsp_ptr: *mut u64,
    new_rsp: u64,
    /// CR3 to load for the incoming task: its own `AddressSpace` if it's a
    /// user process, otherwise the kernel's permanent root.
    new_cr3: PhysFrame,
    /// RSP0 to install into the TSS for the incoming task, or 0 for the
    /// one task that owns no module-allocated stack (id 1, "shell") --
    /// see `Tcb::kernel_stack_top`'s docs on why 0 there is safe to leave
    /// untouched rather than meaningful.
    new_kernel_stack_top: u64,
    /// An outgoing, just-terminated task's address space, taken out here
    /// so `schedule()` can free it -- but only *after* `new_cr3` above has
    /// been loaded (see `schedule()`).
    reclaim: Option<paging::AddressSpace>,
}

impl Scheduler {
    /// Decide whether a switch is needed and, if so, everything about it
    /// -- but do not perform any of it (see `SwitchPlan`'s docs).
    fn prepare_switch(&mut self) -> Option<SwitchPlan> {
        let n = self.tasks.len();
        if n < 2 {
            return None;
        }

        if self.tasks[self.current].state == TaskState::Running {
            self.tasks[self.current].state = TaskState::Ready;
        }

        let mut next = self.current;
        for offset in 1..=n {
            let idx = (self.current + offset) % n;
            if self.tasks[idx].state == TaskState::Ready {
                next = idx;
                break;
            }
        }

        if next == self.current {
            // Nothing else runnable; keep going unless this task just
            // terminated itself (see `exit`), in which case there is
            // truly nothing left to do but let the caller's fallback halt.
            if self.tasks[self.current].state != TaskState::Terminated {
                self.tasks[self.current].state = TaskState::Running;
            }
            return None;
        }

        // Safe to take ownership of the outgoing task's address space here
        // (nothing else can reach it once we're past this point), but not
        // safe to actually free its frames until `new_cr3` below has
        // genuinely been loaded -- CR3 may still be pointing at it.
        let reclaim = if self.tasks[self.current].state == TaskState::Terminated {
            self.tasks[self.current]
                .process
                .as_mut()
                .and_then(|p| p.address_space.take())
        } else {
            None
        };

        let old_ptr: *mut u64 = &mut self.tasks[self.current].saved_rsp;
        let new_val = self.tasks[next].saved_rsp;
        let new_cr3 = self.tasks[next]
            .process
            .as_ref()
            .and_then(|p| p.address_space.as_ref())
            .map(paging::AddressSpace::pml4_frame)
            .unwrap_or_else(|| {
                paging::kernel_pml4_frame().expect("kernel pml4 frame not recorded")
            });
        let new_kernel_stack_top = self.tasks[next].kernel_stack_top;

        self.tasks[next].state = TaskState::Running;
        self.current = next;

        Some(SwitchPlan {
            old_rsp_ptr: old_ptr,
            new_rsp: new_val,
            new_cr3,
            new_kernel_stack_top,
            reclaim,
        })
    }
}

static SCHEDULER: Mutex<Option<Scheduler>> = Mutex::new(None);
static NEXT_ID: AtomicU32 = AtomicU32::new(3); // 1 = shell, 2 = idle
static TICKS_SINCE_SWITCH: AtomicU64 = AtomicU64::new(0);

/// The only sanctioned way to touch `SCHEDULER`. Every call site used to
/// take `SCHEDULER.lock()` directly; several (`init`, `spawn`, `list`,
/// `info`, `kill`) did so with interrupts still enabled. On a single-core
/// kernel that is a real interrupt-reentrancy deadlock, not a theoretical
/// one: `on_timer_tick` (called from the timer ISR -- see
/// `interrupts::timer_interrupt_handler`) also locks `SCHEDULER`, and
/// `spin::Mutex` is not reentrant. If the timer fires while, say, `kill`
/// holds the lock, the ISR spins forever waiting for a lock owned by the
/// exact context it just interrupted -- which can never run again to
/// release it, because the CPU is stuck spinning in the ISR instead.
/// Routing every access through this one function makes that mistake
/// structurally impossible to reintroduce at a new call site.
///
/// Safe to call from interrupt context too: `without_interrupts` only
/// disables/restores the flag it itself changed, so nesting (this being
/// called from `on_timer_tick`, which is already running with IF clear)
/// is a correct no-op rather than a bug.
fn with_scheduler<F, R>(f: F) -> R
where
    F: FnOnce(&mut Option<Scheduler>) -> R,
{
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut guard = SCHEDULER.lock();
        f(&mut guard)
    })
}

// Safety: `context_switch` only touches the callee-saved registers SysV
// requires a normal `extern "C"` call to preserve (see the module docs),
// plus `rsp` itself, which is the whole point. `task_trampoline` is the
// landing point the very first time a freshly spawned task runs (see
// `spawn`): it reads the entry-point function pointer `spawn` stashed in
// `r15`'s saved slot, calls it, and falls through to `task_exit_trampoline`
// if it ever returns.
core::arch::global_asm!(
    r#"
.global context_switch
context_switch:
    push rbp
    push rbx
    push r12
    push r13
    push r14
    push r15
    mov [rdi], rsp
    mov rsp, rsi
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbx
    pop rbp
    ret

.global task_trampoline
task_trampoline:
    sti
    call r15
    call {task_exit}
2:
    hlt
    jmp 2b

.global user_task_trampoline
user_task_trampoline:
    sti
    call {user_entry}
    call {task_exit}
3:
    hlt
    jmp 3b
"#,
    task_exit = sym task_exit_trampoline,
    user_entry = sym rust_user_entry,
);

extern "C" fn task_exit_trampoline() {
    exit();
}

/// Landing point for `user_task_trampoline`: reads back this now-running
/// task's own entry point and user stack top (stashed in its `Tcb` by
/// `spawn_user_process`, since -- unlike the plain kernel `task_trampoline`
/// -- a user task needs more than one word of context smuggled through the
/// fake initial stack frame) and drops to Ring 3.
extern "C" fn rust_user_entry() {
    let (entry_point, user_stack_top) =
        current_process_entry().expect("user_task_trampoline entered without process info");
    // Safety: `entry_point` was validated by `elf::load` to lie within the
    // permitted user address range and mapped `USER_ACCESSIBLE`;
    // `user_stack_top` is the top of a dedicated `WRITABLE | USER_ACCESSIBLE`
    // stack range in this same process's address space (`spawn_user_process`).
    // `schedule()` already loaded this process's own CR3 and this task's own
    // RSP0 before resuming it, so both addresses resolve correctly and any
    // trap lands on the right kernel stack.
    unsafe {
        crate::usermode::enter_ring3(
            entry_point,
            user_stack_top,
            gdt::user_code_selector().0 as u64,
            gdt::user_data_selector().0 as u64,
        )
    }
}

extern "C" {
    fn context_switch(old_rsp: *mut u64, new_rsp: u64);
    fn task_trampoline();
    fn user_task_trampoline();
}

/// Create the built-in tasks (`shell`, `idle`) and the `heartbeat` demo
/// task before the shell starts, then bring the scheduler online.
pub fn init() {
    let shell = Tcb {
        id: 1,
        name: String::from("shell"),
        state: TaskState::Running,
        stack: None,
        saved_rsp: 0,
        wake_at_tick: 0,
        kernel_stack_top: 0,
        process: None,
    };
    let idle = new_tcb(2, "idle", idle_entry);

    let mut sched = Scheduler {
        tasks: Vec::new(),
        current: 0,
    };
    sched.tasks.push(Box::new(shell));
    sched.tasks.push(Box::new(idle));
    with_scheduler(|slot| *slot = Some(sched));

    // Concrete, observable proof that Phase 3 is real: this task sleeps
    // and logs a heartbeat over serial roughly once a second. Watching its
    // count climb in the serial log *while the shell stays interactively
    // responsive* is the demonstration the project's engineering rules
    // require before this phase counts as done -- not just "it compiles".
    spawn("heartbeat", heartbeat_entry);
}

fn new_tcb(id: u32, name: &str, entry: fn()) -> Tcb {
    let mut stack = Box::new([0u8; STACK_SIZE]);
    let stack_top = unsafe { stack.as_mut_ptr().add(STACK_SIZE) as usize };
    let aligned_top = stack_top & !0xF; // 16-byte align, matching the SysV stack ABI
    let mut sp = aligned_top;
    sp -= INITIAL_FRAME_SIZE;

    // Safety: `sp` was just computed from a freshly allocated, 16-byte
    // aligned, INITIAL_FRAME_SIZE-larger-than-needed buffer that nothing
    // else references yet, so writing these 7 words is in-bounds and
    // exclusive. The layout matches context_switch's pop order exactly:
    // whichever value ends up under `r15`'s slot is what `task_trampoline`
    // (see the asm above) will find in the real r15 register the moment
    // it starts running, which is how the entry point is smuggled through
    // a context switch that otherwise doesn't know anything about tasks.
    unsafe {
        let base = sp as *mut u64;
        base.add(0).write(entry as usize as u64); // -> r15 (entry fn ptr)
        base.add(1).write(0); // -> r14
        base.add(2).write(0); // -> r13
        base.add(3).write(0); // -> r12
        base.add(4).write(0); // -> rbx
        base.add(5).write(0); // -> rbp
        base.add(6).write(task_trampoline as *const () as u64); // return address for `ret`
    }

    Tcb {
        id,
        name: String::from(name),
        state: TaskState::Ready,
        stack: Some(stack),
        saved_rsp: sp as u64,
        wake_at_tick: 0,
        kernel_stack_top: aligned_top as u64,
        process: None,
    }
}

/// Build the Tcb for a genuine user process: same kernel-stack setup as
/// `new_tcb`, except the fake initial frame lands in `user_task_trampoline`
/// (which drops to Ring 3 via `enter_ring3`) instead of `task_trampoline`
/// (which calls a plain kernel `fn()`), and `r15` goes unused -- the entry
/// point and user stack top live in `process.entry_point`/`user_stack_top`
/// instead, read back via `current_process_entry()` once this task is
/// actually running (see `rust_user_entry`).
fn new_user_tcb(
    id: u32,
    name: &str,
    address_space: paging::AddressSpace,
    entry_point: u64,
    user_stack_top: u64,
) -> Tcb {
    let mut stack = Box::new([0u8; STACK_SIZE]);
    let stack_top = unsafe { stack.as_mut_ptr().add(STACK_SIZE) as usize };
    let aligned_top = stack_top & !0xF;
    let mut sp = aligned_top;
    sp -= INITIAL_FRAME_SIZE;

    // Safety: see `new_tcb` -- identical reasoning, just landing in
    // `user_task_trampoline` and leaving r15 unused (0).
    unsafe {
        let base = sp as *mut u64;
        base.add(0).write(0); // -> r15 (unused)
        base.add(1).write(0); // -> r14
        base.add(2).write(0); // -> r13
        base.add(3).write(0); // -> r12
        base.add(4).write(0); // -> rbx
        base.add(5).write(0); // -> rbp
        base.add(6).write(user_task_trampoline as *const () as u64);
    }

    Tcb {
        id,
        name: String::from(name),
        state: TaskState::Ready,
        stack: Some(stack),
        saved_rsp: sp as u64,
        wake_at_tick: 0,
        kernel_stack_top: aligned_top as u64,
        process: Some(ProcessState {
            address_space: Some(address_space),
            entry_point,
            user_stack_top,
            exit_code: None,
        }),
    }
}

fn idle_entry() {
    loop {
        crate::interrupts::halt();
    }
}

fn heartbeat_entry() {
    let mut count: u64 = 0;
    loop {
        sleep_ticks(100); // ~1 second at the PIT's 100 Hz tick rate
        count += 1;
        crate::serial_println!("task heartbeat: beat #{}", count);
    }
}

/// Start a new task running `entry` from the beginning. Returns its id.
pub fn spawn(name: &str, entry: fn()) -> u32 {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let tcb = new_tcb(id, name, entry);
    with_scheduler(|slot| {
        let sched = slot.as_mut().expect("scheduler not initialized");
        sched.tasks.push(Box::new(tcb));
    });
    id
}

/// Called from `interrupts::timer_interrupt_handler` on every PIT tick.
/// Wakes any `Blocked` task whose sleep has elapsed, then preempts into
/// the scheduler once a full time slice has passed.
pub fn on_timer_tick() {
    let now = crate::interrupts::ticks();
    with_scheduler(|slot| {
        if let Some(sched) = slot.as_mut() {
            for task in sched.tasks.iter_mut() {
                if task.state == TaskState::Blocked
                    && task.wake_at_tick != 0
                    && now >= task.wake_at_tick
                {
                    task.state = TaskState::Ready;
                    task.wake_at_tick = 0;
                }
            }
        }
    });

    if TICKS_SINCE_SWITCH.fetch_add(1, Ordering::Relaxed) + 1 >= TIME_SLICE_TICKS {
        TICKS_SINCE_SWITCH.store(0, Ordering::Relaxed);
        schedule();
    }
}

/// Perform a scheduling decision and, if warranted, a real context switch
/// -- now including the CR3 and TSS-RSP0 half of that switch, which is
/// what makes any number of user processes safe under real preemption
/// (Phase 4): every switch loads the incoming task's own address space
/// (its private one if it's a user process, the kernel's shared one
/// otherwise) and its own kernel stack top into RSP0 *before* the
/// register/stack-pointer switch happens, so a Ring 3 -> Ring 0 trap for whichever
/// task ends up running always lands in the right place. Safe to call from
/// both interrupt context (the timer ISR already runs with interrupts
/// disabled) and normal context (`yield_now`/`sleep_ticks`/a syscall
/// below, where `without_interrupts` prevents a reentrant tick from
/// corrupting the switch in progress).
pub fn schedule() {
    // Note: this wraps strictly more than the `SCHEDULER` lock itself --
    // `context_switch` below must also run with interrupts continuously
    // disabled (a timer tick landing mid-switch, while `rsp` points
    // somewhere between two tasks' stacks, would be a genuine hazard), so
    // it cannot go through `with_scheduler` alone. `without_interrupts`
    // nests safely with the one inside `with_scheduler` (see its doc
    // comment), so this stays correct either way.
    x86_64::instructions::interrupts::without_interrupts(|| {
        let plan = with_scheduler(|slot| slot.as_mut().and_then(Scheduler::prepare_switch));
        if let Some(plan) = plan {
            // Safety: `new_cr3` is either the kernel's own permanent root
            // or a process's `AddressSpace` that is about to become (or
            // remain) the current task -- both are valid, fully populated
            // PML4s. This runs while still executing as the *outgoing*
            // task, on its own stack, which is safe: kernel mappings are
            // identical across every address space (see
            // `paging::new_address_space`), so nothing this function does
            // between here and `context_switch` below is affected by which
            // one is loaded.
            unsafe { paging::switch_to(plan.new_cr3) };

            // A `kernel_stack_top` of 0 only ever belongs to the boot task
            // (id 1, "shell"), which never runs Ring 3 code -- RSP0's value
            // is simply never consulted while it's current, so it's left
            // alone rather than overwritten with a meaningless 0.
            if plan.new_kernel_stack_top != 0 {
                gdt::set_kernel_stack(VirtAddr::new(plan.new_kernel_stack_top));
            }

            if let Some(space) = plan.reclaim {
                // Safety: `new_cr3` was just loaded above, so this
                // reclaimed address space's PML4 is provably no longer the
                // active CR3 -- see `free_address_space`'s contract.
                unsafe { paging::free_address_space(space) };
            }

            // Safety: `old_rsp_ptr` points at the currently-running task's
            // own `saved_rsp` field (a stable heap address behind
            // `Box<Tcb>`, unaffected by the `Vec` it lives in
            // reallocating), and `new_rsp` was populated either by a
            // previous `context_switch` call or by a fake initial frame
            // (`new_tcb`/`new_user_tcb`) -- either way, a valid stack
            // pointer for `context_switch` to resume from.
            unsafe { context_switch(plan.old_rsp_ptr, plan.new_rsp) };
        }
    });
}

/// Voluntarily give up the remaining time slice.
pub fn yield_now() {
    TICKS_SINCE_SWITCH.store(0, Ordering::Relaxed);
    schedule();
}

/// Block the current task until at least `ticks` PIT ticks have passed.
pub fn sleep_ticks(ticks: u64) {
    let wake_at = crate::interrupts::ticks() + ticks;
    with_scheduler(|slot| {
        if let Some(sched) = slot.as_mut() {
            let idx = sched.current;
            sched.tasks[idx].state = TaskState::Blocked;
            sched.tasks[idx].wake_at_tick = wake_at;
        }
    });
    schedule();
}

/// Terminate the current task with exit code 0. Never returns.
pub fn exit() -> ! {
    exit_with_code(0)
}

/// Terminate the current task, recording `code` as its exit status if it's
/// a user process (see `Task::exit_code`; a no-op for kernel-only tasks,
/// which have nowhere to show it). Never returns: the next `schedule()`
/// call switches away from it permanently (a `Terminated` task is never
/// chosen by `prepare_switch` again), and if it owned a private address
/// space, that same `schedule()` call frees it once CR3 has moved off it
/// (see `Scheduler::prepare_switch` / `schedule`'s docs).
pub fn exit_with_code(code: i32) -> ! {
    with_scheduler(|slot| {
        if let Some(sched) = slot.as_mut() {
            let idx = sched.current;
            sched.tasks[idx].state = TaskState::Terminated;
            if let Some(process) = sched.tasks[idx].process.as_mut() {
                process.exit_code = Some(code);
            }
        }
    });
    loop {
        schedule();
        // Only reachable if `schedule` found nothing else runnable, which
        // should not happen (`idle` never terminates) -- halt rather than
        // spin if it somehow does.
        x86_64::instructions::hlt();
    }
}

fn snapshot(t: &Tcb) -> Task {
    Task {
        id: t.id,
        name: t.name.clone(),
        state: t.state,
        privilege: if t.process.is_some() {
            Privilege::User
        } else {
            Privilege::Kernel
        },
        exit_code: t.process.as_ref().and_then(|p| p.exit_code),
    }
}

/// List all tasks for the `ps` command -- genuine scheduler state, not a
/// static table.
pub fn list() -> Result<Vec<Task>, &'static str> {
    with_scheduler(|slot| {
        let sched = slot.as_ref().ok_or("scheduler not initialized")?;
        Ok(sched.tasks.iter().map(|t| snapshot(t)).collect())
    })
}

/// Detailed information about one task.
pub fn info(id: u32) -> Result<Task, &'static str> {
    with_scheduler(|slot| {
        let sched = slot.as_ref().ok_or("scheduler not initialized")?;
        sched
            .tasks
            .iter()
            .find(|t| t.id == id)
            .map(|t| snapshot(t))
            .ok_or("task not found")
    })
}

/// Terminate a task by id. The killed task stops being scheduled starting
/// with the next `schedule()` call -- a real effect, not a cosmetic state
/// change.
///
/// If `id` owns a private address space and is *not* the currently running
/// task, its frames are freed immediately: CR3 can only ever equal the
/// current task's own address space, so a non-current task's is provably
/// already inactive. If `id` *is* the current task (killing yourself, or
/// -- more realistically -- fault-isolation recovery in `interrupts.rs`
/// calling `exit_with_code` on itself), the address space is left in place
/// for `Scheduler::prepare_switch` to reclaim right after the next context
/// switch genuinely moves CR3 off it (see `schedule()`).
pub fn kill(id: u32) -> Result<(), &'static str> {
    if id == 1 {
        return Err("cannot kill shell task");
    }

    let reclaim = with_scheduler(|slot| {
        let sched = slot.as_mut().ok_or("scheduler not initialized")?;
        let is_current = sched.tasks[sched.current].id == id;
        match sched.tasks.iter_mut().find(|t| t.id == id) {
            Some(task) => {
                task.state = TaskState::Terminated;
                if is_current {
                    Ok(None)
                } else {
                    Ok(task.process.as_mut().and_then(|p| p.address_space.take()))
                }
            }
            None => Err("task not found"),
        }
    })?;

    if let Some(space) = reclaim {
        // Safety: confirmed above that `id` was not the current task's id,
        // so its address space cannot be the active CR3.
        unsafe { paging::free_address_space(space) };
    }

    Ok(())
}

pub fn state_label(state: TaskState) -> &'static str {
    state.label()
}

pub fn privilege_label(privilege: Privilege) -> &'static str {
    privilege.label()
}

/// The currently running task's own id -- used by `syscall::sys_getpid`
/// (a process's task id *is* its pid in this minimal model) and by
/// `copy_from_current_user` to find its address space.
pub fn current_task_id() -> Option<u32> {
    with_scheduler(|slot| {
        let sched = slot.as_ref()?;
        Some(sched.tasks[sched.current].id)
    })
}

/// This now-running user task's entry point and user stack top, stashed by
/// `spawn_user_process` -- see `rust_user_entry`.
fn current_process_entry() -> Option<(u64, u64)> {
    with_scheduler(|slot| {
        let sched = slot.as_ref()?;
        let process = sched.tasks[sched.current].process.as_ref()?;
        Some((process.entry_point, process.user_stack_top))
    })
}

/// Copy `len` bytes out of the *currently running* task's own user memory
/// starting at `addr`, or `None` if the current task isn't a user process
/// at all, or if any byte in range fails validation (unmapped, not
/// user-accessible, or `addr + len` overflows) -- see
/// `paging::read_bytes_from_address_space`, which this is a thin,
/// current-task-scoped wrapper around. This is the only way `syscall.rs`
/// ever reads memory a Ring 3 program pointed it at: never a raw pointer
/// dereference of a user-supplied address.
pub fn copy_from_current_user(addr: u64, len: usize) -> Option<Vec<u8>> {
    with_scheduler(|slot| {
        let sched = slot.as_ref()?;
        let space = sched.tasks[sched.current]
            .process
            .as_ref()?
            .address_space
            .as_ref()?;
        paging::read_bytes_from_address_space(space, VirtAddr::new(addr), len)
    })
}

/// Number of physical frames a process's address space currently owns --
/// diagnostic/isolation-test evidence (`monitor`, the isolation-test shell
/// command), not load-bearing for correctness. `None` if `id` doesn't exist
/// or isn't a user process.
pub fn process_frame_count(id: u32) -> Option<usize> {
    with_scheduler(|slot| {
        let sched = slot.as_ref()?;
        sched
            .tasks
            .iter()
            .find(|t| t.id == id)?
            .process
            .as_ref()?
            .address_space
            .as_ref()
            .map(paging::AddressSpace::frame_count)
    })
}

/// The raw physical address of a process's own PML4 -- concrete,
/// hardware-level evidence that two processes have genuinely separate page
/// tables (see the isolation-test shell command), not just a claim. `None`
/// if `id` doesn't exist or isn't a user process.
pub fn process_pml4_phys(id: u32) -> Option<u64> {
    with_scheduler(|slot| {
        let sched = slot.as_ref()?;
        sched
            .tasks
            .iter()
            .find(|t| t.id == id)?
            .process
            .as_ref()?
            .address_space
            .as_ref()
            .map(|space| space.pml4_frame().start_address().as_u64())
    })
}

/// User stack, in pages, for every process this loader spawns -- 16 KiB,
/// generous for `hello_user`'s needs (a handful of stack frames and a
/// 20-byte decimal-conversion buffer) with headroom to spare.
const USER_STACK_PAGES: u64 = 4;

/// Load `elf_bytes` as a new user process and add it to the scheduler.
/// Returns its task id (== its pid, see `current_task_id`/`sys_getpid`) on
/// success.
///
/// Builds a fresh private address space (`paging::new_address_space`),
/// loads the ELF into it (`elf::load`, which maps and populates every
/// `PT_LOAD` segment with its own real permissions), maps a dedicated user
/// stack, and creates a `Ready` task whose first run drops straight to
/// Ring 3 at the ELF's entry point (`user_task_trampoline` /
/// `rust_user_entry`). None of the address-space setup requires this
/// process's CR3 to be the currently active one -- every `paging::`
/// function used here works against an arbitrary `AddressSpace` via the
/// physical-memory-offset mapping, so this is safe to call from whichever
/// task (ordinarily the shell) initiates the spawn.
pub fn spawn_user_process(name: &str, elf_bytes: &[u8]) -> Result<u32, &'static str> {
    let address_space = paging::new_address_space()?;

    // `address_space` is never installed into a `Tcb`, never reachable from
    // the scheduler, and never scheduled until `build_user_tcb` returns a
    // complete, ready-to-run `Tcb` -- so at every failure point *inside*
    // `build_user_tcb`, it is provably not the active CR3, and freeing it
    // immediately is always sound. See `build_user_tcb`'s own docs for why
    // this matters: without this, every failed spawn (a truncated ELF, an
    // out-of-memory stack mapping, ...) would silently leak every frame
    // `new_address_space` and everything `build_user_tcb` had mapped so
    // far -- the PML4 at minimum, every ELF segment page and stack page
    // mapped before the failing step at worst.
    let mut tcb = Some(build_user_tcb(name, elf_bytes, address_space)?);
    let id = tcb.as_ref().expect("just constructed").id;

    // The very last failure point: `with_scheduler` itself refusing (the
    // scheduler not being initialized -- unreachable in practice, since
    // `task::init()` always runs before any shell command could reach
    // this, but handled for the same reason every other step is). `tcb`
    // is an `Option` captured by the closure (by mutable reference, since
    // the closure only ever calls `.take()`/reads it, never moves it
    // outright) specifically so it's still available in this scope
    // afterward on the error path, to reclaim its address space -- a
    // closure that moved `tcb` in directly would make it unreachable here
    // regardless of which branch inside actually ran.
    let push_result: Result<(), &'static str> = with_scheduler(|slot| {
        let sched = slot.as_mut().ok_or("scheduler not initialized")?;
        sched
            .tasks
            .push(Box::new(tcb.take().expect("tcb not yet taken")));
        Ok(())
    });

    match push_result {
        Ok(()) => Ok(id),
        Err(reason) => {
            // Safety: the push above never ran (this is the `Err` branch,
            // and `with_scheduler`'s closure only calls `tcb.take()` right
            // before the push it's guarding), so `tcb` is still `Some`
            // here, was never added to `sched.tasks`, and therefore never
            // had any chance of being scheduled or having its address
            // space loaded into CR3 -- freeing it is sound for the same
            // reason it's sound inside `build_user_tcb`.
            if let Some(mut tcb) = tcb {
                if let Some(process) = tcb.process.as_mut() {
                    if let Some(space) = process.address_space.take() {
                        unsafe { paging::free_address_space(space) };
                    }
                }
            }
            Err(reason)
        }
    }
}

/// Build a complete, ready-to-run `Tcb` for a new user process: load the
/// ELF, map and zero its stack, and wrap it all up -- freeing
/// `address_space`'s frames before returning on *any* failure along the
/// way, since nothing outside this function call has referenced it yet
/// (see `spawn_user_process`'s docs on why that makes every early-return
/// here safe to reclaim immediately rather than leaking).
fn build_user_tcb(
    name: &str,
    elf_bytes: &[u8],
    mut address_space: paging::AddressSpace,
) -> Result<Tcb, &'static str> {
    macro_rules! try_or_free {
        ($expr:expr) => {
            match $expr {
                Ok(value) => value,
                Err(reason) => {
                    // Safety: see this function's own docs.
                    unsafe { paging::free_address_space(address_space) };
                    return Err(reason);
                }
            }
        };
    }

    let loaded = try_or_free!(elf::load(&mut address_space, elf_bytes));

    let stack_top = paging::USER_SPACE_BASE + paging::USER_SPACE_SIZE - 0x1000;
    let stack_bottom = stack_top - USER_STACK_PAGES * 4096;
    let stack_flags = PageTableFlags::PRESENT
        | PageTableFlags::WRITABLE
        | PageTableFlags::USER_ACCESSIBLE
        | PageTableFlags::NO_EXECUTE;
    let mut page_addr = stack_bottom;
    while page_addr < stack_top {
        let page: Page<Size4KiB> = Page::containing_address(VirtAddr::new(page_addr));
        try_or_free!(paging::map_in_address_space(
            &mut address_space,
            page,
            stack_flags
        ));
        page_addr += 4096;
    }
    // Zero the freshly mapped stack -- same reasoning as `elf.rs`'s segment
    // loading: a reused physical frame must never expose a previous
    // process's leftover contents to this one.
    try_or_free!(paging::zero_bytes_in_address_space(
        &address_space,
        VirtAddr::new(stack_bottom),
        USER_STACK_PAGES * 4096,
    ));

    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    Ok(new_user_tcb(
        id,
        name,
        address_space,
        loaded.entry_point.as_u64(),
        stack_top,
    ))
}
