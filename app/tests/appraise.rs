//! What a page of trade listings supports saying, and the rules the trade
//! worker runs by: what one check fetches, the budget line, the price-fixed
//! re-search and the attribution searches, caches that never hold an error
//! as an answer, loads that are retried, user requests before background
//! ones.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use khaloni_poe2::appraise::{
    appraise, attribute_enabled, attribution_rows, background_request, budget_line, error_text,
    exchange_with_fallback, fetch_plan, gem_rows_enabled, listing_exalted, needs_exchange, price_fixed_query,
    priority_channel, retry_after, row_request, sample_text, search_counters, strongest_mods, without_filter,
    AsyncCache, Retrying, ReuseSlot, RowKind, ATTRIBUTE_MIN_FREE, ERROR_RETRY, GOOD_TTL, PENDING_TIMEOUT,
};
use khaloni_poe2::budget;
use khaloni_poe2::evaluate_ui::{self as ev, Panel};
use khaloni_poe2_core::ninja::{ExchangeOverview, PriceTable};
use khaloni_poe2_core::trade::{Listing, Query, StatFilter, TradeError};

fn table() -> PriceTable {
    let ov: ExchangeOverview =
        serde_json::from_str(include_str!("../../core/tests/fixtures/ninja_currency.json")).unwrap();
    PriceTable::build(&[ov])
}

fn listing(amount: f64, currency: &str) -> Listing {
    Listing {
        price_amount: amount,
        price_currency: currency.into(),
        account: "Seller#0001".into(),
        indexed: String::new(),
        item_name: String::new(),
    }
}

/// Exalted converts, "mystery" does not.
fn exalted_only(l: &Listing) -> Option<f64> {
    (l.price_currency == "exalted").then_some(l.price_amount)
}

#[test]
fn unconvertible_listings_are_counted_not_dropped_silently() {
    let mut ls: Vec<Listing> = (1..=7).map(|i| listing(f64::from(i), "exalted")).collect();
    ls.extend((0..3).map(|_| listing(1.0, "mystery")));
    let sample = appraise(&ls, 0, Some(10), &exalted_only);
    assert_eq!((sample.priced, sample.requested), (7, 10));
    assert_eq!(sample.unpriced_currencies, vec!["mystery".to_string()]);
    assert!(sample_text(&sample).starts_with("7 of 10 priced"));
}

#[test]
fn listings_the_fetch_dropped_count_against_the_sample() {
    // Ten were asked for, six came back gone or unpriced: four prices are
    // not "4 of 4".
    let ls: Vec<Listing> = (1..=4).map(|i| listing(f64::from(i), "exalted")).collect();
    let sample = appraise(&ls, 6, None, &exalted_only);
    assert_eq!((sample.priced, sample.requested), (4, 10));
    let many = appraise(&ls, 6, Some(900), &exalted_only);
    assert!(sample_text(&many).contains("cheapest 10 of 900"));
}

// --- what one check fetches ----------------------------------------------

#[test]
fn a_check_fetches_twenty_listings_in_two_calls() {
    // A hundred ids came back; the table takes the first twenty, ten per
    // call, whether or not the comparison is wanted.
    assert_eq!(fetch_plan(Some(1934), 100, false), vec![0..10, 10..20]);
    assert_eq!(fetch_plan(None, 100, false), vec![0..10, 10..20]);
    // With fewer ids than a full table, only the pages that exist.
    assert_eq!(fetch_plan(Some(14), 14, false), vec![0..10, 10..14]);
    assert_eq!(fetch_plan(Some(7), 7, false), vec![0..7]);
    assert!(fetch_plan(Some(0), 0, true).is_empty(), "nothing to fetch is no call");
}

#[test]
fn a_check_with_more_than_twenty_matches_fetches_up_to_forty() {
    // The closest listings want candidates past the table: four calls when
    // the search matched more than the table holds, and never more.
    assert_eq!(fetch_plan(Some(1934), 100, true), vec![0..10, 10..20, 20..30, 30..40]);
    assert_eq!(fetch_plan(Some(35), 35, true), vec![0..10, 10..20, 20..30, 30..35]);
    // Without a reported total the search is not known to hold more than
    // the table, so the table's two calls are all.
    assert_eq!(fetch_plan(None, 100, true), vec![0..10, 10..20]);
}

#[test]
fn a_check_with_twenty_or_fewer_matches_fetches_once_per_ten() {
    // Twenty or fewer matches: the table already holds every one of them,
    // so the comparison costs nothing more.
    assert_eq!(fetch_plan(Some(20), 20, true), vec![0..10, 10..20]);
    assert_eq!(fetch_plan(Some(14), 14, true), vec![0..10, 10..14]);
    assert_eq!(fetch_plan(Some(21), 21, true), vec![0..10, 10..20, 20..21]);
}

// --- the budget line ------------------------------------------------------

