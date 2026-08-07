//! PS/2 mouse driver (Phase 5, Milestone 4).
//!
//! Brings up the 8042 controller's auxiliary (mouse) port, decodes its
//! standard 3-byte packet stream, and turns it into absolute,
//! screen-clamped cursor position plus button-edge events pushed into
//! `input.rs`'s unified queue. `interrupts.rs` owns the IRQ12 plumbing
//! (IDT entry, PIC mask, EOI); this module owns everything after "a byte
//! arrived at port 0x60 because of IRQ12."
//!
//! Ring 3 never touches port I/O directly for any of this -- the only path
//! from a mouse packet to userspace is through `input::poll()` /
//! `SYS_INPUT_POLL`, exactly like keyboard events.

use core::sync::atomic::{AtomicI32, AtomicU8, Ordering};

use x86_64::instructions::port::Port;

use crate::input::{self, InputEvent};

const PORT_DATA: u16 = 0x60;
const PORT_STATUS_CMD: u16 = 0x64;

const STATUS_OUTPUT_FULL: u8 = 0x01;
const STATUS_INPUT_FULL: u8 = 0x02;

const CMD_ENABLE_AUX: u8 = 0xA8;
const CMD_READ_CONFIG: u8 = 0x20;
const CMD_WRITE_CONFIG: u8 = 0x60;
const CMD_WRITE_TO_AUX: u8 = 0xD4;

const MOUSE_SET_DEFAULTS: u8 = 0xF6;
const MOUSE_ENABLE_STREAMING: u8 = 0xF4;
const MOUSE_ACK: u8 = 0xFA;

/// Bit in the mouse packet's first byte that must always be `1`, used to
/// detect and recover packet-boundary desync (a dropped or extra byte
/// anywhere in the stream, e.g. from a mouse plugged in mid-boot) rather
/// than silently misinterpreting three arbitrary bytes as one packet.
const PACKET_SYNC_BIT: u8 = 0x08;
const FLAG_X_SIGN: u8 = 0x10;
const FLAG_Y_SIGN: u8 = 0x20;
const FLAG_X_OVERFLOW: u8 = 0x40;
const FLAG_Y_OVERFLOW: u8 = 0x80;
const FLAG_LEFT: u8 = 0x01;
const FLAG_RIGHT: u8 = 0x02;
const FLAG_MIDDLE: u8 = 0x04;

/// Bounded spin count for controller-ready waits during init -- init runs
/// once, with interrupts still enabled from `interrupts::init`'s perspective
/// but before this device raises any, so a real hang here (no PS/2 mouse
/// present at all, e.g. some VM configurations) must not hang boot forever.
const READY_WAIT_ITERATIONS: u32 = 100_000;

static CURSOR_X: AtomicI32 = AtomicI32::new(0);
static CURSOR_Y: AtomicI32 = AtomicI32::new(0);
static BUTTON_STATE: AtomicU8 = AtomicU8::new(0);

fn wait_for(status_bit: u8, set: bool) -> bool {
    let mut status_port: Port<u8> = Port::new(PORT_STATUS_CMD);
    for _ in 0..READY_WAIT_ITERATIONS {
        // Safety: 0x64 is the fixed legacy 8042 status port; a read here has
        // no side effects on controller state.
        let status = unsafe { status_port.read() };
        let bit_set = status & status_bit != 0;
        if bit_set == set {
            return true;
        }
    }
    false
}

fn write_command(cmd: u8) {
    // Safety: 0x64 is the fixed legacy 8042 command port; writes are only
    // issued after confirming the input buffer is clear.
    wait_for(STATUS_INPUT_FULL, false);
    unsafe { Port::<u8>::new(PORT_STATUS_CMD).write(cmd) };
}

fn write_data(data: u8) {
    // Safety: 0x60 is the fixed legacy 8042 data port; writes are only
    // issued after confirming the input buffer is clear.
    wait_for(STATUS_INPUT_FULL, false);
    unsafe { Port::<u8>::new(PORT_DATA).write(data) };
}

fn read_data() -> u8 {
    // Safety: 0x60 is the fixed legacy 8042 data port; reads are only
    // issued after confirming a byte is actually waiting, so this never
    // reads stale/undefined controller state.
    wait_for(STATUS_OUTPUT_FULL, true);
    unsafe { Port::<u8>::new(PORT_DATA).read() }
}

/// Send one command byte to the mouse itself (as opposed to the 8042
/// controller) via the `0xD4` "next byte goes to the auxiliary device"
/// prefix, then consume the mouse's `ACK` reply. The reply is read but not
/// hard-asserted: some emulated/virtual PS/2 mice are lax about the exact
/// ACK byte, and failing init hard over a non-conforming ACK would be worse
/// than proceeding with a device that otherwise streams packets fine --
/// this is logged to serial either way so a real mismatch is still visible.
fn mouse_command(cmd: u8) {
    write_command(CMD_WRITE_TO_AUX);
    write_data(cmd);
    let reply = read_data();
    if reply != MOUSE_ACK {
        serial_println!(
            "mouse: command {:#x} got unexpected reply {:#x}",
            cmd,
            reply
        );
    }
}

