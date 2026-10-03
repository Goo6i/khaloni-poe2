use khaloni_poe2_core::item::parse_item;
use khaloni_poe2_core::trade::{EquipKey, EquipmentFilters, Query, StatIndex};

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

/// The catalog interleaves section headers ("Uncut Skill Gems", "Omens")
/// and blank spacers with the currencies, all under the id "sep". The
/// exchange refuses "sep" as a want tag, so none may look like a currency.
#[test]
fn catalog_separators_are_not_currencies() {
    let json = include_str!("fixtures/trade_static.json");
    let map = khaloni_poe2_core::trade::parse_static_currency_ids(json);
    assert!(!map.contains_key(""), "a blank spacer became a currency");
    for header in ["ultimatum fragments", "omens", "catalysts", "soul cores", "flux"] {
        assert!(!map.contains_key(header), "the section header {header:?} became a currency");
    }
    assert!(map.values().all(|id| id != "sep" && !id.is_empty()), "{map:?}");
    assert_eq!(map.get("exalted orb").map(String::as_str), Some("exalted"));
    assert_eq!(map.get("scroll of wisdom").map(String::as_str), Some("wisdom"));
}

#[test]
fn bad_json_is_an_error() {
    assert!(StatIndex::from_json("not json").is_err());
}

// --- rate limiter + query builder (facts verified live 2026-07-21) ---

use khaloni_poe2_core::trade::{
    build_upgrade_query as build_query, build_upgrade_query_with_labels as build_query_with_labels, RateDecision,
    RateLimiter,
};

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
fn a_thousands_separator_is_not_a_second_damage_range() {
    let text = "Item Class: Crossbows\nRarity: Rare\nDragon Core\nSiege Crossbow\n--------\n\
        Physical Damage: 414-1,043 (augmented)\nAttacks per Second: 2.00\n--------\nItem Level: 82\n";
    let w = khaloni_poe2_core::derived::weapon_stats(&parse_item(text).unwrap()).unwrap();
    assert!((w.phys_dps - 1457.0).abs() < 1e-9, "got {}", w.phys_dps);
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
        filters: vec![
            StatFilter {
                value: FilterValue { min: Some(3.5), max: Some(4.2) },
                ..StatFilter::at_least("explicit.stat_attack_speed", 3.5, false)
            },
            StatFilter::at_least("explicit.stat_life", 80.0, false),
        ],
        ..Default::default()
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
        body["query"]["filters"]["type_filters"]["filters"].get("category").is_none(),
        "category disabled -> no category, got {}",
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
    assert_eq!(mana.value.min, Some(59.0));
    assert_eq!(mana.value.max, None, "upgrades are open-ended above the current roll");
    assert!(!mana.disabled, "every kept mod constrains the search");
    let fire = q.filters.iter().find(|f| f.id == "explicit.stat_3372524247").expect("fire filter");
    assert_eq!(fire.value.min, Some(23.0));
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
    assert_eq!(phys.value.min, Some(157.0), "current roll, not the 155 tier floor");
    // Decimal rolls keep their fraction: crafted crit is +3.48(3.11-3.8)%.
    let crit = q.filters.iter().find(|f| f.id == "explicit.stat_518292764").expect("crit filter");
    assert_eq!(crit.value.min, Some(3.48));
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
    assert_eq!(q.filters[0].value.min, Some(59.0));
}

#[test]
fn upgrade_title_names_the_item_class() {
    let item = parse_item(BOW).expect("parse");
    assert_eq!(upgrade_title(&item), "upgrades: Bows");
}

use khaloni_poe2_core::trade::{parse_fetch, parse_search, Endpoint, Limiters, TradeClient, TradeError};

/// A client on its own limiters: these tests ban and saturate them, which
/// must not leak into the process-wide set other tests draw on.
fn isolated_client() -> TradeClient {
    TradeClient::with_limiters("http://127.0.0.1:9", "Runes of Aldur", Limiters::new()).expect("client")
}

#[test]
fn parses_recorded_search_and_fetch_payloads() {
    let s = parse_search(include_str!("fixtures/trade_search.json")).expect("search fixture");
    assert_eq!(s.id, "D6OM49MVf5");
    assert_eq!(s.hashes.len(), 2);

    // The trimmed fixture carries no total; that is not an error.
    assert_eq!(s.total, None);
    let fetched = parse_fetch(include_str!("fixtures/trade_fetch.json")).expect("fetch fixture");
    assert_eq!(fetched.dropped, 0);
    let listings = fetched.listings;
    assert_eq!(listings.len(), 2);
    assert_eq!(listings[0].price_currency, "transmute");
    assert_eq!(listings[0].account, "Zubmission101#7022");
    assert_eq!(listings[1].price_amount, 2.5);
    assert_eq!(listings[1].item_name, "Storm Call");
}

