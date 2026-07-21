use std::time::{Duration, Instant};

use poe2_lens::hover::HoverState;
use poe2_lens::pricing::Denom;
use poe2_lens_core::ninja::{ExchangeOverview, PriceTable};

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
    hs.trigger(clipboard, &t, 1.0);
    let popup = hs.current.as_ref().expect("popup set");
    assert_eq!(popup.title, "Exalted Orb");
    assert_eq!(popup.lines.len(), 1);
    assert_ne!(popup.lines[0].text, poe2_lens_core::value::UNKNOWN);
    assert_ne!(popup.lines[0].denom, Denom::None);
}

#[test]
fn rare_item_gets_stage_b_placeholder() {
    let t = table();
    let mut hs = HoverState::default();
    let clipboard = include_str!("../../core/tests/fixtures/item1-inventory-rare-bow.txt");
    hs.trigger(clipboard, &t, 1.0);
    let popup = hs.current.as_ref().expect("popup set");
    assert_eq!(popup.title, "Horror Bane");
    assert_eq!(popup.lines.len(), 1);
    assert_eq!(popup.lines[0].text, "rare appraisal in Stage B");
    assert_eq!(popup.lines[0].denom, Denom::None);
}

#[test]
fn garbage_clipboard_clears_popup_and_sets_error() {
    let t = table();
    let mut hs = HoverState::default();
    hs.trigger("not an item at all", &t, 1.0);
    assert!(hs.current.is_none());
    assert!(hs.last_error.is_some());
}

#[test]
fn trigger_always_resets_the_ttl() {
    let t = table();
    let mut hs = HoverState::default();
    let clipboard = include_str!("../../core/tests/fixtures/item4-currency-exalted.txt");
    hs.trigger(clipboard, &t, 1.0);
    let first_expiry = hs.current.as_ref().unwrap().expires;
    std::thread::sleep(Duration::from_millis(5));
    hs.trigger(clipboard, &t, 1.0);
    let second_expiry = hs.current.as_ref().unwrap().expires;
    assert!(second_expiry > first_expiry);
}

#[test]
fn tick_expires_the_popup_after_ttl() {
    let t = table();
    let mut hs = HoverState::default();
    let clipboard = include_str!("../../core/tests/fixtures/item4-currency-exalted.txt");
    hs.trigger(clipboard, &t, 1.0);
    // Force expiry without sleeping 6s in a test.
    hs.current.as_mut().unwrap().expires = Instant::now() - Duration::from_millis(1);
    hs.tick();
    assert!(hs.current.is_none());
}
