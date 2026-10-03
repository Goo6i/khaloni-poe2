//! The flip finder on real data: profiles and their file, the scan's
//! searches (the resale one and one per relaxation, each a body the trade
//! site takes), the request cost stated before a scan, resale read from
//! the real fetch fixture counting only listings that meet the profile,
//! and candidates costed by the planner and ranked by margin.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use khaloni_poe2_core::craft::data::CraftData;
use khaloni_poe2_core::craft::model::Model;
use khaloni_poe2_core::craft::sim::SimConfig;
use khaloni_poe2_core::craft::types::{AffixKind, Source};
use khaloni_poe2_core::ee2::data::{Ee2Data, StatDb};
use khaloni_poe2_core::flip::{
    self, candidate, card_state, margin, meets, profiles_from_toml, rank, relaxations, resale, resolve, scan_cost,
    to_toml, Market, Planner, Profile, Relaxation, RequestCost, Resale, Resolved, Wanted, DEFAULT_MARGIN,
    EMPTY_PREFIX_ID, EMPTY_SUFFIX_ID,
};
use khaloni_poe2_core::trade::StatIndex;
use serde_json::Value;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn ee2_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../tools/ee2-parity/data")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn craft() -> &'static CraftData {
    static DATA: OnceLock<CraftData> = OnceLock::new();
    DATA.get_or_init(|| {
        let dir = fixtures();
        CraftData::load(
            &read(&dir.join("craft/mods_slice.json")),
            &read(&dir.join("craft_base_items_sample.json")),
            &read(&dir.join("craft/essences_slice.json")),
        )
        .expect("the craft fixtures load")
    })
}

fn ee2() -> &'static Ee2Data {
    static DATA: OnceLock<Ee2Data> = OnceLock::new();
    DATA.get_or_init(|| {
        let dir = ee2_dir();
        Ee2Data::from_ndjson(&read(&dir.join("stats.ndjson")), &read(&dir.join("items.ndjson"))).expect("pinned ndjson parses")
    })
}

fn stats() -> &'static StatDb {
    &ee2().stats
}

fn catalog() -> &'static StatIndex {
    static INDEX: OnceLock<StatIndex> = OnceLock::new();
    INDEX.get_or_init(|| StatIndex::from_json(&read(&ee2_dir().join("trade-stats.json"))).expect("the trade catalog parses"))
}

/// Every stat id of the trade site's catalog (the pinned
/// `/api/trade2/data/stats` response).
fn trade_ids() -> &'static HashSet<String> {
    static IDS: OnceLock<HashSet<String>> = OnceLock::new();
    IDS.get_or_init(|| {
        let body: Value = serde_json::from_str(&read(&ee2_dir().join("trade-stats.json"))).unwrap();
        body["result"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|g| g["entries"].as_array().unwrap().iter().map(|e| e["id"].as_str().unwrap().to_string()))
            .collect()
    })
}

/// The real fetch response: body armours priced in divines.
fn fetched() -> Vec<Value> {
    let body: Value = serde_json::from_str(&read(&fixtures().join("trade_fetch_full.json"))).unwrap();
    body["result"].as_array().unwrap().clone()
}

fn listing(id_prefix: &str) -> Value {
    fetched()
        .into_iter()
        .find(|e| e["id"].as_str().is_some_and(|id| id.starts_with(id_prefix)))
        .unwrap_or_else(|| panic!("no listing {id_prefix} in the fixture"))
}

fn want(family: &str, min_tier: u8) -> Wanted {
    Wanted { family: family.to_string(), min_tier }
}

/// A body armour with increased armour at T2 or better and fire
/// resistance at T3 or better (31% and up on body armours).
fn cuirass() -> Profile {
    Profile {
        name: "Armour and fire cuirass".to_string(),
        class: "Body Armour".to_string(),
        base: None,
        min_ilvl: 75,
        wants: vec![want("LocalPhysicalDamageReductionRatingPercent", 2), want("FireResistance", 3)],
        margin: DEFAULT_MARGIN,
    }
}

fn resolved(profile: &Profile) -> Resolved {
    resolve(profile, craft(), stats(), catalog()).unwrap_or_else(|e| panic!("{} resolves: {e:?}", profile.name))
}

fn in_divines(amount: f64, currency: &str) -> Option<f64> {
    (currency == "divine").then_some(amount)
}

fn market() -> Market<'static> {
    Market { convert: &in_divines, unit: "div" }
}

