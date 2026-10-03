use std::time::{Duration, Instant};

use khaloni_poe2::hover::{is_corrupted, Freshness, HoverState};
use khaloni_poe2::pricing::Denom;
use std::collections::HashMap;

use khaloni_poe2_core::ninja::{ExchangeOverview, PriceTable};

fn table() -> PriceTable {
    let ov: ExchangeOverview =
        serde_json::from_str(include_str!("../../core/tests/fixtures/ninja_currency.json"))
            .unwrap();
    PriceTable::build(&[ov])
}

#[test]
fn currency_fixture_prices_with_stack_count() {
    let t = table();
    let mut hs = HoverState::default();
    let clipboard = include_str!("../../core/tests/fixtures/item4-currency-exalted.txt");
    hs.trigger(clipboard, &t, &HashMap::new(), 1.0);
    let popup = hs.current.as_ref().expect("popup set");
    assert_eq!(popup.title, "Exalted Orb");
    assert_eq!(popup.lines.len(), 1);
    assert_ne!(popup.lines[0].text, khaloni_poe2_core::value::UNKNOWN);
    assert_ne!(popup.lines[0].denom, Denom::None);
}

#[test]
fn rare_item_queues_an_appraisal() {
    let t = table();
    let mut hs = HoverState::default();
    let clipboard = include_str!("../../core/tests/fixtures/item1-inventory-rare-bow.txt");
    hs.trigger(clipboard, &t, &HashMap::new(), 1.0);
    let popup = hs.current.as_ref().expect("popup set");
    assert_eq!(popup.title, "Horror Bane");
    assert_eq!(popup.lines[0].text, "searching trade...");
    let queued = hs.pending_appraisal.take().expect("rare queues an appraisal request");
    assert_eq!(queued.name, "Horror Bane");
}

#[test]
fn garbage_clipboard_clears_popup_and_sets_error() {
    let t = table();
    let mut hs = HoverState::default();
    hs.trigger("not an item at all", &t, &HashMap::new(), 1.0);
    assert!(hs.current.is_none());
    assert!(hs.last_error.is_some());
}

#[test]
fn trigger_always_resets_the_ttl() {
    let t = table();
    let mut hs = HoverState::default();
    let clipboard = include_str!("../../core/tests/fixtures/item4-currency-exalted.txt");
    hs.trigger(clipboard, &t, &HashMap::new(), 1.0);
    let first_expiry = hs.current.as_ref().unwrap().expires;
    std::thread::sleep(Duration::from_millis(5));
    hs.trigger(clipboard, &t, &HashMap::new(), 1.0);
    let second_expiry = hs.current.as_ref().unwrap().expires;
    assert!(second_expiry > first_expiry);
}

#[test]
fn tick_expires_the_popup_after_ttl() {
    let t = table();
    let mut hs = HoverState::default();
    let clipboard = include_str!("../../core/tests/fixtures/item4-currency-exalted.txt");
    hs.trigger(clipboard, &t, &HashMap::new(), 1.0);
    // Force expiry without sleeping 6s in a test.
    hs.current.as_mut().unwrap().expires = Instant::now() - Duration::from_millis(1);
    hs.tick();
    assert!(hs.current.is_none());
}

#[test]
fn unique_item_prices_from_the_uniques_map() {
    let t = table();
    let mut hs = HoverState::default();
    let uniques = HashMap::from([("The Gnashing Sash".to_string(), 415.0)]);
    let clipboard = include_str!("../../core/tests/fixtures/item5-unique-belt.txt");
    hs.trigger(clipboard, &t, &uniques, 1.0);
    let popup = hs.current.as_ref().expect("popup set");
    assert_eq!(popup.title, "The Gnashing Sash");
    assert_ne!(popup.lines[0].text, khaloni_poe2_core::value::UNKNOWN);
    assert_ne!(popup.lines[0].denom, Denom::None);
    assert!(hs.pending_appraisal.is_none(), "uniques answer locally, no trade search");
}

#[test]
fn unknown_unique_routes_to_trade_appraisal() {
    // poe2scout stopped publishing unique prices for the current league
    // (live 2026-09-08: every category empty), so a name the map lacks must
    // go to the trade site like a rare, not dead-end at "?".
    let t = table();
    let mut hs = HoverState::default();
    let clipboard = include_str!("../../core/tests/fixtures/item5-unique-belt.txt");
    hs.trigger(clipboard, &t, &HashMap::new(), 1.0);
    let popup = hs.current.as_ref().expect("popup set");
    assert_eq!(popup.title, "The Gnashing Sash");
    assert_eq!(popup.lines[0].text, "searching trade...");
    let queued = hs.pending_appraisal.take().expect("unknown unique queues an appraisal");
    assert_eq!(queued.name, "The Gnashing Sash");
}

