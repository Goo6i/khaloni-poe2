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
/// The substring tier accepts a vocabulary entry found verbatim inside a
/// noisy line only when the entry is at least this long, or covers at
/// least SUBSTRING_MIN_COVER_TENTHS tenths of the line. A short name such
/// as "iron rune" or "ox idol" inside a garbled line is far more often
/// noise around a coincidence than a reward.
const SUBSTRING_MIN_LEN: usize = 8;
const SUBSTRING_MIN_COVER_TENTHS: usize = 6;
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
    /// The line carried a count token whose value did not survive OCR
    /// ("0x"): `count` is None, but unlike a row that simply has no count
    /// this one is a stack of unknown size, and pricing it as one unit
    /// would be a guess.
    pub count_unreadable: bool,
    pub tier: MatchTier,
}

/// Longest count token accepted, in characters before the 'x'. Reward
/// stacks never reach five digits; anything longer is a word that happens
/// to end in 'x', or digits tesseract ran together.
const COUNT_MAX_DIGITS: usize = 4;

/// Reads a stack-count token ("3x", "10x"). Tesseract regularly returns
/// the digits 1 and 0 as the letters l/i and o in this font ("lx", "l0x",
/// "lox", "1ox"), so those three letters are folded back to digits - but
/// only here, inside a token that already has the count shape, never in
/// item names. `None` for anything that is not a count, and for a token
/// that folds to zero: the panel never shows "0x", so such a token is
/// either a real word ("ox") or a misread whose value is unknown.
pub fn parse_count_token(word: &str) -> Option<u32> {
    let digits = word.strip_suffix('x')?;
    if digits.is_empty() || digits.len() > COUNT_MAX_DIGITS {
        return None;
    }
    let folded: String = digits
        .chars()
        .map(|c| match c {
            'l' | 'i' => Some('1'),
            'o' => Some('0'),
            d if d.is_ascii_digit() => Some(d),
            _ => None,
        })
        .collect::<Option<String>>()?;
    folded.parse::<u32>().ok().filter(|&n| n > 0)
}

/// True for an all-digit count token reading zero ("0x"): a count was
/// printed there but its value did not survive OCR.
fn is_zero_count(word: &str) -> bool {
    word.strip_suffix('x').is_some_and(|d| {
        !d.is_empty() && d.len() <= COUNT_MAX_DIGITS && d.chars().all(|c| c == '0')
    })
}

/// Extracts a leading or embedded "Nx " count token from a normalized line and
/// returns (count, line with the token removed, whether a count was
/// printed but unreadable). An unreadable "0x" is removed from the name as
/// well but yields no count.
fn extract_count(line_norm: &str) -> (Option<u32>, String, bool) {
    let without = |skip: usize| -> String {
        line_norm
            .split_whitespace()
            .enumerate()
            .filter(|(j, _)| *j != skip)
            .map(|(_, w)| w)
            .collect::<Vec<_>>()
            .join(" ")
    };
    for (i, word) in line_norm.split_whitespace().enumerate() {
        if let Some(c) = parse_count_token(word) {
            return (Some(c), without(i), false);
        }
        if is_zero_count(word) {
            return (None, without(i), true);
        }
    }
    (None, line_norm.to_string(), false)
}

/// A word beside a substring hit that resembles a variant prefix at least
/// this much is evidence the row names the variant, not the plain entry
/// ("pertect exalted orb" contains "exalted orb" verbatim, and a Perfect
/// orb priced as the plain one is wrong by an order of magnitude). Junk
/// beside a hit - icon glyphs, a stray letter - scores far below this
/// against "perfect"/"greater"/"lesser".
const VARIANT_EVIDENCE: f64 = 0.5;
/// The resembling word must reach this, and beat every sibling variant by
/// VARIANT_MARGIN, before the variant itself is reported; between the two
/// bars the row is Ambiguous.
const VARIANT_RESOLVE: f64 = 0.7;
const VARIANT_MARGIN: f64 = 0.15;

/// What the words around a substring hit say about the variant family the
/// hit belongs to.
enum VariantCheck {
    /// Nothing beside the hit resembles a variant name: the hit stands.
    Clear,
    /// The words beside the hit name this longer entry; the score is the
    /// whole-name similarity, for tiering.
    Resolved(usize, f64),
    /// The words beside the hit resemble a variant, but not well enough to
    /// say which: the top candidate, for diagnostics only.
    Unclear(usize),
}

