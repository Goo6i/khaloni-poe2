//! Ctrl+C injection and clipboard read for the hover price check.
//!
//! Wayland compositors do not let a client synthesize keyboard input into
//! another window, so this goes through a virtual keyboard registered with
//! the kernel's uinput driver instead: the compositor sees it as a real
//! keyboard and delivers the Ctrl+C to whatever window is focused, exactly
//! like the game's own "copy item to clipboard" hover shortcut. Proven
//! working at milestone 0 (see spikes/src/bin/inject.rs).
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
//!
//! Verify with `ls -l /dev/uinput` (expect `root input rw-rw----`) and `id`
//! (expect `input` listed).

use std::{process::Command, thread::sleep, time::Duration};

use evdev::{uinput::VirtualDeviceBuilder, AttributeSet, EventType, InputEvent, Key};

/// Persistent virtual keyboard for Ctrl+C injection. Creating the device is
/// slow (compositor registration), so it is built once at startup and kept
/// for the lifetime of the app rather than recreated per hover trigger.
pub struct Injector {
    dev: evdev::uinput::VirtualDevice,
}

impl Injector {
    pub fn new() -> anyhow::Result<Injector> {
        let mut keys = AttributeSet::<Key>::new();
        keys.insert(Key::KEY_LEFTCTRL);
        keys.insert(Key::KEY_C);
        let dev = VirtualDeviceBuilder::new()?
            .name("poe2-lens-kbd")
            .with_keys(&keys)?
            .build()?;
        // Give the compositor a moment to register the device once.
        sleep(Duration::from_millis(700));
        Ok(Injector { dev })
    }

    fn key(&mut self, k: Key, down: bool) -> anyhow::Result<()> {
        self.dev
            .emit(&[InputEvent::new(EventType::KEY, k.code(), down as i32)])?;
        sleep(Duration::from_millis(20));
        Ok(())
    }

    /// Inject Ctrl+C and return the clipboard text. The game replaces the
    /// clipboard with the hovered item's text; if nothing is hovered the
    /// clipboard keeps its previous content, so the caller clears it first.
    pub fn copy_hovered_item(&mut self) -> anyhow::Result<String> {
        // Clear the clipboard so a no-item hover is detectable.
        let _ = Command::new("wl-copy").arg("--clear").status();
        self.key(Key::KEY_LEFTCTRL, true)?;
        self.key(Key::KEY_C, true)?;
        self.key(Key::KEY_C, false)?;
        self.key(Key::KEY_LEFTCTRL, false)?;
        sleep(Duration::from_millis(250));
        let out = Command::new("wl-paste").arg("-n").output()?;
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}
