//! Interactive shell for TuwaiqOS.
//!
//! Features: command history, arrow-key recall, tab completion, and the
//! `tuwaiq@os:~$` prompt.

use alloc::string::String;
use alloc::vec::Vec;

use bootloader_api::{info::MemoryRegionKind, BootInfo};

use crate::ai_bridge;
use crate::allocator;
use crate::apps::{editor, monitor, notes};
use crate::fs;
use crate::interrupts;
use crate::keyboard::{poll_key, KeyEvent};
use crate::loader;
use crate::memory;
use crate::net;
use crate::paging;
use crate::reboot;
use crate::task;

const MAX_LINE: usize = 128;
const HISTORY_SIZE: usize = 16;

/// Which console backend is active.
#[derive(Clone, Copy)]
pub enum ConsoleMode {
    Framebuffer,
    Vga,
}

struct History {
    entries: [[u8; MAX_LINE]; HISTORY_SIZE],
    count: usize,
    browse: usize,
}

impl History {
    const fn new() -> Self {
        Self {
            entries: [[0; MAX_LINE]; HISTORY_SIZE],
            count: 0,
            browse: 0,
        }
    }

    fn push(&mut self, line: &[u8], len: usize) {
        if len == 0 {
            return;
        }
        let index = self.count.min(HISTORY_SIZE - 1);
        self.entries[index][..len].copy_from_slice(&line[..len]);
        self.entries[index][len..].fill(0);
        if self.count < HISTORY_SIZE {
            self.count += 1;
        } else {
            for i in 1..HISTORY_SIZE {
                self.entries[i - 1] = self.entries[i];
            }
            self.entries[HISTORY_SIZE - 1][..len].copy_from_slice(&line[..len]);
        }
        self.browse = self.count;
    }

    fn recall_up(&mut self) -> Option<&[u8]> {
        if self.count == 0 {
            return None;
        }
        if self.browse == 0 {
            self.browse = 0;
        } else if self.browse > self.count {
            self.browse = self.count - 1;
        } else if self.browse > 0 {
            self.browse -= 1;
        }
        let entry = &self.entries[self.browse];
        let len = entry.iter().position(|&b| b == 0).unwrap_or(MAX_LINE);
        Some(&entry[..len])
    }

    fn recall_down(&mut self) -> Option<&[u8]> {
        if self.count == 0 {
            return None;
        }
        if self.browse + 1 >= self.count {
            self.browse = self.count;
            return Some(&[]);
        }
        self.browse += 1;
        let entry = &self.entries[self.browse];
        let len = entry.iter().position(|&b| b == 0).unwrap_or(MAX_LINE);
        Some(&entry[..len])
    }

    fn reset_browse(&mut self) {
        self.browse = self.count;
    }
}

/// Run the interactive shell forever.
pub fn run(boot_info: &'static BootInfo, mode: ConsoleMode) -> ! {
    let mut line = [0u8; MAX_LINE];
    let mut history = History::new();
    let mut first_prompt = true;

    loop {
        if !first_prompt {
            println(mode, "");
        }
        first_prompt = false;

        print_dynamic_prompt(mode);

        let mut len = 0;
        history.reset_browse();

        loop {
            match poll_key() {
                // Nothing queued: halt until the next interrupt (timer or
                // keyboard) instead of burning CPU re-checking the queue.
                // v0.5 busy-polled ports 0x60/0x64 directly in this spot.
                KeyEvent::None => interrupts::halt(),
                KeyEvent::Char(ch) => {
                    if len < MAX_LINE {
                        line[len] = ch;
                        len += 1;
                        print_char(mode, ch);
                    }
                }
                KeyEvent::Backspace => {
                    if len > 0 {
                        len -= 1;
                        backspace(mode);
                    }
                }
                KeyEvent::ArrowUp => {
                    if let Some(entry) = history.recall_up() {
                        len = replace_input(mode, &mut line, len, entry);
                    }
                }
                KeyEvent::ArrowDown => {
                    if let Some(entry) = history.recall_down() {
                        len = replace_input(mode, &mut line, len, entry);
                    }
                }
                KeyEvent::Tab => {
                    len = tab_complete(mode, &mut line, len);
                }
                KeyEvent::Enter => {
                    println(mode, "");
                    let command = core::str::from_utf8(&line[..len]).unwrap_or("");
                    history.push(&line, len);
                    execute_command(boot_info, mode, command.trim());
                    line[..len].fill(0);
                    break;
                }
            }
        }
    }
}

