//! Software rendering primitives for the desktop's own backbuffer -- a
//! process-owned `SYS_MMAP`'d region, never the real framebuffer (see
//! `main.rs`: the finished frame only reaches the screen through
//! `SYS_DISPLAY_PRESENT`, which the kernel validates independently).

#![allow(dead_code)] // Shared applications intentionally use different subsets.

use crate::font;
use crate::sys::{DisplayInfo, PIXEL_FORMAT_BGR, PIXEL_FORMAT_U8};

pub struct Canvas<'a> {
    pub buf: &'a mut [u8],
    pub info: DisplayInfo,
}

impl<'a> Canvas<'a> {
    pub fn put_pixel(&mut self, x: i32, y: i32, r: u8, g: u8, b: u8) {
        if x < 0 || y < 0 || x as u32 >= self.info.width || y as u32 >= self.info.height {
            return;
        }
        let pixel_index = y as u32 * self.info.stride + x as u32;
        let byte_index = (pixel_index as u64 * self.info.bytes_per_pixel as u64) as usize;
        let bpp = self.info.bytes_per_pixel as usize;
        if byte_index + bpp > self.buf.len() {
            return;
        }

        // Mirrors `framebuffer_console.rs::write_pixel`'s format handling
        // on the kernel side -- BGR and U8 get their own byte order/
        // grayscale reduction, everything else (RGB, and any format this
        // bootloader reports as unknown) falls back to plain R,G,B order.
        if self.info.pixel_format == PIXEL_FORMAT_BGR {
            self.buf[byte_index] = b;
            self.buf[byte_index + 1] = g;
            self.buf[byte_index + 2] = r;
        } else if self.info.pixel_format == PIXEL_FORMAT_U8 {
            self.buf[byte_index] = ((r as u16 + g as u16 + b as u16) / 3) as u8;
        } else {
            self.buf[byte_index] = r;
            if bpp >= 2 {
                self.buf[byte_index + 1] = g;
            }
            if bpp >= 3 {
                self.buf[byte_index + 2] = b;
            }
        }
    }

    pub fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, r: u8, g: u8, b: u8) {
        for dy in 0..h {
            for dx in 0..w {
                self.put_pixel(x + dx, y + dy, r, g, b);
            }
        }
    }

    /// Unfilled rectangle outline, one pixel thick.
    pub fn stroke_rect(&mut self, x: i32, y: i32, w: i32, h: i32, r: u8, g: u8, b: u8) {
        for dx in 0..w {
            self.put_pixel(x + dx, y, r, g, b);
            self.put_pixel(x + dx, y + h - 1, r, g, b);
        }
        for dy in 0..h {
            self.put_pixel(x, y + dy, r, g, b);
            self.put_pixel(x + w - 1, y + dy, r, g, b);
        }
    }

    fn draw_char(&mut self, ch: u8, x: i32, y: i32, r: u8, g: u8, b: u8) {
        let glyph = font::glyph_rows(ch);
        for (row, bits) in glyph.iter().enumerate() {
            for col in 0..8u32 {
                if (bits >> col) & 1 == 1 {
                    self.put_pixel(x + col as i32, y + row as i32, r, g, b);
                }
            }
        }
    }

    pub fn draw_text(&mut self, text: &[u8], x: i32, y: i32, r: u8, g: u8, b: u8) {
        let mut cx = x;
        for &ch in text {
            self.draw_char(ch, cx, y, r, g, b);
            cx += 9;
        }
    }

    /// A simple filled diagonal-wedge arrow, white with a thin dark edge --
    /// enough to be an unambiguous, visibly-moving cursor without needing a
    /// pre-rendered bitmap asset.
    pub fn draw_cursor(&mut self, x: i32, y: i32) {
        for dy in 0..14 {
            let width = dy.min(8);
            for dx in 0..=width {
                self.put_pixel(x + dx, y + dy, 0xF0, 0xF0, 0xF0);
            }
            self.put_pixel(x + width + 1, y + dy, 0x10, 0x10, 0x10);
        }
    }
}
