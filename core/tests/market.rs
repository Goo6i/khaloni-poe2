//! The trust rules of the market view, on lines cut from the cached
//! Forbidden Rites overviews of 2026-09-19 (fixtures/market). One line is
//! made up: "Synthetic Four Days" is Forgotten Warden with a fourth day
//! blanked, because every real line with under five days is also too thin
//! to rank and would not isolate the history rule.

use khaloni_poe2_core::market::{
    self, at_price_floor, build, coverage_text, days_note, grade, league_is_young, trend, Direction, Floors, Grade,
    Kind, MarketItem, Model, Options, Source, Verdict,
};
use khaloni_poe2_core::ninja::{ExchangeOverview, ItemOverview};

const ESSENCES: &str = include_str!("fixtures/market/Essences.json");
const BREACH: &str = include_str!("fixtures/market/Breach.json");
const ARMOURS: &str = include_str!("fixtures/market/UniqueArmours.json");

fn source() -> Source {
    let essences: ExchangeOverview = serde_json::from_str(ESSENCES).unwrap();
    let breach: ExchangeOverview = serde_json::from_str(BREACH).unwrap();
    let armours: ItemOverview = serde_json::from_str(ARMOURS).unwrap();
    Source::from_overviews(
        "Forbidden Rites",
        &[("Essences", &essences), ("Breach", &breach)],
        &[("UniqueArmours", &armours)],
    )
}

fn model() -> Model {
    build(&source(), &Options::default())
}

fn find<'a>(m: &'a Model, name: &str, base: Option<&str>) -> &'a market::Graded {
    m.items
        .iter()
        .find(|g| g.item.name == name && g.item.base_type.as_deref() == base)
        .unwrap_or_else(|| panic!("{name} is in the fixture"))
}

fn ranked_names(m: &Model) -> Vec<&str> {
    m.risers.iter().chain(&m.fallers).map(|&i| m.items[i].item.name.as_str()).collect()
}

/// A hand-made exchange item, for baskets whose arithmetic is worked out
/// in the test.
fn exchange(name: &str, volume: f64, points: [Option<f64>; 7]) -> MarketItem {
    MarketItem {
        category: "Ritual".into(),
        id: name.to_lowercase(),
        name: name.into(),
        base_type: None,
        corrupted: false,
        kind: Kind::Exchange,
        price_div: 1.0,
        volume_div: Some(volume),
        listings: None,
        points,
        own_exalted_rate: 474.2,
        own_chaos_rate: 8.32,
    }
}

fn hand_made(items: Vec<MarketItem>) -> Source {
    Source { league: "Test".into(), items, exalted_rate: 474.2, chaos_rate: 8.32, ..Source::default() }
}

/// A week that ends `pct` percent up, climbing evenly.
fn climb(pct: f64) -> [Option<f64>; 7] {
    std::array::from_fn(|i| Some(pct * i as f64 / 6.0))
}

#[test]
fn the_overviews_parse_with_their_history_volume_and_listings() {
    let src = source();
    assert_eq!(src.items.len(), 26);
    assert_eq!((src.exalted_rate, src.chaos_rate), (474.2, 8.32));
    let breach = src.items.iter().find(|i| i.id == "breachstone").unwrap();
    assert_eq!(breach.name, "Breachstone", "named through the overview's own id list");
    assert_eq!((breach.kind, breach.volume_div, breach.listings), (Kind::Exchange, Some(751.5), None));
    assert_eq!(breach.points[6], Some(31.24));
    let star = src.items.iter().find(|i| i.name == "The Mutable Star" && i.listings == Some(4)).unwrap();
    assert_eq!(star.base_type.as_deref(), Some("Runemastered Cleric Vestments"));
    assert_eq!((star.kind, star.volume_div, star.price_div), (Kind::Listed, None, 27.5));
    assert_eq!(star.own_exalted_rate, 424.8, "the unique overview's own rate, not the exchange table's");
}

