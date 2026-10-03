//! The price-check window's wiring, driven through the trade worker's pure
//! pipeline over fixtures: a fetch body and the corpus's built item become
//! the table, the ladder, the closest listings and the budget line; an
//! exchange body becomes the bulk view and the stack's worth; a reward
//! row's request is refused by the budget and logged either way.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use khaloni_poe2::appraise::{self, Fetched, RowKind};
use khaloni_poe2::budget::Budget;
use khaloni_poe2::evaluate_ui::{self as ev, Panel};
use khaloni_poe2::prices::market_from_cache;
use khaloni_poe2_core::bulk;
use khaloni_poe2_core::ee2::request::Built;
use khaloni_poe2_core::ee2::{data, Ee2Data};
use khaloni_poe2_core::market::{self, Floors};
use khaloni_poe2_core::ninja::PriceTable;
use serde_json::Value;

const NOW: i64 = 1_790_000_000;
const ME: &str = "seller3";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../core/tests/fixtures")
}

fn read_json(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(fixtures().join(name)).expect(name)).expect("json")
}

/// The entries of a fetch body as the worker holds them.
fn raw_entries(body: &Value) -> Vec<Option<Value>> {
    body["result"].as_array().cloned().unwrap_or_default().into_iter().map(|v| (!v.is_null()).then_some(v)).collect()
}

/// The currency table from the poe.ninja fixture: what a divine and a
/// chaos are worth in exalted, which is how a listing's price converts.
fn table() -> PriceTable {
    let ov: khaloni_poe2_core::ninja::ExchangeOverview =
        serde_json::from_str(include_str!("../../core/tests/fixtures/ninja_currency.json")).unwrap();
    PriceTable::build(&[ov])
}

fn names() -> HashMap<String, String> {
    HashMap::from([
        ("exalted".to_string(), "Exalted Orb".to_string()),
        ("divine".to_string(), "Divine Orb".to_string()),
        ("chaos".to_string(), "Chaos Orb".to_string()),
        ("omen-of-whittling".to_string(), "Omen of Whittling".to_string()),
    ])
}

/// The corpus's "Hate Pelt", a Soldier Cuirass like the fixture's
/// listings, built the way the price check builds it.
fn built_cuirass() -> Built {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tools/ee2-parity");
    let read = |p: PathBuf| std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    let mut d = Ee2Data::from_ndjson(&read(dir.join("data/stats.ndjson")), &read(dir.join("data/items.ndjson")))
        .expect("pinned ndjson parses");
    d.trade_stats = Some(data::TradeStatTexts::from_json(&read(dir.join("data/trade-stats.json"))).expect("trade stats"));
    d.trade_items = Some(data::trade_item_names(&read(dir.join("data/trade-items.json"))).expect("trade items"));
    khaloni_poe2_core::ee2::request::build(&read(dir.join("items/ee2-ArmourHighValueRareItem.txt")), &d)
        .expect("the corpus item builds")
}

/// The market fixtures as a cache dir, the way the overlay reads them.
fn market_cache(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("khalonipoe2-window-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for typ in ["Essences", "Breach", "UniqueArmours"] {
        std::fs::copy(fixtures().join(format!("market/{typ}.json")), dir.join(format!("Forbidden Rites-{typ}.json")))
            .unwrap();
    }
    dir
}

