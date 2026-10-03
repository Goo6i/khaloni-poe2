//! The suggested price is read off the listings closest to the checked
//! item by mod and tier, and every figure it shows names a listing. The
//! listings are the real fetch entries in fixtures/trade_fetch_full.json;
//! the checked item is the corpus's Soldier Cuirass, built the way the
//! price check builds it.

use khaloni_poe2_core::ee2::{data, Ee2Data};
use khaloni_poe2_core::listing::{parse_fetch_body, Card, CardLine, LineKind, ListingView, SellerState};
use khaloni_poe2_core::suggest::{
    attribution, closest, distance, ladder_text, mod_key, ours_from_built, short_name, theirs_from_card, tier_of,
    OurMod, TheirMod,
};
use serde_json::Value;
use std::path::{Path, PathBuf};

const NOW: i64 = 1_790_000_000;

fn fixture_rows() -> Vec<ListingView> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/trade_fetch_full.json");
    let body: Value = serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("fixture json");
    parse_fetch_body(&body, NOW, "nobody").0
}

fn parity_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../tools/ee2-parity")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn load_data() -> Ee2Data {
    let dir = parity_dir().join("data");
    let mut db = Ee2Data::from_ndjson(&read(&dir.join("stats.ndjson")), &read(&dir.join("items.ndjson")))
        .expect("pinned ndjson parses");
    db.trade_stats = Some(data::TradeStatTexts::from_json(&read(&dir.join("trade-stats.json"))).expect("trade stats"));
    db.trade_items = Some(data::trade_item_names(&read(&dir.join("trade-items.json"))).expect("trade items"));
    db
}

/// The corpus's "Hate Pelt", a Soldier Cuirass like the fixture's listings.
fn hate_pelt() -> khaloni_poe2_core::ee2::request::Built {
    let text = read(&parity_dir().join("items/ee2-ArmourHighValueRareItem.txt"));
    khaloni_poe2_core::ee2::request::build(&text, &load_data()).expect("the corpus item builds")
}

fn our(key: &str, tier: Option<u8>) -> OurMod {
    OurMod { key: mod_key(key), tier }
}

fn their(key: &str, tier: Option<u8>) -> TheirMod {
    TheirMod { key: mod_key(key), tier }
}

/// A listing with these card lines at this price, everything else blank.
fn listing(lines: Vec<CardLine>, price: f64) -> (ListingView, f64) {
    let view = ListingView {
        price: Some((price, "divine".into())),
        indexed_unix: NOW,
        age_text: "just now".into(),
        seller: "seller".into(),
        state: SellerState::Online,
        is_mine: false,
        stack: None,
        ilvl: None,
        quality: None,
        gem_level: None,
        corrupted: false,
        has_note: false,
        instant_buyout: false,
        card: Card { name: String::new(), base: String::new(), rarity: "Rare".into(), figures: Vec::new(), lines },
    };
    (view, price)
}

fn line(text: &str, tier: &str, kind: LineKind) -> CardLine {
    CardLine { text: text.into(), tiers: if tier.is_empty() { Vec::new() } else { vec![tier.into()] }, kind }
}

fn as_ours(theirs: &[TheirMod]) -> Vec<OurMod> {
    theirs.iter().map(|t| OurMod { key: t.key.clone(), tier: t.tier }).collect()
}