#[test]
fn bad_prices_volumes_and_points_are_dropped_and_counted() {
    let mut v: serde_json::Value = serde_json::from_str(BREACH).unwrap();
    v["lines"][0]["primaryValue"] = serde_json::json!(0.0);
    v["lines"][1]["primaryValue"] = serde_json::json!(-3.0);
    v["lines"][2]["volumePrimaryValue"] = serde_json::json!(-1.0);
    v["lines"][3]["sparkline"]["data"][2] = serde_json::json!(-100.0);
    let ov: ExchangeOverview = serde_json::from_value(v).unwrap();
    let src = Source::from_overviews("L", &[("Breach", &ov)], &[]);
    assert_eq!(src.items.len(), 3);
    assert_eq!((src.dropped.prices, src.dropped.volumes, src.dropped.points), (2, 1, 1));
    let no_volume = src.items.iter().find(|i| i.id == "sibilant-catalyst").unwrap();
    assert_eq!(no_volume.volume_div, None);
    assert_eq!(grade(no_volume, &Floors::default(), src.exalted_rate), Grade::Thin);
    let gap = src.items.iter().find(|i| i.id == "reaver-catalyst").unwrap();
    assert_eq!(gap.points[2], None, "a fall of all of the price is not a figure");
    let m = build(&src, &Options::default());
    assert!(m.items.iter().all(|g| g.trend.is_none_or(|t| t.change.is_finite())));
}

#[test]
fn an_item_on_four_listings_is_never_ranked() {
    let m = model();
    let star = find(&m, "The Mutable Star", Some("Runemastered Cleric Vestments"));
    // The raw figure that would top any naive list of risers.
    assert_eq!(star.item.points[6], Some(94304.0));
    assert_eq!(star.item.listings, Some(4));
    assert_eq!(star.grade, Grade::Untrusted);
    assert_eq!(star.trend, None, "an untrusted item carries no trend at all");
    let ranked: Vec<&market::Graded> = m.risers.iter().chain(&m.fallers).map(|&i| &m.items[i]).collect();
    assert!(!ranked.iter().any(|g| g.item.name == "The Mutable Star" && g.item.listings == Some(4)));
    // Its well-listed namesake is a different market and is judged alone:
    // deep, but priced at three exalted, where prices move in whole orbs.
    assert_eq!(find(&m, "The Mutable Star", Some("Cleric Vestments")).grade, Grade::Thin);

    // The listing floors: 9 is untrusted, 10 to 19 thin, 20 ranks.
    let mut item = star.item.clone();
    for (listings, want) in [(9, Grade::Untrusted), (10, Grade::Thin), (19, Grade::Thin), (20, Grade::Liquid)] {
        item.listings = Some(listings);
        assert_eq!(grade(&item, &Floors::default(), 474.2), want, "{listings} listings");
    }
    // Thin is listed but never ranked either.
    let thin = find(&m, "Temporalis", Some("Silk Robe"));
    assert_eq!((thin.grade, thin.item.listings), (Grade::Thin, Some(15)));
    assert!(thin.trend.is_some_and(|t| t.direction == Direction::Rising));
    assert!(!ranked_names(&m).contains(&"Temporalis"));
    assert!(ranked_names(&m).iter().all(|n| m.items.iter().any(|g| g.item.name == *n && g.grade == Grade::Liquid)));
}

#[test]
fn a_floor_priced_item_has_no_trend() {
    let m = model();
    // A unique listed at one exalted by its own overview's rate. 1,797
    // listings and a clean "falling" week, all of it the exalted orb's: a
    // seller cannot list below one orb, so the price cannot move.
    let coil = find(&m, "Lightning Coil", Some("Ancestral Mail"));
    assert_eq!(coil.item.kind, Kind::Listed);
    assert!(coil.item.price_div * coil.item.own_exalted_rate <= 1.0);
    assert_eq!(coil.item.points_used(), 5, "the history is there; it just means nothing");
    assert!(trend(&coil.item.points).is_some_and(|t| t.direction == Direction::Falling));
    assert!(coil.at_floor);
    assert_eq!((coil.grade, coil.trend), (Grade::Untrusted, None));
    assert!(!ranked_names(&m).contains(&"Lightning Coil"));
    // Visage of Ayah fell to the floor this week (-72.83%): how far it fell
    // is cut off by the floor, so that is no figure either.
    let visage = find(&m, "Visage of Ayah", Some("Beaded Circlet"));
    assert_eq!((visage.grade, visage.trend), (Grade::Untrusted, None));
    assert_eq!(m.floor_pinned, 2);
    // A unique well off the floor is not caught by it.
    let proto = find(&m, "Doryani's Prototype", Some("Runemastered Scale Mail"));
    assert!(!proto.at_floor && proto.grade == Grade::Liquid && proto.trend.is_some());
}

