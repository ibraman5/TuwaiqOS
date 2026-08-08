//! Native graphical command terminal for Phase 6 filesystem operations.

#![no_std]
#![no_main]

#[path = "desktop/font.rs"]
mod font;
#[path = "desktop/gfx.rs"]
mod gfx;
#[path = "desktop/sys.rs"]
mod sys;

use core::arch::global_asm;

use gfx::Canvas;

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

const INPUT_CAPACITY: usize = 96;
const LINE_CAPACITY: usize = 96;
const LINE_COUNT: usize = 16;

struct Terminal {
    input: [u8; INPUT_CAPACITY],
    input_len: usize,
    lines: [[u8; LINE_CAPACITY]; LINE_COUNT],
    lengths: [usize; LINE_COUNT],
    count: usize,
}

impl Terminal {
    const fn new() -> Self {
        Self {
            input: [0; INPUT_CAPACITY],
            input_len: 0,
            lines: [[0; LINE_CAPACITY]; LINE_COUNT],
            lengths: [0; LINE_COUNT],
            count: 0,
        }
    }

    fn push_line(&mut self, bytes: &[u8]) {
        if self.count == LINE_COUNT {
            self.lines.copy_within(1.., 0);
            self.lengths.copy_within(1.., 0);
            self.count -= 1;
        }
        let length = bytes.len().min(LINE_CAPACITY);
        self.lines[self.count].fill(0);
        self.lines[self.count][..length].copy_from_slice(&bytes[..length]);
        self.lengths[self.count] = length;
        self.count += 1;
    }

    fn submit(&mut self) -> bool {
        let mut command = [0u8; INPUT_CAPACITY];
        command[..self.input_len].copy_from_slice(&self.input[..self.input_len]);
        let length = self.input_len;
        self.input.fill(0);
        self.input_len = 0;
        let command = trim_ascii(&command[..length]);
        self.push_line_prefixed(b"> ", command);
        hello_user::write(b"terminal: command ");
        hello_user::write(command);
        hello_user::write(b"\n");
        if command == b"exit" {
            return true;
        }
        if command == b"help" || command.is_empty() {
            self.push_line(b"ls [path] | cat <path> | write <name> <text> | stat <path> | exit");
        } else if command == b"ls" || command.starts_with(b"ls ") {
            let path = if command.len() == 2 {
                b"/".as_slice()
            } else {
                trim_ascii(&command[3..])
            };
            let mut output = [0u8; 512];
            match sys::read_dir(path, &mut output) {
                Some(count) => {
                    hello_user::write(b"terminal: listed ");
                    hello_user::write(path);
                    hello_user::write(b"\n");
                    self.push_multiline(&output[..count]);
                }
                None => self.push_line(b"error: directory unavailable"),
            }
        } else if command.starts_with(b"cat ") {
            let path = trim_ascii(&command[4..]);
            let mut output = [0u8; 512];
            match sys::read_file(path, &mut output) {
                Some(count) => {
                    hello_user::write(b"terminal: read ");
                    hello_user::write(path);
                    hello_user::write(b"\n");
                    self.push_multiline(&output[..count]);
                }
                None => self.push_line(b"error: file unavailable or too large"),
            }
        } else if command.starts_with(b"stat ") {
            let path = trim_ascii(&command[5..]);
            match sys::stat(path) {
                Some((kind, size)) => {
                    let mut line = [0u8; LINE_CAPACITY];
                    let mut used = 0;
                    append(
                        &mut line,
                        &mut used,
                        if kind == 1 {
                            b"file size="
                        } else {
                            b"dir children="
                        },
                    );
                    append_decimal(&mut line, &mut used, size);
                    self.push_line(&line[..used]);
                }
                None => self.push_line(b"error: entry unavailable"),
            }
        } else if command.starts_with(b"write ") {
            self.write_command(&command[6..]);
        } else {
            self.push_line(b"error: unknown command; type help");
        }
        false
    }

    fn write_command(&mut self, args: &[u8]) {
        let Some(separator) = args.iter().position(|byte| *byte == b' ') else {
            self.push_line(b"usage: write <name> <text>");
            return;
        };
        let name = &args[..separator];
        let text = trim_ascii(&args[separator + 1..]);
        if name.is_empty()
            || text.is_empty()
            || name.len() > 64
            || name
                .iter()
                .any(|byte| !byte.is_ascii_alphanumeric() && !matches!(*byte, b'-' | b'_' | b'.'))
        {
            self.push_line(b"error: invalid file name or empty text");
            return;
        }
        let mut path = [0u8; 120];
        let prefix = b"/data/terminal/";
        path[..prefix.len()].copy_from_slice(prefix);
        path[prefix.len()..prefix.len() + name.len()].copy_from_slice(name);
        let path = &path[..prefix.len() + name.len()];
        if sys::put_file(path, text) {
            self.push_line(b"saved");
            hello_user::write(b"terminal: saved ");
            hello_user::write(path);
            hello_user::write(b"\n");
        } else {
            self.push_line(b"error: write rejected");
        }
    }

