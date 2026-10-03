//! The suggested price, read off the fetched listings closest to the
//! checked item by mod and tier. Nothing here is a model: every figure is
//! a listing's own price, and the words name which listings they are.
//!
//! Our side comes from the price check's built item (the mod rows and the
//! lines a figure counted, with the tiers the item text carries); theirs
//! from a listing's hover card (the API's description lines with their
//! tier badges). Both are reduced to the same key so they can be compared.

use crate::ee2::request::Built;
use crate::listing::{Card, LineKind, ListingView};

/// A mod line as a comparable key: every number becomes `#`, the game's
/// `[Tag|Display]` brackets fall to their display half, whitespace is
/// collapsed and case dropped, so the checked item's "+45 to maximum
/// Life" and a listing's "+52 to maximum Life" read alike.
pub fn mod_key(display_text: &str) -> String {
    let shown = crate::listing::display_text(display_text);
    let mut out = String::with_capacity(shown.len());
    let mut in_number = false;
    let mut pending_space = false;
    for c in shown.chars() {
        if c.is_whitespace() {
            pending_space = !out.is_empty();
            in_number = false;
            continue;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        if c.is_ascii_digit() || (c == '.' && in_number) {
            if !in_number {
                out.push('#');
                in_number = true;
            }
            continue;
        }
        in_number = false;
        out.extend(c.to_lowercase());
    }
    out
}

/// The tier in a badge: "P1" -> 1, "S3" -> 3, "P1 + S2" -> 1 (the first
/// number). None for a badge without one.
pub fn tier_of(badge: &str) -> Option<u8> {
    let start = badge.find(|c: char| c.is_ascii_digit())?;
    let digits: String = badge[start..].chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// A mod of the checked item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OurMod {
    pub key: String,
    pub tier: Option<u8>,
}

/// A mod of a listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TheirMod {
    pub key: String,
    pub tier: Option<u8>,
}

/// The mods that stand for a line of the checked item: the mod rows of the
/// search and the lines a figure or a total counted instead of searching,
/// of the same kinds a listing's card is compared on (see `searchable`).
/// A row's tier is the search's; a counted line's is read off the item's
/// own modifier header, found by the lines it spans.
pub fn ours_from_built(built: &Built) -> Vec<OurMod> {
    let is_mod = |tag: &str| matches!(tag, "explicit" | "implicit" | "desecrated" | "fractured" | "crafted");
    let rows = built
        .labels
        .iter()
        .filter(|l| is_mod(l.tag) && !l.lines.is_empty())
        .map(|l| OurMod { key: mod_key(&l.text), tier: l.tier });
    let counted = built.counted.iter().filter(|c| is_mod(c.tag)).map(|c| {
        let tier = built
            .item
            .new_mods
            .iter()
            .find(|m| m.stats.iter().any(|s| s.lines == c.lines))
            .and_then(|m| m.info.tier)
            .and_then(|t| u8::try_from(t).ok());
        OurMod { key: mod_key(&c.text), tier }
    });
    rows.chain(counted).collect()
}

/// Whether a card line is one the search can ask for: a mod of the item,
/// not a rune or an enchant it was given.
fn searchable(kind: LineKind) -> bool {
    matches!(
        kind,
        LineKind::Explicit | LineKind::Implicit | LineKind::Desecrated | LineKind::Fractured | LineKind::Crafted
    )
}

/// The searchable lines of a listing's card, with the tier of the first
/// badge on each.
pub fn theirs_from_card(card: &Card) -> Vec<TheirMod> {
    card.lines
        .iter()
        .filter(|l| searchable(l.kind))
        .map(|l| TheirMod { key: mod_key(&l.text), tier: l.tiers.first().and_then(|t| tier_of(t)) })
        .collect()
}

/// What our mod costs against the listing's mod of the same key: nothing
/// at the same tier, the tier difference when both are known, one step
/// when either is not.
fn tier_cost(ours: Option<u8>, theirs: Option<u8>) -> u32 {
    match (ours, theirs) {
        (Some(a), Some(b)) => u32::from(a.abs_diff(b)),
        _ => 1,
    }
}

/// Our mod against the listing: the index of the cheapest unused match of
/// its key and its cost, or None when the listing lacks the key.
fn best_match(ours: &OurMod, theirs: &[TheirMod], used: &[bool]) -> Option<(usize, u32)> {
    theirs
        .iter()
        .enumerate()
        .filter(|(i, t)| !used[*i] && t.key == ours.key)
        .map(|(i, t)| (i, tier_cost(ours.tier, t.tier)))
        .min_by_key(|(_, cost)| *cost)
}