#[test]
fn the_floor_uses_the_items_own_overview_rate() {
    let m = model();
    let coil = find(&m, "Lightning Coil", Some("Ancestral Mail")).item.clone();
    // 0.00235 div is 0.998 ex at the armour overview's 424.8 and 1.11 ex at
    // the exchange table's 474.2. Judged by the table's rate it passed as
    // trusted, and 130 such uniques ranked at an identical -33.80%.
    assert_eq!(coil.own_exalted_rate, 424.8);
    assert!(coil.price_div * m.exalted_rate > 1.1);
    assert!(at_price_floor(&coil, m.exalted_rate));
    // One exalted, rounded by the source to 1.0001: still the floor.
    let mut rounded = coil.clone();
    rounded.price_div = 1.0004 / 424.8;
    assert!(at_price_floor(&rounded, m.exalted_rate));
    rounded.price_div = 1.001 / 424.8;
    assert!(!at_price_floor(&rounded, m.exalted_rate));
    // An overview without a rate of its own falls back to the table's.
    let mut rateless = coil.clone();
    rateless.own_exalted_rate = 0.0;
    assert!(!at_price_floor(&rateless, m.exalted_rate));
    rateless.price_div = 1.0 / 474.2;
    assert!(at_price_floor(&rateless, m.exalted_rate));
}

#[test]
fn an_exchange_item_under_one_exalted_keeps_its_trend() {
    let m = model();
    // A quarter of an exalted, traded at executed ratios: a real price.
    // What holds it back is its volume, and thin is what it is called.
    let cheap = find(&m, "Greater Essence of the Mind", None);
    assert!(cheap.item.price_div * m.exalted_rate < 1.0);
    assert!(!cheap.at_floor);
    assert_eq!(cheap.grade, Grade::Thin);
    assert!(cheap.trend.is_some());
    assert!(!ranked_names(&m).contains(&"Greater Essence of the Mind"), "thin is still never ranked");

    // With volume behind it, it ranks and enters its group's index: the
    // Exalted Orb's own fall against the divine is such a line.
    let week = [Some(-15.12), Some(-22.4), Some(-32.0), Some(-34.24), Some(-36.91), Some(-35.38), Some(-40.52)];
    let mut orb = exchange("Exalted Orb", 6972.0, week);
    orb.price_div = 0.002109;
    let steady = |n: &str| exchange(n, 100.0, climb(0.0));
    let built = build(&hand_made(vec![orb, steady("A"), steady("B")]), &Options::default());
    let orb = find(&built, "Exalted Orb", None);
    assert_eq!(orb.grade, Grade::Liquid);
    assert!(orb.trend.is_some_and(|t| t.direction == Direction::Falling && !t.is_flat()));
    assert_eq!(ranked_names(&built), ["Exalted Orb"]);
    assert_eq!(built.groups[0].coverage.items_used, 3);
}

#[test]
fn a_move_inside_its_own_band_is_not_a_mover() {
    let m = model();
    // Falling on four of five days, -47% in all, inside a band of 52: the
    // week swung further than it moved. (It is also listed at exactly one
    // chaos, so it is thin on the price grid; the band rule is what this
    // test is about.)
    let warden = find(&m, "Forgotten Warden", Some("Primal Markings"));
    let t = warden.trend.unwrap();
    assert_eq!((warden.grade, t.direction, t.band), (Grade::Thin, Direction::Falling, 52));
    assert!(t.is_flat() && !warden.is_mover());
    // "Falling -0.7%" inside a band of 55 used to be ranked.
    let star = find(&m, "The Mutable Star", Some("Cleric Vestments"));
    assert!(star.trend.is_some_and(|t| t.direction == Direction::Falling && t.is_flat()));
    assert!(!ranked_names(&m).contains(&"Forgotten Warden") && !ranked_names(&m).contains(&"The Mutable Star"));
    // Doryani's Prototype fell 77% against a band of 65: a mover.
    let proto = find(&m, "Doryani's Prototype", Some("Runemastered Scale Mail"));
    assert!(proto.is_mover());
    // Waveshaper moved +118% against a band of 95, at three exalted: the
    // move is outside its band and still unranked, on the price grid alone.
    let shaper = find(&m, "Waveshaper", Some("Tideseer Mantle"));
    assert!(shaper.grade == Grade::Thin && !shaper.is_mover());
    // Every mover is outside its band, and every such item is a mover.
    let ranked: Vec<usize> = m.risers.iter().chain(&m.fallers).copied().collect();
    let rule: Vec<usize> = (0..m.items.len())
        .filter(|&i| {
            let g = &m.items[i];
            g.grade == Grade::Liquid
                && g.trend.is_some_and(|t| t.direction != Direction::Unclear && t.change.abs() > f64::from(t.band))
        })
        .collect();
    let mut sorted = ranked.clone();
    sorted.sort_unstable();
    assert_eq!(sorted, rule);
    assert!(!ranked.is_empty());
    // A change exactly on the band is inside it.
    let edge = khaloni_poe2_core::market::Trend { change: -52.0, band: 52, direction: Direction::Falling, points_used: 7 };
    assert!(edge.is_flat());
}

