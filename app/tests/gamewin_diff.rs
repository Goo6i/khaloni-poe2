//! Tests for the polled game-window diff state machine (the pure half of
//! the Windows gamewin backend, so it runs on Linux CI). Contract mirrors
//! the KWin script in platform/linux/gamewin.rs.

use poe2_lens::config::Rect;
use poe2_lens::platform::gamewin_diff::{DiffState, WindowSample};
use poe2_lens::platform::GameWindowEvent;

fn rect(x: i32, y: i32, w: u32, h: u32) -> Rect {
    Rect { x, y, w, h }
}

fn sample(rect: Option<Rect>, focused: bool, cursor: (i32, i32)) -> WindowSample {
    WindowSample { rect, focused, cursor }
}

#[test]
fn first_sample_emits_geometry_and_active() {
    let mut st = DiffState::new();
    let evs = st.diff(&sample(Some(rect(10, 20, 800, 600)), true, (0, 0)));
    // Geometry with the sampled rect, initial focus state, and the initial
    // cursor position (the KWin script's first timer tick does the same).
    assert_eq!(evs.len(), 3);
    assert!(matches!(
        evs[0],
        GameWindowEvent::Geometry(Rect { x: 10, y: 20, w: 800, h: 600 })
    ));
    assert!(matches!(evs[1], GameWindowEvent::Active(true)));
    assert!(matches!(evs[2], GameWindowEvent::Cursor(0, 0)));
}

#[test]
fn first_sample_reports_unfocused_state_too() {
    // The initial Active must fire even when false: the main loop gates
    // hotkeys on focus and must not assume the game starts focused.
    let mut st = DiffState::new();
    let evs = st.diff(&sample(Some(rect(0, 0, 100, 100)), false, (5, 5)));
    assert!(evs.iter().any(|e| matches!(e, GameWindowEvent::Active(false))));
}

#[test]
fn unchanged_sample_emits_nothing() {
    let mut st = DiffState::new();
    let s = sample(Some(rect(10, 20, 800, 600)), true, (50, 50));
    st.diff(&s);
    assert!(st.diff(&s).is_empty());
    assert!(st.diff(&s).is_empty());
}

#[test]
fn rect_change_emits_geometry_only() {
    let mut st = DiffState::new();
    st.diff(&sample(Some(rect(10, 20, 800, 600)), true, (50, 50)));
    let evs = st.diff(&sample(Some(rect(0, 0, 1920, 1080)), true, (50, 50)));
    assert_eq!(evs.len(), 1);
    assert!(matches!(
        evs[0],
        GameWindowEvent::Geometry(Rect { x: 0, y: 0, w: 1920, h: 1080 })
    ));
}

#[test]
fn focus_flip_emits_active_only() {
    let mut st = DiffState::new();
    st.diff(&sample(Some(rect(10, 20, 800, 600)), true, (50, 50)));
    let evs = st.diff(&sample(Some(rect(10, 20, 800, 600)), false, (50, 50)));
    assert_eq!(evs.len(), 1);
    assert!(matches!(evs[0], GameWindowEvent::Active(false)));
    let evs = st.diff(&sample(Some(rect(10, 20, 800, 600)), true, (50, 50)));
    assert_eq!(evs.len(), 1);
    assert!(matches!(evs[0], GameWindowEvent::Active(true)));
}

#[test]
fn sub_4px_cursor_moves_are_swallowed() {
    let mut st = DiffState::new();
    st.diff(&sample(Some(rect(10, 20, 800, 600)), true, (100, 100)));
    // Jitter within the 4px guard on both axes: no Cursor events.
    for c in [(101, 100), (103, 103), (100, 104), (96, 100), (104, 96)] {
        let evs = st.diff(&sample(Some(rect(10, 20, 800, 600)), true, c));
        assert!(evs.is_empty(), "cursor jitter to {c:?} leaked an event");
    }
    // A real move (>4px on one axis) gets through, measured from the last
    // *emitted* position (100,100), not the last jitter sample.
    let evs = st.diff(&sample(Some(rect(10, 20, 800, 600)), true, (105, 100)));
    assert_eq!(evs.len(), 1);
    assert!(matches!(evs[0], GameWindowEvent::Cursor(105, 100)));
}

#[test]
fn window_disappearing_emits_game_gone_exactly_once() {
    let mut st = DiffState::new();
    st.diff(&sample(Some(rect(10, 20, 800, 600)), true, (50, 50)));
    let evs = st.diff(&sample(None, false, (50, 50)));
    let gone = evs.iter().filter(|e| matches!(e, GameWindowEvent::GameGone)).count();
    assert_eq!(gone, 1);
    // The same tick also reports the focus loss.
    assert!(evs.iter().any(|e| matches!(e, GameWindowEvent::Active(false))));
    // Subsequent absent ticks stay silent — GameGone is a one-shot.
    assert!(st.diff(&sample(None, false, (50, 50))).is_empty());
    assert!(st.diff(&sample(None, false, (50, 50))).is_empty());
}

#[test]
fn no_window_yet_never_emits_game_gone() {
    // Polling before the game launches: absent from the start is not a
    // disappearance.
    let mut st = DiffState::new();
    let evs = st.diff(&sample(None, false, (0, 0)));
    assert!(!evs.iter().any(|e| matches!(e, GameWindowEvent::GameGone)));
    assert!(st.diff(&sample(None, false, (0, 0))).is_empty());
}

#[test]
fn reappearance_after_gone_emits_geometry_again() {
    let mut st = DiffState::new();
    let r = rect(10, 20, 800, 600);
    st.diff(&sample(Some(r), true, (50, 50)));
    st.diff(&sample(None, false, (50, 50)));
    // Relaunch at the exact same rect must still re-report it.
    let evs = st.diff(&sample(Some(r), true, (50, 50)));
    assert!(matches!(
        evs[0],
        GameWindowEvent::Geometry(Rect { x: 10, y: 20, w: 800, h: 600 })
    ));
    assert!(evs.iter().any(|e| matches!(e, GameWindowEvent::Active(true))));
}
