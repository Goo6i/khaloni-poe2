//! What a price-check press does when the game may not be focused.
//!
//! The copy behind a price check is a Ctrl+C typed into whatever window has
//! keyboard focus, and the game only copies the hovered item when it is that
//! window. The hotkey itself is global, so a press with a browser focused on
//! the other monitor used to be dropped. Now, when the pointer is over the
//! game, the press first asks the compositor to focus the game and the copy
//! follows once the window feed confirms focus; focus then stays on the game,
//! since the mouse is already there. A press away from the game is answered
//! with a note, never with a Ctrl+C into the wrong window. Pure state, no
//! I/O: the main loop performs the focus request, the copy, and the notes.

use std::time::{Duration, Instant};

use crate::config::Rect;

/// How long a focus request may take before the press is given up on.
/// KWin activates at once and both window feeds report within a 100ms
/// tick; the rest is slack for a loaded system.
pub const FOCUS_WAIT: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Press {
    /// The game has focus: copy now.
    Copy,
    /// The pointer is over the unfocused game: focus it, and copy when
    /// [`FocusGate::focused`] says the feed saw it land.
    FocusGame,
    /// Nothing to copy from; show this at the cursor instead.
    Note(&'static str),
    /// A focus request is already in flight for an earlier press.
    Ignore,
}

/// Tracks the one press that may be waiting on the game taking focus.
#[derive(Debug, Default)]
pub struct FocusGate {
    /// When the pending focus request was made.
    pending: Option<Instant>,
}

impl FocusGate {
    pub fn press(
        &mut self,
        now: Instant,
        game_focused: bool,
        game_present: bool,
        cursor: (i32, i32),
        game: Rect,
    ) -> Press {
        if game_focused {
            self.pending = None;
            return Press::Copy;
        }
        if !game_present {
            return Press::Note("game window not found");
        }
        if !contains(game, cursor) {
            return Press::Note("hover an item in the game, then press the key");
        }
        if self.pending.is_some_and(|since| now < since + FOCUS_WAIT) {
            return Press::Ignore;
        }
        self.pending = Some(now);
        Press::FocusGame
    }

    /// The feed reported the game active. True exactly once when a press
    /// was waiting for that within [`FOCUS_WAIT`]; a later activation is
    /// the user's own doing and copies nothing.
    pub fn focused(&mut self, now: Instant) -> bool {
        match self.pending.take() {
            Some(since) if now < since + FOCUS_WAIT => true,
            _ => false,
        }
    }

    /// True once when a pending press outlived [`FOCUS_WAIT`] without the
    /// game taking focus, so the loop can say so at the cursor.
    pub fn timed_out(&mut self, now: Instant) -> bool {
        match self.pending {
            Some(since) if now >= since + FOCUS_WAIT => {
                self.pending = None;
                true
            }
            _ => false,
        }
    }
}

fn contains(r: Rect, (x, y): (i32, i32)) -> bool {
    x >= r.x && y >= r.y && x < r.x.saturating_add(r.w as i32) && y < r.y.saturating_add(r.h as i32)
}
