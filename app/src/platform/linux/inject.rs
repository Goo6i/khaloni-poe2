//! Ctrl+C injection and clipboard read for the hover price check.
//!
//! Wayland compositors do not let a client synthesize keyboard input into
//! another window, so this goes through a virtual keyboard registered with
//! the kernel's uinput driver instead: the compositor sees it as a real
//! keyboard and delivers the Ctrl+C to whatever window is focused, exactly
//! like the game's own "copy item to clipboard" hover shortcut. Proven
//! working at milestone 0 (see spikes/src/bin/inject.rs).
//!
//! The device is built once and every injection runs on one dedicated
//! thread (a device injected from a short-lived per-press thread does not
//! deliver events to the game under gamescope). A no-item hover is
//! detected by the clipboard not changing after the Ctrl+C.
//!
//! The clipboard is never written, only read. Under gamescope the game
//! can only overwrite a stale clipboard, not one an external process just
//! set, so priming or restoring the clipboard blocks the game's copy
//! (verified against wl-copy, Klipper DBus, and Exiled Exchange 2's
//! sentinel approach, which relies on Electron's clipboard behaving unlike
//! wl-copy). The copied item text is therefore left in the clipboard; the
//! previous content stays in the clipboard manager's history.
//!
//! One-time setup required on the user's machine before this runs (the app
//! must never run as root just to reach /dev/uinput):
//!
//! ```text
//! sudo usermod -aG input $USER
//! echo 'KERNEL=="uinput", GROUP="input", MODE="0660"' | sudo tee /etc/udev/rules.d/99-poe2-lens-uinput.rules
//! sudo udevadm control --reload-rules && sudo udevadm trigger /dev/uinput
//! # then log out and back in for the new group membership to take effect
//! ```

use std::{process::Command, thread::sleep, time::Duration};

use evdev::{uinput::VirtualDeviceBuilder, AttributeSet, EventType, InputEvent, Key};


/// Runs one virtual keyboard on a dedicated thread and does every
/// injection on that same thread. A uinput device injected from a
/// short-lived worker thread (a thread spawned per price check) does not
/// deliver its events to the game under gamescope, while the exact same
/// code on a long-lived thread does; the milestone-0 spike works because
/// it creates and emits on its process's main thread and keeps it alive.
/// So the device is created on, and only ever emitted from, this one
/// persistent thread. Requests arrive as reply channels; the result (item
/// text, or empty when nothing was hovered) is sent back on each.
/// A request to the injector thread: copy the hovered item, or type a chat
/// macro. Both run on the one long-lived injector thread (see the struct
/// doc for why injection must stay on a single thread).
enum InjectReq {
    /// Copy the hovered item. The u64 is a pre-copy delay in ms: for hotkeys
    /// that hold Ctrl (chat-style CTRL+N actions), we wait for the user to
    /// release the modifier before injecting Ctrl+C, or the held Ctrl
    /// collides with the injected Ctrl+C and the game does not copy.
    Copy(std::sync::mpsc::Sender<anyhow::Result<String>>, u64),
    Type(String, u64),
}

pub struct Injector {
    req_tx: std::sync::mpsc::Sender<InjectReq>,
}