#[test]
fn the_budget_line_reads_the_five_minute_rule_off_the_response_line() {
    let line = "trade response: search 200 policy=trade-search-request-limit ip 1/5 (10s), 3/15 (60s), 12/30 (300s); account 2/45 (300s)";
    assert_eq!(search_counters(line), Some((12, 30)), "the fullest family's five-minute rule");
    let (text, low) = budget_line(12, 30);
    assert_eq!(text, "searches 12/30 (5 min)");
    assert!(!low);
    assert!(budget_line(25, 30).1, "within five of the cap turns the line red");
    assert!(budget_line(30, 30).1);
    // Another endpoint's line, or one without a five-minute rule, says
    // nothing about the searches.
    assert_eq!(search_counters("trade response: fetch 200 policy=trade-fetch-request-limit ip 1/12 (4s), 2/16 (12s)"), None);
    assert_eq!(search_counters("trade response: search 200 policy=x ip 1/5 (10s)"), None);
    assert_eq!(search_counters("trade request: search Standard policy=x"), None);
    // The ban line is not a response line.
    assert_eq!(search_counters("trade 429: search banned 60s (retry-after none, state ip 5:10:60) after request search Standard"), None);
}

#[test]
fn the_attribute_button_is_disabled_under_eight_free_slots() {
    assert_eq!(ATTRIBUTE_MIN_FREE, 8);
    assert!(!attribute_enabled(7));
    assert!(attribute_enabled(8));
    assert!(attribute_enabled(30));
    assert!(!attribute_enabled(0));
}

// --- the price-fixed re-search and what each mod is worth ---------------

#[test]
fn the_price_fixed_re_search_keeps_only_exalted_and_divine_prices() {
    let q = Query {
        category: Some("armour.chest".into()),
        category_enabled: true,
        collapse: true,
        filters: vec![StatFilter::at_least("explicit.stat_1", 60.0, false)],
        ..Query::default()
    };
    let honest = price_fixed_query(&q);
    let body = honest.to_body();
    assert_eq!(body["query"]["filters"]["trade_filters"]["filters"]["price"]["option"], "exalted_divine");
    // Everything else of the search is kept.
    assert_eq!(body["query"]["filters"]["trade_filters"]["filters"]["collapse"]["option"], "true");
    assert_eq!(body["query"]["stats"], q.to_body()["query"]["stats"]);
    assert_eq!(body["query"]["filters"]["type_filters"], q.to_body()["query"]["filters"]["type_filters"]);
    // Without the option the body carries no price filter, byte for byte
    // as before.
    assert!(q.to_body()["query"]["filters"]["trade_filters"]["filters"].get("price").is_none());
    let plain = Query { collapse: false, ..q.clone() };
    assert!(plain.to_body()["query"]["filters"].get("trade_filters").is_none(), "no trade filters at all when nothing asks for one");
}

fn mod_row(label: &str, kind: ev::AffixKind, tier: u8, score: f32, enabled: bool, filter: usize) -> ev::StatRow {
    ev::StatRow {
        label: label.into(),
        badge: Some(ev::TierBadge { kind, tier }),
        score: Some(score),
        min: Some(1.0),
        max: None,
        enabled,
        target: Some(ev::Target::Stat(filter)),
        hidden: false,
        group: ev::RowGroup::Explicit,
        note: None,
    }
}

#[test]
fn the_strongest_three_ticked_mods_are_searched_without_their_filter() {
    let panel = Panel {
        rows: vec![
            mod_row("+45% to Fire Resistance", ev::AffixKind::Suffix, 2, 3.0, true, 0),
            mod_row("+60 to maximum Life", ev::AffixKind::Prefix, 1, 4.0, true, 1),
            mod_row("+30% to Cold Resistance", ev::AffixKind::Suffix, 3, 2.0, false, 2),
            mod_row("+348 to Armour", ev::AffixKind::Prefix, 1, 4.5, true, 3),
            mod_row("+12 to Dexterity", ev::AffixKind::Suffix, 7, 1.0, true, 4),
            // A figure row has no badge and drives no stat filter.
            ev::StatRow {
                badge: None,
                target: Some(ev::Target::Equipment(ev::EquipKey::Armour)),
                ..mod_row("Armour: 2972", ev::AffixKind::Other, 1, 5.0, true, 9)
            },
        ],
        ..Panel::default()
    };
    let q = Query {
        filters: (0..5).map(|i| StatFilter::at_least(format!("explicit.stat_{i}"), 1.0, false)).collect(),
        ..Query::default()
    };
    let mods = strongest_mods(&panel, &q, q.filters.len());
    // Best tier first, best roll first within a tier, three at most, and
    // the unticked cold res is not among them.
    assert_eq!(
        mods,
        vec![
            ("T1 armour".to_string(), without_filter(&q, 3)),
            ("T1 life".to_string(), without_filter(&q, 1)),
            ("T2 fire res".to_string(), without_filter(&q, 0)),
        ]
    );
    let without = without_filter(&q, 1);
    assert!(without.filters[1].disabled && !q.filters[1].disabled);
    assert!(without.filters.iter().enumerate().all(|(i, f)| i == 1 || !f.disabled));
    // A disabled filter still rides along in the body, marked disabled.
    let sent = without.to_body();
    let and_group = sent["query"]["stats"][0]["filters"].as_array().unwrap();
    assert_eq!(and_group[1]["disabled"], true);
}

