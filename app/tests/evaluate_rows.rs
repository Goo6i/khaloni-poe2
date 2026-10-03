//! The Evaluate card's property rows: which of them may drive a trade
//! filter, and that no two rows ever write the same bound.

use khaloni_poe2::evaluate_ui::{property_rows, Target};
use khaloni_poe2_core::props::{EquipKey, PropFilter};

/// The figures `ee2::build` reports for a bow with a little lightning
/// damage: each with the item's own value, its search floor, and whether
/// the search leads with it.
fn bow_props() -> Vec<PropFilter> {
    let prop = |key, value, min, enabled| PropFilter { key, value, min, enabled, hidden: false };
    vec![
        prop(EquipKey::Dps, 467.5, 420.0, true),
        prop(EquipKey::Edps, 46.75, 42.0, false),
        prop(EquipKey::Pdps, 420.75, 378.0, true),
        prop(EquipKey::Aps, 1.1, 1.1, false),
        prop(EquipKey::Crit, 8.48, 7.63, false),
    ]
}

#[test]
fn property_rows_never_share_a_search_bound() {
    let props = bow_props();
    let rows = property_rows(&props, 99.2);
    let chaos = rows.iter().find(|r| r.label.starts_with("Chaos DPS")).expect("chaos row shown");
    assert_eq!(chaos.label, "Chaos DPS: 99.2", "a display-only line carries its value");
    assert!(chaos.target.is_none(), "the trade site has no chaos-DPS filter; the row is display-only");
    let mut bounds: Vec<_> = rows
        .iter()
        .filter_map(|r| match r.target {
            Some(Target::Equipment(b)) => Some(b),
            _ => None,
        })
        .collect();
    let n = bounds.len();
    assert!(n >= 4, "DPS figures, crit and attack rate are all offered, got {n}");
    bounds.sort_by_key(|b| format!("{b:?}"));
    bounds.dedup();
    assert_eq!(bounds.len(), n, "two rows driving one bound would fight over it");
    // No chaos damage, no chaos line.
    assert!(property_rows(&props, 0.0).iter().all(|r| !r.label.starts_with("Chaos DPS")));
}

#[test]
fn a_row_shows_the_figure_and_searches_the_floor() {
    let props = bow_props();
    let rows = property_rows(&props, 0.0);
    let dps = rows.iter().find(|r| r.label.starts_with("Total DPS")).expect("total dps row");
    assert!(dps.enabled, "a weapon with elemental damage is searched on its total DPS");
    let own = props.iter().find(|p| p.key == EquipKey::Dps).unwrap().value;
    assert!(dps.min.expect("a floor") < own, "the box holds the search floor, not the item's own figure");
}

// --- the listings table, the hover card and the blocks under the card ----

use khaloni_poe2::evaluate_ui::{self as ev, Action, ListingRow, SellerState};
use khaloni_poe2_core::listing::{self, GroupedListing};
use khaloni_poe2_core::ninja::PriceTable;

const NOW: i64 = 1_790_000_000;
const ME: &str = "seller3";

fn measure(s: &str) -> i32 {
    7 * s.len() as i32
}

fn fixture_rows() -> Vec<GroupedListing> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../core/tests/fixtures/trade_fetch_full.json");
    let body: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("json");
    listing::group(listing::parse_fetch_body(&body, NOW, ME).0)
}

/// Rates close to the league's (2026-09-19): 459 ex and 8.4 chaos to the divine.
fn table() -> PriceTable {
    let mut t = PriceTable::default();
    t.exalted_per_divine = 459.0;
    t.chaos_per_divine = 8.4;
    t
}

/// A fetch row as the panel shows it. Only the price is converted; every
/// other field is the listing's own.
fn row_of(g: &GroupedListing, table: &PriceTable) -> ListingRow {
    let exalted = g.view.price.as_ref().and_then(|(amount, currency)| match currency.as_str() {
        "exalted" => Some(*amount),
        "divine" => Some(amount * table.exalted_per_divine),
        "chaos" => Some(amount * table.exalted_per_divine / table.chaos_per_divine),
        _ => None,
    });
    ListingRow::priced(g, exalted, table, 1.0)
}

