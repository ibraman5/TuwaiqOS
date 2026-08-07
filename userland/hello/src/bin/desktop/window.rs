//! Minimal userspace window model (Phase 5, Milestone 7). Intentionally
//! small: fixed-capacity arrays (this process has no heap -- `hello_user`
//! is `#![no_std]` without `alloc`), rectangular windows, z-order, a single
//! focused/dragged window at a time, and just enough hit-testing to move
//! and close a window with the mouse. This is architecture, not a general
//! compositor -- window *policy* belongs here in userspace, deliberately
//! kept out of the kernel (see `display.rs`/`ARCHITECTURE.md`).

pub const MAX_WINDOWS: usize = 4;
pub const TITLE_BAR_HEIGHT: i32 = 22;
const CLOSE_BOX_WIDTH: i32 = 20;
const TITLE_MAX: usize = 20;

#[derive(Clone, Copy)]
pub struct Window {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub title: [u8; TITLE_MAX],
    pub title_len: usize,
    pub visible: bool,
}

impl Window {
    const fn empty() -> Self {
        Self {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
            title: [0; TITLE_MAX],
            title_len: 0,
            visible: false,
        }
    }

    pub fn title_bytes(&self) -> &[u8] {
        &self.title[..self.title_len]
    }

    fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }

    fn in_close_box(&self, x: i32, y: i32) -> bool {
        y >= self.y
            && y < self.y + TITLE_BAR_HEIGHT
            && x >= self.x + self.w - CLOSE_BOX_WIDTH
            && x < self.x + self.w
    }

    fn in_title_bar(&self, x: i32, y: i32) -> bool {
        y >= self.y && y < self.y + TITLE_BAR_HEIGHT && x >= self.x && x < self.x + self.w
    }
}

pub struct WindowManager {
    pub windows: [Window; MAX_WINDOWS],
    /// How many slots in `windows` are live (spawned, possibly since
    /// closed but never reused -- see `spawn`'s docs on why this design
    /// deliberately doesn't recycle closed slots).
    count: usize,
    /// Back-to-front draw/hit-test order over the first `count` window
    /// indices.
    order: [usize; MAX_WINDOWS],
    /// `(window index, cursor-to-window-origin offset)` while a title-bar
    /// drag is in progress.
    dragging: Option<(usize, i32, i32)>,
}

impl WindowManager {
    pub const fn new() -> Self {
        Self {
            windows: [Window::empty(); MAX_WINDOWS],
            count: 0,
            order: [0; MAX_WINDOWS],
            dragging: None,
        }
    }

    /// Create a new visible window. Returns `None` once `MAX_WINDOWS` have
    /// ever been spawned -- deliberately not reused after a close: the
    /// fixed-capacity array trades "a long-running desktop can eventually
    /// run out of window slots" for "no slot-reuse bookkeeping to get
    /// wrong," which is the right trade for a first, intentionally small
    /// window model.
    pub fn spawn(&mut self, x: i32, y: i32, w: i32, h: i32, title: &[u8]) -> Option<usize> {
        if self.count >= MAX_WINDOWS {
            return None;
        }
        let idx = self.count;
        let mut window = Window::empty();
        window.x = x;
        window.y = y;
        window.w = w;
        window.h = h;
        window.visible = true;
        let len = title.len().min(TITLE_MAX);
        window.title[..len].copy_from_slice(&title[..len]);
        window.title_len = len;
        self.windows[idx] = window;
        self.order[idx] = idx;
        self.count += 1;
        Some(idx)
    }

    /// Draw order, back to front -- draw in this order, hit-test in reverse.
    pub fn draw_order(&self) -> &[usize] {
        &self.order[..self.count]
    }

    /// Topmost visible window under `(x, y)`, or `None`.
    pub fn hit_test(&self, x: i32, y: i32) -> Option<usize> {
        for &idx in self.order[..self.count].iter().rev() {
            let window = &self.windows[idx];
            if window.visible && window.contains(x, y) {
                return Some(idx);
            }
        }
        None
    }

    pub fn window(&self, idx: usize) -> &Window {
        &self.windows[idx]
    }

    fn bring_to_front(&mut self, idx: usize) {
        if let Some(pos) = self.order[..self.count].iter().position(|&i| i == idx) {
            for k in pos..self.count - 1 {
                self.order[k] = self.order[k + 1];
            }
            self.order[self.count - 1] = idx;
        }
    }

    pub fn close(&mut self, idx: usize) {
        self.windows[idx].visible = false;
        if self.dragging.map(|(d, _, _)| d) == Some(idx) {
            self.dragging = None;
        }
    }

    /// Handle a left-button press at `(x, y)`: closes a clicked close-box,
    /// starts a drag from a clicked title bar, or just raises the clicked
    /// window to the front -- whichever applies, in that priority order.
    /// Returns `true` if the click actually hit a window (as opposed to
    /// empty desktop), so `main.rs` can decide whether to also check the
    /// launcher button.
    pub fn handle_press(&mut self, x: i32, y: i32) -> bool {
        let Some(idx) = self.hit_test(x, y) else {
            return false;
        };
        self.bring_to_front(idx);
        let window = self.windows[idx];
        if window.in_close_box(x, y) {
            self.close(idx);
        } else if window.in_title_bar(x, y) {
            self.dragging = Some((idx, x - window.x, y - window.y));
        }
        true
    }

    pub fn handle_drag(&mut self, x: i32, y: i32) {
        if let Some((idx, off_x, off_y)) = self.dragging {
            self.windows[idx].x = x - off_x;
            self.windows[idx].y = y - off_y;
        }
    }

    pub fn handle_release(&mut self) {
        self.dragging = None;
    }
}
