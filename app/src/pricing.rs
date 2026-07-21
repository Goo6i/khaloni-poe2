use poe2_lens_core::matcher::{match_rows, normalize, MatchTier, Vocab};
use poe2_lens_core::ninja::{Price, PriceTable};
use poe2_lens_core::value::{display_price, format_amount, UNKNOWN};

use crate::config::Config;
use crate::ocr::OcrLine;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Junk,
    Decent,
    Jackpot,
    Unknown,
}

/// Which currency icon a priced row's amount is denominated in. `None` means
/// no icon (the "?" unpriced rows).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denom {
    Divine,
    Exalted,
    Chaos,
    None,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Priced {
    pub y_top: u32,
    pub height: u32,
    /// Full "N ex" / "N div (each)" text, kept for headless/debug output.
    pub label: String,
    /// Same number as `label`, minus the "div"/"ex" suffix; the renderer
    /// draws this next to the currency icon instead of the suffix.
    pub amount: String,
    pub denom: Denom,
    pub tier: Tier,
    /// Stable identity for "is this the same catalog item as last pass",
    /// independent of count/amount: the normalized vocab entry name for a
    /// matched row, a gem-type tag for skill/support rows, or a fixed tag
    /// for the "?" cases. Used by `stabilize::Stabilizer`'s slot model to
    /// decide a same-item bump vs a different-item pending switch.
    pub item_key: String,
    /// Stack count this row was priced at (1 when uncounted).
    pub count: u32,
    /// True when this pass's OCR line carried an explicit "Nx" count token;
    /// false when the count was implied (defaulted to 1) or this row has no
    /// count concept at all (gem rows, "?" rows). Drives the stabilizer's
    /// stack-count stickiness.
    pub count_explicit: bool,
    /// True when the match tier that produced this row is confident enough
    /// to lock a display slot after a single read (Exact/Substring/Prefix/
    /// HighConfidence vocab matches, or any of the deterministic gem/
    /// ambiguous/unpriceable classifications, none of which are a per-frame
    /// guess). False only for a plain Fuzzy vocab match, which the
    /// stabilizer must see twice in a row, identically, before displaying.
    pub locks_in_one: bool,
}

/// Mirrors `display_price`'s divine-vs-exalted choice and formatting, but
/// returns the amount without a trailing "div"/"ex" word since the renderer
/// shows that as an icon instead. `pub(crate)` so `hover.rs` can reuse it
/// for the price-check popup instead of duplicating the divine/exalted
/// choice.
pub(crate) fn denom_amount(price: &Price, count: u32, divine_threshold: f64) -> (Denom, String) {
    let count = count.max(1);
    let total_divine = price.divine * f64::from(count);
    let total_exalted = price.exalted * f64::from(count);
    if total_divine >= divine_threshold {
        let amount = if count == 1 {
            format_amount(total_divine)
        } else {
            format!("{} ({} each)", format_amount(total_divine), format_amount(price.divine))
        };
        (Denom::Divine, amount)
    } else {
        let amount = if count == 1 {
            format_amount(total_exalted)
        } else {
            format!("{} ({} each)", format_amount(total_exalted), format_amount(price.exalted))
        };
        (Denom::Exalted, amount)
    }
}

pub fn build_vocab(table: &PriceTable) -> Vocab {
    Vocab::new(table.names().map(str::to_string).collect())
}

/// Rows reading "skill level N ..." map to the poe.ninja uncut skill gem for
/// that exact level (verified live: ids uncut-skill-gem-N, names
/// "Uncut Skill Gem (Level N)"). Support/spirit rows carry no level on the
/// panel, so they price as UNKNOWN rather than a guess.
fn gem_row(unfiltered: &str) -> Option<GemRow> {
    // Band crops can carry leading icon-glyph junk inside the same OCR
    // line ("booa vl skill level 20 skyfall"), so the markers are matched
    // anywhere in the line, not as prefixes. Word-boundary safety comes
    // from splitting on whitespace below.
    let words: Vec<&str> = unfiltered.split_whitespace().collect();
    for w in words.windows(3) {
        if w[0] == "skill" && w[1] == "level" {
            if let Ok(level) = w[2].parse::<u32>() {
                return Some(GemRow::Skill(level));
            }
        }
    }
    if words.iter().any(|&w| w == "support" || w == "spirit") {
        return Some(GemRow::Unleveled);
    }
    None
}