#[test]
fn a_gap_in_the_history_is_never_a_zero() {
    let m = model();
    let warden = find(&m, "Forgotten Warden", Some("Primal Markings"));
    assert_eq!(
        warden.item.points,
        [Some(0.0), Some(-5.86), None, Some(-50.41), Some(-51.85), None, Some(-47.07)],
        "the two days the source lost stay lost"
    );
    let t = warden.trend.unwrap();
    // Band over the five figures that exist: 2 x stddev(0, -5.86, -50.41,
    // -51.85, -47.07) = 52.
    assert_eq!((t.band, t.points_used), (52, 5));
    // Waveshaper's five figures swing 95; with its two lost days read as
    // zeros the same week swings 91, and shows two collapses to the
    // starting price that never happened.
    let shaper = find(&m, "Waveshaper", Some("Tideseer Mantle"));
    assert_eq!(shaper.trend.unwrap().band, 95);
    let zero_filled = trend(&shaper.item.points.map(|p| Some(p.unwrap_or(0.0)))).unwrap();
    assert_eq!(zero_filled.band, 91);

    // The first figure is not always day one: the move is measured from
    // the first day that has one.
    let late = trend(&[None, Some(25.0), Some(30.0), Some(40.0), Some(45.0), None, Some(50.0)]).unwrap();
    assert!((late.change - 20.0).abs() < 1e-9, "(1.50 / 1.25 - 1) = +20%, not +50%: {}", late.change);

    // A group index has gaps where its items do.
    let gappy = |pct: f64| {
        let mut p = climb(pct);
        p[3] = None;
        p
    };
    let src = hand_made(vec![
        exchange("A", 100.0, gappy(10.0)),
        exchange("B", 100.0, gappy(20.0)),
        exchange("C", 100.0, gappy(30.0)),
    ]);
    let g = &build(&src, &Options::default()).groups[0];
    assert!(g.index_pct.is_some());
    assert_eq!(g.index_points[3], None);
    assert!(g.index_points[2].is_some() && g.index_points[4].is_some());
}

#[test]
fn five_of_seven_days_is_enough_and_says_so() {
    let m = model();
    let warden = find(&m, "Forgotten Warden", Some("Primal Markings"));
    let t = warden.trend.expect("five days with today among them is enough");
    assert_eq!((t.points_used, t.direction), (5, Direction::Falling));
    assert!((t.change - -47.07).abs() < 1e-9);
    assert_eq!(days_note(t.points_used).as_deref(), Some("5 of 7 days"));
    assert_eq!(days_note(7), None, "a full week needs no note");
    // Five days rank as well as seven do: Doryani's Prototype (95 ex, 53
    // listed) is a faller on five.
    let proto = find(&m, "Doryani's Prototype", Some("Runemastered Scale Mail"));
    assert_eq!(proto.trend.unwrap().points_used, 5);
    assert!(ranked_names(&m).contains(&"Doryani's Prototype"));

    // Four days are not.
    let four = find(&m, "Synthetic Four Days", Some("Primal Markings"));
    // Cut from Forgotten Warden, so it shares its one-chaos price grid.
    assert_eq!((four.grade, four.item.points_used()), (Grade::Thin, 4));
    assert_eq!(four.trend, None);
    assert_eq!(market::direction_text(four.trend.as_ref()), "not enough history");
    assert!(!ranked_names(&m).contains(&"Synthetic Four Days"));

    // Nor are six when today is the missing one.
    assert_eq!(trend(&[Some(0.0), Some(1.0), Some(2.0), Some(3.0), Some(4.0), Some(5.0), None]), None);
}

