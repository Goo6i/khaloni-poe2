//! Global hotkeys via RegisterHotKey (the global-hotkey crate). Public API
//! mirrors platform/linux/hotkeys.rs so main.rs calls both identically.

use std::collections::HashMap;
use std::time::Duration;

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, TranslateMessage, MSG,
};

use crate::platform::triggers;
pub use crate::platform::Hotkey;

/// Same contract as the Linux twin: bind the triggers, forward activations
/// to `tx`, never return while listening. Unlike the portal there is no
/// user-approval round-trip; triggers bind exactly as configured, and an
/// unparsable or already-taken trigger is logged and skipped so one bad
/// binding can't kill the rest. Err only when the manager itself can't be
/// created (no hotkeys at all — main reports and runs without them).
pub async fn listen(
    tx: std::sync::mpsc::Sender<Hotkey>,
    price_check: String,
    overlay: String,
    extra: Vec<(String, String)>,
) -> anyhow::Result<()> {
    // RegisterHotKey delivers WM_HOTKEY to the registering thread's message
    // queue (the crate's hidden window), so the manager must be created and
    // pumped on one dedicated thread — the async fn is only a thin wrapper.
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<anyhow::Result<()>>();
    std::thread::Builder::new()
        .name("hotkeys".into())
        .spawn(move || pump(tx, price_check, overlay, extra, ready_tx))?;

    // The handshake is a handful of Win32 calls; spawn_blocking keeps the
    // bounded wait off the async workers, and the timeout only guards
    // against a wedged pump thread so startup can't hang on it.
    tokio::task::spawn_blocking(move || {
        ready_rx
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| anyhow::anyhow!("hotkeys: registration thread died or timed out"))?
    })
    .await??;

    // The pump thread owns the registrations for the process lifetime;
    // mirror the portal listener by never resolving while they are active.
    std::future::pending::<()>().await;
    Ok(())
}

fn pump(
    tx: std::sync::mpsc::Sender<Hotkey>,
    price_check: String,
    overlay: String,
    extra: Vec<(String, String)>,
    ready_tx: std::sync::mpsc::Sender<anyhow::Result<()>>,
) {
    let manager = match GlobalHotKeyManager::new() {
        Ok(m) => m,
        Err(e) => {
            let _ = ready_tx.send(Err(
                anyhow::Error::new(e).context("hotkeys: GlobalHotKeyManager failed")
            ));
            return;
        }
    };

    // Registered hotkey id -> the app-level action it fires.
    let mut actions: HashMap<u32, Hotkey> = HashMap::new();
    let mut bind = |action: Hotkey, trigger: &str| match to_hotkey(trigger) {
        Some(hk) => match manager.register(hk) {
            Ok(()) => {
                actions.insert(hk.id(), action);
            }
            Err(e) => eprintln!("hotkeys: cannot register {trigger:?}: {e}"),
        },
        None => eprintln!("hotkeys: unsupported trigger {trigger:?}, skipped"),
    };
    bind(Hotkey::OverlayToggle, &overlay);
    bind(Hotkey::PriceCheck, &price_check);
    for (id, trigger) in &extra {
        bind(Hotkey::Extra(id.clone()), trigger);
    }
    let _ = ready_tx.send(Ok(()));

    // Classic message pump. The crate's WndProc pushes into its channel
    // during DispatchMessageW, so draining right after each dispatched
    // message never lags and never needs a busy-wait.
    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
            while let Ok(ev) = GlobalHotKeyEvent::receiver().try_recv() {
                // WM_HOTKEY has no key-up; the crate synthesizes Released
                // immediately after Pressed, so forward Pressed only.
                if ev.state() == HotKeyState::Pressed {
                    if let Some(action) = actions.get(&ev.id()) {
                        let _ = tx.send(action.clone());
                    }
                }
            }
        }
    }
}

/// None = the trigger fails to parse or its key has no RegisterHotKey
/// mapping; the caller logs and skips it.
fn to_hotkey(trigger: &str) -> Option<HotKey> {
    let t = triggers::parse(trigger)?;
    let code = to_code(&t.key)?;
    let mut mods = Modifiers::empty();
    if t.ctrl {
        mods |= Modifiers::CONTROL;
    }
    if t.alt {
        mods |= Modifiers::ALT;
    }
    if t.shift {
        mods |= Modifiers::SHIFT;
    }
    Some(HotKey::new((!mods.is_empty()).then_some(mods), code))
}

/// Keys the config UI offers today: F1-F12, digits, letters. `key` arrives
/// uppercased from triggers::parse.
fn to_code(key: &str) -> Option<Code> {
    use Code::*;
    Some(match key {
        "F1" => F1,
        "F2" => F2,
        "F3" => F3,
        "F4" => F4,
        "F5" => F5,
        "F6" => F6,
        "F7" => F7,
        "F8" => F8,
        "F9" => F9,
        "F10" => F10,
        "F11" => F11,
        "F12" => F12,
        "0" => Digit0,
        "1" => Digit1,
        "2" => Digit2,
        "3" => Digit3,
        "4" => Digit4,
        "5" => Digit5,
        "6" => Digit6,
        "7" => Digit7,
        "8" => Digit8,
        "9" => Digit9,
        "A" => KeyA,
        "B" => KeyB,
        "C" => KeyC,
        "D" => KeyD,
        "E" => KeyE,
        "F" => KeyF,
        "G" => KeyG,
        "H" => KeyH,
        "I" => KeyI,
        "J" => KeyJ,
        "K" => KeyK,
        "L" => KeyL,
        "M" => KeyM,
        "N" => KeyN,
        "O" => KeyO,
        "P" => KeyP,
        "Q" => KeyQ,
        "R" => KeyR,
        "S" => KeyS,
        "T" => KeyT,
        "U" => KeyU,
        "V" => KeyV,
        "W" => KeyW,
        "X" => KeyX,
        "Y" => KeyY,
        "Z" => KeyZ,
        _ => return None,
    })
}