#[test]
fn a_check_produces_table_ninja_ladder_closest_and_budget_from_fixtures() {
    let body = read_json("trade_fetch_full.json");
    let raw = raw_entries(&body);
    let built = built_cuirass();
    assert!(appraise::wants_closest(&built), "a rare with mods is compared with the listings");
    let table = table();
    let names = names();
    let blocks = appraise::blocks(&Fetched {
        raw: &raw,
        total: Some(1934),
        now_unix: NOW,
        my_account: ME,
        currency_names: &names,
        table: &table,
        divine_threshold: 1.0,
        built: Some(&built),
    });

    // The table: the fixture's 18 entries, one null, folded by EE2's rule.
    assert_eq!(blocks.shown, 17);
    assert_eq!(blocks.listings.len(), 17);
    assert_eq!(blocks.dropped, 2, "the null entry and the listing without a price");
    let sol = &blocks.listings[3];
    assert_eq!((sol.price.as_str(), sol.seller.as_str(), sol.mine), ("5 div", "seller3", true));
    // Every row stands for its own listings: the fixture's sellers are
    // distinct, so nothing folds and the cells claim no "x2".
    assert!(blocks.listings.iter().all(|r| r.times == 1 && !r.cells()[0].contains(" x")));
    // The ladder reads off the table, in the unit of its cheapest listing.
    assert!(blocks.ladder.starts_with("cheapest "), "{}", blocks.ladder);
    assert!(blocks.ladder.contains("of 1,934 matched"), "{}", blocks.ladder);
    assert!(blocks.ladder.contains(&format!(" {} ", blocks.unit)), "{} in {}", blocks.ladder, blocks.unit);
    // The cheapest names its seller, for the attribution baseline.
    let (cheapest, seller) = blocks.cheapest.clone().expect("a priced table has a cheapest listing");
    assert!(cheapest > 0.0 && !seller.is_empty());
    // Seventeen honest prices: no price-fixed strip.
    assert!(blocks.price_fixed.is_none());
    // The closest listings compare mod by mod and name listings.
    let closest = blocks.closest.clone().expect("a rare with mods gets the comparison");
    assert_eq!(closest.lines.len(), 1);
    // The fixture's cuirasses are not this item's: the block says so and
    // names how the nearest differs, mod by mod, instead of a figure.
    assert!(
        closest.lines[0].contains("listing") || closest.lines[0].starts_with("no close match among the cheapest 16;"),
        "{}",
        closest.lines[0]
    );
    assert!(closest.lines[0].contains("differs by:") || closest.lines[0].contains(" ex") || closest.lines[0].contains(" div"));
    let nearest = closest.nearest.clone().expect("the nearest listing is named");
    assert!(nearest.starts_with("nearest listing: "), "{nearest}");
    assert!(blocks.listings.iter().any(|r| nearest.contains(&r.seller)), "{nearest}");

    // poe.ninja: the market fixtures through the market view's own model,
    // for an item they track, in the panel's currency.
    let dir = market_cache("check");
    let model = appraise::market_model(&market_from_cache(&dir, "Forbidden Rites"), Floors::default(), NOW);
    let tracked = model.items.iter().find(|g| g.grade != market::Grade::Untrusted).expect("a graded item");
    let block = market::ninja_block(&model, &tracked.item.name, tracked.item.base_type.as_deref(), tracked.item.corrupted, None)
        .expect("the tracked item has a line");
    let ninja = appraise::ninja_view(&block, &table, 1.0);
    assert!(ninja.price.ends_with(" div") || ninja.price.ends_with(" ex") || ninja.price.ends_with(" chaos"), "{}", ninja.price);
    assert!(!ninja.direction.is_empty());
    assert!(!ninja.band.contains('±'), "the overlay face has no plus-minus: {}", ninja.band);
    // An item poe.ninja does not track gets no block.
    assert!(market::ninja_block(&model, "Hate Pelt", Some("Soldier Cuirass"), false, None).is_none());
    let _ = std::fs::remove_dir_all(dir);

    // The budget line from the server's own counters, as the request log
    // reported the last search response.
    appraise::note_request_line(
        "trade response: search 200 policy=trade-search-request-limit ip 1/5 (10s), 3/15 (60s), 4/30 (300s)",
    );
    let (budget_text, low) = appraise::budget_text("search 1/5 (10s)");
    assert_eq!(budget_text, "searches 4/30 (5 min)");
    assert!(!low);

    // Everything lands on one panel, with the item's first mod ticked the
    // way the card opens.
    let first_mod = built.labels.iter().position(|l| l.tag == "explicit").expect("the cuirass has explicit mods");
    let row = ev::StatRow {
        label: built.labels[first_mod].text.clone(),
        badge: Some(ev::TierBadge { kind: ev::AffixKind::Prefix, tier: 1 }),
        score: Some(4.0),
        min: Some(1.0),
        max: None,
        enabled: true,
        target: Some(ev::Target::Stat(first_mod)),
        hidden: false,
        group: ev::RowGroup::Explicit,
        note: None,
    };
    let panel = Panel {
        header: ev::ItemHeader { name: "Hate Pelt".into(), rarity: "Rare".into(), ..Default::default() },
        rows: vec![row],
        listings: blocks.listings.clone(),
        ladder: blocks.ladder.clone(),
        closest: blocks.closest.clone(),
        ninja: Some(ninja),
        budget_text,
        budget_low: low,
        attribute_enabled: appraise::attribute_enabled(30),
        ..Panel::default()
    };
    let text = ev::all_text(&panel, &|s| 7 * s.len() as i32);
    assert!(text.iter().any(|s| s == "searches 4/30 (5 min)"));
    assert!(text.iter().any(|s| s == "poe.ninja"));
    assert!(text.iter().any(|s| s == "Closest listings"));
    assert!(text.iter().any(|s| s.starts_with("cheapest ")));
    assert!(text.iter().any(|s| s == ev::ATTRIBUTE_BUTTON), "a rare with a ticked mod offers the attribution button");
    assert!(text.iter().any(|s| s == "seller3 (you)"));
    let lay = ev::layout(&panel, &|s| 7 * s.len() as i32);
    assert_eq!(lay.table.unwrap().rows.len(), 17);
    assert!(lay.ninja.is_some() && lay.closest.is_some() && lay.ladder_pos.is_some() && lay.budget_pos.is_some());
}