#[test]
fn unreachable_host_is_an_http_error_not_a_panic() {
    let mut c = isolated_client();
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
    let mut c = isolated_client();
    c.limiters().apply_state(Endpoint::Search, "ip", "1:10:60");
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
    let mut c = isolated_client();
    c.set_session("testsession");
    c.limiters().apply_state(Endpoint::Search, "ip", "1:10:60");
    match c.saved_search_ids("Runes of Aldur", "D6OM49MVf5") {
        Err(TradeError::Cooldown(d)) => assert!(d.as_secs() >= 59),
        other => panic!("expected Cooldown, got {other:?}"),
    }
}

#[test]
fn saved_search_against_unreachable_host_is_an_http_error() {
    let mut c = isolated_client();
    c.set_session("testsession");
    match c.saved_search_ids("Runes of Aldur", "D6OM49MVf5") {
        Err(TradeError::Http(_)) => {}
        other => panic!("expected Http error, got {other:?}"),
    }
}

#[test]
fn relaxing_a_query_lowers_only_live_minimums() {
    // Weapon bounds relax with everything else.
    let q = Query {
        equipment: Some(
            EquipmentFilters::default()
                .with(EquipKey::Dps, 400.0)
                .with(EquipKey::Aps, 1.5)
                .with(EquipKey::RuneSockets, 3.0),
        ),
        ..Default::default()
    };
    let relaxed = khaloni_poe2_core::trade::relax_query(&q, 0.10);
    let w = relaxed.equipment.unwrap();
    assert!((w.get(EquipKey::Dps).unwrap() - 360.0).abs() < 1e-9);
    assert!((w.get(EquipKey::Aps).unwrap() - 1.35).abs() < 1e-9);
    assert_eq!(w.get(EquipKey::Pdps), None);
    // A socket count is not a roll; "10% fewer sockets" means nothing.
    assert_eq!(w.get(EquipKey::RuneSockets), Some(3.0));

    use khaloni_poe2_core::trade::relax_query;
    let item = khaloni_poe2_core::item::parse_item(BOW).unwrap();
    let stats = StatIndex::from_json(STATS_JSON).expect("stats fixture");
    let (mut q, _) = build_query_with_labels(&item, &stats);
    assert!(!q.filters.is_empty(), "the bow fixture yields filters");
    // Disable one filter to prove a relaxation leaves it untouched.
    q.filters[0].disabled = true;
    let before: Vec<(f64, bool)> =
        q.filters.iter().map(|f| (f.value.min.expect("upgrade filters carry a minimum"), f.disabled)).collect();

    let relaxed = relax_query(&q, 0.10);
    assert_eq!(relaxed.filters.len(), q.filters.len());
    for (i, f) in relaxed.filters.iter().enumerate() {
        let (min0, disabled) = before[i];
        if disabled {
            assert_eq!(f.value.min, Some(min0), "filter {i} must be untouched");
        } else {
            assert!((f.value.min.unwrap() - min0 * 0.9).abs() < 1e-9, "filter {i} not relaxed by 10%");
        }
    }
}

#[test]
fn weapon_bounds_serialize_into_equipment_filters() {
    let mut q = Query {
        category: Some("weapon.bow".into()),
        category_enabled: true,
        ..Default::default()
    };
    // Unset weapon bounds leave no trace in the body.
    assert!(q.to_body()["query"]["filters"].get("equipment_filters").is_none());
    q.equipment = Some(EquipmentFilters::default());
    assert!(q.to_body()["query"]["filters"].get("equipment_filters").is_none());

    q.equipment = Some(
        EquipmentFilters::default()
            .with(EquipKey::Dps, 467.5)
            .with(EquipKey::Pdps, 420.0)
            .with(EquipKey::Aps, 1.1),
    );
    let body = q.to_body();
    let eq = &body["query"]["filters"]["equipment_filters"];
    // The trade2 section name: "weapon_filters" is PoE1's and is rejected.
    assert_eq!(eq["filters"]["dps"]["min"], 467.5);
    assert_eq!(eq["filters"]["pdps"]["min"], 420);
    assert_eq!(eq["filters"]["aps"]["min"], 1.1);
    // Bounds are minimums only, and unset keys are absent, not null.
    assert!(eq["filters"]["dps"].get("max").is_none());
    assert!(eq["filters"].get("edps").is_none());
    assert!(eq["filters"].get("crit").is_none());
}

