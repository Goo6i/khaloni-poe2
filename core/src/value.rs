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

/// The currency a value is shown in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Divine,
    Chaos,
    Exalted,
}

impl Unit {
    pub fn suffix(self) -> &'static str {
        match self {
            Unit::Divine => "div",
            Unit::Chaos => "chaos",
            Unit::Exalted => "ex",
        }
    }
}

/// Picks the currency for `count` items and returns the total and the
/// per-item value in it: divine at or above the threshold, chaos from one
/// chaos up, exalted for what is left. An exalted is worth about a fiftieth
/// of a chaos (459 to the divine against 8.4, 2026-09-19), so exalted
/// figures for anything but small change ran to three digits and said
/// little; below one chaos it is still the unit that reads ("12 ex", not
/// "0.22 chaos").
pub fn pick_unit(unit: &Price, count: u32, divine_threshold: f64) -> (Unit, f64, f64) {
    let n = f64::from(count.max(1));
    if unit.divine * n >= divine_threshold {
        (Unit::Divine, unit.divine * n, unit.divine)
    } else if unit.chaos * n >= 1.0 {
        (Unit::Chaos, unit.chaos * n, unit.chaos)
    } else {
        (Unit::Exalted, unit.exalted * n, unit.exalted)
    }
}

/// Total value of `count` items in the currency `pick_unit` chooses. Stacks
/// show the per-item value in parentheses.
pub fn display_price(unit: &Price, count: u32, divine_threshold: f64) -> String {
    let (u, total, each) = pick_unit(unit, count, divine_threshold);
    if count.max(1) == 1 {
        format!("{} {}", format_amount(total), u.suffix())
    } else if u == Unit::Divine {
        format!("{} div ({} div each)", format_amount(total), format_amount(each))
    } else {
        format!("{} {} ({} each)", format_amount(total), u.suffix(), format_amount(each))
    }
}
