use strsim::normalized_levenshtein;

const FUZZY_THRESHOLD: f64 = 0.75;
/// If the second-best fuzzy candidate scores within this margin of the best,
/// the two vocab entries are too close to call and the row is Ambiguous
/// rather than a guess. Sized for near-identical variant families (e.g. the
/// Lesser/Greater/Perfect Jeweller's Orb line, whose entries sit ~0.86 apart
/// from each other) while still letting a clearly-best fuzzy match through.
const AMBIGUITY_MARGIN: f64 = 0.08;

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
    Substring,
    Fuzzy,
    /// Two or more vocab entries scored within AMBIGUITY_MARGIN of each
    /// other on the fuzzy tier; entry_index names the top candidate for
    /// diagnostics only and must not be priced or displayed as a guess.
    Ambiguous,
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

/// Two-tier matching, one hit per input line at most:
/// 1. Substring: a vocabulary entry contained verbatim in the UNFILTERED
///    line; the longest matching entry wins.
/// 2. Fuzzy: normalized Levenshtein >= 0.75 between the count-stripped
///    FILTERED line and a vocabulary entry. When a second entry scores
///    within AMBIGUITY_MARGIN of the best (and isn't an exact normalized
///    match), the hit is tagged Ambiguous instead of picking a winner, since
///    near-identical variant names can otherwise fuzzy-collide.
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

        // Fuzzy tier: score every vocab entry and keep the best and the
        // runner-up (a different entry). A runner-up within AMBIGUITY_MARGIN
        // of the best means two variants are too close to tell apart, so the
        // row is reported Ambiguous rather than guessed. An exact normalized
        // match always wins outright, ambiguity or not.
        let mut best: Option<(usize, f64)> = None;
        let mut runner_up: Option<(usize, f64)> = None;
        for (i, entry) in vocab.normalized.iter().enumerate() {
            if entry.is_empty() {
                continue;
            }
            let ratio = normalized_levenshtein(&name_part, entry);
            if ratio < FUZZY_THRESHOLD {
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
            let is_exact = name_part == vocab.normalized[entry_index];
            let ambiguous = !is_exact
                && match runner_up {
                    Some((idx, score)) => {
                        idx != entry_index && best_score - score <= AMBIGUITY_MARGIN
                    }
                    None => false,
                };
            let tier = if ambiguous {
                MatchTier::Ambiguous
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
    // confident tier (Substring, then Fuzzy, then Ambiguous last)
    hits.sort_by_key(|h| {
        (
            h.entry_index,
            h.count,
            match h.tier {
                MatchTier::Substring => 0,
                MatchTier::Fuzzy => 1,
                MatchTier::Ambiguous => 2,
            },
        )
    });
    hits.dedup_by_key(|h| (h.entry_index, h.count));
    hits
}
