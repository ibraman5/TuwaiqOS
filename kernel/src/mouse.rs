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

use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, AtomicU8, Ordering};

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
const MOUSE_SET_SAMPLE_RATE: u8 = 0xF3;
const MOUSE_ENABLE_STREAMING: u8 = 0xF4;
const MOUSE_ACK: u8 = 0xFA;
const MOUSE_RESEND: u8 = 0xFE;

/// The standard PS/2 device accepts 10, 20, 40, 60, 80, 100, or 200 Hz.
/// Use its highest defined sampling rate to reduce device-side movement
/// latency without changing PIT frequency, scheduler time slices, or the
/// amount of work done for any one packet.
const MOUSE_SAMPLE_RATE_HZ: u8 = 200;
const MOUSE_COMMAND_ATTEMPTS: usize = 3;

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

/// Bounded spin count for controller-ready waits during init. `main.rs` keeps
/// interrupts disabled across the shared 8042 transaction so IRQ1 cannot
/// consume a reply; a missing PS/2 mouse must still fail within this bound
/// instead of hanging boot forever.
const READY_WAIT_ITERATIONS: u32 = 100_000;

static CURSOR_X: AtomicI32 = AtomicI32::new(0);
static CURSOR_Y: AtomicI32 = AtomicI32::new(0);
static BUTTON_STATE: AtomicU8 = AtomicU8::new(0);
static INITIALIZED: AtomicBool = AtomicBool::new(false);
static RAW_BYTES: AtomicU64 = AtomicU64::new(0);
static COMPLETE_PACKETS: AtomicU64 = AtomicU64::new(0);
static DESYNC_BYTES: AtomicU64 = AtomicU64::new(0);
static OVERFLOW_PACKETS: AtomicU64 = AtomicU64::new(0);

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

fn write_command(cmd: u8) -> bool {
    // Safety: 0x64 is the fixed legacy 8042 command port; writes are only
    // issued after confirming the input buffer is clear.
    if !wait_for(STATUS_INPUT_FULL, false) {
        return false;
    }
    unsafe { Port::<u8>::new(PORT_STATUS_CMD).write(cmd) };
    true
}

fn write_data(data: u8) -> bool {
    // Safety: 0x60 is the fixed legacy 8042 data port; writes are only
    // issued after confirming the input buffer is clear.
    if !wait_for(STATUS_INPUT_FULL, false) {
        return false;
    }
    unsafe { Port::<u8>::new(PORT_DATA).write(data) };
    true
}

fn read_data() -> Option<u8> {
    // Safety: 0x60 is the fixed legacy 8042 data port; reads are only
    // issued after confirming a byte is actually waiting, so this never
    // reads stale/undefined controller state.
    if !wait_for(STATUS_OUTPUT_FULL, true) {
        return None;
    }
    Some(unsafe { Port::<u8>::new(PORT_DATA).read() })
}

/// Send one command byte to the mouse itself (as opposed to the 8042
/// controller) via the `0xD4` "next byte goes to the auxiliary device"
/// prefix, then consume the mouse's `ACK` reply. `RESEND` is retried a
/// bounded number of times. A timeout or any other reply fails closed:
/// IRQ12 is not considered ready and no unbounded wait can stall boot.
fn mouse_command(cmd: u8) -> bool {
    for _ in 0..MOUSE_COMMAND_ATTEMPTS {
        if !write_command(CMD_WRITE_TO_AUX) || !write_data(cmd) {
            serial_println!("mouse: command {:#x} timed out while writing", cmd);
            return false;
        }

        match read_data() {
            Some(MOUSE_ACK) => return true,
            Some(MOUSE_RESEND) => continue,
            Some(reply) => {
                serial_println!(
                    "mouse: command {:#x} got unexpected reply {:#x}",
                    cmd,
                    reply
                );
                return false;
            }
            None => {
                serial_println!("mouse: command {:#x} timed out waiting for ACK", cmd);
                return false;
            }
        }
    }

    serial_println!(
        "mouse: command {:#x} exceeded {} RESEND attempts",
        cmd,
        MOUSE_COMMAND_ATTEMPTS
    );
    false
}

