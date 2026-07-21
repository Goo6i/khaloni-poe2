use strsim::normalized_levenshtein;

const FUZZY_THRESHOLD: f64 = 0.75;

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
/// 1. Substring: a vocabulary entry contained verbatim in the UNFILTERED line.
/// 2. Fuzzy: normalized Levenshtein >= 0.75 between the count-stripped
///    FILTERED line and a vocabulary entry.
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
        let mut best: Option<(usize, MatchTier, f64)> = None;
        for (i, entry) in vocab.normalized.iter().enumerate() {
            if entry.is_empty() {
                continue;
            }
            if norm.contains(entry.as_str()) {
                let score = entry.len() as f64;
                if best.map(|(_, _, s)| score > s).unwrap_or(true) {
                    best = Some((i, MatchTier::Substring, score));
                }
            } else if allow_fuzzy {
                let ratio = normalized_levenshtein(&name_part, entry);
                if ratio >= FUZZY_THRESHOLD {
                    let already_substring =
                        matches!(best, Some((_, MatchTier::Substring, _)));
                    if !already_substring
                        && best.map(|(_, _, s)| ratio > s).unwrap_or(true)
                    {
                        best = Some((i, MatchTier::Fuzzy, ratio));
                    }
                }
            }
        }
        if let Some((entry_index, tier, _)) = best {
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

    // dedupe: keep one hit per (entry, count), preferring Substring
    hits.sort_by_key(|h| {
        (
            h.entry_index,
            h.count,
            match h.tier {
                MatchTier::Substring => 0,
                MatchTier::Fuzzy => 1,
            },
        )
    });
    hits.dedup_by_key(|h| (h.entry_index, h.count));
    hits
}