/// Every currency the planner can name, at a flat hundredth of a divine.
fn flat_prices(_: &str) -> Option<f64> {
    Some(0.01)
}

fn no_prices(_: &str) -> Option<f64> {
    None
}

fn small_runs() -> SimConfig {
    SimConfig { runs: 200, ..SimConfig::default() }
}

fn resale_of(prices: &[f64]) -> Resale {
    Resale {
        cheapest: prices.first().copied(),
        prices: prices.to_vec(),
        listings_counted: prices.len(),
        below_profile: 0,
        not_counted: 0,
        unit: "div".to_string(),
        text: String::new(),
    }
}

// --- the scan's searches ---

#[test]
fn a_profile_yields_one_resale_and_one_relaxed_query_per_relaxation() {
    let r = resolved(&cuirass());
    assert_eq!(r.wants[0].kind, AffixKind::Prefix);
    assert_eq!(r.wants[1].kind, AffixKind::Suffix);
    assert_eq!(r.wants[1].lowest_tier, 8, "fire resistance has eight tiers on body armours");

    let scan = relaxations(&r);
    let kinds: Vec<&Relaxation> = scan.queries.iter().map(|q| &q.relaxation).collect();
    assert_eq!(
        kinds,
        vec![
            &Relaxation::Resale,
            &Relaxation::Dropped("LocalPhysicalDamageReductionRatingPercent".to_string()),
            &Relaxation::Dropped("FireResistance".to_string()),
            &Relaxation::Lowered { family: "LocalPhysicalDamageReductionRatingPercent".to_string(), tier: 3 },
            &Relaxation::Lowered { family: "FireResistance".to_string(), tier: 4 },
            &Relaxation::OpenAffix,
        ]
    );
    assert!(scan.skipped.is_empty(), "{:?}", scan.skipped);
    let labels: Vec<&str> = scan.queries.iter().map(|q| q.label.as_str()).collect();
    assert_eq!(
        labels,
        vec![
            "resale: the finished profile",
            "without #% increased Armour",
            "without #% to Fire Resistance",
            "#% increased Armour at T3 instead of T2",
            "#% to Fire Resistance at T4 instead of T3",
            "with an open prefix or suffix",
        ]
    );

    // The floors are the low end of each tier's range: increased armour
    // T2 is 92-100%, T3 80-91%; fire resistance T3 is 31-35%, T4 26-30%.
    let mins = |i: usize| -> Vec<(String, f64)> {
        scan.queries[i].query.filters.iter().map(|f| (f.id.clone(), f.value.min.unwrap())).collect()
    };
    let armour = "explicit.stat_1062208444".to_string();
    let fire = "explicit.stat_3372524247".to_string();
    assert_eq!(mins(0), vec![(armour.clone(), 92.0), (fire.clone(), 31.0)]);
    assert_eq!(mins(1), vec![(fire.clone(), 31.0)]);
    assert_eq!(mins(2), vec![(armour.clone(), 92.0)]);
    assert_eq!(mins(3), vec![(armour.clone(), 80.0), (fire.clone(), 31.0)]);
    assert_eq!(mins(4), vec![(armour.clone(), 92.0), (fire.clone(), 26.0)]);
    assert_eq!(mins(5), vec![(armour, 92.0), (fire, 31.0), (EMPTY_PREFIX_ID.to_string(), 1.0)]);
    assert_eq!(scan.queries[5].query.filters[2].alt_ids, vec![EMPTY_SUFFIX_ID.to_string()]);

    // A want at its family's lowest tier is not lowered, and the scan says
    // so; the count drops by exactly that one search.
    let mut lowest = cuirass();
    lowest.wants[1].min_tier = 8;
    let scan = relaxations(&resolved(&lowest));
    assert_eq!(scan.queries.len(), 5);
    assert!(!scan.queries.iter().any(|q| matches!(&q.relaxation, Relaxation::Lowered { family, .. } if family == "FireResistance")));
    assert_eq!(
        scan.skipped,
        vec!["#% to Fire Resistance is already at its lowest tier (T8) on Body Armour, so it is not lowered".to_string()]
    );

    // One base: the base is searched instead of the category, and tiers
    // are that base's.
    let mut soldier = cuirass();
    soldier.base = Some("Soldier Cuirass".to_string());
    let scan = relaxations(&resolved(&soldier));
    let body = scan.queries[0].query.to_body();
    assert_eq!(body["query"]["type"], "Soldier Cuirass");
    assert!(body["query"]["filters"]["type_filters"]["filters"].get("category").is_none());
}