// --- regressions from the 2026-09 bug hunt ---

#[test]
fn absorbed_rate_rules_keep_the_request_history() {
    use khaloni_poe2_core::trade::{RateDecision, RateLimiter};
    let mut rl = RateLimiter::from_header("5:10:60");
    for _ in 0..5 {
        rl.record();
    }
    // Every trade response carries x-rate-limit-ip; absorbing it must not
    // forget the five requests just sent (the old code rebuilt the limiter
    // from scratch, so the burst rule never fired client-side).
    rl.set_rules("5:10:60,15:60:300");
    assert!(matches!(rl.check(), RateDecision::Wait(_)), "history survives a rules refresh");

    // The server's own count in x-rate-limit-ip-state tops the history up
    // when it has seen more than we recorded (another client on the same IP).
    let mut fresh = RateLimiter::from_header("5:10:60");
    fresh.apply_state("4:10:0");
    assert!(matches!(fresh.check(), RateDecision::Ready), "4 of 5 used leaves room");
    fresh.apply_state("5:10:0");
    assert!(matches!(fresh.check(), RateDecision::Wait(_)), "5 of 5 used means wait");
}

#[test]
fn every_gear_class_maps_to_its_live_trade_category() {
    use khaloni_poe2_core::trade::category_for;
    // Ids from /api/trade2/data/filters (fetched 2026-09-08).
    for (class, cat) in [
        ("Quarterstaves", "weapon.warstaff"),
        ("Crossbows", "weapon.crossbow"),
        ("Body Armours", "armour.chest"),
        ("Foci", "armour.focus"),
        ("Bucklers", "armour.buckler"),
        ("Life Flasks", "flask.life"),
        ("Charms", "flask.charm"),
        ("Waystones", "map.waystone"),
        ("Tablets", "map.tablet"),
        ("Relics", "sanctum.relic"),
        ("Skill Gems", "gem.activegem"),
        ("Support Gems", "gem.supportgem"),
        ("Jewels", "jewel"),
    ] {
        assert_eq!(category_for(class).as_deref(), Some(cat), "{class}");
    }
    assert_eq!(category_for("Stackable Currency"), None, "currency is not gear");
}

// --- two-line affixes: local-id twins and multi-line catalog stats ---

const HYBRID_CHEST: &str = "Item Class: Body Armours\nRarity: Rare\nGrim Guardian\nVaal Cuirass\n\
    --------\nItem Level: 80\n--------\n\
    { Prefix Modifier \"Girded\" (Tier: 3) \u{2014} Defences }\n\
    +90(86-102) to Armour\n+85(79-94) to Evasion Rating\n\
    { Prefix Modifier \"Healthy\" (Tier: 5) }\n+45(40-49) to maximum Life\n--------\n";

#[test]
fn a_gear_line_resolves_to_its_global_id_and_its_local_twin() {
    let index = StatIndex::from_json(STATS_JSON).unwrap();
    // The catalog indexes armour on a chest under "# to Armour (Local)" and
    // armour on anything else under "# to Armour"; the item text is the same
    // line either way, so both ids are candidates.
    let ids: Vec<&str> = index.resolve_all("+90(86-102) to Armour").iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, vec!["explicit.stat_809229260", "explicit.stat_3484657501"]);
    // The primary stays the plain entry, so `resolve` is unchanged.
    assert_eq!(index.resolve("+90(86-102) to Armour").unwrap().id, "explicit.stat_809229260");
    // A catalog text listed twice (two explicit "# to Spirit" ids) yields
    // both, in catalog order.
    let ids: Vec<&str> = index.resolve_all("+12 to Spirit").iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, vec!["explicit.stat_3981240776", "explicit.stat_2704225257"]);
    // A line with a single id yields just that.
    let ids: Vec<&str> = index.resolve_all("+45 to maximum Life").iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, vec!["explicit.stat_3299347043"]);
    assert!(index.resolve_all("not a mod").is_empty());
}