    fn push_line_prefixed(&mut self, prefix: &[u8], value: &[u8]) {
        let mut line = [0u8; LINE_CAPACITY];
        let mut used = 0;
        append(&mut line, &mut used, prefix);
        append(&mut line, &mut used, value);
        self.push_line(&line[..used]);
    }

    fn push_multiline(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            self.push_line(b"(empty)");
            return;
        }
        for line in bytes.split(|byte| *byte == b'\n') {
            if !line.is_empty() {
                self.push_line(line);
            }
        }
    }
}

extern "C" fn rust_main() -> ! {
    hello_user::write(b"terminal: started from /apps/terminal\n");
    let Some(info) = sys::display_info() else {
        sys::exit(1);
    };
    let buffer_len = info.buffer_len();
    let mapped_len = match buffer_len.checked_add(4095).map(|value| value & !4095) {
        Some(value) => value,
        None => sys::exit(1),
    };
    let Some(address) = sys::mmap(mapped_len, true) else {
        sys::exit(1);
    };
    // Safety: this is the process-owned mapping returned above, bounded to
    // the exact visible framebuffer length.
    let buffer =
        unsafe { core::slice::from_raw_parts_mut(address as *mut u8, buffer_len as usize) };
    let mut canvas = Canvas { buf: buffer, info };
    let mut terminal = Terminal::new();
    terminal.push_line(b"Tuwaiq Terminal - type help");
    render(&mut canvas, &terminal);
    if !unsafe { sys::display_present(address, buffer_len) } {
        let _ = sys::munmap(address, mapped_len);
        sys::exit(2);
    }

    loop {
        let mut event = [0u8; 8];
        let mut dirty = false;
        while sys::input_poll(&mut event) {
            if event[0] != 1 {
                continue;
            }
            match event[1] {
                sys::KEY_ESCAPE => {
                    let _ = sys::munmap(address, mapped_len);
                    hello_user::write(b"terminal: normal exit\n");
                    sys::exit(0);
                }
                sys::KEY_ENTER => {
                    dirty = true;
                    if terminal.submit() {
                        let _ = sys::munmap(address, mapped_len);
                        hello_user::write(b"terminal: normal exit\n");
                        sys::exit(0);
                    }
                }
                sys::KEY_BACKSPACE => {
                    if terminal.input_len != 0 {
                        terminal.input_len -= 1;
                        terminal.input[terminal.input_len] = 0;
                        dirty = true;
                    }
                }
                code @ 0x20..=0x7E if terminal.input_len < INPUT_CAPACITY => {
                    terminal.input[terminal.input_len] = code;
                    terminal.input_len += 1;
                    dirty = true;
                }
                _ => {}
            }
        }
        if dirty {
            render(&mut canvas, &terminal);
            if !unsafe { sys::display_present(address, buffer_len) } {
                let _ = sys::munmap(address, mapped_len);
                sys::exit(2);
            }
        }
        sys::yield_now();
    }
}

fn render(canvas: &mut Canvas, terminal: &Terminal) {
    let width = canvas.info.width as i32;
    let height = canvas.info.height as i32;
    canvas.fill_rect(0, 0, width, height, 0x08, 0x0B, 0x10);
    canvas.fill_rect(0, 0, width, 36, 0x12, 0x16, 0x1D);
    canvas.fill_rect(0, 34, width, 2, 0x00, 0xA8, 0x6B);
    canvas.draw_text(b"Tuwaiq Terminal", 16, 12, 0xE6, 0xEA, 0xF0);
    let visible = terminal.count.min(((height - 92) / 18).max(1) as usize);
    let start = terminal.count.saturating_sub(visible);
    let mut y = 54;
    for index in start..terminal.count {
        canvas.draw_text(
            &terminal.lines[index][..terminal.lengths[index]],
            20,
            y,
            0xB8,
            0xE8,
            0xD0,
        );
        y += 18;
    }
    canvas.fill_rect(12, height - 32, width - 24, 24, 0x12, 0x16, 0x1D);
    canvas.draw_text(b">", 20, height - 24, 0x00, 0xD0, 0x83);
    canvas.draw_text(
        &terminal.input[..terminal.input_len],
        38,
        height - 24,
        0xE6,
        0xEA,
        0xF0,
    );
}

fn trim_ascii(mut bytes: &[u8]) -> &[u8] {
    while bytes.first() == Some(&b' ') {
        bytes = &bytes[1..];
    }
    while bytes.last() == Some(&b' ') {
        bytes = &bytes[..bytes.len() - 1];
    }
    bytes
}

fn append(out: &mut [u8], used: &mut usize, bytes: &[u8]) {
    let count = bytes.len().min(out.len().saturating_sub(*used));
    out[*used..*used + count].copy_from_slice(&bytes[..count]);
    *used += count;
}

fn append_decimal(out: &mut [u8], used: &mut usize, value: u64) {
    let mut digits = [0u8; 20];
    append(out, used, hello_user::u64_to_decimal(value, &mut digits));
}
