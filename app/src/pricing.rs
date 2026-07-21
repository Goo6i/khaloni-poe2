use poe2_lens_core::matcher::{match_rows, MatchTier, Vocab};
use poe2_lens_core::ninja::PriceTable;
use poe2_lens_core::value::{display_price, UNKNOWN};

use crate::config::Config;
use crate::ocr::OcrLine;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Junk,
    Decent,
    Jackpot,
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Priced {
    pub y_top: u32,
    pub height: u32,
    pub label: String,
    pub tier: Tier,
}

pub fn build_vocab(table: &PriceTable) -> Vocab {
    Vocab::new(table.names().map(str::to_string).collect())
}

/// Rows reading "skill level N ..." map to the poe.ninja uncut skill gem for
/// that exact level (verified live: ids uncut-skill-gem-N, names
/// "Uncut Skill Gem (Level N)"). Support/spirit rows carry no level on the
/// panel, so they price as UNKNOWN rather than a guess.
fn gem_row(unfiltered: &str) -> Option<GemRow> {
    let l = unfiltered.trim();
    if let Some(rest) = l.strip_prefix("skill level ") {
        let level: u32 = rest.split_whitespace().next()?.parse().ok()?;
        return Some(GemRow::Skill(level));
    }
    if l.starts_with("support") || l.starts_with("spirit") {
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

/// A row's normalized text carries a leading "Nx " count token, e.g. a
/// currency stack ("3x chaos orb") that failed to match the vocab because
/// the item name itself was mangled by OCR.
fn has_leading_count(normalized: &str) -> bool {
    normalized
        .split_whitespace()
        .next()
        .map(|w| {
            let digits = w.strip_suffix('x').unwrap_or("");
            !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
        })
        .unwrap_or(false)
}

/// A line that never matched the vocab is still worth a "?" row, rather
/// than silent disappearance, when it looks like a real panel entry: a
/// counted stack, or a unique item (uniques are never in the currency
/// vocab, so they always miss the match).
fn is_unpriceable_but_present(line: &OcrLine) -> bool {
    has_leading_count(&line.unfiltered) || line.unfiltered.contains("unique")
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
            let (label, tier) = match g {
                GemRow::Skill(level) => {
                    let name = format!("Uncut Skill Gem (Level {level})");
                    match table.lookup(&name) {
                        Some(p) => (
                            display_price(p, 1, cfg.divine_threshold),
                            tier_for(p.exalted, cfg),
                        ),
                        None => (UNKNOWN.to_string(), Tier::Unknown),
                    }
                }
                GemRow::Unleveled => (UNKNOWN.to_string(), Tier::Unknown),
            };
            rows.push(Priced {
                y_top: line.y_top,
                height: line.height,
                label,
                tier,
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
                    tier: Tier::Unknown,
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
                tier: Tier::Unknown,
            });
            continue;
        }
        let name = vocab.entry(hit.entry_index);
        let Some(price) = table.lookup(name) else {
            continue;
        };
        let count = hit.count.unwrap_or(1);
        rows.push(Priced {
            y_top: line.y_top,
            height: line.height,
            label: display_price(price, count, cfg.divine_threshold),
            tier: tier_for(price.exalted * f64::from(count), cfg),
        });
    }

    // The panel is a pick-one choice; a summed total across unrelated rows
    // is misleading, so this is always empty. The String stays in the
    // signature to keep draw_frame's API (and every call site) unchanged.
    (rows, String::new())
}
