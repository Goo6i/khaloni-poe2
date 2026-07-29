//! Island Rumour dataset and name matching, ported from the reference
//! overlay's rumour system: a community Google Sheet (CSV export) maps
//! rumour names to map type, mods, and rating. Matching tolerates OCR
//! damage through the reference's exact -> prefix -> fuzzy -> skeleton
//! chain with its published constants.

use crate::matcher::normalize;

pub const SHEET_CSV_URL: &str = "https://docs.google.com/spreadsheets/d/16YU8mSS7TdLPdmOunVjiPn_NrKVGfcnMkuMQDy8jgZA/export?format=csv&gid=0";

/// `match_line` constants, from danielmtv2/poe2-expedition-overlay via the
/// Python spike (`fuzz.ratio` is 0..100). The reference uses 70; this port
/// runs leptess (in-process libtesseract) rather than the subprocess
/// tesseract the spike used, and leptess's noisier line grouping lets pure
/// border-garble graze exactly 70 on real frames. Measured on the 5 real
/// fixtures: genuine rumour lines score >= 84, garble tops out at 73, so 75
/// keeps every true match and rejects the noise (see app/tests/rumours.rs).
const MATCH_THRESHOLD: f64 = 75.0;
const MATCH_MARGIN: f64 = 5.0;
const MATCH_MIN_KEY: usize = 4;

/// Tooltip text that shares the rumour region but is not a rumour; a match
/// is rejected when any of these scores at least as high (chrome guard).
const CHROME: [&str; 6] = [
    "ISLAND RUMOURS",
    "USE A LOGBOOK TO CHART THE AREA",
    "UNCHARTED WATERS",
    "EXPEDITION LOGBOOK",
    "REQUIRES",
    "CONSUMES",
];

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

/// Class-normalized key mirroring the reference `_cnorm`: lowercase, strip
/// every non-alphanumeric character (so spacing/punctuation are irrelevant),
/// then collapse the glyph classes OCR routinely confuses so a garbled read
/// lands on top of the truth. Distinct from `skeleton` (a different, coarser
/// class map used by `resolve`); this one matches the danielmtv2 `_CLASS`.
pub fn cnorm(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| match c.to_ascii_lowercase() {
            'v' | 'w' => 'u',
            'm' | 'r' => 'n',
            'i' | 'j' | 't' | '1' => 'l',
            '0' | 'e' | 'c' => 'o',
            '5' => 's',
            '8' => 'b',
            other => other,
        })
        .collect()
}

/// `fuzz.ratio`-equivalent similarity (0..100): the normalized indel
/// (insertion/deletion only) similarity, `200 * LCS / (len_a + len_b)`.
fn indel_ratio(a: &str, b: &str) -> f64 {
    let ab: Vec<char> = a.chars().collect();
    let bb: Vec<char> = b.chars().collect();
    let total = ab.len() + bb.len();
    if total == 0 {
        return 100.0;
    }
    // LCS length via rolling DP.
    let mut prev = vec![0usize; bb.len() + 1];
    let mut cur = vec![0usize; bb.len() + 1];
    for &ca in &ab {
        for (j, &cb) in bb.iter().enumerate() {
            cur[j + 1] = if ca == cb {
                prev[j] + 1
            } else {
                prev[j + 1].max(cur[j])
            };
        }
        std::mem::swap(&mut prev, &mut cur);
        cur.iter_mut().for_each(|v| *v = 0);
    }
    let lcs = prev[bb.len()];
    200.0 * lcs as f64 / total as f64
}

pub struct RumourIndex {
    entries: Vec<RumourEntry>,
    keys: Vec<String>,
    cnorm_keys: Vec<String>,
}

impl RumourIndex {
    pub fn new(entries: Vec<RumourEntry>) -> RumourIndex {
        let keys = entries.iter().map(|e| normalize(&e.rumour)).collect();
        let cnorm_keys = entries.iter().map(|e| cnorm(&e.rumour)).collect();
        RumourIndex { entries, keys, cnorm_keys }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Match one OCR line to a rumour with the proven danielmtv2 method
    /// (verified 8/10 recall, 0 false positives on 5 real 4K frames in the
    /// Python spike): class-normalize the text, `fuzz.ratio` against the
    /// class-normalized names, accept only above `MATCH_THRESHOLD` and by
    /// more than `MATCH_MARGIN` over the runner-up, and reject anything a
    /// tooltip-chrome phrase matches at least as well. Distinct from
    /// `resolve` (used by reward-panel pricing); do not conflate them.
    pub fn match_line(&self, text: &str) -> Option<&RumourEntry> {
        let key = cnorm(text);
        if key.chars().count() < MATCH_MIN_KEY {
            return None;
        }
        // Best and runner-up rumour by fuzz.ratio over class-normalized keys.
        let mut best: Option<(usize, f64)> = None;
        let mut runner = 0.0;
        for (i, k) in self.cnorm_keys.iter().enumerate() {
            let s = indel_ratio(&key, k);
            match best {
                Some((_, b)) if s > b => {
                    runner = b;
                    best = Some((i, s));
                }
                Some((_, b)) if s > runner && s <= b => runner = s,
                None => best = Some((i, s)),
                _ => {}
            }
        }
        let (idx, score) = best?;
        if score < MATCH_THRESHOLD || score - runner < MATCH_MARGIN {
            return None;
        }
        // Chrome guard: reject if any non-rumour tooltip phrase fits as well.
        let chrome_best = CHROME
            .iter()
            .map(|c| indel_ratio(&key, &cnorm(c)))
            .fold(0.0, f64::max);
        if chrome_best >= score {
            return None;
        }
        Some(&self.entries[idx])
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
    fn match_line_exact_and_glyph_tolerant() {
        let idx = RumourIndex::new(parse_csv(SHEET));
        assert_eq!(idx.match_line("Endless Cliffs").unwrap().rumour, "Endless Cliffs");
        // e/c/o collapse to one glyph class: "ico" reads as "ice".
        assert_eq!(idx.match_line("Cold as ico").unwrap().rumour, "Cold as ice");
        // Punctuation in the sheet name is stripped; spacing is irrelevant.
        assert_eq!(
            idx.match_line("Wild Roaming Free").unwrap().rumour,
            "Wild,.Roaming Free"
        );
    }

    #[test]
    fn match_line_rejects_chrome_short_and_garbage() {
        let idx = RumourIndex::new(parse_csv(SHEET));
        // Tooltip chrome must never produce a phantom rumour.
        assert!(idx.match_line("REQUIRES").is_none(), "chrome word");
        assert!(idx.match_line("UNCHARTED WATERS").is_none(), "title chrome");
        // Too short to be a rumour name.
        assert!(idx.match_line("abc").is_none());
        // Nothing close enough.
        assert!(idx.match_line("xyzzy plugh").is_none());
    }

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
