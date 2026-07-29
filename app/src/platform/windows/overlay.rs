//! Windows overlay backend: a transparent, always-on-top, click-through
//! winit window presented with softbuffer, behind the same 12-method public
//! API as platform/linux/overlay.rs (wlr-layer-shell).
//!
//! Windows-specific contracts and divergences from the Linux twin:
//!
//! - **Event loop on the caller's thread.** Wayland lets the overlay own a
//!   connection anywhere; winit on Windows requires the event loop thread
//!   to be the window's thread, so `new()` builds the loop with
//!   `with_any_thread(true)` on the calling thread and `pump()` drains it
//!   with `pump_app_events(Some(Duration::ZERO))` — same call-often,
//!   never-block contract as the Linux `roundtrip`.
//! - **Single-rect input region, approximated.** Wayland has a real
//!   per-surface input region: outside it, clicks reach the game. winit's
//!   `set_cursor_hittest` is all-or-nothing, so while `set_interactive`
//!   has a rect stored, hit-testing stays ON for the whole window and
//!   clicks landing outside the rect are dropped in the event handler
//!   rather than passed through — i.e. the window eats clicks in the gap
//!   between the interactive rect and the window bounds instead of letting
//!   the game see them. Toggling hittest off on pointer-exit was rejected:
//!   once the cursor stops hitting the window there is no re-entry event
//!   to turn it back on. `set_interactive(None)` restores true
//!   click-through everywhere.
//! - **Keyboard focus is window focus.** The Linux side flips layer-shell
//!   keyboard interactivity; here `set_keyboard(true)` focuses the overlay
//!   window and `set_keyboard(false)` hands focus back to the game via
//!   `SetForegroundWindow` on the tracked game HWND (from
//!   `crate::platform::gamewin::game_hwnd()`), when one is available.
//!   Key events are additionally gated on the keyboard flag so stray focus
//!   never leaks keystrokes into `take_keys`.
//! - **Presentation.** tiny-skia's premultiplied RGBA becomes softbuffer's
//!   `0x00RRGGBB` u32s (alpha byte unused by softbuffer's GDI path; the
//!   transparent/layered window setup determines what zero pixels do —
//!   verified by the SP3 Windows live-test checklist). `hide()` presents an
//!   all-zero buffer, matching the Linux behavior.

use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use tiny_skia::Pixmap;
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key as WinitKey, NamedKey};
use winit::platform::pump_events::EventLoopExtPumpEvents;
use winit::platform::windows::{EventLoopBuilderExtWindows, WindowAttributesExtWindows};
use winit::window::{Window, WindowId, WindowLevel};

pub use crate::platform::Key;

struct App {
    /// Global position the overlay should cover the monitor of.
    target_center: (i32, i32),
    window: Option<Arc<Window>>,
    /// Position of the covered monitor (global physical px; with the
    /// process PER_MONITOR_AWARE_V2, logical == physical).
    monitor_pos: (i32, i32),
    /// Window-creation failure captured inside `resumed`, surfaced by `new()`.
    init_err: Option<String>,
    /// The interactive rect while one is set (window-local px). Hit-testing
    /// is on for the whole window in that state; this rect decides which
    /// clicks are kept (see the module doc's input-region note).
    region: Option<(i32, i32, u32, u32)>,
    /// Gate for `keys`: only record keystrokes while a panel wants them.
    keyboard_on: bool,
    /// Left-button presses inside the interactive rect (window-local px),
    /// drained by the main loop.
    clicks: Vec<(i32, i32)>,
    /// Latest pointer position (window-local px), updated on every motion.
    pointer_pos: (i32, i32),
    /// Whether the left button is currently held (drag tracking).
    button_down: bool,
    /// Editing keystrokes, drained by the main loop while a box is focused.
    keys: Vec<Key>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        // Pick the monitor containing the tracked window's center, like the
        // Linux side picks the wl_output; fall back to the primary monitor.
        let (cx, cy) = self.target_center;
        let mut target = None;
        for m in event_loop.available_monitors() {
            let pos = m.position();
            let size = m.size();
            if cx >= pos.x
                && cx < pos.x + size.width as i32
                && cy >= pos.y
                && cy < pos.y + size.height as i32
            {
                target = Some(m);
                break;
            }
        }
        let target = target
            .or_else(|| event_loop.primary_monitor())
            .or_else(|| event_loop.available_monitors().next());
        let Some(monitor) = target else {
            self.init_err = Some("no monitor found".into());
            return;
        };
        let pos = monitor.position();
        let size = monitor.size();
        self.monitor_pos = (pos.x, pos.y);

