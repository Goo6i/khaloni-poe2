use crate::ninja::Price;

/// Shown for rows that must not be guessed (for example unreadable gem levels).
pub const UNKNOWN: &str = "?";

/// One decimal below 100, integers at or above 100, trailing .0 trimmed.
pub fn format_amount(x: f64) -> String {
    if x >= 100.0 {
        return format!("{}", x.round() as i64);
    }
    if x < 0.1 {
        return format!("{x:.2}");
    }
    let s = format!("{x:.1}");
    s.strip_suffix(".0").map(|t| t.to_string()).unwrap_or(s)
}

/// Total value of `count` items, in divine above the threshold, else exalted.
/// Stacks show the per-item value in parentheses.
pub fn display_price(unit: &Price, count: u32, divine_threshold: f64) -> String {
    let count = count.max(1);
    let total_divine = unit.divine * count as f64;
    let total_exalted = unit.exalted * count as f64;

    if total_divine >= divine_threshold {
        if count == 1 {
            format!("{} div", format_amount(total_divine))
        } else {
            format!(
                "{} div ({} div each)",
                format_amount(total_divine),
                format_amount(unit.divine)
            )
        }
    } else if count == 1 {
        format!("{} ex", format_amount(total_exalted))
    } else {
        format!(
            "{} ex ({} each)",
            format_amount(total_exalted),
            format_amount(unit.exalted)
        )
    }
}
