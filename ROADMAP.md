# TuwaiqOS Roadmap

## v0.5 (current)

- [x] Bootloader + kernel + framebuffer/VGA console
- [x] PS/2 keyboard with Shift and punctuation
- [x] Shell with history, tab completion, `tuwaiq@os:~$` prompt
- [x] Heap allocator
- [x] TuwaiqFS v2 persistent tree filesystem
- [x] Cooperative task scheduler (`ps`, `taskinfo`, `kill`)
- [x] Loopback networking foundation
- [x] AI Bridge stub (`ask`, `ai status`)
- [x] Program loader (`run hello`, `run demo`)
- [x] Built-in apps: `notes`, `editor`, `monitor`

## v0.6 — in progress

- [x] Real interrupt architecture: GDT/TSS, IDT with exception handlers,
      PIC remap, PIT timer tick, interrupt-driven keyboard, real `uptime`
- [x] Physical frame allocator + paging: real physical memory access,
      `OffsetPageTable`, a heap backed by mapped pages instead of a static
      array, memory diagnostics in `sysinfo`/`monitor`
- [x] Preemptive scheduler: real per-task stacks and a hand-written
      context switch, timer-driven preemption (50ms slices), `spawn`/
      `yield_now`/`sleep_ticks`/`exit`; `ps`/`taskinfo`/`kill` act on real
      scheduler state. Demonstrated with 3 concurrently-scheduled tasks
      (shell, idle, a heartbeat task) verified live in QEMU.
- [x] Ring 3 foundation: user code/data GDT segments, a TSS whose RSP0 can
      be pointed at a specific kernel stack at runtime, and a real
      `iretq`-based Ring 0 -> Ring 3 transition with a safe trap back.
      Verified live in QEMU: CPL=3 genuinely reached (CS/SS RPL=3 on the
      trap frame), a privileged instruction executed from Ring 3 takes a
      General Protection Fault the kernel recovers from, and the full
      Phase 1-3 regression still passes afterward. Superseded by the full
      process model below -- this milestone's demo code no longer exists.
- [x] Full user-mode process model, per-process address spaces, syscall
      ABI, and ELF64 execution: real `Tcb`-integrated user processes (own
      PID, state, private address space, own kernel + user stacks, exit
      status) scheduled by the *same* Phase 3 scheduler, no second
      scheduler built. Each process gets a genuinely private page-table
      root (`paging::AddressSpace`) -- kernel mappings copied in without
      `USER_ACCESSIBLE`, the process's own 1 GiB user region left entirely
      empty for it alone -- with CR3 and the TSS's RSP0 both switched
      correctly on every scheduler transition, so any number of processes
      interleave safely under real preemption. `elf.rs` is a real,
      hand-rolled ELF64 loader (documented supported subset: `ET_EXEC`,
      `EM_X86_64`, `PT_LOAD` only, checked arithmetic throughout) that maps
      each segment with its own real, final permissions (code RX, data
      RW+NX, stack RW+NX, `EFER.NXE` enabled and verified before relying on
      any of it). Phase 4 introduced EXIT/WRITE/YIELD/GETPID; the current
      Phase 5 ABI has 10 syscalls (0-9) over the `int 0x80` gate, with every user
      pointer validated against the caller's own page tables before the
      kernel ever dereferences it -- an invalid pointer or an unknown
      syscall number both fail cleanly, never crash the kernel. A user
      fault (privileged instruction, kernel-memory access, unmapped
      access) terminates only the offending process; a Ring 0 fault keeps
      the kernel's unchanged, strict halt policy. Verified live in QEMU in
      one session: a real ELF process completing a full syscall round
      trip; each of the fault-isolation categories above triggered and
      recovered from individually; two concurrent processes with distinct,
      hardware-confirmed PML4 physical addresses interleaving under timer
      preemption, one of them deliberately faulted without affecting the
      other or the kernel; the full Phase 1-3 regression checklist; and
      TuwaiqFS content surviving a full VM reset. See `ARCHITECTURE.md`'s
      "Phase 4: user-mode process model" section for the complete design,
      ABI reference, and known limitations (fixed single user-address
      range, no dynamic linking, and no filesystem-backed executable loading
      yet). Phase 5 has since closed the terminated-task/kernel-stack leak.