#[test]
fn the_direction_needs_the_week_to_agree_with_the_change() {
    // Up on the last day only: a change, not a direction.
    let spike = trend(&[Some(0.0), Some(-5.0), Some(-4.0), Some(-6.0), Some(-3.0), Some(-2.0), Some(12.0)]).unwrap();
    assert!(spike.change > 0.0);
    assert_eq!(spike.direction, Direction::Unclear);
    // The last three days above the first is enough.
    let late = trend(&[Some(0.0), Some(-5.0), Some(-4.0), Some(-6.0), Some(3.0), Some(2.0), Some(12.0)]).unwrap();
    assert_eq!(late.direction, Direction::Rising);
    // Four of the six later days below the first, ending lower.
    let sag = trend(&[Some(0.0), Some(-5.0), Some(-4.0), Some(-6.0), Some(-3.0), Some(2.0), Some(-1.0)]).unwrap();
    assert_eq!(sag.direction, Direction::Falling);
    // Flat is judged against the item's own band.
    assert!(sag.is_flat() && !late.is_flat());
}

#[test]
fn a_group_index_matches_the_hand_computed_basket() {
    let m = model();
    let breach = m.group("Breach").unwrap();
    // exp(sum(w ln(1 + change/100)) / sum(w)) - 1 over the five catalysts
    // and stones, w their traded volume (worked in the fixture notes).
    assert!((breach.index_pct.unwrap() - 84.634_843_430_713_57).abs() < 1e-9);
    assert!((breach.volume_div - 10_003.3).abs() < 1e-9);
    assert_eq!((breach.coverage.items_used, breach.coverage.items_total), (5, 5));
    assert!((breach.coverage.volume_share - 1.0).abs() < 1e-12);
    // Sibilant Catalyst moved +24% inside a band of 22: up. The refined one
    // moved +150% inside a band of 185: flat, by its own measure.
    assert_eq!((breach.breadth.up, breach.breadth.down, breach.breadth.flat, breach.breadth.of), (3, 0, 2, 5));
    assert_eq!(breach.verdict, Verdict::Rising);
    assert!(breach.is_mechanic);
    assert_eq!(
        coverage_text(breach),
        "index from 5 of 5 items (100% of the group's volume)",
        "every index states what it rests on"
    );
    // The sparkline is on the index's own basis: its last day IS the
    // figure, and its first day is the start it is measured from.
    assert!((breach.index_points[6].unwrap() - breach.index_pct.unwrap()).abs() < 1e-9);
    assert!(breach.index_points[0].unwrap().abs() < 1e-9);
    // Groups rank by traded volume and share out all exchange volume.
    assert_eq!(m.groups[0].category, "Breach");
    let shares: f64 = m.groups.iter().map(|g| g.volume_share).sum();
    assert!((shares - 1.0).abs() < 1e-12);
    assert!(!m.groups.iter().any(|g| g.category.starts_with("Unique")), "uniques are not a group");
    assert!(market::is_mechanic("Verisium") && !market::is_mechanic("Currency") && !market::is_mechanic("Runes"));
}

#[test]
fn one_runaway_item_cannot_carry_a_group_index() {
    let mut items: Vec<MarketItem> = (0..4).map(|i| exchange(&format!("Steady {i}"), 100.0, climb(0.0))).collect();
    items.push(exchange("Runaway", 100.0, climb(900.0)));
    let g = &build(&hand_made(items), &Options::default()).groups[0];
    // Tenfold on a fifth of the volume: 10^(1/5) - 1 = +58.5%. The
    // arithmetic mean would say +180%.
    let index = g.index_pct.unwrap();
    assert!((index - (10f64.powf(0.2) - 1.0) * 100.0).abs() < 1e-9, "{index}");
    assert!(index < 60.0);
    // And the group is not called rising on the strength of it: one item
    // up, none down, four flat, so the item is named instead.
    assert_eq!((g.breadth.up, g.breadth.flat), (1, 4));
    assert_eq!(g.verdict, Verdict::CarriedBy(vec!["Runaway".to_string()]));
    assert_eq!(market::verdict_text(&g.verdict), "carried by Runaway");
}

