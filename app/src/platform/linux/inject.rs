//! Item-copy injection and clipboard read for the hover price check. The
//! chord is Ctrl+Alt+C, or plain Ctrl+C by setting (`platform::chord`);
//! "Ctrl+C" below stands for whichever is sent.
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
//! echo 'KERNEL=="uinput", GROUP="input", MODE="0660"' | sudo tee /etc/udev/rules.d/99-khaloni-poe2-uinput.rules
//! sudo udevadm control --reload-rules && sudo udevadm trigger /dev/uinput
//! # then log out and back in for the new group membership to take effect
//! ```

use std::{
    thread::sleep,
    time::{Duration, Instant},
};

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
    /// Copy the hovered item. A non-zero u64 marks a hotkey that holds a
    /// modifier (chat-style CTRL+N actions): the copy waits for its release,
    /// or the held Ctrl collides with the injected Ctrl+C and the game does
    /// not copy. The value is the delay used when keyboards are unreadable.
    /// The bool picks the advanced chord (Ctrl+Alt+C) over plain Ctrl+C.
    Copy(std::sync::mpsc::Sender<anyhow::Result<String>>, u64, bool),
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
                    InjectReq::Copy(reply, pre_delay, advanced) => {
                        // The caller holds an in-flight flag until this
                        // reply arrives, so every request is answered, a
                        // panic included, or the price-check key stays dead
                        // for the rest of the session.
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            copy_hovered(&mut dev, pre_delay, advanced, watcher.as_ref())
                        }))
                        .unwrap_or_else(|_| Err(anyhow::anyhow!("price check failed unexpectedly")));
                        let _ = reply.send(result);
                    }
                    InjectReq::Type(msg, delay) => {
                        let typed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            type_text(&mut dev, &msg, delay)
                        }));
                        match typed {
                            Ok(Ok(())) => {}
                            Ok(Err(e)) => eprintln!("macro type failed: {e}"),
                            Err(_) => eprintln!("macro type failed unexpectedly"),
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
    /// on `reply`, always exactly once. A non-zero `pre_delay_ms` marks a
    /// hotkey with a modifier: the copy waits until the modifiers are
    /// physically released (or, when the keyboards cannot be read, for this
    /// many ms). Pass 0 for a plain-key hotkey. `advanced` is the config's
    /// `advanced_copy`: Ctrl+Alt+C, or plain Ctrl+C when false.
    pub fn submit(&self, reply: std::sync::mpsc::Sender<anyhow::Result<String>>, pre_delay_ms: u64, advanced: bool) {
        if let Err(std::sync::mpsc::SendError(InjectReq::Copy(reply, _, _))) =
            self.req_tx.send(InjectReq::Copy(reply, pre_delay_ms, advanced))
        {
            let _ = reply.send(Err(anyhow::anyhow!("injector thread is gone")));
        }
    }

    /// Queues a chat macro: waits for held modifier keys to be released,
    /// opens chat, waits `open_delay_ms` for the chat box to be ready, types
    /// `msg`, and sends it. Non-blocking.
    pub fn type_text(&self, msg: String, open_delay_ms: u64) {
        let _ = self.req_tx.send(InjectReq::Type(msg, open_delay_ms));
    }
}

/// True when clipboard text is a copied PoE item (English client).
fn is_poe_item(text: &str) -> bool {
    text.starts_with("Item Class: ") || text.starts_with("Rarity: ")
}

/// Why a clipboard read produced no text. Carried inside the
/// `anyhow::Error` a copy request answers with, so the caller can tell a
/// missing tool from a hung clipboard (`err.downcast_ref::<ClipError>()`)
/// or just show the message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipError {
    /// `wl-paste` is not installed.
    Missing,
    /// `wl-paste` did not finish in time: the application holding the
    /// clipboard is not answering.
    TimedOut,
    Failed(String),
}

impl std::fmt::Display for ClipError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClipError::Missing => {
                write!(f, "wl-paste not found: install wl-clipboard to price-check items")
            }
            ClipError::TimedOut => {
                write!(f, "clipboard read timed out: the application holding the clipboard is not answering")
            }
            ClipError::Failed(why) => write!(f, "clipboard read failed: {why}"),
        }
    }
}