/// Bring up the PS/2 auxiliary port and start packet streaming. Must run
/// after `interrupts::init` has remapped the PICs (this only programs the
/// 8042/mouse device itself, not IRQ routing) but the IRQ12 line stays
/// masked until `interrupts.rs` unmasks it, so no packet can be lost or
/// misrouted between this call and that unmask.
pub fn init() -> bool {
    INITIALIZED.store(false, Ordering::Release);

    if !write_command(CMD_ENABLE_AUX) || !write_command(CMD_READ_CONFIG) {
        serial_println!("mouse: PS/2 controller unavailable during auxiliary-port setup");
        return false;
    }

    let Some(mut config) = read_data() else {
        serial_println!("mouse: PS/2 controller timed out reading configuration");
        return false;
    };
    config |= 0b0000_0010; // enable IRQ12
    config &= !0b0010_0000; // enable the auxiliary device's clock
    if !write_command(CMD_WRITE_CONFIG) || !write_data(config) {
        serial_println!("mouse: PS/2 controller timed out writing configuration");
        return false;
    }

    if !mouse_command(MOUSE_SET_DEFAULTS)
        || !mouse_command(MOUSE_SET_SAMPLE_RATE)
        || !mouse_command(MOUSE_SAMPLE_RATE_HZ)
        || !mouse_command(MOUSE_ENABLE_STREAMING)
    {
        serial_println!("mouse: auxiliary device initialization failed; input disabled");
        return false;
    }

    INITIALIZED.store(true, Ordering::Release);
    serial_println!(
        "mouse: PS/2 auxiliary device initialized, streaming enabled at {} Hz",
        MOUSE_SAMPLE_RATE_HZ
    );
    true
}

/// Whether the auxiliary device completed its bounded initialization and
/// acknowledged streaming. Callers can use this to avoid unmasking IRQ12
/// when no PS/2 mouse is present.
pub fn is_initialized() -> bool {
    INITIALIZED.load(Ordering::Acquire)
}

#[derive(Clone, Copy)]
struct PacketDecoder {
    bytes: [u8; 3],
    index: usize,
}

impl PacketDecoder {
    const fn new() -> Self {
        Self {
            bytes: [0; 3],
            index: 0,
        }
    }

    fn feed(&mut self, byte: u8) -> Option<[u8; 3]> {
        if self.index == 0 && byte & PACKET_SYNC_BIT == 0 {
            return None;
        }
        self.bytes[self.index] = byte;
        self.index += 1;
        if self.index != self.bytes.len() {
            return None;
        }
        self.index = 0;
        Some(self.bytes)
    }
}

#[derive(Clone, Copy)]
struct MouseState {
    x: i32,
    y: i32,
    buttons: u8,
}

#[derive(Clone, Copy)]
struct DecodedPacket {
    x: i32,
    y: i32,
    moved: bool,
    buttons: u8,
    changed_buttons: u8,
}

fn decode_packet(
    packet: [u8; 3],
    previous: MouseState,
    max_x: i32,
    max_y: i32,
) -> Option<DecodedPacket> {
    let [flags, x_raw, y_raw] = packet;
    if flags & (FLAG_X_OVERFLOW | FLAG_Y_OVERFLOW) != 0 {
        return None;
    }

    let dx = if flags & FLAG_X_SIGN != 0 {
        i32::from(x_raw) - 256
    } else {
        i32::from(x_raw)
    };
    let ps2_dy = if flags & FLAG_Y_SIGN != 0 {
        i32::from(y_raw) - 256
    } else {
        i32::from(y_raw)
    };
    let dy = -ps2_dy;
    let x = previous.x.saturating_add(dx).clamp(0, max_x.max(0));
    let y = previous.y.saturating_add(dy).clamp(0, max_y.max(0));
    let buttons = flags & 0b111;

    Some(DecodedPacket {
        x,
        y,
        moved: x != previous.x || y != previous.y,
        buttons,
        changed_buttons: previous.buttons ^ buttons,
    })
}

