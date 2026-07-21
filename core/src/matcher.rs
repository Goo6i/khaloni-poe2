use strsim::normalized_levenshtein;

/// A fuzzy score must strictly exceed this to be a candidate at all. Set
/// above the raw entry-to-entry similarity between "Lesser Jewellers Orb"
/// and "Greater Jewellers Orb" (measured ~0.8095, i.e. ~0.81) so the two
/// variant names can never both clear the bar purely by being similar to
/// each other; a garbled line can still cross it and land Ambiguous (see
/// AMBIGUITY_MARGIN), but plain variant-to-variant similarity no longer can.
const FUZZY_THRESHOLD: f64 = 0.84;
/// A fuzzy score at or above this is treated as exact-confidence for
/// locking purposes (see MatchTier::locks_in_one): it still runs through
/// the ambiguity check below, but once it clears that, callers may lock a
/// display slot on a single read instead of waiting for a second
/// confirming scan.
const HIGH_CONFIDENCE_THRESHOLD: f64 = 0.92;
/// Only vocab entries whose normalized length is within this many
/// characters of the query are scored on the fuzzy tier. Keeps the
/// candidate set small and stops a short garbled query from ever fuzzy-
/// matching a much longer (or shorter) entry it has no real business
/// resembling.
const FUZZY_LEN_TOLERANCE: usize = 3;
/// Minimum query length for the prefix tier: short queries are too likely
/// to be a prefix of many unrelated entries.
const PREFIX_MIN_LEN: usize = 10;
/// If the second-best fuzzy candidate scores within this margin of the best,
/// the two vocab entries are too close to call and the row is Ambiguous
/// rather than a guess. Sized for near-identical variant families (e.g. the
/// Lesser/Greater/Perfect Jeweller's Orb line, whose entries sit ~0.86 apart
/// from each other) while still letting a clearly-best fuzzy match through.
const AMBIGUITY_MARGIN: f64 = 0.08;

/// OCR look-alike digits, folded back to the letters they're commonly
/// misread from, for the exact-match retry: 0<->o, 1<->l, 5<->s, 8<->b.
fn digit_fold(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '0' => 'o',
            '1' => 'l',
            '5' => 's',
            '8' => 'b',
            other => other,
        })
        .collect()
}

/// Lowercase, keep only [a-z0-9 ], collapse whitespace.
pub fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_space = true;
    for c in s.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
            last_space = false;
        } else if !last_space {
            out.push(' ');
            last_space = true;
        }
    }
    out.trim().to_string()
}

pub struct Vocab {
    entries: Vec<String>,
    normalized: Vec<String>,
}

impl Vocab {
    pub fn new(entries: Vec<String>) -> Vocab {
        let normalized = entries.iter().map(|e| normalize(e)).collect();
        Vocab {
            entries,
            normalized,
        }
    }

