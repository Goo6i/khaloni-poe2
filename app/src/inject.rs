//! Ctrl+C injection and clipboard read for the hover price check.
//!
//! Wayland compositors do not let a client synthesize keyboard input into
//! another window, so this goes through a virtual keyboard registered with
//! the kernel's uinput driver instead: the compositor sees it as a real
//! keyboard and delivers the Ctrl+C to whatever window is focused, exactly
//! like the game's own "copy item to clipboard" hover shortcut. Proven
//! working at milestone 0 (see spikes/src/bin/inject.rs).
//!
//! The virtual keyboard is created fresh for each injection and dropped
//! after. A device created once at startup and reused minutes later stops
//! delivering events (the milestone-0 spike copies item text as the user,
//! while a persistent device built at startup and used from a worker
//! thread came back with the clipboard untouched); the spike works
//! because it builds the device and injects while it is fresh, so this
//! does the same. The ~700ms build+settle cost is invisible on a manual,
//! occasional price check.
//!
//! The user's clipboard is saved before the copy and restored after, so
//! pressing the price-check key never leaves item text (or an empty
//! clipboard) behind. That save/restore is also how a no-item hover is
//! detected: if the clipboard is unchanged after the injected Ctrl+C, the
//! cursor was not over an item. This sidesteps KDE's clipboard manager,
//! which restores a cleared clipboard and would defeat a plain --clear.
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

/// Quiet time after the last price check before the user's clipboard is
/// restored. Long enough that a rapid burst of checks never triggers a
/// restore mid-burst (which would break the next copy), short enough that
/// the clipboard is back to normal moments after the user stops.
const RESTORE_IDLE: Duration = Duration::from_millis(2500);

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
            // The user's clipboard captured at the start of a price-check
            // burst, restored once the burst ends. Restoring between
            // presses breaks the game's next copy under gamescope (any
            // active clipboard write contends with the immediately
            // following Ctrl+C), so a burst runs with no restore and the
            // original is put back only after RESTORE_IDLE of quiet.
            let mut original: Option<String> = None;
            loop {
                match req_rx.recv_timeout(RESTORE_IDLE) {
                    Ok(reply) => {
                        // Capture the user's real clipboard the first time
                        // this burst clobbers it (item text always starts
                        // with "Item Class:", so it is never mistaken for
                        // the user's own content).
                        if original.is_none() {
                            if let Some(s) = clipboard_read() {
                                if !s.starts_with("Item Class:") {
                                    original = Some(s);
                                }
                            }
                        }
                        let _ = reply.send(copy_hovered(&mut dev));
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        if let Some(orig) = original.take() {
                            clipboard_restore(&orig);
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
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

/// Saves the clipboard, injects Ctrl+C, reads the item text the game
/// wrote, restores the clipboard, and returns the item text (empty string
/// when nothing was under the cursor). Runs only on the injector thread.
fn copy_hovered(dev: &mut evdev::uinput::VirtualDevice) -> anyhow::Result<String> {
    let saved = clipboard_read();
    emit(dev, Key::KEY_LEFTCTRL, true)?;
    emit(dev, Key::KEY_C, true)?;
    emit(dev, Key::KEY_C, false)?;
    emit(dev, Key::KEY_LEFTCTRL, false)?;
    // Poll for the clipboard to change rather than reading once after a
    // fixed delay: the game's copy latency varies, and KDE's clipboard
    // manager (Klipper) re-asserts the previous content, so a single
    // timed read lands on the wrong value intermittently. Grab the item
    // text the moment it appears, before anything reverts it.
    let mut item = String::new();
    let deadline = std::time::Instant::now() + Duration::from_millis(1200);
    let mut polls = 0u32;
    while std::time::Instant::now() < deadline {
        sleep(Duration::from_millis(20));
        if let Some(cur) = clipboard_read() {
            if Some(&cur) != saved.as_ref() && !cur.trim().is_empty() {
                item = cur;
                break;
            }
        }
        polls += 1;
    }
    if std::env::var("POE2LENS_DEBUG").is_ok() {
        eprintln!(
            "INJECT saved={} bytes, item={} bytes after {polls} polls",
            saved.as_ref().map(|s| s.len()).unwrap_or(0),
            item.len(),
        );
    }
    // No restore here: the burst's original clipboard is restored by the
    // injector thread once the burst goes idle (see RESTORE_IDLE).
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

/// Restores clipboard content through KDE's Klipper (the persistent
/// clipboard manager) over DBus, so no new transient owner is created.
/// Falls back to wl-copy only when Klipper is absent (non-KDE); on KDE the
/// wl-copy path is exactly what breaks the game's next copy, so it is a
/// last resort.
fn clipboard_restore(text: &str) {
    let via_klipper = Command::new("qdbus6")
        .args(["org.kde.klipper", "/klipper", "setClipboardContents", text])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if via_klipper {
        return;
    }
    use std::io::Write;
    if let Ok(mut child) = Command::new("wl-copy")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        let _ = child.wait();
    }
}