fn folded_line(text: &str, ids: &[&str]) -> khaloni_poe2_core::ee2::request::ExtraRow {
    khaloni_poe2_core::ee2::request::ExtraRow {
        text: text.into(),
        tag: "explicit",
        rolled: Some(27.0),
        lines: vec![text.into()],
        into: vec!["Total DPS".into()],
        group: "explicit",
        ids: ids.iter().map(|s| s.to_string()).collect(),
        option: None,
        value: khaloni_poe2_core::trade::FilterValue { min: Some(25.0), max: None },
        lookup: Vec::new(),
        stat_keys: Vec::new(),
    }
}

/// A line EE2 folded into a total, ticked into the search, is a mod like
/// any other: when it ranks among the strongest, its "without" search is
/// the query that was searched (Broad's relaxed bounds included) with only
/// its filter taken out, and the block names the line. An unticked line is
/// never searched without, and neither is one the trade site has no stat
/// for, whatever their tier.
#[test]
fn what_each_mod_is_worth_covers_rows_ticked_from_folded_lines() {
    let mut query = Query {
        filters: (0..2).map(|i| StatFilter::at_least(format!("explicit.stat_{i}"), 40.0, false)).collect(),
        ..Query::default()
    };
    let mut extras = Vec::new();
    let folded = ev::extra_rows(
        &[
            folded_line("Adds 27 to 36 Fire Damage", &["explicit.stat_709508406"]),
            folded_line("+20 to Strength", &["explicit.stat_4080418644"]),
            folded_line("12% increased Wombat Summoning Speed", &[]),
        ],
        &mut extras,
    );
    let badge = |row: &ev::StatRow, score: f32| ev::StatRow {
        badge: Some(ev::TierBadge { kind: ev::AffixKind::Prefix, tier: 1 }),
        score: Some(score),
        ..row.clone()
    };
    let mut panel = Panel {
        rows: vec![
            mod_row("+45% to Fire Resistance", ev::AffixKind::Suffix, 2, 3.0, true, 0),
            mod_row("+30% to Cold Resistance", ev::AffixKind::Suffix, 3, 2.0, true, 1),
            badge(&folded[0], 2.0),
            // Better than every other row, and not ticked.
            badge(&folded[1], 5.0),
            // No trade id, so no checkbox; marked ticked all the same, to
            // show the id is what keeps it out.
            ev::StatRow { enabled: true, ..badge(&folded[2], 5.0) },
        ],
        extras,
        ..Panel::default()
    };
    ev::toggle_row(&mut panel, &mut query, 2);
    let searched = khaloni_poe2_core::trade::relax_query(&ev::search_query(&panel, &query), 0.10);
    assert_eq!(searched.filters.len(), 3, "EE2's two rows and the ticked line");

    let mods = strongest_mods(&panel, &searched, query.filters.len());
    let labels: Vec<&str> = mods.iter().map(|(l, _)| l.as_str()).collect();
    assert_eq!(labels, ["T1 adds # to # fire damage", "T2 fire res", "T3 cold res"]);
    let mut minus_line = searched.clone();
    minus_line.filters.remove(2);
    assert_eq!(mods[0].1, minus_line, "the searched query minus exactly the line's filter");
    assert_eq!(mods[1].1, without_filter(&searched, 0));
    assert_eq!(mods[2].1, without_filter(&searched, 1));
    assert!(
        mods.iter().all(|(l, _)| !l.contains("strength") && !l.contains("wombat")),
        "unticked and unsearchable lines are never chosen: {labels:?}"
    );
}

/// A ticked row with no tier badge (a total, an implicit, a unique's line)
/// is a mod too: it is priced after every badged row, in the card's order,
/// and named by its stat; the three-search cap still holds.
#[test]
fn a_ticked_row_without_a_tier_badge_is_priced_after_the_badged_ones() {
    let mut query = Query {
        filters: (0..1).map(|i| StatFilter::at_least(format!("explicit.stat_{i}"), 40.0, false)).collect(),
        ..Query::default()
    };
    let mut extras = Vec::new();
    let folded = ev::extra_rows(
        &[
            folded_line("Adds 27 to 36 Fire Damage", &["explicit.stat_709508406"]),
            folded_line("+20 to Strength", &["explicit.stat_4080418644"]),
            folded_line("+15% to Chaos Resistance", &["explicit.stat_2923486259"]),
        ],
        &mut extras,
    );
    let mut panel = Panel {
        rows: vec![
            mod_row("+45% to Fire Resistance", ev::AffixKind::Suffix, 2, 3.0, true, 0),
            // Three unbadged lines, ticked below in this order.
            folded[0].clone(),
            folded[1].clone(),
            folded[2].clone(),
        ],
        extras,
        ..Panel::default()
    };
    for i in 1..=3 {
        ev::toggle_row(&mut panel, &mut query, i);
    }
    assert!(panel.rows[1..].iter().all(|r| r.badge.is_none() && r.enabled));
    let searched = ev::search_query(&panel, &query);
    let mods = strongest_mods(&panel, &searched, query.filters.len());
    let labels: Vec<&str> = mods.iter().map(|(l, _)| l.as_str()).collect();
    // The badged row first, then the unbadged in the card's order, three in all.
    assert_eq!(labels.len(), 3, "{labels:?}");
    assert_eq!(labels[0], "T2 fire res");
    assert!(labels[1].contains("fire damage") && labels[2].contains("strength"), "{labels:?}");
    assert!(!labels[1].starts_with('T') || !labels[1][1..2].chars().all(|c| c.is_ascii_digit()), "no tier is claimed: {labels:?}");
    let mut minus_first = searched.clone();
    minus_first.filters.remove(1);
    assert_eq!(mods[1].1, minus_first);
}

