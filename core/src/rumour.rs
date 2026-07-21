//! Island Rumour dataset and name matching, ported from the reference
//! overlay's rumour system: a community Google Sheet (CSV export) maps
//! rumour names to map type, mods, and rating. Matching tolerates OCR
//! damage through the reference's exact -> prefix -> fuzzy -> skeleton
//! chain with its published constants.

use crate::matcher::normalize;

pub const SHEET_CSV_URL: &str = "https://docs.google.com/spreadsheets/d/16YU8mSS7TdLPdmOunVjiPn_NrKVGfcnMkuMQDy8jgZA/export?format=csv&gid=0";

const FUZZY: f64 = 0.84;
const FUZZY_LEN_TOL: usize = 3;
const PREFIX_MIN: usize = 4;
const SKELETON_MIN_LEN: usize = 8;
const SKELETON_ACCEPT: f64 = 0.72;
const SKELETON_FLOOR: f64 = 0.55;
const SKELETON_MARGIN: f64 = 0.18;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RumourEntry {
    pub rumour: String,
    pub map_type: String,
    pub mods: String,
    pub rating: String,
}

/// Tolerant parse of the community sheet: skips the header, blank
/// separator rows, and section-header rows (a name with no data columns);
/// handles quoted names containing commas.
pub fn parse_csv(csv: &str) -> Vec<RumourEntry> {
    let mut out = Vec::new();
    for (i, line) in csv.lines().enumerate() {
        if i == 0 || line.trim().is_empty() {
            continue;
        }
        let cols = split_csv_line(line);
        if cols.len() < 4 {
            continue;
        }
        let (r, m, mods, rating) = (&cols[0], &cols[1], &cols[2], &cols[3]);
        if r.trim().is_empty() {
            continue;
        }
        // Section headers ("Unique Maps,,,") carry no data columns.
        if m.trim().is_empty() && mods.trim().is_empty() && rating.trim().is_empty() {
            continue;
        }
        out.push(RumourEntry {
            rumour: r.trim().to_string(),
            map_type: m.trim().to_string(),
            mods: mods.trim().to_string(),
            rating: rating.trim().to_string(),
        });
    }
    out
}

fn split_csv_line(line: &str) -> Vec<String> {
    let mut cols = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    for c in line.chars() {
        match c {
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => cols.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    cols.push(cur);
    cols
}

/// Glyph-class collapse for systematically garbled OCR reads (reference
/// constants): w/m/n/u -> n, r/v -> r, i/l/j/t -> i, o/0/e/c -> o, 4/a -> a.
pub fn skeleton(s: &str) -> String {
    s.chars()
        .map(|c| match c.to_ascii_lowercase() {
            'w' | 'm' | 'n' | 'u' => 'n',
            'r' | 'v' => 'r',
            'i' | 'l' | 'j' | 't' => 'i',
            'o' | '0' | 'e' | 'c' => 'o',
            '4' | 'a' => 'a',
            other => other,
        })
        .collect()
}

pub struct RumourIndex {
    entries: Vec<RumourEntry>,
    keys: Vec<String>,
}

impl RumourIndex {
    pub fn new(entries: Vec<RumourEntry>) -> RumourIndex {
        let keys = entries.iter().map(|e| normalize(&e.rumour)).collect();
        RumourIndex { entries, keys }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Reference resolution chain: exact normalized -> prefix (>= 4
    /// chars, shortest key) -> fuzzy (>= 0.84 within +-3 length) ->
    /// skeleton (>= 8 chars: accept >= 0.72, or >= 0.55 with >= 0.18
    /// margin over the runner-up).
    pub fn resolve(&self, ocr_name: &str) -> Option<&RumourEntry> {
        let n = normalize(ocr_name);
        if n.is_empty() {
            return None;
        }
        if let Some(i) = self.keys.iter().position(|k| *k == n) {
            return Some(&self.entries[i]);
        }
        if n.len() >= PREFIX_MIN {
            let mut candidates: Vec<usize> = (0..self.keys.len())
                .filter(|&i| self.keys[i].starts_with(&n))
                .collect();
            candidates.sort_by_key(|&i| self.keys[i].len());
            if let Some(&i) = candidates.first() {
                return Some(&self.entries[i]);
            }
        }
        let mut best: Option<(usize, f64)> = None;
        for (i, k) in self.keys.iter().enumerate() {
            if k.len().abs_diff(n.len()) > FUZZY_LEN_TOL {
                continue;
            }
            let s = strsim::normalized_levenshtein(&n, k);
            if s >= FUZZY && best.is_none_or(|(_, b)| s > b) {
                best = Some((i, s));
            }
        }
        if let Some((i, _)) = best {
            return Some(&self.entries[i]);
        }
        if n.len() >= SKELETON_MIN_LEN {
            let ns = skeleton(&n);
            let mut scored: Vec<(usize, f64)> = self
                .keys
                .iter()
                .enumerate()
                .map(|(i, k)| (i, strsim::normalized_levenshtein(&ns, &skeleton(k))))
                .collect();
            scored.sort_by(|a, b| b.1.total_cmp(&a.1));
            if let Some(&(i, s)) = scored.first() {
                let runner = scored.get(1).map(|&(_, s)| s).unwrap_or(0.0);
                if s >= SKELETON_ACCEPT || (s >= SKELETON_FLOOR && s - runner >= SKELETON_MARGIN) {
                    return Some(&self.entries[i]);
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHEET: &str = include_str!("../tests/fixtures/rumours.csv");

    #[test]
    fn parses_the_live_sheet_snapshot() {
        let entries = parse_csv(SHEET);
        assert!(entries.len() >= 20, "live snapshot has 20+ rows, got {}", entries.len());
        let wild = entries.iter().find(|e| e.rumour.contains("Wild")).expect("quoted-name row");
        assert_eq!(wild.rumour, "Wild,.Roaming Free", "quoted comma preserved");
        assert!(entries.iter().all(|e| !e.rumour.is_empty()));
        assert!(
            !entries.iter().any(|e| e.rumour == "Unique Maps" || e.rumour == "Bosses"),
            "section headers are not entries"
        );
    }

    #[test]
    fn resolves_exact_prefix_fuzzy_and_skeleton() {
        let idx = RumourIndex::new(parse_csv(SHEET));
        assert_eq!(idx.resolve("Fallen Stars").unwrap().rating, "S+");
        // Prefix: truncated OCR read.
        assert_eq!(idx.resolve("Cold").unwrap().map_type, "Frigid Bluffs");
        // Fuzzy: one OCR slip.
        assert_eq!(idx.resolve("Fallen Stbrs").unwrap().rating, "S+");
        // Skeleton: systematic glyph damage (reference example pattern).
        assert_eq!(idx.resolve("Uwkwoww Ruiws").unwrap().map_type, "Exhumed Ruins");
        // Garbage stays unresolved.
        assert!(idx.resolve("xyzzy plugh").is_none());
        assert!(idx.resolve("").is_none());
    }
}