fn bare_panel() -> ev::Panel {
    ev::Panel {
        header: ev::ItemHeader {
            name: "Sol Wrap".into(),
            rarity: "Rare".into(),
            item_level: Some(81),
            requires_level: None,
            base: None,
        },
        rows: vec![ev::StatRow {
            label: "+60 to maximum Life".into(),
            badge: Some(ev::TierBadge { kind: ev::AffixKind::Prefix, tier: 3 }),
            score: Some(3.0),
            min: Some(60.0),
            max: None,
            enabled: true,
            target: Some(ev::Target::Stat(0)),
            hidden: false,
            group: ev::RowGroup::Explicit,
            note: None,
        }],
        status: "20 of 1,934 shown".into(),
        ..ev::Panel::default()
    }
}

#[test]
fn listing_rows_carry_price_age_seller_and_times() {
    let table = table();
    let groups = fixture_rows();
    let rows: Vec<ListingRow> = groups.iter().map(|g| row_of(g, &table)).collect();
    assert_eq!(rows.len(), 17);

    // Sol Wrap: 5 divine from seller3 (the configured account), afk, ilvl
    // 81, quality 20, instant buyout, listed two days ago.
    let sol = &rows[3];
    assert_eq!(sol.price, "5 div");
    assert_eq!(sol.raw, "", "a listing already in the display currency repeats nothing");
    assert_eq!(sol.age, "2 d");
    assert_eq!((sol.seller.as_str(), sol.state, sol.mine), ("seller3", SellerState::Afk, true));
    let cells = sol.cells();
    assert_eq!(cells[0], "5 div");
    assert_eq!(cells[1], "2 d");
    assert_eq!(cells[2], "seller3 (you)");
    assert_eq!(cells[3], "afk");
    assert!(cells[4].contains("ilvl 81") && cells[4].contains("q20") && cells[4].contains("instant"), "{}", cells[4]);
    assert!(!cells[4].contains("stack") && !cells[4].contains("corrupted") && !cells[4].contains("gem"), "{}", cells[4]);

    // The first row's seller is offline and the listing is five days old.
    assert_eq!((rows[0].state, rows[0].age.as_str()), (SellerState::Offline, "5 d"));
    assert_eq!(rows[0].cells()[3], "offline");

    // A listing without a price says so instead of showing a number.
    let priceless = rows.iter().find(|r| r.seller == "seller6").expect("seller6");
    assert_eq!(priceless.price, "");
    assert_eq!(priceless.cells()[0], "no price");

    // Folded rows say how many they stand for, and a stack, a gem level
    // and a price in a currency the panel does not show in are all named.
    let mut many = rows[3].clone();
    many.times = 3;
    many.stack = Some(37);
    many.gem_level = Some(20);
    many.corrupted = true;
    many.raw = "2 aug".into();
    let cells = many.cells();
    assert_eq!(cells[0], "5 div (2 aug) x3", "the font has no multiplication sign, so x it is");
    assert!(cells[4].contains("stack 37") && cells[4].contains("gem 20") && cells[4].contains("corrupted"), "{}", cells[4]);

    // Prices convert into the panel's currency and keep the listing's own
    // when that differs.
    let (price, raw) = ev::listing_price_text(1500.0, "exalted", Some(1500.0), &table, 1.0);
    assert_eq!((price.as_str(), raw.as_str()), ("3.3 div", "1500 exalted"));
    let (price, raw) = ev::listing_price_text(12.0, "exalted", Some(12.0), &table, 1.0);
    assert_eq!((price.as_str(), raw.as_str()), ("12 ex", ""));
    let (price, raw) = ev::listing_price_text(2.0, "aug", None, &table, 1.0);
    assert_eq!((price.as_str(), raw.as_str()), ("2 aug", ""), "no rate: the listing's own price stands alone");

    // A row straight from the folded listing keeps its own currency.
    let own = ListingRow::from(&groups[3]);
    assert_eq!((own.price.as_str(), own.raw.as_str(), own.times), ("5 divine", "", 1));
    assert_eq!(own.card, groups[3].view.card);

    // Every row gets a hit rect in the table, in order, inside the panel.
    let panel = ev::Panel { listings: rows.clone(), ..bare_panel() };
    let lay = ev::layout(&panel, &measure);
    let t = lay.table.as_ref().expect("a table for the rows");
    assert_eq!(t.rows.len(), rows.len());
    for pair in t.rows.windows(2) {
        assert!(pair[1].y >= pair[0].y + pair[0].h as i32, "rows stack downward without overlap");
    }
    assert!(t.rows.last().unwrap().y + ev::LISTING_H <= lay.size.1);
    assert!(t.rows.iter().all(|r| r.x >= 0 && r.x + r.w as i32 <= lay.size.0));
    // Columns run left to right and the widest cell of each fits its column.
    assert!(t.cols.windows(2).all(|c| c[1] > c[0]), "{:?}", t.cols);
    for (ci, x) in t.cols.iter().enumerate().take(4) {
        let widest = rows.iter().map(|r| measure(&r.cells()[ci])).max().unwrap();
        assert!(x + widest <= t.cols[ci + 1], "column {ci} runs into the next");
    }
    // Without listings there is no table and no room taken for one.
    let bare = ev::layout(&bare_panel(), &measure);
    assert!(bare.table.is_none());
    assert!(bare.size.1 < lay.size.1);
}

