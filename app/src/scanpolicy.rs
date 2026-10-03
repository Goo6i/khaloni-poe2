//! Whether the reward and rumour scanners run, and whether their rows
//! draw, given where the game is and who has focus.
//!
//! Scanning follows keyboard focus: an unfocused game is not being played,
//! so nothing on it needs pricing, and with the capture set to a monitor
//! any window over the game (a white web page with text rows) reads like
//! a reward panel and yields phantom labels. Focus on our own overlay
//! counts as the game's: clicking the trade card makes the overlay the
//! active window, and the labels must not blink out for that. Hiding on
//! occlusion stays the user's `pause_when_hidden` choice, since a covered
//! game is also one the always-on-top overlay would draw over.

use crate::platform::Focus;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Inputs {
    pub game_present: bool,
    /// From the window feed: not minimized and not covered.
    pub game_visible: bool,
    pub focus: Focus,
    pub pause_when_hidden: bool,
    /// The tray's "Pause Pricing". It is an input here, not a write to the
    /// pause flag, because this decision is recomputed every tick and
    /// overwrote such a write within 16ms.
    pub user_paused: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decision {
    /// Stop feeding frames to OCR and rumour recognition.
    pub paused: bool,
    /// Draw the priced rows and rumour badges.
    pub show_rows: bool,
    /// The game is on screen as far as the user's hiding preference goes:
    /// where a cursor popup may still draw.
    pub on_screen: bool,
}

pub fn decide(i: Inputs) -> Decision {
    let attended = i.focus != Focus::Other;
    let on_screen = i.game_present && (i.game_visible || !i.pause_when_hidden);
    let paused = i.user_paused
        || !i.game_present
        || !attended
        || (!i.game_visible && i.pause_when_hidden);
    // Rows read while pricing ran would sit there unrefreshed for as long
    // as the pause lasts, so they leave with it.
    Decision { paused, show_rows: !i.user_paused && on_screen && attended, on_screen }
}