impl std::error::Error for ClipError {}

/// What `await_copy` needs from the outside world, so its decisions run
/// against a scripted clipboard in the tests.
trait CopyEnv {
    /// Current clipboard text; `Ok(None)` when empty or not text.
    fn read(&mut self) -> Result<Option<String>, ClipError>;
    /// Whether CLIPBOARD ownership changed since the Ctrl+C, waiting up to
    /// `wait` for it. `None` when ownership cannot be observed.
    fn ownership_changed(&mut self, wait: Duration) -> Option<bool>;
    fn sleep(&mut self, d: Duration);
    fn now(&self) -> Instant;
}

struct LiveEnv<'a> {
    watcher: Option<&'a crate::platform::clipwatch::ClipboardWatcher>,
}

impl CopyEnv for LiveEnv<'_> {
    fn read(&mut self) -> Result<Option<String>, ClipError> {
        clipboard_read()
    }
    fn ownership_changed(&mut self, wait: Duration) -> Option<bool> {
        self.watcher.map(|w| w.wait_for_change(wait))
    }
    fn sleep(&mut self, d: Duration) {
        sleep(d);
    }
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// How long a changed clipboard is polled for after the Ctrl+C.
const CHANGE_WINDOW: Duration = Duration::from_millis(500);
/// Grace for the ownership event after that window.
const OWNERSHIP_GRACE: Duration = Duration::from_millis(150);
/// After an ownership event, how long the content may lag behind it.
const SETTLE_WINDOW: Duration = Duration::from_millis(300);
const POLL: Duration = Duration::from_millis(40);

/// Polls until `window` has passed for clipboard text that is a PoE item
/// and differs from `before`. Returns it, or the last thing read.
fn poll_for_new_item(
    env: &mut dyn CopyEnv,
    before: Option<&str>,
    window: Duration,
    timed_out: &mut bool,
) -> Result<(Option<String>, Option<String>), ClipError> {
    let deadline = env.now() + window;
    let mut last = None;
    while env.now() < deadline {
        env.sleep(POLL);
        match env.read() {
            Ok(Some(cur)) => {
                if is_poe_item(&cur) && Some(cur.as_str()) != before {
                    return Ok((Some(cur), None));
                }
                last = Some(cur);
            }
            Ok(None) => last = None,
            Err(ClipError::TimedOut) => *timed_out = true,
            Err(e) => return Err(e),
        }
    }
    Ok((None, last))
}

/// Decides what the injected Ctrl+C copied, given the clipboard text from
/// just before it. Empty string = nothing hovered.
///
/// A NEW hovered item makes the content differ from `before`; that is the
/// fast path. A re-check of the SAME item and a misclick over empty space
/// both leave the content byte-identical, and only the X11 ownership event
/// tells them apart: the game re-asserts ownership on every copy. Without
/// an ownership watcher the two cannot be told apart, and the answer is
/// "nothing hovered": showing the previous item's price under a different
/// item is worse than asking for a second press.
fn await_copy(env: &mut dyn CopyEnv, before: Option<&str>) -> Result<String, ClipError> {
    let mut timed_out = false;
    let (found, _) = poll_for_new_item(env, before, CHANGE_WINDOW, &mut timed_out)?;
    if let Some(item) = found {
        return Ok(item);
    }
    // The ownership check comes AFTER the window (not in a tight loop right
    // after Ctrl+C): by now the game's copy event, if any, has arrived on
    // the socket and the first poll reads it. A tight poll started
    // immediately races the event and misses it (x11rb reads non-blocking
    // per call).
    let nothing = |timed_out: bool| if timed_out { Err(ClipError::TimedOut) } else { Ok(String::new()) };
    match env.ownership_changed(OWNERSHIP_GRACE) {
        None | Some(false) => nothing(timed_out),
        Some(true) => {
            // The game copied. Ownership changes before the new text is
            // readable through XWayland's bridge, so a single read here can
            // still return the PREVIOUS item; give the content a bounded
            // chance to change before concluding it is the same item again.
            let (found, last) = poll_for_new_item(env, before, SETTLE_WINDOW, &mut timed_out)?;
            match found.or(last) {
                Some(text) if is_poe_item(&text) => Ok(text),
                _ => nothing(timed_out),
            }
        }
    }
}

