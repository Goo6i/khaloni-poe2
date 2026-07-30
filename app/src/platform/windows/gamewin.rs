//! Win32 game-window feed: polls the game window at 100ms and reports the
//! same edge-triggered `GameWindowEvent` stream the KWin script produces on
//! Linux (see platform/linux/gamewin.rs). Screen-reading only by design:
//! nothing here (or anywhere else) takes a process handle to the game —
//! just EnumWindows/GetClientRect/GetForegroundWindow/GetCursorPos, the
//! same calls any screenshot tool makes.
//!
//! Polling instead of WinEvent hooks keeps this a plain thread with no
//! message pump, and 10Hz already matches the cursor cadence the Linux
//! feed contracts (100ms / >4px). All change detection lives in the
//! platform-neutral `DiffState` (platform/gamewin_diff.rs), which is where
//! the semantics are tested.
//!
//! The client rect (not the frame rect) is reported, converted to screen
//! coordinates via ClientToScreen: the overlay and capture layers want the
//! drawable game area, not the titlebar/border. The thread opts into
//! PER_MONITOR_AWARE_V2 so these are physical pixels regardless of DPI
//! scaling — logical == physical, matching what capture sees.

use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::thread::sleep;
use std::time::Duration;

use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::UI::HiDpi::{
    SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetAncestor, GetClientRect, GetCursorPos, GetForegroundWindow, GetWindowTextW,
    IsIconic, IsWindowVisible, WindowFromPoint, GA_ROOT,
};

use crate::config::Rect;
use crate::platform::gamewin_diff::{DiffState, WindowSample};

pub use crate::platform::GameWindowEvent;

/// The game window's HWND as of the latest poll tick, 0 when absent.
/// An HWND is a process-wide integer token, safe to pass between threads;
/// stored as isize so a plain atomic carries it.
static GAME_HWND: AtomicIsize = AtomicIsize::new(0);

/// The most recently seen game window handle, or `None` when the game is
/// not running (or `start` was never called).
///
/// Contract for the overlay and capture backends: the feed's poll thread
/// refreshes this every 100ms, so it is at most one tick stale — always
/// re-read it rather than caching, and treat a `GameGone` event as the
/// signal that a previously obtained handle is now dead. The value is a
/// raw `HWND` bit pattern (cast back with `HWND(v as *mut _)`); it is
/// published here instead of piped through the event channel so consumers
/// that only need the handle (WGC session setup, SetForegroundWindow on
/// Escape) don't have to tap the event stream.
pub fn game_hwnd() -> Option<isize> {
    match GAME_HWND.load(Ordering::Relaxed) {
        0 => None,
        h => Some(h),
    }
}

pub struct GameWindowFeed {
    pub rx: Receiver<GameWindowEvent>,
}

/// Platform-neutral facade, matching the Linux side.
pub fn start() -> anyhow::Result<GameWindowFeed> {
    GameWindowFeed::start()
}

impl GameWindowFeed {
    pub fn start() -> anyhow::Result<GameWindowFeed> {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            // Per-thread DPI awareness is enough: every coordinate this
            // feed reports comes from calls made on this thread. V2 makes
            // GetClientRect/ClientToScreen/GetCursorPos return physical
            // pixels on scaled monitors instead of virtualized ones.
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
            let mut diff = DiffState::new();
            loop {
                let hwnd = find_game_window();
                GAME_HWND.store(hwnd.map_or(0, |h| h.0 as isize), Ordering::Relaxed);

                let rect = hwnd.and_then(client_rect_on_screen);
                // A window whose rect just became unreadable (mid-close) is
                // treated as gone; `rect: None` makes DiffState emit
                // GameGone, so don't report it focused either.
                let focused =
                    rect.is_some() && hwnd.is_some_and(|h| unsafe { GetForegroundWindow() } == h);
                let visible = match (hwnd, rect) {
                    (Some(h), Some(r)) => window_visible(h, r),
                    _ => false,
                };
                let mut pt = POINT::default();
                let _ = unsafe { GetCursorPos(&mut pt) };

                let sample = WindowSample { rect, focused, visible, cursor: (pt.x, pt.y) };
                for ev in diff.diff(&sample) {
                    if tx.send(ev).is_err() {
                        // Main loop dropped the feed: stop polling.
                        GAME_HWND.store(0, Ordering::Relaxed);
                        return;
                    }
                }
                sleep(Duration::from_millis(100));
            }
        });
        Ok(GameWindowFeed { rx })
    }
}

