//! PS/2 keyboard input (interrupt-driven).
//!
//! Scancodes arrive one byte at a time from `interrupts::keyboard_interrupt_handler`
//! (IRQ1), are decoded here into `KeyEvent`s, and queued for the shell to
//! drain with `poll_key()`. This replaces the v0.5 implementation, which
//! re-read ports 0x60/0x64 in a tight loop from the shell's own input loop;
//! `poll_key()` keeps its exact old signature so `shell.rs` needed no changes.
//!
//! Supports printable keys, Shift modifiers, arrow keys, and Tab.

use alloc::collections::VecDeque;
use core::sync::atomic::{AtomicBool, Ordering};

use lazy_static::lazy_static;
use spin::Mutex;

/// A decoded keyboard event for the shell.
#[derive(Clone, Copy)]
pub enum KeyEvent {
    Char(u8),
    Enter,
    Backspace,
    ArrowUp,
    ArrowDown,
    Tab,
    None,
}

static LEFT_SHIFT: AtomicBool = AtomicBool::new(false);
static RIGHT_SHIFT: AtomicBool = AtomicBool::new(false);
/// Set after seeing the 0xE0 extended-scancode prefix; the *next* byte
/// delivered by IRQ1 completes that two-byte sequence.
static EXTENDED_PENDING: AtomicBool = AtomicBool::new(false);

fn shift_active() -> bool {
    LEFT_SHIFT.load(Ordering::Relaxed) || RIGHT_SHIFT.load(Ordering::Relaxed)
}

const QUEUE_CAPACITY: usize = 32;

lazy_static! {
    static ref QUEUE: Mutex<VecDeque<KeyEvent>> =
        Mutex::new(VecDeque::with_capacity(QUEUE_CAPACITY));
}

/// The only sanctioned way to touch `QUEUE`. `push` runs inside the
/// keyboard ISR (interrupts already disabled by the CPU); `poll_key` runs
/// in ordinary shell context with interrupts enabled. Without disabling
/// interrupts here, a keyboard IRQ landing at the exact moment `poll_key`
/// held this lock would deadlock: `on_scancode`'s own attempt to lock
/// `QUEUE` inside the ISR would spin forever waiting for a lock that can
/// only be released by `poll_key` finishing -- which can't happen until
/// the ISR itself returns via `iretq`. Same bug class, and same fix, as
/// `task::with_scheduler` and the interrupt-safe heap allocator.
fn with_queue<F, R>(f: F) -> R
where
    F: FnOnce(&mut VecDeque<KeyEvent>) -> R,
{
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut guard = QUEUE.lock();
        f(&mut guard)
    })
}

/// Feed one raw scancode byte in from the keyboard ISR. Runs with
/// interrupts disabled (we're inside the ISR), so this must stay fast and
/// must never block.
pub fn on_scancode(scancode: u8) {
    if EXTENDED_PENDING.swap(false, Ordering::Relaxed) {
        if let Some(event) = translate_extended(scancode) {
            push(event);
            // Additive fan-out into the Phase 5 unified input queue -- see
            // `input.rs`'s module docs for why this cannot regress this
            // (unchanged) shell-facing queue/path.
            crate::input::push_key_event(event);
        }
        return;
    }

    if scancode == 0xE0 {
        EXTENDED_PENDING.store(true, Ordering::Relaxed);
        return;
    }

    if let Some(event) = translate_scancode(scancode) {
        push(event);
        crate::input::push_key_event(event);
    }
}

fn push(event: KeyEvent) {
    with_queue(|queue| {
        if queue.len() < QUEUE_CAPACITY {
            queue.push_back(event);
        }
        // Silently drop when full: better to lose an unread keystroke than
        // to block the ISR or grow the queue unbounded.
    });
}

/// Drain one queued key event. Returns `KeyEvent::None` immediately if
/// nothing is waiting -- callers that want to idle instead of spin should
/// call `interrupts::halt()` on `None` (see `shell::run`).
pub fn poll_key() -> KeyEvent {
    with_queue(|queue| queue.pop_front()).unwrap_or(KeyEvent::None)
}