#[test]
fn magic_relics_route_to_trade_appraisal() {
    // Sekhemas relics are usually Magic; their value is in the mods, so
    // they must appraise instead of dead-ending at the "?" name lookup.
    let clip = "Item Class: Relics\nRarity: Magic\nUrn Relic of Vitality\n--------\nItem Level: 62\n--------\n21% increased Honour restored\n";
    let table = khaloni_poe2_core::ninja::PriceTable::default();
    let uniques = std::collections::HashMap::new();
    let mut h = khaloni_poe2::hover::HoverState::default();
    h.trigger(clip, &table, &uniques, 1.0);
    assert!(h.pending_appraisal.is_some(), "relic must queue an appraisal");
    let p = h.current.expect("popup shown");
    assert!(p.lines[0].text.contains("searching"), "got {:?}", p.lines[0].text);
}

#[test]
fn cut_gems_and_magic_gear_route_to_trade_appraisal() {
    let table = khaloni_poe2_core::ninja::PriceTable::default();
    let uniques = HashMap::new();
    // A cut skill gem: nothing in the currency table can price a specific
    // skill at a level; the trade site can.
    let gem = "Item Class: Skill Gems\nRarity: Gem\nFireball\n--------\nLevel: 20 (Max)\n--------\nRequirements:\nLevel: 70\n";
    let mut h = HoverState::default();
    h.trigger(gem, &table, &uniques, 1.0);
    assert!(h.pending_appraisal.is_some(), "gem must queue an appraisal");
    assert_eq!(h.current.as_ref().unwrap().lines[0].text, "searching trade...");
    // Magic gear with mods appraises like a rare: its value is in the mods.
    let ring = "Item Class: Rings\nRarity: Magic\nKraken Grip Sapphire Ring\n--------\nItem Level: 74\n--------\n+35% to Cold Resistance\n";
    let mut h = HoverState::default();
    h.trigger(ring, &table, &uniques, 1.0);
    assert!(h.pending_appraisal.is_some(), "magic gear must queue an appraisal");
    // A plain normal item with no mods and no table price stays honest.
    let plain = "Item Class: Boots\nRarity: Normal\nIron Greaves\n--------\nItem Level: 3\n";
    let mut h = HoverState::default();
    h.trigger(plain, &table, &uniques, 1.0);
    assert!(h.pending_appraisal.is_none());
    assert_eq!(h.current.as_ref().unwrap().lines[0].text, khaloni_poe2_core::value::UNKNOWN);
}

fn unique_lines() -> khaloni_poe2_core::ninja::UniquePrices {
    khaloni_poe2_core::ninja::UniquePrices::from_names(HashMap::from([("The Gnashing Sash".to_string(), 415.0)]))
}

#[test]
fn a_unique_priced_from_poe_ninja_says_so() {
    // The figure is an average over every copy of the unique; the popup
    // must not read like an appraisal of this copy's rolls.
    let mut hs = HoverState::default();
    let clipboard = include_str!("../../core/tests/fixtures/item5-unique-belt.txt");
    hs.trigger_priced(clipboard, &table(), &unique_lines(), 1.0, Freshness::default());
    let popup = hs.current.as_ref().expect("popup set");
    assert_ne!(popup.lines[0].denom, Denom::None, "the price line comes first");
    assert!(
        popup.lines.iter().any(|l| l.text.contains("poe.ninja") && l.text.contains("rolls")),
        "provenance line missing: {:?}",
        popup.lines
    );
    assert!(!popup.lines.iter().any(|l| l.text.contains("old price data")));
}

#[test]
fn stale_price_data_is_said_in_the_popup() {
    let clipboard = include_str!("../../core/tests/fixtures/item4-currency-exalted.txt");
    let fresh_lines = |fresh: Freshness| {
        let mut hs = HoverState::default();
        hs.trigger_priced(clipboard, &table(), &unique_lines(), 1.0, fresh);
        hs.current.unwrap().lines
    };
    let stale = fresh_lines(Freshness { table_stale: true, uniques_stale: false });
    assert!(stale.iter().any(|l| l.text.contains("old price data")), "{stale:?}");
    let current = fresh_lines(Freshness::default());
    assert!(!current.iter().any(|l| l.text.contains("old price data")), "{current:?}");

    // A unique answers from the unique lines, so their staleness counts.
    let belt = include_str!("../../core/tests/fixtures/item5-unique-belt.txt");
    let mut hs = HoverState::default();
    hs.trigger_priced(belt, &table(), &unique_lines(), 1.0, Freshness { table_stale: false, uniques_stale: true });
    assert!(hs.current.unwrap().lines.iter().any(|l| l.text.contains("old price data")));
}

