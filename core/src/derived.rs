//! Statistics an item card shows that the clipboard text does not state
//! outright: weapon DPS figures.
//!
//! Everything here is derived from the item's own text. Nothing is looked up,
//! defaulted, or inferred from the base type — a figure this module reports is
//! one the item said, arithmetic aside.
//!
//! # Weapon DPS
//!
//! Damage lives in the item's property section as `Key: value` lines, which
//! [`crate::item::parse_item`] leaves in [`Item::sections`] and keeps out of
//! the mod lists. The value may carry display suffixes (`(augmented)`,
//! `(lightning)`) and, in the aggregate elemental form, several
//! comma-separated ranges. A range is worth its midpoint, and DPS is average
//! damage times attacks per second.
//!
//! Attacks per second is what makes an item a weapon here: without it a damage
//! range has no rate to multiply by, so [`weapon_stats`] reports `None` rather
//! than a DPS that means nothing.

use crate::item::Item;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct WeaponStats {
    pub phys_dps: f64,
    pub ele_dps: f64,
    pub chaos_dps: f64,
    pub total_dps: f64,
    pub aps: f64,
    pub crit_chance: f64,
}

/// Per-element damage property lines. PoE2 exports list the elements it rolled
/// individually; the older aggregate `Elemental Damage:` line packs them into
/// one comma-separated value instead.
const ELEMENT_DAMAGE_PREFIXES: [&str; 3] =
    ["Fire Damage: ", "Cold Damage: ", "Lightning Damage: "];

/// DPS figures for a weapon, or `None` when the item states no attack rate.
pub fn weapon_stats(item: &Item) -> Option<WeaponStats> {
    let mut aps: Option<f64> = None;
    let mut crit_chance = 0.0;
    let mut phys = 0.0;
    let mut chaos = 0.0;
    let mut ele_per_element = 0.0;
    let mut ele_aggregate = 0.0;
    let mut has_per_element = false;

    for line in item.sections.iter().flatten() {
        if let Some(v) = line.strip_prefix("Attacks per Second: ") {
            aps = number(v);
        } else if let Some(v) = line.strip_prefix("Critical Hit Chance: ") {
            crit_chance = number(v).unwrap_or(0.0);
        } else if let Some(v) = line.strip_prefix("Physical Damage: ") {
            phys += average_damage(v);
        } else if let Some(v) = line.strip_prefix("Chaos Damage: ") {
            chaos += average_damage(v);
        } else if let Some(v) = line.strip_prefix("Elemental Damage: ") {
            ele_aggregate += average_damage(v);
        } else if let Some(v) = ELEMENT_DAMAGE_PREFIXES
            .iter()
            .find_map(|p| line.strip_prefix(p))
        {
            has_per_element = true;
            ele_per_element += average_damage(v);
        }
    }

    let aps = aps?;
    // The aggregate line is the sum of the per-element ones, so an export
    // carrying both forms must not be counted twice.
    let ele = if has_per_element {
        ele_per_element
    } else {
        ele_aggregate
    };

    let phys_dps = phys * aps;
    let ele_dps = ele * aps;
    let chaos_dps = chaos * aps;
    Some(WeaponStats {
        phys_dps,
        ele_dps,
        chaos_dps,
        total_dps: phys_dps + ele_dps + chaos_dps,
        aps,
        crit_chance,
    })
}

/// Sum of the midpoints of every comma-separated range in a damage property
/// value: `"12-24 (augmented), 5-9 (augmented)"` → 25. Ranges are separated
/// by a comma and a space; a bare comma is a thousands separator
/// (`"414-1,043"`), which a split on every comma read as two ranges.
fn average_damage(value: &str) -> f64 {
    without_parentheticals(value)
        .split(", ")
        .filter_map(|part| range_midpoint(&part.replace(',', "")))
        .sum()
}

/// `"266-499"` → 382.5, `"35"` → 35. Damage bounds are never negative, so the
/// first `-` is always the range separator.
fn range_midpoint(part: &str) -> Option<f64> {
    let part = part.trim();
    match part.split_once('-') {
        Some((lo, hi)) => {
            let lo: f64 = lo.trim().parse().ok()?;
            let hi: f64 = hi.trim().parse().ok()?;
            Some((lo + hi) / 2.0)
        }
        None => part.parse().ok(),
    }
}

/// A scalar property value, ignoring display suffixes and a trailing percent:
/// `"8.48% (augmented)"` → 8.48.
fn number(value: &str) -> Option<f64> {
    without_parentheticals(value)
        .trim()
        .trim_end_matches('%')
        .trim()
        .parse()
        .ok()
}

/// Drops every parenthesized group. In property values these are display tags
/// (`(augmented)`, `(lightning)`); in mod text they are roll ranges.
fn without_parentheticals(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0usize;
    for c in s.chars() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}