/// How far a listing is from the checked item: per mod of ours, 0 when the
/// listing has it at the same tier, the tier difference when the tiers
/// differ, 1 when a tier is unknown on either side, 3 when the mod is
/// missing; plus 1 per searchable mod the listing has that we lack. A
/// key the listing carries twice (an explicit and a desecrated "+# to
/// Armour") pairs with our line of that key once.
pub fn distance(ours: &[OurMod], theirs: &[TheirMod]) -> u32 {
    let (pairs, used) = pair(ours, theirs);
    let missing_or_off: u32 = pairs.iter().map(|p| p.map_or(3, |(_, cost)| cost)).sum();
    missing_or_off + used.iter().filter(|u| !**u).count() as u32
}

/// A mod's name as the card would say it in passing: "+# to maximum Life"
/// is "life", "+#% to Fire Resistance" is "fire res".
pub fn short_name(key: &str) -> String {
    let mut s = key.trim();
    for prefix in ["+#% to ", "+# to ", "-#% to ", "-# to ", "#% to ", "# to ", "+#% ", "+# ", "#% ", "# "] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest;
            break;
        }
    }
    s.strip_prefix("maximum ").unwrap_or(s).replace("resistance", "res")
}

/// "T1 life" when the tier is known, else the name alone.
fn tiered(tier: Option<u8>, key: &str) -> String {
    match tier {
        Some(t) => format!("T{t} {}", short_name(key)),
        None => short_name(key),
    }
}

/// How the closest listings compare with the checked item.
#[derive(Debug, Clone, PartialEq)]
pub struct Closest {
    /// Listings at distance two or under, by (distance, price): the index
    /// into the caller's list and the distance.
    pub within: Vec<(usize, u32)>,
    /// The nearest listing of all: its index, distance and how it differs
    /// from ours (empty when it does not).
    pub nearest: Option<(usize, u32, String)>,
    pub text: String,
}

/// Our mods paired with the listing's: per mod of ours, the index of its
/// match and the cost, or None when the listing lacks it; and which of
/// theirs went unpaired.
fn pair(ours: &[OurMod], theirs: &[TheirMod]) -> (Vec<Option<(usize, u32)>>, Vec<bool>) {
    let mut used = vec![false; theirs.len()];
    let pairs = ours
        .iter()
        .map(|m| {
            let found = best_match(m, theirs, &used);
            if let Some((i, _)) = found {
                used[i] = true;
            }
            found
        })
        .collect();
    (pairs, used)
}

/// The listing's searchable mods we lack, worded.
fn extras(theirs: &[TheirMod], used: &[bool]) -> Vec<String> {
    theirs.iter().zip(used).filter(|(_, u)| !**u).map(|(t, _)| tiered(t.tier, &t.key)).collect()
}

fn tier_word(tier: Option<u8>) -> String {
    match tier {
        Some(t) => format!("T{t}"),
        None => "untiered".to_string(),
    }
}

/// What the listing has, mod by mod of ours: "T1 life", "T2 life against
/// your T3", "no cold res", then what it has besides. True when every mod
/// of ours is there at the same tier and nothing is besides.
fn has_text(ours: &[OurMod], theirs: &[TheirMod]) -> (String, bool) {
    let (pairs, used) = pair(ours, theirs);
    let mut alike = !ours.is_empty();
    let mut parts: Vec<String> = ours
        .iter()
        .zip(&pairs)
        .map(|(m, found)| match found {
            Some((_, 0)) if m.tier.is_some() => tiered(m.tier, &m.key),
            Some((i, _)) => {
                alike = false;
                let t = &theirs[*i];
                if m.tier.is_none() && t.tier.is_none() {
                    short_name(&m.key)
                } else {
                    format!("{} against your {}", tiered(t.tier, &m.key), tier_word(m.tier))
                }
            }
            None => {
                alike = false;
                format!("no {}", short_name(&m.key))
            }
        })
        .collect();
    let extra = extras(theirs, &used);
    if !extra.is_empty() {
        alike = false;
        parts.push(format!("{} besides", extra.join(", ")));
    }
    (parts.join(", "), alike)
}

/// "T1 life vs T3, no cold res, T2 fire res besides": only where the
/// listing differs from ours; empty when it does not.
fn difference_text(ours: &[OurMod], theirs: &[TheirMod]) -> String {
    let (pairs, used) = pair(ours, theirs);
    let mut parts: Vec<String> = ours
        .iter()
        .zip(&pairs)
        .filter_map(|(m, found)| match found {
            Some((_, 0)) => None,
            Some((i, _)) => Some(format!("{} vs {}", tiered(m.tier, &m.key), tier_word(theirs[*i].tier))),
            None => Some(format!("no {}", short_name(&m.key))),
        })
        .collect();
    let extra = extras(theirs, &used);
    if !extra.is_empty() {
        parts.push(format!("{} besides", extra.join(", ")));
    }
    parts.join(", ")
}

