//! Ctrl+C injection and clipboard read for the hover price check —
//! Windows twin of platform/linux/inject.rs, same public API.
//!
//! Injection goes through SendInput: unlike Wayland, Windows lets any
//! process synthesize keyboard input into the foreground window, so no
//! virtual device or elevated setup is needed. The events land on whatever
//! window has focus, exactly like the game's own "copy item to clipboard"
//! hover shortcut.
//!
//! Everything still runs on one dedicated thread. SendInput itself has no
//! thread affinity, but a single thread keeps injections strictly ordered
//! (a macro typed while a price check is mid-Ctrl+C would interleave
//! keystrokes), and it mirrors the Linux architecture so the main loop
//! treats both the same.
//!
//! The clipboard is never written, only read (via arboard). Read-only was
//! forced on Linux by wine's clipboard behavior; here it is kept because it
//! costs nothing and leaves the user's clipboard history untouched by us —
//! the copied item text is the game's own write. A no-item hover is
//! detected by the clipboard not changing after the Ctrl+C; the same-item
//! re-check (content byte-identical, so no change to observe) is
//! disambiguated by GetClipboardSequenceNumber — Windows bumps a global
//! counter on every clipboard write, even one writing identical content —
//! which replaces the X11 XFIXES ownership probe the Linux side needs.

use std::{thread::sleep, time::Duration};

use windows::Win32::System::DataExchange::GetClipboardSequenceNumber;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, VkKeyScanW, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, VIRTUAL_KEY, VK_CONTROL, VK_RETURN, VK_SHIFT,
};

