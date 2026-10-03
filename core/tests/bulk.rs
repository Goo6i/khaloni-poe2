//! The exchange response becomes bulk rows: what the seller gives, what
//! they want for it, stock, seller and state, cheapest per unit first. The
//! going rate the popup quotes stays the median of the five cheapest that
//! `trade::exchange_rate_of` draws, so both readings of one body agree.

use khaloni_poe2_core::bulk::{parse_exchange, BulkView};
use khaloni_poe2_core::listing::SellerState;
use khaloni_poe2_core::trade::exchange_rate_of;
use serde_json::{json, Value};

fn fixture() -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/trade_exchange_full.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("fixture json")
}

/// A one-listing body whose single offer has the given sides, each side a
/// JSON value so a test can hand it a number, `null` or leave it out.
fn body(item_amount: Value, exchange_amount: Value, stock: Value) -> Value {
    let mut item = json!({ "currency": "omen-of-whittling" });
    if !item_amount.is_null() {
        item["amount"] = item_amount;
    }
    if !stock.is_null() {
        item["stock"] = stock;
    }
    let mut exchange = json!({ "currency": "exalted" });
    if !exchange_amount.is_null() {
        exchange["amount"] = exchange_amount;
    }
    json!({
        "result": {
            "l1": {
                "listing": {
                    "account": { "name": "sellerZ#1", "online": { "league": "x" } },
                    "offers": [{ "exchange": exchange, "item": item }]
                }
            }
        },
        "total": 1
    })
}

fn per_units(view: &BulkView) -> Vec<f64> {
    view.offers.iter().map(|o| o.per_unit).collect()
}

#[test]
fn offers_carry_stock_seller_and_per_unit_price_sorted_ascending() {
    let view = parse_exchange(&fixture());

    assert_eq!(view.offers.len(), 12, "13 offers in the fixture, one of them for nothing");
    assert_eq!(view.skipped, 1);
    let rates = per_units(&view);
    assert!(rates.windows(2).all(|w| w[0] <= w[1]), "not ascending: {rates:?}");
    assert_eq!(rates[0], 2.0);
    assert_eq!(rates[11], 8.0);

    let cheapest = &view.offers[0];
    assert_eq!(cheapest.seller, "sellerF#1006");
    assert_eq!(cheapest.state, SellerState::Afk);
    assert_eq!(cheapest.have, ("omen-of-whittling".to_string(), 1.0));
    assert_eq!(cheapest.want, ("exalted".to_string(), 2.0));
    assert_eq!(cheapest.stock, Some(250));

    // A bulk offer keeps both amounts and prices per unit of what is sold.
    let bulk = view.offers.iter().find(|o| o.seller == "sellerA#1001" && o.have.1 == 2.0).expect("sellerA's 2-for-7");
    assert_eq!(bulk.want.1, 7.0);
    assert_eq!(bulk.per_unit, 3.5);
    assert_eq!(bulk.state, SellerState::Online);
    assert_eq!(bulk.stock, Some(12));

    let absent = view.offers.iter().find(|o| o.seller == "sellerC#1003").expect("sellerC");
    assert_eq!(absent.state, SellerState::Offline);
    let unstocked = view.offers.iter().find(|o| o.seller == "sellerE#1005").expect("sellerE");
    assert_eq!(unstocked.stock, None);
    assert!((unstocked.per_unit - 10.0 / 3.0).abs() < 1e-12);

    let rows = view.text_rows("omen", "exalted");
    assert_eq!(rows.len(), 12);
    assert_eq!(rows[0], "250 in stock · 2 exalted each · sellerF#1006 (afk)");
    for expected in [
        "12 in stock · 3.5 exalted each · 7 exalted for 2 omen · sellerA#1001 (online)",
        "stock unknown · 3.3333 exalted each · 10 exalted for 3 omen · sellerE#1005 (online)",
        "1 in stock · 5 exalted each · sellerC#1003 (offline)",
    ] {
        assert!(rows.iter().any(|r| r == expected), "missing {expected:?} in {rows:#?}");
    }
}

#[test]
fn a_zero_or_missing_amount_is_skipped_not_a_division() {
    let cases = [
        ("zero item amount", body(json!(0), json!(5), json!(3))),
        ("zero exchange amount", body(json!(1), json!(0), json!(3))),
        ("missing item amount", body(Value::Null, json!(5), json!(3))),
        ("missing exchange amount", body(json!(1), Value::Null, json!(3))),
        ("item amount not a number", body(json!("1"), json!(5), json!(3))),
        ("negative exchange amount", body(json!(1), json!(-5), json!(3))),
    ];
    for (what, body) in &cases {
        let view = parse_exchange(body);
        assert!(view.offers.is_empty(), "{what}: {:?}", per_units(&view));
        assert_eq!(view.skipped, 1, "{what}");
        assert_eq!(view.median_rate(), None, "{what}");
        assert!(view.text_rows("omen", "exalted").is_empty(), "{what}");
        assert_eq!(exchange_rate_of(body), None, "{what}: trade agrees there is no rate");
    }

    // A missing stock is not an amount: the offer stays, priced, unstocked.
    let view = parse_exchange(&body(json!(2), json!(9), Value::Null));
    assert_eq!(view.skipped, 0);
    assert_eq!(view.offers.len(), 1);
    assert_eq!(view.offers[0].stock, None);
    assert_eq!(view.offers[0].per_unit, 4.5);

    // An empty body and a body with no listings are both simply empty.
    for body in [json!({}), json!({ "result": {} }), json!({ "result": { "l1": { "listing": {} } } })] {
        let view = parse_exchange(&body);
        assert!(view.offers.is_empty());
        assert_eq!(view.skipped, 0);
        assert_eq!(view.median_rate(), None);
    }
}

#[test]
fn the_median_rate_still_matches_parse_exchange_rate() {
    let full = fixture();
    let view = parse_exchange(&full);
    assert_eq!(view.median_rate(), exchange_rate_of(&full));
    assert_eq!(view.median_rate(), Some(3.1), "middle of 2, 3, 3.1, 3.33, 3.5");

    let small = serde_json::from_str::<Value>(include_str!("fixtures/trade_exchange.json")).expect("fixture json");
    let view = parse_exchange(&small);
    assert_eq!(view.offers.len(), 6);
    assert_eq!(view.median_rate(), exchange_rate_of(&small));

    // One bait offer far under the market moves neither reading.
    let mut bait = fixture();
    bait["result"]["bait"] = extra_listing("sellerX#9", 100, 1);
    let view = parse_exchange(&bait);
    assert_eq!(view.offers[0].per_unit, 0.01);
    assert_eq!(view.median_rate(), exchange_rate_of(&bait));
    assert_eq!(view.median_rate(), Some(3.0));

    // Two offers: the even-count average both sides take.
    let mut two = body(json!(1), json!(4), json!(1));
    two["result"]["l2"] = extra_listing("sellerY#2", 1, 6);
    let view = parse_exchange(&two);
    assert_eq!(view.median_rate(), Some(5.0));
    assert_eq!(view.median_rate(), exchange_rate_of(&two));
}

/// A listing with one offer: `have` omens for `want` exalted, one in stock.
fn extra_listing(seller: &str, have: u32, want: u32) -> Value {
    json!({
        "listing": {
            "account": { "name": seller },
            "offers": [{
                "exchange": { "currency": "exalted", "amount": want },
                "item": { "currency": "omen-of-whittling", "amount": have, "stock": 1 }
            }]
        }
    })
}
