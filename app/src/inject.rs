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

/// Injects Ctrl+C and returns the item text the game copies (empty when
/// nothing is hovered). Deliberately never writes the clipboard: under
/// gamescope the game can only replace a stale clipboard, not one an
/// external process (wl-copy, Klipper) just set, so any priming or restore
/// blocks the copy. The item is detected by content becoming a PoE item
/// that differs from what was there before. Runs only on the injector
/// thread.
fn copy_hovered(dev: &mut evdev::uinput::VirtualDevice) -> anyhow::Result<String> {
    let before = clipboard_read();
    emit(dev, Key::KEY_LEFTCTRL, true)?;
    emit(dev, Key::KEY_C, true)?;
    emit(dev, Key::KEY_C, false)?;
    emit(dev, Key::KEY_LEFTCTRL, false)?;

    let mut item = String::new();
    let deadline = std::time::Instant::now() + Duration::from_millis(600);
    while std::time::Instant::now() < deadline {
        sleep(Duration::from_millis(40));
        if let Some(cur) = clipboard_read() {
            if is_poe_item(&cur) && Some(&cur) != before.as_ref() {
                item = cur;
                break;
            }
        }
    }
    Ok(item)
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