fn translate_extended(scancode: u8) -> Option<KeyEvent> {
    if scancode & 0x80 != 0 {
        return None;
    }
    match scancode {
        0x48 => Some(KeyEvent::ArrowUp),
        0x50 => Some(KeyEvent::ArrowDown),
        _ => None,
    }
}

fn translate_scancode(scancode: u8) -> Option<KeyEvent> {
    if scancode & 0x80 != 0 {
        match scancode {
            0xAA => LEFT_SHIFT.store(false, Ordering::Relaxed),
            0xB6 => RIGHT_SHIFT.store(false, Ordering::Relaxed),
            _ => {}
        }
        return None;
    }

    match scancode {
        0x2A => {
            LEFT_SHIFT.store(true, Ordering::Relaxed);
            None
        }
        0x36 => {
            RIGHT_SHIFT.store(true, Ordering::Relaxed);
            None
        }
        0x1C => Some(KeyEvent::Enter),
        0x0E => Some(KeyEvent::Backspace),
        0x0F => Some(KeyEvent::Tab),
        0x39 => Some(KeyEvent::Char(b' ')),
        0x02 => emit_pair(b'1', b'!'),
        0x03 => emit_pair(b'2', b'@'),
        0x04 => emit_pair(b'3', b'#'),
        0x05 => emit_pair(b'4', b'$'),
        0x06 => emit_pair(b'5', b'%'),
        0x07 => emit_pair(b'6', b'^'),
        0x08 => emit_pair(b'7', b'&'),
        0x09 => emit_pair(b'8', b'*'),
        0x0A => emit_pair(b'9', b'('),
        0x0B => emit_pair(b'0', b')'),
        0x0C => emit_pair(b'-', b'_'),
        0x0D => emit_pair(b'=', b'+'),
        0x29 => emit_pair(b'`', b'~'),
        0x1A => emit_pair(b'[', b'{'),
        0x1B => emit_pair(b']', b'}'),
        0x2B => emit_pair(b'\\', b'|'),
        0x27 => emit_pair(b';', b':'),
        0x28 => emit_pair(b'\'', b'"'),
        0x33 => emit_pair(b',', b'<'),
        0x34 => emit_pair(b'.', b'>'),
        0x35 => emit_pair(b'/', b'?'),
        0x10 => emit_letter(b'q'),
        0x11 => emit_letter(b'w'),
        0x12 => emit_letter(b'e'),
        0x13 => emit_letter(b'r'),
        0x14 => emit_letter(b't'),
        0x15 => emit_letter(b'y'),
        0x16 => emit_letter(b'u'),
        0x17 => emit_letter(b'i'),
        0x18 => emit_letter(b'o'),
        0x19 => emit_letter(b'p'),
        0x1E => emit_letter(b'a'),
        0x1F => emit_letter(b's'),
        0x20 => emit_letter(b'd'),
        0x21 => emit_letter(b'f'),
        0x22 => emit_letter(b'g'),
        0x23 => emit_letter(b'h'),
        0x24 => emit_letter(b'j'),
        0x25 => emit_letter(b'k'),
        0x26 => emit_letter(b'l'),
        0x2C => emit_letter(b'z'),
        0x2D => emit_letter(b'x'),
        0x2E => emit_letter(b'c'),
        0x2F => emit_letter(b'v'),
        0x30 => emit_letter(b'b'),
        0x31 => emit_letter(b'n'),
        0x32 => emit_letter(b'm'),
        _ => None,
    }
}

fn emit_pair(normal: u8, shifted: u8) -> Option<KeyEvent> {
    Some(KeyEvent::Char(if shift_active() {
        shifted
    } else {
        normal
    }))
}

fn emit_letter(lower: u8) -> Option<KeyEvent> {
    Some(KeyEvent::Char(if shift_active() {
        lower - b'a' + b'A'
    } else {
        lower
    }))
}