fn last_words(text: &str, n: usize) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    words[words.len().saturating_sub(n)..].join(" ")
}

fn first_words(text: &str, n: usize) -> String {
    text.split_whitespace().take(n).collect::<Vec<_>>().join(" ")
}

/// Checks a substring hit against every longer vocabulary entry that is
/// the hit plus leading or trailing words ("perfect exalted orb",
/// "greater chaos orb"). Had the line carried such a name verbatim the
/// longest-entry rule would already have chosen it; this catches the
/// garbled case, where only the plain name survived OCR intact.
fn variant_check(vocab: &Vocab, name_part: &str, hit: usize) -> VariantCheck {
    let entry = vocab.normalized[hit].as_str();
    let Some(pos) = name_part.find(entry) else {
        return VariantCheck::Clear;
    };
    let before = &name_part[..pos];
    let after = &name_part[pos + entry.len()..];

    // (entry index, affix similarity, whole-name similarity)
    let mut best: Option<(usize, f64, f64)> = None;
    let mut runner_up = 0f64;
    for (i, longer) in vocab.normalized.iter().enumerate() {
        if i == hit || longer.len() <= entry.len() + 1 {
            continue;
        }
        let (affix, seen, whole) = if let Some(prefix) =
            longer.strip_suffix(entry).and_then(|p| p.strip_suffix(' '))
        {
            let seen = last_words(before, prefix.split_whitespace().count());
            let whole = format!("{seen} {entry}");
            (prefix, seen, whole)
        } else if let Some(suffix) = longer.strip_prefix(entry).and_then(|s| s.strip_prefix(' ')) {
            let seen = first_words(after, suffix.split_whitespace().count());
            let whole = format!("{entry} {seen}");
            (suffix, seen, whole)
        } else {
            continue;
        };
        if seen.is_empty() {
            continue;
        }
        let sim = normalized_levenshtein(&seen, affix);
        match best {
            Some((_, b, _)) if sim <= b => runner_up = runner_up.max(sim),
            _ => {
                if let Some((_, b, _)) = best {
                    runner_up = runner_up.max(b);
                }
                best = Some((i, sim, normalized_levenshtein(&whole, longer)));
            }
        }
    }
    match best {
        Some((i, sim, whole)) if sim >= VARIANT_RESOLVE && sim - runner_up >= VARIANT_MARGIN => {
            VariantCheck::Resolved(i, whole)
        }
        Some((i, sim, _)) if sim >= VARIANT_EVIDENCE => VariantCheck::Unclear(i),
        _ => VariantCheck::Clear,
    }
}

fn tier_rank(tier: MatchTier) -> u8 {
    match tier {
        MatchTier::Exact => 0,
        MatchTier::Substring => 1,
        MatchTier::Prefix => 2,
        MatchTier::HighConfidence => 3,
        MatchTier::Fuzzy => 4,
        MatchTier::Ambiguous => 5,
    }
}

