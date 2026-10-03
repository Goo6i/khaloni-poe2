//! One fetch entry becomes a listing row and a hover card; rows fold and
//! flag price-fixing the way EE2's table does (trade-api.ts grouping,
//! TradeListing.vue's isLikelyPriceFixed). The fixture is cut from real
//! fetch responses and keeps a `null` entry and a listing without a price.

use khaloni_poe2_core::listing::{
    age_text, display_text, group, group_indices, parse_entry, parse_fetch_body, price_fixed, GroupedListing,
    LineKind, ListingView, SellerState,
};
use serde_json::{json, Value};

const NOW: i64 = 1_790_000_000;
const ME: &str = "seller3";

fn fixture() -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/trade_fetch_full.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("fixture json")
}

/// A minimal fetch entry: seller, price, and the item fields a test sets.
fn entry(seller: &str, price: Option<(f64, &str)>, item: Value, online: Option<&str>) -> Value {
    let mut listing = json!({
        "indexed": "2026-09-19T08:35:28Z",
        "account": { "name": seller },
    });
    if let Some((amount, currency)) = price {
        listing["price"] = json!({ "type": "~b/o", "amount": amount, "currency": currency });
    }
    if let Some(status) = online {
        listing["account"]["online"] = json!({ "status": status });
    }
    json!({ "id": "x", "listing": listing, "item": item })
}

fn row(seller: &str, amount: f64, currency: &str) -> ListingView {
    parse_entry(&entry(seller, Some((amount, currency)), json!({ "name": "", "typeLine": "Chaos Orb", "rarity": "Currency" }), None), NOW, ME)
        .expect("entry parses")
}

fn stacked(seller: &str, amount: f64, currency: &str, stack: u32) -> ListingView {
    let item = json!({ "name": "", "typeLine": "Chaos Orb", "rarity": "Currency", "stackSize": stack });
    parse_entry(&entry(seller, Some((amount, currency)), item, None), NOW, ME).expect("entry parses")
}

fn times(groups: &[GroupedListing]) -> Vec<u32> {
    groups.iter().map(|g| g.times).collect()
}

#[test]
fn a_null_entry_and_a_priceless_listing_are_reported_not_fatal() {
    assert!(parse_entry(&Value::Null, NOW, ME).is_none());

    let (rows, dropped) = parse_fetch_body(&fixture(), NOW, ME);
    assert_eq!(rows.len(), 17, "the fixture has 18 entries, one of them null");
    assert_eq!(dropped, 2, "one null and one listing without a price");
    let priceless: Vec<&ListingView> = rows.iter().filter(|r| r.price.is_none()).collect();
    assert_eq!(priceless.len(), 1);
    assert_eq!(priceless[0].seller, "seller6");
    assert_eq!(priceless[0].card.name, "Soul Skin");
    assert!(rows.iter().filter(|r| r.price.is_some()).all(|r| r.price.as_ref().unwrap().1 == "divine"));
}