/// A request to the injector thread: copy the hovered item, or type a chat
/// macro. Both run on the one long-lived injector thread (see the module
/// doc for why injection stays on a single thread).
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
            // One clipboard handle for the thread's lifetime, like the Linux
            // side's one uinput device: arboard's Windows backend opens and
            // closes the system clipboard per call, so this is cheap, but
            // failing here (no clipboard access at all) must fail new() the
            // same way a missing /dev/uinput does on Linux.
            let mut cb = match arboard::Clipboard::new() {
                Ok(c) => {
                    let _ = ready_tx.send(Ok(()));
                    c
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e.to_string()));
                    return;
                }
            };
            for req in req_rx {
                match req {
                    InjectReq::Copy(reply, pre_delay) => {
                        let _ = reply.send(copy_hovered(&mut cb, pre_delay));
                    }
                    InjectReq::Type(msg, delay) => type_text(&msg, delay),
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
/// nothing is hovered). Same detection contract as the Linux twin:
///
/// Primary signal is content change — a genuinely NEW hovered item makes
/// the clipboard differ from `before`, and the 500ms poll breaks out as
/// soon as it sees that, keeping the common case snappy. A misclick over
/// empty space and a re-check of the SAME item both leave the content
/// byte-identical, so the clipboard *sequence number* decides: the game's
/// copy bumps it even when the bytes match, a no-op hover does not. Runs
/// only on the injector thread.
fn copy_hovered(cb: &mut arboard::Clipboard, pre_delay_ms: u64) -> anyhow::Result<String> {
    // Let a held hotkey modifier (Ctrl for CTRL+N actions) release before we
    // inject Ctrl+C; otherwise the held Ctrl collides with the injection and
    // the game copies nothing (F7 passes 0: no modifier to clear).
    if pre_delay_ms > 0 {
        sleep(Duration::from_millis(pre_delay_ms));
    }

    // Snapshot the sequence number before triggering the copy, so any bump
    // observed later is attributable to it (the drain() analogue).
    let seq_before = unsafe { GetClipboardSequenceNumber() };
    let before = clipboard_read(cb);
    emit(VK_CONTROL, true);
    emit(VIRTUAL_KEY(b'C' as u16), true);
    emit(VIRTUAL_KEY(b'C' as u16), false);
    emit(VK_CONTROL, false);

    // Primary, fast signal: new item -> content change, break out early.
    let deadline = std::time::Instant::now() + Duration::from_millis(500);
    while std::time::Instant::now() < deadline {
        sleep(Duration::from_millis(40));
        if let Some(cur) = clipboard_read(cb) {
            if is_poe_item(&cur) && Some(&cur) != before.as_ref() {
                return Ok(cur);
            }
        }
    }

    // Content did not change: same-item re-check vs. misclick tie-break.
    // The sequence number is a cheap synchronous read (no OpenClipboard),
    // so unlike the X11 event probe there is no race to wait out — but the
    // game's copy can still land marginally after our 500ms window, so give
    // it the same 150ms grace the Linux side does before concluding
    // "nothing hovered".
    let grace = std::time::Instant::now() + Duration::from_millis(150);
    loop {
        if unsafe { GetClipboardSequenceNumber() } != seq_before {
            break;
        }
        if std::time::Instant::now() >= grace {
            return Ok(String::new());
        }
        sleep(Duration::from_millis(25));
    }
    match clipboard_read(cb) {
        Some(c) if is_poe_item(&c) => Ok(c),
        _ => Ok(String::new()),
    }
}

/// Reads the clipboard via arboard, retrying once: the read can fail or
/// come back empty transiently when another process (the game, mid-copy)
/// holds the clipboard open. Mirrors the Linux wl-paste retry.
fn clipboard_read(cb: &mut arboard::Clipboard) -> Option<String> {
    for _ in 0..2 {
        if let Ok(s) = cb.get_text() {
            if !s.is_empty() {
                return Some(s);
            }
        }
    }
    None
}

/// Types `msg` into the game chat: Enter opens the chat box, the message is
/// typed key by key (with Shift where the layout needs it), and Enter sends
/// it. Runs only on the injector thread. VkKeyScanW consults the user's
/// actual keyboard layout, so unlike the Linux twin's hardcoded US-QWERTY
/// table this types correctly on any layout; characters the layout cannot
/// produce (or that would need Ctrl/Alt chords) are skipped rather than
/// mistyped.
fn type_text(msg: &str, open_delay_ms: u64) {
    emit(VK_RETURN, true);
    emit(VK_RETURN, false);
    // The chat input takes a moment to open and accept focus; without this
    // settle the first character is dropped (observed live on Linux:
    // "thanks!" typed as "hanks!", and a leading "/" lost so commands did
    // not fire). Tunable via Config::macro_open_delay_ms.
    sleep(Duration::from_millis(open_delay_ms));
    for c in msg.chars() {
        // VkKeyScanW takes one UTF-16 unit; chars outside the BMP (code
        // points that don't fit u16) have no key anyway.
        let Ok(unit) = u16::try_from(c as u32) else { continue };
        let scan = unsafe { VkKeyScanW(unit) };
        if scan == -1 {
            continue; // layout cannot produce this character
        }
        let vk = VIRTUAL_KEY((scan & 0xff) as u16);
        // High byte is the needed modifier state: bit 0 Shift, bit 1 Ctrl,
        // bit 2 Alt. Ctrl/Alt chords are skipped — injecting Ctrl inside
        // the chat box triggers game shortcuts instead of typing.
        let mods = (scan >> 8) & 0x07;
        if mods & 0b110 != 0 {
            continue;
        }
        let shift = mods & 1 != 0;
        if shift {
            emit(VK_SHIFT, true);
        }
        emit(vk, true);
        emit(vk, false);
        if shift {
            emit(VK_SHIFT, false);
        }
    }
    emit(VK_RETURN, true);
    emit(VK_RETURN, false);
}

/// Sends one key transition, then paces 25ms — the same pacing as the Linux
/// emit(): the game samples input per frame and drops transitions that
/// arrive faster than it polls.
fn emit(vk: VIRTUAL_KEY, down: bool) {
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if down { KEYBD_EVENT_FLAGS(0) } else { KEYEVENTF_KEYUP },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
    sleep(Duration::from_millis(25));
}
