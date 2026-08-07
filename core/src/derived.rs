//! Statistics an item card shows that the clipboard text does not state
//! outright: weapon DPS figures and the trade site's pseudo totals.
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
//!
//! # Pseudo totals
//!
//! These mirror the trade site's `pseudo` stats, which is what makes them
//! useful for pricing:
//!
//! - Implicits and explicits both count; the trade pseudo does not care where
//!   a stat came from.
//! - Elemental resistance is Fire + Cold + Lightning only. Chaos resistance is
//!   a separate pseudo, and maximum-resistance mods are a different stat
//!   entirely.
//! - A mod granting several stats at once counts once per stat it grants, so
//!   `+5 to all Attributes` is worth 15 attributes and `+12% to all Elemental
//!   Resistances` is worth 36 resistance.
//! - Percentage-increase mods are not flat grants and never fold in.

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

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PseudoTotals {
    pub total_life: f64,
    pub total_es: f64,
    pub total_elemental_resistance: f64,
    pub total_attributes: f64,
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

/// Trade-style pseudo totals summed over the item's implicit and explicit mods.
pub fn pseudo_totals(item: &Item) -> PseudoTotals {
    let mut totals = PseudoTotals::default();
    for m in item.implicits.iter().chain(item.explicits.iter()) {
        // Advanced-format mod text carries its roll range inline
        // (`+31(31-33) to Dexterity`); the actual roll is the bare number.
        let text = without_parentheticals(&m.text);
        let Some((value, target)) = split_flat_grant(&text) else {
            continue;
        };
        match target {
            "maximum Life" => totals.total_life += value,
            "maximum Energy Shield" => totals.total_es += value,
            _ => {
                totals.total_elemental_resistance += value * elemental_resistances(target);
                totals.total_attributes += value * attributes(target);
            }
        }
    }
    totals
}

/// Splits a flat grant into its rolled value and what it grants:
/// `"+31 to Dexterity"` → `(31.0, "Dexterity")`, `"+28% to Fire Resistance"` →
/// `(28.0, "Fire Resistance")`. `None` for any other mod shape, including
/// percentage-increase mods, which have no ` to ` and no leading number.
fn split_flat_grant(text: &str) -> Option<(f64, &str)> {
    let (head, target) = text.trim().split_once(" to ")?;
    let value = head.trim().trim_end_matches('%').parse().ok()?;
    Some((value, target.trim()))
}

/// How many elemental resistances a grant target covers: `"Fire Resistance"` →
/// 1, `"all Elemental Resistances"` → 3, `"Chaos Resistance"` → 0.
fn elemental_resistances(target: &str) -> f64 {
    if !target.ends_with("Resistance") && !target.ends_with("Resistances") {
        return 0.0;
    }
    // Maximum resistance is its own trade stat and is never part of this total.
    if target.contains("Maximum") || target.contains("maximum") {
        return 0.0;
    }
    if target.starts_with("all ") {
        return 3.0;
    }
    ["Fire", "Cold", "Lightning"]
        .iter()
        .filter(|e| target.contains(*e))
        .count() as f64
}

/// How many attributes a grant target covers: `"Dexterity"` → 1,
/// `"all Attributes"` → 3.
fn attributes(target: &str) -> f64 {
    if target == "all Attributes" {
        return 3.0;
    }
    ["Strength", "Dexterity", "Intelligence"]
        .iter()
        .filter(|a| target.contains(*a))
        .count() as f64
}

/// Sum of the midpoints of every comma-separated range in a damage property
/// value: `"12-24 (augmented), 5-9 (augmented)"` → 25.
fn average_damage(value: &str) -> f64 {
    without_parentheticals(value)
        .split(',')
        .filter_map(range_midpoint)
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
