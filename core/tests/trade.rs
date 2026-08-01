use khaloni_poe2_core::item::parse_item;
use khaloni_poe2_core::trade::StatIndex;

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
fn waystone_query_searches_by_base_type_tier_and_reward_props() {
    let stats = StatIndex::from_json(STATS_JSON).expect("stats fixture");
    let text = "Item Class: Waystones\nRarity: Rare\nAbandoned Carving\n\
        Waystone (Tier 15)\n--------\nItem Rarity: +24% (augmented)\n\
        Pack Size: +19% (augmented)\nMonster Effectiveness: +28% (augmented)\n\
        --------\nItem Level: 81\n--------\n\
        Monsters have 238% increased Critical Hit Chance\n--------\n";
    let item = parse_item(text).expect("parses");
    let mut q = build_query(&item, &stats);
    assert_eq!(q.type_name.as_deref(), Some("Waystone"), "search by base type");
    assert_eq!(q.map_tier, Some(15), "tier parsed");
    // Reward properties are pickable map_ filters, disabled by default.
    let iir = q.filters.iter().find(|f| f.id == "map_iir").expect("Item Rarity filter");
    assert!(iir.disabled, "disabled by default so the user picks");
    assert_eq!(iir.value.min, 24.0);
    // "Monster Effectiveness" maps to the trade key map_magic_monsters.
    assert!(q.filters.iter().any(|f| f.id == "map_magic_monsters"), "effectiveness pickable");
    assert!(q.filters.iter().any(|f| f.id == "map_packsize"), "pack size pickable");

    let body = q.to_body();
    assert_eq!(body["query"]["type"], "Waystone");
    let mf = &body["query"]["filters"]["map_filters"]["filters"];
    assert_eq!(mf["map_tier"]["min"], 15);
    assert_eq!(mf["map_tier"]["max"], 15);
    assert!(mf["map_iir"].is_null(), "disabled reward filter is not searched");

    // Picking Item Rarity emits it in the map_filters section.
    q.filters.iter_mut().find(|f| f.id == "map_iir").unwrap().disabled = false;
    let body2 = q.to_body();
    assert_eq!(body2["query"]["filters"]["map_filters"]["filters"]["map_iir"]["min"], 24);
}

#[test]
fn unknown_mod_resolves_to_none() {
    let index = StatIndex::from_json(STATS_JSON).unwrap();
    // A fabricated mod must never silently match anything.
    assert!(index.resolve("this is not a real mod at all").is_none());
    assert!(index.resolve("Adds 3 to 82 Voidfire Damage").is_none());
}

#[test]
fn parse_exchange_rate_finds_cheapest_offer() {
    const EXCHANGE: &str = include_str!("fixtures/trade_exchange.json");
    // Live want=divine/have=exalted fixture: a divine costs several exalted.
    let rate = khaloni_poe2_core::trade::parse_exchange_rate(EXCHANGE).expect("offers present");
    assert!(rate.is_finite() && rate >= 1.0, "plausible divine->exalted rate, got {rate}");
    // No offers -> None, not a panic.
    assert!(khaloni_poe2_core::trade::parse_exchange_rate(r#"{"result":{}}"#).is_none());
}

#[test]
fn parse_static_currency_ids_maps_names_to_ids() {
    let json = r#"{"result":[{"id":"Misc","entries":[
        {"id":"omen-of-whittling","text":"Omen of Whittling"},
        {"id":"exalted","text":"Exalted Orb"}]}]}"#;
    let map = khaloni_poe2_core::trade::parse_static_currency_ids(json);
    assert_eq!(map.get("omen of whittling").map(String::as_str), Some("omen-of-whittling"));
    assert_eq!(map.get("exalted orb").map(String::as_str), Some("exalted"));
}

#[test]
fn bad_json_is_an_error() {
    assert!(StatIndex::from_json("not json").is_err());
}

// --- rate limiter + query builder (facts verified live 2026-07-21) ---

use khaloni_poe2_core::trade::{build_query, build_query_with_labels, RateDecision, RateLimiter};

