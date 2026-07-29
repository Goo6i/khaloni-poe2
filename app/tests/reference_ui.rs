//! Geometry/behavior tests for the reference panel model: hitboxes resolve
//! from the same Layout the renderer draws, so these pin the interaction
//! contract without a compositor or font.

use poe2_lens::reference_ui::{self, Action, Cat};

/// Fixed-advance stand-in for the glyph measurer.
fn m(s: &str) -> i32 {
    7 * s.len() as i32
}

#[test]
fn hit_resolves_pill_search_and_close() {
    let mut p = reference_ui::Panel::default();
    p.rows = vec!["Row A".into(), "Row B".into()];
    let lay = reference_ui::layout(&p, &m);
    assert_eq!(reference_ui::hit(&p, &lay, lay.close.x + 1, lay.close.y + 1), Some(Action::Close));
    assert_eq!(
        reference_ui::hit(&p, &lay, lay.search.x + 1, lay.search.y + 1),
        Some(Action::FocusSearch)
    );
    let (r, cat) = (lay.pills[1].0, lay.pills[1].1);
    assert_eq!(reference_ui::hit(&p, &lay, r.x + 1, r.y + 1), Some(Action::SetCat(cat)));
}

#[test]
fn all_thirteen_pills_are_present_and_wrap_within_the_panel() {
    let p = reference_ui::Panel::default();
    let lay = reference_ui::layout(&p, &m);
    assert_eq!(lay.pills.len(), 13);
    assert_eq!(lay.pills[0].1, Cat::Affixes);
    assert_eq!(lay.pills[5].1, Cat::Xile(0));
    assert_eq!(lay.pills[12].1, Cat::Xile(7));
    for (r, _) in &lay.pills {
        assert!(r.x >= 0 && r.x + r.w as i32 <= lay.w, "pill inside panel width");
    }
    // With 13 pills and a narrow panel they cannot all share one row.
    let first_y = lay.pills[0].0.y;
    assert!(lay.pills.iter().any(|(r, _)| r.y > first_y), "pills wrap to a second row");
}

#[test]
fn scroll_clamps_and_visible_window_tracks() {
    let mut p = reference_ui::Panel::default();
    p.rows = (0..200).map(|i| format!("row {i}")).collect();
    p.scroll = 500; // way past the end: layout must clamp, not panic or blank
    let lay = reference_ui::layout(&p, &m);
    assert!(lay.visible.end <= 200 && lay.visible.start < lay.visible.end);
    assert_eq!(lay.rows.len(), lay.visible.len(), "one rect per visible row");
    assert!(lay.visible.len() <= 14);
}

#[test]
fn list_rows_are_informational_not_clickable() {
    let mut p = reference_ui::Panel::default();
    p.rows = (0..20).map(|i| format!("row {i}")).collect();
    let lay = reference_ui::layout(&p, &m);
    let r = &lay.rows[0];
    assert_eq!(reference_ui::hit(&p, &lay, r.x + 3, r.y + 3), None);
}

#[test]
fn width_clamps_and_grows_with_long_rows() {
    let mut p = reference_ui::Panel::default();
    p.rows = vec!["x".into()];
    let narrow = reference_ui::layout(&p, &m).w;
    assert_eq!(narrow, 420, "short rows sit at the minimum width");
    p.rows = vec!["y".repeat(300)];
    let wide = reference_ui::layout(&p, &m).w;
    assert!(wide > narrow && wide <= 900);
}

#[test]
fn default_panel_is_affixes_and_focused() {
    let p = reference_ui::Panel::default();
    assert_eq!(p.cat, Cat::Affixes);
    assert!(p.focused);
    assert!(p.query.is_empty() && p.rows.is_empty() && p.scroll == 0);
}
