//! Roll quality: where an item's actual rolled value sits in its affix's tier
//! ladder, for the Evaluate panel's tiering badge and scoring gutter.
//!
//! # Ladder direction
//!
//! [`refdata::AffixTier`] ladders are stored ilvl-ascending (see
//! [`refdata::parse_affixes_tiered`]), so the LAST entry is the top tier — the
//! one that needs the highest item level and rolls the biggest numbers. Players
//! count the other way round: the top tier is "T1". So tier numbers here are
//! `n - i` for a ladder of `n` rungs and a 0-based index `i` into the stored
//! (worst-first) order: the last stored tier is T1, the first is T`n`.
//!
//! # Placing a roll
//!
//! A roll is placed in the tier whose range contains it, searched from the top
//! (T1) down, so where ladders overlap the roll is credited to the best tier
//! that could have produced it. A value outside every range is clamped to the
//! nearest tier by distance (ties go to the better tier) rather than being
//! rejected or panicking — items carry rolls from quality, corruption, and
//! crafted variants that a plain ladder does not cover, and dropping them
//! would lose the badge entirely.
//!
//! `None` is returned only when there is nothing to place the roll against: an
//! empty ladder, a ladder whose ranges none parse, or a non-finite value. This
//! module never invents a tier or a score.
//!
//! # Ranges
//!
//! Tier ranges are the strings [`refdata::AffixTier::range`] carries:
//! `"200-214"` for a rolled range, `"35"` for a fixed roll, and
//! `"26-39, 44-66"` for a single-line mod with two stats (added min/max
//! damage). Multi-part ranges are scored against the FIRST part only, because
//! the caller has one rolled value to place and the first stat is the one the
//! affix's readable text leads with; the remaining parts are ignored.
//!
//! # Scoring
//!
//! [`score`] blends how far up the ladder the roll reached with where it sits
//! inside that tier: `5 * (i + f) / n`, where `i` is the 0-based worst-first
//! index of the matched tier, `f` the fraction of that tier's range the roll
//! reached (clamped to `0..=1`; a fixed-roll tier is `1.0`, since the only roll
//! it has is its best), and `n` the number of rungs. A ladder-topping max roll
//! is 5.0, the floor of the bottom tier is 0.0, and the middle of a middle tier
//! lands in between.
//!
//! Bigger is better is assumed, which is how every ladder in the export that
//! matters reads (`min <= max`, higher tiers rolling higher). The handful of
//! mods whose value is negative ("reduced Attribute Requirements") therefore
//! score on raw numeric order, not on player-perceived goodness.

use crate::refdata::AffixTier;

/// The first part of a tier range string as `(min, max)`: `"200-214"` →
/// `(200, 214)`, `"35"` → `(35, 35)` (a fixed roll), `"26-39, 44-66"` →
/// `(26, 39)` (see the module docs on multi-part ranges). Negative bounds
/// parse too (`"-25--10"` → `(-25, -10)`), since the separator is the first
/// `-` after the leading sign. `None` when the text is not a number pair.
pub fn first_range(range: &str) -> Option<(f64, f64)> {
    let part = range.split(',').next()?.trim();
    if part.is_empty() {
        return None;
    }
    // The separator is the first '-' that is not the leading sign. Indexed by
    // char so a non-ASCII first character cannot split a byte boundary.
    match part.char_indices().skip(1).find(|&(_, c)| c == '-').map(|(i, _)| i) {
        Some(sep) => {
            let min: f64 = part[..sep].trim().parse().ok()?;
            let max: f64 = part[sep + 1..].trim().parse().ok()?;
            (min.is_finite() && max.is_finite()).then_some((min, max))
        }
        None => {
            let v: f64 = part.parse().ok()?;
            v.is_finite().then_some((v, v))
        }
    }
}

/// The ladder as scorable rungs, worst-first, dropping tiers whose range does
/// not parse (they are not rungs anything can be scored against).
fn rungs(tiers: &[AffixTier]) -> Vec<(f64, f64)> {
    tiers.iter().filter_map(|t| first_range(&t.range)).collect()
}