#[test]
fn the_row_fields_read_age_seller_state_stack_ilvl_quality_gem_level() {
    let (rows, _) = parse_fetch_body(&fixture(), NOW, ME);
    let sol_wrap = &rows[3];
    assert_eq!(sol_wrap.card.name, "Sol Wrap");
    assert_eq!(sol_wrap.price, Some((5.0, "divine".to_string())));
    assert_eq!(sol_wrap.indexed_unix, 1_789_806_928, "2026-09-19T08:35:28Z");
    assert_eq!(sol_wrap.age_text, "2 d");
    assert_eq!(sol_wrap.seller, "seller3");
    assert_eq!(sol_wrap.state, SellerState::Afk);
    assert!(sol_wrap.is_mine);
    assert_eq!(sol_wrap.stack, None);
    assert_eq!(sol_wrap.ilvl, Some(81));
    assert_eq!(sol_wrap.quality, Some(20), "+20% reads as 20");
    assert_eq!(sol_wrap.gem_level, None);
    assert!(!sol_wrap.corrupted);
    assert!(sol_wrap.has_note);
    assert!(sol_wrap.instant_buyout);

    assert_eq!(sol_wrap.state.as_str(), "afk");

    let corpse_suit = &rows[4];
    assert_eq!(corpse_suit.state, SellerState::Online);
    assert_eq!(corpse_suit.state.as_str(), "online");
    assert_eq!(SellerState::Offline.as_str(), "offline");
    assert!(!corpse_suit.is_mine);
    assert_eq!(rows[0].state, SellerState::Offline);
    assert_eq!(rows[0].age_text, "5 d");

    // A gem: its level is property 5, the level shown on the row is
    // property 78 ahead of the bare ilvl, and the stack comes from the item.
    let gem = entry(
        "seller9",
        Some((2.0, "exalted")),
        json!({
            "name": "", "typeLine": "Spark", "rarity": "Gem", "ilvl": 1, "stackSize": 3, "corrupted": true,
            "properties": [
                { "name": "Level", "values": [["12 (Max)", 0]], "type": 5 },
                { "name": "Quality", "values": [["+7%", 1]], "type": 6 },
                { "name": "Item Level", "values": [["81", 0]], "type": 78 }
            ]
        }),
        Some("online"),
    );
    let mut gem = gem;
    gem["listing"]["indexed"] = json!("2026-09-30T00:00:00Z");
    let gem = parse_entry(&gem, NOW, ME).expect("gem parses");
    assert_eq!(gem.gem_level, Some(12));
    assert_eq!(gem.quality, Some(7));
    assert_eq!(gem.ilvl, Some(81));
    assert_eq!(gem.stack, Some(3));
    assert!(gem.corrupted);
    assert!(!gem.has_note);
    assert!(!gem.instant_buyout);
    assert_eq!(gem.state, SellerState::Online);
    assert_eq!(gem.age_text, "just now", "indexed after now still reads as just now");

    assert_eq!(age_text(NOW - 59, NOW), "just now");
    assert_eq!(age_text(NOW - 60, NOW), "1 min");
    assert_eq!(age_text(NOW - 3_599, NOW), "59 min");
    assert_eq!(age_text(NOW - 3_600, NOW), "1 h");
    assert_eq!(age_text(NOW - 86_399, NOW), "23 h");
    assert_eq!(age_text(NOW - 86_400, NOW), "1 d");
    assert_eq!(age_text(NOW - 30 * 86_400, NOW), "30 d");
}

#[test]
fn the_card_carries_every_mod_line_with_its_tier_badges() {
    let (rows, _) = parse_fetch_body(&fixture(), NOW, ME);
    let card = &rows[3].card;
    assert_eq!(card.name, "Sol Wrap");
    assert_eq!(card.base, "Warlord Cuirass");
    assert_eq!(card.rarity, "Rare");
    assert_eq!(card.figures, vec![("Armour".to_string(), "2972".to_string())]);

    let kinds: Vec<LineKind> = card.lines.iter().map(|l| l.kind).collect();
    assert_eq!(
        kinds,
        vec![
            LineKind::Rune,
            LineKind::Rune,
            LineKind::Rune,
            LineKind::Implicit,
            LineKind::Explicit,
            LineKind::Explicit,
            LineKind::Explicit,
            LineKind::Explicit,
            LineKind::Crafted,
            LineKind::Desecrated,
        ],
        "runes, then implicits, then every explicit in order with the domain the API gave it"
    );
    let named: Vec<&str> = kinds.iter().map(|k| k.as_str()).collect();
    assert_eq!(named[2..5], ["rune", "implicit", "explicit"]);
    assert_eq!(named[8..], ["crafted", "desecrated"]);
    let tiers: Vec<Vec<String>> = card.lines.iter().map(|l| l.tiers.clone()).collect();
    let t = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(
        tiers,
        vec![t(&[]), t(&[]), t(&[]), t(&[]), t(&["P1", "P2"]), t(&["P2"]), t(&["S3"]), t(&["S1"]), t(&["S3"]), t(&["P2"])],
        "a line merging two mods carries both badges; an implicit without a tier carries none"
    );
    assert_eq!(card.lines[4].text, "+373 to Armour");
    assert_eq!(card.lines[9].text, "95% increased Armour");

    let unnamed = entry(
        "seller9",
        Some((1.0, "exalted")),
        json!({ "name": "", "typeLine": "", "baseType": "Iron Ring", "rarity": "Normal" }),
        None,
    );
    assert_eq!(parse_entry(&unnamed, NOW, ME).unwrap().card.base, "Iron Ring", "an empty typeLine falls back to the base type");

    let normal = &rows[11].card;
    assert_eq!(normal.name, "");
    assert_eq!(normal.base, "Exceptional Soldier Cuirass");
    assert_eq!(normal.rarity, "Normal");
    assert!(normal.lines.is_empty());

    // Plain-string mod lines, the shape the API used before `mods[]`: no tier,
    // the kind of the block they sit in.
    let old = entry(
        "seller9",
        Some((1.0, "exalted")),
        json!({
            "name": "", "typeLine": "Iron Ring", "rarity": "Magic",
            "implicitMods": ["+10 to [Strength|Strength]"],
            "explicitMods": ["+20 to maximum Life"]
        }),
        None,
    );
    let old = parse_entry(&old, NOW, ME).expect("parses");
    assert_eq!(old.card.lines.len(), 2);
    assert_eq!(old.card.lines[0].kind, LineKind::Implicit);
    assert_eq!(old.card.lines[0].text, "+10 to Strength");
    assert!(old.card.lines[0].tiers.is_empty());
    assert_eq!(old.card.lines[1].kind, LineKind::Explicit);
}