enum GemRow {
    Skill(u32),
    Unleveled,
}

fn tier_for(total_ex: f64, cfg: &Config) -> Tier {
    if total_ex >= cfg.tier_good_ex {
        Tier::Jackpot
    } else if total_ex >= cfg.tier_decent_ex {
        Tier::Decent
    } else {
        Tier::Junk
    }
}

/// True when `word` looks like a stack-count token ("Nx"): a short digit
/// run followed by 'x'. Measured against the real panel_choice fixture
/// (see gem_row's doc comment for the matching band-crop-junk issue): the
/// band crop for "1x Cyclonic Alloy" OCRs at BAND_OCR_SCALE=3x/psm 6 as
/// "e lx cyclonic alloy" - an icon-glyph word ("e") ahead of the count,
/// and the leading "1" itself misread as lowercase "l" at 24% confidence
/// (the exact "1"/"I" glyph ambiguity already documented on
/// BAND_OCR_SCALE, just resurfacing at psm 6/3x instead of psm 4/3x since
/// the whitelist that used to force it toward a digit is gone). 'l'/'i'
/// are accepted alongside digits for this reason; the run is also capped
/// at 3 chars so this can't accidentally match an unrelated word ending
/// in "x".
fn is_count_token(word: &str) -> bool {
    let Some(prefix) = word.strip_suffix('x') else {
        return false;
    };
    !prefix.is_empty()
        && prefix.len() <= 3
        && prefix.chars().all(|c| c.is_ascii_digit() || c == 'l' || c == 'i')
}

/// A row's normalized text carries an "Nx " count token somewhere in the
/// line, e.g. a currency stack ("3x chaos orb") that failed to match the
/// vocab because the item name itself was mangled by OCR. Not anchored to
/// the first word: band crops can carry leading icon-glyph junk ahead of
/// the count (see `is_count_token`'s doc comment), the same
/// anywhere-in-line reasoning as `gem_row`'s marker search.
fn has_leading_count(normalized: &str) -> bool {
    normalized.split_whitespace().any(is_count_token)
}

/// A line that never matched the vocab is still worth a "?" row, rather
/// than silent disappearance, when it looks like a real panel entry: a
/// counted stack, or a unique item (uniques are never in the currency
/// vocab, so they always miss the match).
fn is_unpriceable_but_present(line: &OcrLine) -> bool {
    has_leading_count(&line.unfiltered) || line.unfiltered.contains("unique")
}

/// Prices a template-identified band without any OCR text: the learned
/// key reproduces the same Priced a full OCR+match pass would have built.
/// Returns None only when the key no longer resolves against the current
/// price table (league change, catalog drift); the caller then falls back
/// to OCR so the template gets re-learned.
pub fn price_resolved(
    table: &PriceTable,
    item_key: &str,
    count: u32,
    count_explicit: bool,
    y_top: u32,
    height: u32,
    cfg: &Config,
) -> Option<Priced> {
    let lookup_name = item_key
        .strip_prefix("gem-skill-")
        .map(|lvl| format!("Uncut Skill Gem (Level {lvl})"))
        .unwrap_or_else(|| item_key.to_string());
    let price = table.lookup(&lookup_name)?;
    let (denom, amount) = denom_amount(price, count, cfg.divine_threshold);
    Some(Priced {
        y_top,
        height,
        label: display_price(price, count, cfg.divine_threshold),
        amount,
        denom,
        tier: tier_for(price.exalted * f64::from(count.max(1)), cfg),
        item_key: item_key.to_string(),
        count,
        count_explicit,
        locks_in_one: true,
    })
}

