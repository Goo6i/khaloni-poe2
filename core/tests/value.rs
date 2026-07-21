use poe2_lens_core::ninja::Price;
use poe2_lens_core::value::{display_price, format_amount, UNKNOWN};

fn price(divine: f64, exalted: f64) -> Price {
    Price {
        divine,
        exalted,
        chaos: 0.0,
    }
}

#[test]
fn amounts_use_one_decimal_and_trim() {
    assert_eq!(format_amount(2.5), "2.5");
    assert_eq!(format_amount(2.0), "2");
    assert_eq!(format_amount(0.04), "0.04");
    assert_eq!(format_amount(12.49), "12.5");
    assert_eq!(format_amount(150.2), "150");
}

#[test]
fn single_item_below_threshold_shows_exalted() {
    let p = price(0.006, 2.5);
    assert_eq!(display_price(&p, 1, 1.0), "2.5 ex");
}

#[test]
fn stack_shows_total_and_each() {
    let p = price(0.03, 2.5);
    assert_eq!(display_price(&p, 5, 1.0), "12.5 ex (2.5 each)");
}

#[test]
fn total_crossing_threshold_switches_to_divine() {
    let p = price(0.5, 205.0);
    assert_eq!(display_price(&p, 4, 1.0), "2 div (0.5 div each)");
}

#[test]
fn single_item_above_threshold_shows_divine() {
    let p = price(3.2, 1312.0);
    assert_eq!(display_price(&p, 1, 1.0), "3.2 div");
}

#[test]
fn unknown_constant_is_question_mark() {
    assert_eq!(UNKNOWN, "?");
}
