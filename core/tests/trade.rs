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
    // A fabricated mod must never silently match anything.
    assert!(index.resolve("this is not a real mod at all").is_none());
    assert!(index.resolve("Adds 3 to 82 Voidfire Damage").is_none());
}

#[test]
fn bad_json_is_an_error() {
    assert!(StatIndex::from_json("not json").is_err());
}

// --- rate limiter + query builder (facts verified live 2026-07-21) ---

use poe2_lens_core::trade::{build_query, RateDecision, RateLimiter};

#[test]
fn parses_the_real_search_rate_rules() {
    let mut rl = RateLimiter::from_header("5:10:60,15:60:300,30:300:1800");
    assert_eq!(rl.check(), RateDecision::Ready);
    for _ in 0..5 {
        rl.record();
    }
    match rl.check() {
        RateDecision::Wait(d) => assert!(d.as_secs() <= 10, "waits for the 10s window"),
        RateDecision::Ready => panic!("5 requests in the 5:10:60 window must saturate it"),
    }
}

#[test]
fn a_reported_ban_locks_the_limiter() {
    let mut rl = RateLimiter::from_header("5:10:60");
    rl.apply_state("1:10:60");
    match rl.check() {
        RateDecision::Wait(d) => assert!(d.as_secs() >= 59, "ban must hold ~60s, got {d:?}"),
        RateDecision::Ready => panic!("an active ban in the state header must lock the limiter"),
    }
    let mut ok = RateLimiter::from_header("5:10:60");
    ok.apply_state("1:10:0");
    assert_eq!(ok.check(), RateDecision::Ready, "zero ban field is not a ban");
}

#[test]
fn builds_the_verified_body_shape_for_the_rare_bow() {
    let stats = StatIndex::from_json(STATS_JSON).expect("stats fixture");
    let item = parse_item(BOW).expect("parse");
    let q = build_query(&item, &stats);
    assert_eq!(q.category.as_deref(), Some("weapon.bow"));
    assert!(q.filters.len() >= 4, "most bow mods resolve, got {}", q.filters.len());
    let body = q.to_body();
    assert_eq!(body["query"]["status"]["option"], "online");
    assert_eq!(body["sort"]["price"], "asc");
    assert_eq!(
        body["query"]["filters"]["type_filters"]["filters"]["category"]["option"],
        "weapon.bow"
    );
    let filters = body["query"]["stats"][0]["filters"].as_array().expect("filters");
    let phys = filters
        .iter()
        .find(|f| f["id"] == "explicit.stat_1509134228")
        .expect("physical damage filter present");
    // 157% rolled, undershot by 10% -> 141
    assert_eq!(phys["value"]["min"], 141);
    assert_eq!(phys["disabled"], false, "damage mods preselect");
}

use poe2_lens_core::trade::{parse_fetch, parse_search, TradeClient, TradeError};

#[test]
fn parses_recorded_search_and_fetch_payloads() {
    let s = parse_search(include_str!("fixtures/trade_search.json")).expect("search fixture");
    assert_eq!(s.id, "D6OM49MVf5");
    assert_eq!(s.hashes.len(), 2);

    let listings = parse_fetch(include_str!("fixtures/trade_fetch.json")).expect("fetch fixture");
    assert_eq!(listings.len(), 2);
    assert_eq!(listings[0].price_currency, "transmute");
    assert_eq!(listings[0].account, "Zubmission101#7022");
    assert_eq!(listings[1].price_amount, 2.5);
    assert_eq!(listings[1].item_name, "Storm Call");
}

#[test]
fn unreachable_host_is_an_http_error_not_a_panic() {
    let mut c = TradeClient::new("http://127.0.0.1:9", "Runes of Aldur").expect("client");
    let q = build_query(
        &poe2_lens_core::item::parse_item(BOW).unwrap(),
        &StatIndex::from_json(STATS_JSON).unwrap(),
    );
    match c.search(&q) {
        Err(TradeError::Http(_)) => {}
        other => panic!("expected Http error, got {other:?}"),
    }
}

#[test]
fn cooldown_blocks_before_any_request_leaves() {
    let mut c = TradeClient::new("http://127.0.0.1:9", "Runes of Aldur").expect("client");
    c.search_limiter.apply_state("1:10:60");
    let q = build_query(
        &poe2_lens_core::item::parse_item(BOW).unwrap(),
        &StatIndex::from_json(STATS_JSON).unwrap(),
    );
    match c.search(&q) {
        Err(TradeError::Cooldown(d)) => assert!(d.as_secs() >= 59),
        other => panic!("expected Cooldown, got {other:?}"),
    }
}

#[test]
#[ignore]
fn live_trade_smoke() {
    let mut c = TradeClient::new("https://www.pathofexile.com", "Runes of Aldur").expect("client");
    let stats = StatIndex::from_json(STATS_JSON).unwrap();
    let item = poe2_lens_core::item::parse_item(BOW).unwrap();
    let mut q = build_query(&item, &stats);
    // Keep only the two filters of the verified live probe: a full
    // 5-filter exact rare can legitimately have zero online matches.
    for f in q.filters.iter_mut() {
        f.disabled = !(f.id == "explicit.stat_1509134228" || f.id == "explicit.stat_3261801346");
    }
    q.filters.retain(|f| !f.disabled);
    let s = c.search(&q).expect("live search");
    assert!(!s.hashes.is_empty());
    let l = c.fetch(&s.id, &s.hashes[..s.hashes.len().min(5)]).expect("live fetch");
    assert!(!l.is_empty());
    println!("live: {} listings, cheapest {} {}", l.len(), l[0].price_amount, l[0].price_currency);
}