/// A price with one decimal at most and thousands separated: 38, 38.5,
/// 1,200.
pub fn price_text(price: f64) -> String {
    let tenths = (price.abs() * 10.0).round() as u64;
    let sign = if price < 0.0 && tenths > 0 { "-" } else { "" };
    if tenths.is_multiple_of(10) {
        format!("{sign}{}", with_thousands(tenths / 10))
    } else {
        format!("{sign}{}.{}", with_thousands(tenths / 10), tenths % 10)
    }
}

/// 1934 -> "1,934".
pub fn with_thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The listings within two of ours, and the nearest of all. `listings`
/// are the fetched rows with their price in the caller's display unit
/// (`unit` names it: "ex", "div"); indices in the result point into it.
pub fn closest(ours: &[OurMod], listings: &[(ListingView, f64)], unit: &str) -> Closest {
    let scored: Vec<(usize, u32)> =
        listings.iter().enumerate().map(|(i, (view, _))| (i, distance(ours, &theirs_from_card(&view.card)))).collect();
    let mut within: Vec<(usize, u32)> = scored.iter().copied().filter(|(_, d)| *d <= 2).collect();
    within.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| listings[a.0].1.total_cmp(&listings[b.0].1)));
    let nearest = scored
        .iter()
        .copied()
        .min_by(|a, b| a.1.cmp(&b.1).then_with(|| listings[a.0].1.total_cmp(&listings[b.0].1)))
        .map(|(i, d)| (i, d, difference_text(ours, &theirs_from_card(&listings[i].0.card))));

    let text = match &nearest {
        None => "no listings to compare with".to_string(),
        Some((_, _, diff)) if within.is_empty() => {
            format!("no close match among the cheapest {}; nearest differs by: {diff}", listings.len())
        }
        Some((i, _, diff)) if within.len() == 1 => {
            let how = if diff.is_empty() {
                "it has your mods at your tiers".to_string()
            } else {
                format!("it differs by: {diff}")
            };
            let price = price_text(listings[*i].1);
            format!("one close listing among the cheapest {}: {price} {unit}; {how}", listings.len())
        }
        Some((i, _, _)) => {
            let farthest = within.iter().map(|(_, d)| *d).max().unwrap_or(0);
            let how_close = match farthest {
                0 => "with your mods at your tiers",
                1 => "within one tier",
                _ => "within two tiers",
            };
            let mut prices: Vec<f64> = within.iter().map(|(i, _)| listings[*i].1).collect();
            prices.sort_by(f64::total_cmp);
            let prices: Vec<String> = prices.iter().map(|p| price_text(*p)).collect();
            let (has, alike) = has_text(ours, &theirs_from_card(&listings[*i].0.card));
            format!(
                "{} listings {how_close}: {} {unit}; the nearest has {has}{}",
                within.len(),
                prices.join(", "),
                if alike { " like yours" } else { "" }
            )
        }
    };
    Closest { within, nearest, text }
}

/// "cheapest 2, then 5, 6, 6, 8 ex · 20 of 1,934 matched": the first five
/// prices as fetched, how many were fetched and how many the search
/// matched. Empty without prices: a ladder with no rungs is not "0".
pub fn ladder_text(prices: &[f64], total: Option<u64>, unit: &str) -> String {
    // The site orders by its own currency equivalents, which need not agree
    // with the rates the prices were converted at: "cheapest" is ours.
    let mut sorted = prices.to_vec();
    sorted.sort_by(f64::total_cmp);
    let Some((first, rest)) = sorted.split_first() else { return String::new() };
    let mut out = format!("cheapest {}", price_text(*first));
    let then: Vec<String> = rest.iter().take(4).map(|p| price_text(*p)).collect();
    if !then.is_empty() {
        out.push_str(&format!(", then {}", then.join(", ")));
    }
    out.push_str(&format!(" {unit} · "));
    match total {
        Some(total) => out.push_str(&format!("{} of {} matched", prices.len(), with_thousands(total))),
        None => out.push_str(&format!("{} listings", prices.len())),
    }
    out
}

/// What one mod is worth: the cheapest listing with it and the cheapest
/// of the search without it.
#[derive(Debug, Clone, PartialEq)]
pub struct Attribution {
    pub label: String,
    pub with: f64,
    pub without: Option<f64>,
    pub text: String,
}

/// Per dropped mod, the baseline's cheapest against the cheapest of the
/// search with that mod's filter dropped; None when that search matched
/// nothing, which is said in words, never as a figure.
pub fn attribution(baseline_cheapest: f64, dropped: &[(String, Option<f64>)], unit: &str) -> Vec<Attribution> {
    dropped
        .iter()
        .map(|(label, without)| Attribution {
            label: label.clone(),
            with: baseline_cheapest,
            without: *without,
            text: match without {
                Some(w) => {
                    format!("{label}: with {} {unit}, without {} {unit}", price_text(baseline_cheapest), price_text(*w))
                }
                None => format!("{label}: without it nothing matched"),
            },
        })
        .collect()
}
