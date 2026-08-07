//! Turning a page of trade listings into one honest number.
//!
//! A raw listing list answers "what are people asking?", not "what is this
//! worth?". The trade site's cheapest entries are routinely bait, dead
//! accounts, or mispriced, and the top end is fantasy, so a plain min or
//! mean is misleading in both directions. This computes a trimmed central
//! value, keeps the FULL observed range visible next to it, and rates its
//! own confidence — a wide spread or a thin sample must show as unreliable
//! rather than hide behind a precise-looking number.

/// How much the headline number deserves to be trusted, from the sample
/// size and how far apart the asking prices are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Reliability {
    VeryLow,
    Low,
    Medium,
    High,
}

impl Reliability {
    pub fn label(self) -> &'static str {
        match self {
            Reliability::VeryLow => "Very Low",
            Reliability::Low => "Low",
            Reliability::Medium => "Medium",
            Reliability::High => "High",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Estimate {
    /// Headline value in exalted: the median of the middle half, so a few
    /// lowballs or moonshots cannot drag it.
    pub exalted: f64,
    /// Lowest and highest asking prices actually seen, untrimmed — the
    /// honest spread behind the headline.
    pub low: f64,
    pub high: f64,
    /// Listings that contributed (unpriceable currencies already dropped).
    pub count: usize,
    pub reliability: Reliability,
}

/// Percentile by nearest rank over an ascending slice; `q` in 0.0..=1.0.
fn percentile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() - 1) as f64 * q).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn median(sorted: &[f64]) -> f64 {
    match sorted.len() {
        0 => 0.0,
        n if n % 2 == 1 => sorted[n / 2],
        n => (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0,
    }
}

/// Estimates a price from listing values already normalized to exalted.
/// `None` when nothing priceable was supplied. Non-finite and non-positive
/// values are dropped: a listing of "0" is a placeholder, not a price.
pub fn estimate(values_exalted: &[f64]) -> Option<Estimate> {
    let mut v: Vec<f64> = values_exalted.iter().copied().filter(|x| x.is_finite() && *x > 0.0).collect();
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    let count = v.len();
    let low = v[0];
    let high = v[count - 1];

    let q1 = percentile(&v, 0.25);
    let q3 = percentile(&v, 0.75);
    // Middle half only; with a handful of listings that degenerates to the
    // whole set, which is the right behavior for a thin sample.
    let trimmed: Vec<f64> = v.iter().copied().filter(|x| *x >= q1 && *x <= q3).collect();
    let exalted = if trimmed.is_empty() { median(&v) } else { median(&trimmed) };

    Some(Estimate { exalted, low, high, count, reliability: reliability(count, q1, q3) })
}

/// Confidence from sample size and dispersion. The ratio (not the
/// difference) is what matters: 1ex vs 4ex is the same uncertainty as
/// 100ex vs 400ex to someone deciding whether to sell.
fn reliability(count: usize, q1: f64, q3: f64) -> Reliability {
    if count < 5 {
        return Reliability::VeryLow;
    }
    let ratio = if q1 > 0.0 { q3 / q1 } else { f64::INFINITY };
    let by_spread = if ratio > 4.0 {
        Reliability::VeryLow
    } else if ratio > 2.5 {
        Reliability::Low
    } else if ratio > 1.6 {
        Reliability::Medium
    } else {
        Reliability::High
    };
    // A tight but tiny sample is still a tiny sample.
    if count < 10 {
        by_spread.min(Reliability::Medium)
    } else {
        by_spread
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_lowball_and_moonshot_outliers() {
        // Eleven listings clustered near 5, with one bait at 0.1 and one
        // fantasy at 400: the headline must stay in the cluster.
        let v = [0.1, 4.5, 4.8, 5.0, 5.0, 5.2, 5.5, 5.5, 6.0, 6.5, 400.0];
        let e = estimate(&v).unwrap();
        assert!((4.5..=6.0).contains(&e.exalted), "headline {} left the cluster", e.exalted);
        // The range still tells the truth about what was seen.
        assert_eq!(e.low, 0.1);
        assert_eq!(e.high, 400.0);
        assert_eq!(e.count, 11);
    }

    #[test]
    fn wide_spread_reads_as_unreliable() {
        // The screenshot case: 0.62 to 49 divine is not a price, it is a
        // shrug, and must be labelled as one.
        let v = [0.62, 1.0, 2.0, 5.5, 8.0, 15.0, 30.0, 49.0];
        let e = estimate(&v).unwrap();
        assert_eq!(e.reliability, Reliability::VeryLow);
        assert_eq!(e.reliability.label(), "Very Low");
    }

    #[test]
    fn tight_deep_sample_reads_as_reliable() {
        let v: Vec<f64> = (0..20).map(|i| 10.0 + f64::from(i % 3)).collect();
        let e = estimate(&v).unwrap();
        assert_eq!(e.reliability, Reliability::High);
    }

    #[test]
    fn a_thin_sample_never_claims_confidence() {
        // Four identical prices look perfect and prove nothing.
        let e = estimate(&[5.0, 5.0, 5.0, 5.0]).unwrap();
        assert_eq!(e.reliability, Reliability::VeryLow);
        // Even a tight nine-listing sample is capped below High.
        let nine: Vec<f64> = std::iter::repeat_n(7.0, 9).collect();
        assert_eq!(estimate(&nine).unwrap().reliability, Reliability::Medium);
    }

    #[test]
    fn junk_values_are_dropped_and_emptiness_is_none() {
        let e = estimate(&[0.0, -3.0, f64::NAN, 8.0, 8.0]).unwrap();
        assert_eq!(e.count, 2);
        assert_eq!(e.exalted, 8.0);
        assert_eq!(estimate(&[]), None);
        assert_eq!(estimate(&[0.0, f64::NAN]), None);
    }

    #[test]
    fn single_listing_is_reported_but_distrusted() {
        let e = estimate(&[12.5]).unwrap();
        assert_eq!((e.exalted, e.low, e.high, e.count), (12.5, 12.5, 12.5, 1));
        assert_eq!(e.reliability, Reliability::VeryLow);
    }
}
