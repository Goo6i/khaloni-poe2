//! The Evaluate panel's nameplate and group-heading geometry.

use khaloni_poe2::evaluate_ui::{self as ev, AffixKind, ItemHeader, Panel, RowGroup, StatRow, TierBadge};

fn measure(s: &str) -> i32 {
    s.chars().count() as i32 * 7
}

fn row(label: &str, group: RowGroup, kind: AffixKind, tier: u8) -> StatRow {
    StatRow {
        label: label.into(),
        badge: Some(TierBadge { kind, tier }),
        score: Some(3.0),
        min: Some(10.0),
        max: None,
        enabled: true,
        target: Some(ev::Target::Stat(0)),
        hidden: false,
        group,
        note: None,
    }
}

fn panel(base_type: Option<&str>, art: bool) -> Panel {
    Panel {
        header: ItemHeader {
            name: "Sol Wrap".into(),
            rarity: "Rare".into(),
            item_level: Some(81),
            requires_level: Some(65),
            base: None,
            base_type: base_type.map(str::to_string),
        },
        rows: vec![
            row("Armour: 2972", RowGroup::Property, AffixKind::Other, 0),
            row("+348 to Armour", RowGroup::Explicit, AffixKind::Prefix, 1),
            row("+60 to maximum Life", RowGroup::Explicit, AffixKind::Prefix, 3),
        ],
        art: art.then(|| ev::Art::new(image::RgbaImage::new(48, 48))),
        ..Panel::default()
    }
}

#[test]
fn the_banner_sits_under_the_title_bar_and_the_rows_start_after_its_rule() {
    let lay = ev::layout(&panel(Some("Soldier Cuirass"), false), &measure);
    assert_eq!(lay.title_bar.y, 0);
    assert_eq!(lay.banner.y, lay.title_bar.y + lay.title_bar.h as i32);
    assert_eq!(lay.banner.w, lay.size.0 as u32, "the banner spans the panel");
    let base = lay.base_type_pos.expect("a base line");
    assert!(base.1 > lay.name_pos.1, "base line under the name");
    assert!(lay.banner_rule_y > lay.banner.y + lay.banner.h as i32 - 1);
    assert!(lay.rarity_pos.1 > lay.banner_rule_y, "properties start under the rule");
    assert!(lay.close.y >= 0 && lay.close.y + (lay.close.h as i32) <= lay.title_bar.h as i32, "close sits in the title bar");
}

#[test]
fn every_row_group_present_gets_one_heading_with_its_count() {
    let lay = ev::layout(&panel(None, false), &measure);
    let heads: Vec<&str> = lay.group_heads.iter().map(|(_, t)| t.as_str()).collect();
    assert_eq!(heads, ["Properties · 1", "Modifiers · 2"]);
    // Each heading precedes the first row of its group.
    assert!(lay.group_heads[0].0 < lay.rows[0].label_pos.1);
    assert!(lay.group_heads[1].0 < lay.rows[1].label_pos.1);
    assert!(lay.group_heads[1].0 > lay.rows[0].label_pos.1);
}

#[test]
fn the_art_slot_is_reserved_when_art_is_expected_even_before_it_arrives() {
    let waiting = ev::layout(&Panel { expects_art: true, ..panel(None, false) }, &measure);
    let with = ev::layout(&panel(None, true), &measure);
    let art = waiting.art.expect("a reserved art slot");
    assert_eq!((art.w, art.h), (48, 48));
    assert!(art.y >= waiting.banner.y && art.y + 48 <= waiting.banner.y + waiting.banner.h as i32, "art inside the banner");
    assert_eq!(waiting.banner.h, with.banner.h, "the banner is already as tall as with art");
}

#[test]
fn no_slot_is_reserved_for_a_panel_that_expects_no_art() {
    let without = ev::layout(&panel(None, false), &measure);
    assert!(without.art.is_none());
    assert!(without.base_type_pos.is_none(), "no base line without a base type");
}

#[test]
fn arriving_art_does_not_change_the_layout() {
    let card = ev::Card { name: String::new(), base: String::new(), rarity: String::new(), figures: Vec::new(), lines: Vec::new() };
    let listing = ev::ListingRow {
        price: "4.8 div".into(),
        raw: String::new(),
        age: "2 h".into(),
        seller: "Vaalstep#3311".into(),
        state: ev::SellerState::Online,
        mine: false,
        times: 1,
        stack: None,
        ilvl: Some(81),
        quality: None,
        gem_level: None,
        corrupted: false,
        instant_buyout: true,
        card,
    };
    let before = Panel { expects_art: true, listings: vec![listing], ..panel(Some("Soldier Cuirass"), false) };
    let after = Panel { art: Some(ev::Art::new(image::RgbaImage::new(48, 48))), ..before.clone() };
    let (a, b) = (ev::layout(&before, &measure), ev::layout(&after, &measure));
    assert_eq!(a.size, b.size);
    assert_eq!(a.banner, b.banner);
    assert_eq!(a.rows.len(), b.rows.len());
    for (ra, rb) in a.rows.iter().zip(&b.rows) {
        assert_eq!(ra.label_pos, rb.label_pos);
    }
    let first = |l: &ev::Layout| l.table.as_ref().expect("a listings table").rows[0];
    assert_eq!(first(&a), first(&b));
}

#[test]
fn art_compares_by_identity_not_by_pixels() {
    let a = ev::Art::new(image::RgbaImage::new(2, 2));
    let b = a.clone();
    let c = ev::Art::new(image::RgbaImage::new(2, 2));
    assert_eq!(a, b);
    assert_ne!(a, c);
}

#[test]
fn late_art_lands_only_on_the_panel_it_was_fetched_for() {
    let mut p = Panel::default();
    p.header.name = "Item".into();
    ev::apply_art(&mut p, "Other", ev::Art::new(image::RgbaImage::new(1, 1)));
    assert!(p.art.is_none());
    ev::apply_art(&mut p, "Item", ev::Art::new(image::RgbaImage::new(1, 1)));
    assert!(p.art.is_some());
}

#[test]
fn a_panel_animates_only_within_its_appear_and_hover_windows() {
    use std::time::{Duration, Instant};
    let t0 = Instant::now();
    let mut p = panel(None, false);
    assert!(!ev::animating(&p, t0), "nothing started");
    p.opened_at = Some(t0);
    assert!(ev::animating(&p, t0 + Duration::from_millis(100)));
    assert!(!ev::animating(&p, t0 + Duration::from_millis(200)));
    p.hover_at = Some(t0 + Duration::from_millis(500));
    assert!(ev::animating(&p, t0 + Duration::from_millis(600)));
    assert!(!ev::animating(&p, t0 + Duration::from_millis(700)));
    p.button_hover = Some((0, t0 + Duration::from_secs(1)));
    assert!(ev::animating(&p, t0 + Duration::from_millis(1100)));
    assert!(!ev::animating(&p, t0 + Duration::from_millis(1200)));
}

#[test]
fn button_at_names_the_button_under_a_point() {
    let lay = ev::layout(&panel(None, false), &measure);
    let (rect, _, _) = &lay.buttons[0];
    assert_eq!(ev::button_at(&lay, rect.x + 1, rect.y + 1), Some(0));
    assert_eq!(ev::button_at(&lay, rect.x - 1, rect.y + 1), None);
}
