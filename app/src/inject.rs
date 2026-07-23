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
pub struct Injector {
    req_tx: std::sync::mpsc::Sender<std::sync::mpsc::Sender<anyhow::Result<String>>>,
}

impl Injector {
    pub fn new() -> anyhow::Result<Injector> {
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let (req_tx, req_rx) =
            std::sync::mpsc::channel::<std::sync::mpsc::Sender<anyhow::Result<String>>>();
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
            for reply in req_rx {
                let _ = reply.send(copy_hovered(&mut dev));
            }
        });
        ready_rx
            .recv()
            .map_err(|_| anyhow::anyhow!("injector thread died"))?
            .map_err(|e| anyhow::anyhow!(e))?;
        Ok(Injector { req_tx })
    }

    /// Queues a price check; the item text (or an error) is delivered on
    /// `reply`. Non-blocking: the injection runs on the injector thread.
    pub fn submit(&self, reply: std::sync::mpsc::Sender<anyhow::Result<String>>) {
        let _ = self.req_tx.send(reply);
    }
}

/// True when clipboard text is a copied PoE item (English client).
fn is_poe_item(text: &str) -> bool {
    text.starts_with("Item Class: ") || text.starts_with("Rarity: ")
}

/// The game runs on gamescope's nested Xwayland (DISPLAY=:N) and copies
/// items to THAT inner X clipboard first; gamescope then forwards the text
/// to the user's real Wayland clipboard. Reading the inner clipboard
/// directly (a) never touches the user's real clipboard and (b) lets us
/// CLEAR it before the copy so a re-check of the same item is detectable,
/// which the outer path could not do (any outer write blocks the copy).
/// Discovered from gamescopereaper's environ; None means no gamescope
/// (dev desktop), and the caller falls back to the outer Wayland read.
fn inner_display() -> Option<String> {
    let out = Command::new("pgrep").args(["-x", "gamescopereaper"]).output().ok()?;
    let pid = String::from_utf8_lossy(&out.stdout).split_whitespace().next()?.to_string();
    let environ = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
    environ
        .split(|&b| b == 0)
        .filter_map(|kv| std::str::from_utf8(kv).ok())
        .find_map(|kv| kv.strip_prefix("DISPLAY=").map(str::to_string))
}

/// Injects Ctrl+C and returns the item text the game copies (empty when
/// nothing is hovered). Reads the inner gamescope X clipboard when present
/// so the user's real clipboard is never involved; clears it first so a
/// re-check of the same item still registers as a fresh copy. Falls back
/// to the outer Wayland clipboard (read-only, change-detected) off
/// gamescope. Runs only on the injector thread.
fn copy_hovered(dev: &mut evdev::uinput::VirtualDevice) -> anyhow::Result<String> {
    let inner = inner_display();
    if let Some(dpy) = &inner {
        // Clear the inner selection so the game's copy is the only PoE
        // item present afterward; an empty-space F7 then leaves it empty
        // and reports "no item" correctly. Wait for the clear to actually
        // take before injecting, otherwise xclip taking ownership AFTER
        // the game's copy would wipe the very item we want.
        inner_clear(dpy);
        let clear_deadline = std::time::Instant::now() + Duration::from_millis(200);
        while std::time::Instant::now() < clear_deadline {
            match inner_read(dpy) {
                Some(s) if !is_poe_item(&s) => break,
                None => break,
                _ => sleep(Duration::from_millis(15)),
            }
        }
    }
    let before = if inner.is_none() { clipboard_read() } else { None };

    emit(dev, Key::KEY_LEFTCTRL, true)?;
    emit(dev, Key::KEY_C, true)?;
    emit(dev, Key::KEY_C, false)?;
    emit(dev, Key::KEY_LEFTCTRL, false)?;

    let mut item = String::new();
    let deadline = std::time::Instant::now() + Duration::from_millis(600);
    while std::time::Instant::now() < deadline {
        sleep(Duration::from_millis(40));
        let cur = match &inner {
            Some(dpy) => inner_read(dpy),
            None => clipboard_read(),
        };
        if let Some(cur) = cur {
            // Inner path: it was cleared, so any PoE item present is the
            // fresh copy (same-item re-checks included). Outer path: keep
            // change-detection, since we cannot safely clear it.
            let fresh = inner.is_some() || Some(&cur) != before.as_ref();
            if is_poe_item(&cur) && fresh {
                item = cur;
                break;
            }
        }
    }
    Ok(item)
}

fn inner_read(display: &str) -> Option<String> {
    let out = Command::new("xclip")
        .args(["-o", "-selection", "clipboard", "-d", display])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn inner_clear(display: &str) {
    use std::io::Write;
    // xclip becomes the selection owner with empty content; the game
    // reclaims ownership on its next copy (standard X selection handoff).
    if let Ok(mut child) = Command::new("xclip")
        .args(["-i", "-selection", "clipboard", "-d", display])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
    {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(b"");
        }
        // xclip forks to hold the selection; do not wait on it.
    }
}

fn build_device() -> anyhow::Result<evdev::uinput::VirtualDevice> {
    let mut keys = AttributeSet::<Key>::new();
    keys.insert(Key::KEY_LEFTCTRL);
    keys.insert(Key::KEY_C);
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

fn clipboard_read() -> Option<String> {
    let out = Command::new("wl-paste").arg("-n").output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