impl Injector {
    pub fn new() -> anyhow::Result<Injector> {
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let (req_tx, req_rx) = std::sync::mpsc::channel::<InjectReq>();
        std::thread::spawn(move || {
            let mut dev = match build_device() {
                Ok(d) => {
                    let _ = ready_tx.send(Ok(()));
                    d
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e.to_string()));
                    return;
                }
            };
            // Settle once so the first price check after launch works.
            sleep(Duration::from_millis(700));
            // Distinguishes a real copy from a misclick via X11 CLIPBOARD
            // ownership events. None if X/XFIXES is unavailable, in which case
            // copy_hovered falls back to content-change detection.
            let watcher = crate::platform::clipwatch::ClipboardWatcher::new();
            if watcher.is_none() {
                eprintln!("clipboard watcher unavailable; F7 uses content detection only");
            }
            for req in req_rx {
                match req {
                    InjectReq::Copy(reply, pre_delay) => {
                        let _ = reply.send(copy_hovered(&mut dev, pre_delay, watcher.as_ref()));
                    }
                    InjectReq::Type(msg, delay) => {
                        if let Err(e) = type_text(&mut dev, &msg, delay) {
                            eprintln!("macro type failed: {e}");
                        }
                    }
                }
            }
        });
        ready_rx
            .recv()
            .map_err(|_| anyhow::anyhow!("injector thread died"))?
            .map_err(|e| anyhow::anyhow!(e))?;
        Ok(Injector { req_tx })
    }

    /// Queues a copy of the hovered item; the text (or an error) is delivered
    /// on `reply`. `pre_delay_ms` waits before injecting Ctrl+C so a held
    /// hotkey modifier (Ctrl) can clear first; pass 0 for a plain-key hotkey.
    pub fn submit(&self, reply: std::sync::mpsc::Sender<anyhow::Result<String>>, pre_delay_ms: u64) {
        let _ = self.req_tx.send(InjectReq::Copy(reply, pre_delay_ms));
    }

    /// Queues a chat macro: opens chat, waits `open_delay_ms` for the chat
    /// box to be ready, types `msg`, and sends it. Non-blocking.
    pub fn type_text(&self, msg: String, open_delay_ms: u64) {
        let _ = self.req_tx.send(InjectReq::Type(msg, open_delay_ms));
    }
}

/// True when clipboard text is a copied PoE item (English client).
fn is_poe_item(text: &str) -> bool {
    text.starts_with("Item Class: ") || text.starts_with("Rarity: ")
}

/// Injects Ctrl+C and returns the item text the game copies (empty when
/// nothing is hovered). Deliberately never writes the clipboard: under
/// wine/Proton the game will not copy over a clipboard an external process
/// just set (a primed sentinel blocks the copy entirely, verified live), so
/// we only read.
///
/// Detection is by content change: a NEW hovered item makes the clipboard
/// differ from `before`. A misclick over empty space and a re-check of the
/// SAME item both leave the clipboard byte-for-byte identical, so they cannot
/// be told apart from content; in that case we return whatever PoE item is
/// still on the clipboard (a re-check re-prices; a misclick re-shows the last
/// item). Runs only on the injector thread.
fn copy_hovered(
    dev: &mut evdev::uinput::VirtualDevice,
    pre_delay_ms: u64,
    watcher: Option<&crate::platform::clipwatch::ClipboardWatcher>,
) -> anyhow::Result<String> {
    // Let a held hotkey modifier (Ctrl for CTRL+N actions) release before we
    // inject Ctrl+C; otherwise the held Ctrl collides with the injection and
    // the game copies nothing (F7 passes 0: no modifier to clear).
    if pre_delay_ms > 0 {
        sleep(Duration::from_millis(pre_delay_ms));
    }

    // Clear stale ownership events before we trigger the copy.
    if let Some(w) = watcher {
        w.drain();
    }
    let before = clipboard_read();
    emit(dev, Key::KEY_LEFTCTRL, true)?;
    emit(dev, Key::KEY_C, true)?;
    emit(dev, Key::KEY_C, false)?;
    emit(dev, Key::KEY_LEFTCTRL, false)?;

    // Primary, fast signal: a genuinely NEW hovered item makes the clipboard
    // content change. This breaks out as soon as it is seen, so the common case
    // (hovering different items) stays snappy.
    let deadline = std::time::Instant::now() + Duration::from_millis(500);
    while std::time::Instant::now() < deadline {
        sleep(Duration::from_millis(40));
        if let Some(cur) = clipboard_read() {
            if is_poe_item(&cur) && Some(&cur) != before.as_ref() {
                return Ok(cur);
            }
        }
    }

    // Content did not change: a re-check of the same item, or a misclick over
    // empty space. They are byte-identical on the clipboard, so the ownership
    // event decides. Checked AFTER the 500ms window (not in a tight loop right
    // after Ctrl+C): by now the game's copy event, if any, has arrived on the
    // socket and the first poll reads it. A tight poll loop started immediately
    // after Ctrl+C races the event and misses it (x11rb reads non-blocking per
    // call), which is why we wait first. No event within the grace window means
    // nothing was copied -> nothing hovered.
    if let Some(w) = watcher {
        if !w.wait_for_change(Duration::from_millis(150)) {
            return Ok(String::new());
        }
    }
    match clipboard_read() {
        Some(c) if is_poe_item(&c) => Ok(c),
        _ => Ok(String::new()),
    }
}