/// Tiered matching, one hit per input line at most, checked in this order:
/// 1. Exact: the count-stripped query equals a vocabulary entry verbatim
///    (directly, or after folding OCR digit-look-alikes back to letters).
/// 2. Substring: a vocabulary entry contained verbatim in the UNFILTERED
///    line; the longest matching entry wins - unless the words beside it
///    look like the garbled prefix of a longer variant of the same name
///    (`variant_check`), in which case the plain entry is struck for the
///    row and the variant, or Ambiguous, is reported instead.
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
/// The count always comes from the same line that produced the name match;
/// when the two reads of one row agree on the entry and only one kept the
/// count word, they merge into one counted hit.
///
/// Hits come back most confident tier first, counted before uncounted,
/// then by name, so the first hit is the one to price and does not depend
/// on the order of the vocabulary.
pub fn match_rows(vocab: &Vocab, filtered: &[String], unfiltered: &[String]) -> Vec<RowHit> {
    // A hit tagged with the index of the line that produced it. When the
    // two slices pair up (the pricing path passes one row's filtered and
    // unfiltered text), filtered[i] and unfiltered[i] are the same row and
    // their evidence is combined; otherwise every line stands alone under
    // one shared row number.
    let paired = filtered.len() == unfiltered.len();
    let row_of = |i: usize| if paired { i } else { 0 };
    let mut hits: Vec<(usize, RowHit)> = Vec::new();
    // (row, entry) pairs whose plain-name hit lost to variant evidence.
    let mut vetoed: Vec<(usize, usize)> = Vec::new();

    let mut consider = |row: usize, line: &str, allow_fuzzy: bool| {
        let norm = normalize(line);
        if norm.is_empty() {
            return;
        }
        // A whole line that is a vocabulary entry as it stands is that
        // entry, even if one of its words has the shape of a count ("ox").
        if let Some(entry_index) = vocab.normalized.iter().position(|e| *e == norm) {
            let hit = RowHit { entry_index, count: None, count_unreadable: false, tier: MatchTier::Exact };
            hits.push((row, hit));
            return;
        }
        let (count, name_part, count_unreadable) = extract_count(&norm);

        // Exact tier: the count-stripped query equals a vocab entry
        // verbatim. Cheap and safe to try on both the noisy unfiltered line
        // and the confidence-filtered one, since equality (unlike substring
        // or fuzzy) can't be fooled by extra surrounding garbage. If plain
        // equality misses and the query contains a digit, retry once after
        // folding OCR digit-look-alikes (0/1/5/8) back to letters.
        if let Some(entry_index) = vocab.normalized.iter().position(|e| *e == name_part) {
            hits.push((row, RowHit { entry_index, count, count_unreadable, tier: MatchTier::Exact }));
            return;
        }
        if name_part.chars().any(|c| c.is_ascii_digit()) {
            let folded = digit_fold(&name_part);
            if let Some(entry_index) = vocab.normalized.iter().position(|e| *e == folded) {
                hits.push((row, RowHit { entry_index, count, count_unreadable, tier: MatchTier::Exact }));
                return;
            }
        }

        // Substring tier: every vocab entry contained verbatim in the
        // unfiltered line is a candidate; the longest (most specific) one
        // wins, e.g. "perfect jewellers orb" over "jewellers orb". Short
        // entries qualify only when they make up most of the line. Ties in
        // length go to the alphabetically first entry so the pick does not
        // depend on vocabulary order.
        let mut substring_best: Option<usize> = None;
        for (i, entry) in vocab.normalized.iter().enumerate() {
            if entry.is_empty() {
                continue;
            }
            let long_enough = entry.len() >= SUBSTRING_MIN_LEN
                || entry.len() * 10 >= norm.len() * SUBSTRING_MIN_COVER_TENTHS;
            if long_enough && norm.contains(entry.as_str()) {
                let better = substring_best.is_none_or(|b| {
                    let cur = &vocab.normalized[b];
                    entry.len() > cur.len() || (entry.len() == cur.len() && entry < cur)
                });
                if better {
                    substring_best = Some(i);
                }
            }
        }
        if let Some(entry_index) = substring_best {
            // The plain name read verbatim is not the last word when the
            // text beside it looks like a garbled variant prefix: see
            // variant_check. The plain entry is then struck for this row,
            // whichever line or tier proposed it (the confidence filter
            // often drops exactly the garbled word, leaving the filtered
            // line an Exact match for the wrong item).
            match variant_check(vocab, &name_part, entry_index) {
                VariantCheck::Clear => {
                    hits.push((row, RowHit { entry_index, count, count_unreadable, tier: MatchTier::Substring }));
                }
                VariantCheck::Resolved(variant, score) => {
                    vetoed.push((row, entry_index));
                    let tier = if score >= HIGH_CONFIDENCE_THRESHOLD {
                        MatchTier::HighConfidence
                    } else {
                        MatchTier::Fuzzy
                    };
                    hits.push((row, RowHit { entry_index: variant, count, count_unreadable, tier }));
                }
                VariantCheck::Unclear(variant) => {
                    vetoed.push((row, entry_index));
                    hits.push((row, RowHit { entry_index: variant, count, count_unreadable, tier: MatchTier::Ambiguous }));
                }
            }
            return;
        }

        if !allow_fuzzy {
            return;
        }

        // Prefix tier: the query is long enough to be meaningful on its own
        // (>= PREFIX_MIN_LEN) and is a prefix of a vocab entry, or a vocab
        // entry is a prefix of it (e.g. a clipped or over-read panel line).
        // Ties go to the shortest qualifying entry, since it's the closest
        // match to the query's own length, then alphabetically.
        if name_part.len() >= PREFIX_MIN_LEN {
            let mut prefix_best: Option<usize> = None;
            for (i, entry) in vocab.normalized.iter().enumerate() {
                if entry.is_empty() || entry == &name_part {
                    continue;
                }
                if entry.starts_with(name_part.as_str()) || name_part.starts_with(entry.as_str()) {
                    let better = prefix_best.is_none_or(|b| {
                        let cur = &vocab.normalized[b];
                        entry.len() < cur.len() || (entry.len() == cur.len() && entry < cur)
                    });
                    if better {
                        prefix_best = Some(i);
                    }
                }
            }
            if let Some(entry_index) = prefix_best {
                hits.push((row, RowHit { entry_index, count, count_unreadable, tier: MatchTier::Prefix }));
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
                Some((b, best_score))
                    if ratio > best_score
                        || (ratio == best_score && *entry < vocab.normalized[b]) =>
                {
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
            hits.push((row, RowHit { entry_index, count, count_unreadable, tier }));
        }
    };

    for (i, line) in unfiltered.iter().enumerate() {
        consider(row_of(i), line, false);
    }
    for (i, line) in filtered.iter().enumerate() {
        consider(row_of(i), line, true);
    }

    hits.retain(|(row, h)| !vetoed.contains(&(*row, h.entry_index)));

    // One read of a row naming "jewellers orb" and the other "perfect
    // jewellers orb" do not disagree: the confidence filter dropped a word,
    // and the read that kept it names the item. A verbatim longer name
    // retires the shorter one it contains.
    let verbatim: Vec<(usize, usize)> = hits
        .iter()
        .filter(|(_, h)| matches!(h.tier, MatchTier::Exact | MatchTier::Substring))
        .map(|(row, h)| (*row, h.entry_index))
        .collect();
    hits.retain(|(row, h)| {
        let short = &vocab.normalized[h.entry_index];
        !verbatim.iter().any(|&(r, longer)| {
            let long = &vocab.normalized[longer];
            r == *row && long.len() > short.len() && long.contains(short.as_str())
        })
    });

    // The confidence filter drops the count word as readily as any other,
    // so one row often yields the same entry twice: once counted, once not.
    // The uncounted hit then says nothing the counted one does not, except
    // possibly a better tier, which the counted hit inherits.
    let uncounted: Vec<(usize, usize, MatchTier)> = hits
        .iter()
        .filter(|(_, h)| h.count.is_none())
        .map(|(row, h)| (*row, h.entry_index, h.tier))
        .collect();
    for (row, entry, tier) in uncounted {
        let mut absorbed = false;
        for (r, h) in hits.iter_mut() {
            if *r == row && h.entry_index == entry && h.count.is_some() {
                if tier_rank(tier) < tier_rank(h.tier) {
                    h.tier = tier;
                }
                absorbed = true;
            }
        }
        if absorbed {
            hits.retain(|(r, h)| !(*r == row && h.entry_index == entry && h.count.is_none()));
        }
    }

    // A count one read could not make out, the other may have: only when
    // no read of the row produced a number does the stack stay unsized.
    let sized: Vec<(usize, usize)> =
        hits.iter().filter(|(_, h)| h.count.is_some()).map(|(row, h)| (*row, h.entry_index)).collect();
    let unsized_rows: Vec<(usize, usize)> = hits
        .iter()
        .filter(|(_, h)| h.count_unreadable)
        .map(|(row, h)| (*row, h.entry_index))
        .filter(|key| !sized.contains(key))
        .collect();
    for (row, h) in hits.iter_mut() {
        h.count_unreadable = unsized_rows.contains(&(*row, h.entry_index));
    }

    // Most confident first, a counted hit ahead of an uncounted one, then
    // by name: callers price the first hit, and the vocabulary's own order
    // (built from a hash map) must not decide which that is.
    let mut hits: Vec<RowHit> = hits.into_iter().map(|(_, h)| h).collect();
    hits.sort_by(|a, b| {
        tier_rank(a.tier)
            .cmp(&tier_rank(b.tier))
            .then(a.count.is_none().cmp(&b.count.is_none()))
            .then_with(|| vocab.normalized[a.entry_index].cmp(&vocab.normalized[b.entry_index]))
            .then(a.entry_index.cmp(&b.entry_index))
            .then(a.count.cmp(&b.count))
    });
    let mut seen: Vec<(usize, Option<u32>)> = Vec::new();
    hits.retain(|h| {
        let key = (h.entry_index, h.count);
        if seen.contains(&key) {
            false
        } else {
            seen.push(key);
            true
        }
    });
    hits
}
