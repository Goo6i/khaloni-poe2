use khaloni_poe2_core::ninja::{
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
fn missing_exalted_rate_is_rejected_and_not_cached() {
    // nonempty lines, but rates is empty so exalted is missing entirely
    let malformed = CURRENCY_JSON.replacen(
        r#""rates": {"exalted": 410.0, "chaos": 7.29}"#,
        r#""rates": {}"#,
        1,
    );
    assert_ne!(malformed, CURRENCY_JSON, "string patch must have matched");
    let ov: ExchangeOverview = serde_json::from_str(&malformed).unwrap();
    assert!(!ov.lines.is_empty());
    assert!(NinjaClient::validate(&ov, "Currency").is_err());

    let dir = std::env::temp_dir().join("khaloni-poe2-test-cache-malformed-rates");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let client = NinjaClient::with_base(serve_once(&malformed), dir.clone());
    let result = client.exchange_overview("Runes of Aldur", "Currency");
    assert!(result.is_err());

    let cache_file = dir.join("Runes of Aldur-Currency.json");
    assert!(
        !cache_file.exists(),
        "malformed response must not be written to cache"
    );
}

#[test]
fn non_divine_primary_is_rejected_and_not_cached() {
    let malformed = CURRENCY_JSON.replacen(r#""primary": "divine""#, r#""primary": "chaos""#, 1);
    assert_ne!(malformed, CURRENCY_JSON, "string patch must have matched");
    let ov: ExchangeOverview = serde_json::from_str(&malformed).unwrap();
    assert!(!ov.lines.is_empty());
    assert!(NinjaClient::validate(&ov, "Currency").is_err());

    let dir = std::env::temp_dir().join("khaloni-poe2-test-cache-malformed-primary");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let client = NinjaClient::with_base(serve_once(&malformed), dir.clone());
    let result = client.exchange_overview("Runes of Aldur", "Currency");
    assert!(result.is_err());

    let cache_file = dir.join("Runes of Aldur-Currency.json");
    assert!(
        !cache_file.exists(),
        "malformed response must not be written to cache"
    );
}

#[test]
fn stale_cache_fallback_when_network_unreachable() {
    let dir = std::env::temp_dir().join("khaloni-poe2-test-cache");
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
    let empty_dir = std::env::temp_dir().join("khaloni-poe2-test-cache-empty");
    let _ = std::fs::remove_dir_all(&empty_dir);
    std::fs::create_dir_all(&empty_dir).unwrap();
    let client2 = NinjaClient::with_base("http://127.0.0.1:9".to_string(), empty_dir);
    assert!(client2.exchange_overview("Runes of Aldur", "Currency").is_err());
}

/// Spawns a one-shot raw HTTP server on a random local port that replies with
/// a 200 response carrying the given body, then returns its base URL. Used to
/// exercise NinjaClient::exchange_overview without a real network dependency.
fn serve_once(body: &str) -> String {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let port = listener.local_addr().unwrap().port();
    let body = body.to_string();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    format!("http://127.0.0.1:{port}")
}

#[test]
#[ignore = "live network smoke test, run manually"]
fn live_smoke() {
    let dir = std::env::temp_dir().join("khaloni-poe2-live-cache");
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