/// Reads the Wayland clipboard via `wl-paste`, retrying once because it can
/// return empty transiently under game load. Same-item re-checks (where the
/// content, and so the mirror, does not change) are disambiguated not here
/// but by the X11 ownership probe in `clipwatch` — see `copy_hovered`.
fn clipboard_read() -> Option<String> {
    for _ in 0..2 {
        if let Ok(out) = Command::new("wl-paste").arg("-n").output() {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout).into_owned();
                if !s.is_empty() {
                    return Some(s);
                }
            }
        }
    }
    None
}



/// Maps an ASCII char to (key, needs_shift) for a US QWERTY layout, for
/// typing chat/macro text through the virtual keyboard. Returns `None` for
/// characters we cannot type (skipped rather than mistyped). Covers what
/// PoE chat needs: letters, digits, space, and the common punctuation in
/// commands and player names.
fn char_to_key(c: char) -> Option<(Key, bool)> {
    let plain = |k| Some((k, false));
    let shift = |k| Some((k, true));
    if c.is_ascii_uppercase() {
        return letter_key(c.to_ascii_lowercase()).map(|k| (k, true));
    }
    if c.is_ascii_lowercase() {
        return letter_key(c).map(|k| (k, false));
    }
    match c {
        ' ' => plain(Key::KEY_SPACE),
        '1' => plain(Key::KEY_1),
        '2' => plain(Key::KEY_2),
        '3' => plain(Key::KEY_3),
        '4' => plain(Key::KEY_4),
        '5' => plain(Key::KEY_5),
        '6' => plain(Key::KEY_6),
        '7' => plain(Key::KEY_7),
        '8' => plain(Key::KEY_8),
        '9' => plain(Key::KEY_9),
        '0' => plain(Key::KEY_0),
        '-' => plain(Key::KEY_MINUS),
        '_' => shift(Key::KEY_MINUS),
        '=' => plain(Key::KEY_EQUAL),
        '+' => shift(Key::KEY_EQUAL),
        '/' => plain(Key::KEY_SLASH),
        '?' => shift(Key::KEY_SLASH),
        '.' => plain(Key::KEY_DOT),
        ',' => plain(Key::KEY_COMMA),
        '\'' => plain(Key::KEY_APOSTROPHE),
        '"' => shift(Key::KEY_APOSTROPHE),
        ';' => plain(Key::KEY_SEMICOLON),
        ':' => shift(Key::KEY_SEMICOLON),
        '!' => shift(Key::KEY_1),
        '@' => shift(Key::KEY_2),
        _ => None,
    }
}