/// Injects Ctrl+C and returns the item text the game copies (empty when
/// nothing is hovered). Deliberately never writes the clipboard: under
/// wine/Proton the game will not copy over a clipboard an external process
/// just set (a primed sentinel blocks the copy entirely, verified live), so
/// we only read. Runs only on the injector thread.
fn copy_hovered(
    dev: &mut evdev::uinput::VirtualDevice,
    pre_delay_ms: u64,
    advanced: bool,
    watcher: Option<&crate::platform::clipwatch::ClipboardWatcher>,
) -> anyhow::Result<String> {
    // A hotkey with a modifier (CTRL+N actions) is still physically held
    // when the request arrives; the held Ctrl collides with the injected
    // Ctrl+C and the game copies nothing. F7 passes 0: nothing to clear.
    if pre_delay_ms > 0 && !wait_for_modifier_release(Duration::from_millis(pre_delay_ms)) {
        anyhow::bail!("release the hotkey's modifier keys to copy the item");
    }

    // Clear stale ownership events before we trigger the copy.
    if let Some(w) = watcher {
        w.drain();
    }
    let before = match clipboard_read() {
        Ok(text) => text,
        // No point pressing keys in the game when the result cannot be read.
        Err(ClipError::Missing) => return Err(ClipError::Missing.into()),
        // A hung or failing owner is replaced by the game's copy; any item
        // read afterwards is new.
        Err(_) => None,
    };
    // Every key is up again before the clipboard is waited on, whether the
    // chord went through or not: no later failure can leave one down.
    crate::platform::chord::press_copy_chord(advanced, &mut |key, down| emit(dev, chord_key(key), down))?;

    Ok(await_copy(&mut LiveEnv { watcher }, before.as_deref())?)
}

fn chord_key(key: crate::platform::chord::ChordKey) -> Key {
    use crate::platform::chord::ChordKey;
    match key {
        ChordKey::Ctrl => Key::KEY_LEFTCTRL,
        ChordKey::Alt => Key::KEY_LEFTALT,
        ChordKey::C => Key::KEY_C,
    }
}

/// One `wl-paste` may take this long. A healthy read is a few tens of ms;
/// a selection owner that stopped answering would otherwise hold the
/// injector thread, and every later price check, forever.
const CLIPBOARD_READ_TIMEOUT: Duration = Duration::from_millis(1000);
/// Item text is a few KB. Reading stops here so a huge non-item clipboard
/// (an image, a file) costs neither memory nor time.
const CLIPBOARD_READ_CAP: u64 = 256 * 1024;

/// Reads the Wayland clipboard via `wl-paste`, retrying once because it can
/// return empty transiently under game load. Same-item re-checks (where the
/// content, and so the mirror, does not change) are disambiguated not here
/// but by the X11 ownership probe in `clipwatch` — see `await_copy`.
fn clipboard_read() -> Result<Option<String>, ClipError> {
    for _ in 0..2 {
        let mut cmd = crate::platform::host_command("wl-paste");
        cmd.arg("-n");
        if let Some(bytes) = run_with_deadline(cmd, CLIPBOARD_READ_TIMEOUT, CLIPBOARD_READ_CAP)? {
            if !bytes.is_empty() {
                return Ok(Some(String::from_utf8_lossy(&bytes).into_owned()));
            }
        }
    }
    Ok(None)
}

