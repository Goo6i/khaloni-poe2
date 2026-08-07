//! Geometry/behavior tests for the reference panel model: hitboxes resolve
//! from the same Layout the renderer draws, so these pin the interaction
//! contract without a compositor or font.

use khaloni_poe2::reference_ui::{self, Action, Cat, Family, Row};

/// Fixed-advance stand-in for the glyph measurer.
fn m(s: &str) -> i32 {
    7 * s.len() as i32
}

fn plain(text: &str) -> Row {
    Row { text: text.to_string(), ..Row::default() }
}

#[test]
fn hit_resolves_pill_search_and_close() {
    let mut p = reference_ui::Panel {
        rows: vec![plain("Row A"), plain("Row B")],
        ..Default::default()
    };
    let lay = reference_ui::layout(&p, &m);
    assert_eq!(
        reference_ui::hit(&mut p, &lay, lay.close.x + 1, lay.close.y + 1),
        Some(Action::Close)
    );
    assert_eq!(
        reference_ui::hit(&mut p, &lay, lay.search.x + 1, lay.search.y + 1),
        Some(Action::FocusSearch)
    );
    let (r, cat) = (lay.pills[1].0, lay.pills[1].1);
    assert_eq!(reference_ui::hit(&mut p, &lay, r.x + 1, r.y + 1), Some(Action::SetCat(cat)));
}

#[test]
fn all_category_pills_are_present_and_wrap_within_the_panel() {
    let p = reference_ui::Panel::default();
    let lay = reference_ui::layout(&p, &m);
    // 5 fixed categories + every XILE_CATEGORIES entry: derived, so adding
    // a dataset never silently breaks this test again.
    assert_eq!(lay.pills.len(), 5 + khaloni_poe2::refcache::XILE_CATEGORIES.len());
    assert_eq!(lay.pills[0].1, Cat::Affixes);
    assert_eq!(lay.pills[5].1, Cat::Xile(0));
    assert_eq!(lay.pills[12].1, Cat::Xile(7));
    for (r, _) in &lay.pills {
        assert!(r.x >= 0 && r.x + r.w as i32 <= lay.w, "pill inside panel width");
    }
    // With this many pills and a narrow panel they cannot all share one row.
    let first_y = lay.pills[0].0.y;
    assert!(lay.pills.iter().any(|(r, _)| r.y > first_y), "pills wrap to a second row");
}

#[test]
fn family_pills_sit_under_the_categories_and_resolve_the_filter() {
    let mut p = reference_ui::Panel::default();
    let lay = reference_ui::layout(&p, &m);
    assert_eq!(lay.fam_pills.len(), 3);
    let cats_bottom = lay.pills.iter().map(|(r, _)| r.y).max().unwrap();
    assert!(lay.fam_pills.iter().all(|(r, _)| r.y > cats_bottom), "filter row below categories");
    // The click stores the filter on the model and rides SetCat(current) so
    // the caller re-runs the search it alone can run.
    let (r, _) = lay.fam_pills[2];
    assert_eq!(reference_ui::hit(&mut p, &lay, r.x + 1, r.y + 1), Some(Action::SetCat(Cat::Affixes)));
    assert_eq!(p.family, Family::Suffix);
}

#[test]
fn scroll_clamps_and_visible_window_tracks() {
    let p = reference_ui::Panel {
        rows: (0..200).map(|i| plain(&format!("row {i}"))).collect(),
        scroll: 500, // way past the end: layout must clamp, not panic or blank
        ..Default::default()
    };
    let lay = reference_ui::layout(&p, &m);
    assert!(lay.visible.end <= 200 && lay.visible.start < lay.visible.end);
    assert_eq!(lay.rows.len(), lay.visible.len(), "one geom per visible row");
    assert!(lay.visible.len() <= 14);
}

#[test]
fn ladderless_rows_are_informational_not_expandable() {
    let mut p = reference_ui::Panel {
        rows: (0..20).map(|i| plain(&format!("row {i}"))).collect(),
        ..Default::default()
    };
    let lay = reference_ui::layout(&p, &m);
    let r = lay.rows[0].rect;
    assert_eq!(reference_ui::hit(&mut p, &lay, r.x + 3, r.y + 3), None);
    assert_eq!(p.expanded, None, "no ladder, so the click unfolds nothing");
}

#[test]
fn row_click_unfolds_the_ladder_into_the_layout() {
    let mut p = reference_ui::Panel {
        rows: (0..20)
            .map(|i| Row {
                text: format!("row {i}"),
                ladder: "T1  i75  (30-39)\nT2  i1  (10-19)".to_string(),
                ..Row::default()
            })
            .collect(),
        scroll: 3,
        ..Default::default()
    };
    let lay = reference_ui::layout(&p, &m);
    // Clicking the second visible row targets absolute row 4.
    let r = lay.rows[1].rect;
    assert_eq!(reference_ui::hit(&mut p, &lay, r.x + 3, r.y + 3), None);
    assert_eq!(p.expanded, Some(4));
    let lay = reference_ui::layout(&p, &m);
    assert_eq!(lay.expanded_slot, Some(1));
    assert_eq!(lay.ladder.len(), 2, "one rect per tier line");
    assert_eq!(lay.ladder[0].y, lay.rows[1].rect.y + lay.rows[1].rect.h as i32);
    // A second click on the row folds it back up.
    assert_eq!(reference_ui::hit(&mut p, &lay, r.x + 3, r.y + 3), None);
    assert_eq!(p.expanded, None);
}

#[test]
fn width_clamps_and_grows_with_long_rows() {
    let mut p = reference_ui::Panel { rows: vec![plain("x")], ..Default::default() };
    let narrow = reference_ui::layout(&p, &m).w;
    assert_eq!(narrow, 420, "short rows sit at the minimum width");
    p.rows = vec![plain(&"y".repeat(300))];
    let wide = reference_ui::layout(&p, &m).w;
    assert!(wide > narrow && wide <= 900);
}

#[test]
fn default_panel_is_affixes_focused_and_unfiltered() {
    let p = reference_ui::Panel::default();
    assert_eq!(p.cat, Cat::Affixes);
    assert_eq!(p.family, Family::All);
    assert!(p.focused);
    assert!(p.query.is_empty() && p.rows.is_empty() && p.scroll == 0 && p.expanded.is_none());
}
