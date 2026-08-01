//! Geometry/behavior tests for the leveling panel model: act navigation,
//! checkbox hit zones carrying stable step ids, and the done-set persistence
//! roundtrip.

use khaloni_poe2::leveling_ui::{self, Action};
use khaloni_poe2_core::refdata::{LevelingAct, LevelingStep};

/// Fixed-advance stand-in for the glyph measurer.
fn m(s: &str) -> i32 {
    7 * s.len() as i32
}

fn step(id: &str, desc: &str) -> LevelingStep {
    LevelingStep {
        id: id.into(),
        kind: "travel".into(),
        zone: "Clearfell".into(),
        description: desc.into(),
        hint: String::new(),
    }
}

fn panel(nsteps: usize) -> leveling_ui::Panel {
    let steps: Vec<LevelingStep> =
        (0..nsteps).map(|i| step(&format!("a1_{i}"), &format!("do thing {i}"))).collect();
    leveling_ui::Panel {
        acts: vec![
            LevelingAct { act: 1, name: "Grelwood".into(), steps },
            LevelingAct { act: 2, name: "Vastiri".into(), steps: vec![step("a2_0", "go east")] },
        ],
        act: 0,
        done: std::collections::HashSet::new(),
        scroll: 0,
    }
}

#[test]
fn prev_next_arrows_and_close_hit_resolve() {
    let p = panel(3);
    let lay = leveling_ui::layout(&p, &m);
    assert_eq!(leveling_ui::hit(&p, &lay, lay.close.x + 1, lay.close.y + 1), Some(Action::Close));
    assert_eq!(leveling_ui::hit(&p, &lay, lay.prev.x + 1, lay.prev.y + 1), Some(Action::PrevAct));
    assert_eq!(leveling_ui::hit(&p, &lay, lay.next.x + 1, lay.next.y + 1), Some(Action::NextAct));
}

#[test]
fn checkbox_click_resolves_toggle_with_the_step_id() {
    let p = panel(3);
    let lay = leveling_ui::layout(&p, &m);
    let (check, id) = &lay.checks[1];
    assert_eq!(
        leveling_ui::hit(&p, &lay, check.x + 1, check.y + 1),
        Some(Action::ToggleStep(id.clone()))
    );
    assert_eq!(id, "a1_1");
    // The whole row is a toggle zone too (checkboxes are small targets).
    let row = &lay.rows[1];
    assert_eq!(
        leveling_ui::hit(&p, &lay, row.x + row.w as i32 - 2, row.y + 2),
        Some(Action::ToggleStep("a1_1".into()))
    );
}

#[test]
fn blank_step_ids_fall_back_to_act_and_index() {
    let mut p = panel(2);
    p.acts[0].steps[1].id.clear();
    let lay = leveling_ui::layout(&p, &m);
    assert_eq!(lay.checks[1].1, "act1:1");
}

#[test]
fn scroll_clamps_and_visible_window_tracks() {
    let mut p = panel(100);
    p.scroll = 500; // clamp, never leave the tail
    let lay = leveling_ui::layout(&p, &m);
    assert!(lay.visible.end <= 100 && lay.visible.start < lay.visible.end);
    assert_eq!(lay.rows.len(), lay.visible.len());
    assert_eq!(lay.checks.len(), lay.visible.len());
    assert!(lay.visible.len() <= 16);
}