/// Packet-assembly state is IRQ12-local. The handler cannot reenter itself,
/// so this single writer is safe without a lock; diagnostics use a separate
/// local decoder and never touch live ISR state.
static mut DECODER: PacketDecoder = PacketDecoder::new();

/// Feed one raw byte in from the mouse ISR (IRQ12). Runs with interrupts
/// disabled (we're inside the ISR), so this must stay fast and must never
/// block -- matches `keyboard::on_scancode`'s contract exactly.
pub fn on_byte(byte: u8) {
    RAW_BYTES.fetch_add(1, Ordering::Relaxed);
    // Safety: `DECODER` has one non-reentrant IRQ writer, documented above.
    let decoder = unsafe { &mut *core::ptr::addr_of_mut!(DECODER) };
    if decoder.index == 0 && byte & PACKET_SYNC_BIT == 0 {
        DESYNC_BYTES.fetch_add(1, Ordering::Relaxed);
    }
    let packet = decoder.feed(byte);
    if let Some(packet) = packet {
        COMPLETE_PACKETS.fetch_add(1, Ordering::Relaxed);
        handle_packet(packet);
    }
}

fn handle_packet(packet: [u8; 3]) {
    if packet[0] & (FLAG_X_OVERFLOW | FLAG_Y_OVERFLOW) != 0 {
        OVERFLOW_PACKETS.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let (max_x, max_y) = crate::display::info()
        .map(|info| {
            (
                i32::try_from(info.width)
                    .unwrap_or(i32::MAX)
                    .saturating_sub(1),
                i32::try_from(info.height)
                    .unwrap_or(i32::MAX)
                    .saturating_sub(1),
            )
        })
        .unwrap_or((i32::MAX, i32::MAX));
    let previous = MouseState {
        x: CURSOR_X.load(Ordering::Relaxed),
        y: CURSOR_Y.load(Ordering::Relaxed),
        buttons: BUTTON_STATE.load(Ordering::Relaxed),
    };
    let Some(decoded) = decode_packet(packet, previous, max_x, max_y) else {
        return;
    };
    CURSOR_X.store(decoded.x, Ordering::Relaxed);
    CURSOR_Y.store(decoded.y, Ordering::Relaxed);
    BUTTON_STATE.store(decoded.buttons, Ordering::Relaxed);

    if decoded.moved {
        input::push(InputEvent::MouseMove {
            x: decoded.x.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
            y: decoded.y.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
        });
    }
    for (mask, id) in [(FLAG_LEFT, 0u8), (FLAG_RIGHT, 1u8), (FLAG_MIDDLE, 2u8)] {
        if decoded.changed_buttons & mask != 0 {
            input::push(InputEvent::MouseButton {
                button: id,
                pressed: decoded.buttons & mask != 0,
            });
        }
    }
}

#[derive(Clone, Copy)]
pub struct MouseTelemetry {
    pub raw_bytes: u64,
    pub complete_packets: u64,
    pub desync_bytes: u64,
    pub overflow_packets: u64,
}

pub fn telemetry() -> MouseTelemetry {
    MouseTelemetry {
        raw_bytes: RAW_BYTES.load(Ordering::Relaxed),
        complete_packets: COMPLETE_PACKETS.load(Ordering::Relaxed),
        desync_bytes: DESYNC_BYTES.load(Ordering::Relaxed),
        overflow_packets: OVERFLOW_PACKETS.load(Ordering::Relaxed),
    }
}

pub fn cursor_position() -> (i32, i32) {
    (
        CURSOR_X.load(Ordering::Relaxed),
        CURSOR_Y.load(Ordering::Relaxed),
    )
}

/// Acceptance-only packet injection that still traverses the production
/// assembler/decoder and input queue. Deltas are screen-relative (positive Y
/// is down); this converts them back to PS/2's positive-Y-is-up wire format.
pub fn inject_screen_packet(dx: i16, dy: i16, buttons: u8) -> Result<(), &'static str> {
    if !(-256..=255).contains(&i32::from(dx))
        || !(-256..=255).contains(&i32::from(dy))
        || buttons & !0b111 != 0
    {
        return Err("injected packet is outside standard PS/2 bounds");
    }
    let ps2_dy = -i32::from(dy);
    if !(-256..=255).contains(&ps2_dy) {
        return Err("injected Y delta is outside standard PS/2 bounds");
    }
    let mut flags = PACKET_SYNC_BIT | buttons;
    if dx < 0 {
        flags |= FLAG_X_SIGN;
    }
    if ps2_dy < 0 {
        flags |= FLAG_Y_SIGN;
    }
    let x_raw = i32::from(dx).rem_euclid(256) as u8;
    let y_raw = ps2_dy.rem_euclid(256) as u8;
    x86_64::instructions::interrupts::without_interrupts(|| {
        on_byte(flags);
        on_byte(x_raw);
        on_byte(y_raw);
    });
    Ok(())
}