fn replace_input(mode: ConsoleMode, line: &mut [u8], len: usize, new_text: &[u8]) -> usize {
    for _ in 0..len {
        backspace(mode);
    }
    let new_len = new_text.len().min(MAX_LINE);
    line[..new_len].copy_from_slice(&new_text[..new_len]);
    for &ch in &new_text[..new_len] {
        print_char(mode, ch);
    }
    new_len
}

fn tab_complete(mode: ConsoleMode, line: &mut [u8], len: usize) -> usize {
    let current = core::str::from_utf8(&line[..len]).unwrap_or("");
    let prefix = current.trim_end();
    if prefix.is_empty() {
        return len;
    }

    let (token, is_command) = if let Some(index) = prefix.rfind(' ') {
        (&prefix[index + 1..], false)
    } else {
        (prefix, true)
    };

    let mut matches = Vec::new();
    if is_command {
        for cmd in command_names() {
            if cmd.starts_with(token) {
                matches.push(String::from(*cmd));
            }
        }
        for name in loader::program_names() {
            if (*name).starts_with(token) {
                matches.push(String::from(*name));
            }
        }
    }
    if let Ok(files) = fs::completion_candidates(token) {
        for file in files {
            if file.starts_with(token) {
                matches.push(file);
            }
        }
    }

    if matches.is_empty() {
        return len;
    }

    if matches.len() == 1 {
        let prefix_owned = String::from(prefix);
        let token_owned = String::from(token);
        let completion = matches[0].clone();
        return apply_completion(mode, line, len, &prefix_owned, &token_owned, &completion);
    }

    println(mode, "");
    for m in &matches {
        println(mode, m);
    }
    print_dynamic_prompt(mode);
    for i in 0..len {
        print_char(mode, line[i]);
    }
    len
}

fn apply_completion(
    mode: ConsoleMode,
    line: &mut [u8],
    len: usize,
    prefix: &str,
    _token: &str,
    completion: &str,
) -> usize {
    let base = if let Some(index) = prefix.rfind(' ') {
        &prefix[..=index]
    } else {
        ""
    };
    let mut new_line = String::from(base);
    new_line.push_str(completion);
    if command_names().iter().any(|c| *c == completion) || base.is_empty() {
        new_line.push(' ');
    }
    replace_input(mode, line, len, new_line.as_bytes())
}

fn print_dynamic_prompt(mode: ConsoleMode) {
    let mut prompt = String::from("tuwaiq@os:");
    match fs::pwd() {
        Ok(path) if path == "/" => prompt.push_str("~"),
        Ok(path) => prompt.push_str(&path),
        Err(_) => prompt.push('~'),
    }
    prompt.push('$');
    prompt.push(' ');
    print(mode, &prompt);
}

fn execute_command(boot_info: &BootInfo, mode: ConsoleMode, line: &str) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }

    let (command, args) = split_command(line);

    match command {
        "help" => print_help(mode),
        "about" => {
            println(mode, "TuwaiqOS");
            println(
                mode,
                "Experimental AI-Native Operating System written in Rust.",
            );
        }
        "version" => println(mode, "TuwaiqOS v0.5"),
        "banner" => print_banner(mode),
        "sysinfo" => print_sysinfo(boot_info, mode),
        "uptime" => print_uptime(mode),
        "reboot" => reboot::system(),
        "clear" | "cls" => clear_screen(mode),
        "echo" => println(mode, args),
        "meminfo" => print_meminfo(boot_info, mode),
        "memtest" => match memory::memtest() {
            Ok(()) => println(mode, "Heap allocation test passed."),
            Err(reason) => {
                print(mode, "Heap allocation test failed: ");
                println(mode, reason);
            }
        },
        "pwd" => match fs::pwd() {
            Ok(path) => println(mode, &path),
            Err(reason) => print_fs_error(mode, reason),
        },
        "ls" => match fs::ls() {
            Ok(entries) => print_entries(mode, entries),
            Err(reason) => print_fs_error(mode, reason),
        },
        "touch" => handle_touch(mode, args),
        "mkdir" => handle_mkdir(mode, args),
        "cat" => handle_cat(mode, args),
        "write" => handle_write(mode, args),
        "ps" => handle_ps(mode),
        "taskinfo" => handle_taskinfo(mode, args),
        "kill" => handle_kill(mode, args),
        "yield" => {
            task::yield_now();
            println(mode, "Yielded one time slice.");
        }
        "net" => handle_net_command(mode, args),
        "ping" => handle_ping(mode, args),
        "run" => handle_run(mode, args),
        "notes" => handle_notes(mode, args),
        "editor" => handle_editor(mode, args),
        "monitor" => handle_monitor(boot_info, mode),
        "runelf" => handle_runelf(mode, args),
        "isolate" => handle_isolate(mode, args),
        "ai" => handle_ai_command(mode, line, args),
        "ask" => handle_ask_command(mode, args),
        _ => {
            print(mode, "Unknown command: ");
            println(mode, command);
        }
    }
}