#[test]
fn a_hovered_row_opens_its_card_beside_the_panel() {
    let table = table();
    let rows: Vec<ListingRow> = fixture_rows().iter().map(|g| row_of(g, &table)).collect();
    let mut panel = ev::Panel { listings: rows.clone(), ..bare_panel() };

    // Nothing hovered: no card.
    let lay = ev::layout(&panel, &measure);
    assert!(lay.card.is_none());

    // Pointer over row 3 (Sol Wrap): the hover hit names it, and once the
    // model carries it the card is laid out to the right of the panel, level
    // with the row, with every card line and its badges.
    let t = lay.table.clone().unwrap();
    let r3 = &t.rows[3];
    assert_eq!(ev::hover_hit(&lay, r3.x + 4, r3.y + 4), Some(3));
    assert_eq!(ev::hover_hit(&lay, lay.rarity_pos.0, lay.rarity_pos.1), None);
    assert_eq!(ev::hover_hit(&lay, -50, -50), None);
    assert_eq!(ev::hit(&panel, &lay, r3.x + 4, r3.y + 4), Some(Action::HoverRow(3)));
    assert_eq!(ev::hit(&panel, &lay, lay.rarity_pos.0, lay.rarity_pos.1), None);

    panel.hover = Some(3);
    let lay = ev::layout(&panel, &measure);
    let card = lay.card.clone().expect("a hovered row has a card");
    assert!(card.rect.x >= lay.size.0, "the card sits right of the panel: {:?} vs {:?}", card.rect, lay.size);
    // Level with its row: the card's span covers the row, as far down as
    // the panel's own height lets it start.
    let row = &t.rows[3];
    assert!(card.rect.y <= row.y && card.rect.y + card.rect.h as i32 >= row.y + row.h as i32, "{:?} vs {row:?}", card.rect);
    assert!(card.rect.y == row.y || card.rect.y + card.rect.h as i32 == lay.size.1 || card.rect.y == 0);
    assert_eq!(card.lines.len(), rows[3].card.lines.len(), "one baseline per mod line");
    assert_eq!(card.figures.len(), rows[3].card.figures.len());
    assert!(card.lines.windows(2).all(|p| p[1] > p[0]));
    assert!(card.lines.last().unwrap() < &(card.rect.y + card.rect.h as i32));
    // Wide enough for the longest line after the badge gutter.
    let widest = rows[3].card.lines.iter().map(|l| measure(&l.text)).max().unwrap();
    assert!(card.text_x + widest <= card.rect.x + card.rect.w as i32);
    assert!(card.badge_x < card.text_x);
    // The drawn strings include the title, every line and every badge.
    let text = ev::all_text(&panel, &measure);
    assert!(text.iter().any(|s| s == "Sol Wrap"));
    for line in &rows[3].card.lines {
        assert!(text.contains(&line.text), "card line missing: {}", line.text);
        for tier in &line.tiers {
            assert!(text.contains(tier), "badge missing: {tier}");
        }
    }

    // No room on the right: the card goes to the left of the panel.
    panel.screen_right = Some(lay.size.0 + 20);
    let lay = ev::layout(&panel, &measure);
    let card = lay.card.unwrap();
    assert!(card.rect.x + card.rect.w as i32 <= 0, "{:?}", card.rect);

    // A hover index past the table is ignored, not a panic.
    panel.hover = Some(99);
    assert!(ev::layout(&panel, &measure).card.is_none());
}