pub fn price_lines(
    table: &PriceTable,
    vocab: &Vocab,
    lines: &[OcrLine],
    cfg: &Config,
) -> (Vec<Priced>, String) {
    let mut rows = Vec::new();

    for line in lines {
        // Gem rows first: they never match the vocab (panel text is not a catalog name).
        if let Some(g) = gem_row(&line.unfiltered) {
            let (label, tier, denom, amount, item_key) = match g {
                GemRow::Skill(level) => {
                    let name = format!("Uncut Skill Gem (Level {level})");
                    let item_key = format!("gem-skill-{level}");
                    match table.lookup(&name) {
                        Some(p) => {
                            let (denom, amount) = denom_amount(p, 1, cfg.divine_threshold);
                            (
                                display_price(p, 1, cfg.divine_threshold),
                                tier_for(p.exalted, cfg),
                                denom,
                                amount,
                                item_key,
                            )
                        }
                        None => (UNKNOWN.to_string(), Tier::Unknown, Denom::None, UNKNOWN.to_string(), item_key),
                    }
                }
                GemRow::Unleveled => (
                    UNKNOWN.to_string(),
                    Tier::Unknown,
                    Denom::None,
                    UNKNOWN.to_string(),
                    "gem-unleveled".to_string(),
                ),
            };
            rows.push(Priced {
                y_top: line.y_top,
                height: line.height,
                label,
                amount,
                denom,
                tier,
                item_key,
                // Skill/support/spirit rows never carry a count on the panel.
                count: 1,
                count_explicit: false,
                // A deterministic type+level (or unleveled) pin, never a
                // per-frame fuzzy guess: locks a display slot on read 1.
                locks_in_one: true,
            });
            continue;
        }

        // Vocabulary rows: per-line call keeps the hit tied to this line's geometry.
        let hits = match_rows(
            vocab,
            std::slice::from_ref(&line.filtered),
            std::slice::from_ref(&line.unfiltered),
        );
        let Some(hit) = hits.first() else {
            if is_unpriceable_but_present(line) {
                rows.push(Priced {
                    y_top: line.y_top,
                    height: line.height,
                    label: UNKNOWN.to_string(),
                    amount: UNKNOWN.to_string(),
                    denom: Denom::None,
                    tier: Tier::Unknown,
                    item_key: "unpriceable".to_string(),
                    count: 1,
                    count_explicit: false,
                    locks_in_one: true,
                });
            }
            continue;
        };
        // Two or more near-identical vocab entries scored too close to call:
        // showing either name would risk a wrong price, so this renders as
        // "?" same as an unmatched line, never a guessed variant.
        if hit.tier == MatchTier::Ambiguous {
            rows.push(Priced {
                y_top: line.y_top,
                height: line.height,
                label: UNKNOWN.to_string(),
                amount: UNKNOWN.to_string(),
                denom: Denom::None,
                tier: Tier::Unknown,
                item_key: "ambiguous".to_string(),
                count: 1,
                count_explicit: false,
                locks_in_one: true,
            });
            continue;
        }
        let name = vocab.entry(hit.entry_index);
        let Some(price) = table.lookup(name) else {
            continue;
        };
        let count = hit.count.unwrap_or(1);
        let (denom, amount) = denom_amount(price, count, cfg.divine_threshold);
        rows.push(Priced {
            y_top: line.y_top,
            height: line.height,
            label: display_price(price, count, cfg.divine_threshold),
            amount,
            denom,
            tier: tier_for(price.exalted * f64::from(count), cfg),
            item_key: normalize(name),
            count: hit.count.unwrap_or(1),
            count_explicit: hit.count.is_some(),
            locks_in_one: hit.tier.locks_in_one(),
        });
    }

    // The panel is a pick-one choice; a summed total across unrelated rows
    // is misleading, so this is always empty. The String stays in the
    // signature to keep draw_frame's API (and every call site) unchanged.
    (rows, String::new())
}