fn print_entries(mode: ConsoleMode, entries: Vec<String>) {
    if entries.is_empty() {
        println(mode, "(empty)");
    } else {
        for entry in entries {
            println(mode, &entry);
        }
    }
}

fn handle_run(mode: ConsoleMode, args: &str) {
    let name = args.trim();
    if name.is_empty() {
        println(mode, "Usage: run <program>");
        return;
    }
    match loader::run(name, "") {
        Ok(lines) => {
            for line in lines {
                println(mode, &line);
            }
        }
        Err(reason) => {
            print(mode, "Program error: ");
            println(mode, reason);
        }
    }
}

fn handle_notes(mode: ConsoleMode, args: &str) {
    let (sub, rest) = split_command(args);
    match notes::handle(sub, rest) {
        Ok(lines) => {
            for line in lines {
                println(mode, &line);
            }
        }
        Err(reason) => {
            print(mode, "Notes error: ");
            println(mode, reason);
        }
    }
}

fn handle_editor(mode: ConsoleMode, args: &str) {
    match editor::handle(args) {
        Ok(lines) => {
            for line in lines {
                println(mode, &line);
            }
        }
        Err(reason) => {
            print(mode, "Editor error: ");
            println(mode, reason);
        }
    }
}

fn handle_monitor(boot_info: &BootInfo, mode: ConsoleMode) {
    match monitor::snapshot(boot_info) {
        Ok(lines) => {
            for line in lines {
                println(mode, &line);
            }
        }
        Err(reason) => {
            print(mode, "Monitor error: ");
            println(mode, reason);
        }
    }
}

/// The six real, compiled ELF64 test programs (`userland/hello`), embedded
/// at build time -- see that crate's `src/bin/*.rs` for what each one
/// actually does. `hello` is the well-behaved one; the five `bad_*` binaries
/// each deliberately trigger one required fault-isolation category (see
/// `ARCHITECTURE.md`'s "User process fault isolation" section).
fn embedded_program(name: &str) -> Option<&'static [u8]> {
    match name {
        "hello" => Some(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../target/x86_64-unknown-none/release/hello"
        ))),
        "bad_syscall" => Some(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../target/x86_64-unknown-none/release/bad_syscall"
        ))),
        "bad_pointer" => Some(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../target/x86_64-unknown-none/release/bad_pointer"
        ))),
        "bad_privileged" => Some(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../target/x86_64-unknown-none/release/bad_privileged"
        ))),
        "bad_kernel" => Some(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../target/x86_64-unknown-none/release/bad_kernel"
        ))),
        "bad_unmapped" => Some(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../target/x86_64-unknown-none/release/bad_unmapped"
        ))),
        _ => None,
    }
}

fn wait_for_terminated(id: u32) {
    loop {
        match task::info(id) {
            Ok(t) if t.state == task::TaskState::Terminated => break,
            Ok(_) => task::yield_now(),
            Err(_) => break,
        }
    }
}