#[test]
fn a_mod_key_reads_the_same_from_our_card_and_a_listing() {
    // The checked item's row wording against the API's description.
    assert_eq!(mod_key("+45 to maximum Life"), mod_key("+52 to maximum Life"));
    assert_eq!(mod_key("+45 to maximum Life"), "+# to maximum life");
    assert_eq!(mod_key("+#% to Fire Resistance"), mod_key("+35% to [Resistances|Fire Resistance]"));
    assert_eq!(mod_key("Adds # to # Physical Damage"), mod_key("Adds 12 to 20 Physical Damage"));
    assert_eq!(mod_key("11.9 Life Regeneration per second"), mod_key("33.4  Life Regeneration per second"));
    assert_eq!(tier_of("P1"), Some(1));
    assert_eq!(tier_of("S3"), Some(3));
    assert_eq!(tier_of("P1 + S2"), Some(1));
    assert_eq!(tier_of(""), None);

    // A real pair: the corpus's Soldier Cuirass against the fixture's
    // "Soul Skin", which shares its Strength and bleeding suffixes.
    let ours = ours_from_built(&hate_pelt());
    let rows = fixture_rows();
    let soul_skin = rows.iter().find(|r| r.card.name == "Soul Skin").expect("Soul Skin is in the fixture");
    let theirs = theirs_from_card(&soul_skin.card);
    for want in ["+32 to Strength", "48% reduced Duration of Bleeding on You", "+70 to Armour"] {
        let key = mod_key(want);
        assert!(ours.iter().any(|m| m.key == key), "our item carries {want}: {ours:?}");
        assert!(theirs.iter().any(|m| m.key == key), "Soul Skin carries {want}: {theirs:?}");
    }
    // The tier rides along from the item text on our side and from the
    // API's badge on theirs.
    let strength = mod_key("+32 to Strength");
    assert_eq!(ours.iter().find(|m| m.key == strength).unwrap().tier, Some(1));
    let desecrated = ours.iter().filter(|m| m.key == mod_key("+256 to Armour")).map(|m| m.tier).collect::<Vec<_>>();
    assert_eq!(desecrated, [Some(1), Some(2)], "Hardened's +70 and the desecrated +256 each keep their own tier");
    assert!(ours.iter().all(|m| m.tier.is_some()), "every line of the item carries its header's tier: {ours:?}");
    assert_eq!(theirs.iter().find(|m| m.key == strength).unwrap().tier, Some(1));
    // Rune lines are never a mod to compare.
    assert!(!theirs.iter().any(|m| m.key.contains("bonded")), "{theirs:?}");
}

#[test]
fn an_identical_listing_is_at_distance_zero() {
    let rows = fixture_rows();
    let plague: Vec<&ListingView> = rows.iter().filter(|r| r.card.name == "Plague Curtain").collect();
    assert_eq!(plague.len(), 3, "the fixture lists Plague Curtain three times");
    let ours = as_ours(&theirs_from_card(&plague[0].card));
    assert!(!ours.is_empty());
    for row in &plague {
        assert_eq!(distance(&ours, &theirs_from_card(&row.card)), 0);
    }
    // Its rune lines count for nothing on either side.
    assert!(plague[0].card.lines.iter().any(|l| l.kind == LineKind::Rune));
}

#[test]
fn a_tier_apart_costs_the_tier_difference_and_a_missing_mod_costs_three() {
    let ours = vec![our("+# to maximum Life", Some(1)), our("+#% to Fire Resistance", Some(2))];
    let two_tiers_down = vec![their("+# to maximum Life", Some(3)), their("+#% to Fire Resistance", Some(2))];
    assert_eq!(distance(&ours, &two_tiers_down), 2);
    let no_fire = vec![their("+# to maximum Life", Some(1))];
    assert_eq!(distance(&ours, &no_fire), 3);
    let untiered = vec![their("+# to maximum Life", None), their("+#% to Fire Resistance", Some(2))];
    assert_eq!(distance(&ours, &untiered), 1, "a tier unknown on either side is one step");
    let ours_untiered = vec![our("+# to maximum Life", None)];
    assert_eq!(distance(&ours_untiered, &no_fire), 1);
    assert_eq!(distance(&ours, &[]), 6);
}

