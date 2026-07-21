use poe2_lens_core::ninja::{ExchangeOverview, PriceTable};

#[test]
fn names_exposes_every_priced_entry() {
    let fixture = include_str!("fixtures/ninja_currency.json");
    let ov: ExchangeOverview = serde_json::from_str(fixture).unwrap();
    let table = PriceTable::build(&[ov]);
    let names: Vec<&str> = table.names().collect();
    assert_eq!(names.len(), table.len());
    assert!(names.contains(&"chaos orb"));
    assert!(names.contains(&"divine orb"));
}