fn print_process_result(mode: ConsoleMode, id: u32) {
    match task::info(id) {
        Ok(t) => {
            print(mode, "  pid=");
            print_u64(mode, t.id as u64);
            print(mode, " name=");
            print(mode, &t.name);
            print(mode, " state=");
            print(mode, task::state_label(t.state));
            print(mode, " exit_code=");
            match t.exit_code {
                Some(code) => print_i64(mode, code as i64),
                None => print(mode, "none"),
            }
            println(mode, "");
        }
        Err(reason) => {
            print(mode, "  task error: ");
            println(mode, reason);
        }
    }
}

/// `runelf <name>` -- loads one of the embedded ELF64 test programs as a
/// real user process (`task::spawn_user_process`, which goes through the
/// genuine ELF loader in `elf.rs`) and waits for it to terminate. The
/// reported result reflects the process's actual final state (`ps`
/// under the hood), not a fixed string; the detailed evidence of what
/// happened while it ran (Ring 3 entry, syscalls, any fault) is on the
/// serial log.
fn handle_runelf(mode: ConsoleMode, args: &str) {
    let name = args.trim();
    let Some(bytes) = embedded_program(name) else {
        println(
            mode,
            "Usage: runelf <hello|bad_syscall|bad_pointer|bad_privileged|bad_kernel|bad_unmapped>",
        );
        return;
    };

    match task::spawn_user_process(name, bytes) {
        Ok(id) => {
            print(mode, "Spawned pid ");
            print_u64(mode, id as u64);
            print(mode, " (");
            print(mode, name);
            println(mode, "), waiting for it to finish...");
            wait_for_terminated(id);
            print_process_result(mode, id);
        }
        Err(reason) => {
            print(mode, "Process load error: ");
            println(mode, reason);
        }
    }
}

/// `isolate [program_b]` -- the Phase 4 process-isolation proof. Spawns
/// *two* processes back to back (both `Ready` and interleaving under the
/// same 100 Hz timer preemption everything else in this kernel runs under
/// -- neither is waited on before the other starts): `hello` and, by
/// default, a second independent `hello` instance -- or, if `program_b` is
/// given (e.g. `isolate bad_privileged`), that program instead, which lets
/// this same command double as the "faulting one process does not kill
/// another" proof: pid A keeps running and exits cleanly regardless of
/// what happens to pid B.
///
/// Prints hardware evidence that the two are genuinely separate: each
/// process's own PML4 physical address (`task::process_pml4_phys`) and
/// owned physical frame count (`task::process_frame_count`) -- two
/// different page-table roots is the actual mechanism behind "process A
/// cannot read process B's memory," not a claim this command makes on its
/// own.
fn handle_isolate(mode: ConsoleMode, args: &str) {
    let name_b = {
        let trimmed = args.trim();
        if trimmed.is_empty() {
            "hello"
        } else {
            trimmed
        }
    };
    let (Some(bytes_a), Some(bytes_b)) = (embedded_program("hello"), embedded_program(name_b))
    else {
        println(
            mode,
            "Usage: isolate [bad_syscall|bad_pointer|bad_privileged|bad_kernel|bad_unmapped]",
        );
        return;
    };

    // Capture each process's PML4/frame-count evidence *immediately* after
    // spawning it -- not after printing anything. Printing goes through
    // the framebuffer/VGA character-by-character path, which takes long
    // enough in wall-clock terms that the 100 Hz timer can (and, for a
    // process as short-lived as `hello`, reliably does) preempt into it,
    // let it run to completion, and free its address space before this
    // function would otherwise have gotten around to reading it -- which
    // would print a reclaimed `PML4=0x0` instead of real evidence. This
    // way the values printed below are always the genuine snapshot taken
    // right as each process started, regardless of how fast it finishes.
    let Ok(id_a) = task::spawn_user_process("hello-a", bytes_a) else {
        print(mode, "Process load error spawning hello-a");
        println(mode, "");
        return;
    };
    let pml4_a = task::process_pml4_phys(id_a).unwrap_or(0);
    let frames_a = task::process_frame_count(id_a).unwrap_or(0);

    let Ok(id_b) = task::spawn_user_process(name_b, bytes_b) else {
        print(mode, "Process load error spawning ");
        println(mode, name_b);
        return;
    };
    let pml4_b = task::process_pml4_phys(id_b).unwrap_or(0);
    let frames_b = task::process_frame_count(id_b).unwrap_or(0);

    print(mode, "Spawned pid ");
    print_u64(mode, id_a as u64);
    print(mode, " (hello-a) and pid ");
    print_u64(mode, id_b as u64);
    print(mode, " (");
    print(mode, name_b);
    println(mode, "), running concurrently under preemption.");

    print(mode, "  pid ");
    print_u64(mode, id_a as u64);
    print(mode, " PML4=");
    print_hex(mode, pml4_a);
    print(mode, " frames=");
    print_u64(mode, frames_a as u64);
    println(mode, "");

    print(mode, "  pid ");
    print_u64(mode, id_b as u64);
    print(mode, " PML4=");
    print_hex(mode, pml4_b);
    print(mode, " frames=");
    print_u64(mode, frames_b as u64);
    println(mode, "");

    if pml4_a != 0 && pml4_a == pml4_b {
        println(
            mode,
            "  WARNING: both processes report the SAME PML4 -- address spaces are NOT isolated!",
        );
    } else if pml4_a != 0 && pml4_b != 0 {
        println(
            mode,
            "  Confirmed: distinct PML4 physical addresses -- genuinely separate page tables.",
        );
    }

    wait_for_terminated(id_a);
    wait_for_terminated(id_b);
    println(mode, "Both finished:");
    print_process_result(mode, id_a);
    print_process_result(mode, id_b);
}

