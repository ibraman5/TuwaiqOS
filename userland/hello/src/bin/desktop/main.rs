//! Tuwaiq Desktop: the first real graphical userspace program (Phase 5,
//! Milestones 6-8). A genuine Ring 3 ELF64 process, loaded and run through
//! exactly the same `task::spawn_user_process` path as every other
//! `runelf`-launched program (see `shell.rs`'s `desktop` command) -- no
//! kernel-side special case for this binary.
//!
//! Everything the desktop draws is software-rendered into its own
//! `SYS_MMAP`'d backbuffer (`gfx::Canvas`) and only ever reaches the real
//! screen through `SYS_DISPLAY_PRESENT`, which the kernel validates
//! independently (`display::present`) -- this process never touches
//! privileged hardware, a physical address, or another process's memory.
//! Input arrives the same way: `SYS_INPUT_POLL` drains the kernel's
//! unified keyboard+mouse queue (`input.rs`); there is no raw port I/O
//! here at all.

#![no_std]
#![no_main]

mod font;
mod gfx;
mod sys;
mod window;

use core::arch::global_asm;

use gfx::Canvas;
use hello_user::u64_to_decimal;
use window::{DragResult, PressAction, WindowManager, TITLE_BAR_HEIGHT};

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

// Tuwaiq identity: dark, clean, a single restrained green accent rather
// than a neon palette -- a system bar and desktop, not a demo effect.
const COLOR_BG: (u8, u8, u8) = (0x0B, 0x0E, 0x13);
const COLOR_BAR: (u8, u8, u8) = (0x12, 0x16, 0x1D);
const COLOR_ACCENT: (u8, u8, u8) = (0x00, 0xA8, 0x6B);
const COLOR_TEXT: (u8, u8, u8) = (0xE6, 0xEA, 0xF0);
const COLOR_TEXT_DIM: (u8, u8, u8) = (0x8A, 0x93, 0xA0);
const COLOR_WINDOW: (u8, u8, u8) = (0x17, 0x1B, 0x24);
const COLOR_TITLEBAR: (u8, u8, u8) = (0x1E, 0x24, 0x30);
const COLOR_BORDER: (u8, u8, u8) = (0x2A, 0x31, 0x40);
const COLOR_LAUNCHER: (u8, u8, u8) = (0x0F, 0x7A, 0x50);
const COLOR_CLOSE: (u8, u8, u8) = (0xB0, 0x3A, 0x3A);

const SYSTEM_BAR_HEIGHT: i32 = 28;
/// Matches `interrupts::TIMER_HZ` on the kernel side (see `sys::uptime_ticks`'s
/// docs for why this is a documented assumption rather than a syscall).
const TIMER_HZ: u64 = 100;

const LAUNCHER_X: i32 = 8;
const LAUNCHER_Y: i32 = 4;
const LAUNCHER_W: i32 = 96;
const LAUNCHER_H: i32 = 20;

/// Rolling log of recently typed printable characters -- the desktop's
/// visible proof that keyboard input actually reaches it (Milestone 6's
/// "keyboard input must produce a visible response" requirement).
const KEY_LOG_LEN: usize = 40;

struct DesktopState {
    cursor_x: i32,
    cursor_y: i32,
    left_down: bool,
    wm: WindowManager,
    key_log: [u8; KEY_LOG_LEN],
    key_log_len: usize,
    next_panel: u32,
}