fn letter_key(c: char) -> Option<Key> {
    Some(match c {
        'a' => Key::KEY_A, 'b' => Key::KEY_B, 'c' => Key::KEY_C, 'd' => Key::KEY_D,
        'e' => Key::KEY_E, 'f' => Key::KEY_F, 'g' => Key::KEY_G, 'h' => Key::KEY_H,
        'i' => Key::KEY_I, 'j' => Key::KEY_J, 'k' => Key::KEY_K, 'l' => Key::KEY_L,
        'm' => Key::KEY_M, 'n' => Key::KEY_N, 'o' => Key::KEY_O, 'p' => Key::KEY_P,
        'q' => Key::KEY_Q, 'r' => Key::KEY_R, 's' => Key::KEY_S, 't' => Key::KEY_T,
        'u' => Key::KEY_U, 'v' => Key::KEY_V, 'w' => Key::KEY_W, 'x' => Key::KEY_X,
        'y' => Key::KEY_Y, 'z' => Key::KEY_Z,
        _ => return None,
    })
}

/// Types `msg` into the game chat: Enter opens the chat box, the message is
/// typed key by key (with shift where the layout needs it), and Enter sends
/// it. Runs only on the injector thread. Characters `char_to_key` cannot
/// map are skipped rather than mistyped.
fn type_text(dev: &mut evdev::uinput::VirtualDevice, msg: &str, open_delay_ms: u64) -> anyhow::Result<()> {
    emit(dev, Key::KEY_ENTER, true)?;
    emit(dev, Key::KEY_ENTER, false)?;
    // The chat input takes a moment to open and accept focus; without this
    // settle the first character is dropped (observed live: "thanks!" typed
    // as "hanks!", and a leading "/" lost so commands did not fire).
    // Tunable via Config::macro_open_delay_ms.
    sleep(Duration::from_millis(open_delay_ms));
    for c in msg.chars() {
        let Some((k, shift)) = char_to_key(c) else { continue };
        if shift {
            emit(dev, Key::KEY_LEFTSHIFT, true)?;
        }
        emit(dev, k, true)?;
        emit(dev, k, false)?;
        if shift {
            emit(dev, Key::KEY_LEFTSHIFT, false)?;
        }
    }
    emit(dev, Key::KEY_ENTER, true)?;
    emit(dev, Key::KEY_ENTER, false)?;
    Ok(())
}

fn build_device() -> anyhow::Result<evdev::uinput::VirtualDevice> {
    let mut keys = AttributeSet::<Key>::new();
    keys.insert(Key::KEY_LEFTCTRL);
    keys.insert(Key::KEY_LEFTSHIFT);
    keys.insert(Key::KEY_ENTER);
    keys.insert(Key::KEY_C);
    // Every key the chat-macro typist can emit (letters, digits, space,
    // punctuation), discovered through char_to_key so the two never drift.
    for b in 0x20u8..0x7f {
        if let Some((k, _)) = char_to_key(b as char) {
            keys.insert(k);
        }
    }
    Ok(VirtualDeviceBuilder::new()?
        .name("poe2-lens-kbd")
        .with_keys(&keys)?
        .build()?)
}

fn emit(dev: &mut evdev::uinput::VirtualDevice, k: Key, down: bool) -> anyhow::Result<()> {
    dev.emit(&[InputEvent::new(EventType::KEY, k.code(), down as i32)])?;
    sleep(Duration::from_millis(25));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn char_to_key_covers_chat_characters() {
        assert_eq!(char_to_key('a'), Some((Key::KEY_A, false)));
        assert_eq!(char_to_key('z'), Some((Key::KEY_Z, false)));
        // Uppercase needs shift on the same key.
        assert_eq!(char_to_key('A'), Some((Key::KEY_A, true)));
        assert_eq!(char_to_key(' '), Some((Key::KEY_SPACE, false)));
        assert_eq!(char_to_key('/'), Some((Key::KEY_SLASH, false)));
        assert_eq!(char_to_key('1'), Some((Key::KEY_1, false)));
        assert_eq!(char_to_key('!'), Some((Key::KEY_1, true)));
        // A full command types with no gaps.
        assert!("/hideout".chars().all(|c| char_to_key(c).is_some()));
        assert!("thanks!".chars().all(|c| char_to_key(c).is_some()));
        // Unsupported char is skipped, not mistyped.
        assert_eq!(char_to_key('€'), None);
    }
}

