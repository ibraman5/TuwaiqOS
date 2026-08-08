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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PressAction {
    None,
    Raised { window_index: usize },
    DragStarted { window_index: usize },
    Closed { window_index: usize },
}

#[derive(Clone, Copy)]
struct DragState {
    window_index: usize,
    offset_x: i32,
    offset_y: i32,
    start_x: i32,
    start_y: i32,
}

#[derive(Clone, Copy)]
pub struct DragResult {
    pub window_index: usize,
    pub from_x: i32,
    pub from_y: i32,
    pub to_x: i32,
    pub to_y: i32,
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
    /// Number of initialized slots in `windows`. Closed slots stay in this
    /// range and are reused by a later `spawn`.
    count: usize,
    /// Back-to-front draw/hit-test order over the first `count` window
    /// indices.
    order: [usize; MAX_WINDOWS],
    /// `(window index, cursor-to-window-origin offset)` while a title-bar
    /// drag is in progress.
    dragging: Option<DragState>,
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

    /// Create a new visible window. A closed slot is recycled before the
    /// fixed array grows, so repeatedly opening and closing panels does not
    /// exhaust the desktop for the rest of its lifetime.
    pub fn spawn(&mut self, x: i32, y: i32, w: i32, h: i32, title: &[u8]) -> Option<usize> {
        let recycled = self.windows[..self.count]
            .iter()
            .position(|window| !window.visible);
        let idx = match recycled {
            Some(idx) => idx,
            None if self.count < MAX_WINDOWS => self.count,
            None => return None,
        };
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
        if recycled.is_some() {
            self.bring_to_front(idx);
        } else {
            self.order[idx] = idx;
            self.count += 1;
        }
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
        if self.dragging.map(|drag| drag.window_index) == Some(idx) {
            self.dragging = None;
        }
    }

    /// Handle a left-button press at `(x, y)`: closes a clicked close-box,
    /// starts a drag from a clicked title bar, or just raises the clicked
    /// window to the front -- whichever applies, in that priority order.
    /// Returns the exact model action so acceptance tests can distinguish
    /// a real close/drag/z-order transition from a merely redrawn frame.
    pub fn handle_press(&mut self, x: i32, y: i32) -> PressAction {
        let Some(idx) = self.hit_test(x, y) else {
            return PressAction::None;
        };
        self.bring_to_front(idx);
        let window = self.windows[idx];
        if window.in_close_box(x, y) {
            self.close(idx);
            PressAction::Closed { window_index: idx }
        } else if window.in_title_bar(x, y) {
            self.dragging = Some(DragState {
                window_index: idx,
                offset_x: x - window.x,
                offset_y: y - window.y,
                start_x: window.x,
                start_y: window.y,
            });
            PressAction::DragStarted { window_index: idx }
        } else {
            PressAction::Raised { window_index: idx }
        }
    }

    pub fn handle_drag(&mut self, x: i32, y: i32) {
        if let Some(drag) = self.dragging {
            self.windows[drag.window_index].x = x - drag.offset_x;
            self.windows[drag.window_index].y = y - drag.offset_y;
        }
    }

    pub fn handle_release(&mut self) -> Option<DragResult> {
        let drag = self.dragging.take()?;
        let window = self.windows[drag.window_index];
        Some(DragResult {
            window_index: drag.window_index,
            from_x: drag.start_x,
            from_y: drag.start_y,
            to_x: window.x,
            to_y: window.y,
        })
    }
}

/// Deterministic, framebuffer-independent acceptance check for the window
/// policy. The kernel shell compiles this exact module as a diagnostics-only
/// module and exposes it through `desktoptest`; the real desktop also uses
/// the same methods below for live mouse interaction.
pub fn self_test() -> Result<(), &'static str> {
    let mut wm = WindowManager::new();
    let back = wm
        .spawn(0, 0, 100, 80, b"back")
        .ok_or("initial spawn failed")?;
    let front = wm
        .spawn(10, 10, 100, 80, b"front")
        .ok_or("overlapping spawn failed")?;

    if wm.hit_test(20, 30) != Some(front) {
        return Err("initial z-order is not front-to-back");
    }

    // A title-bar click in the non-overlapping part of `back` raises it.
    if wm.handle_press(5, 5) != (PressAction::DragStarted { window_index: back }) {
        return Err("title-bar press missed visible window");
    }
    let _ = wm.handle_release();
    if wm.hit_test(20, 30) != Some(back) {
        return Err("clicked window was not raised");
    }

    // Drag while held, then prove release ends the drag.
    wm.handle_press(20, 10);
    wm.handle_drag(50, 45);
    let drag = wm.handle_release().ok_or("drag release was not recorded")?;
    let dragged = *wm.window(back);
    if dragged.x != 30
        || dragged.y != 35
        || drag.from_x != 0
        || drag.from_y != 0
        || drag.to_x != 30
        || drag.to_y != 35
    {
        return Err("title-bar drag produced incorrect bounds");
    }
    wm.handle_drag(70, 70);
    if wm.window(back).x != 30 || wm.window(back).y != 35 {
        return Err("window continued moving after button release");
    }

    // The close-box must hide the selected window, and its slot must be
    // reusable without changing fixed-capacity/z-order invariants.
    let close_x = wm.window(back).x + wm.window(back).w - 1;
    let close_y = wm.window(back).y + 1;
    if wm.handle_press(close_x, close_y) != (PressAction::Closed { window_index: back }) {
        return Err("close-box did not report close action");
    }
    if wm.window(back).visible {
        return Err("close-box did not hide window");
    }
    let recycled = wm
        .spawn(40, 40, 80, 60, b"reused")
        .ok_or("closed slot was not reusable")?;
    if recycled != back || wm.hit_test(45, 45) != Some(recycled) {
        return Err("recycled window was not restored at front");
    }

    Ok(())
}