#[test]
fn build_query_covers_implicit_mods() {
    let stats = StatIndex::from_json(STATS_JSON).expect("stats fixture");
    // An item whose only searchable mod is an implicit - like a waystone,
    // valued on its implicit rarity/pack-size. The old explicits-only
    // builder produced zero filters for this; implicits must be covered now.
    let text = "Item Class: Amulets\nRarity: Rare\nTest\nStellar Amulet\n\
        --------\nItem Level: 82\n--------\n+59 to maximum Mana (implicit)\n--------\n";
    let item = parse_item(text).expect("parses");
    assert!(item.explicits.is_empty(), "fixture has no explicit mods");
    assert!(!item.implicits.is_empty(), "fixture has the implicit");
    let (q, labels) = build_query_with_labels(&item, &stats);
    assert!(
        q.filters.iter().any(|f| f.id == "explicit.stat_1050105434"),
        "the implicit maximum-mana mod produced a trade filter"
    );
    assert!(labels.iter().any(|l| l.text.contains("maximum Mana")));
}

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
    // "any" (not "online"): the async marketplace lets you Secure Item from
    // offline sellers, so their listings are part of the real buy price.
    assert_eq!(body["query"]["status"]["option"], "any");
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
    // tier floor of 157(155-169) -> 155
    assert_eq!(phys["value"]["min"], 155); // tier floor of 157(155-169)
    assert_eq!(phys["disabled"], false, "damage mods preselect");
}

#[test]
fn parses_gem_types_and_matches_ocr_to_exact_name() {
    use khaloni_poe2_core::trade::{match_gem_name, parse_gem_types};
    let items = r#"{"result":[
        {"label":"Currency","entries":[{"type":"Exalted Orb"}]},
        {"label":"Gems","entries":[
            {"type":"Detonate Living"},
            {"type":"Fragments Of The Past"},
            {"type":"Conductive Runes"}
        ]}
    ]}"#;
    let gems = parse_gem_types(items);
    assert!(gems.contains(&"Detonate Living".to_string()));
    assert!(!gems.contains(&"Exalted Orb".to_string()), "currency is not a gem");
    // Exact (case-insensitive) match from lowercased OCR.
    assert_eq!(match_gem_name("detonate living", &gems).as_deref(), Some("Detonate Living"));
    // Minor OCR slip still resolves.
    assert_eq!(
        match_gem_name("fragments of the pasl", &gems).as_deref(),
        Some("Fragments Of The Past")
    );
    // Nonsense resolves to nothing rather than the wrong gem.
    assert_eq!(match_gem_name("xyzzy nonsense words", &gems), None);
}

#[test]
fn gem_query_searches_by_skill_name_category_and_exact_level() {
    use khaloni_poe2_core::trade::build_gem_query;
    let body = build_gem_query("Detonate Living", 20).to_body();
    assert_eq!(body["query"]["type"], "Detonate Living");
    assert_eq!(
        body["query"]["filters"]["type_filters"]["filters"]["category"]["option"],
        "gem.activegem"
    );
    let gl = &body["query"]["filters"]["misc_filters"]["filters"]["gem_level"];
    assert_eq!(gl["min"], 20);
    assert_eq!(gl["max"], 20);
}

#[test]
fn decimal_bounds_serialize_as_floats_and_whole_ones_as_integers() {
    use khaloni_poe2_core::trade::{FilterValue, Query, StatFilter};
    let q = Query {
        category: None,
        category_enabled: false,
        type_name: None,
        map_tier: None,
        gem_level: None,
        filters: vec![
            StatFilter {
                id: "explicit.stat_attack_speed".into(),
                value: FilterValue { min: 3.5, max: Some(4.2) },
                disabled: false,
            },
            StatFilter {
                id: "explicit.stat_life".into(),
                value: FilterValue { min: 80.0, max: None },
                disabled: false,
            },
        ],
    };
    let body = q.to_body();
    let filters = body["query"]["stats"][0]["filters"].as_array().unwrap();
    assert_eq!(filters[0]["value"]["min"], 3.5, "decimal kept as float");
    assert_eq!(filters[0]["value"]["max"], 4.2);
    // Whole value stays an integer, not 80.0.
    assert_eq!(filters[1]["value"]["min"], 80);
    assert!(filters[1]["value"]["min"].is_i64(), "whole -> integer json");
}

