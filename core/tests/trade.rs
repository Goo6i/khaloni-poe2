use poe2_lens_core::item::parse_item;
use poe2_lens_core::trade::StatIndex;

const STATS_JSON: &str = include_str!("fixtures/trade_stats.json");
const BOW: &str = include_str!("fixtures/item1-inventory-rare-bow.txt");
const JEWEL: &str = include_str!("fixtures/item2-stash-rare-jewel.txt");
const AMULET: &str = include_str!("fixtures/item3-chatlink-rare-amulet.txt");

fn mod_text(item_text: &str, needle: &str) -> String {
    let item = parse_item(item_text).expect("fixture parses");
    item.explicits
        .iter()
        .chain(item.implicits.iter())
        .map(|m| m.text.clone())
        .find(|t| t.contains(needle))
        .unwrap_or_else(|| panic!("no mod containing {needle:?} in fixture"))
}

#[test]
fn resolves_six_verified_stat_ids_from_real_fixtures() {
    let index = StatIndex::from_json(STATS_JSON).unwrap();

    let cases: [(&str, &str, &str); 6] = [
        (BOW, "increased Physical Damage", "explicit.stat_1509134228"),
        (BOW, "to Dexterity", "explicit.stat_3261801346"),
        (BOW, "Level of all Projectile Skills", "explicit.stat_1202301673"),
        (JEWEL, "increased Evasion Rating", "explicit.stat_2106365538"),
        (AMULET, "to maximum Mana", "explicit.stat_1050105434"),
        (AMULET, "to Fire Resistance", "explicit.stat_3372524247"),
    ];

    for (fixture, needle, expected_id) in cases {
        let text = mod_text(fixture, needle);
        let entry = index
            .resolve(&text)
            .unwrap_or_else(|| panic!("mod text {text:?} did not resolve"));
        assert_eq!(entry.id, expected_id, "mod text was {text:?}");
    }
}

#[test]
fn unknown_mod_resolves_to_none() {
    let index = StatIndex::from_json(STATS_JSON).unwrap();
    // Real mod text, but its stat id was trimmed out of the fixture, so it
    // must not be found rather than silently matching something else.
    let text = mod_text(BOW, "Lightning Damage");
    assert!(index.resolve(&text).is_none());

    assert!(index.resolve("this is not a real mod at all").is_none());
}

#[test]
fn bad_json_is_an_error() {
    assert!(StatIndex::from_json("not json").is_err());
}
