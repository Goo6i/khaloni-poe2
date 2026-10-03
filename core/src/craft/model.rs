//! Which (family, tier) entry rolls, under the two models the client's data
//! and this tool's own observations can back. PoE2 publishes no spawn
//! weights, so every craft probability comes from one of these and says
//! which.

use std::collections::HashMap;

use super::types::{AffixKind, Candidate, EntryId};

/// The label every uniform figure carries.
pub const UNIFORM_LABEL: &str = "optimistic floor: assumes every tier equally likely";

#[derive(Debug, Clone, PartialEq)]
pub enum Model {
    /// Every eligible entry equally likely: the only model the client's
    /// data supports, and optimistic because it rates a top tier as likely
    /// as a bottom one.
    Uniform,
    /// Frequencies of entries among listings of one item class that price
    /// checks fetched. Biased toward what sellers list.
    Observed {
        class: String,
        /// Times each entry was seen.
        counts: HashMap<EntryId, u32>,
        /// Entries seen per kind, the denominators of `share`.
        totals: HashMap<AffixKind, u32>,
        /// Listings the counts came from.
        listings: u32,
    },
}

impl Model {
    /// The relative weight of `c` in a draw. Under the observed model an
    /// entry never seen has weight 0: the sample gives no ground for any
    /// other figure.
    pub fn weight(&self, c: &Candidate) -> f64 {
        match self {
            Model::Uniform => 1.0,
            Model::Observed { counts, .. } => counts.get(&c.entry_id).copied().unwrap_or(0) as f64,
        }
    }

    /// The probability that `candidates[i]` is the one drawn from
    /// `candidates`; 0 when nothing in them has weight.
    pub fn p(&self, candidates: &[Candidate], i: usize) -> f64 {
        let total: f64 = candidates.iter().map(|c| self.weight(c)).sum();
        match candidates.get(i) {
            Some(c) if total > 0.0 => self.weight(c) / total,
            _ => 0.0,
        }
    }

    /// The entry's share of every entry of its kind seen on the class
    /// (observed), or `None` under the uniform model, which has no sample.
    pub fn share(&self, entry: &str, kind: AffixKind) -> Option<f64> {
        match self {
            Model::Uniform => None,
            Model::Observed { counts, totals, .. } => {
                let total = totals.get(&kind).copied().unwrap_or(0);
                (total > 0).then(|| counts.get(entry).copied().unwrap_or(0) as f64 / total as f64)
            }
        }
    }

    /// The line shown beside every figure this model produced.
    pub fn label(&self) -> String {
        match self {
            Model::Uniform => UNIFORM_LABEL.to_string(),
            Model::Observed { class, listings, .. } => format!("observed on {listings} listings of {class}"),
        }
    }
}