#[test]
fn a_group_too_thin_to_index_says_so() {
    // Three good items, but the fourth holds most of the volume and has no
    // history: the basket speaks for 37.5% of the group.
    let mut items: Vec<MarketItem> = (0..3).map(|i| exchange(&format!("Known {i}"), 100.0, climb(10.0))).collect();
    items.push(exchange("Unknown", 500.0, [None, None, None, None, None, Some(0.0), Some(4.0)]));
    let g = build(&hand_made(items), &Options::default()).groups.remove(0);
    assert_eq!(g.index_pct, None);
    assert_eq!(g.verdict, Verdict::Unindexed);
    assert_eq!(g.index_points, [None; 7]);
    assert!((g.coverage.volume_share - 0.375).abs() < 1e-12);
    assert_eq!(coverage_text(&g), "too thin to index: 3 of 4 items hold 38% of the group's volume");

    // Full coverage, but two items are not a group.
    let two = vec![exchange("A", 100.0, climb(10.0)), exchange("B", 100.0, climb(12.0))];
    let g = build(&hand_made(two), &Options::default()).groups.remove(0);
    assert_eq!(g.index_pct, None);
    assert!(coverage_text(&g).starts_with("too thin to index: 2 of 2 items"));

    // The real case: Lesser Essence of Battle trades 235 div on two days
    // of history, and thin essences are outside the basket, which still
    // holds 81% of the fixture's essence volume.
    let essences = model().group("Essences").cloned().unwrap();
    assert_eq!((essences.coverage.items_used, essences.coverage.items_total), (4, 10));
    assert!(essences.index_pct.is_some());
    assert!(coverage_text(&essences).starts_with("index from 4 of 10 items (81%"), "{}", coverage_text(&essences));
    // Thin items sit under the liquid ones; no exchange item is untrusted.
    let m = model();
    let grades: Vec<Grade> = essences.items.iter().map(|&i| m.items[i].grade).collect();
    let first_thin = grades.iter().position(|g| *g == Grade::Thin).unwrap();
    assert!(grades[..first_thin].iter().all(|g| *g == Grade::Liquid));
    assert!(grades[first_thin..].iter().all(|g| *g == Grade::Thin));
}

#[test]
fn a_young_league_shows_prices_but_no_rankings() {
    let young = build(&source(), &Options { young: true, ..Options::default() });
    assert!(young.young);
    assert!(young.risers.is_empty() && young.fallers.is_empty());
    assert!(young.groups.iter().all(|g| g.index_pct.is_none() && g.verdict == Verdict::Unindexed));
    assert!(young.items.iter().all(|g| g.trend.is_none()));
    // Prices are all still there, in the three currencies.
    assert_eq!(young.items.len(), source().items.len());
    let stone = find(&young, "Breachstone", None);
    let price = young.price(&stone.item);
    assert_eq!((price.divine, price.chaos), (2.48, 2.48 * 8.32));
    // Not ranked by volume either: the fixed mechanic order.
    let order: Vec<&str> = young.groups.iter().map(|g| g.category.as_str()).collect();
    assert_eq!(order, ["Breach", "Essences"]);
    assert!(!model().risers.is_empty(), "the same data ranks once the league is older");

    let (hour, day) = (3600, 86_400);
    let now = 1_800_000_000;
    // A league list that carries the start decides alone.
    assert!(league_is_young(now, Some(now - 71 * hour), None, 7));
    assert!(!league_is_young(now, Some(now - 72 * hour), None, 0));
    // Without one, the first record this install wrote stands in...
    assert!(league_is_young(now, None, Some(now - 2 * day), 2));
    assert!(!league_is_young(now, None, Some(now - 4 * day), 2));
    // ...unless the overviews carry figures older than that: an install
    // that met the league in its third month must not call it young.
    assert!(!league_is_young(now, None, Some(now - hour), 7));
    assert!(!league_is_young(now, None, None, 4));
    assert!(league_is_young(now, None, None, 3));
    assert_eq!(source().history_days(), 7);
}

#[test]
fn floors_are_validated_and_move_the_grades() {
    assert_eq!(Floors::default(), Floors { volume_div: 5.0, listings_rank: 20, listings_min: 10 });
    assert!(Floors { volume_div: -1.0, ..Floors::default() }.problem().is_some());
    assert!(Floors { volume_div: f64::NAN, ..Floors::default() }.problem().is_some());
    assert!(Floors { listings_min: 0, ..Floors::default() }.problem().is_some());
    assert!(Floors { listings_min: 30, ..Floors::default() }.problem().is_some());
    assert!(Floors { volume_div: 0.0, listings_min: 20, listings_rank: 20 }.problem().is_none());
    assert_eq!(Floors { listings_min: 30, ..Floors::default() }.or_default(), Floors::default());

    // Perfect Essence of Haste trades 4.97 div: thin at the default floor
    // of 5, liquid at 1.
    let m = model();
    assert_eq!(find(&m, "Perfect Essence of Haste", None).grade, Grade::Thin);
    let loose = build(&source(), &Options { floors: Floors { volume_div: 1.0, ..Floors::default() }, young: false });
    assert_eq!(find(&loose, "Perfect Essence of Haste", None).grade, Grade::Liquid);
}

