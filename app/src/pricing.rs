use poe2_lens_core::matcher::{match_rows, Vocab};
use poe2_lens_core::ninja::PriceTable;
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

pub fn price_lines(
    table: &PriceTable,
    vocab: &Vocab,
    lines: &[OcrLine],
    cfg: &Config,
) -> (Vec<Priced>, String) {
    let mut rows = Vec::new();
    let mut total_ex = 0.0f64;
    let mut priced_count = 0u32;

    for line in lines {
        // Gem rows first: they never match the vocab (panel text is not a catalog name).
        if let Some(g) = gem_row(&line.unfiltered) {
            let (label, tier) = match g {
                GemRow::Skill(level) => {
                    let name = format!("Uncut Skill Gem (Level {level})");
                    match table.lookup(&name) {
                        Some(p) => {
                            total_ex += p.exalted;
                            priced_count += 1;
                            (
                                display_price(p, 1, cfg.divine_threshold),
                                tier_for(p.exalted, cfg),
                            )
                        }
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
        let Some(hit) = hits.first() else { continue };
        let name = vocab.entry(hit.entry_index);
        let Some(price) = table.lookup(name) else {
            continue;
        };
        let count = hit.count.unwrap_or(1);
        total_ex += price.exalted * f64::from(count);
        priced_count += 1;
        rows.push(Priced {
            y_top: line.y_top,
            height: line.height,
            label: display_price(price, count, cfg.divine_threshold),
            tier: tier_for(price.exalted * f64::from(count), cfg),
        });
    }

    let total = if priced_count == 0 {
        String::new()
    } else if total_ex / table.exalted_per_divine.max(f64::MIN_POSITIVE) >= cfg.divine_threshold
        && table.exalted_per_divine > 0.0
    {
        format!(
            "Total: {} div",
            format_amount(total_ex / table.exalted_per_divine)
        )
    } else {
        format!("Total: {} ex", format_amount(total_ex))
    };
    (rows, total)
}