#[test]
fn a_multi_id_filter_is_a_count_group_over_all_its_ids() {
    use khaloni_poe2_core::trade::StatFilter;
    // A hybrid chest's flat armour and evasion are each indexed under a
    // global and a local id; its life has one.
    let twin = |id: &str, alt: &str, min: f64| StatFilter {
        alt_ids: vec![alt.to_string()],
        ..StatFilter::at_least(id, min, true)
    };
    let mut q = Query {
        filters: vec![
            twin("explicit.stat_809229260", "explicit.stat_3484657501", 86.0),
            twin("explicit.stat_2144192055", "explicit.stat_53045048", 76.0),
            StatFilter::at_least("explicit.stat_3299347043", 40.0, true),
        ],
        ..Default::default()
    };
    // Verified live 2026-09-10: on body armours the global armour id finds
    // nothing and the local one finds thousands, and a `count >= 1` group
    // over both matches exactly what the local id alone matches. So a
    // multi-id filter leaves the "and" group and becomes its own count
    // group, every id carrying the same bound.
    q.filters[0].disabled = false;
    let body = q.to_body();
    let stats_groups = body["query"]["stats"].as_array().unwrap();
    assert_eq!(stats_groups.len(), 3, "and group + one count group per multi-id filter");
    let and_ids: Vec<&str> = stats_groups[0]["filters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["id"].as_str().unwrap())
        .collect();
    assert_eq!(and_ids, vec!["explicit.stat_3299347043"], "single-id filters stay in the and group");
    let armour = &stats_groups[1];
    assert_eq!(armour["type"], "count");
    assert_eq!(armour["value"]["min"], 1);
    assert_eq!(armour["disabled"], false);
    let members: Vec<(&str, i64)> = armour["filters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| (f["id"].as_str().unwrap(), f["value"]["min"].as_i64().unwrap()))
        .collect();
    assert_eq!(members, vec![("explicit.stat_809229260", 86), ("explicit.stat_3484657501", 86)]);
    // A switched-off multi-id filter is a switched-off group, not a
    // constraint the site still applies.
    assert_eq!(stats_groups[2]["type"], "count");
    assert_eq!(stats_groups[2]["disabled"], true, "evasion is not preselected");
    for m in stats_groups[2]["filters"].as_array().unwrap() {
        assert_eq!(m["disabled"], true, "EE2 marks every member of the group, not only the group");
    }
    // The relaxed bound reaches every member.
    let relaxed = khaloni_poe2_core::trade::relax_query(&q, 0.5).to_body();
    for m in relaxed["query"]["stats"][1]["filters"].as_array().unwrap() {
        assert_eq!(m["value"]["min"], 43);
    }
}

#[test]
fn upgrade_query_carries_local_twins_too() {
    let stats = StatIndex::from_json(STATS_JSON).unwrap();
    let item = parse_item(HYBRID_CHEST).unwrap();
    let q = build_upgrade_query(&item, &stats);
    let armour = q.filters.iter().find(|f| f.id == "explicit.stat_809229260").expect("armour filter");
    assert_eq!(armour.alt_ids, vec!["explicit.stat_3484657501"]);
    assert_eq!(armour.value.min, Some(90.0), "current roll, not tier floor");
    let group = &q.to_body()["query"]["stats"][1];
    assert_eq!(group["type"], "count");
    assert_eq!(group["filters"][1]["id"], "explicit.stat_3484657501");
    assert_eq!(group["filters"][1]["value"]["min"], 90);
}

#[test]
fn a_stat_whose_catalog_text_spans_two_lines_is_one_filter() {
    let stats = StatIndex::from_json(STATS_JSON).unwrap();
    let text = "Item Class: Wands\nRarity: Rare\nTest\nBone Wand\n--------\nItem Level: 80\n--------\n\
        { Prefix Modifier \"Circular\" (Tier: 1) }\n\
        Spells fire 2 additional Projectiles\nSpells fire Projectiles in a circle\n\
        { Suffix Modifier \"of the Hoard\" (Tier: 1) }\n+45(40-49) to maximum Mana\n--------\n";
    let item = parse_item(text).unwrap();
    assert_eq!(item.explicits.len(), 3, "the parser keeps one line per entry");
    let (q, labels) = build_query_with_labels(&item, &stats);
    let ids: Vec<&str> = q.filters.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(ids, vec!["explicit.stat_1013492127", "explicit.stat_1050105434"]);
    assert_eq!(q.filters[0].value.min, Some(2.0));
    assert_eq!(
        labels[0].text,
        "Spells fire 2 additional Projectiles / Spells fire Projectiles in a circle"
    );
    assert_eq!(labels[0].tier, Some(1));
    assert_eq!(labels[0].rolled, Some(2.0));
}

#[test]
fn the_two_line_stat_beats_its_single_line_lookalike() {
    let stats = StatIndex::from_json(STATS_JSON).unwrap();
    // "#% increased Rarity of Items found" exists alone (stat_3917489142)
    // and as the first line of a two-line stat whose second line changes
    // its meaning. The pair must resolve to the two-line stat, never to the
    // plain rarity id with the second line dropped on the floor.
    let text = "Item Class: Rings\nRarity: Rare\nTest\nGold Ring\n--------\nItem Level: 80\n--------\n\
        { Prefix Modifier \"Greedy\" (Tier: 1) }\n\
        60% increased Rarity of Items found\nYour other Modifiers to Rarity of Items found do not apply\n\
        --------\n";
    let item = parse_item(text).unwrap();
    let (q, labels) = build_query_with_labels(&item, &stats);
    assert_eq!(q.filters.len(), 1);
    assert_eq!(q.filters[0].id, "explicit.stat_1602191394");
    // The catalog lists that two-line text twice; the duplicate rides along.
    assert_eq!(q.filters[0].alt_ids, vec!["explicit.stat_2261942307"]);
    assert_eq!(q.filters[0].value.min, Some(60.0));
    assert_eq!(labels.len(), 1);
}

#[test]
fn multi_line_catalog_text_matches_regardless_of_stray_line_padding() {
    let stats = StatIndex::from_json(STATS_JSON).unwrap();
    // The catalog carries "Increases and Reductions to\n Fire and ..." with
    // a space opening the second line; the parsed item line has none.
    let text = "Item Class: Jewels\nRarity: Rare\nTest\nEmerald\n--------\nItem Level: 80\n--------\n\
        { Prefix Modifier \"Transforming\" (Tier: 1) }\n\
        Increases and Reductions to\nFire and Lightning Damage in Radius are transformed to apply to Cold Damage\n\
        --------\n";
    let item = parse_item(text).unwrap();
    let ids: Vec<&str> = stats
        .resolve_all("Increases and Reductions to\nFire and Lightning Damage in Radius are transformed to apply to Cold Damage")
        .iter()
        .map(|e| e.id.as_str())
        .collect();
    assert_eq!(ids, vec!["explicit.stat_3368921525"]);
    // No number on either line, so the builder skips it (mirrors the
    // valueless-mod rule); the point is that the pair resolved as a unit.
    let (q, _) = build_query_with_labels(&item, &stats);
    assert!(q.filters.is_empty());
}

#[test]
fn lines_under_different_affix_headers_are_never_joined() {
    let stats = StatIndex::from_json(STATS_JSON).unwrap();
    // The same two lines, but copied under two headers: two affixes, so the
    // first resolves alone to the plain rarity stat and the second is an
    // unknown line on its own.
    let text = "Item Class: Rings\nRarity: Rare\nTest\nGold Ring\n--------\nItem Level: 80\n--------\n\
        { Prefix Modifier \"Greedy\" (Tier: 1) }\n60% increased Rarity of Items found\n\
        { Suffix Modifier \"of Oddity\" (Tier: 1) }\nYour other Modifiers to Rarity of Items found do not apply\n\
        --------\n";
    let item = parse_item(text).unwrap();
    let (q, _) = build_query_with_labels(&item, &stats);
    let ids: Vec<&str> = q.filters.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(ids, vec!["explicit.stat_3917489142"]);
}

#[test]
fn a_long_windows_server_count_does_not_trip_the_short_rules() {
    // Live 2026-09-19: the search policy carries a six-hour rule, and after
    // a restart the server's count for it (47 here) is far above what this
    // limiter has recorded. Those requests happened hours ago; counting the
    // difference as sent just now put a fresh client straight into a
    // five-minute cooldown on its first price check.
    let mut l = RateLimiter::from_header("5:10:60,15:60:300,30:300:1800,600:21600:3600");
    l.apply_state("1:10:0,1:60:0,1:300:0,47:21600:0");
    assert_eq!(l.check(), RateDecision::Ready);
    // The six-hour rule itself still learns from the server: near its cap
    // it must hold the client back.
    let mut near = RateLimiter::from_header("5:10:60,600:21600:3600");
    near.apply_state("1:10:0,600:21600:0");
    assert!(matches!(near.check(), RateDecision::Wait(_)));
}

// --- search total, fetch, exchange rate and Broad regressions (2026-09) ---

#[test]
fn the_search_total_is_kept() {
    let s = parse_search(r#"{"id":"abc","complexity":6,"result":["h1","h2"],"total":3187,"inexact":false}"#)
        .expect("parses");
    assert_eq!(s.total, Some(3187));
    assert_eq!(s.hashes.len(), 2);
}

#[test]
fn gone_and_unpriced_listings_are_counted_not_fatal() {
    // A listing that sold between the search and the fetch comes back as a
    // bare null, which used to fail the whole page; one without a price was
    // dropped without a trace.
    let body = r#"{"result":[
        null,
        {"id":"a","listing":{"indexed":"2026-09-01T00:00:00Z","account":{"name":"A#1"},"price":{"type":"~price","amount":5,"currency":"exalted"}},"item":{"name":"","baseType":"Gold Ring"}},
        {"id":"b","listing":{"indexed":"2026-09-01T00:00:00Z","account":{"name":"B#2"},"price":null},"item":{"name":"","baseType":"Gold Ring"}}
    ]}"#;
    let out = parse_fetch(body).expect("a null entry is not a parse error");
    assert_eq!(out.listings.len(), 1);
    assert_eq!(out.listings[0].account, "A#1");
    assert_eq!(out.dropped, 2);
}

#[test]
fn a_fetch_keeps_its_raw_entries_including_the_nulls() {
    // The listings table and the hover card read fields the `Listing`
    // summary never carried (mods, tiers, online state, fee), so the entries
    // are kept as received, in the order the search listed them, with the
    // API's null where a listing had gone.
    const FULL: &str = include_str!("fixtures/trade_fetch_full.json");
    let out = parse_fetch(FULL).expect("the full fixture parses");
    assert_eq!(out.raw.len(), 18, "one slot per requested hash");
    assert!(out.raw[2].is_none(), "the gone listing stays a null in its slot");
    assert_eq!(out.raw.iter().flatten().count(), 17);
    // A priceless listing is still an entry: it is dropped from the priced
    // summary, not from the raw page.
    let unpriced = out.raw[7].as_ref().expect("entry present");
    assert!(unpriced["listing"]["price"].is_null());
    assert_eq!(out.listings.len(), 16);
    assert_eq!(out.dropped, 2);
    // The entries are the API's own objects, untouched.
    let first = out.raw[0].as_ref().expect("entry present");
    assert_eq!(
        first["id"].as_str(),
        Some("7e41d18eb53e91a2a3434bbf4400515928d81445fbc32ce9abb59d01bc32cc84")
    );
    assert!(first["item"]["explicitMods"].is_array(), "the item block rides along whole");
}

fn exchange_body(rates: &[(f64, f64)]) -> String {
    let listings: Vec<String> = rates
        .iter()
        .enumerate()
        .map(|(i, (pay, get))| {
            format!(
                r#""l{i}":{{"listing":{{"offers":[{{"exchange":{{"currency":"exalted","amount":{pay}}},"item":{{"currency":"omen","amount":{get}}}}}]}}}}"#
            )
        })
        .collect();
    format!(r#"{{"result":{{{}}}}}"#, listings.join(","))
}

#[test]
fn one_bait_offer_does_not_set_the_exchange_rate() {
    use khaloni_poe2_core::trade::parse_exchange_rate;
    // A single 1 ex offer under a market sitting at 40-44 ex.
    let body = exchange_body(&[(1.0, 1.0), (40.0, 1.0), (41.0, 1.0), (42.0, 1.0), (44.0, 1.0), (90.0, 1.0)]);
    let rate = parse_exchange_rate(&body).expect("offers present");
    assert_eq!(rate, 41.0, "median of the five cheapest: 1, 40, 41, 42, 44");
    // Bulk offers are compared per unit.
    let body = exchange_body(&[(400.0, 10.0), (41.0, 1.0), (84.0, 2.0)]);
    assert_eq!(parse_exchange_rate(&body), Some(41.0));
    // One or two offers: the median of what there is.
    assert_eq!(parse_exchange_rate(&exchange_body(&[(40.0, 1.0)])), Some(40.0));
    assert_eq!(parse_exchange_rate(&exchange_body(&[(40.0, 1.0), (44.0, 1.0)])), Some(42.0));
    // A zero amount on either side is not an offer.
    assert_eq!(parse_exchange_rate(&exchange_body(&[(0.0, 1.0), (40.0, 0.0)])), None);
}

use khaloni_poe2_core::trade::{FilterValue, StatFilter};

fn stat_filter(id: &str, min: Option<f64>, max: Option<f64>) -> StatFilter {
    StatFilter { value: FilterValue { min, max }, ..StatFilter::at_least(id, 0.0, false) }
}

#[test]
fn broad_widens_every_bound_in_the_direction_that_admits_more() {
    use khaloni_poe2_core::trade::{relax_query, FilterRole};
    let q = Query {
        filters: vec![
            stat_filter("explicit.stat_positive_min", Some(20.0), None),
            // A "reduced" stat the site indexes negated: min -20 means "at
            // least 20% reduced"; looser is -22.
            stat_filter("explicit.stat_negative_min", Some(-20.0), None),
            // Lower is better: only a maximum, which loosens upward.
            stat_filter("explicit.stat_max_only", None, Some(30.0)),
            stat_filter("explicit.stat_negative_max", None, Some(-10.0)),
            // A flag stat carries no bound at all.
            stat_filter("explicit.stat_flag", None, None),
            StatFilter { disabled: true, ..stat_filter("explicit.stat_disabled", Some(50.0), Some(60.0)) },
            // Counts are not rolls.
            stat_filter("pseudo.pseudo_number_of_unrevealed_mods", Some(2.0), None),
            stat_filter("pseudo.pseudo_number_of_uses_remaining", Some(3.0), None),
            StatFilter {
                role: FilterRole::EmptyModifier,
                ..stat_filter("pseudo.pseudo_number_of_empty_prefix_mods", Some(1.0), Some(1.0))
            },
        ],
        ..Default::default()
    };
    let r = relax_query(&q, 0.10);
    let bounds: Vec<(Option<f64>, Option<f64>)> = r.filters.iter().map(|f| (f.value.min, f.value.max)).collect();
    let close = |a: Option<f64>, b: Option<f64>| match (a, b) {
        (Some(a), Some(b)) => (a - b).abs() < 1e-9,
        (None, None) => true,
        _ => false,
    };
    let want = [
        (Some(18.0), None),
        (Some(-22.0), None),
        (None, Some(33.0)),
        (None, Some(-9.0)),
        (None, None),
        (Some(50.0), Some(60.0)),
        (Some(2.0), None),
        (Some(3.0), None),
        (Some(1.0), Some(1.0)),
    ];
    for (i, (got, want)) in bounds.iter().zip(want).enumerate() {
        assert!(close(got.0, want.0) && close(got.1, want.1), "filter {i}: got {got:?}, want {want:?}");
    }
    // Every relaxed range contains the range it came from.
    for (before, after) in q.filters.iter().zip(&r.filters) {
        if let (Some(b), Some(a)) = (before.value.min, after.value.min) {
            assert!(a <= b);
        }
        if let (Some(b), Some(a)) = (before.value.max, after.value.max) {
            assert!(a >= b);
        }
    }
}

// --- the anonymous search limit (measured live 2026-09-30) ---

fn ee2_db() -> khaloni_poe2_core::ee2::Ee2Data {
    use khaloni_poe2_core::ee2::{data, Ee2Data};
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tools/ee2-parity/data");
    let read = |f: &str| std::fs::read_to_string(dir.join(f)).unwrap();
    let mut db = Ee2Data::from_ndjson(&read("stats.ndjson"), &read("items.ndjson")).unwrap();
    db.trade_stats = Some(data::TradeStatTexts::from_json(&read("trade-stats.json")).unwrap());
    db.trade_items = Some(data::trade_item_names(&read("trade-items.json")).unwrap());
    db
}

fn tablet_query() -> Query {
    let text = include_str!("fixtures/item-tablet-mythical-instigation.txt");
    khaloni_poe2_core::ee2::build(text, &ee2_db()).expect("the tablet builds").query
}

/// A local stand-in for the search endpoint that keeps every request it
/// was sent, so a test sees the body that left without any real traffic.
fn search_stub() -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { break };
            let mut req = Vec::new();
            let mut buf = [0u8; 16384];
            // Read the head, then as much body as Content-Length says.
            loop {
                let n = s.read(&mut buf).unwrap_or(0);
                req.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&req).to_string();
                if let Some(head_end) = text.find("\r\n\r\n") {
                    let len = text[..head_end]
                        .lines()
                        .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
                        .unwrap_or(0);
                    if req.len() >= head_end + 4 + len || n == 0 {
                        log.lock().unwrap().push(text[head_end + 4..].to_string());
                        break;
                    }
                } else if n == 0 {
                    break;
                }
            }
            let body = r#"{"id":"stub","complexity":1,"result":[],"total":0}"#;
            let _ = write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    (base, seen)
}