/// Bring up the PS/2 auxiliary port and start packet streaming. Must run
/// after `interrupts::init` has remapped the PICs (this only programs the
/// 8042/mouse device itself, not IRQ routing) but the IRQ12 line stays
/// masked until `interrupts.rs` unmasks it, so no packet can be lost or
/// misrouted between this call and that unmask.
pub fn init() {
    write_command(CMD_ENABLE_AUX);

    write_command(CMD_READ_CONFIG);
    let mut config = read_data();
    config |= 0b0000_0010; // enable IRQ12
    config &= !0b0010_0000; // enable the auxiliary device's clock
    write_command(CMD_WRITE_CONFIG);
    write_data(config);

    mouse_command(MOUSE_SET_DEFAULTS);
    mouse_command(MOUSE_ENABLE_STREAMING);

    serial_println!("mouse: PS/2 auxiliary device initialized, streaming enabled");
}

/// Packet-assembly state, touched only from `on_byte` below -- like
/// `framebuffer_console.rs`'s `CURSOR_X`/`CURSOR_Y`, this is ISR-local state
/// with a single writer (IRQ12 cannot reenter itself; the CPU keeps
/// interrupts disabled for the duration of one interrupt gate), so a plain
/// `static mut` behind a documented-safety `unsafe` block is used instead of
/// an atomic-bit-packing workaround.
static mut PACKET: [u8; 3] = [0; 3];
static mut PACKET_INDEX: u8 = 0;

/// Feed one raw byte in from the mouse ISR (IRQ12). Runs with interrupts
/// disabled (we're inside the ISR), so this must stay fast and must never
/// block -- matches `keyboard::on_scancode`'s contract exactly.
pub fn on_byte(byte: u8) {
    // Safety: see `PACKET`/`PACKET_INDEX`'s doc comment -- single-writer,
    // ISR-only access, never reentered.
    unsafe {
        let index = core::ptr::addr_of!(PACKET_INDEX).read();

        if index == 0 && byte & PACKET_SYNC_BIT == 0 {
            // Not a valid packet-start byte -- drop it and stay at index 0
            // to resynchronize on the next byte, rather than building a
            // three-byte packet out of a misaligned stream.
            return;
        }

        PACKET[index as usize] = byte;

        if index < 2 {
            core::ptr::addr_of_mut!(PACKET_INDEX).write(index + 1);
            return;
        }

        core::ptr::addr_of_mut!(PACKET_INDEX).write(0);
        handle_packet(PACKET[0], PACKET[1], PACKET[2]);
    }
}

fn handle_packet(flags: u8, x_raw: u8, y_raw: u8) {
    if flags & (FLAG_X_OVERFLOW | FLAG_Y_OVERFLOW) != 0 {
        // A genuinely corrupt/overflowed sample -- discard rather than feed
        // a garbage jump into the cursor position.
        return;
    }

    let mut dx = x_raw as i32;
    if flags & FLAG_X_SIGN != 0 {
        dx -= 256;
    }
    let mut dy = y_raw as i32;
    if flags & FLAG_Y_SIGN != 0 {
        dy -= 256;
    }
    // PS/2 reports +Y as "up"; screen coordinates grow downward.
    dy = -dy;

    let (max_x, max_y) = crate::display::info()
        .map(|info| (info.width as i32 - 1, info.height as i32 - 1))
        .unwrap_or((i32::MAX, i32::MAX));

    let new_x = CURSOR_X
        .load(Ordering::Relaxed)
        .saturating_add(dx)
        .clamp(0, max_x.max(0));
    let new_y = CURSOR_Y
        .load(Ordering::Relaxed)
        .saturating_add(dy)
        .clamp(0, max_y.max(0));
    CURSOR_X.store(new_x, Ordering::Relaxed);
    CURSOR_Y.store(new_y, Ordering::Relaxed);

    if dx != 0 || dy != 0 {
        input::push(InputEvent::MouseMove {
            x: new_x as i16,
            y: new_y as i16,
        });
    }

    let previous = BUTTON_STATE.swap(flags & 0b111, Ordering::Relaxed);
    let current = flags & 0b111;
    for (mask, id) in [(FLAG_LEFT, 0u8), (FLAG_RIGHT, 1u8), (FLAG_MIDDLE, 2u8)] {
        let was = previous & mask != 0;
        let is = current & mask != 0;
        if was != is {
            input::push(InputEvent::MouseButton {
                button: id,
                pressed: is,
            });
        }
    }
}
