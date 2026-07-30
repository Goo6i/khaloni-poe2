use std::time::{Duration, Instant};

use khaloni_poe2::hover::HoverState;
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

    // The worker reporting back replaces the popup with listings.
    hs.appraisal_done(
        "Horror Bane",
        Ok(vec![khaloni_poe2_core::trade::Listing {
            price_amount: 2.5,
            price_currency: "exalted".into(),
            account: "Someone#1234".into(),
            indexed: String::new(),
            item_name: "Storm Call".into(),
        }]),
    );
    let popup = hs.current.as_ref().expect("appraisal popup");
    assert!(popup.lines[0].text.contains("2.5 exalted"));
    assert!(popup.lines[0].text.contains("Someone#1234"));

    // And an error outcome shows the message instead of vanishing.
    hs.appraisal_done("Horror Bane", Err("rate limited; retry in 60s".into()));
    assert!(hs.current.as_ref().unwrap().lines[0].text.contains("rate limited"));
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
fn unknown_unique_still_shows_the_question_mark() {
    let t = table();
    let mut hs = HoverState::default();
    let clipboard = include_str!("../../core/tests/fixtures/item5-unique-belt.txt");
    hs.trigger(clipboard, &t, &HashMap::new(), 1.0);
    let popup = hs.current.as_ref().expect("popup set");
    assert_eq!(popup.lines[0].text, khaloni_poe2_core::value::UNKNOWN);
}
