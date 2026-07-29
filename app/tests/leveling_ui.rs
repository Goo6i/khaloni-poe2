//! Geometry/behavior tests for the leveling panel model: act navigation,
//! checkbox hit zones carrying stable step ids, and the done-set persistence
//! roundtrip.

use poe2_lens::leveling_ui::{self, Action};
use poe2_lens_core::refdata::{LevelingAct, LevelingStep};

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
    let dir = std::env::temp_dir().join(format!("poe2lens-lvl-test-{}", std::process::id()));
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
    let dir = std::env::temp_dir().join(format!("poe2lens-lvl-none-{}", std::process::id()));
    assert!(leveling_ui::load_done(&dir).is_empty());
}