    pub fn entry(&self, index: usize) -> &str {
        &self.entries[index]
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchTier {
    /// The count-stripped query equals a vocab entry verbatim, either
    /// directly or after the OCR digit-look-alike fold.
    Exact,
    /// A vocab entry is contained verbatim in the unfiltered line; the
    /// longest (most specific) containing entry wins.
    Substring,
    /// The query is a prefix of exactly one vocab entry (or vice versa),
    /// with the query at least PREFIX_MIN_LEN characters long.
    Prefix,
    /// A fuzzy match whose score cleared HIGH_CONFIDENCE_THRESHOLD: treated
    /// as exact-confidence for locking, but still a fuzzy match by origin.
    HighConfidence,
    Fuzzy,
    /// Two or more vocab entries scored within AMBIGUITY_MARGIN of each
    /// other on the fuzzy tier; entry_index names the top candidate for
    /// diagnostics only and must not be priced or displayed as a guess.
    Ambiguous,
}

impl MatchTier {
    /// Exact/Substring/Prefix/HighConfidence are confident enough to lock a
    /// display slot after a single scan; plain Fuzzy needs a second,
    /// identical, confirming read first (see app::stabilize). Ambiguous
    /// never locks or displays at all.
    pub fn locks_in_one(self) -> bool {
        !matches!(self, MatchTier::Fuzzy | MatchTier::Ambiguous)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowHit {
    pub entry_index: usize,
    pub count: Option<u32>,
    pub tier: MatchTier,
}

/// Extracts a leading or embedded "Nx " count token from a normalized line and
/// returns (count, line with the token removed).
fn extract_count(line_norm: &str) -> (Option<u32>, String) {
    for (i, word) in line_norm.split_whitespace().enumerate() {
        if let Some(n) = word.strip_suffix('x') {
            if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) {
                if let Ok(c) = n.parse() {
                    let rest: Vec<&str> = line_norm
                        .split_whitespace()
                        .enumerate()
                        .filter(|(j, _)| *j != i)
                        .map(|(_, w)| w)
                        .collect();
                    return (Some(c), rest.join(" "));
                }
            }
        }
    }
    (None, line_norm.to_string())
}

/// Tiered matching, one hit per input line at most, checked in this order:
/// 1. Exact: the count-stripped query equals a vocabulary entry verbatim
///    (directly, or after folding OCR digit-look-alikes back to letters).
/// 2. Substring: a vocabulary entry contained verbatim in the UNFILTERED
///    line; the longest matching entry wins.
/// 3. Prefix (FILTERED line only): the query is a prefix of exactly one
///    vocabulary entry, or vice versa, and is at least PREFIX_MIN_LEN long.
/// 4. Fuzzy (FILTERED line only): normalized Levenshtein > FUZZY_THRESHOLD
///    between the count-stripped FILTERED line and a vocabulary entry
///    within FUZZY_LEN_TOLERANCE characters of it in length. A score at or
///    above HIGH_CONFIDENCE_THRESHOLD is tagged HighConfidence rather than
///    Fuzzy. When a second entry scores within AMBIGUITY_MARGIN of the
///    best, the hit is tagged Ambiguous instead of picking a winner, since
///    near-identical variant names can otherwise fuzzy-collide.
///
/// MatchTier::locks_in_one distinguishes tiers confident enough to display
/// after a single scan (Exact/Substring/Prefix/HighConfidence) from plain
/// Fuzzy, which callers should confirm with a second identical read first.
///
/// The count always comes from the same line that produced the name match.
pub fn match_rows(vocab: &Vocab, filtered: &[String], unfiltered: &[String]) -> Vec<RowHit> {
    let mut hits = Vec::new();

    let consider = |line: &str, allow_fuzzy: bool, hits: &mut Vec<RowHit>| {
        let norm = normalize(line);
        if norm.is_empty() {
            return;
        }
        let (count, name_part) = extract_count(&norm);

        // Exact tier: the count-stripped query equals a vocab entry
        // verbatim. Cheap and safe to try on both the noisy unfiltered line
        // and the confidence-filtered one, since equality (unlike substring
        // or fuzzy) can't be fooled by extra surrounding garbage. If plain
        // equality misses and the query contains a digit, retry once after
        // folding OCR digit-look-alikes (0/1/5/8) back to letters.
        if let Some(entry_index) = vocab.normalized.iter().position(|e| *e == name_part) {
            hits.push(RowHit { entry_index, count, tier: MatchTier::Exact });
            return;
        }
        if name_part.chars().any(|c| c.is_ascii_digit()) {
            let folded = digit_fold(&name_part);
            if let Some(entry_index) = vocab.normalized.iter().position(|e| *e == folded) {
                hits.push(RowHit { entry_index, count, tier: MatchTier::Exact });
                return;
            }
        }

        // Substring tier: every vocab entry contained verbatim in the
        // unfiltered line is a candidate; the longest (most specific) one
        // wins, e.g. "perfect jewellers orb" over "jewellers orb".
        let mut substring_best: Option<(usize, usize)> = None;
        for (i, entry) in vocab.normalized.iter().enumerate() {
            if entry.is_empty() {
                continue;
            }
            if norm.contains(entry.as_str()) {
                let len = entry.len();
                if substring_best.map(|(_, l)| len > l).unwrap_or(true) {
                    substring_best = Some((i, len));
                }
            }
        }
        if let Some((entry_index, _)) = substring_best {
            hits.push(RowHit {
                entry_index,
                count,
                tier: MatchTier::Substring,
            });
            return;
        }

        if !allow_fuzzy {
            return;
        }

        // Prefix tier: the query is long enough to be meaningful on its own
        // (>= PREFIX_MIN_LEN) and is a prefix of a vocab entry, or a vocab
        // entry is a prefix of it (e.g. a clipped or over-read panel line).
        // Ties go to the shortest qualifying entry, since it's the closest
        // match to the query's own length.
        if name_part.len() >= PREFIX_MIN_LEN {
            let mut prefix_best: Option<(usize, usize)> = None;
            for (i, entry) in vocab.normalized.iter().enumerate() {
                if entry.is_empty() || entry == &name_part {
                    continue;
                }
                if entry.starts_with(name_part.as_str()) || name_part.starts_with(entry.as_str()) {
                    let len = entry.len();
                    if prefix_best.map(|(_, l)| len < l).unwrap_or(true) {
                        prefix_best = Some((i, len));
                    }
                }
            }
            if let Some((entry_index, _)) = prefix_best {
                hits.push(RowHit {
                    entry_index,
                    count,
                    tier: MatchTier::Prefix,
                });
                return;
            }
        }

        // Fuzzy tier: score every vocab entry within FUZZY_LEN_TOLERANCE
        // characters of the query, and keep the best and the runner-up (a
        // different entry). A runner-up within AMBIGUITY_MARGIN of the best
        // means two variants are too close to tell apart, so the row is
        // reported Ambiguous rather than guessed. A score at or above
        // HIGH_CONFIDENCE_THRESHOLD is tagged HighConfidence instead of
        // Fuzzy so callers can lock on it in one scan.
        let mut best: Option<(usize, f64)> = None;
        let mut runner_up: Option<(usize, f64)> = None;
        for (i, entry) in vocab.normalized.iter().enumerate() {
            if entry.is_empty() {
                continue;
            }
            if entry.len().abs_diff(name_part.len()) > FUZZY_LEN_TOLERANCE {
                continue;
            }
            let ratio = normalized_levenshtein(&name_part, entry);
            if ratio <= FUZZY_THRESHOLD {
                continue;
            }
            match best {
                None => best = Some((i, ratio)),
                Some((_, best_score)) if ratio > best_score => {
                    runner_up = best;
                    best = Some((i, ratio));
                }
                Some(_) => {
                    if runner_up.map(|(_, r)| ratio > r).unwrap_or(true) {
                        runner_up = Some((i, ratio));
                    }
                }
            }
        }

        if let Some((entry_index, best_score)) = best {
            let ambiguous = match runner_up {
                Some((idx, score)) => idx != entry_index && best_score - score <= AMBIGUITY_MARGIN,
                None => false,
            };
            let tier = if ambiguous {
                MatchTier::Ambiguous
            } else if best_score >= HIGH_CONFIDENCE_THRESHOLD {
                MatchTier::HighConfidence
            } else {
                MatchTier::Fuzzy
            };
            hits.push(RowHit {
                entry_index,
                count,
                tier,
            });
        }
    };

    for line in unfiltered {
        consider(line, false, &mut hits);
    }
    for line in filtered {
        consider(line, true, &mut hits);
    }

    // dedupe: keep one hit per (entry, count), preferring the most
    // confident tier (Exact, Substring, Prefix, HighConfidence, Fuzzy,
    // Ambiguous last)
    hits.sort_by_key(|h| {
        (
            h.entry_index,
            h.count,
            match h.tier {
                MatchTier::Exact => 0,
                MatchTier::Substring => 1,
                MatchTier::Prefix => 2,
                MatchTier::HighConfidence => 3,
                MatchTier::Fuzzy => 4,
                MatchTier::Ambiguous => 5,
            },
        )
    });
    hits.dedup_by_key(|h| (h.entry_index, h.count));
    hits
}