#[test]
fn closest_listings_build_from_the_fetch_fixture() {
    let body = read_json("trade_fetch_full.json");
    let raw = raw_entries(&body);
    let built = built_cuirass();
    let fetched = Fetched {
        raw: &raw,
        total: Some(1934),
        now_unix: NOW,
        my_account: ME,
        currency_names: &names(),
        table: &table(),
        divine_threshold: 1.0,
        built: Some(&built),
    };
    let blocks = appraise::blocks(&fetched);
    let closest = blocks.closest.expect("the comparison runs for the built rare");
    // The wording is the comparison's own, and the prices in it are the
    // table's unit.
    assert!(closest.lines[0].contains(&blocks.unit) || closest.lines[0].starts_with("no close match"), "{}", closest.lines[0]);
    // Without a built item (a unique priced by name) there is nothing to
    // compare, and the block is absent rather than empty.
    assert!(appraise::blocks(&Fetched { built: None, ..fetched }).closest.is_none());
    // With no listings at all the block says so.
    let none = appraise::blocks(&Fetched { raw: &[], ..fetched });
    assert!(none.listings.is_empty() && none.ladder.is_empty());
    assert_eq!(none.closest.unwrap().lines[0], "no listings to compare with");
}

#[test]
fn the_bulk_view_builds_from_the_exchange_fixture() {
    let body = read_json("trade_exchange_full.json");
    let view = bulk::parse_exchange(&body);
    let block = appraise::bulk_block(&view, &names());
    assert_eq!(block.offers.len(), view.offers.len());
    assert!(block.offers.len() >= 10, "the fixture holds many offers, got {}", block.offers.len());
    // Cheapest per unit first, as the exchange sorts them.
    assert!(view.offers.windows(2).all(|w| w[0].per_unit <= w[1].per_unit));
    // The names come from the currency table's names, not the API ids.
    assert!(block.offers[0].have.contains("Omen of Whittling"), "{}", block.offers[0].have);
    assert!(block.offers[0].want.contains("Exalted Orb"), "{}", block.offers[0].want);
    assert!(block.offers.iter().all(|o| !o.seller.is_empty()));
    assert!(block.note.contains(&format!("{} offers", view.offers.len())), "{}", block.note);
    assert!(block.note.contains("median of the cheapest 5"), "{}", block.note);
    let rate = view.median_rate().expect("offers give a rate");
    assert!(block.note.contains(&khaloni_poe2_core::value::format_amount(rate)), "{}", block.note);
    // An empty body is an empty view with a note saying so.
    let empty = appraise::bulk_block(&bulk::parse_exchange(&serde_json::json!({ "result": {} })), &names());
    assert!(empty.offers.is_empty() && empty.note.contains("no offers"));
}

