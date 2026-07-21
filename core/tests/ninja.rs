use poe2_lens_core::ninja::{
    DataOrigin, ExchangeOverview, NinjaClient, NinjaError, PriceTable,
};

const CURRENCY_JSON: &str = include_str!("fixtures/ninja_currency.json");

#[test]
fn parses_recorded_overview() {
    let ov: ExchangeOverview = serde_json::from_str(CURRENCY_JSON).unwrap();
    assert_eq!(ov.core.primary, "divine");
    assert_eq!(ov.core.rates.get("chaos"), Some(&7.29));
    assert_eq!(ov.lines.len(), 3);
    assert_eq!(ov.lines[0].id, "chaos");
    assert!((ov.lines[0].primary_value - 0.1372).abs() < 1e-9);
    assert_eq!(ov.items[0].name, "Orb of Annulment");
}

#[test]
fn price_table_resolves_names_and_denominations() {
    let ov: ExchangeOverview = serde_json::from_str(CURRENCY_JSON).unwrap();
    let table = PriceTable::build(&[ov]);

    // catalog-name lookup, case and punctuation insensitive
    let annul = table.lookup("Orb of Annulment").unwrap();
    assert!((annul.divine - 0.0325).abs() < 1e-9);
    assert!((annul.chaos - 0.0325 * 7.29).abs() < 1e-6);
    assert!((annul.exalted - 0.0325 * 410.0).abs() < 1e-6);

    // core reference currencies resolve too
    let chaos = table.lookup("chaos orb").unwrap();
    assert!((chaos.divine - 0.1372).abs() < 1e-9);
    // divine itself is the unit
    let divine = table.lookup("Divine Orb").unwrap();
    assert!((divine.divine - 1.0).abs() < 1e-9);

    assert!(table.lookup("Mirror of Kalandra").is_none());
}

#[test]
fn empty_lines_is_an_error() {
    let empty = r#"{"core":{"items":[],"rates":{},"primary":"chaos","secondary":"divine"},"lines":[],"items":[]}"#;
    let ov: ExchangeOverview = serde_json::from_str(empty).unwrap();
    assert!(matches!(
        NinjaClient::validate(&ov, "Bogus"),
        Err(NinjaError::EmptyResponse(_))
    ));
}

#[test]
fn stale_cache_fallback_when_network_unreachable() {
    let dir = std::env::temp_dir().join("poe2-lens-test-cache");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    // seed the cache exactly as the client writes it
    let cache_file = dir.join("Runes of Aldur-Currency.json");
    std::fs::write(&cache_file, CURRENCY_JSON).unwrap();

    // unroutable base: fetch fails, cache must be served with StaleCache origin
    let client = NinjaClient::with_base("http://127.0.0.1:9".to_string(), dir.clone());
    let (ov, origin) = client
        .exchange_overview("Runes of Aldur", "Currency")
        .unwrap();
    assert_eq!(origin, DataOrigin::StaleCache);
    assert_eq!(ov.lines.len(), 3);

    // no cache and no network: hard error
    let empty_dir = std::env::temp_dir().join("poe2-lens-test-cache-empty");
    let _ = std::fs::remove_dir_all(&empty_dir);
    std::fs::create_dir_all(&empty_dir).unwrap();
    let client2 = NinjaClient::with_base("http://127.0.0.1:9".to_string(), empty_dir);
    assert!(client2.exchange_overview("Runes of Aldur", "Currency").is_err());
}

#[test]
#[ignore = "live network smoke test, run manually"]
fn live_smoke() {
    let dir = std::env::temp_dir().join("poe2-lens-live-cache");
    std::fs::create_dir_all(&dir).unwrap();
    let client = NinjaClient::new(dir);
    let leagues = client.leagues().unwrap();
    assert!(leagues.iter().any(|l| l.id == "Standard"));
    let (ov, origin) = client
        .exchange_overview(&leagues[0].id, "Currency")
        .unwrap();
    assert_eq!(origin, DataOrigin::Fresh);
    assert!(!ov.lines.is_empty());
}
