//! Unified input-event queue (Phase 5) -- keyboard and mouse events merged
//! into one bounded queue a Ring 3 desktop process can drain via
//! `SYS_INPUT_POLL`.
//!
//! This is **additive**, not a replacement: `keyboard.rs`'s own `QUEUE`/
//! `poll_key()` (which the text shell's input loop calls directly) is
//! completely untouched. `interrupts::keyboard_interrupt_handler` now
//! pushes into *both* queues from the same scancode -- one decoded
//! `KeyEvent` for the shell, one `InputEvent::KeyDown` for this module --
//! so existing shell behavior has zero dependency on this module and
//! cannot regress from anything added here.

use alloc::collections::VecDeque;

use lazy_static::lazy_static;
use spin::Mutex;

use crate::keyboard::KeyEvent;

/// Same bounded-queue policy as `keyboard::QUEUE` (`keyboard.rs`'s own
/// docs): silently drop new events once full rather than block the
/// producer (an ISR) or grow without limit. A desktop process is expected
/// to drain this every frame, so staying full for any length of time
/// would only happen if nothing is polling at all -- at which point
/// dropping is exactly the right behavior.
const QUEUE_CAPACITY: usize = 64;

/// One decoded input event. Encoded as a fixed 8-byte little-endian record
/// for `SYS_INPUT_POLL` (see `to_le_bytes`) -- deliberately small and
/// fixed-size so the syscall boundary never needs a variable-length,
/// unbounded user buffer.
#[derive(Clone, Copy)]
pub enum InputEvent {
    /// A key was pressed. `code` is the value `to_le_bytes` puts in byte 1
    /// -- either the ASCII byte itself (for `KeyEvent::Char`) or one of the
    /// small control-byte codes below for the handful of non-character
    /// keys this project's keyboard driver decodes. There is no `KeyUp`:
    /// `keyboard.rs`'s scancode decoder does not track release events for
    /// ordinary keys (only for the Shift modifiers, internally) -- adding
    /// full per-key press/release tracking is future work, not implemented
    /// here, and this is documented rather than faked with a synthetic
    /// release that never actually happens.
    KeyDown { code: u8 },
    /// Absolute, screen-clamped cursor position (see `mouse.rs`), not a
    /// delta -- the kernel already tracks and clamps cursor position, so a
    /// desktop process never needs to replicate that bookkeeping or risk
    /// drawing a cursor off-screen.
    MouseMove { x: i16, y: i16 },
    /// `button`: 0 = left, 1 = right, 2 = middle. `pressed`: `true` on
    /// press, `false` on release -- unlike keyboard events, the PS/2 mouse
    /// protocol reports button state on every packet, so release events
    /// are genuinely available here (see `mouse.rs`) and are unconditionally
    /// captured, not just presses.
    MouseButton { button: u8, pressed: bool },
}

/// Control-byte codes `InputEvent::KeyDown` uses for keys that don't have
/// an ordinary printable ASCII value -- chosen from the C0 control range,
/// clear of any printable character `keyboard::KeyEvent::Char` could ever
/// carry (0x20..=0x7E).
pub const KEY_ENTER: u8 = 0x0D;
pub const KEY_BACKSPACE: u8 = 0x08;
pub const KEY_TAB: u8 = 0x09;
pub const KEY_ARROW_UP: u8 = 0x11;
pub const KEY_ARROW_DOWN: u8 = 0x12;

/// Byte length of `InputEvent::to_le_bytes`'s output -- what
/// `syscall::sys_input_poll` checks the caller's destination buffer against
/// before ever touching it.
pub const ENCODED_EVENT_LEN: usize = 8;

impl InputEvent {
    /// Fixed 8-byte encoding: `[tag, a, b, pad, x_lo, x_hi, y_lo, y_hi]`.
    /// `tag`: 1 = KeyDown, 2 = MouseMove, 3 = MouseButton. Unused fields
    /// for a given tag are zeroed, not left undefined, so a consumer that
    /// (incorrectly) reads them anyway still gets a deterministic value
    /// rather than stale/uninitialized-looking bytes.
    pub fn to_le_bytes(self) -> [u8; ENCODED_EVENT_LEN] {
        let mut out = [0u8; ENCODED_EVENT_LEN];
        match self {
            InputEvent::KeyDown { code } => {
                out[0] = 1;
                out[1] = code;
            }
            InputEvent::MouseMove { x, y } => {
                out[0] = 2;
                out[4..6].copy_from_slice(&x.to_le_bytes());
                out[6..8].copy_from_slice(&y.to_le_bytes());
            }
            InputEvent::MouseButton { button, pressed } => {
                out[0] = 3;
                out[1] = button;
                out[2] = pressed as u8;
            }
        }
        out
    }
}

lazy_static! {
    static ref QUEUE: Mutex<VecDeque<InputEvent>> =
        Mutex::new(VecDeque::with_capacity(QUEUE_CAPACITY));
}

/// The only sanctioned way to touch `QUEUE` -- identical reasoning and
/// identical pattern to `keyboard::with_queue`/`task::with_scheduler`:
/// `push` runs from interrupt context (keyboard and mouse ISRs both feed
/// this queue), `poll` runs from ordinary syscall context (interrupts
/// already disabled for the syscall gate's whole duration, but this stays
/// correct and self-contained even if that ever changed) -- without
/// disabling interrupts for the critical section, an IRQ landing at the
/// exact moment the other side held the lock would deadlock the same way
/// every other shared-queue bug in this codebase already did, and was
/// fixed, before this module existed.
fn with_queue<F, R>(f: F) -> R
where
    F: FnOnce(&mut VecDeque<InputEvent>) -> R,
{
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut guard = QUEUE.lock();
        f(&mut guard)
    })
}

/// Push one event in from an ISR (keyboard or mouse). Must stay fast and
/// non-blocking, same requirement as `keyboard::push` -- this only ever
/// does a bounded `VecDeque` push, never allocates on the hot path beyond
/// the queue's own pre-reserved capacity, and never blocks.
pub fn push(event: InputEvent) {
    with_queue(|queue| {
        if queue.len() < QUEUE_CAPACITY {
            queue.push_back(event);
        }
    });
}

/// Drain one queued event, or `None` if empty -- `SYS_INPUT_POLL`'s
/// underlying primitive.
pub fn poll() -> Option<InputEvent> {
    with_queue(|queue| queue.pop_front())
}

/// Translate an already-decoded `keyboard::KeyEvent` into this module's
/// `InputEvent::KeyDown` and push it, if it maps to one at all --
/// `KeyEvent::None` (nothing decoded from this scancode, e.g. a bare Shift
/// press/release) has nothing to push. Called from
/// `interrupts::keyboard_interrupt_handler` immediately after (and
/// completely independently of) the existing `keyboard::on_scancode` call
/// the shell's input path depends on -- see this module's own docs on why
/// that ordering/independence is what makes this purely additive.
pub fn push_key_event(event: KeyEvent) {
    let code = match event {
        KeyEvent::Char(c) => c,
        KeyEvent::Enter => KEY_ENTER,
        KeyEvent::Backspace => KEY_BACKSPACE,
        KeyEvent::Tab => KEY_TAB,
        KeyEvent::ArrowUp => KEY_ARROW_UP,
        KeyEvent::ArrowDown => KEY_ARROW_DOWN,
        KeyEvent::None => return,
    };
    push(InputEvent::KeyDown { code });
}