#[test]
fn a_stackable_check_produces_the_bulk_view_and_stack_value() {
    let body = read_json("trade_exchange_full.json");
    let view = bulk::parse_exchange(&body);
    let rate = view.median_rate().expect("a rate");
    let table = table();
    let stack_value = appraise::stack_value_text(37, rate, &table, 1.0);
    // "37 x 3.5 ex = 130 ex": both figures in one unit, an x the face can draw.
    assert!(stack_value.starts_with("37 x "), "{stack_value}");
    assert!(stack_value.contains(" = "), "{stack_value}");
    assert!(!stack_value.contains('×'));
    let unit = stack_value.rsplit(' ').next().unwrap();
    assert_eq!(stack_value.matches(unit).count(), 2, "the per-unit and the total share the unit: {stack_value}");
    // A stack worth more than the divine threshold shows in divines.
    let dear = appraise::stack_value_text(1000, rate, &table, 1.0);
    assert!(dear.ends_with(" div"), "{dear}");

    let panel = Panel {
        header: ev::ItemHeader { name: "Omen of Whittling".into(), rarity: "Currency".into(), ..Default::default() },
        bulk: Some(appraise::bulk_block(&view, &names())),
        stack_value: Some(stack_value.clone()),
        budget_text: "searches 4/30 (5 min)".into(),
        ..Panel::default()
    };
    let text = ev::all_text(&panel, &|s| 7 * s.len() as i32);
    assert!(text.iter().any(|s| s == "Bulk offers"));
    assert!(text.iter().any(|s| s == &stack_value));
    assert!(text.iter().any(|s| s.contains("Omen of Whittling") && s != "Omen of Whittling"), "an offer names the currency");
    let lay = ev::layout(&panel, &|s| 7 * s.len() as i32);
    assert!(lay.stack_pos.is_some(), "the stack's worth sits on the header");
    assert_eq!(lay.bulk.unwrap().table.rows.len(), view.offers.len());
    assert!(lay.table.is_none() && lay.attribution.is_none(), "a currency has no listings table and no mods to price");
}

#[test]
fn a_row_request_is_refused_when_the_budget_says_no_and_logged_either_way() {
    let now = Instant::now();
    let mut budget = Budget::default();
    // Room: the row goes, and the line says so with the numbers.
    let (sent, line) = appraise::row_decision(RowKind::Currency, false, &mut budget, now, 30);
    assert!(sent);
    assert!(line.contains("currency row") && line.contains("sent;") && !line.contains("not sent"), "{line}");
    assert!(line.contains("30 free search slots") && line.contains("1 background sent"), "{line}");
    // The user just searched: refused, counted nowhere, and still logged.
    budget.note_user(now);
    let (sent, line) = appraise::row_decision(RowKind::Currency, false, &mut budget, now + Duration::from_secs(5), 30);
    assert!(!sent);
    assert!(line.contains("not sent: the budget has no room"), "{line}");
    assert!(line.contains("1 background sent"), "nothing was counted for the refusal: {line}");
    // Too few free slots for the user's own checks after it: refused.
    let (sent, line) = appraise::row_decision(RowKind::Currency, false, &mut budget, now + Duration::from_secs(120), 9);
    assert!(!sent && line.contains("9 free search slots"), "{line}");
    // A gem row with the setting off never reaches the budget, and the
    // line names the setting rather than the budget.
    let (sent, line) = appraise::row_decision(RowKind::Gem, false, &mut budget, now + Duration::from_secs(120), 30);
    assert!(!sent && line.contains("gem row") && line.contains("gem rows are off"), "{line}");
    let (sent, line) = appraise::row_decision(RowKind::Gem, true, &mut budget, now + Duration::from_secs(120), 30);
    assert!(sent && line.contains("gem row (search) sent"), "{line}");
}