fn handle_touch(mode: ConsoleMode, args: &str) {
    let name = args.trim();
    if name.is_empty() {
        println(mode, "Usage: touch <name>");
        return;
    }
    match fs::touch(name) {
        Ok(()) => {
            print(mode, "Created file: ");
            println(mode, name);
        }
        Err(reason) => print_fs_error(mode, reason),
    }
}

fn handle_mkdir(mode: ConsoleMode, args: &str) {
    let name = args.trim();
    if name.is_empty() {
        println(mode, "Usage: mkdir <name>");
        return;
    }
    match fs::mkdir(name) {
        Ok(()) => {
            print(mode, "Created directory: ");
            println(mode, name);
        }
        Err(reason) => print_fs_error(mode, reason),
    }
}

fn handle_cat(mode: ConsoleMode, args: &str) {
    let name = args.trim();
    if name.is_empty() {
        println(mode, "Usage: cat <name>");
        return;
    }
    match fs::cat(name) {
        Ok(content) => println(mode, &content),
        Err(reason) => print_fs_error(mode, reason),
    }
}

fn handle_write(mode: ConsoleMode, args: &str) {
    let Some((name, text)) = split_first_token(args) else {
        println(mode, "Usage: write <name> <text>");
        return;
    };
    match fs::write(name, text) {
        Ok(()) => {
            print(mode, "Wrote to: ");
            println(mode, name);
        }
        Err(reason) => print_fs_error(mode, reason),
    }
}

fn handle_ps(mode: ConsoleMode) {
    match task::list() {
        Ok(tasks) => {
            println(mode, "PID   NAME             PRIV    STATE      EXIT");
            for task in tasks {
                print_u64(mode, task.id as u64);
                print(mode, "     ");
                pad_print(mode, &task.name, 17);
                pad_print(mode, task::privilege_label(task.privilege), 8);
                pad_print(mode, task::state_label(task.state), 11);
                match task.exit_code {
                    Some(code) => print_i64(mode, code as i64),
                    None => print(mode, "-"),
                }
                println(mode, "");
            }
        }
        Err(reason) => {
            print(mode, "Task error: ");
            println(mode, reason);
        }
    }
}

/// Print `text` left-padded to at least `width` columns with spaces --
/// keeps `ps`'s columns aligned regardless of name/state string length.
fn pad_print(mode: ConsoleMode, text: &str, width: usize) {
    print(mode, text);
    for _ in text.len()..width {
        print_char(mode, b' ');
    }
}

