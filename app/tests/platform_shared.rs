//! The platform-neutral pieces the main loop builds on: startup state from
//! the game-window feed, the host-tool environment, hotkey re-binding.

use std::time::Duration;

use khaloni_poe2::config::Rect;
use khaloni_poe2::platform::{
    Focus, GameWindowEvent, GameWindowState, HotkeyBindings, HotkeyRebind,
};

#[test]
fn startup_state_assumes_nothing_about_the_game() {
    let s = GameWindowState::default();
    assert_eq!(s.focus, Focus::Other);
    assert!(!s.present());
    assert!(!s.visible);
    assert!(!s.game_focused());
}

#[test]
fn events_received_while_waiting_for_geometry_are_kept() {
    let (tx, rx) = std::sync::mpsc::channel();
    let rect = Rect { x: 2560, y: 0, w: 2560, h: 1440 };
    // The feed's startup burst, with the geometry in the middle of it.
    tx.send(GameWindowEvent::Active(Focus::Game)).unwrap();
    tx.send(GameWindowEvent::Cursor(3000, 500)).unwrap();
    tx.send(GameWindowEvent::Geometry(rect)).unwrap();
    tx.send(GameWindowEvent::Visible(true)).unwrap();
    let s = GameWindowState::wait_for_geometry(&rx, Duration::from_secs(5));
    assert_eq!(s.rect, Some(rect));
    assert_eq!(s.focus, Focus::Game);
    assert!(s.visible);
    assert_eq!(s.cursor, Some((3000, 500)));
    assert!(rx.try_recv().is_err(), "nothing is left behind or dropped");
}

#[test]
fn no_game_at_startup_is_not_a_focused_visible_game() {
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(GameWindowEvent::Active(Focus::Other)).unwrap();
    tx.send(GameWindowEvent::Visible(false)).unwrap();
    let s = GameWindowState::wait_for_geometry(&rx, Duration::from_millis(50));
    assert_eq!(s, GameWindowState { cursor: None, ..GameWindowState::default() });
}

#[test]
fn game_gone_clears_presence_and_visibility() {
    let mut s = GameWindowState::default();
    s.apply(GameWindowEvent::Geometry(Rect { x: 0, y: 0, w: 10, h: 10 }));
    s.apply(GameWindowEvent::Visible(true));
    s.apply(GameWindowEvent::GameGone);
    assert!(!s.present());
    assert!(!s.visible);
}

/// The host-tool environment changes for `env`; the Steam runtime it undoes
/// exists on Linux only.
#[cfg(not(windows))]
fn changes(env: &[(&str, &str)]) -> std::collections::HashMap<&'static str, Option<String>> {
    let env: std::collections::HashMap<String, String> =
        env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    khaloni_poe2::platform::host_env_changes(|k| env.get(k).cloned()).into_iter().collect()
}

#[cfg(not(windows))]
#[test]
fn outside_steam_the_environment_is_left_alone() {
    assert!(changes(&[("PATH", "/usr/bin"), ("LD_LIBRARY_PATH", "/opt/mine")]).is_empty());
    assert!(changes(&[("LD_PRELOAD", "/usr/lib/libjemalloc.so")]).is_empty());
}

#[cfg(not(windows))]
#[test]
fn steam_runtime_environment_is_undone_for_host_tools() {
    let c = changes(&[
        ("STEAM_RUNTIME", "/home/u/.steam/ubuntu12_32/steam-runtime"),
        ("LD_PRELOAD", "/home/u/.steam/ubuntu12_32/gameoverlayrenderer.so:/home/u/.steam/ubuntu12_64/gameoverlayrenderer.so"),
        ("LD_LIBRARY_PATH", "/home/u/.steam/ubuntu12_32/steam-runtime/pinned_libs_64:/usr/lib"),
        ("SYSTEM_LD_LIBRARY_PATH", "/opt/cuda/lib"),
        ("PATH", "/home/u/.steam/ubuntu12_32/steam-runtime/amd64/usr/bin:/usr/bin"),
        ("SYSTEM_PATH", "/usr/local/bin:/usr/bin"),
    ]);
    assert_eq!(c["LD_PRELOAD"], None);
    assert_eq!(c["LD_LIBRARY_PATH"], Some("/opt/cuda/lib".to_string()));
    assert_eq!(c["PATH"], Some("/usr/local/bin:/usr/bin".to_string()));
}

#[cfg(not(windows))]
#[test]
fn only_the_steam_overlay_leaves_ld_preload() {
    // No SYSTEM_* saved: the runtime's library path is dropped, PATH kept.
    let c = changes(&[
        ("STEAM_RUNTIME", "/rt"),
        ("LD_PRELOAD", "/usr/lib/libmangohud.so /rt/gameoverlayrenderer.so"),
        ("LD_LIBRARY_PATH", "/rt/pinned_libs_64"),
    ]);
    assert_eq!(c["LD_PRELOAD"], Some("/usr/lib/libmangohud.so".to_string()));
    assert_eq!(c["LD_LIBRARY_PATH"], None);
    assert!(!c.contains_key("PATH"));
    // The overlay preloaded without the classic runtime (container runtime).
    let c = changes(&[("LD_PRELOAD", "/rt/gameoverlayrenderer.so"), ("LD_LIBRARY_PATH", "/mine")]);
    assert_eq!(c["LD_PRELOAD"], None);
    assert!(!c.contains_key("LD_LIBRARY_PATH"));
}

#[test]
fn a_rebind_wakes_the_listener_with_the_latest_bindings() {
    let rebind = HotkeyRebind::new();
    let theirs = rebind.clone();
    let set = |key: &str| HotkeyBindings { price_check: key.to_string(), ..Default::default() };
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(1).build().unwrap();
    let waiter = rt.spawn(async move { theirs.next().await });
    std::thread::sleep(Duration::from_millis(50));
    rebind.rebind(set("F7"));
    assert_eq!(rt.block_on(waiter).unwrap(), set("F7"));
    // Two changes before anyone listens: only the last one matters.
    rebind.rebind(set("F8"));
    rebind.rebind(set("F9"));
    assert_eq!(rt.block_on(rebind.next()), set("F9"));
}