#[test]
fn no_label_names_a_period_for_the_volume() {
    let m = model();
    let mut said = vec![market::volume_text(1234.5), market::band_text(12), market::percent_text(3.21)];
    for g in &m.groups {
        said.push(coverage_text(g));
        said.push(market::breadth_text(&g.breadth));
        said.push(market::verdict_text(&g.verdict));
    }
    for text in said {
        let t = text.to_lowercase();
        for banned in ["income", "profit", "per day", "per hour", "/hr", "/h", "daily", "hourly"] {
            assert!(!t.contains(banned), "{text:?} names {banned:?}");
        }
    }
    assert_eq!(market::volume_text(1234.5), "1235 div");
    assert_eq!(market::band_text(12), "±12% over 7 days");
    assert_eq!(market::percent_text(-0.44), "-0.4%");
    assert_eq!(market::percent_text(117.8), "+118%");
    assert_eq!(market::age_text(3 * 3600 + 5 * 60), "3 h 05 min");
}

#[test]
fn a_unique_priced_in_whole_orbs_is_never_ranked() {
    // Deeply listed, clearly "moving", and priced at two exalted: its divine
    // history is the exalted orb's drift, so it keeps its price and no rank.
    use khaloni_poe2_core::market::{grade, is_coarsely_priced, Floors, Grade, Kind, MarketItem};
    let mut item = MarketItem {
        category: "UniqueArmours".into(),
        id: "two-orb-hat".into(),
        name: "Two Orb Hat".into(),
        base_type: Some("Cap".into()),
        corrupted: false,
        kind: Kind::Listed,
        price_div: 2.0 / 440.0,
        volume_div: None,
        listings: Some(3000),
        points: [Some(0.0), Some(-10.0), Some(-20.0), Some(-25.0), Some(-30.0), Some(-32.0), Some(-33.76)],
        own_exalted_rate: 440.0,
        own_chaos_rate: 7.8,
    };
    assert!(is_coarsely_priced(&item, 0.0));
    assert_eq!(grade(&item, &Floors::default(), 474.2), Grade::Thin);
    // The same market at fifty exalted is ranked.
    item.price_div = 50.0 / 440.0;
    assert!(!is_coarsely_priced(&item, 0.0));
    assert_eq!(grade(&item, &Floors::default(), 474.2), Grade::Liquid);
    // An exchange item under ten exalted is an executed price and stays.
    item.kind = Kind::Exchange;
    item.listings = None;
    item.volume_div = Some(500.0);
    item.price_div = 2.0 / 440.0;
    assert_eq!(grade(&item, &Floors::default(), 474.2), Grade::Liquid);
}

#[test]
fn a_tracked_unique_gets_its_ninja_price_band_and_direction() {
    use khaloni_poe2_core::market::{band_text, ninja_block, volume_text};
    let m = model();
    let proto = find(&m, "Doryani's Prototype", Some("Runemastered Scale Mail"));
    let block = ninja_block(&m, "Doryani's Prototype", Some("Runemastered Scale Mail"), false, None)
        .expect("a listed unique is tracked");
    assert_eq!(block.price_div, proto.item.price_div);
    assert_eq!(block.grade, Grade::Liquid);
    let trend = block.trend.expect("a liquid item with a week of history has a trend");
    assert_eq!(trend, proto.trend.unwrap());
    // Five of the week's days carry a figure, and the note says so.
    assert_eq!(trend.points_used, 5);
    assert_eq!(block.note, format!("{} · 5 of 7 days", market::direction_text(Some(&trend))));
    assert_eq!(block.note, "falling · 5 of 7 days");
    assert_eq!(block.band_text, band_text(trend.band));
    assert_eq!(block.volume_text, "53 listed");
    // The name is matched without regard to case, and a category hint
    // narrows the search without changing the answer.
    let again = ninja_block(&m, "doryani's prototype", Some("runemastered scale mail"), false, Some("UniqueArmours"));
    assert_eq!(again.as_ref().map(|b| b.price_div), Some(proto.item.price_div));
    assert!(ninja_block(&m, "Doryani's Prototype", Some("Runemastered Scale Mail"), false, Some("Essences")).is_none());

    // An exchange item carries its traded volume, no unit of time.
    let stone = ninja_block(&m, "Breachstone", None, false, None).expect("tracked");
    assert_eq!(stone.volume_text, volume_text(751.5));
    assert!(!stone.volume_text.contains("day") && !stone.volume_text.contains("/"));
}