#[test]
fn an_extra_searchable_mod_on_theirs_costs_one() {
    let ours = vec![our("+# to maximum Life", Some(1))];
    let card = Card {
        name: String::new(),
        base: String::new(),
        rarity: "Rare".into(),
        figures: Vec::new(),
        lines: vec![
            line("+52 to maximum Life", "P1", LineKind::Explicit),
            line("+35% to Cold Resistance", "S3", LineKind::Crafted),
            line("Bonded: +60 to maximum Life", "", LineKind::Rune),
            line("10% increased Rarity of Items found", "", LineKind::Enchant),
        ],
    };
    let theirs = theirs_from_card(&card);
    assert_eq!(theirs.len(), 2, "rune and enchant lines are not searchable: {theirs:?}");
    assert_eq!(distance(&ours, &theirs), 1);
    // Desecrated, fractured and implicit lines are.
    let more = vec![
        line("+52 to maximum Life", "P1", LineKind::Explicit),
        line("+70 to Armour", "P2", LineKind::Desecrated),
        line("30% reduced Attribute Requirements", "S2", LineKind::Fractured),
        line("5% increased Movement Speed", "", LineKind::Implicit),
    ];
    assert_eq!(distance(&ours, &theirs_from_card(&Card { lines: more, ..card.clone() })), 3);
}

#[test]
fn closest_names_the_listings_within_two_and_their_prices() {
    let rows = fixture_rows();
    let plague = rows.iter().find(|r| r.card.name == "Plague Curtain").unwrap();
    // Our item is Plague Curtain with its life a tier lower: the three
    // Plague Curtain listings sit at distance one, the rest far off.
    let mut ours = as_ours(&theirs_from_card(&plague.card));
    let life = ours.iter_mut().find(|m| m.key == mod_key("+33 to maximum Life")).expect("life is on the card");
    assert_eq!(life.tier, Some(2));
    life.tier = Some(3);
    let listings: Vec<(ListingView, f64)> =
        rows.iter().filter_map(|r| Some((r.clone(), r.price.as_ref()?.0))).collect();
    let c = closest(&ours, &listings, "div");
    let within: Vec<usize> = c.within.iter().map(|(i, _)| *i).collect();
    assert_eq!(within.len(), 3);
    assert!(c.within.iter().all(|(_, d)| *d == 1), "{:?}", c.within);
    let names: Vec<&str> = within.iter().map(|&i| listings[i].0.card.name.as_str()).collect();
    assert_eq!(names, ["Plague Curtain"; 3]);
    let prices: Vec<f64> = within.iter().map(|&i| listings[i].1).collect();
    assert_eq!(prices, [3.0, 23.0, 24.0], "sorted by price inside the same distance");
    assert_eq!(c.nearest.as_ref().map(|(i, d, _)| (*i, *d)), Some((within[0], 1)));
    assert_eq!(
        c.text,
        "3 listings within one tier: 3, 23, 24 div; the nearest has T1 increased armour, T2 life against your T3, \
         T6 life regeneration per second, T1 reduced poison duration on you, T3 cold res, T2 armour"
    );

    // With the life tier put back, the same three listings match at every
    // tier and the text says so.
    ours.iter_mut().find(|m| m.key == mod_key("+33 to maximum Life")).unwrap().tier = Some(2);
    let c = closest(&ours, &listings, "div");
    assert!(c.within.iter().all(|(_, d)| *d == 0));
    let lead = "3 listings with your mods at your tiers: 3, 23, 24 div; the nearest has ";
    assert!(c.text.starts_with(lead), "{}", c.text);
    assert!(c.text.ends_with(" like yours"), "{}", c.text);
}