        let attrs = Window::default_attributes()
            .with_title("poe2-lens")
            .with_transparent(true)
            .with_decorations(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_position(PhysicalPosition::new(pos.x, pos.y))
            .with_inner_size(PhysicalSize::new(size.width, size.height))
            // Never steal focus from the game at creation.
            .with_active(false)
            .with_skip_taskbar(true);
        match event_loop.create_window(attrs) {
            Ok(window) => {
                // Click-through by default, like the Linux empty input region.
                if let Err(e) = window.set_cursor_hittest(false) {
                    self.init_err = Some(format!("cursor hittest: {e}"));
                    return;
                }
                self.window = Some(Arc::new(window));
            }
            Err(e) => self.init_err = Some(format!("create window: {e}")),
        }
    }

    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer_pos = (position.x as i32, position.y as i32);
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => match state {
                ElementState::Pressed => {
                    // Only clicks inside the interactive rect count; ones in
                    // the hittest-on/outside-rect gap are dropped (module doc).
                    let inside = self.region.is_some_and(|(x, y, w, h)| {
                        let (px, py) = self.pointer_pos;
                        px >= x && px < x + w as i32 && py >= y && py < y + h as i32
                    });
                    if inside {
                        self.clicks.push(self.pointer_pos);
                        self.button_down = true;
                    }
                }
                ElementState::Released => self.button_down = false,
            },
            WindowEvent::KeyboardInput { event, .. } => {
                if !self.keyboard_on || event.state != ElementState::Pressed {
                    return;
                }
                // Same mapping as the Linux press_key handler: named keys
                // first, then the text produced by the key (utf8 fallback).
                match event.logical_key {
                    WinitKey::Named(NamedKey::Backspace) => self.keys.push(Key::Backspace),
                    WinitKey::Named(NamedKey::Enter) => self.keys.push(Key::Enter),
                    WinitKey::Named(NamedKey::Escape) => self.keys.push(Key::Escape),
                    WinitKey::Named(NamedKey::ArrowUp) => self.keys.push(Key::Up),
                    WinitKey::Named(NamedKey::ArrowDown) => self.keys.push(Key::Down),
                    ref other => {
                        if let Some(c) = other.to_text().and_then(|s| s.chars().next()) {
                            if c.is_ascii_digit() {
                                self.keys.push(Key::Digit(c));
                            } else if c == '.' {
                                self.keys.push(Key::Dot);
                            } else if c.is_ascii_graphic() || c == ' ' {
                                self.keys.push(Key::Char(c));
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

pub struct Overlay {
    app: App,
    surface: softbuffer::Surface<Arc<Window>, Arc<Window>>,
    _context: softbuffer::Context<Arc<Window>>,
    /// Last known game window, for handing focus back in set_keyboard(false).
    game_hwnd: Option<isize>,
    // Dropped last: the window (held via app/surface) must not outlive the loop.
    event_loop: EventLoop<()>,
}

impl Overlay {
    pub fn new(target_center: (i32, i32)) -> anyhow::Result<Overlay> {
        // Winit normally insists on the process main thread; the overlay is
        // driven from the app's render loop thread, which Windows allows as
        // long as the same thread creates and pumps the loop.
        let mut event_loop = {
            let mut builder = EventLoop::builder();
            builder.with_any_thread(true);
            builder
                .build()
                .map_err(|e| anyhow::anyhow!("event loop: {e}"))?
        };
        let mut app = App {
            target_center,
            window: None,
            monitor_pos: (0, 0),
            init_err: None,
            region: None,
            keyboard_on: false,
            clicks: Vec::new(),
            pointer_pos: (0, 0),
            button_down: false,
            keys: Vec::new(),
        };
        // First pump delivers the init events, running `resumed` (window
        // creation) synchronously before we wire up softbuffer.
        let _ = event_loop.pump_app_events(Some(Duration::ZERO), &mut app);
        if let Some(e) = app.init_err.take() {
            anyhow::bail!("overlay init: {e}");
        }
        let window = app
            .window
            .clone()
            .ok_or_else(|| anyhow::anyhow!("overlay window was not created"))?;
        let context = softbuffer::Context::new(window.clone())
            .map_err(|e| anyhow::anyhow!("softbuffer context: {e}"))?;
        let surface = softbuffer::Surface::new(&context, window)
            .map_err(|e| anyhow::anyhow!("softbuffer surface: {e}"))?;
        Ok(Overlay {
            app,
            surface,
            _context: context,
            game_hwnd: crate::platform::gamewin::game_hwnd(),
            event_loop,
        })
    }

    /// Makes `rect` (window-local px) accept pointer input, or restores full
    /// click-through with None. Approximation of the Wayland input region:
    /// while a rect is set, the whole window hit-tests and clicks outside
    /// the rect are dropped rather than reaching the game (module doc).
    pub fn set_interactive(&mut self, rect: Option<(i32, i32, u32, u32)>) -> anyhow::Result<()> {
        let window = self.app.window.as_ref().expect("window created in new()");
        window
            .set_cursor_hittest(rect.is_some())
            .map_err(|e| anyhow::anyhow!("cursor hittest: {e}"))?;
        if rect.is_none() {
            // Dropping the region also drops any stale clicks nobody
            // drained (a click raced the panel closing).
            self.app.clicks.clear();
        }
        self.app.region = rect;
        Ok(())
    }

    /// Drains left-clicks received since the last call (window-local).
    pub fn take_clicks(&mut self) -> Vec<(i32, i32)> {
        std::mem::take(&mut self.app.clicks)
    }

    /// Latest pointer position, window-local px.
    pub fn pointer_pos(&self) -> (i32, i32) {
        self.app.pointer_pos
    }

    /// Whether the left mouse button is currently held (for drag tracking).
    pub fn button_down(&self) -> bool {
        self.app.button_down
    }

    /// Drains editing keystrokes received since the last call.
    pub fn take_keys(&mut self) -> Vec<Key> {
        std::mem::take(&mut self.app.keys)
    }

    /// Grabs (or releases) keyboard focus for the overlay window, so a
    /// focused value box can receive typed digits. Releasing hands focus
    /// back to the game window when the tracker knows it.
    pub fn set_keyboard(&mut self, on: bool) -> anyhow::Result<()> {
        self.app.keyboard_on = on;
        if on {
            self.app
                .window
                .as_ref()
                .expect("window created in new()")
                .focus_window();
        } else {
            self.app.keys.clear();
            self.game_hwnd = crate::platform::gamewin::game_hwnd().or(self.game_hwnd);
            if let Some(hwnd) = self.game_hwnd {
                unsafe {
                    let _ = windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow(
                        windows::Win32::Foundation::HWND(hwnd as *mut core::ffi::c_void),
                    );
                }
            }
        }
        Ok(())
    }

    /// Drains pending window events without blocking; the Windows analogue
    /// of the Linux Wayland roundtrip. Call frequently from the main loop.
    pub fn pump(&mut self) -> anyhow::Result<()> {
        let _ = self
            .event_loop
            .pump_app_events(Some(Duration::ZERO), &mut self.app);
        Ok(())
    }

    pub fn present(&mut self, pixmap: &Pixmap) -> anyhow::Result<()> {
        let (w, h) = (pixmap.width(), pixmap.height());
        let (Some(nw), Some(nh)) = (NonZeroU32::new(w), NonZeroU32::new(h)) else {
            return Ok(());
        };
        self.surface
            .resize(nw, nh)
            .map_err(|e| anyhow::anyhow!("surface resize: {e}"))?;
        let mut buffer = self
            .surface
            .buffer_mut()
            .map_err(|e| anyhow::anyhow!("surface buffer: {e}"))?;
        // tiny-skia stores premultiplied RGBA; softbuffer wants 0x00RRGGBB.
        for (dst, s) in buffer.iter_mut().zip(pixmap.data().chunks_exact(4)) {
            *dst = ((s[0] as u32) << 16) | ((s[1] as u32) << 8) | s[2] as u32;
        }
        buffer
            .present()
            .map_err(|e| anyhow::anyhow!("surface present: {e}"))?;
        Ok(())
    }

    pub fn hide(&mut self) -> anyhow::Result<()> {
        let (w, h) = self.size();
        let (Some(nw), Some(nh)) = (NonZeroU32::new(w), NonZeroU32::new(h)) else {
            return Ok(());
        };
        self.surface
            .resize(nw, nh)
            .map_err(|e| anyhow::anyhow!("surface resize: {e}"))?;
        let mut buffer = self
            .surface
            .buffer_mut()
            .map_err(|e| anyhow::anyhow!("surface buffer: {e}"))?;
        buffer.fill(0);
        buffer
            .present()
            .map_err(|e| anyhow::anyhow!("surface present: {e}"))?;
        Ok(())
    }

    pub fn size(&self) -> (u32, u32) {
        self.app
            .window
            .as_ref()
            .map(|w| {
                let s = w.inner_size();
                (s.width, s.height)
            })
            .unwrap_or((0, 0))
    }

    pub fn output_pos(&self) -> (i32, i32) {
        self.app.monitor_pos
    }
}