#[test]
fn attribution_rows_name_both_searches_cheapest_listings() {
    let rows = attribution_rows(
        (40.0, "seller1"),
        &[("T1 life".into(), Some((9.0, "seller4".into()))), ("T2 fire res".into(), None)],
        "ex",
    );
    assert_eq!(rows.len(), 2);
    assert_eq!((rows[0].label.as_str(), rows[0].with.as_str(), rows[0].without.as_str()), ("T1 life", "40 ex", "9 ex"));
    assert_eq!(rows[0].text, "with: seller1 40 ex; without: seller4 9 ex");
    assert_eq!(rows[1].without, "nothing matched");
    assert!(rows[1].text.contains("nothing matched"), "{}", rows[1].text);
}

#[test]
fn listings_convert_through_the_currency_names() {
    let t = table();
    let names = HashMap::from([("divine".to_string(), "Divine Orb".to_string())]);
    assert_eq!(listing_exalted(&listing(3.0, "exalted"), &names, &t), Some(3.0));
    let div = t.lookup("Divine Orb").expect("fixture has divine").exalted;
    assert_eq!(listing_exalted(&listing(2.0, "divine"), &names, &t), Some(2.0 * div));
    // An id with no name, or a name with no rate, is not guessed at.
    assert_eq!(listing_exalted(&listing(2.0, "mirror"), &names, &t), None);
}

#[test]
fn exchange_falls_back_to_chaos_then_divine() {
    let rate = |name: &str| match name {
        "Chaos Orb" => Some(50.0),
        "Divine Orb" => Some(400.0),
        _ => None,
    };
    // Offered for exalted: asked once.
    let mut asked = Vec::new();
    let got = exchange_with_fallback::<String>(
        &mut |have| {
            asked.push(have.to_string());
            Ok(Some(3.0))
        },
        &rate,
    );
    assert_eq!((got, asked.as_slice()), (Ok(Some(3.0)), &["exalted".to_string()][..]));
    // Only offered for divine: exalted and chaos come back empty first.
    let mut asked = Vec::new();
    let got = exchange_with_fallback::<String>(
        &mut |have| {
            asked.push(have.to_string());
            Ok((have == "divine").then_some(0.5))
        },
        &rate,
    );
    assert_eq!(got, Ok(Some(200.0)));
    assert_eq!(asked, ["exalted", "chaos", "divine"]);
    // Nobody offers it at all.
    assert_eq!(exchange_with_fallback::<String>(&mut |_| Ok(None), &rate), Ok(None));
    // An error ends the walk: no second request into a cooldown.
    let mut calls = 0;
    let got = exchange_with_fallback(
        &mut |_| {
            calls += 1;
            Err("cooldown".to_string())
        },
        &rate,
    );
    assert_eq!((got, calls), (Err("cooldown".to_string()), 1));
    // A currency the table cannot convert is not asked about.
    let mut asked = Vec::new();
    let _ = exchange_with_fallback::<String>(
        &mut |have| {
            asked.push(have.to_string());
            Ok(None)
        },
        &|_| None,
    );
    assert_eq!(asked, ["exalted"]);
}

#[test]
fn a_cooldown_reads_in_whole_seconds_and_sets_the_retry() {
    let e = TradeError::Cooldown(Duration::from_millis(11_400));
    assert_eq!(error_text(&e), "trade cooldown 11s");
    assert!(retry_after(&e) >= Duration::from_millis(11_400));
    assert_eq!(retry_after(&TradeError::Http("timeout".into())), ERROR_RETRY);
}

#[test]
fn an_error_is_never_cached_as_the_answer() {
    let t0 = Instant::now();
    let mut c: AsyncCache<String, Option<f64>> = AsyncCache::default();
    let key = "Omen of Testing".to_string();
    // A miss asks once, and only once while the request is out.
    assert!(c.lookup(&key, t0).request);
    let again = c.lookup(&key, t0 + Duration::from_secs(1));
    assert!(!again.request && again.value.is_none());
    // The request failed: no value, the reason is kept, and it is asked
    // again once the wait is over.
    c.store(key.clone(), Err(("trade cooldown 12s".into(), Duration::from_secs(12))), t0);
    let during = c.lookup(&key, t0 + Duration::from_secs(5));
    assert_eq!((during.value, during.request), (None, false));
    assert_eq!(during.error.as_deref(), Some("trade cooldown 12s"));
    assert!(c.lookup(&key, t0 + Duration::from_secs(13)).request, "retried after the wait");
}

