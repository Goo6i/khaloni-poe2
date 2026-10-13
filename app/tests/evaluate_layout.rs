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
fn the_art_slot_is_laid_out_only_when_the_panel_has_art() {
    let without = ev::layout(&panel(None, false), &measure);
    assert!(without.art.is_none());
    assert!(without.base_type_pos.is_none(), "no base line without a base type");
    let with = ev::layout(&panel(None, true), &measure);
    let art = with.art.expect("an art slot");
    assert_eq!((art.w, art.h), (48, 48));
    assert!(art.y >= with.banner.y && art.y + 48 <= with.banner.y + with.banner.h as i32, "art inside the banner");
    assert!(with.banner.h >= without.banner.h, "the banner grows to hold the art");
}

#[test]
fn art_compares_by_identity_not_by_pixels() {
    let a = ev::Art::new(image::RgbaImage::new(2, 2));
    let b = a.clone();
    let c = ev::Art::new(image::RgbaImage::new(2, 2));
    assert_eq!(a, b);
    assert_ne!(a, c);
}