extern "C" fn rust_main() -> ! {
    hello_user::write(b"desktop: started\n");
    match window::self_test() {
        Ok(()) => hello_user::write(b"desktop: window model self-test PASS\n"),
        Err(reason) => {
            hello_user::write(b"desktop: window model self-test FAIL: ");
            hello_user::write(reason.as_bytes());
            hello_user::write(b"\n");
            sys::exit(4);
        }
    }

    let Some(info) = sys::display_info() else {
        // No framebuffer active (VGA-text boot) -- nothing a graphical
        // desktop can do; exit cleanly rather than spin.
        sys::exit(1);
    };

    let buffer_len = info.buffer_len();
    let mapped_len = match buffer_len.checked_add(4095) {
        Some(len) => len & !4095,
        None => sys::exit(1),
    };
    let Some(addr) = sys::mmap(buffer_len, true) else {
        sys::exit(1);
    };

    // Safety: `mmap` just returned this exact address as a writable
    // mapping of at least `buffer_len` bytes in this process's own address
    // space; nothing else in this single-threaded process touches it
    // concurrently.
    let buf = unsafe { core::slice::from_raw_parts_mut(addr as *mut u8, buffer_len as usize) };
    let mut canvas = Canvas { buf, info };

    let mut state = DesktopState {
        cursor_x: (info.width / 2) as i32,
        cursor_y: (info.height / 2) as i32,
        left_down: false,
        wm: WindowManager::new(),
        key_log: [0; KEY_LOG_LEN],
        key_log_len: 0,
        next_panel: 1,
    };

    state.wm.spawn(120, 90, 340, 180, b"Welcome to TuwaiqOS");

    let mut last_clock_second = u64::MAX;
    let mut first_frame = true;

    loop {
        let (input_dirty, exit_requested) = pump_input(&mut state, &info);
        if exit_requested {
            hello_user::write(b"desktop: normal exit requested\n");
            if !sys::munmap(addr, mapped_len) {
                hello_user::write(b"desktop: backbuffer cleanup failed\n");
                sys::exit(2);
            }
            sys::exit(0);
        }

        let clock_second = sys::uptime_ticks() / TIMER_HZ;
        if first_frame || input_dirty || clock_second != last_clock_second {
            redraw(&mut canvas, &state, clock_second);
            // Safety: `canvas.buf` is exactly the mapping `SYS_MMAP`
            // returned, presented at the display's exact byte length.
            let presented = unsafe { sys::display_present(addr, buffer_len) };
            if !presented {
                hello_user::write(b"desktop: display present failed\n");
                let _ = sys::munmap(addr, mapped_len);
                sys::exit(3);
            }
            if first_frame {
                hello_user::write(b"desktop: first frame presented\n");
            }
            first_frame = false;
            last_clock_second = clock_second;
        }

        // Even an idle desktop always yields. Avoiding needless full-screen
        // redraw/present work improves input latency without extending its
        // time slice or weakening round-robin fairness for peer processes.
        sys::yield_now();
    }
}

fn pump_input(state: &mut DesktopState, info: &sys::DisplayInfo) -> (bool, bool) {
    let mut buf = [0u8; 8];
    let mut dirty = false;
    // Drain every queued event this frame rather than just one, so a burst
    // of mouse packets (the PS/2 controller can deliver several between
    // two of this process's time slices) doesn't visibly lag the cursor.
    while sys::input_poll(&mut buf) {
        match buf[0] {
            1 if buf[1] == sys::KEY_ESCAPE => return (dirty, true),
            1 => dirty |= on_key_down(state, buf[1]),
            2 => {
                let x = i16::from_le_bytes([buf[4], buf[5]]) as i32;
                let y = i16::from_le_bytes([buf[6], buf[7]]) as i32;
                let new_x = x.clamp(0, info.width as i32 - 1);
                let new_y = y.clamp(0, info.height as i32 - 1);
                dirty |= new_x != state.cursor_x || new_y != state.cursor_y;
                state.cursor_x = new_x;
                state.cursor_y = new_y;
                if state.left_down {
                    state.wm.handle_drag(state.cursor_x, state.cursor_y);
                }
            }
            3 => {
                let button = buf[1];
                let pressed = buf[2] != 0;
                if button == 0 {
                    on_left_button(state, pressed);
                    dirty = true;
                }
            }
            _ => {}
        }
    }
    (dirty, false)
}

fn on_key_down(state: &mut DesktopState, code: u8) -> bool {
    if !(0x20..=0x7E).contains(&code) {
        return false;
    }
    if state.key_log_len < KEY_LOG_LEN {
        state.key_log[state.key_log_len] = code;
        state.key_log_len += 1;
    } else {
        state.key_log.copy_within(1.., 0);
        state.key_log[KEY_LOG_LEN - 1] = code;
    }
    true
}

fn on_left_button(state: &mut DesktopState, pressed: bool) {
    if !pressed {
        state.left_down = false;
        if let Some(drag) = state.wm.handle_release() {
            emit_drag_marker(drag);
        }
        return;
    }
    state.left_down = true;

    if point_in_rect(
        state.cursor_x,
        state.cursor_y,
        LAUNCHER_X,
        LAUNCHER_Y,
        LAUNCHER_W,
        LAUNCHER_H,
    ) {
        spawn_panel(state);
        return;
    }

    match state.wm.handle_press(state.cursor_x, state.cursor_y) {
        PressAction::Closed { window_index } => emit_close_marker(window_index),
        PressAction::Raised { window_index } => {
            emit_action_marker(b"desktop: focus window=", window_index)
        }
        PressAction::None | PressAction::DragStarted { .. } => {}
    }
}