- [ ] Real NIC driver (e1000 / virtio-net)
- [ ] AI Bridge HTTP client wired to gateway
- [ ] `cd` command and path-aware completion
- [x] Process lifecycle cleanup: terminated `Tcb`s and their kernel stacks
      are now genuinely reaped (grace-period reaping, `reap <count>` proves
      task count and physical-frame bump cursor both stabilize across
      repeated spawn/exit cycles) -- closes the leak Phase 4 documented and
      deferred.
- [x] Userspace anonymous memory: `SYS_MMAP`/`SYS_MUNMAP`, a minimal
      TuwaiqOS-specific ABI (`mmap(len, writable)` chooses an address inside
      the caller's arena; `munmap(ptr, len)` accepts only aligned, owned arena
      ranges). Mapping/zeroing/permission failures roll back; unmap
      prevalidates the complete range and commits atomically.
- [x] Kernel display abstraction + validated present syscall
      (`SYS_DISPLAY_INFO`/`SYS_DISPLAY_PRESENT`): userspace renders into its
      own `mmap`'d buffer, the kernel copies it into the real framebuffer
      only after full pointer/length/mapping validation.
- [x] Real PS/2 mouse driver (IRQ12 via the slave-PIC cascade line) with
      packet resync, signed relative motion, absolute screen-clamped
      cursor position, and button-edge detection.
- [x] Unified keyboard+mouse input queue and `SYS_INPUT_POLL`, with explicit
      foreground ownership. Each key is routed to either the privileged shell
      or one foreground Ring 3 process, never permanently fanned out to both;
      ownership transitions clear stale events.
- [x] **First Tuwaiq Desktop**: a real Ring 3 ELF64 process (`desktop`
      shell command, same `spawn_user_process` path as every other
      `runelf`-launched program) with a small userspace window model
      (fixed-capacity, z-order, drag, close), a system bar with a live
      clock, a working mouse cursor, a launcher button, and visible
      keyboard echo, normal Escape exit, and safe relaunch. Software-rendered,
      no GPU. See `ARCHITECTURE.md`'s
      "Phase 5" section for the complete design, ABI additions, security
      boundaries, and known limitations (bump-only mmap arena, no
      `KeyUp` events, fixed `MAX_WINDOWS = 4`, still launched from an
      embedded ELF rather than the filesystem).
- [x] Phase 5 acceptance: repository-local clean-build/QEMU harness covers
      hostile pointer tests, atomic display/input/VM behavior, CPU permissions,
      mouse decode/absence/latency, exclusive foreground input, concurrent
      Ring 3 execution, 20 present-and-exit desktop cycles, resource baselines,
      genuine reboot persistence, and full Phase 1-5 regression. See
      `ARCHITECTURE.md` -> "Phase 5 Verification Performed". Phase 5 closure
      does not complete the remaining v0.6 NIC/AI Gateway/`cd` roadmap items.

## v0.7 — planned

- [ ] Filesystem-backed executable loading (`elf.rs` already parses real
      ELF64 bytes; the desktop and every test program are still loaded
      from build-time-embedded binaries, not the filesystem)
- [ ] Dynamic linking / relocations (current loader is `ET_EXEC`-only)
- [ ] Broader syscall surface (filesystem, IPC) as real use cases justify
      each one
- [ ] `KeyUp` events (keyboard scancode decoder doesn't track per-key
      release state yet, only Shift)
- [ ] A free-list-backed `mmap` arena (current one is bump-only; a
      `munmap`'d range's virtual addresses aren't reused within the same
      process)
- [ ] Dirty-rectangle presentation for the desktop compositor (the current
      desktop redraws/presents a full frame only on input, initial display, or
      clock changes; idle iterations yield without redrawing)
- [ ] FAT32 read-only partition support
- [ ] VirtualBox/VMware optimized drivers
- [ ] Package manager for built-in apps

## v1.0 — vision

- [ ] Multi-user sessions
- [ ] TLS + DNS for AI Bridge
- [ ] Self-hosting toolchain on TuwaiqOS
- [ ] Public SDK for TuwaiqOS applications

## Historical note

This project began as AbdullahOS (learning OS). It was renamed to TuwaiqOS at v0.5 for public release.
