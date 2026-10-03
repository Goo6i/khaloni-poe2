//! poe.ninja's PoE2 unique item overviews (`/poe2/api/economy/stash/current/
//! item/overview`, live-verified 2026-09-08 for Forbidden Rites): the bulk
//! unique price source that replaced poe2scout once it stopped tracking the
//! league. Fixture trimmed from the live UniqueWeapons response.

use khaloni_poe2_core::ninja::{unique_prices, ItemOverview, UniqueMatch, UniquePrices, UNIQUE_TYPES};

const WEAPONS: &str = include_str!("fixtures/ninja_unique_weapons.json");

fn exalted(m: UniqueMatch) -> f64 {
    match m {
        UniqueMatch::Exact(ex) => ex,
        other => panic!("expected an exact price, got {other:?}"),
    }
}

#[test]
fn unique_prices_are_in_exalted() {
    let ov: ItemOverview = serde_json::from_str(WEAPONS).expect("live shape parses");
    let prices = unique_prices(&[ov]);
    // primaryValue is in divine; the table speaks exalted.
    let mark = exalted(prices.lookup("Brynhand's Mark", Some("Wooden Club"), false));
    assert!((mark - 0.05 * 142.2).abs() < 1e-6, "got {mark}");
}

#[test]
fn each_base_and_corruption_state_of_a_name_has_its_own_price() {
    let ov: ItemOverview = serde_json::from_str(WEAPONS).expect("parses");
    let prices = unique_prices(&[ov]);
    // Three lines carry the name. The best-listed uncorrupted line used to
    // speak for all of them.
    let fork = exalted(prices.lookup("Runeseeker's Call", Some("Runic Fork"), false));
    let runemastered = exalted(prices.lookup("Runeseeker's Call", Some("Runemastered Runic Fork"), false));
    let corrupted = exalted(prices.lookup("Runeseeker's Call", Some("Runic Fork"), true));
    assert!((fork - 299.4 * 142.2).abs() < 1e-6, "got {fork}");
    assert!((runemastered - 316.4 * 142.2).abs() < 1e-6, "got {runemastered}");
    assert!((corrupted - 40.0 * 142.2).abs() < 1e-6, "got {corrupted}");
    // Base names compare without regard to case or padding.
    assert_eq!(prices.lookup("Runeseeker's Call", Some(" runic fork "), false), UniqueMatch::Exact(fork));
}

#[test]
fn a_variant_without_its_own_line_gets_no_price_from_another() {
    let ov: ItemOverview = serde_json::from_str(WEAPONS).expect("parses");
    let prices = unique_prices(&[ov]);
    // Priced name, unlisted base.
    assert_eq!(prices.lookup("Runeseeker's Call", Some("Gilded Fork"), false), UniqueMatch::Ambiguous);
    // Only the uncorrupted Runemastered line exists; its corrupted twin is
    // a different item.
    assert_eq!(prices.lookup("Runeseeker's Call", Some("Runemastered Runic Fork"), true), UniqueMatch::Ambiguous);
    assert_eq!(prices.lookup("Brynhand's Mark", Some("Wooden Club"), true), UniqueMatch::Ambiguous);
    // No base to go on, and two uncorrupted bases to choose between.
    assert_eq!(prices.lookup("Runeseeker's Call", None, false), UniqueMatch::Ambiguous);
    // No base to go on, but only one line it could be.
    assert!(matches!(prices.lookup("Brynhand's Mark", None, false), UniqueMatch::Exact(_)));
    assert_eq!(prices.lookup("No Such Unique", Some("Wooden Club"), false), UniqueMatch::Unknown);
}

#[test]
fn two_lines_disagreeing_about_one_variant_price_nothing() {
    let mut v: serde_json::Value = serde_json::from_str(WEAPONS).unwrap();
    let mut twin = v["lines"][3].clone();
    twin["primaryValue"] = serde_json::json!(7.5);
    v["lines"].as_array_mut().unwrap().push(twin);
    let ov: ItemOverview = serde_json::from_value(v).unwrap();
    assert_eq!(unique_prices(&[ov]).lookup("Brynhand's Mark", Some("Wooden Club"), false), UniqueMatch::Ambiguous);
}

#[test]
fn a_name_only_source_speaks_for_uncorrupted_items_alone() {
    let prices = UniquePrices::from_names([("The Gnashing Sash".to_string(), 415.0)].into());
    assert_eq!(prices.lookup("The Gnashing Sash", Some("Wide Belt"), false), UniqueMatch::Exact(415.0));
    assert_eq!(prices.lookup("The Gnashing Sash", Some("Wide Belt"), true), UniqueMatch::Ambiguous);
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