#[test]
fn a_thin_or_untrusted_item_gets_the_same_wording_as_the_market_view() {
    use khaloni_poe2_core::market::{ninja_block, note_text, THIN_MARKET};
    let m = model();
    // Thin: the direction is still said, then the market view's own words.
    let temporalis = ninja_block(&m, "Temporalis", Some("Silk Robe"), false, None).expect("tracked");
    assert_eq!(temporalis.grade, Grade::Thin);
    assert!(temporalis.trend.is_some());
    assert_eq!(temporalis.note, format!("rising · 5 of 7 days · {THIN_MARKET}"));
    assert_eq!(temporalis.note, note_text(find(&m, "Temporalis", Some("Silk Robe")), m.young));
    // Untrusted: no trend words at all, the market view's reason instead.
    let star =
        ninja_block(&m, "The Mutable Star", Some("Runemastered Cleric Vestments"), false, None).expect("tracked");
    assert_eq!((star.grade, star.trend), (Grade::Untrusted, None));
    assert_eq!(star.note, "untrusted: 4 listings");
    assert_eq!(star.band_text, "");
    let coil = ninja_block(&m, "Lightning Coil", Some("Ancestral Mail"), false, None).expect("tracked");
    assert_eq!(coil.note, "untrusted: listed at the 1 ex floor");
    // Too little history is said as such, with how little there was.
    let four = ninja_block(&m, "Synthetic Four Days", Some("Primal Markings"), false, None).expect("tracked");
    assert_eq!(four.trend, None);
    assert_eq!(four.note, "not enough history · 4 of 7 days · priced in whole orbs");
    // A young league says so once and nothing about direction.
    let young = build(&source(), &Options { young: true, ..Options::default() });
    let temporalis = ninja_block(&young, "Temporalis", Some("Silk Robe"), false, None).expect("tracked");
    assert_eq!((temporalis.trend, temporalis.note.as_str()), (None, market::TOO_YOUNG));
}

#[test]
fn an_untracked_item_gets_no_ninja_block() {
    use khaloni_poe2_core::market::ninja_block;
    let m = model();
    assert!(ninja_block(&m, "Headhunter", Some("Heavy Belt"), false, None).is_none());
    // The corrupted copy is its own market; the fixture tracks only the clean one.
    assert!(ninja_block(&m, "Doryani's Prototype", Some("Runemastered Scale Mail"), true, None).is_none());
    // A name on two bases with no base to tell them apart is no answer.
    assert!(ninja_block(&m, "The Mutable Star", None, false, None).is_none());
    // A name on one base answers without the base.
    assert!(ninja_block(&m, "Temporalis", None, false, None).is_some());
    assert!(ninja_block(&m, "", None, false, None).is_none());
}

#[test]
fn a_unique_priced_in_whole_chaos_is_never_ranked() {
    // Eight uniques at 0.1279 div (one chaos at 7.8 to the divine) shared
    // one move of +8.85% on 2026-09-26: the chaos orb's, not theirs.
    use khaloni_poe2_core::market::{grade, is_coarsely_priced, Floors, Grade, Kind, MarketItem};
    let mut item = MarketItem {
        category: "UniqueArmours".into(),
        id: "one-chaos-hat".into(),
        name: "One Chaos Hat".into(),
        base_type: Some("Iron Crown".into()),
        corrupted: false,
        kind: Kind::Listed,
        price_div: 1.0 / 7.8,
        volume_div: None,
        listings: Some(923),
        points: [Some(0.0), Some(2.0), Some(4.0), Some(6.0), Some(7.0), Some(8.0), Some(8.85)],
        own_exalted_rate: 486.9,
        own_chaos_rate: 7.8,
    };
    assert!(is_coarsely_priced(&item, 0.0), "one chaos is on the chaos grid");
    assert_eq!(grade(&item, &Floors::default(), 486.9), Grade::Thin);
    item.price_div = 3.0 / 7.8;
    assert!(is_coarsely_priced(&item, 0.0), "three chaos is too");
    // 3.37 chaos is a real price, and so is 25 chaos.
    item.price_div = 3.37 / 7.8;
    assert!(!is_coarsely_priced(&item, 0.0));
    item.price_div = 25.0 / 7.8;
    assert!(!is_coarsely_priced(&item, 0.0));
    assert_eq!(grade(&item, &Floors::default(), 486.9), Grade::Liquid);
}
