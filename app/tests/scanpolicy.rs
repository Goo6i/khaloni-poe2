//! Contract tests for the scan/show policy: focus loss pauses scanning
//! and hides the rows; focus on our own overlay does not.

use khaloni_poe2::platform::Focus;
use khaloni_poe2::scanpolicy::{decide, Decision, Inputs};

fn playing() -> Inputs {
    Inputs { scanning: true, game_present: true, game_visible: true, focus: Focus::Game, pause_when_hidden: true }
}

#[test]
fn a_focused_visible_game_scans_and_shows() {
    assert_eq!(decide(playing()), Decision { paused: false, show_rows: true, on_screen: true });
}

#[test]
fn losing_focus_to_another_window_pauses_and_hides_rows() {
    let d = decide(Inputs { focus: Focus::Other, ..playing() });
    assert!(d.paused, "no scans of an unattended game");
    assert!(!d.show_rows, "labels leave with the focus");
    assert!(d.on_screen, "a cursor note can still draw over the visible game");
}

#[test]
fn focus_on_our_own_overlay_is_not_a_loss() {
    // Clicking the trade card activates the overlay window.
    let d = decide(Inputs { focus: Focus::Overlay, ..playing() });
    assert_eq!(d, Decision { paused: false, show_rows: true, on_screen: true });
}

#[test]
fn a_covered_game_pauses_and_hides_when_the_user_asked_for_that() {
    let d = decide(Inputs { game_visible: false, ..playing() });
    assert_eq!(d, Decision { paused: true, show_rows: false, on_screen: false });
    // With the preference off, occlusion changes nothing.
    let d = decide(Inputs { game_visible: false, pause_when_hidden: false, ..playing() });
    assert_eq!(d, Decision { paused: false, show_rows: true, on_screen: true });
}

#[test]
fn the_master_switch_pauses_without_taking_the_game_off_screen() {
    let d = decide(Inputs { scanning: false, ..playing() });
    assert_eq!(d, Decision { paused: true, show_rows: false, on_screen: true });
}

#[test]
fn no_game_means_nothing_runs_or_draws() {
    let d = decide(Inputs { game_present: false, ..playing() });
    assert_eq!(d, Decision { paused: true, show_rows: false, on_screen: false });
}
