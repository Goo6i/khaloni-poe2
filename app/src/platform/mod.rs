//! Platform layer: the shared event/data types the main loop speaks, plus
//! the cfg-selected backend modules (overlay, capture, inject, hotkeys,
//! gamewin). Selection is compile-time — concrete types with identical
//! public APIs per target (the winit/tauri pattern), not trait objects:
//! the capture and hotkey layers are async and each backend is picked once
//! at startup, so cfg dispatch is simpler and avoids async-trait friction.

use image::GrayImage;

use crate::config::Rect;

pub mod chord;
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hotkey {
    PriceCheck,
    /// A dynamically-registered action fired, identified by its id string
    /// (e.g. "macro-0", "url-1"). The main loop routes by id prefix, so new
    /// hotkey-bound features add an id namespace without touching this enum.
    Extra(String),
}

/// Who holds keyboard focus, as far as the overlay cares. `Game` is what
/// injection needs (a Ctrl+C goes to the focused window); `Overlay` is our
/// own surface, which the scan policy treats as still playing (clicking
/// the trade card activates it); `Other` is any other window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Game,
    Overlay,
    Other,
}

/// An event from the game-window feed (KWin scripting on Linux; Win32
/// polling through `gamewin_diff` on Windows).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameWindowEvent {
    Geometry(Rect),
    /// Reported on every change of who holds focus, and once initially.
    Active(Focus),
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

/// Everything the game-window feed has said so far, folded into one value.
/// Starts from "no game, another window focused, nothing visible": the feed
/// reports its real state within its first tick, and until it does the safe
/// assumption is the one under which no hotkey types into a window and no
/// label is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GameWindowState {
    /// Last reported game rect; `None` before the first report and after
    /// `GameGone`.
    pub rect: Option<Rect>,
    pub focus: Focus,
    pub visible: bool,
    pub cursor: Option<(i32, i32)>,
}

impl Default for GameWindowState {
    fn default() -> GameWindowState {
        GameWindowState { rect: None, focus: Focus::Other, visible: false, cursor: None }
    }
}

impl GameWindowState {
    pub fn apply(&mut self, ev: GameWindowEvent) {
        match ev {
            GameWindowEvent::Geometry(r) => self.rect = Some(r),
            GameWindowEvent::Active(f) => self.focus = f,
            GameWindowEvent::GameGone => {
                self.rect = None;
                self.visible = false;
            }
            GameWindowEvent::Cursor(x, y) => self.cursor = Some((x, y)),
            GameWindowEvent::Visible(v) => self.visible = v,
        }
    }

    pub fn present(&self) -> bool {
        self.rect.is_some()
    }

    pub fn game_focused(&self) -> bool {
        self.focus == Focus::Game
    }

    /// Waits up to `timeout` for the feed's first geometry, folding every
    /// event received meanwhile into the returned state. The feed reports
    /// focus and visibility only on change, so an event dropped here would
    /// stay wrong until the user alt-tabs.
    pub fn wait_for_geometry(
        rx: &std::sync::mpsc::Receiver<GameWindowEvent>,
        timeout: std::time::Duration,
    ) -> GameWindowState {
        let mut state = GameWindowState::default();
        let deadline = std::time::Instant::now() + timeout;
        while state.rect.is_none() {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match rx.recv_timeout(remaining) {
                Ok(ev) => state.apply(ev),
                Err(_) => break,
            }
        }
        // Whatever else is already queued belongs to the same startup
        // burst (the feed sends geometry before focus).
        while let Ok(ev) = rx.try_recv() {
            state.apply(ev);
        }
        state
    }
}

/// Why an overlay call failed, carried inside the `anyhow::Error` the
/// methods return (`err.downcast_ref::<OverlayError>()`).
///
/// The contract: `Overlay::new`/`open` fail with `Startup` or `NoOutput`.
/// Once built, the methods fail only with `Connection` - the compositor
/// itself is gone, which nothing here can recover. The surface being closed
/// by the compositor is NOT an error: the methods turn into no-ops and
/// `is_closed()` reports it, so the caller can `open` a new overlay.
#[derive(Debug)]
pub enum OverlayError {
    /// No Wayland compositor, or one without layer-shell/shm.
    Startup(String),
    /// No output exists right now (all monitors off). Worth retrying.
    NoOutput,
    /// The Wayland connection broke while running.
    Connection(String),
}

impl std::fmt::Display for OverlayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OverlayError::Startup(why) => write!(f, "overlay cannot start: {why}"),
            OverlayError::NoOutput => write!(f, "no display output to put the overlay on"),
            OverlayError::Connection(why) => write!(f, "lost the Wayland connection: {why}"),
        }
    }
}

impl std::error::Error for OverlayError {}

/// What the capture backend reports about its stream, for the main loop to
/// show: a stream that ends would otherwise stop reward pricing silently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureEvent {
    /// Frames are flowing (first start and every successful re-open).
    Streaming,
    /// The stream ended or errored; a re-open follows when one is possible.
    Lost(String),
    /// A re-opened session handed out a new restore token to persist.
    NewToken(String),
    /// Re-opening failed for good; capture is off until restart.
    GaveUp(String),
}