/// Runs `cmd` to completion within `timeout` and returns its stdout (up to
/// `cap` bytes), or `None` when it exited unsuccessfully. The child is
/// killed at the deadline. stdout is drained on a helper thread so a child
/// blocked on a full pipe cannot outlive the deadline either.
fn run_with_deadline(
    mut cmd: std::process::Command,
    timeout: Duration,
    cap: u64,
) -> Result<Option<Vec<u8>>, ClipError> {
    use std::io::Read;
    use std::process::Stdio;
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => ClipError::Missing,
            _ => ClipError::Failed(e.to_string()),
        })?;
    let deadline = Instant::now() + timeout;
    let stdout = child.stdout.take();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(out) = stdout {
            let _ = out.take(cap).read_to_end(&mut buf);
        }
        // The pipe closes here; a child still writing past the cap gets
        // SIGPIPE and exits instead of blocking.
        let _ = tx.send(buf);
    });
    let reap = |child: &mut std::process::Child| {
        let _ = child.kill();
        let _ = child.wait();
    };
    let bytes = match rx.recv_timeout(timeout) {
        Ok(b) => b,
        Err(_) => {
            reap(&mut child);
            return Err(ClipError::TimedOut);
        }
    };
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status.success().then_some(bytes)),
            Ok(None) if Instant::now() < deadline => sleep(Duration::from_millis(2)),
            Ok(None) => {
                reap(&mut child);
                return Err(ClipError::TimedOut);
            }
            Err(e) => {
                reap(&mut child);
                return Err(ClipError::Failed(e.to_string()));
            }
        }
    }
}

/// Modifier keys that change what an injected key means to the game.
const MODIFIERS: [Key; 8] = [
    Key::KEY_LEFTCTRL,
    Key::KEY_RIGHTCTRL,
    Key::KEY_LEFTSHIFT,
    Key::KEY_RIGHTSHIFT,
    Key::KEY_LEFTALT,
    Key::KEY_RIGHTALT,
    Key::KEY_LEFTMETA,
    Key::KEY_RIGHTMETA,
];
/// Longest wait for the user to let go of a hotkey's modifiers.
const MODIFIER_RELEASE_MAX: Duration = Duration::from_millis(1500);
/// After the release is seen here, the compositor and the game still have
/// to see it too.
const MODIFIER_RELEASE_SETTLE: Duration = Duration::from_millis(40);

/// Waits until no modifier is physically held, up to `max`. `held` reports
/// the current state, `None` when it cannot be read; then `fallback` is
/// slept instead, the fixed delay this replaces. False = still held at the
/// deadline. A key pressed on one keyboard cannot be released from
/// another (the kernel drops a release for a key that device never
/// pressed, and the compositor counts presses per key), so waiting is the
/// only way to get a clean modifier state.
fn wait_released(
    held: &mut dyn FnMut() -> Option<bool>,
    sleep_for: &mut dyn FnMut(Duration),
    fallback: Duration,
    max: Duration,
) -> bool {
    let step = Duration::from_millis(15);
    let mut waited = Duration::ZERO;
    let mut was_held = false;
    loop {
        match held() {
            None => {
                sleep_for(fallback.saturating_sub(waited));
                return true;
            }
            Some(false) => {
                if was_held {
                    sleep_for(MODIFIER_RELEASE_SETTLE);
                }
                return true;
            }
            Some(true) if waited >= max => return false,
            Some(true) => {
                was_held = true;
                sleep_for(step);
                waited += step;
            }
        }
    }
}

