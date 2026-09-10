//! Contract tests for the price-check focus gate: what an F7 press does
//! when the game is, or is not, the focused window.

use std::time::{Duration, Instant};

use khaloni_poe2::config::Rect;
use khaloni_poe2::pricecheck::{FocusGate, Press, FOCUS_WAIT};

const GAME: Rect = Rect { x: 2560, y: 0, w: 2560, h: 1440 };
const INSIDE: (i32, i32) = (3000, 700);
const OUTSIDE: (i32, i32) = (100, 700);

#[test]
fn a_press_with_the_game_focused_copies_at_once() {
    let mut gate = FocusGate::default();
    assert_eq!(gate.press(Instant::now(), true, true, OUTSIDE, GAME), Press::Copy);
}

#[test]
fn a_press_over_the_unfocused_game_asks_for_focus_then_copies_when_it_lands() {
    let mut gate = FocusGate::default();
    let t0 = Instant::now();
    assert_eq!(gate.press(t0, false, true, INSIDE, GAME), Press::FocusGame);
    // The feed reports the game active a tick later: copy exactly once.
    assert!(gate.focused(t0 + Duration::from_millis(120)));
    assert!(!gate.focused(t0 + Duration::from_millis(130)), "one press, one copy");
    assert!(!gate.timed_out(t0 + Duration::from_secs(5)), "a served press never times out");
}

#[test]
fn a_press_away_from_the_game_is_a_note_not_a_copy_into_another_window() {
    let mut gate = FocusGate::default();
    assert_eq!(
        gate.press(Instant::now(), false, true, OUTSIDE, GAME),
        Press::Note("hover an item in the game, then press the key")
    );
    assert_eq!(
        gate.press(Instant::now(), false, false, INSIDE, GAME),
        Press::Note("game window not found")
    );
}

#[test]
fn focus_that_never_arrives_times_out_once() {
    let mut gate = FocusGate::default();
    let t0 = Instant::now();
    assert_eq!(gate.press(t0, false, true, INSIDE, GAME), Press::FocusGame);
    assert!(!gate.timed_out(t0 + FOCUS_WAIT / 2));
    assert!(gate.timed_out(t0 + FOCUS_WAIT + Duration::from_millis(1)));
    assert!(!gate.timed_out(t0 + FOCUS_WAIT * 2), "reported once, not every tick");
    // Focus arriving after the deadline is the user's own alt-tab, not ours.
    assert!(!gate.focused(t0 + FOCUS_WAIT * 2));
}

#[test]
fn a_second_press_while_focus_is_pending_is_ignored() {
    let mut gate = FocusGate::default();
    let t0 = Instant::now();
    assert_eq!(gate.press(t0, false, true, INSIDE, GAME), Press::FocusGame);
    assert_eq!(gate.press(t0 + Duration::from_millis(50), false, true, INSIDE, GAME), Press::Ignore);
    // Once the wait has lapsed a new press starts a new attempt.
    assert!(gate.timed_out(t0 + FOCUS_WAIT * 2));
    assert_eq!(gate.press(t0 + FOCUS_WAIT * 2, false, true, INSIDE, GAME), Press::FocusGame);
}

#[test]
fn an_unrelated_focus_change_never_copies() {
    let mut gate = FocusGate::default();
    assert!(!gate.focused(Instant::now()), "no press waiting");
}

#[test]
fn the_game_rect_edges_count_as_inside_only_on_the_near_side() {
    let mut gate = FocusGate::default();
    let now = Instant::now();
    assert_eq!(gate.press(now, false, true, (2560, 0), GAME), Press::FocusGame);
    let mut gate = FocusGate::default();
    assert!(matches!(gate.press(now, false, true, (5120, 1440), GAME), Press::Note(_)));
}
