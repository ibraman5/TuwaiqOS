//! Native graphical file browser for Phase 6.

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

const PATHS: [&[u8]; 4] = [b"/apps", b"/data", b"/boot", b"/boot/DOCS"];
const LIST_CAPACITY: usize = 2048;

struct Browser {
    path_index: usize,
    entries: [u8; LIST_CAPACITY],
    entries_len: usize,
    status: &'static [u8],
}

extern "C" fn rust_main() -> ! {
    hello_user::write(b"file-manager: started from /apps/file-manager\n");
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
    // Safety: `address` is this process's writable mapping of at least
    // `mapped_len`; the visible canvas is restricted to `buffer_len`.
    let buffer =
        unsafe { core::slice::from_raw_parts_mut(address as *mut u8, buffer_len as usize) };
    let mut canvas = Canvas { buf: buffer, info };
    let mut browser = Browser {
        path_index: 0,
        entries: [0; LIST_CAPACITY],
        entries_len: 0,
        status: b"A Apps  D Data  B Boot  R Boot Docs  Esc Back",
    };
    refresh(&mut browser);
    render(&mut canvas, &browser);
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
                    hello_user::write(b"file-manager: normal exit\n");
                    let _ = sys::munmap(address, mapped_len);
                    sys::exit(0);
                }
                b'a' | b'A' => dirty |= select(&mut browser, 0),
                b'd' | b'D' => dirty |= select(&mut browser, 1),
                b'b' | b'B' => dirty |= select(&mut browser, 2),
                b'r' | b'R' => dirty |= select(&mut browser, 3),
                _ => {}
            }
        }
        if dirty {
            render(&mut canvas, &browser);
            if !unsafe { sys::display_present(address, buffer_len) } {
                let _ = sys::munmap(address, mapped_len);
                sys::exit(2);
            }
        }
        sys::yield_now();
    }
}

fn select(browser: &mut Browser, index: usize) -> bool {
    if browser.path_index == index {
        return false;
    }
    browser.path_index = index;
    refresh(browser);
    true
}

fn refresh(browser: &mut Browser) {
    browser.entries.fill(0);
    match sys::read_dir(PATHS[browser.path_index], &mut browser.entries) {
        Some(length) => {
            browser.entries_len = length;
            browser.status = b"Directory loaded from VFS";
            hello_user::write(b"file-manager: listed ");
            hello_user::write(PATHS[browser.path_index]);
            hello_user::write(b"\n");
        }
        None => {
            browser.entries_len = 0;
            browser.status = b"Directory unavailable";
            hello_user::write(b"file-manager: list failed\n");
        }
    }
}

fn render(canvas: &mut Canvas, browser: &Browser) {
    let width = canvas.info.width as i32;
    let height = canvas.info.height as i32;
    canvas.fill_rect(0, 0, width, height, 0x0B, 0x0E, 0x13);
    canvas.fill_rect(0, 0, width, 36, 0x12, 0x16, 0x1D);
    canvas.fill_rect(0, 34, width, 2, 0x00, 0xA8, 0x6B);
    canvas.draw_text(b"Tuwaiq File Manager", 16, 12, 0xE6, 0xEA, 0xF0);
    canvas.draw_text(PATHS[browser.path_index], 24, 58, 0x00, 0xD0, 0x83);
    canvas.draw_text(browser.status, 24, height - 34, 0x8A, 0x93, 0xA0);
    canvas.draw_text(
        b"A Apps | D Data | B Boot | R Boot Docs | Esc Desktop",
        24,
        height - 18,
        0x8A,
        0x93,
        0xA0,
    );

    let mut y = 86;
    for line in browser.entries[..browser.entries_len].split(|byte| *byte == b'\n') {
        if line.is_empty() || y > height - 56 {
            continue;
        }
        canvas.draw_text(line, 36, y, 0xE6, 0xEA, 0xF0);
        y += 18;
    }
    if browser.entries_len == 0 {
        canvas.draw_text(b"(empty)", 36, y, 0x8A, 0x93, 0xA0);
    }
}