#[test]
fn disabling_category_searches_mods_only_across_all_bases() {
    let stats = StatIndex::from_json(STATS_JSON).expect("stats fixture");
    let item = parse_item(BOW).expect("parse");
    let mut q = build_query(&item, &stats);
    // The base is present but the user toggled it off: the body must not
    // constrain by category, so the same mods price across every base.
    q.category_enabled = false;
    let body = q.to_body();
    assert!(
        body["query"]["filters"].get("type_filters").is_none(),
        "category disabled -> no type_filters, got {}",
        body["query"]["filters"]
    );
    // The stat filters (the mods) are still there.
    assert!(!body["query"]["stats"][0]["filters"].as_array().unwrap().is_empty());
}

// --- upgrade finder: query side ---

use khaloni_poe2_core::trade::{build_upgrade_query, upgrade_title};

#[test]
fn upgrade_query_meets_or_beats_both_current_values_in_same_category() {
    let stats = StatIndex::from_json(STATS_JSON).expect("stats fixture");
    // Synthetic rare with two numeric explicit mods whose stat ids the
    // fixture catalog verifies elsewhere in this file.
    let text = "Item Class: Amulets\nRarity: Rare\nTest Torc\nGold Amulet\n--------\n\
        Item Level: 80\n--------\n+59 to maximum Mana\n+23% to Fire Resistance\n--------\n";
    let item = parse_item(text).expect("parses");
    assert_eq!(item.explicits.len(), 2, "fixture has exactly the two mods");
    let q = build_upgrade_query(&item, &stats);

    // Same-category constraint, applied the way build_query applies it.
    assert_eq!(q.category.as_deref(), Some("accessory.amulet"));
    assert!(q.category_enabled);

    // Both mods filtered at min = the item's CURRENT value, and enabled:
    // an upgrade must meet-or-beat every kept mod.
    assert_eq!(q.filters.len(), 2);
    let mana = q.filters.iter().find(|f| f.id == "explicit.stat_1050105434").expect("mana filter");
    assert_eq!(mana.value.min, 59.0);
    assert_eq!(mana.value.max, None, "upgrades are open-ended above the current roll");
    assert!(!mana.disabled, "every kept mod constrains the search");
    let fire = q.filters.iter().find(|f| f.id == "explicit.stat_3372524247").expect("fire filter");
    assert_eq!(fire.value.min, 23.0);
    assert!(!fire.disabled);

    // The serialized body carries the category and both mins, cheapest-first.
    let body = q.to_body();
    assert_eq!(
        body["query"]["filters"]["type_filters"]["filters"]["category"]["option"],
        "accessory.amulet"
    );
    assert_eq!(body["sort"]["price"], "asc", "results come back cheapest-first");
    let filters = body["query"]["stats"][0]["filters"].as_array().expect("filters");
    assert!(filters
        .iter()
        .any(|f| f["id"] == "explicit.stat_1050105434" && f["value"]["min"] == 59));
    assert!(filters
        .iter()
        .any(|f| f["id"] == "explicit.stat_3372524247" && f["value"]["min"] == 23));
}

#[test]
fn upgrade_query_uses_current_roll_not_tier_floor() {
    let stats = StatIndex::from_json(STATS_JSON).expect("stats fixture");
    let item = parse_item(BOW).expect("parse");
    let q = build_upgrade_query(&item, &stats);
    // The bow's phys mod is 157(155-169): build_query searches the tier
    // floor (155); an upgrade must beat the actual roll (157).
    let phys = q.filters.iter().find(|f| f.id == "explicit.stat_1509134228").expect("phys filter");
    assert_eq!(phys.value.min, 157.0, "current roll, not the 155 tier floor");
    // Decimal rolls keep their fraction: crafted crit is +3.48(3.11-3.8)%.
    let crit = q.filters.iter().find(|f| f.id == "explicit.stat_518292764").expect("crit filter");
    assert_eq!(crit.value.min, 3.48);
    // Every filter is enabled: no preselect tiering in an upgrade search.
    assert!(q.filters.iter().all(|f| !f.disabled));
}