#[test]
fn display_brackets_keep_only_the_display_half() {
    assert_eq!(display_text("[ShamanOnlyMods|Bonded]: +60 to maximum Life"), "Bonded: +60 to maximum Life");
    assert_eq!(
        display_text("54% increased [Armour|Armour], [Evasion|Evasion] and [EnergyShield|Energy Shield]"),
        "54% increased Armour, Evasion and Energy Shield"
    );
    assert_eq!(display_text("18% increased [Physical] Damage"), "18% increased Physical Damage");
    assert_eq!(display_text("no brackets here"), "no brackets here");
    assert_eq!(display_text("[Tag|]"), "Tag", "an empty display half falls back to the tag, as EE2 does");

    let (rows, _) = parse_fetch_body(&fixture(), NOW, ME);
    assert_eq!(rows[3].card.lines[1].text, "Bonded: +60 to maximum Life");
    assert!(rows.iter().flat_map(|r| r.card.lines.iter()).all(|l| !l.text.contains('[') && !l.text.contains(']')));
}

#[test]
fn the_same_seller_at_the_same_price_folds_into_one_row() {
    let rows = vec![row("a", 5.0, "divine"), row("b", 5.0, "divine"), row("c", 6.0, "divine"), row("a", 5.0, "divine")];
    assert_eq!(group_indices(&rows), vec![(0, 2), (1, 1), (2, 1)]);
    let groups = group(rows);
    assert_eq!(times(&groups), vec![2, 1, 1]);
    assert_eq!(groups[0].view.seller, "a");

    // The same seller at a different price, three rows back, stays its own row.
    let rows = vec![row("a", 5.0, "divine"), row("b", 5.0, "divine"), row("c", 6.0, "divine"), row("a", 7.0, "divine")];
    assert_eq!(group_indices(&rows), vec![(0, 1), (1, 1), (2, 1), (3, 1)]);

    // Stacks fold by adding up, the way EE2 shows one row for a seller's
    // whole supply: the row keeps ×1 and the stack grows.
    let rows = vec![stacked("a", 1.0, "exalted", 10), stacked("a", 1.0, "exalted", 25)];
    let groups = group(rows);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].times, 1);
    assert_eq!(groups[0].view.stack, Some(35));
}

#[test]
fn the_same_seller_within_two_rows_folds_too() {
    let rows = vec![row("a", 5.0, "divine"), row("b", 6.0, "divine"), row("a", 7.0, "divine")];
    assert_eq!(group_indices(&rows), vec![(0, 2), (1, 1)], "two rows back folds");

    let rows = vec![row("a", 5.0, "divine"), row("a", 6.0, "divine")];
    assert_eq!(group_indices(&rows), vec![(0, 2)], "the row just before folds");

    let rows = vec![row("a", 5.0, "divine"), row("b", 6.0, "divine"), row("c", 7.0, "divine"), row("a", 8.0, "divine")];
    assert_eq!(group_indices(&rows), vec![(0, 1), (1, 1), (2, 1), (3, 1)], "three rows back does not");

    // The distance is counted in grouped rows, not raw ones.
    let rows = vec![
        row("a", 5.0, "divine"),
        row("b", 6.0, "divine"),
        row("b", 6.0, "divine"),
        row("b", 6.0, "divine"),
        row("a", 9.0, "divine"),
    ];
    assert_eq!(group_indices(&rows), vec![(0, 2), (1, 3)]);
}