pub fn reset_decoder_for_diagnostics() {
    x86_64::instructions::interrupts::without_interrupts(|| {
        // Safety: interrupts are disabled, so IRQ12 cannot access its sole
        // packet-assembly state while the diagnostic stream establishes a
        // known packet boundary.
        unsafe {
            core::ptr::addr_of_mut!(DECODER).write(PacketDecoder::new());
        }
    });
}

/// Pure deterministic checks over the exact decoder used by IRQ12. No live
/// cursor, button state, or foreground queue is modified.
pub fn self_test() -> Result<(), &'static str> {
    let mut decoder = PacketDecoder::new();
    if decoder.feed(0x00).is_some() || decoder.index != 0 {
        return Err("unsynchronized byte was not discarded");
    }
    if decoder.feed(PACKET_SYNC_BIT).is_some()
        || decoder.feed(5).is_some()
        || decoder.feed(0) != Some([PACKET_SYNC_BIT, 5, 0])
    {
        return Err("three-byte packet assembly/resynchronization failed");
    }

    let initial = MouseState {
        x: 10,
        y: 10,
        buttons: 0,
    };
    let signed = decode_packet([PACKET_SYNC_BIT | FLAG_X_SIGN, 0xFF, 1], initial, 20, 20)
        .ok_or("signed packet was discarded")?;
    if signed.x != 9 || signed.y != 9 || !signed.moved {
        return Err("signed X/Y decode or Y inversion failed");
    }

    let clamped = decode_packet([PACKET_SYNC_BIT, 0x7F, 0x7F], initial, 20, 20)
        .ok_or("clamp packet was discarded")?;
    if clamped.x != 20 || clamped.y != 0 {
        return Err("screen-edge clamping failed");
    }
    if decode_packet([PACKET_SYNC_BIT | FLAG_X_OVERFLOW, 1, 1], initial, 20, 20).is_some() {
        return Err("overflow packet was not discarded");
    }

    let pressed = decode_packet([PACKET_SYNC_BIT | FLAG_LEFT, 0, 0], initial, 20, 20)
        .ok_or("button press packet was discarded")?;
    if pressed.changed_buttons != FLAG_LEFT || pressed.buttons != FLAG_LEFT {
        return Err("button press edge was not detected");
    }
    let held = decode_packet(
        [PACKET_SYNC_BIT | FLAG_LEFT, 0, 0],
        MouseState {
            buttons: pressed.buttons,
            ..initial
        },
        20,
        20,
    )
    .ok_or("held-button packet was discarded")?;
    if held.changed_buttons != 0 {
        return Err("held button generated a duplicate edge");
    }
    let released = decode_packet(
        [PACKET_SYNC_BIT, 0, 0],
        MouseState {
            buttons: held.buttons,
            ..initial
        },
        20,
        20,
    )
    .ok_or("button release packet was discarded")?;
    if released.changed_buttons != FLAG_LEFT || released.buttons != 0 {
        return Err("button release edge was not detected");
    }
    Ok(())
}