#[test]
fn toggle_and_persist_roundtrip() {
    let dir = std::env::temp_dir().join(format!("khalonipoe2-lvl-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut done = std::collections::HashSet::new();
    done.insert("a1:step3".to_string());
    done.insert("a2:step7".to_string());
    leveling_ui::save_done(&dir, &done).unwrap();
    assert_eq!(leveling_ui::load_done(&dir), done);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn missing_done_file_loads_an_empty_set() {
    let dir = std::env::temp_dir().join(format!("khalonipoe2-lvl-none-{}", std::process::id()));
    assert!(leveling_ui::load_done(&dir).is_empty());
}

// ---- auto-advance from ZoneEnter events ----

fn zstep(id: &str, zone: &str) -> LevelingStep {
    LevelingStep {
        id: id.into(),
        kind: "travel".into(),
        zone: zone.into(),
        description: format!("do {id}"),
        hint: String::new(),
    }
}

/// Two acts with repeated zones, mirroring the real data where a zone hosts
/// several consecutive steps.
fn zone_panel() -> leveling_ui::Panel {
    leveling_ui::Panel {
        acts: vec![
            LevelingAct {
                act: 1,
                name: "Grelwood".into(),
                steps: vec![
                    zstep("a1_0", "The Riverbank"),
                    zstep("a1_1", "The Riverbank"),
                    zstep("a1_2", "Clearfell Encampment"),
                    zstep("a1_3", "Clearfell"),
                ],
            },
            LevelingAct {
                act: 2,
                name: "Vastiri".into(),
                steps: vec![zstep("a2_0", "Vastiri Outskirts"), zstep("a2_1", "Mawdun Quarry")],
            },
        ],
        act: 0,
        done: std::collections::HashSet::new(),
        scroll: 0,
    }
}

#[test]
fn advance_marks_prior_steps_done_and_switches_act() {
    let mut p = zone_panel();
    // Entering an act-2 zone means all of act 1 and the act-2 steps before it
    // are behind the player.
    assert!(leveling_ui::advance_to_zone(&mut p, "Mawdun Quarry"));
    assert_eq!(p.act, 1);
    for id in ["a1_0", "a1_1", "a1_2", "a1_3", "a2_0"] {
        assert!(p.done.contains(id), "{id} should be done");
    }
    // The matched step itself stays pending: entering the zone is not the
    // same as completing what the guide says to do there.
    assert!(!p.done.contains("a2_1"));
}

#[test]
fn advance_is_case_insensitive() {
    let mut p = zone_panel();
    assert!(leveling_ui::advance_to_zone(&mut p, "cLeArFeLl EnCaMpMeNt"));
    assert!(p.done.contains("a1_0") && p.done.contains("a1_1"));
    assert!(!p.done.contains("a1_2"));
}

#[test]
fn advance_targets_the_first_pending_step_in_the_zone() {
    let mut p = zone_panel();
    // Zone with two steps: entering it marks nothing (first pending match is
    // step 0, nothing earlier), and does not skip past the zone's own steps.
    assert!(!leveling_ui::advance_to_zone(&mut p, "The Riverbank"));
    assert!(p.done.is_empty());
    // Once step 0 is checked off, re-entering targets step 1 — still nothing
    // EARLIER to mark, so no change.
    p.done.insert("a1_0".into());
    assert!(!leveling_ui::advance_to_zone(&mut p, "The Riverbank"));
    assert!(!p.done.contains("a1_1"));
}

#[test]
fn advance_same_zone_twice_changes_nothing_the_second_time() {
    let mut p = zone_panel();
    assert!(leveling_ui::advance_to_zone(&mut p, "Vastiri Outskirts"));
    let snapshot = p.clone();
    assert!(!leveling_ui::advance_to_zone(&mut p, "Vastiri Outskirts"));
    assert_eq!(p, snapshot);
}

#[test]
fn advance_unknown_zone_is_a_no_op() {
    let mut p = zone_panel();
    assert!(!leveling_ui::advance_to_zone(&mut p, "The Nonexistent Depths"));
    assert!(p.done.is_empty());
    assert_eq!(p.act, 0);
}

#[test]
fn advance_when_all_matching_steps_are_done_is_a_no_op() {
    let mut p = zone_panel();
    p.done.insert("a1_2".into());
    // Re-entering a fully-completed zone must not yank the view around.
    assert!(!leveling_ui::advance_to_zone(&mut p, "Clearfell Encampment"));
}

#[test]
fn advance_scrolls_the_matched_step_into_view() {
    let steps: Vec<LevelingStep> = (0..40).map(|i| zstep(&format!("s{i}"), &format!("Zone {i}"))).collect();
    let mut p = leveling_ui::Panel {
        acts: vec![LevelingAct { act: 1, name: "Long".into(), steps }],
        act: 0,
        done: std::collections::HashSet::new(),
        scroll: 0,
    };
    assert!(leveling_ui::advance_to_zone(&mut p, "Zone 30"));
    let lay = leveling_ui::layout(&p, &m);
    assert!(lay.visible.contains(&30), "step 30 should be visible, got {:?}", lay.visible);
    // And scrolling back up when the player revisits an earlier pending zone:
    // undo the auto-done so an early step is pending again.
    p.done.remove("s2");
    assert!(leveling_ui::advance_to_zone(&mut p, "Zone 2"));
    let lay = leveling_ui::layout(&p, &m);
    assert!(lay.visible.contains(&2), "step 2 should be visible, got {:?}", lay.visible);
}