fn handle_taskinfo(mode: ConsoleMode, args: &str) {
    let args = args.trim();
    if args.is_empty() {
        match task::list() {
            Ok(tasks) => {
                for task in tasks {
                    print(mode, "Task ");
                    print_u64(mode, task.id as u64);
                    print(mode, ": ");
                    println(mode, &task.name);
                    print(mode, "  Privilege: ");
                    println(mode, task::privilege_label(task.privilege));
                    print(mode, "  State: ");
                    println(mode, task::state_label(task.state));
                }
            }
            Err(reason) => {
                print(mode, "Task error: ");
                println(mode, reason);
            }
        }
        return;
    }

    let id = parse_u32(args).unwrap_or(0);
    if id == 0 {
        println(mode, "Usage: taskinfo [id]");
        return;
    }

    match task::info(id) {
        Ok(task) => {
            print(mode, "PID: ");
            print_u64(mode, task.id as u64);
            println(mode, "");
            print(mode, "Name: ");
            println(mode, &task.name);
            print(mode, "Privilege: ");
            println(mode, task::privilege_label(task.privilege));
            print(mode, "State: ");
            println(mode, task::state_label(task.state));
            if task.privilege == task::Privilege::User {
                print(mode, "Exit code: ");
                match task.exit_code {
                    Some(code) => {
                        print_i64(mode, code as i64);
                        println(mode, "");
                    }
                    None => println(mode, "none (still running)"),
                }
                if let Some(pml4) = task::process_pml4_phys(id) {
                    print(mode, "Address space PML4: ");
                    print_hex(mode, pml4);
                    println(mode, "");
                }
                if let Some(frames) = task::process_frame_count(id) {
                    print(mode, "Address space frames: ");
                    print_u64(mode, frames as u64);
                    println(mode, "");
                }
            }
        }
        Err(reason) => {
            print(mode, "Task error: ");
            println(mode, reason);
        }
    }
}

fn handle_kill(mode: ConsoleMode, args: &str) {
    let args = args.trim();
    if args.is_empty() {
        println(mode, "Usage: kill <id>");
        return;
    }
    let id = parse_u32(args).unwrap_or(0);
    if id == 0 {
        println(mode, "Usage: kill <id>");
        return;
    }
    match task::kill(id) {
        Ok(()) => {
            print(mode, "Stopped task ");
            print_u64(mode, id as u64);
            println(mode, "");
        }
        Err(reason) => {
            print(mode, "Task error: ");
            println(mode, reason);
        }
    }
}

fn handle_net_command(mode: ConsoleMode, args: &str) {
    let sub = args.trim();
    if sub.eq_ignore_ascii_case("status") || sub.is_empty() {
        for line in net::status_lines() {
            println(mode, line);
        }
        return;
    }
    print(mode, "Unknown net command: ");
    println(mode, sub);
}

fn handle_ping(mode: ConsoleMode, args: &str) {
    let host = args.trim();
    if host.is_empty() {
        println(mode, "Usage: ping <host>");
        return;
    }
    match net::ping(host) {
        Ok(message) => println(mode, &message),
        Err(reason) => {
            print(mode, "Network error: ");
            println(mode, reason);
        }
    }
}

fn handle_ai_command(mode: ConsoleMode, line: &str, args: &str) {
    let sub = args.trim();

    if sub.eq_ignore_ascii_case("status") {
        let status = ai_bridge::bridge().status();
        print(mode, "AI Bridge: ");
        println(mode, if status.online { "online" } else { "offline" });
        print(mode, "Mode: ");
        match status.mode {
            ai_bridge::BridgeMode::Stub => println(mode, "stub"),
        }
        print(mode, "Phase: ");
        print_u64(mode, status.phase as u64);
        println(mode, "");
        return;
    }

    if sub.eq_ignore_ascii_case("help") {
        for line in ai_bridge::bridge().help_lines() {
            println(mode, line);
        }
        return;
    }

    if sub.is_empty() {
        println(mode, ai_bridge::bridge().offline_notice());
        println(mode, "Try: ai help | ai status | ask <question>");
        return;
    }

    print(mode, "Unknown command: ");
    println(mode, line);
}

fn handle_ask_command(mode: ConsoleMode, question: &str) {
    let response = ai_bridge::bridge().ask(question.trim());
    for line in response.lines() {
        println(mode, line);
    }
}

fn print_uptime(mode: ConsoleMode) {
    let seconds = interrupts::uptime_seconds();
    print(mode, "Uptime: ");
    print_u64(mode, seconds);
    print(mode, " s (");
    print_u64(mode, interrupts::ticks());
    println(mode, " timer ticks)");
}