#[test]
fn corrupted_is_read_in_both_clipboard_formats() {
    // Plain Ctrl+C: the state is the last section.
    let plain = "Item Class: Belts\nRarity: Unique\nThe Gnashing Sash\nRawhide Belt\n--------\nItem Level: 80\n--------\n+30 to maximum Life\n--------\nCorrupted\n";
    assert!(is_corrupted(plain));
    // Advanced Ctrl+Alt+C: mod headers in braces, the same closing line.
    let advanced = "Item Class: Belts\nRarity: Unique\nThe Gnashing Sash\nRawhide Belt\n--------\nItem Level: 80\n--------\n{ Unique Modifier \u{2014} Life }\n+30(25-35) to maximum Life\n--------\nCorrupted\n";
    assert!(is_corrupted(advanced));
    assert!(is_corrupted("Rarity: Unique\nX\n--------\nTwice Corrupted\n"));
    // The word inside a mod or flavour line is not the state.
    let clean = "Item Class: Belts\nRarity: Unique\nThe Gnashing Sash\nRawhide Belt\n--------\n+30 to maximum Life\n--------\n\"Corrupted blood runs thick.\"\n";
    assert!(!is_corrupted(clean));
}

#[test]
fn a_corrupted_unique_is_not_priced_as_a_clean_one() {
    // The name-only map speaks for uncorrupted copies; a corrupted one is a
    // different market and goes to the trade search.
    let text = format!("{}--------\nCorrupted\n", include_str!("../../core/tests/fixtures/item5-unique-belt.txt"));
    let mut hs = HoverState::default();
    hs.trigger_priced(&text, &table(), &unique_lines(), 1.0, Freshness::default());
    assert!(hs.pending_appraisal.is_some(), "a corrupted unique must be searched");
    assert_eq!(hs.current.unwrap().lines[0].text, "searching trade...");
}

#[test]
fn an_exchange_check_carries_the_hovered_stack() {
    let omen = "Item Class: Omens\nRarity: Currency\nOmen of Testing\n--------\nStack Size: 7/10\n";
    let mut hs = HoverState::default();
    hs.trigger_priced(omen, &table(), &unique_lines(), 1.0, Freshness::default());
    assert_eq!(hs.pending_currency, Some(("Omen of Testing".to_string(), 7)));

    // The answer is the stack's total with the per-unit price beside it.
    hs.show_exchange("Omen of Testing", &Ok(Some(2.0)), None, 7, &table(), 1e9, Freshness::default());
    let line = &hs.current.as_ref().unwrap().lines[0];
    assert!(line.text.contains("each"), "stack of 7 must show total (each): {:?}", line.text);
    hs.show_exchange("Omen of Testing", &Ok(Some(2.0)), None, 1, &table(), 1e9, Freshness::default());
    assert!(!hs.current.as_ref().unwrap().lines[0].text.contains("each"));
}

#[test]
fn an_exchange_answer_names_how_many_offers_stood_behind_it() {
    let mut hs = HoverState::default();
    hs.show_exchange("Omen of Testing", &Ok(Some(2.0)), Some(12), 1, &table(), 1e9, Freshness::default());
    let lines = &hs.current.as_ref().unwrap().lines;
    assert!(lines[1].text.contains("12 offers"), "{lines:?}");
    // Without the bulk view the source line says what the figure is and
    // claims no count.
    hs.show_exchange("Omen of Testing", &Ok(Some(2.0)), None, 1, &table(), 1e9, Freshness::default());
    let lines = &hs.current.as_ref().unwrap().lines;
    assert!(lines[1].text.contains("median") && !lines[1].text.contains("offers,"), "{lines:?}");
}

#[test]
fn an_exchange_failure_shows_its_reason_not_no_price() {
    let mut hs = HoverState::default();
    hs.show_exchange("Omen of Testing", &Err("trade cooldown 12s".into()), None, 1, &table(), 1.0, Freshness::default());
    let lines = &hs.current.as_ref().unwrap().lines;
    assert!(lines[0].text.contains("trade cooldown 12s"), "{lines:?}");
    hs.show_exchange("Omen of Testing", &Ok(None), None, 1, &table(), 1.0, Freshness::default());
    assert!(hs.current.as_ref().unwrap().lines[0].text.contains("no exchange offers"));
}

#[test]
fn a_notice_is_sticky_and_a_note_is_not() {
    let mut hs = HoverState::default();
    hs.show_notice("screen capture stopped");
    assert!(hs.sticky);
    assert!(hs.current.as_ref().unwrap().expires > Instant::now() + Duration::from_secs(4));
    hs.show_note("overlay on");
    assert!(!hs.sticky);
}