#[test]
fn the_price_fixed_button_and_the_attribute_button_hit_test() {
    let panel = ev::Panel {
        price_fixed: Some(ev::PriceFixedStrip {
            text: "likely price-fixed: 14 listings under 1 aug, next at 5 ex".into(),
            button: "ex/div only".into(),
        }),
        attribute_enabled: true,
        ..bare_panel()
    };
    let lay = ev::layout(&panel, &measure);
    let strip = lay.price_fixed.clone().expect("the strip is laid out");
    assert_eq!(ev::hit(&panel, &lay, strip.button.x + 2, strip.button.y + 2), Some(Action::PriceFixedFilter));
    assert_eq!(ev::hit(&panel, &lay, strip.text_pos.0 + 2, strip.button.y + 2), None, "the text is not a button");
    assert!(strip.button.x + strip.button.w as i32 <= lay.size.0);
    let attr = lay.attribution.clone().expect("a rare with a ticked mod offers the attribution button");
    assert_eq!(ev::hit(&panel, &lay, attr.button.x + 2, attr.button.y + 2), Some(Action::Attribute));
    assert!(attr.button.x + attr.button.w as i32 <= lay.size.0);
    // A disabled button is still a button: the caller decides not to send.
    let off = ev::Panel { attribute_enabled: false, ..panel.clone() };
    let lay = ev::layout(&off, &measure);
    let attr = lay.attribution.clone().unwrap();
    assert_eq!(ev::hit(&off, &lay, attr.button.x + 2, attr.button.y + 2), Some(Action::Attribute));
    // No strip, no button.
    let plain = bare_panel();
    let lay = ev::layout(&plain, &measure);
    assert!(lay.price_fixed.is_none());
    // An item with no ticked mod (a currency) offers no attribution.
    let mut currency = panel.clone();
    currency.rows.clear();
    assert!(ev::layout(&currency, &measure).attribution.is_none());
    // Attribution rows are laid out under the heading, one baseline pair each.
    let rows = ev::Panel {
        attribution: vec![ev::AttributionRow {
            label: "T1 life".into(),
            with: "40 ex".into(),
            without: "9 ex".into(),
            text: "with: seller1 40 ex; without: seller4 9 ex".into(),
        }],
        ..panel.clone()
    };
    let lay = ev::layout(&rows, &measure);
    assert_eq!(lay.attribution.unwrap().rows.len(), 1);
}

#[test]
fn nothing_on_the_panel_speaks_of_an_estimate() {
    // The value box and its reliability label are gone: whatever the panel
    // carries, no drawn string says either word.
    let rows: Vec<ListingRow> = fixture_rows().iter().map(|g| row_of(g, &table())).collect();
    let panel = ev::Panel {
        listings: rows,
        ladder: "cheapest 2 ex, then 5, 6, 6, 8 ex · 17 of 1,934 matched".into(),
        budget_text: "searches 4/30 (5 min)".into(),
        hover: Some(1),
        ..bare_panel()
    };
    for s in ev::all_text(&panel, &measure) {
        let l = s.to_lowercase();
        assert!(!l.contains("estimat") && !l.contains("reliab"), "still drawn: {s}");
    }
}