#[test]
fn a_good_answer_expires_and_is_served_while_it_refreshes() {
    let t0 = Instant::now();
    let mut c: AsyncCache<String, Option<f64>> = AsyncCache::default();
    let key = "Greater Rune".to_string();
    assert!(c.lookup(&key, t0).request);
    c.store(key.clone(), Ok(Some(4.0)), t0);
    let hit = c.lookup(&key, t0 + Duration::from_secs(60));
    assert_eq!((hit.value, hit.request), (Some(Some(4.0)), false));
    // Past the TTL it is asked again, and the old price keeps showing.
    let expired = c.lookup(&key, t0 + GOOD_TTL + Duration::from_secs(1));
    assert_eq!((expired.value, expired.request), (Some(Some(4.0)), true));
    // A refresh that fails keeps the last good price too.
    c.store(key.clone(), Err(("http: timeout".into(), ERROR_RETRY)), t0 + GOOD_TTL + Duration::from_secs(2));
    assert_eq!(c.lookup(&key, t0 + GOOD_TTL + Duration::from_secs(3)).value, Some(Some(4.0)));
    // ...but not for ever: refreshes that keep failing stop vouching for it.
    assert_eq!(c.lookup(&key, t0 + GOOD_TTL * 2 + Duration::from_secs(1)).value, None);
    // "Nobody offers it" is an answer and is cached like one.
    c.store("Dust".to_string(), Ok(None), t0);
    let none = c.lookup(&"Dust".to_string(), t0 + Duration::from_secs(1));
    assert_eq!((none.value, none.request), (Some(None), false));
}

#[test]
fn a_request_nobody_answered_is_sent_again() {
    let t0 = Instant::now();
    let mut c: AsyncCache<(String, u32), Option<f64>> = AsyncCache::default();
    let key = ("fireball".to_string(), 20);
    assert!(c.lookup(&key, t0).request);
    assert!(!c.lookup(&key, t0 + Duration::from_secs(30)).request);
    assert!(c.lookup(&key, t0 + PENDING_TIMEOUT).request, "a lost request must not stay pending forever");
    // A request that could not even be sent is forgotten at once.
    let other = ("spark".to_string(), 10);
    assert!(c.lookup(&other, t0).request);
    c.unsent(&other);
    assert!(c.lookup(&other, t0).request);
}

#[test]
fn an_identical_check_reuses_the_last_result_briefly() {
    let t0 = Instant::now();
    let mut slot: ReuseSlot<String, u32> = ReuseSlot::new(Duration::from_secs(30));
    assert!(slot.get(&"body-a".to_string(), t0).is_none());
    slot.put("body-a".to_string(), 7, t0);
    let (v, age) = slot.get(&"body-a".to_string(), t0 + Duration::from_secs(10)).expect("reused");
    assert_eq!((v, age), (7, Duration::from_secs(10)));
    // A different request, or the same one later, runs again.
    assert!(slot.get(&"body-b".to_string(), t0 + Duration::from_secs(10)).is_none());
    assert!(slot.get(&"body-a".to_string(), t0 + Duration::from_secs(31)).is_none());
}

#[test]
fn a_failed_load_is_retried_with_a_growing_wait() {
    let t0 = Instant::now();
    let mut r: Retrying<u32> = Retrying::default();
    let mut attempts = 0;
    let mut try_load = |r: &mut Retrying<u32>, at: Instant, ok: bool| {
        r.get_or_load(at, || {
            attempts += 1;
            if ok { Ok(42) } else { Err("catalog down".to_string()) }
        })
        .map(|v| *v)
    };
    assert!(r.attempt_due(t0));
    assert_eq!(try_load(&mut r, t0, false), Err("catalog down".to_string()));
    assert!(!r.attempt_due(t0 + Duration::from_secs(1)), "inside the wait");
    assert!(r.attempt_due(t0 + Duration::from_secs(6)));
    // Inside the wait nothing is attempted, and the error names the wait.
    let waiting = try_load(&mut r, t0 + Duration::from_secs(1), true).unwrap_err();
    assert!(waiting.contains("catalog down") && waiting.contains("next try in"), "{waiting}");
    // After it, the load runs again and its result sticks.
    assert_eq!(try_load(&mut r, t0 + Duration::from_secs(6), true), Ok(42));
    assert_eq!(try_load(&mut r, t0 + Duration::from_secs(7), false), Ok(42));
    assert_eq!(attempts, 2, "one failure, one success, nothing while waiting or once loaded");
    assert!(r.is_loaded() && !r.attempt_due(t0 + Duration::from_secs(3600)));
}