/// `wait_released` against the real keyboards: every evdev device with a
/// Ctrl key, read through the `input` group membership the uinput setup
/// already requires. Our own virtual keyboard is left out by name, so the
/// Ctrl and Alt the copy chord presses there never count as "still held";
/// the wait also runs before the chord, on the one injector thread, and the
/// chord releases its keys before it returns.
fn wait_for_modifier_release(fallback: Duration) -> bool {
    let keyboards: Vec<evdev::Device> = evdev::enumerate()
        .map(|(_, dev)| dev)
        .filter(|dev| dev.name() != Some(DEVICE_NAME))
        .filter(|dev| dev.supported_keys().is_some_and(|k| k.contains(Key::KEY_LEFTCTRL)))
        .collect();
    let mut held = || {
        let mut readable = false;
        for dev in &keyboards {
            if let Ok(state) = dev.get_key_state() {
                readable = true;
                if MODIFIERS.iter().any(|m| state.contains(*m)) {
                    return Some(true);
                }
            }
        }
        readable.then_some(false)
    };
    wait_released(&mut held, &mut |d| sleep(d), fallback, MODIFIER_RELEASE_MAX)
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
    // A macro bound to CTRL+1 arrives with Ctrl still down, and the game
    // reads Ctrl+Enter as "reply to the last whisper": the message would go
    // to another player instead of local chat. Shift and Alt are no better
    // (Alt+Enter toggles fullscreen). Nothing is typed until every
    // modifier is up, and a macro whose modifiers never come up is dropped.
    if !wait_for_modifier_release(TYPE_FALLBACK_DELAY) {
        anyhow::bail!("a modifier key is still held; macro not sent");
    }
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

/// The delay a macro gets when the keyboards cannot be read: the same one
/// modifier-bound copy actions use.
const TYPE_FALLBACK_DELAY: Duration = Duration::from_millis(300);

const DEVICE_NAME: &str = "khaloni-poe2-kbd";

fn build_device() -> anyhow::Result<evdev::uinput::VirtualDevice> {
    let mut keys = AttributeSet::<Key>::new();
    keys.insert(Key::KEY_LEFTCTRL);
    // A key the device does not declare is dropped by the kernel: without
    // this the advanced chord would arrive as plain Ctrl+C.
    keys.insert(Key::KEY_LEFTALT);
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
        .name(DEVICE_NAME)
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

    const OLD: &str = "Item Class: Rings\nRarity: Rare\nOld Ring";
    const NEW: &str = "Item Class: Belts\nRarity: Rare\nNew Belt";

    /// A clipboard whose content is a function of elapsed time since the
    /// Ctrl+C, on a clock that only `sleep` advances.
    struct Scripted {
        base: Instant,
        elapsed: Duration,
        content: Box<dyn Fn(Duration) -> Result<Option<String>, ClipError>>,
        /// When the ownership event arrives; `None` = never. Outer `None` =
        /// no watcher at all.
        ownership: Option<Option<Duration>>,
    }

    impl Scripted {
        fn new(
            ownership: Option<Option<Duration>>,
            content: impl Fn(Duration) -> Result<Option<String>, ClipError> + 'static,
        ) -> Scripted {
            Scripted { base: Instant::now(), elapsed: Duration::ZERO, content: Box::new(content), ownership }
        }
    }

    impl CopyEnv for Scripted {
        fn read(&mut self) -> Result<Option<String>, ClipError> {
            (self.content)(self.elapsed)
        }
        fn ownership_changed(&mut self, wait: Duration) -> Option<bool> {
            let at = self.ownership?;
            let seen = at.is_some_and(|t| t <= self.elapsed + wait);
            self.elapsed += if seen { Duration::ZERO } else { wait };
            Some(seen)
        }
        fn sleep(&mut self, d: Duration) {
            self.elapsed += d;
        }
        fn now(&self) -> Instant {
            self.base + self.elapsed
        }
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn a_new_item_is_returned_as_soon_as_it_appears() {
        let mut env = Scripted::new(Some(Some(ms(90))), |t| {
            Ok(Some(if t < ms(100) { OLD } else { NEW }.to_string()))
        });
        assert_eq!(await_copy(&mut env, Some(OLD)).unwrap(), NEW);
        assert!(env.elapsed < ms(200), "fast path must not wait out the window");
    }

    #[test]
    fn a_late_copy_is_not_priced_as_the_previous_item() {
        // Ownership flips inside the first window but the new text only
        // becomes readable after it: the old item must not be returned.
        let mut env = Scripted::new(Some(Some(ms(480))), |t| {
            Ok(Some(if t < ms(640) { OLD } else { NEW }.to_string()))
        });
        assert_eq!(await_copy(&mut env, Some(OLD)).unwrap(), NEW);
    }

    #[test]
    fn the_same_item_copied_again_is_a_recheck() {
        let mut env = Scripted::new(Some(Some(ms(60))), |_| Ok(Some(OLD.to_string())));
        assert_eq!(await_copy(&mut env, Some(OLD)).unwrap(), OLD);
    }

    #[test]
    fn no_ownership_event_means_nothing_hovered() {
        let mut env = Scripted::new(Some(None), |_| Ok(Some(OLD.to_string())));
        assert_eq!(await_copy(&mut env, Some(OLD)).unwrap(), "");
    }

    #[test]
    fn without_a_watcher_unchanged_content_is_no_item() {
        let mut env = Scripted::new(None, |_| Ok(Some(OLD.to_string())));
        assert_eq!(await_copy(&mut env, Some(OLD)).unwrap(), "");
        // A changed clipboard still works without the watcher.
        let mut env = Scripted::new(None, |_| Ok(Some(NEW.to_string())));
        assert_eq!(await_copy(&mut env, Some(OLD)).unwrap(), NEW);
    }

    #[test]
    fn non_item_text_is_never_returned() {
        let mut env = Scripted::new(Some(Some(ms(50))), |_| Ok(Some("https://example.org".to_string())));
        assert_eq!(await_copy(&mut env, Some(OLD)).unwrap(), "");
    }

    #[test]
    fn a_hung_clipboard_is_reported_and_a_missing_tool_stops_at_once() {
        let mut env = Scripted::new(Some(None), |_| Err(ClipError::TimedOut));
        assert_eq!(await_copy(&mut env, None), Err(ClipError::TimedOut));
        let mut env = Scripted::new(Some(None), |_| Err(ClipError::Missing));
        assert_eq!(await_copy(&mut env, None), Err(ClipError::Missing));
        assert!(env.elapsed < ms(100));
    }

    fn sh(script: &str) -> std::process::Command {
        let mut c = std::process::Command::new("sh");
        c.args(["-c", script]);
        c
    }

    #[test]
    fn a_child_that_hangs_is_killed_at_the_deadline() {
        let t0 = Instant::now();
        assert_eq!(run_with_deadline(sh("sleep 30"), ms(200), 1024), Err(ClipError::TimedOut));
        assert!(t0.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn child_output_status_and_absence_are_told_apart() {
        assert_eq!(run_with_deadline(sh("printf hello"), ms(5000), 1024), Ok(Some(b"hello".to_vec())));
        assert_eq!(run_with_deadline(sh("exit 1"), ms(5000), 1024), Ok(None));
        let missing = std::process::Command::new("khaloni-poe2-no-such-program");
        assert_eq!(run_with_deadline(missing, ms(5000), 1024), Err(ClipError::Missing));
        // Output past the cap is cut, and the writer does not block us.
        // A writer far past the cap is cut off (SIGPIPE) instead of being
        // waited on: not an item, and not a timeout.
        let t0 = Instant::now();
        assert_eq!(run_with_deadline(sh("head -c 5000000 /dev/zero"), ms(5000), 1024), Ok(None));
        assert!(t0.elapsed() < Duration::from_secs(4));
    }

    #[test]
    fn typing_waits_for_the_modifiers_to_come_up() {
        // Held for three polls, then released: waits, settles, proceeds.
        let mut polls = 0;
        let mut slept = Duration::ZERO;
        let ok = wait_released(
            &mut || {
                polls += 1;
                Some(polls <= 3)
            },
            &mut |d| slept += d,
            ms(300),
            ms(1500),
        );
        assert!(ok);
        assert_eq!(slept, ms(45) + MODIFIER_RELEASE_SETTLE);

        // Nothing held: no delay at all.
        let mut slept = Duration::ZERO;
        assert!(wait_released(&mut || Some(false), &mut |d| slept += d, ms(300), ms(1500)));
        assert_eq!(slept, Duration::ZERO);

        // Never released: refuse rather than send Ctrl+Enter.
        let mut slept = Duration::ZERO;
        assert!(!wait_released(&mut || Some(true), &mut |d| slept += d, ms(300), ms(1500)));
        assert_eq!(slept, ms(1500));

        // Keyboards unreadable: the fixed delay.
        let mut slept = Duration::ZERO;
        assert!(wait_released(&mut || None, &mut |d| slept += d, ms(300), ms(1500)));
        assert_eq!(slept, ms(300));
    }
}
