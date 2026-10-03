use khaloni_poe2_core::derived::weapon_stats;
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