fn print_fs_error(mode: ConsoleMode, reason: &str) {
    print(mode, "Filesystem error: ");
    println(mode, reason);
}

fn print_help(mode: ConsoleMode) {
    println(mode, "Commands:");
    println(
        mode,
        "  help | about | version | banner | sysinfo | monitor",
    );
    println(mode, "  uptime | reboot | clear | cls | echo <text>");
    println(mode, "  meminfo | memtest");
    println(mode, "  ls | pwd | touch | mkdir | cat | write");
    println(mode, "  ps | taskinfo | kill | yield | net status | ping");
    println(mode, "  run <program> | notes | editor");
    println(
        mode,
        "  runelf <hello|bad_syscall|bad_pointer|bad_privileged|bad_kernel|bad_unmapped>",
    );
    println(mode, "  isolate [bad_program]");
    println(mode, "  ai | ai status | ask <question>");
    println(mode, "");
    println(mode, "Tip: use Up/Down for history, Tab to complete.");
}

fn print_banner(mode: ConsoleMode) {
    println(mode, "========================================");
    println(mode, "  TuwaiqOS v0.5");
    println(mode, "  AI-Native Experimental OS");
    println(mode, "========================================");
}

fn print_sysinfo(boot_info: &BootInfo, mode: ConsoleMode) {
    let mut usable_bytes: u64 = 0;
    for region in boot_info.memory_regions.iter() {
        if region.kind == MemoryRegionKind::Usable {
            usable_bytes = usable_bytes.saturating_add(region.end.saturating_sub(region.start));
        }
    }

    println(mode, "System Information");
    println(mode, "  OS: TuwaiqOS v0.5");
    println(mode, "  Architecture: x86_64");
    print(mode, "  Uptime: ");
    print_u64(mode, interrupts::uptime_seconds());
    println(mode, " s");
    print(mode, "  Usable RAM: ");
    print_u64(mode, usable_bytes);
    println(mode, " bytes");
    print(mode, "  Kernel heap: ");
    print_u64(mode, memory::HEAP_SIZE as u64);
    print(mode, " bytes (");
    print_u64(mode, allocator::used() as u64);
    print(mode, " used, ");
    print_u64(mode, allocator::free() as u64);
    println(mode, " free)");
    print(mode, "  Paging: ");
    if paging::is_active() {
        print(mode, "active");
        if let Some(stats) = paging::frame_stats() {
            print(mode, " (");
            print_u64(mode, stats.allocated as u64);
            print(mode, " frames allocated, ");
            print_u64(mode, stats.free_in_pool as u64);
            print(mode, " in free pool)");
        }
        println(mode, "");
    } else {
        println(mode, "inactive (static-array heap fallback)");
    }
    print(mode, "  Filesystem: ");
    println(mode, fs::label());
    println(
        mode,
        "  Tasks: preemptive scheduler (round-robin, 50ms slices)",
    );
    println(mode, "  Network: loopback");
    println(mode, "  AI Bridge: offline (stub)");
    if let Some(fb) = boot_info.framebuffer.as_ref() {
        let info = fb.info();
        print(mode, "  Framebuffer: ");
        print_u64(mode, info.width as u64);
        print(mode, "x");
        print_u64(mode, info.height as u64);
        println(mode, "");
    } else {
        println(mode, "  Display: VGA text mode");
    }
}

fn command_names() -> &'static [&'static str] {
    &[
        "help", "about", "version", "banner", "sysinfo", "monitor", "uptime", "reboot", "clear",
        "cls", "echo", "meminfo", "memtest", "ls", "pwd", "touch", "mkdir", "cat", "write", "ps",
        "taskinfo", "kill", "yield", "net", "ping", "run", "notes", "editor", "runelf", "isolate",
        "ai", "ask",
    ]
}

fn split_command(line: &str) -> (&str, &str) {
    let line = line.trim();
    match line.find(char::is_whitespace) {
        Some(index) => {
            let (command, rest) = line.split_at(index);
            (command.trim(), rest.trim())
        }
        None => (line, ""),
    }
}

fn split_first_token(args: &str) -> Option<(&str, &str)> {
    let args = args.trim();
    if args.is_empty() {
        return None;
    }
    match args.find(char::is_whitespace) {
        Some(index) => {
            let (first, rest) = args.split_at(index);
            Some((first.trim(), rest.trim()))
        }
        None => Some((args, "")),
    }
}