#[test]
fn different_sellers_never_fold() {
    let rows = vec![row("a", 5.0, "divine"), row("b", 5.0, "divine"), row("c", 5.0, "divine")];
    assert_eq!(group_indices(&rows), vec![(0, 1), (1, 1), (2, 1)]);
    let groups = group(rows);
    assert_eq!(times(&groups), vec![1, 1, 1]);

    let (rows, _) = parse_fetch_body(&fixture(), NOW, ME);
    let groups = group(rows);
    assert_eq!(groups.len(), 17, "seven sellers, never adjacent, never at one price twice");
    assert!(groups.iter().all(|g| g.times == 1));
}

#[test]
fn fifteen_rows_or_fewer_are_never_price_fixed() {
    let rows: Vec<ListingView> = (0..15).map(|i| row(&format!("s{i}"), 1.0, "alch")).collect();
    let groups = group(rows);
    assert_eq!(groups.len(), 15);
    assert!(price_fixed(&groups).is_none(), "15 rows of bait are not enough to say");

    let (rows, _) = parse_fetch_body(&fixture(), NOW, ME);
    assert!(price_fixed(&group(rows)).is_none());

    // 16 rows in a real currency are fine too.
    let rows: Vec<ListingView> = (0..16).map(|i| row(&format!("s{i}"), 1.0 + i as f64, "exalted")).collect();
    assert!(price_fixed(&group(rows)).is_none());
}

#[test]
fn cheap_currency_at_the_bottom_flags_price_fixing_with_its_numbers() {
    let mut rows: Vec<ListingView> = (0..13)
        .map(|i| row(&format!("s{i}"), 1.0, if i % 2 == 0 { "alch" } else { "chance" }))
        .collect();
    rows.push(row("s13", 2.0, "exalted"));
    rows.push(row("s14", 3.0, "exalted"));
    rows.push(row("s15", 1.0, "divine"));
    let groups = group(rows);
    assert_eq!(groups.len(), 16);
    let fixed = price_fixed(&groups).expect("13 bait rows, three real ones");
    assert_eq!(fixed.count, 13);
    assert_eq!(fixed.under_currency.as_deref(), Some("alch"));
    assert_eq!(fixed.next_price, Some((2.0, "exalted".to_string())));

    // Five real prices are enough to trust the table.
    let mut rows: Vec<ListingView> = (0..11).map(|i| row(&format!("s{i}"), 1.0, "alch")).collect();
    rows.extend((11..16).map(|i| row(&format!("s{i}"), 2.0, "chaos")));
    assert!(price_fixed(&group(rows)).is_none());

    // Small amounts of aug/regal/transmute count as real prices; 30 or more
    // do not, and the "greater" orbs never do.
    let mut rows: Vec<ListingView> = (0..11).map(|i| row(&format!("s{i}"), 1.0, "alch")).collect();
    rows.extend((11..16).map(|i| row(&format!("s{i}"), 29.0, "aug")));
    assert!(price_fixed(&group(rows)).is_none());
    let mut rows: Vec<ListingView> = (0..11).map(|i| row(&format!("s{i}"), 1.0, "alch")).collect();
    rows.extend((11..16).map(|i| row(&format!("s{i}"), 30.0, "aug")));
    let fixed = price_fixed(&group(rows)).expect("30 aug is bait");
    assert_eq!(fixed.count, 16);
    assert_eq!(fixed.next_price, None);
    let mut rows: Vec<ListingView> = (0..11).map(|i| row(&format!("s{i}"), 1.0, "alch")).collect();
    rows.extend((11..16).map(|i| row(&format!("s{i}"), 1.0, "greater-orb-of-augmentation")));
    assert_eq!(price_fixed(&group(rows)).map(|f| f.count), Some(16));

    // A priceless row is not a real price either.
    let mut rows: Vec<ListingView> = (0..11).map(|i| row(&format!("s{i}"), 1.0, "alch")).collect();
    rows.extend((11..16).map(|i| row(&format!("s{i}"), 5.0, "greater-chaos-orb")));
    assert!(price_fixed(&group(rows)).is_none(), "greater chaos still says chaos");
    let mut rows: Vec<ListingView> = (0..12).map(|i| row(&format!("s{i}"), 1.0, "alch")).collect();
    rows.extend((12..15).map(|i| row(&format!("s{i}"), 5.0, "chaos")));
    let item = json!({ "name": "", "typeLine": "Chaos Orb", "rarity": "Currency" });
    rows.push(parse_entry(&entry("s15", None, item, None), NOW, ME).unwrap());
    let fixed = price_fixed(&group(rows)).expect("four real prices and a blank");
    assert_eq!(fixed.count, 13);
}