fn stat_groups(body: &serde_json::Value) -> Vec<serde_json::Value> {
    body["query"]["stats"].as_array().cloned().unwrap_or_default()
}

fn group_ids(g: &serde_json::Value) -> Vec<String> {
    let mut ids: Vec<String> =
        g["filters"].as_array().unwrap().iter().map(|f| f["id"].as_str().unwrap().to_string()).collect();
    ids.sort();
    ids
}

#[test]
fn an_anonymous_search_merges_twin_groups_to_three_or_fewer() {
    use khaloni_poe2_core::trade::{search_body, ANONYMOUS_STAT_GROUPS};
    let q = tablet_query();
    let built = q.to_body();
    let groups = stat_groups(&built);
    // EE2's body for this tablet: "and" plus three twin groups, which the
    // site refused without a session.
    assert_eq!(groups.len(), 4);
    let twins: Vec<_> = groups.iter().filter(|g| g["type"] == "count").collect();
    assert_eq!(twins.len(), 3);

    let sent = search_body(&q, true).expect("fits once merged");
    let sent_groups = stat_groups(&sent);
    assert!(sent_groups.len() <= ANONYMOUS_STAT_GROUPS, "{sent_groups:?}");
    let and: Vec<_> = sent_groups.iter().filter(|g| g["type"] == "and").collect();
    assert_eq!(and, groups.iter().filter(|g| g["type"] == "and").collect::<Vec<_>>(), "the and group is untouched");
    let counts: Vec<_> = sent_groups.iter().filter(|g| g["type"] == "count").collect();
    assert_eq!(counts.len(), 1);
    assert_eq!(counts[0]["value"], serde_json::json!({"min": 3}));
    assert_eq!(counts[0]["disabled"], false);
    let mut all_twin_ids: Vec<String> = twins.iter().flat_map(|g| group_ids(g)).collect();
    all_twin_ids.sort();
    assert_eq!(group_ids(counts[0]), all_twin_ids, "the same six ids at their own bounds");
    // Everything outside the stat groups is as built.
    let mut rest = sent.clone();
    let mut built_rest = built.clone();
    rest["query"]["stats"] = serde_json::Value::Null;
    built_rest["query"]["stats"] = serde_json::Value::Null;
    assert_eq!(rest, built_rest);

    // The client without a session sends exactly that.
    let (base, seen) = search_stub();
    let mut c = TradeClient::with_limiters(&base, "Rise of the Abyssal", Limiters::new()).unwrap();
    c.search(&q).expect("the stub answers");
    let sent_by_client: serde_json::Value = serde_json::from_str(&seen.lock().unwrap()[0]).unwrap();
    assert_eq!(sent_by_client, sent);

    // A body that still needs four groups is refused before it leaves,
    // with the way out named.
    let mut too_many = q.clone();
    for (i, not) in ["explicit.stat_1", "explicit.stat_2"].into_iter().enumerate() {
        let mut f = StatFilter::at_least(format!("explicit.stat_10{i}"), 1.0, false);
        f.not_id = Some(not.to_string());
        too_many.filters.push(f);
    }
    let err = search_body(&too_many, true).expect_err("four groups after merging");
    let why = err.to_string();
    assert!(why.contains("POESESSID") && why.contains("Settings -> Account"), "{why}");
    let before = seen.lock().unwrap().len();
    assert!(c.search(&too_many).is_err());
    assert_eq!(seen.lock().unwrap().len(), before, "a search the site would refuse was sent");
}

#[test]
fn a_session_search_is_sent_as_built() {
    use khaloni_poe2_core::trade::search_body;
    let q = tablet_query();
    assert_eq!(search_body(&q, false).unwrap(), q.to_body());
    let (base, seen) = search_stub();
    let mut c = TradeClient::with_limiters(&base, "Rise of the Abyssal", Limiters::new()).unwrap();
    c.set_session("testsession");
    c.search(&q).expect("the stub answers");
    let sent: serde_json::Value = serde_json::from_str(&seen.lock().unwrap()[0]).unwrap();
    assert_eq!(sent, q.to_body());
    assert_eq!(stat_groups(&sent).len(), 4);
}