#[test]
fn upgrade_query_skips_unmatched_and_valueless_mods_not_guesses() {
    // A catalog with one numeric stat and one valueless stat, so both skip
    // paths are exercised against known entries.
    let stats = StatIndex::from_json(
        r##"{"result":[{"id":"explicit","entries":[
            {"id":"explicit.stat_1050105434","text":"# to maximum Mana"},
            {"id":"explicit.stat_frozen","text":"Cannot be Frozen"}]}]}"##,
    )
    .expect("synthetic catalog");
    // Advanced format so the valueless line still classifies as an explicit.
    let text = "Item Class: Amulets\nRarity: Rare\nTest Torc\nGold Amulet\n--------\n\
        { Prefix Modifier \"Mazarine\" (Tier: 4) }\n+59 to maximum Mana\n\
        { Suffix Modifier \"of Ice\" (Tier: 1) }\nCannot be Frozen\n\
        { Suffix Modifier \"of Voidfire\" (Tier: 1) }\n+42 to Voidfire Mastery\n--------\n";
    let item = parse_item(text).expect("parses");
    assert_eq!(item.explicits.len(), 3, "all three lines parsed as explicits");
    let q = build_upgrade_query(&item, &stats);
    // "+42 to Voidfire Mastery" resolves to nothing (mirrors
    // unknown_mod_resolves_to_none) and "Cannot be Frozen" resolves but has
    // no numeric roll: both are dropped, never guessed onto some stat id.
    assert_eq!(q.filters.len(), 1, "only the resolvable numeric mod filters");
    assert_eq!(q.filters[0].id, "explicit.stat_1050105434");
    assert_eq!(q.filters[0].value.min, 59.0);
}

#[test]
fn upgrade_title_names_the_item_class() {
    let item = parse_item(BOW).expect("parse");
    assert_eq!(upgrade_title(&item), "upgrades: Bows");
}

use khaloni_poe2_core::trade::{parse_fetch, parse_search, TradeClient, TradeError};

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
        &khaloni_poe2_core::item::parse_item(BOW).unwrap(),
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
        &khaloni_poe2_core::item::parse_item(BOW).unwrap(),
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
    let item = khaloni_poe2_core::item::parse_item(BOW).unwrap();
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

// --- account features: session cookie + saved-search polling ---

#[test]
fn parse_search_url_roundtrips_a_pasted_search_link() {
    use khaloni_poe2_core::trade::parse_search_url;
    assert_eq!(
        parse_search_url("https://www.pathofexile.com/trade2/search/poe2/Runes%20of%20Aldur/D6OM49MVf5"),
        Some(("Runes of Aldur".to_string(), "D6OM49MVf5".to_string()))
    );
    assert_eq!(parse_search_url("https://example.com/nope"), None);
}

#[test]
fn parse_saved_query_yields_a_repostable_body() {
    use khaloni_poe2_core::trade::parse_saved_query;
    // The saved-search GET returns the stored query (and usually a sort).
    let body = parse_saved_query(
        r#"{"id":"D6OM49MVf5","query":{"status":{"option":"any"},"stats":[{"type":"and","filters":[]}]},"sort":{"price":"asc"}}"#,
    )
    .expect("parses");
    assert_eq!(body["query"]["status"]["option"], "any");
    assert_eq!(body["sort"]["price"], "asc");

    // A saved search without a stored sort still gets the default price
    // sort: the POST endpoint requires one.
    let body = parse_saved_query(r#"{"query":{"status":{"option":"any"}}}"#).expect("parses");
    assert_eq!(body["sort"]["price"], "asc");

    // No query at all is an error, not an empty search of everything.
    assert!(parse_saved_query(r#"{"id":"x"}"#).is_err());
    assert!(parse_saved_query("not json").is_err());
}

#[test]
fn saved_search_ids_respects_the_rate_limiter() {
    // A banned limiter must block the saved-search GET before any request
    // leaves, exactly like the plain search path.
    let mut c = TradeClient::new("http://127.0.0.1:9", "Runes of Aldur").expect("client");
    c.set_session("testsession");
    c.search_limiter.apply_state("1:10:60");
    match c.saved_search_ids("Runes of Aldur", "D6OM49MVf5") {
        Err(TradeError::Cooldown(d)) => assert!(d.as_secs() >= 59),
        other => panic!("expected Cooldown, got {other:?}"),
    }
}

#[test]
fn saved_search_against_unreachable_host_is_an_http_error() {
    let mut c = TradeClient::new("http://127.0.0.1:9", "Runes of Aldur").expect("client");
    c.set_session("testsession");
    match c.saved_search_ids("Runes of Aldur", "D6OM49MVf5") {
        Err(TradeError::Http(_)) => {}
        other => panic!("expected Http error, got {other:?}"),
    }
}
