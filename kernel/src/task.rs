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

/// Lightweight snapshot returned by `list`/`info` -- deliberately does not
/// expose a task's stack or saved context, only what `ps`/`taskinfo` need.
#[derive(Clone)]
pub struct Task {
    pub id: u32,
    pub name: String,
    pub state: TaskState,
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
    /// (see `stack` above). Exposed via `current_kernel_stack_top` so a
    /// task can register itself as the Ring 3 trap landing pad
    /// (`gdt::set_kernel_stack`) before dropping to user mode -- see
    /// `usermode.rs`.
    kernel_stack_top: u64,
}

struct Scheduler {
    tasks: Vec<Box<Tcb>>,
    current: usize,
}

impl Scheduler {
    /// Decide whether a switch is needed and, if so, which two tasks are
    /// involved -- but do not perform it. Returning raw pointers/values
    /// instead of switching here keeps `SCHEDULER`'s lock scoped to just
    /// the bookkeeping: `context_switch` must run with the lock already
    /// released, since it may not return to this call frame for an
    /// arbitrarily long time (not until this task is next scheduled).
    fn prepare_switch(&mut self) -> Option<(*mut u64, u64)> {
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

        let old_ptr: *mut u64 = &mut self.tasks[self.current].saved_rsp;
        let new_val = self.tasks[next].saved_rsp;
        self.tasks[next].state = TaskState::Running;
        self.current = next;
        Some((old_ptr, new_val))
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
"#,
    task_exit = sym task_exit_trampoline,
);

extern "C" fn task_exit_trampoline() {
    exit();
}

extern "C" {
    fn context_switch(old_rsp: *mut u64, new_rsp: u64);
    fn task_trampoline();
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

/// Perform a scheduling decision and, if warranted, a real context switch.
/// Safe to call from both interrupt context (the timer ISR already runs
/// with interrupts disabled) and normal context (`yield_now`/`sleep_ticks`
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
        let switch = with_scheduler(|slot| slot.as_mut().and_then(Scheduler::prepare_switch));
        if let Some((old_rsp, new_rsp)) = switch {
            // Safety: `old_rsp` points at the currently-running task's own
            // `saved_rsp` field (a stable heap address behind `Box<Tcb>`,
            // unaffected by the `Vec` it lives in reallocating), and
            // `new_rsp` was populated either by a previous `context_switch`
            // call or by `spawn`'s fake initial frame -- either way, a
            // valid stack pointer for `context_switch` to resume from.
            unsafe { context_switch(old_rsp, new_rsp) };
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

/// Terminate the current task. Never returns: the next `schedule()` call
/// switches away from it permanently (a `Terminated` task is never chosen
/// by `prepare_switch` again).
pub fn exit() -> ! {
    with_scheduler(|slot| {
        if let Some(sched) = slot.as_mut() {
            let idx = sched.current;
            sched.tasks[idx].state = TaskState::Terminated;
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

/// List all tasks for the `ps` command -- genuine scheduler state, not a
/// static table.
pub fn list() -> Result<Vec<Task>, &'static str> {
    with_scheduler(|slot| {
        let sched = slot.as_ref().ok_or("scheduler not initialized")?;
        Ok(sched
            .tasks
            .iter()
            .map(|t| Task {
                id: t.id,
                name: t.name.clone(),
                state: t.state,
            })
            .collect())
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
            .map(|t| Task {
                id: t.id,
                name: t.name.clone(),
                state: t.state,
            })
            .ok_or("task not found")
    })
}

/// Terminate a task by id. The killed task stops being scheduled starting
/// with the next `schedule()` call -- a real effect, not a cosmetic state
/// change.
pub fn kill(id: u32) -> Result<(), &'static str> {
    if id == 1 {
        return Err("cannot kill shell task");
    }

    with_scheduler(|slot| {
        let sched = slot.as_mut().ok_or("scheduler not initialized")?;
        match sched.tasks.iter_mut().find(|t| t.id == id) {
            Some(task) => {
                task.state = TaskState::Terminated;
                Ok(())
            }
            None => Err("task not found"),
        }
    })
}

pub fn state_label(state: TaskState) -> &'static str {
    state.label()
}

/// The currently running task's own kernel stack top, or `None` for the
/// boot task (id 1, "shell"), which owns no stack this module allocated.
/// See `Tcb::kernel_stack_top`'s docs -- this is how a task about to run
/// Ring 3 code finds the address to hand to `gdt::set_kernel_stack`.
pub fn current_kernel_stack_top() -> Option<u64> {
    with_scheduler(|slot| {
        let sched = slot.as_ref()?;
        let top = sched.tasks[sched.current].kernel_stack_top;
        if top == 0 {
            None
        } else {
            Some(top)
        }
    })
}