fn emit_drag_marker(drag: DragResult) {
    let mut line = [0u8; 160];
    let mut len = 0;
    append_bytes(&mut line, &mut len, b"desktop: drag window=");
    append_u64(&mut line, &mut len, drag.window_index as u64);
    append_bytes(&mut line, &mut len, b" from=(");
    append_i32(&mut line, &mut len, drag.from_x);
    append_bytes(&mut line, &mut len, b",");
    append_i32(&mut line, &mut len, drag.from_y);
    append_bytes(&mut line, &mut len, b") to=(");
    append_i32(&mut line, &mut len, drag.to_x);
    append_bytes(&mut line, &mut len, b",");
    append_i32(&mut line, &mut len, drag.to_y);
    append_bytes(&mut line, &mut len, b") delta=(");
    append_i32(&mut line, &mut len, drag.to_x.saturating_sub(drag.from_x));
    append_bytes(&mut line, &mut len, b",");
    append_i32(&mut line, &mut len, drag.to_y.saturating_sub(drag.from_y));
    append_bytes(&mut line, &mut len, b")\n");
    hello_user::write(&line[..len]);
}

fn emit_close_marker(window_index: usize) {
    emit_action_marker(b"desktop: close window=", window_index);
}

fn emit_action_marker(prefix: &[u8], window_index: usize) {
    let mut line = [0u8; 48];
    let mut len = 0;
    append_bytes(&mut line, &mut len, prefix);
    append_u64(&mut line, &mut len, window_index as u64);
    append_bytes(&mut line, &mut len, b"\n");
    hello_user::write(&line[..len]);
}

fn append_bytes(out: &mut [u8], len: &mut usize, bytes: &[u8]) {
    let available = out.len().saturating_sub(*len);
    let count = bytes.len().min(available);
    out[*len..*len + count].copy_from_slice(&bytes[..count]);
    *len += count;
}

fn append_u64(out: &mut [u8], len: &mut usize, value: u64) {
    let mut digits = [0u8; 20];
    append_bytes(out, len, u64_to_decimal(value, &mut digits));
}

fn append_i32(out: &mut [u8], len: &mut usize, value: i32) {
    if value < 0 {
        append_bytes(out, len, b"-");
    }
    append_u64(out, len, value.unsigned_abs() as u64);
}

fn spawn_panel(state: &mut DesktopState) {
    let offset = (state.next_panel as i32 % 5) * 24;
    let mut title = [0u8; 12];
    title[..6].copy_from_slice(b"Panel ");
    let mut digits = [0u8; 20];
    let text = u64_to_decimal(state.next_panel as u64, &mut digits);
    let len = text.len().min(title.len() - 6);
    title[6..6 + len].copy_from_slice(&text[..len]);
    let idx = state
        .wm
        .spawn(160 + offset, 130 + offset, 260, 140, &title[..6 + len]);
    if let Some(idx) = idx {
        emit_action_marker(b"desktop: launcher window=", idx);
        state.next_panel += 1;
    }
}

fn point_in_rect(px: i32, py: i32, x: i32, y: i32, w: i32, h: i32) -> bool {
    px >= x && px < x + w && py >= y && py < y + h
}

fn redraw(canvas: &mut Canvas, state: &DesktopState, clock_second: u64) {
    let width = canvas.info.width as i32;
    let height = canvas.info.height as i32;

    canvas.fill_rect(0, 0, width, height, COLOR_BG.0, COLOR_BG.1, COLOR_BG.2);

    draw_system_bar(canvas, width, clock_second);

    for &idx in state.wm.draw_order() {
        let window = *state.wm.window(idx);
        if !window.visible {
            continue;
        }
        draw_window(canvas, &window);
    }

    if state.key_log_len > 0 {
        canvas.draw_text(
            &state.key_log[..state.key_log_len],
            16,
            height - 26,
            COLOR_TEXT_DIM.0,
            COLOR_TEXT_DIM.1,
            COLOR_TEXT_DIM.2,
        );
    }

    canvas.draw_cursor(state.cursor_x, state.cursor_y);
}