#[test]
fn the_users_request_goes_before_a_reward_rows() {
    let (tx, rx) = priority_channel::<&'static str>();
    tx.send_background("gem row 1").unwrap();
    tx.send_background("gem row 2").unwrap();
    tx.send_user("F7 price check").unwrap();
    assert_eq!(rx.recv(), Some("F7 price check"));
    assert_eq!(rx.recv(), Some("gem row 1"));
    tx.send_user("Search press").unwrap();
    assert_eq!(rx.recv(), Some("Search press"));
    assert_eq!(rx.recv(), Some("gem row 2"));
    // Idle: a timeout, not a hang; closed: the end.
    assert!(matches!(
        rx.recv_timeout(Duration::from_millis(20)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    ));
    drop(tx);
    assert_eq!(rx.recv(), None);
}

#[test]
fn a_currency_the_table_prices_never_asks_the_exchange() {
    // poe.ninja prices it: the exchange is not asked, whatever it would say.
    let rate = |name: &str| (name == "Chaos Orb").then_some(50.0);
    let priced = Some(3.5);
    assert!(!needs_exchange(priced));
    let mut asked = 0;
    let got = if needs_exchange(priced) {
        exchange_with_fallback::<String>(
            &mut |_| {
                asked += 1;
                panic!("the exchange was asked for a currency the table prices")
            },
            &rate,
        )
    } else {
        Ok(priced)
    };
    assert_eq!((got, asked), (Ok(Some(3.5)), 0));
    // Only a miss, or a price that says nothing, sends the row to the exchange.
    assert!(needs_exchange(None));
    assert!(needs_exchange(Some(0.0)));
    assert!(needs_exchange(Some(f64::NAN)));
    let mut asked = 0;
    let got = exchange_with_fallback::<String>(
        &mut |_| {
            asked += 1;
            Ok(Some(2.0))
        },
        &rate,
    );
    assert_eq!((got, asked), (Ok(Some(2.0)), 1));
}

#[test]
fn background_rows_go_through_the_budget() {
    let now = Instant::now();
    let mut b = budget::Budget::default();
    // Room to spare: a currency row goes, and is counted.
    assert!(row_request(RowKind::Currency, false, &mut b, now, 30));
    assert_eq!(b.sent_in_window(now), 1);
    // The user's slots come first: with ten or fewer free after it the row
    // is refused, and counts nothing.
    assert!(!row_request(RowKind::Currency, false, &mut b, now, 11));
    assert!(!row_request(RowKind::Currency, false, &mut b, now, 5));
    assert_eq!(b.sent_in_window(now), 1);
    // A minute of quiet after the user's own request.
    b.note_user(now);
    assert!(!row_request(RowKind::Currency, false, &mut b, now + Duration::from_secs(30), 30));
    assert!(row_request(RowKind::Currency, false, &mut b, now + Duration::from_secs(61), 30));
    // Gem rows are refused outright while the setting is off, before the
    // budget is consulted; on, they go through it like a currency row.
    let sent = b.sent_in_window(now + Duration::from_secs(61));
    assert!(!row_request(RowKind::Gem, false, &mut b, now + Duration::from_secs(61), 30));
    assert_eq!(b.sent_in_window(now + Duration::from_secs(61)), sent);
    assert!(row_request(RowKind::Gem, true, &mut b, now + Duration::from_secs(61), 30));
    assert!(!row_request(RowKind::Gem, true, &mut b, now + Duration::from_secs(61), 8));
}

#[test]
fn gem_rows_send_no_search_unless_enabled() {
    let now = Instant::now();
    assert!(!gem_rows_enabled(false));
    assert!(gem_rows_enabled(true));
    // Off: a gem row never reaches the budget, so nothing is spent or
    // counted, even with the whole window free.
    let mut b = budget::Budget::default();
    assert!(!background_request(RowKind::Gem, false, || b.allow_background(now, 30, None)));
    assert_eq!(b.sent_in_window(now), 0);
    // A currency row is unaffected by the gem setting.
    assert!(background_request(RowKind::Currency, false, || b.allow_background(now, 30, None)));
    assert_eq!(b.sent_in_window(now), 1);
    // On: the gem row goes when the budget allows, and not otherwise.
    assert!(background_request(RowKind::Gem, true, || b.allow_background(now, 30, None)));
    assert_eq!(b.sent_in_window(now), 2);
    assert!(!background_request(RowKind::Gem, true, || b.allow_background(now, 5, None)));
    assert_eq!(b.sent_in_window(now), 2);
}

/// A 400 is the site saying the request itself is wrong ("No valid
/// exchange `want` tags were found"): sent again it gets the same answer
/// and spends a slot of the background budget each time. Only new inputs
/// (a price table refresh, a catalog reload, another league) can change it.
#[test]
fn a_refused_request_is_not_sent_again_until_the_data_changes() {
    use std::io::{Read, Write};
    // A local stand-in for the trade site that answers the exchange the
    // way the real one answered the "sep" want tag.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { break };
            let mut buf = [0u8; 8192];
            let _ = s.read(&mut buf);
            let body = r#"{"error":{"code":2,"message":"No valid exchange `want` tags were found"}}"#;
            let _ = write!(
                s,
                "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    let mut client = khaloni_poe2_core::trade::TradeClient::with_limiters(
        &base,
        "Rise of the Abyssal",
        khaloni_poe2_core::trade::Limiters::new(),
    )
    .unwrap();
    let e = client.exchange_raw("sep", "exalted").expect_err("the stub refuses");
    assert!(matches!(e, TradeError::Refused(_)), "{e:?}");
    let why = error_text(&e);
    assert!(why.contains("No valid exchange"), "the reason is kept for display: {why}");

    let t0 = Instant::now();
    let key = "uncut skill gems".to_string();
    let mut c: AsyncCache<String, Option<f64>> = AsyncCache::default();
    assert!(c.lookup(&key, t0).request);
    c.store(key.clone(), Err((why.clone(), retry_after(&e))), t0);
    for later in [ERROR_RETRY, Duration::from_secs(3600), Duration::from_secs(7 * 24 * 3600)] {
        let found = c.lookup(&key, t0 + later);
        assert!(!found.request, "a refusal was sent again after {later:?}");
        assert_eq!(found.error.as_deref(), Some(why.as_str()));
    }

    // Other keys' failures keep their own clock: a timeout still retries.
    let other = "omen of testing".to_string();
    assert!(c.lookup(&other, t0).request);
    let timeout = TradeError::Http("timeout".into());
    c.store(other.clone(), Err((error_text(&timeout), retry_after(&timeout))), t0);
    assert!(c.lookup(&other, t0 + ERROR_RETRY).request, "a transient failure is retried");

    // New inputs lift the refusal, and nothing else about the cache.
    let good = "exalted orb".to_string();
    assert!(c.lookup(&good, t0).request);
    c.store(good.clone(), Ok(Some(1.0)), t0);
    c.inputs_changed();
    assert!(c.lookup(&key, t0 + Duration::from_secs(1)).request, "new data asks again");
    assert_eq!(c.lookup(&good, t0 + Duration::from_secs(1)).value, Some(Some(1.0)));
}

// --- "what each mod is worth" only ever widens the searched query ---

fn ee2_db() -> khaloni_poe2_core::ee2::Ee2Data {
    use khaloni_poe2_core::ee2::{data, Ee2Data};
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tools/ee2-parity/data");
    let read = |f: &str| std::fs::read_to_string(dir.join(f)).unwrap();
    let mut db = Ee2Data::from_ndjson(&read("stats.ndjson"), &read("items.ndjson")).unwrap();
    db.trade_stats = Some(data::TradeStatTexts::from_json(&read("trade-stats.json")).unwrap());
    db.trade_items = Some(data::trade_item_names(&read("trade-items.json")).unwrap());
    db
}

/// The card the app opens for a built item: a row per EE2 filter, then a
/// row per line EE2 gave no row, the way the panel builder lays them out.
fn card_of(built: &khaloni_poe2_core::ee2::request::Built) -> Panel {
    let mut rows: Vec<ev::StatRow> = built
        .labels
        .iter()
        .zip(&built.query.filters)
        .enumerate()
        .map(|(i, (l, f))| ev::StatRow {
            label: l.text.clone(),
            badge: None,
            score: None,
            min: f.value.min,
            max: f.value.max,
            enabled: !f.disabled,
            target: Some(ev::Target::Stat(i)),
            hidden: l.hidden,
            group: ev::group_of(l.tag),
            note: None,
        })
        .collect();
    let mut extras = Vec::new();
    rows.extend(ev::extra_rows(&built.extra, &mut extras));
    Panel { rows, extras, ..Panel::default() }
}

fn bound_no_tighter(wide: &serde_json::Value, narrow: &serde_json::Value) -> bool {
    let (w, n) = (&wide["value"], &narrow["value"]);
    let min_ok = w["min"].as_f64().is_none_or(|wm| n["min"].as_f64().is_some_and(|nm| wm <= nm));
    let max_ok = w["max"].as_f64().is_none_or(|wm| n["max"].as_f64().is_some_and(|nm| wm >= nm));
    let option_ok = w["option"].is_null() || w["option"] == n["option"];
    min_ok && max_ok && option_ok
}

/// Every constraint `without` applies is one `searched` applies at least
/// as tightly, so whatever `searched` matches, `without` matches too.
fn assert_no_narrower(without: &serde_json::Value, searched: &serde_json::Value, what: &str) {
    let enabled_groups = |b: &serde_json::Value| -> Vec<serde_json::Value> {
        b["query"]["stats"].as_array().unwrap().iter().filter(|g| g["disabled"] != true).cloned().collect()
    };
    let enabled_members = |g: &serde_json::Value| -> Vec<serde_json::Value> {
        g["filters"].as_array().unwrap().iter().filter(|f| f["disabled"] != true).cloned().collect()
    };
    let searched_groups = enabled_groups(searched);
    for g in enabled_groups(without) {
        match g["type"].as_str().unwrap() {
            "and" => {
                for m in enabled_members(&g) {
                    let held = searched_groups.iter().filter(|s| s["type"] == "and").flat_map(&enabled_members).any(|s| {
                        s["id"] == m["id"] && bound_no_tighter(&m, &s)
                    });
                    assert!(held, "{what}: {m} is required without, not as loosely in the search");
                }
            }
            kind => {
                let ids = |g: &serde_json::Value| {
                    let mut v: Vec<String> = g["filters"].as_array().unwrap().iter().map(|f| f["id"].to_string()).collect();
                    v.sort();
                    v
                };
                let held = searched_groups.iter().any(|s| {
                    s["type"] == kind
                        && ids(s) == ids(&g)
                        && g["value"]["min"].as_f64().unwrap_or(0.0) <= s["value"]["min"].as_f64().unwrap_or(0.0)
                        && g["value"]["max"].as_f64().is_none_or(|wm| s["value"]["max"].as_f64().is_some_and(|nm| wm >= nm))
                        && enabled_members(&g).iter().all(|m| {
                            enabled_members(s).iter().any(|sm| sm["id"] == m["id"] && bound_no_tighter(m, sm))
                        })
                });
                assert!(held, "{what}: the {kind} group {g} has no counterpart as loose in the search");
            }
        }
    }
    // Outside the stat groups: every section filter the without search
    // applies, the searched one applies the same.
    let sections = |b: &serde_json::Value| b["query"]["filters"].as_object().cloned().unwrap_or_default();
    let searched_sections = sections(searched);
    for (name, section) in sections(without) {
        for (key, v) in section["filters"].as_object().cloned().unwrap_or_default() {
            let theirs = &searched_sections.get(&name).map(|s| s["filters"][&key].clone()).unwrap_or_default();
            // Any rarity but unique is wider than one of them.
            let wider_rarity = key == "rarity"
                && v["option"] == "nonunique"
                && theirs["option"].as_str().is_some_and(|r| r != "unique");
            if !wider_rarity {
                assert_eq!(&v, theirs, "{what}: {name}.{key} differs");
            }
        }
    }
    for key in ["name", "type"] {
        if !without["query"][key].is_null() {
            assert_eq!(without["query"][key], searched["query"][key], "{what}: query.{key}");
        }
    }
}

/// The searches a card sends for `built` under each selection a user
/// makes: every row ticked, every row but one, the lines EE2 gave no row
/// ticked too, and each at Broad's relaxed bounds. `distinct` also holds
/// every without search to differ from the searched one, which a corpus
/// item with two rows on one map filter id does not.
fn attribution_cases(name: &str, built: &khaloni_poe2_core::ee2::request::Built, distinct: bool) -> usize {
    let mut checked = 0;
    let base = card_of(built);
    let tickable: Vec<usize> = (0..base.rows.len()).filter(|&i| base.rows[i].target.is_some()).collect();
    let mut selections: Vec<Vec<usize>> = vec![tickable.clone()];
    for skip in &tickable {
        selections.push(tickable.iter().copied().filter(|i| i != skip).collect());
    }
    for selection in selections {
        for broad in [false, true] {
            let mut panel = base.clone();
            let mut query = built.query.clone();
            for i in 0..panel.rows.len() {
                if panel.rows[i].enabled != selection.contains(&i) {
                    ev::toggle_row(&mut panel, &mut query, i);
                }
            }
            let full = ev::search_query(&panel, &query);
            let searched = if broad { khaloni_poe2_core::trade::relax_query(&full, 0.10) } else { full };
            let searched_body = searched.to_body();
            for (label, without) in strongest_mods(&panel, &searched, query.filters.len()) {
                let what = format!("{name} {selection:?} broad={broad} without {label}");
                if distinct {
                    assert_ne!(without.to_body(), searched_body, "{what}: the without search is the searched one");
                }
                assert_no_narrower(&without.to_body(), &searched_body, &what);
                checked += 1;
            }
        }
    }
    checked
}

/// Dropping a mod from a search can only widen it: the log of 2026-09-29
/// read two searches that matched 0 and 13 after one that matched 10000
/// as attribution searches, which prompted this check on the two items
/// involved (and the tablet, whose mods are twin groups) under every
/// selection a user makes.
#[test]
fn a_without_search_is_never_narrower_than_the_searched_one() {
    let db = ee2_db();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/checked");
    let mut checked = 0;
    for name in ["jewel-gale-stone", "jewel-sol-sliver", "tablet-mythical-instigation"] {
        let text = std::fs::read_to_string(dir.join(format!("{name}.txt"))).unwrap();
        let built = khaloni_poe2_core::ee2::build(&text, &db).expect("the item builds");
        checked += attribution_cases(name, &built, true);
    }
    assert!(checked >= 60, "only {checked} without searches were compared");

    // The same over EE2's parity corpus: every kind of row it builds.
    let corpus = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tools/ee2-parity/items");
    let mut items = 0;
    for entry in std::fs::read_dir(corpus).unwrap().flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "txt") {
            continue;
        }
        let Ok(built) = khaloni_poe2_core::ee2::build(&std::fs::read_to_string(&path).unwrap(), &db) else { continue };
        attribution_cases(&path.file_stem().unwrap().to_string_lossy(), &built, false);
        items += 1;
    }
    assert!(items >= 52, "the corpus shrank to {items} items");
}

