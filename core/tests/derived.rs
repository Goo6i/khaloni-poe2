use khaloni_poe2_core::derived::{pseudo_totals, weapon_stats};
use khaloni_poe2_core::item::parse_item;

const BOW: &str = include_str!("fixtures/item1-inventory-rare-bow.txt");
const AMULET: &str = include_str!("fixtures/item3-chatlink-rare-amulet.txt");

#[test]
fn the_bow_fixture_computes_to_its_real_dps() {
    let item = parse_item(BOW).unwrap();
    let w = weapon_stats(&item).expect("a bow states Attacks per Second");
    // Physical 266-499 averages 382.5; Lightning 3-82 averages 42.5; both
    // times the stated 1.10 attacks per second.
    assert!((w.phys_dps - 420.75).abs() < 1e-9, "phys {}", w.phys_dps);
    assert!((w.ele_dps - 46.75).abs() < 1e-9, "ele {}", w.ele_dps);
    assert_eq!(w.chaos_dps, 0.0);
    assert!((w.total_dps - 467.5).abs() < 1e-9, "total {}", w.total_dps);
    assert!((w.aps - 1.10).abs() < 1e-9);
    assert!((w.crit_chance - 8.48).abs() < 1e-9);
}

#[test]
fn a_non_weapon_has_no_dps() {
    let item = parse_item(AMULET).unwrap();
    assert_eq!(weapon_stats(&item), None);
}

#[test]
fn pseudo_totals_follow_the_trade_site_rules() {
    // The amulet: "+5 to all Attributes" (implicit) counts once per
    // attribute, "+28% to Fire Resistance" counts toward the elemental
    // total, and "+59 to maximum Mana" / "24% increased Armour" /
    // "24.7 Life Regeneration per second" contribute nothing.
    let p = pseudo_totals(&parse_item(AMULET).unwrap());
    assert_eq!(p.total_attributes, 15.0);
    assert_eq!(p.total_elemental_resistance, 28.0);
    assert_eq!(p.total_life, 0.0);
    assert_eq!(p.total_es, 0.0);

    // The bow: its only flat grant is "+31 to Dexterity"; the advanced
    // format's inline roll ranges ("+31(31-33)") must not confuse it.
    let p = pseudo_totals(&parse_item(BOW).unwrap());
    assert_eq!(p.total_attributes, 31.0);
    assert_eq!(p.total_elemental_resistance, 0.0);
}

#[test]
fn chaos_resistance_stays_out_of_the_elemental_total() {
    let item = parse_item(
        "Item Class: Rings\nRarity: Rare\nDoom Loop\nRuby Ring\n--------\nItem Level: 70\n--------\n+30% to Chaos Resistance\n+10% to Cold Resistance\n+12% to all Elemental Resistances\n",
    )
    .unwrap();
    let p = pseudo_totals(&item);
    // Cold 10 + all-elemental 12*3; chaos excluded entirely.
    assert_eq!(p.total_elemental_resistance, 46.0);
}

#[test]
fn life_and_energy_shield_sum_across_implicit_and_explicit() {
    let item = parse_item(
        "Item Class: Body Armours\nRarity: Rare\nCorpse Shell\nVile Robe\n--------\nItem Level: 70\n--------\n+20 to maximum Life (implicit)\n--------\n+85 to maximum Life\n+64 to maximum Energy Shield\n",
    )
    .unwrap();
    let p = pseudo_totals(&item);
    assert_eq!(p.total_life, 105.0);
    assert_eq!(p.total_es, 64.0);
}

#[test]
fn aggregate_elemental_damage_line_is_not_double_counted() {
    // Some exports carry the combined "Elemental Damage:" line, some the
    // per-element lines; an item is never charged for both.
    let combined = parse_item(
        "Item Class: Wands\nRarity: Rare\nStorm Call\nAcrid Wand\n--------\nElemental Damage: 10-20 (augmented), 30-40 (augmented)\nAttacks per Second: 1.00\n--------\nItem Level: 70\n",
    )
    .unwrap();
    let w = weapon_stats(&combined).unwrap();
    // (15 + 35) * 1.0
    assert_eq!(w.ele_dps, 50.0);
    assert_eq!(w.total_dps, 50.0);

    let both = parse_item(
        "Item Class: Wands\nRarity: Rare\nStorm Call\nAcrid Wand\n--------\nFire Damage: 10-20 (augmented)\nCold Damage: 30-40 (augmented)\nElemental Damage: 10-20, 30-40\nAttacks per Second: 1.00\n--------\nItem Level: 70\n",
    )
    .unwrap();
    assert_eq!(weapon_stats(&both).unwrap().ele_dps, 50.0);
}
