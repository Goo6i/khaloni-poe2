//! Platform layer: the shared event/data types the main loop speaks, plus
//! the cfg-selected backend modules (overlay, capture, inject, hotkeys,
//! gamewin). Selection is compile-time — concrete types with identical
//! public APIs per target (the winit/tauri pattern), not trait objects:
//! the capture and hotkey layers are async and each backend is picked once
//! at startup, so cfg dispatch is simpler and avoids async-trait friction.

use image::GrayImage;

use crate::config::Rect;

pub mod gamewin_diff;
pub mod triggers;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::*;

/// A keyboard input relevant to editing a value box or a search field.
/// Digits and '.' keep dedicated variants (the appraisal value boxes match
/// on them); every other printable ASCII arrives as `Char` for text search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Digit(char),
    Char(char),
    Dot,
    Backspace,
    Enter,
    Escape,
    Up,
    Down,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hotkey {
    /// Master switch: hides the overlay and pauses the whole pipeline.
    /// Everything else (panel detection, focus pause, price freshness) is
    /// automatic, so this is the only state key a user needs.
    OverlayToggle,
    PriceCheck,
    /// A dynamically-registered action fired, identified by its id string
    /// (e.g. "macro-0", "url-1"). The main loop routes by id prefix, so new
    /// hotkey-bound features add an id namespace without touching this enum.
    Extra(String),
}

/// An event from the game-window feed (KWin scripting on Linux; the
/// Windows equivalent lands in SP3).
pub enum GameWindowEvent {
    Geometry(Rect),
    /// True when the game window currently holds focus.
    Active(bool),
    GameGone,
    /// Live pointer position in global logical coordinates (throttled to
    /// 100ms and >4px moves by the feed).
    Cursor(i32, i32),
}

pub struct RegionFrame {
    pub gray: GrayImage,
}