fn parse_u32(text: &str) -> Option<u32> {
    let mut value: u32 = 0;
    for ch in text.bytes() {
        if ch.is_ascii_digit() {
            value = value.saturating_mul(10).saturating_add((ch - b'0') as u32);
        } else {
            return None;
        }
    }
    Some(value)
}

fn print_meminfo(boot_info: &BootInfo, mode: ConsoleMode) {
    let regions = &boot_info.memory_regions;
    let mut usable_bytes: u64 = 0;

    println(mode, "Memory map:");
    for (index, region) in regions.iter().enumerate() {
        let size = region.end.saturating_sub(region.start);
        let kind = match region.kind {
            MemoryRegionKind::Usable => {
                usable_bytes = usable_bytes.saturating_add(size);
                "usable"
            }
            MemoryRegionKind::Bootloader => "bootloader",
            MemoryRegionKind::UnknownBios(_) => "bios",
            MemoryRegionKind::UnknownUefi(_) => "uefi",
            _ => "other",
        };

        print(mode, "  region ");
        print_u64(mode, index as u64);
        print(mode, ": ");
        print_hex(mode, region.start);
        print(mode, "-");
        print_hex(mode, region.end);
        print(mode, " (");
        print_u64(mode, size);
        print(mode, " bytes, ");
        print(mode, kind);
        println(mode, ")");
    }

    println(mode, "");
    print(mode, "Total usable RAM: ");
    print_u64(mode, usable_bytes);
    println(mode, " bytes");
}

fn print(mode: ConsoleMode, text: &str) {
    match mode {
        ConsoleMode::Framebuffer => crate::framebuffer_console::print(text),
        ConsoleMode::Vga => crate::vga_buffer::print(text),
    }
}

fn println(mode: ConsoleMode, text: &str) {
    match mode {
        ConsoleMode::Framebuffer => crate::framebuffer_console::println(text),
        ConsoleMode::Vga => crate::vga_buffer::println(text),
    }
}

fn print_char(mode: ConsoleMode, ch: u8) {
    print(mode, core::str::from_utf8(&[ch]).unwrap_or("?"));
}

fn backspace(mode: ConsoleMode) {
    match mode {
        ConsoleMode::Framebuffer => crate::framebuffer_console::backspace(),
        ConsoleMode::Vga => crate::vga_buffer::backspace(),
    }
}

fn clear_screen(mode: ConsoleMode) {
    match mode {
        ConsoleMode::Framebuffer => crate::framebuffer_console::clear_screen(),
        ConsoleMode::Vga => crate::vga_buffer::clear_screen(),
    }
}

fn print_u64(mode: ConsoleMode, mut value: u64) {
    if value == 0 {
        print_char(mode, b'0');
        return;
    }
    let mut digits = [0u8; 20];
    let mut count = 0;
    while value > 0 {
        digits[count] = b'0' + (value % 10) as u8;
        value /= 10;
        count += 1;
    }
    while count > 0 {
        count -= 1;
        print_char(mode, digits[count]);
    }
}

fn print_i64(mode: ConsoleMode, value: i64) {
    if value < 0 {
        print_char(mode, b'-');
        // `wrapping_neg` rather than plain `-value`: avoids overflow for
        // `i64::MIN`, whose magnitude doesn't fit in an `i64` (this ABI
        // never produces anything near that extreme, but the conversion
        // should not panic even in principle).
        print_u64(mode, value.wrapping_neg() as u64);
    } else {
        print_u64(mode, value as u64);
    }
}

fn print_hex(mode: ConsoleMode, mut value: u64) {
    print(mode, "0x");
    if value == 0 {
        print_char(mode, b'0');
        return;
    }
    let mut digits = [0u8; 16];
    let mut count = 0;
    while value > 0 {
        let nibble = (value & 0xF) as u8;
        digits[count] = if nibble < 10 {
            b'0' + nibble
        } else {
            b'a' + (nibble - 10)
        };
        value >>= 4;
        count += 1;
    }
    while count > 0 {
        count -= 1;
        print_char(mode, digits[count]);
    }
}
