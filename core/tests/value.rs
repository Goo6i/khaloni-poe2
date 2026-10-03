use khaloni_poe2_core::ninja::Price;
use khaloni_poe2_core::value::{display_price, format_amount, UNKNOWN};

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

fn price3(divine: f64, exalted: f64, chaos: f64) -> Price {
    Price { divine, exalted, chaos }
}

#[test]
fn from_one_chaos_up_a_price_reads_in_chaos() {
    // 459 ex and 8.44 chaos to the divine: 0.4 div is 3.4 chaos, 184 ex.
    assert_eq!(display_price(&price3(0.4, 183.6, 3.376), 1, 1.0), "3.4 chaos");
    assert_eq!(display_price(&price3(0.1, 45.9, 0.844), 3, 1.0), "2.5 chaos (0.8 each)");
}

#[test]
fn small_change_stays_in_exalted() {
    assert_eq!(display_price(&price3(0.05, 23.0, 0.42), 1, 1.0), "23 ex");
}

#[test]
fn a_table_without_a_chaos_rate_falls_back_to_exalted() {
    assert_eq!(display_price(&price3(0.4, 183.6, 0.0), 1, 1.0), "184 ex");
}