/// Index of the rung a value belongs to, worst-first. Containment is searched
/// from the top tier down; a value outside every range falls to the nearest
/// rung by distance, ties going to the better (higher-index) one.
fn place(rungs: &[(f64, f64)], value: f64) -> Option<usize> {
    if rungs.is_empty() || !value.is_finite() {
        return None;
    }
    if let Some(i) = (0..rungs.len()).rev().find(|&i| {
        let (min, max) = rungs[i];
        value >= min.min(max) && value <= max.max(min)
    }) {
        return Some(i);
    }
    // Out of range (below the ladder, above it, or in a gap between rungs):
    // clamp to whichever rung it is closest to.
    let dist = |i: usize| {
        let (min, max) = rungs[i];
        let (lo, hi) = (min.min(max), max.max(min));
        if value < lo { lo - value } else { value - hi }
    };
    (0..rungs.len()).rev().min_by(|&a, &b| {
        dist(a).partial_cmp(&dist(b)).unwrap_or(std::cmp::Ordering::Equal)
    })
}

/// Which tier a rolled `value` falls in, 1 = best (T1 = the last, highest-ilvl
/// entry of the stored ladder). Out-of-range values clamp to the nearest tier.
/// `None` for an empty ladder, a ladder with no parseable range, or a
/// non-finite value.
pub fn tier_of(tiers: &[AffixTier], value: f64) -> Option<u8> {
    let rungs = rungs(tiers);
    let i = place(&rungs, value)?;
    // Worst-first index -> player-facing tier number, saturating for the
    // absurd ladder lengths that cannot occur in the real export.
    Some(u8::try_from(rungs.len() - i).unwrap_or(u8::MAX))
}

