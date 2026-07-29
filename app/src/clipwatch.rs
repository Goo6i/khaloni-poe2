//! Watches the X11 CLIPBOARD selection for ownership changes. Used as a probe
//! to learn whether the wine/Proton game re-asserts selection ownership on
//! each in-game copy (which would let F7 tell a real copy from a misclick).
//! The game is an XWayland client on the session DISPLAY, so its copy touches
//! the real X11 CLIPBOARD there; we listen via XFIXES.

use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::xfixes::{ConnectionExt as XfixesExt, SelectionEventMask};
use x11rb::protocol::xproto::{Atom, ConnectionExt as XprotoExt, CreateWindowAux, WindowClass};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;

pub struct ClipboardWatcher {
    conn: RustConnection,
    clipboard: Atom,
}

impl ClipboardWatcher {
    /// Connects to $DISPLAY (the XWayland server the game runs on) and
    /// subscribes to CLIPBOARD ownership changes. `None` if X/XFIXES is
    /// unavailable.
    pub fn new() -> Option<Self> {
        let (conn, screen_num) = x11rb::connect(None).ok()?;
        conn.xfixes_query_version(5, 0).ok()?.reply().ok()?;
        let screen = &conn.setup().roots[screen_num];
        let win = conn.generate_id().ok()?;
        conn.create_window(
            x11rb::COPY_DEPTH_FROM_PARENT,
            win,
            screen.root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_ONLY,
            x11rb::COPY_FROM_PARENT,
            &CreateWindowAux::new(),
        )
        .ok()?;
        let clipboard = conn.intern_atom(false, b"CLIPBOARD").ok()?.reply().ok()?.atom;
        conn.xfixes_select_selection_input(
            win,
            clipboard,
            SelectionEventMask::SET_SELECTION_OWNER
                | SelectionEventMask::SELECTION_WINDOW_DESTROY
                | SelectionEventMask::SELECTION_CLIENT_CLOSE,
        )
        .ok()?;
        conn.flush().ok()?;
        Some(ClipboardWatcher { conn, clipboard })
    }

    /// Discards any pending selection events.
    pub fn drain(&self) {
        while let Ok(Some(_)) = self.conn.poll_for_event() {}
    }

    /// True if a CLIPBOARD ownership change arrived within `timeout`.
    pub fn wait_for_change(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            while let Ok(Some(ev)) = self.conn.poll_for_event() {
                if let Event::XfixesSelectionNotify(e) = ev {
                    if e.selection == self.clipboard {
                        return true;
                    }
                }
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
