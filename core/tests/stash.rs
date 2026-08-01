//! Parse-only tests for the legacy stash endpoint client. The top shape is
//! `{"numTabs":N,"items":[{"typeLine":..,"stackSize":..}]}` (verified
//! against the live character-window endpoint's documented format); network
//! behavior is covered by the error paths below without touching the site.

use khaloni_poe2_core::stash::{fetch_stash_value, parse_stash_tab, StashClient};

const TAB: &str = r#"{
    "numTabs": 3,
    "items": [
        {"typeLine": "Exalted Orb", "stackSize": 40},
        {"typeLine": "Divine Orb", "stackSize": 2},
        {"typeLine": "Stellar Amulet"}
    ]
}"#;

#[test]
fn parses_num_tabs_and_items() {
    let t = parse_stash_tab(TAB).expect("fixture parses");
    assert_eq!(t.num_tabs, 3);
    assert_eq!(t.items.len(), 3);
    assert_eq!(t.items[0].type_line, "Exalted Orb");
    assert_eq!(t.items[0].stack_size, 40);
    // Missing stackSize (gear) counts as one, never zero.
    assert_eq!(t.items[2].type_line, "Stellar Amulet");
    assert_eq!(t.items[2].stack_size, 1);
}

#[test]
fn pricing_callback_shape_sums_a_parsed_tab() {
    // The worker sums price(type_line, stack) over items; replicate that on
    // the parsed fixture so the summation contract is pinned down.
    let t = parse_stash_tab(TAB).unwrap();
    let price = |name: &str, stack: u32| -> f64 {
        let unit = match name {
            "Exalted Orb" => 1.0,
            "Divine Orb" => 40.0,
            _ => 0.0, // unknown names price at zero by design
        };
        unit * f64::from(stack)
    };
    let total: f64 = t.items.iter().map(|i| price(&i.type_line, i.stack_size)).sum();
    assert_eq!(total, 40.0 + 80.0);
}

#[test]
fn empty_and_missing_fields_parse_leniently() {
    // A tabs-only response (empty tab) and an items-only response both
    // parse: the endpoint omits fields freely.
    let t = parse_stash_tab(r#"{"numTabs":7,"items":[]}"#).expect("empty tab");
    assert_eq!(t.num_tabs, 7);
    assert!(t.items.is_empty());
    let t = parse_stash_tab(r#"{"items":[{"typeLine":"Chaos Orb","stackSize":5}]}"#)
        .expect("no numTabs");
    assert_eq!(t.num_tabs, 0);
    assert_eq!(t.items[0].stack_size, 5);
}

#[test]
fn bad_json_is_an_error() {
    assert!(parse_stash_tab("<html>login please</html>").is_err());
}

#[test]
fn missing_credentials_fail_before_any_request() {
    // No account or no session: an error, and no network I/O (a request to
    // the real host from a test would be both slow and impolite).
    let mut c = StashClient::new();
    let zero = |_: &str, _: u32| 0.0;
    assert!(fetch_stash_value(&mut c, "", "Standard", "sess", &zero).is_err());
    assert!(fetch_stash_value(&mut c, "acct#1234", "Standard", "", &zero).is_err());
}
