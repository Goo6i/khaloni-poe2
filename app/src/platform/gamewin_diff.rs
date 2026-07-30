//! Platform-neutral diffing for a *polled* game-window feed.
//!
//! The Linux backend is push-based (the KWin script reports geometry,
//! focus, and cursor changes over DBus), so change detection lives in the
//! compositor script. A polling backend (Windows: FindWindow/GetClientRect/
//! GetForegroundWindow/GetCursorPos every 100ms) instead samples absolute
//! state each tick, and this module turns those samples into the same
//! edge-triggered `GameWindowEvent` stream the main loop already speaks.
//! The semantics deliberately mirror the KWin script embedded in
//! platform/linux/gamewin.rs:
//!
//! - Geometry on first appearance and on every rect change.
//! - Active(bool) on every focus flip (and once for the initial state, like
//!   the script's `lastActiveKey = " "` sentinel forcing a first report).
//! - GameGone exactly once when the window disappears after being seen.
//! - Cursor only when the pointer moved more than 4px on either axis (the
//!   script's `> 4` guard); the 100ms half of the throttle contract is the
//!   caller's poll cadence, not enforced here.
//!
//! Kept free of any OS calls so the state machine is unit-tested on Linux
//! even though only the Windows backend drives it.

use crate::config::Rect;
use crate::platform::GameWindowEvent;

/// One poll tick's absolute observation of the game window.
pub struct WindowSample {
    /// Client rect in screen coordinates; `None` when the window is gone
    /// (or its rect could not be read, which we treat the same way).
    pub rect: Option<Rect>,
    /// Whether the game window is the foreground window right now.
    pub focused: bool,
    /// Whether the game is actually on screen (not minimized, not covered
    /// by other windows). Focus is deliberately independent of this.
    pub visible: bool,
    /// Global cursor position.
    pub cursor: (i32, i32),
}

/// Remembers the last reported state so `diff` only emits edges.
pub struct DiffState {
    last_rect: Option<Rect>,
    /// `None` until the first sample so the initial focus state is always
    /// reported, whatever it is (the main loop gates hotkeys on it).
    last_focused: Option<bool>,
    /// Same first-sample-always-reports contract as focus.
    last_visible: Option<bool>,
    /// Last cursor position actually *emitted* (not merely seen), so a slow
    /// drift of sub-threshold steps still accumulates into an event —
    /// exactly like the script's lastCx/lastCy.
    last_cursor: (i32, i32),
    /// True after the rect has been Some at least once and GameGone has not
    /// been emitted since; keeps GameGone a one-shot per disappearance.
    present: bool,
}

impl DiffState {
    pub fn new() -> DiffState {
        DiffState {
            last_rect: None,
            last_focused: None,
            last_visible: None,
            // Same far-away sentinel as the KWin script so the first sample
            // always reports the cursor (popup anchoring wants a position
            // before the pointer ever moves).
            last_cursor: (-100_000, -100_000),
            present: false,
        }
    }

    /// Feeds one sample and returns the events it implies (possibly empty).
    pub fn diff(&mut self, sample: &WindowSample) -> Vec<GameWindowEvent> {
        let mut out = Vec::new();

        match sample.rect {
            Some(r) => {
                if self.last_rect != Some(r) {
                    out.push(GameWindowEvent::Geometry(r));
                }
                self.last_rect = Some(r);
                self.present = true;
            }
            None => {
                if self.present {
                    out.push(GameWindowEvent::GameGone);
                    self.present = false;
                }
                // Forget the rect so a reappearance re-emits Geometry even
                // when the window comes back at the exact same place.
                self.last_rect = None;
            }
        }

        if self.last_focused != Some(sample.focused) {
            out.push(GameWindowEvent::Active(sample.focused));
            self.last_focused = Some(sample.focused);
        }

        if self.last_visible != Some(sample.visible) {
            out.push(GameWindowEvent::Visible(sample.visible));
            self.last_visible = Some(sample.visible);
        }

        let (cx, cy) = sample.cursor;
        if (cx - self.last_cursor.0).abs() > 4 || (cy - self.last_cursor.1).abs() > 4 {
            self.last_cursor = (cx, cy);
            out.push(GameWindowEvent::Cursor(cx, cy));
        }

        out
    }
}

impl Default for DiffState {
    fn default() -> DiffState {
        DiffState::new()
    }
}