#[test]
fn under_two_close_listings_says_so_and_names_the_nearest_difference() {
    let ours = vec![our("+# to maximum Life", Some(1)), our("+#% to Cold Resistance", Some(1))];
    let listings = vec![
        // Life two tiers down and no cold res: 2 + 3.
        listing(vec![line("+40 to maximum Life", "P3", LineKind::Explicit)], 9.0),
        // The same, plus a fire res we lack: 2 + 3 + 1.
        listing(
            vec![
                line("+40 to maximum Life", "P3", LineKind::Explicit),
                line("+30% to Fire Resistance", "S2", LineKind::Explicit),
            ],
            4.0,
        ),
    ];
    let c = closest(&ours, &listings, "ex");
    assert!(c.within.is_empty());
    let (idx, dist, diff) = c.nearest.clone().expect("a nearest listing");
    assert_eq!((idx, dist), (0, 5));
    assert_eq!(diff, "T1 life vs T3, no cold res");
    assert_eq!(c.text, "no close match among the cheapest 2; nearest differs by: T1 life vs T3, no cold res");

    // One close listing is named alone, with what sets it apart.
    let one = vec![
        listing(
            vec![
                line("+40 to maximum Life", "P1", LineKind::Explicit),
                line("+30% to Cold Resistance", "S1", LineKind::Explicit),
                line("+30% to Fire Resistance", "S2", LineKind::Explicit),
            ],
            12.5,
        ),
        listings[0].clone(),
    ];
    let c = closest(&ours, &one, "ex");
    assert_eq!(c.within, vec![(0, 1)]);
    assert_eq!(c.text, "one close listing among the cheapest 2: 12.5 ex; it differs by: T2 fire res besides");
    assert_eq!(c.nearest.map(|(i, d, _)| (i, d)), Some((0, 1)));

    // Nothing fetched: nothing to compare with, and no figure.
    let c = closest(&ours, &[], "ex");
    assert!(c.within.is_empty() && c.nearest.is_none());
    assert_eq!(c.text, "no listings to compare with");
}

#[test]
fn the_ladder_reads_the_cheapest_prices_and_the_total() {
    assert_eq!(
        ladder_text(&[2.0, 5.0, 6.0, 6.0, 8.0, 9.0, 10.0], Some(1934), "ex"),
        "cheapest 2, then 5, 6, 6, 8 ex · 7 of 1,934 matched"
    );
    assert_eq!(ladder_text(&[2.5], Some(1), "div"), "cheapest 2.5 div · 1 of 1 matched");
    assert_eq!(ladder_text(&[1.25, 1200.0], None, "ex"), "cheapest 1.3, then 1,200 ex · 2 listings");
}

#[test]
fn the_ladder_reads_cheapest_first_whatever_order_the_listings_came_in() {
    assert_eq!(
        ladder_text(&[3.0, 3.0, 5.0, 12.0, 1.0, 1.0, 24.0], Some(1934), "div"),
        "cheapest 1, then 1, 3, 3, 5 div · 7 of 1,934 matched"
    );
}

#[test]
fn the_ladder_says_nothing_without_prices() {
    assert_eq!(ladder_text(&[], Some(1934), "ex"), "");
    assert_eq!(ladder_text(&[], None, "ex"), "");
}

#[test]
fn attribution_reads_with_and_without_and_names_both_searches() {
    let rows = attribution(40.0, &[("T1 life".to_string(), Some(9.0)), ("T2 fire res".to_string(), Some(38.5))], "ex");
    assert_eq!(rows.len(), 2);
    assert_eq!((rows[0].label.as_str(), rows[0].with, rows[0].without), ("T1 life", 40.0, Some(9.0)));
    assert_eq!(rows[0].text, "T1 life: with 40 ex, without 9 ex");
    assert_eq!(rows[1].text, "T2 fire res: with 40 ex, without 38.5 ex");
}

#[test]
fn a_dropped_search_that_found_nothing_reports_no_figure() {
    let rows = attribution(40.0, &[("T1 life".to_string(), None)], "ex");
    assert_eq!(rows[0].without, None);
    assert_eq!(rows[0].text, "T1 life: without it nothing matched");
    assert!(!rows[0].text.contains('0'), "no figure is invented for an empty search");
}

#[test]
fn short_names_read_like_the_card() {
    assert_eq!(short_name(&mod_key("+45 to maximum Life")), "life");
    assert_eq!(short_name(&mod_key("+35% to Fire Resistance")), "fire res");
    assert_eq!(short_name(&mod_key("136% increased Armour")), "increased armour");
    assert_eq!(short_name(&mod_key("+277 to Armour")), "armour");
    assert_eq!(short_name(&mod_key("+13 to maximum Energy Shield")), "energy shield");
}