fn draw_system_bar(canvas: &mut Canvas, width: i32, seconds: u64) {
    canvas.fill_rect(
        0,
        0,
        width,
        SYSTEM_BAR_HEIGHT,
        COLOR_BAR.0,
        COLOR_BAR.1,
        COLOR_BAR.2,
    );
    canvas.fill_rect(
        0,
        SYSTEM_BAR_HEIGHT - 2,
        width,
        2,
        COLOR_ACCENT.0,
        COLOR_ACCENT.1,
        COLOR_ACCENT.2,
    );

    canvas.fill_rect(
        LAUNCHER_X,
        LAUNCHER_Y,
        LAUNCHER_W,
        LAUNCHER_H,
        COLOR_LAUNCHER.0,
        COLOR_LAUNCHER.1,
        COLOR_LAUNCHER.2,
    );
    canvas.draw_text(
        b"+ Launch",
        LAUNCHER_X + 6,
        LAUNCHER_Y + 6,
        COLOR_TEXT.0,
        COLOR_TEXT.1,
        COLOR_TEXT.2,
    );

    canvas.draw_text(
        b"TuwaiqOS",
        LAUNCHER_X + LAUNCHER_W + 20,
        LAUNCHER_Y + 6,
        COLOR_TEXT.0,
        COLOR_TEXT.1,
        COLOR_TEXT.2,
    );

    canvas.draw_text(
        b"Esc: Exit",
        LAUNCHER_X + LAUNCHER_W + 120,
        LAUNCHER_Y + 6,
        COLOR_TEXT_DIM.0,
        COLOR_TEXT_DIM.1,
        COLOR_TEXT_DIM.2,
    );

    let hh = (seconds / 3600) % 24;
    let mm = (seconds / 60) % 60;
    let ss = seconds % 60;
    let mut clock = [0u8; 8];
    clock[0] = b'0' + (hh / 10) as u8;
    clock[1] = b'0' + (hh % 10) as u8;
    clock[2] = b':';
    clock[3] = b'0' + (mm / 10) as u8;
    clock[4] = b'0' + (mm % 10) as u8;
    clock[5] = b':';
    clock[6] = b'0' + (ss / 10) as u8;
    clock[7] = b'0' + (ss % 10) as u8;
    canvas.draw_text(
        &clock,
        width - 8 - (clock.len() as i32) * 9,
        LAUNCHER_Y + 6,
        COLOR_TEXT.0,
        COLOR_TEXT.1,
        COLOR_TEXT.2,
    );
}

fn draw_window(canvas: &mut Canvas, window: &window::Window) {
    canvas.fill_rect(
        window.x,
        window.y,
        window.w,
        window.h,
        COLOR_WINDOW.0,
        COLOR_WINDOW.1,
        COLOR_WINDOW.2,
    );
    canvas.fill_rect(
        window.x,
        window.y,
        window.w,
        TITLE_BAR_HEIGHT,
        COLOR_TITLEBAR.0,
        COLOR_TITLEBAR.1,
        COLOR_TITLEBAR.2,
    );
    canvas.stroke_rect(
        window.x,
        window.y,
        window.w,
        window.h,
        COLOR_BORDER.0,
        COLOR_BORDER.1,
        COLOR_BORDER.2,
    );
    canvas.draw_text(
        window.title_bytes(),
        window.x + 6,
        window.y + 6,
        COLOR_TEXT.0,
        COLOR_TEXT.1,
        COLOR_TEXT.2,
    );
    canvas.fill_rect(
        window.x + window.w - 18,
        window.y + 4,
        14,
        14,
        COLOR_CLOSE.0,
        COLOR_CLOSE.1,
        COLOR_CLOSE.2,
    );
    canvas.draw_text(
        b"Type to see keys echoed at the bottom of the screen.",
        window.x + 10,
        window.y + TITLE_BAR_HEIGHT + 16,
        COLOR_TEXT_DIM.0,
        COLOR_TEXT_DIM.1,
        COLOR_TEXT_DIM.2,
    );
    canvas.draw_text(
        b"Drag this title bar to move the window.",
        window.x + 10,
        window.y + TITLE_BAR_HEIGHT + 34,
        COLOR_TEXT_DIM.0,
        COLOR_TEXT_DIM.1,
        COLOR_TEXT_DIM.2,
    );
}