/// What the EnumWindows sweep collected. An exact "Path of Exile 2" title
/// is preferred over a contains-match so windows *about* the game (this
/// overlay's own debug tools, an open wiki page titled "... - Path of
/// Exile 2") never shadow the real client; the contains-fallback still
/// catches title decorations (e.g. a build/region suffix).
#[derive(Default)]
struct FoundWindows {
    exact: Option<isize>,
    partial: Option<isize>,
}

/// On-screen check mirroring the KWin script's visibility heuristic: not
/// minimized, and a majority of probe points across the client rect resolve
/// to the game's own top-level window (WindowFromPoint sees whatever is
/// topmost there, so a covering window flips the probes). Five probes —
/// center + the four quarter points — so a partial overlap (chat window on
/// one edge) keeps the overlay while real coverage hides it.
fn window_visible(hwnd: HWND, r: Rect) -> bool {
    if unsafe { IsIconic(hwnd) }.as_bool() {
        return false;
    }
    let (qx, qy) = ((r.w / 4) as i32, (r.h / 4) as i32);
    let probes = [
        (r.x + 2 * qx, r.y + 2 * qy),
        (r.x + qx, r.y + qy),
        (r.x + 3 * qx, r.y + qy),
        (r.x + qx, r.y + 3 * qy),
        (r.x + 3 * qx, r.y + 3 * qy),
    ];
    let ours = probes
        .iter()
        .filter(|(x, y)| {
            let at = unsafe { WindowFromPoint(POINT { x: *x, y: *y }) };
            !at.is_invalid() && unsafe { GetAncestor(at, GA_ROOT) } == hwnd
        })
        .count();
    ours >= 3
}

fn find_game_window() -> Option<HWND> {
    let mut found = FoundWindows::default();
    // EnumWindows returns Err when the callback stops the sweep early
    // (exact match found) — not a failure, so the Result is ignored.
    let _ = unsafe {
        EnumWindows(Some(enum_cb), LPARAM(&mut found as *mut FoundWindows as isize))
    };
    found
        .exact
        .or(found.partial)
        .map(|h| HWND(h as *mut core::ffi::c_void))
}

unsafe extern "system" fn enum_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let found = unsafe { &mut *(lparam.0 as *mut FoundWindows) };
    // Invisible windows are skipped: the game keeps hidden helper windows
    // around, and a minimized-to-tray launcher must not be tracked.
    if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
        return true.into();
    }
    let mut buf = [0u16; 128];
    let len = unsafe { GetWindowTextW(hwnd, &mut buf) };
    if len <= 0 {
        return true.into();
    }
    let title = String::from_utf16_lossy(&buf[..len as usize]);
    if title == "Path of Exile 2" {
        found.exact = Some(hwnd.0 as isize);
        return false.into(); // exact match wins; stop the sweep
    }
    if found.partial.is_none() && title.contains("Path of Exile 2") {
        found.partial = Some(hwnd.0 as isize);
    }
    true.into()
}

/// The window's client area in screen coordinates, or `None` when it can't
/// be read (window died mid-call) or is degenerate (minimized windows
/// report a 0x0 client rect, which must not reach the overlay as a
/// geometry).
fn client_rect_on_screen(hwnd: HWND) -> Option<Rect> {
    let mut rc = RECT::default();
    unsafe { GetClientRect(hwnd, &mut rc) }.ok()?;
    // GetClientRect's left/top are always 0; ClientToScreen shifts the
    // origin into screen space (physical pixels under PER_MONITOR_AWARE_V2).
    let mut origin = POINT { x: rc.left, y: rc.top };
    if !unsafe { ClientToScreen(hwnd, &mut origin) }.as_bool() {
        return None;
    }
    let w = (rc.right - rc.left).max(0) as u32;
    let h = (rc.bottom - rc.top).max(0) as u32;
    if w == 0 || h == 0 {
        return None;
    }
    Some(Rect { x: origin.x, y: origin.y, w, h })
}
