//! poe.ninja's PoE2 unique item overviews (`/poe2/api/economy/stash/current/
//! item/overview`, live-verified 2026-09-08 for Forbidden Rites): the bulk
//! unique price source that replaced poe2scout once it stopped tracking the
//! league. Fixture trimmed from the live UniqueWeapons response.

use khaloni_poe2_core::ninja::{unique_prices, ItemOverview, UNIQUE_TYPES};

const WEAPONS: &str = include_str!("fixtures/ninja_unique_weapons.json");

#[test]
fn unique_prices_are_keyed_by_name_in_exalted() {
    let ov: ItemOverview = serde_json::from_str(WEAPONS).expect("live shape parses");
    let map = unique_prices(&[ov]);
    // primaryValue is in divine; the table speaks exalted.
    let mark = map.get("Brynhand's Mark").copied().expect("priced");
    assert!((mark - 0.05 * 142.2).abs() < 1e-6, "got {mark}");
}

#[test]
fn a_name_on_several_bases_takes_the_uncorrupted_best_listed_line() {
    let ov: ItemOverview = serde_json::from_str(WEAPONS).expect("parses");
    let map = unique_prices(&[ov]);
    // Two bases carry the name; the corrupted line has the most listings
    // and a bargain price, but a corrupted price is not the item's price.
    let call = map.get("Runeseeker's Call").copied().expect("priced");
    assert!((call - 299.4 * 142.2).abs() < 1e-6, "the 25-listing uncorrupted Runic Fork line, got {call}");
}

#[test]
fn an_overview_in_an_unexpected_currency_contributes_nothing() {
    let mut v: serde_json::Value = serde_json::from_str(WEAPONS).unwrap();
    v["core"]["primary"] = serde_json::json!("chaos");
    let ov: ItemOverview = serde_json::from_value(v).unwrap();
    assert!(unique_prices(&[ov]).is_empty(), "never mis-scale a price by guessing the unit");
}

#[test]
fn the_type_list_covers_every_unique_category_ninja_publishes() {
    for t in ["UniqueWeapons", "UniqueArmours", "UniqueAccessories", "UniqueFlasks", "UniqueCharms", "UniqueJewels", "UniqueSanctumRelics", "UniqueTablets"] {
        assert!(UNIQUE_TYPES.contains(&t), "{t} missing");
    }
    assert!(!UNIQUE_TYPES.contains(&"PrecursorTablets"), "rare tablets are not uniques");
}