/// Runtime controls for `capture::consume_supervised`.
#[derive(Clone, Default)]
pub struct CaptureControl {
    /// While true, frames are dequeued and dropped without the grayscale
    /// conversion (pricing is paused or the game is not on screen).
    pub paused: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Stream lifecycle reports; `None` to only log them.
    pub events: Option<std::sync::mpsc::Sender<CaptureEvent>>,
}

impl CaptureControl {
    pub(crate) fn report(&self, ev: CaptureEvent) {
        match &ev {
            CaptureEvent::Streaming => {}
            CaptureEvent::Lost(why) => eprintln!("capture: stream lost: {why}"),
            CaptureEvent::NewToken(_) => {}
            CaptureEvent::GaveUp(why) => eprintln!("capture: giving up: {why}"),
        }
        if let Some(tx) = &self.events {
            let _ = tx.send(ev);
        }
    }
}

/// The full set of triggers the hotkey backend binds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HotkeyBindings {
    pub price_check: String,
    /// (id, trigger) for every dynamically-bound action.
    pub extra: Vec<(String, String)>,
}

/// Hands a new set of bindings to a running `hotkeys::listen_with`. Only
/// the latest unapplied set is kept. Runtime-agnostic (a mutex and a
/// waker), so the main loop calls `rebind` from a plain thread.
#[derive(Clone, Default)]
pub struct HotkeyRebind(std::sync::Arc<RebindInner>);

#[derive(Default)]
struct RebindInner {
    slot: std::sync::Mutex<Option<HotkeyBindings>>,
    waker: futures_util::task::AtomicWaker,
}

impl HotkeyRebind {
    pub fn new() -> HotkeyRebind {
        HotkeyRebind::default()
    }

    pub fn rebind(&self, bindings: HotkeyBindings) {
        *self.0.slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(bindings);
        self.0.waker.wake();
    }

    /// Resolves with the next requested set.
    pub async fn next(&self) -> HotkeyBindings {
        futures_util::future::poll_fn(|cx| {
            self.0.waker.register(cx.waker());
            match self.0.slot.lock().unwrap_or_else(|e| e.into_inner()).take() {
                Some(b) => std::task::Poll::Ready(b),
                None => std::task::Poll::Pending,
            }
        })
        .await
    }
}

/// A `Command` for a program of the host system (`qdbus6`, `wl-paste`,
/// `xdg-open`, `kdialog`, ...). Started from a Steam launch option, this
/// process inherits Steam's runtime environment: `LD_PRELOAD` carries the
/// Steam overlay and `LD_LIBRARY_PATH` pins the runtime's old libraries,
/// under which host tools fail to load (seen as `libcurl.so.4: version
/// CURL_OPENSSL_4 not found` from kde-open). Steam keeps the pre-runtime
/// values in `SYSTEM_LD_LIBRARY_PATH` / `SYSTEM_PATH`; the child gets those
/// back. Outside Steam, and on Windows, this is `Command::new`.
pub fn host_command(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    let mut cmd = std::process::Command::new(program);
    for (key, value) in host_env_changes(|k| std::env::var(k).ok()) {
        match value {
            Some(v) => cmd.env(key, v),
            None => cmd.env_remove(key),
        };
    }
    cmd
}

/// The environment edits `host_command` applies, given a lookup into the
/// current environment: `(name, Some(new value))` to set, `(name, None)` to
/// remove. Empty when the process does not run under Steam.
pub fn host_env_changes(get: impl Fn(&str) -> Option<String>) -> Vec<(&'static str, Option<String>)> {
    let mut out = Vec::new();
    if cfg!(windows) {
        return out;
    }
    const STEAM_OVERLAY: &str = "gameoverlayrenderer";
    let preload = get("LD_PRELOAD").unwrap_or_default();
    let in_runtime = get("STEAM_RUNTIME").is_some_and(|v| !v.is_empty());
    if !in_runtime && !preload.contains(STEAM_OVERLAY) {
        return out;
    }
    if !preload.is_empty() {
        // LD_PRELOAD separates entries with colons or spaces. Only Steam's
        // overlay goes; anything the user preloads on purpose stays.
        let kept: Vec<&str> = preload
            .split([':', ' '])
            .filter(|e| !e.is_empty() && !e.contains(STEAM_OVERLAY))
            .collect();
        out.push(("LD_PRELOAD", (!kept.is_empty()).then(|| kept.join(":"))));
    }
    if in_runtime {
        let system_libs = get("SYSTEM_LD_LIBRARY_PATH").filter(|v| !v.is_empty());
        if system_libs.is_some() || get("LD_LIBRARY_PATH").is_some() {
            out.push(("LD_LIBRARY_PATH", system_libs));
        }
        if let Some(path) = get("SYSTEM_PATH").filter(|v| !v.is_empty()) {
            out.push(("PATH", Some(path)));
        }
    }
    out
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
