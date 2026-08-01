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

/// An event from the game-window feed (KWin scripting on Linux; Win32
/// polling through `gamewin_diff` on Windows).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameWindowEvent {
    Geometry(Rect),
    /// True when the game window currently holds focus.
    Active(bool),
    GameGone,
    /// Live pointer position in global logical coordinates (throttled to
    /// 100ms and >4px moves by the feed).
    Cursor(i32, i32),
    /// True while the game is actually on screen: not minimized and not
    /// covered by other windows. Focus is deliberately NOT part of this —
    /// an unfocused-but-visible game keeps its overlay.
    Visible(bool),
}

pub struct RegionFrame {
    pub gray: GrayImage,
}

/// Held for the overlay's lifetime; a second overlay instance fails to
/// acquire it and exits with a clear message instead of fighting the first
/// over hotkeys, the D-Bus name, and the tray (which is exactly what
/// happened when stale instances piled up).
pub struct InstanceLock {
    #[cfg(target_os = "linux")]
    _sock: std::os::unix::net::UnixListener,
    #[cfg(target_os = "windows")]
    _mutex: isize,
}

#[cfg(target_os = "linux")]
pub fn single_instance() -> anyhow::Result<InstanceLock> {
    use std::os::linux::net::SocketAddrExt;
    // Abstract-namespace socket: kernel-owned, vanishes with the process,
    // no stale lockfiles to clean up after a crash.
    let addr = std::os::unix::net::SocketAddr::from_abstract_name(b"khaloni-poe2-overlay")?;
    let sock = std::os::unix::net::UnixListener::bind_addr(&addr)
        .map_err(|_| anyhow::anyhow!("khaloni-poe2 is already running"))?;
    Ok(InstanceLock { _sock: sock })
}

#[cfg(target_os = "windows")]
pub fn single_instance() -> anyhow::Result<InstanceLock> {
    // Leading :: — inside this module, bare `windows` is our backend
    // submodule, not the external crate.
    use ::windows::core::HSTRING;
    use ::windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use ::windows::Win32::System::Threading::CreateMutexW;
    let handle = unsafe { CreateMutexW(None, true, &HSTRING::from("khaloni-poe2-overlay")) }
        .map_err(|e| anyhow::anyhow!("instance lock: {e}"))?;
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        anyhow::bail!("khaloni-poe2 is already running");
    }
    // Deliberately leaked: the mutex must live until process exit.
    Ok(InstanceLock { _mutex: handle.0 as isize })
}