#[test]
fn relaxed_queries_stay_valid_request_bodies() {
    let r = resolved(&cuirass());
    let scan = relaxations(&r);

    // What EE2 itself sent for a rare body armour searched with its item
    // level (a golden body of the parity harness): the sections, their
    // keys and the outer shape a profile search must share.
    let golden: Value =
        serde_json::from_str(&read(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../tools/ee2-parity/golden/edge-body-unrevealed-desecrated.json")))
            .unwrap();
    let reference = golden["body"].clone();
    let keys = |v: &Value| -> HashSet<String> { v.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default() };

    for q in &scan.queries {
        let body = q.query.to_body();
        let text = serde_json::to_string(&body).unwrap();
        let back: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(back, body, "{}: the body round-trips through its JSON text", q.label);

        assert_eq!(body["sort"], reference["sort"], "{}", q.label);
        assert_eq!(body["query"]["status"], reference["query"]["status"], "{}", q.label);
        let filters = &body["query"]["filters"];
        for section in ["type_filters", "misc_filters", "trade_filters"] {
            let ours = keys(&filters[section]["filters"]);
            let theirs = keys(&reference["query"]["filters"][section]["filters"]);
            assert!(!ours.is_empty(), "{}: {section} is sent", q.label);
            assert_eq!(ours, theirs, "{}: {section} carries the keys EE2 sends", q.label);
        }
        assert_eq!(filters["misc_filters"], reference["query"]["filters"]["misc_filters"], "{}", q.label);
        assert_eq!(filters["trade_filters"], reference["query"]["filters"]["trade_filters"], "{}", q.label);
        assert_eq!(filters["type_filters"]["filters"]["category"]["option"], "armour.chest");
        assert_eq!(filters["type_filters"]["filters"]["rarity"]["option"], "nonunique");
        assert_eq!(filters["type_filters"]["filters"]["ilvl"]["min"], 75);
        assert_eq!(filters["misc_filters"]["filters"]["corrupted"]["option"], "false");
        assert_eq!(filters["trade_filters"]["filters"]["collapse"]["option"], "true");

        // Stat groups: the leading "and" group, then one "count" group
        // per filter with alternate ids, as the builder sends multi-id
        // stats; every id is one the trade site's catalog lists, and
        // every bound is a whole number or a float, never a string.
        let groups = body["query"]["stats"].as_array().unwrap();
        assert_eq!(groups[0]["type"], "and");
        for g in &groups[1..] {
            assert_eq!(g["type"], "count", "{}", q.label);
            assert_eq!(g["value"]["min"], 1, "{}", q.label);
            assert_eq!(g["disabled"], false, "{}", q.label);
            let members = g["filters"].as_array().unwrap();
            assert!(members.len() >= 2, "{}", q.label);
            for m in members {
                let id = m["id"].as_str().unwrap();
                assert!(trade_ids().contains(id), "{}: {id} is in the trade stats catalog", q.label);
                assert!(m["value"]["min"].is_number(), "{}: {m}", q.label);
                assert_eq!(m["disabled"], false);
            }
        }
        let wanted = q.query.filters.len();
        assert_eq!(groups.len(), 1 + wanted, "{}: one group per filter", q.label);
    }

    // Each member of a want's group is the same stat under another mod
    // type, so a fractured or desecrated fire resistance counts too. EE2's
    // stat list also names a crafted id, but the pinned trade catalog has
    // no crafted group, and an id the site does not list would make it
    // refuse the whole search, so it is left out.
    let fire = &scan.queries[0].query.filters[1];
    assert_eq!(fire.id, "explicit.stat_3372524247");
    assert_eq!(fire.alt_ids, vec!["fractured.stat_3372524247", "desecrated.stat_3372524247"]);
    assert!(catalog().entry_by_id("crafted.stat_3372524247").is_none());
}

#[test]
fn a_family_that_cannot_be_searched_is_reported_with_its_reason() {
    let mut p = cuirass();
    p.wants = vec![
        want("ReducedPoisonDuration", 2),
        want("NoSuchFamily", 1),
        want("FireResistance", 9),
        want("IncreasedLife", 1),
    ];
    let reasons = resolve(&p, craft(), stats(), catalog()).expect_err("three of the four cannot be searched");
    assert_eq!(reasons.len(), 3, "{reasons:?}");
    assert!(reasons[0].starts_with("ReducedPoisonDuration: "), "{}", reasons[0]);
    assert!(reasons[0].contains("mirrored bounds"), "{}", reasons[0]);
    assert_eq!(reasons[1], "NoSuchFamily is not a family of the mod database");
    assert_eq!(reasons[2], "FireResistance has 8 tiers on Body Armour; tier 9 does not exist");

    // A family only a desecration adds is searched by the site's desecrated
    // ids: a listing card's desecrated lines join their entries, so such a
    // listing is counted when it meets the profile.
    let amulet = Profile {
        name: "Skill quality amulet".into(),
        class: "Amulet".into(),
        base: Some("Absent Amulet".into()),
        min_ilvl: 65,
        wants: vec![want("GlobalSkillGemQuality", 1)],
        margin: 30.0,
    };
    let r = resolved(&amulet);
    let quality = &r.wants[0];
    assert_eq!(quality.kind, khaloni_poe2_core::craft::types::AffixKind::Suffix);
    assert!(quality.lines[0].ids.iter().any(|id| id.starts_with("desecrated.")), "{:?}", quality.lines[0].ids);
    assert_eq!(quality.floor(1), Some(&[3.0][..]), "tier 1 asks at least the entry's low roll");

    let mut wrong_base = cuirass();
    wrong_base.base = Some("Gold Ring".to_string());
    let reasons = resolve(&wrong_base, craft(), stats(), catalog()).unwrap_err();
    assert_eq!(reasons, vec!["Gold Ring is a Ring, not a Body Armour".to_string()]);
}

#[test]
fn the_scan_cost_is_stated_from_the_query_count() {
    let scan = relaxations(&resolved(&cuirass()));
    let cost = scan_cost(&scan);
    assert_eq!(cost, RequestCost { searches: 6, fetches: 12 }, "twenty listings per search, ten per fetch");
    assert_eq!(cost.statement(27, 30), "this scan: 6 searches, 12 fetches; budget 27/30 free");

    assert_eq!(RequestCost::of(5), RequestCost { searches: 5, fetches: 10 });
    assert_eq!(RequestCost::of(5).statement(27, 30), "this scan: 5 searches, 10 fetches; budget 27/30 free");
    assert_eq!(RequestCost::of(1).statement(0, 30), "this scan: 1 search, 2 fetches; budget 0/30 free");
    assert_eq!(flip::LISTINGS_PER_SEARCH, 20);
}

// --- resale and margin ---

#[test]
fn resale_reads_only_listings_at_or_above_the_profile() {
    let r = resolved(&cuirass());
    let fetched = fetched();

    // Each listing of the fixture against the profile. Crafted and
    // desecrated modifiers are on the card and count: a crafted fire
    // resistance of T3 meets the want, and so does a desecrated increased
    // armour of T2.
    let cases: [(&str, bool); 8] = [
        ("7e41d18e", false), // increased armour T1, no fire resistance
        ("bb0610fc", true),  // T1 armour, crafted fire resistance S3
        ("bb2b2db5", true),  // desecrated armour P2, crafted fire S3
        ("bd6012a3", false), // desecrated armour P2, no fire resistance
        ("1ce8f868", false), // T1 armour, lightning but no fire
        ("c8929922", true),  // T1 armour, fire S2
        ("cbbb131f", false), // desecrated armour only P3
        ("4d6e6f9c", true),  // fractured item: T1 armour, fire S1
    ];
    for (id, expected) in cases {
        let state = card_state(&listing(id), craft()).unwrap_or_else(|e| panic!("{id}: {e}"));
        assert_eq!(meets(&r, &state).is_ok(), expected, "{id}: {:?}", meets(&r, &state));
    }
    let bb06 = card_state(&listing("bb0610fc"), craft()).unwrap();
    assert!(bb06.mods.iter().any(|m| m.family == "FireResistance" && m.source == Source::Crafted && m.tier == Some(3)));
    assert!(bb06.mods.iter().any(|m| m.source == Source::Desecrated));
    let fractured = card_state(&listing("4d6e6f9c"), craft()).unwrap();
    assert!(fractured.mods.iter().any(|m| m.source == Source::Fractured));

    let got = resale(&fetched, &r, craft(), &market());
    assert_eq!(got.prices, vec![3.0, 5.0, 7.0, 10.0, 10.0, 12.0]);
    assert_eq!(got.cheapest, Some(3.0));
    assert_eq!(got.listings_counted, 6);
    assert_eq!(got.below_profile, 11, "every other rare and the normal base fall short");
    assert_eq!(got.not_counted, 0);
    assert_eq!(got.text, "cheapest 3, then 5, 7, 10, 10 div · 6 listings; 11 below the profile not counted");

    // Raising the fire want to T1 leaves only the fractured cuirass.
    let mut strict = cuirass();
    strict.wants[1].min_tier = 1;
    let got = resale(&fetched, &resolved(&strict), craft(), &market());
    assert_eq!(got.prices, vec![10.0]);

    // A listing that meets the profile but is priced in a currency the
    // market cannot convert is not counted, and the text says so.
    let only_chaos = |amount: f64, currency: &str| (currency == "chaos").then_some(amount);
    let got = resale(&fetched, &r, craft(), &Market { convert: &only_chaos, unit: "c" });
    assert_eq!(got.cheapest, None);
    assert_eq!(got.not_counted, 6);
    assert!(got.text.starts_with("no listing of the finished search meets the profile"), "{}", got.text);
}

#[test]
fn margin_is_resale_minus_price_minus_fix_under_one_model() {
    let six = resale_of(&[6.0, 7.0, 9.0]);
    let m = margin(2.0, Some(0.9), &six).unwrap();
    assert!((m - 3.1).abs() < 1e-9, "{m}");
    assert_eq!(margin(2.0, None, &six), Err("fix cost unknown".to_string()));
    assert!(margin(2.0, Some(0.9), &resale_of(&[])).unwrap_err().starts_with("resale unknown"));

    // A candidate from the fixture: the margin is the resale minus its
    // listed price minus the uniform model's fix, and the observed model's
    // figure sits beside it without moving it.
    let r = resolved(&cuirass());
    let resale = resale(&fetched(), &r, craft(), &market());
    let observed = Model::Observed {
        class: "Body Armour".to_string(),
        counts: [("FireResist6".to_string(), 3), ("FireResist7".to_string(), 1), ("ColdResist6".to_string(), 4)]
            .into_iter()
            .collect(),
        totals: [(AffixKind::Suffix, 8)].into_iter().collect(),
        listings: 412,
    };
    let config = small_runs();
    let uniform_only = Planner { pool: craft(), observed: None, prices: &flat_prices, config: &config };
    let with_observed = Planner { pool: craft(), observed: Some(&observed), prices: &flat_prices, config: &config };
    let entry = listing("1ce8f868");

    let a = candidate(&entry, &r, &resale, craft(), &uniform_only, &market()).unwrap();
    let fix = a.fix.clone().unwrap_or_else(|e| panic!("a fix under uniform: {e}"));
    assert_eq!(a.price, Ok(6.0));
    let m = a.margin.clone().unwrap();
    assert!((m - (3.0 - 6.0 - fix.cost)).abs() < 1e-9, "{m} against {fix:?}");
    assert!(!a.meets_margin);
    let expected_start = format!(
        "listed 6 div by seller4 (online) · fix ~{} div ({}",
        khaloni_poe2_core::suggest::price_text(fix.cost),
        fix.strategy
    );
    assert!(a.text.starts_with(&expected_start), "{}", a.text);
    assert!(a.text.contains(" · resale cheapest 3 div (6 listings) · margin "), "{}", a.text);
    assert!(a.text.contains(" div under uniform"), "{}", a.text);
    assert!(a.fix_observed.is_none() && a.margin_observed.is_none());

    // With an observed model the plan heads with it and keeps the uniform
    // figures beside; the ranked margin still uses the uniform one, and
    // the observed margin is its own figure.
    let b = candidate(&entry, &r, &resale, craft(), &with_observed, &market()).unwrap();
    let plan = b.plan.as_ref().expect("the planner ran");
    let uniform_best = plan
        .strategies
        .iter()
        .filter_map(|s| s.uniform.as_ref().and_then(|u| u.per_finished))
        .min_by(f64::total_cmp)
        .unwrap();
    let fix = b.fix.clone().unwrap();
    assert_eq!(fix.cost, uniform_best, "the fix is the uniform model's cheapest strategy");
    assert!((b.margin.clone().unwrap() - (3.0 - 6.0 - fix.cost)).abs() < 1e-9);
    match (&b.fix_observed, &b.margin_observed) {
        (Some(Ok(o)), Some(Ok(m))) => assert!((m - (3.0 - 6.0 - o.cost)).abs() < 1e-9),
        (Some(Err(reason)), Some(Err(m))) => assert_eq!(m, &format!("fix cost unknown: {reason}")),
        other => panic!("an observed figure or its reason beside the uniform one: {other:?}"),
    }
    assert_eq!(b.observed_label.as_deref(), Some("observed on 412 listings of Body Armour"));
}

#[test]
fn a_candidate_with_an_unknown_fix_cost_sorts_last_with_its_reason() {
    let r = resolved(&cuirass());
    let resale = resale(&fetched(), &r, craft(), &market());
    let config = small_runs();
    let priced = Planner { pool: craft(), observed: None, prices: &flat_prices, config: &config };
    let unpriced = Planner { pool: craft(), observed: None, prices: &no_prices, config: &config };

    let unknown = candidate(&listing("bd6012a3"), &r, &resale, craft(), &unpriced, &market()).unwrap();
    let reason = unknown.fix.clone().expect_err("no currency has a price");
    assert!(!reason.is_empty());
    assert!(unknown.text.contains(&format!("fix cost unknown: {reason}")), "{}", unknown.text);
    assert_eq!(unknown.margin, Err(format!("fix cost unknown: {reason}")));
    assert!(!unknown.text.contains("margin"), "{}", unknown.text);

    // An evasion jacket can never take increased armour: the reason says
    // so rather than that no strategy reached the profile.
    let jacket = candidate(&listing("cfd514dc"), &r, &resale, craft(), &priced, &market()).unwrap();
    assert_eq!(jacket.fix, Err("#% increased Armour does not roll on Falconer's Jacket".to_string()));
    assert!(jacket.plan.is_none());

    // A finished listing needs no fix at all.
    let finished = candidate(&listing("c8929922"), &r, &resale, craft(), &priced, &market()).unwrap();
    assert_eq!(finished.fix.as_ref().map(|f| f.cost), Ok(0.0));
    assert_eq!(finished.margin, Ok(3.0 - 7.0));
    assert!(finished.text.contains("no fix: it meets the profile"), "{}", finished.text);

    let cheap = candidate(&listing("1ce8f868"), &r, &resale, craft(), &priced, &market()).unwrap();
    let dear = candidate(&listing("e331fd6e"), &r, &resale, craft(), &priced, &market()).unwrap();
    assert!(cheap.margin.is_ok() && dear.margin.is_ok());

    let ranked = rank(vec![unknown.clone(), dear.clone(), finished.clone(), cheap.clone()]);
    let order: Vec<&str> = ranked.iter().map(|c| &c.listing_id[..8]).collect();
    let mut known = [(&cheap, "1ce8f868"), (&dear, "e331fd6e"), (&finished, "c8929922")];
    known.sort_by(|a, b| b.0.margin.clone().unwrap().total_cmp(&a.0.margin.clone().unwrap()));
    let expected: Vec<&str> = known.iter().map(|(_, id)| *id).chain(["bd6012a3"]).collect();
    assert_eq!(order, expected, "largest uniform margin first, the unknown fix last");
    assert!(ranked.last().unwrap().text.contains("fix cost unknown: "));

    // An unreadable card is a candidate with its reason, sorted last too.
    let mut broken = listing("1ce8f868");
    broken["item"]["baseType"] = Value::from("Nonexistent Cuirass");
    broken["item"]["typeLine"] = Value::from("Nonexistent Cuirass");
    let unreadable = candidate(&broken, &r, &resale, craft(), &priced, &market()).unwrap();
    assert!(unreadable.item.is_err());
    assert!(unreadable.text.contains("fix cost unknown: the card cannot be read: "), "{}", unreadable.text);
    let ranked = rank(vec![unreadable, cheap]);
    assert!(ranked[1].item.is_err());
}

// --- the profiles file ---

#[test]
fn profiles_round_trip_through_toml() {
    let mut ring = Profile {
        name: "Fire and cold ring".to_string(),
        class: "Ring".to_string(),
        base: Some("Gold Ring".to_string()),
        min_ilvl: 0,
        wants: vec![want("FireResistance", 2), want("ColdResistance", 2)],
        margin: 45.5,
    };
    let profiles = vec![cuirass(), ring.clone()];
    let text = to_toml(&profiles).unwrap();
    let back = profiles_from_toml(&text);
    assert!(back.errors.is_empty(), "{:?}\n{text}", back.errors);
    assert_eq!(back.profiles, profiles, "{text}");
    assert!(text.contains("[[profile]]"), "{text}");

    // Written by hand: the base and item level may be left out, the margin
    // defaults to 30% and may be a whole number.
    let hand = r#"
[[profile]]
name = "Life boots"
class = "Boots"
wants = [{ family = "IncreasedLife", min_tier = 3 }]

[[profile]]
name = "Fire and cold ring"
class = "Ring"
base = "Gold Ring"
margin = 45.5
wants = [
  { family = "FireResistance", min_tier = 2 },
  { family = "ColdResistance", min_tier = 2 },
]
"#;
    let loaded = profiles_from_toml(hand);
    assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
    assert_eq!(loaded.profiles.len(), 2);
    assert_eq!(loaded.profiles[0].margin, DEFAULT_MARGIN);
    assert_eq!(loaded.profiles[0].base, None);
    assert_eq!(loaded.profiles[0].min_ilvl, 0);
    assert_eq!(loaded.profiles[1], ring);

    ring.margin = 25.0;
    let whole = profiles_from_toml(&to_toml(&[ring.clone()]).unwrap().replace("25.0", "25"));
    assert!(whole.errors.is_empty(), "{:?}", whole.errors);
    assert_eq!(whole.profiles, vec![ring]);

    assert_eq!(profiles_from_toml(""), flip::LoadedProfiles::default());
}

#[test]
fn a_malformed_profile_is_reported_not_dropped() {
    let text = r#"
[[profile]]
name = "Good"
class = "Boots"
wants = [{ family = "IncreasedLife", min_tier = 3 }]

[[profile]]
name = "Tier zero"
class = "Boots"
wants = [{ family = "IncreasedLife", min_tier = 0 }]

[[profile]]
name = "No class"
wants = [{ family = "IncreasedLife", min_tier = 1 }]

[[profile]]
class = "Boots"
wants = [{ family = "IncreasedLife", min_tier = 1 }]
mragin = 20

[[profile]]
name = "Good"
class = "Gloves"
wants = [{ family = "IncreasedLife", min_tier = 1 }]

[[profile]]
name = "Negative"
class = "Gloves"
margin = -5
wants = [{ family = "IncreasedLife", min_tier = 1 }]

[[profile]]
name = "Twice"
class = "Gloves"
wants = [{ family = "IncreasedLife", min_tier = 1 }, { family = "IncreasedLife", min_tier = 2 }]

[[profile]]
name = "Also good"
class = "Helmet"
wants = [{ family = "IncreasedLife", min_tier = 2 }]
"#;
    let loaded = profiles_from_toml(text);
    let names: Vec<&str> = loaded.profiles.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, vec!["Good", "Also good"], "the valid profiles still load");

    let errors: Vec<(Option<usize>, Option<&str>)> =
        loaded.errors.iter().map(|e| (e.index, e.name.as_deref())).collect();
    assert_eq!(
        errors,
        vec![
            (Some(1), Some("Tier zero")),
            (Some(2), Some("No class")),
            (Some(3), None),
            (Some(4), Some("Good")),
            (Some(5), Some("Negative")),
            (Some(6), Some("Twice")),
        ]
    );
    let reasons: Vec<String> = loaded.errors.iter().map(ToString::to_string).collect();
    assert_eq!(reasons[0], "profile 2 (\"Tier zero\"): IncreasedLife: min_tier is 0, and tiers count from 1");
    assert!(reasons[1].starts_with("profile 3 (\"No class\"): ") && reasons[1].contains("class"), "{}", reasons[1]);
    assert!(reasons[2].starts_with("profile 4: ") && reasons[2].contains("mragin"), "{}", reasons[2]);
    assert_eq!(reasons[3], "profile 5 (\"Good\"): an earlier profile has the same name");
    assert!(reasons[4].contains("-5"), "{}", reasons[4]);
    assert_eq!(reasons[5], "profile 7 (\"Twice\"): IncreasedLife is wanted twice");

    // A file that is not TOML at all is one error for the whole file.
    let broken = profiles_from_toml("[[profile]\nname = ");
    assert!(broken.profiles.is_empty());
    assert_eq!(broken.errors.len(), 1);
    assert_eq!(broken.errors[0].index, None);
    assert!(broken.errors[0].to_string().starts_with("the profiles file: it is not valid TOML"), "{}", broken.errors[0]);
}