/// A local stand-in for the search endpoint that keeps each request's
/// head, so a test sees which cookie left without any real traffic.
fn head_stub() -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
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
                        log.lock().unwrap().push(text[..head_end].to_ascii_lowercase());
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

/// The price check's searches carry the POESESSID saved in settings (the
/// anonymous limit's error tells the user to set one), and a change to it
/// applies to the next search on the same client.
#[test]
fn a_price_check_search_sends_the_saved_session_and_follows_its_changes() {
    use khaloni_poe2::appraise::{session_client, Retrying};
    use khaloni_poe2_core::trade::{Limiters, TradeClient};
    let (base, seen) = head_stub();
    let mut slot: Retrying<TradeClient> = Retrying::default();
    let load = || TradeClient::with_limiters(&base, "Standard", Limiters::new()).map_err(|e| e.to_string());
    let q = Query::default();
    let now = Instant::now();

    session_client(&mut slot, "", now, load).unwrap().search(&q).unwrap();
    session_client(&mut slot, "  abc123  ", now, load).unwrap().search(&q).unwrap();
    session_client(&mut slot, "", now, load).unwrap().search(&q).unwrap();

    let heads = seen.lock().unwrap().clone();
    assert_eq!(heads.len(), 3);
    assert!(!heads[0].contains("poesessid"), "no session saved, yet one was sent:\n{}", heads[0]);
    assert!(heads[1].contains("cookie: poesessid=abc123\r\n"), "the saved session was not sent:\n{}", heads[1]);
    assert!(!heads[2].contains("poesessid"), "a cleared session was still sent:\n{}", heads[2]);
}