/// Roll quality on a 0.0..=5.0 scale: how good `value` is across the whole
/// ladder, combining which tier it reached with where it sits inside that
/// tier's range (see the module docs for the exact blend). `None` for an empty
/// ladder, a ladder with no parseable range, or a non-finite value — never a
/// fabricated number.
pub fn score(tiers: &[AffixTier], value: f64) -> Option<f32> {
    let rungs = rungs(tiers);
    let i = place(&rungs, value)?;
    let (min, max) = rungs[i];
    let (lo, hi) = (min.min(max), max.max(min));
    // A fixed-roll tier has one possible roll, and it is that tier's best.
    let f = if hi > lo { ((value - lo) / (hi - lo)).clamp(0.0, 1.0) } else { 1.0 };
    let overall = (i as f64 + f) / rungs.len() as f64;
    Some((5.0 * overall).clamp(0.0, 5.0) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::refdata::AffixKind;

    /// A ladder rung; `kind` is irrelevant to scoring, so tests use one value.
    fn tier(ilvl: u32, range: &str) -> AffixTier {
        AffixTier { ilvl, range: range.to_string(), kind: AffixKind::Prefix }
    }

    /// Three rungs, ilvl-ascending as core stores them: T3, T2, T1.
    fn ladder() -> Vec<AffixTier> {
        vec![tier(1, "5-8"), tier(11, "9-12"), tier(22, "13-20")]
    }

    #[test]
    fn ranges_parse_including_fixed_and_multi_part_and_negative() {
        assert_eq!(first_range("200-214"), Some((200.0, 214.0)));
        assert_eq!(first_range("35"), Some((35.0, 35.0)), "fixed roll");
        assert_eq!(first_range("26-39, 44-66"), Some((26.0, 39.0)), "first part only");
        assert_eq!(first_range("-25--10"), Some((-25.0, -10.0)), "negative bounds");
        assert_eq!(first_range(""), None);
        assert_eq!(first_range("n/a"), None);
        assert_eq!(first_range("5-"), None, "half a range is not a range");
    }

    #[test]
    fn tier_numbering_counts_down_from_the_top_of_the_ladder() {
        let l = ladder();
        assert_eq!(tier_of(&l, 20.0), Some(1), "last stored tier is T1");
        assert_eq!(tier_of(&l, 13.0), Some(1));
        assert_eq!(tier_of(&l, 10.0), Some(2));
        assert_eq!(tier_of(&l, 5.0), Some(3), "first stored tier is the worst");
    }

    #[test]
    fn scores_span_zero_to_five_across_the_ladder() {
        let l = ladder();
        let s = |v: f64| score(&l, v).unwrap();
        assert!((s(20.0) - 5.0).abs() < 1e-6, "ladder-topping max roll is 5");
        assert!(s(5.0).abs() < 1e-6, "bottom of the bottom tier is 0");
        // Middle of the middle tier: (1 + 0.5) / 3 * 5.
        assert!((s(10.5) - 2.5).abs() < 1e-6, "mid-ladder mid-roll is mid-score");
        // Monotone up the ladder.
        assert!(s(5.0) < s(8.0) && s(8.0) < s(10.5) && s(10.5) < s(13.0) && s(13.0) < s(20.0));
        for v in [5.0, 8.0, 10.5, 13.0, 20.0] {
            let x = s(v);
            assert!((0.0..=5.0).contains(&x), "{v} scored {x}, outside 0..=5");
        }
    }

    #[test]
    fn out_of_range_values_clamp_instead_of_panicking() {
        let l = ladder();
        assert_eq!(tier_of(&l, 999.0), Some(1), "above the ladder clamps to the top");
        assert!((score(&l, 999.0).unwrap() - 5.0).abs() < 1e-6);
        assert_eq!(tier_of(&l, -50.0), Some(3), "below the ladder clamps to the bottom");
        assert!(score(&l, -50.0).unwrap().abs() < 1e-6);
        // A gap between rungs goes to the nearer one.
        let gapped = vec![tier(1, "5-8"), tier(22, "13-20")];
        assert_eq!(tier_of(&gapped, 9.0), Some(2), "nearer the low rung");
        assert_eq!(tier_of(&gapped, 12.0), Some(1), "nearer the high rung");
        assert_eq!(tier_of(&gapped, 10.5), Some(1), "exact tie goes to the better tier");
    }

    #[test]
    fn fixed_roll_tier_scores_as_that_tier_maxed() {
        let l = vec![tier(1, "5-8"), tier(22, "13")];
        assert_eq!(tier_of(&l, 13.0), Some(1));
        assert!((score(&l, 13.0).unwrap() - 5.0).abs() < 1e-6, "the only roll it has is its best");
        // A single-rung ladder still spans the full scale.
        let one = vec![tier(1, "10-20")];
        assert_eq!(tier_of(&one, 15.0), Some(1));
        assert!((score(&one, 15.0).unwrap() - 2.5).abs() < 1e-6);
    }

    #[test]
    fn multi_part_ranges_score_against_the_first_part() {
        // Added damage: "min-max, min-max"; the roll placed is the first stat.
        let l = vec![tier(1, "10-15, 20-30"), tier(40, "26-39, 44-66")];
        assert_eq!(tier_of(&l, 30.0), Some(1), "30 is in the top tier's first part");
        assert_eq!(tier_of(&l, 12.0), Some(2));
        assert!((score(&l, 39.0).unwrap() - 5.0).abs() < 1e-6);
    }

    #[test]
    fn unscorable_ladders_yield_none() {
        assert_eq!(tier_of(&[], 5.0), None, "empty ladder");
        assert_eq!(score(&[], 5.0), None);
        let junk = vec![tier(1, ""), tier(2, "unknown")];
        assert_eq!(tier_of(&junk, 5.0), None, "no parseable range to place against");
        assert_eq!(score(&junk, 5.0), None);
        assert_eq!(score(&ladder(), f64::NAN), None, "non-finite value is not placeable");
        assert_eq!(tier_of(&ladder(), f64::INFINITY), None);
        // A ladder with one bad rung still scores against the rungs it has.
        let partial = vec![tier(1, "?"), tier(11, "9-12"), tier(22, "13-20")];
        assert_eq!(tier_of(&partial, 20.0), Some(1));
        assert_eq!(tier_of(&partial, 10.0), Some(2), "unparseable rungs are not counted");
    }
}
