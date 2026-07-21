//! Maps a parsed item mod's display text to the trade-site stat id it
//! corresponds to, using the `/api/trade2/data/stats` catalog (verified live
//! 2026-07-21; see `core/tests/fixtures/trade_stats.json` for a trimmed
//! recorded response).

use std::collections::HashMap;

use serde::Deserialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TradeError {
    #[error("bad json: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatEntry {
    pub id: String,
    pub text: String,
}

#[derive(Debug, Deserialize)]
struct StatsResponse {
    result: Vec<RawGroup>,
}

#[derive(Debug, Deserialize)]
struct RawGroup {
    id: String,
    entries: Vec<RawEntry>,
}

#[derive(Debug, Deserialize)]
struct RawEntry {
    id: String,
    text: String,
}

/// Group ids tried in this order when a mod text could plausibly appear in
/// more than one group; an item's own mods are always explicit or implicit,
/// with pseudo only ever relevant for aggregate ("total resistance") stats.
const GROUP_PRIORITY: [&str; 3] = ["explicit", "implicit", "pseudo"];

pub struct StatIndex {
    groups: HashMap<String, HashMap<String, StatEntry>>,
}

impl StatIndex {
    pub fn from_json(s: &str) -> Result<StatIndex, TradeError> {
        let parsed: StatsResponse = serde_json::from_str(s)?;
        let mut groups = HashMap::new();
        for g in parsed.result {
            let mut by_text = HashMap::new();
            for e in g.entries {
                by_text.insert(
                    e.text.clone(),
                    StatEntry {
                        id: e.id,
                        text: e.text,
                    },
                );
            }
            groups.insert(g.id, by_text);
        }
        Ok(StatIndex { groups })
    }

    /// Resolves a parsed item mod's raw text to its stat entry: strips the
    /// game's `[Tag|Display]` bracket syntax down to the display half, drops
    /// roll-annotation parentheticals like `(155-169)`, replaces every
    /// remaining number with `#`, then exact-matches against the catalog,
    /// preferring explicit, then implicit, then pseudo.
    pub fn resolve(&self, mod_text: &str) -> Option<&StatEntry> {
        let normalized = normalize_mod_text(mod_text);
        for group_id in GROUP_PRIORITY {
            if let Some(entry) = self.groups.get(group_id).and_then(|g| g.get(&normalized)) {
                return Some(entry);
            }
        }
        for (group_id, entries) in &self.groups {
            if GROUP_PRIORITY.contains(&group_id.as_str()) {
                continue;
            }
            if let Some(entry) = entries.get(&normalized) {
                return Some(entry);
            }
        }
        None
    }
}

/// Keeps only the display half of `[Tag|Display]` (or the bare word of a
/// tagless `[Display]`), dropping the brackets.
fn strip_tag_brackets(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '[' {
            let mut inner = String::new();
            for c2 in chars.by_ref() {
                if c2 == ']' {
                    break;
                }
                inner.push(c2);
            }
            let display = inner.rsplit('|').next().unwrap_or(&inner);
            out.push_str(display);
        } else {
            out.push(c);
        }
    }
    out
}

/// Drops parenthesized roll ranges such as `(155-169)` or `(3.11-3.8)`
/// entirely: a parenthetical whose contents are only digits, `.`, and `-`.
/// Any other parenthetical (there should be none left in mod text at this
/// point) is left untouched.
fn strip_roll_parens(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '(' {
            let mut inner = String::new();
            let mut closed = false;
            for c2 in chars.by_ref() {
                if c2 == ')' {
                    closed = true;
                    break;
                }
                inner.push(c2);
            }
            let is_roll = closed
                && !inner.is_empty()
                && inner.chars().all(|ch| ch.is_ascii_digit() || ch == '.' || ch == '-');
            if !is_roll {
                out.push('(');
                out.push_str(&inner);
                if closed {
                    out.push(')');
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Replaces every integer or decimal number (optionally signed) with `#`,
/// dropping the sign; the catalog's own template text carries any `+` that
/// belongs in the display (e.g. pseudo's `+#% total to Cold Resistance`).
fn replace_numbers(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let sign_starts_number =
            (c == '+' || c == '-') && chars.get(i + 1).is_some_and(|n| n.is_ascii_digit());
        if c.is_ascii_digit() || sign_starts_number {
            let mut j = i + usize::from(sign_starts_number);
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
            if chars.get(j) == Some(&'.') && chars.get(j + 1).is_some_and(|d| d.is_ascii_digit()) {
                j += 1;
                while j < chars.len() && chars[j].is_ascii_digit() {
                    j += 1;
                }
            }
            out.push('#');
            i = j;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

fn normalize_mod_text(text: &str) -> String {
    let no_tags = strip_tag_brackets(text);
    let no_rolls = strip_roll_parens(&no_tags);
    replace_numbers(&no_rolls)
}

#[cfg(test)]
mod normalize_tests {
    use super::normalize_mod_text;

    #[test]
    fn strips_roll_annotation_and_numbers() {
        assert_eq!(
            normalize_mod_text("157(155-169)% increased Physical Damage"),
            "#% increased Physical Damage"
        );
    }

    #[test]
    fn drops_sign_on_replaced_number() {
        assert_eq!(normalize_mod_text("+31(31-33) to Dexterity"), "# to Dexterity");
    }

    #[test]
    fn strips_display_tag_brackets() {
        assert_eq!(
            normalize_mod_text("Adds 1 to 13 [Lightning|Lightning] Damage"),
            "Adds # to # Lightning Damage"
        );
    }

    #[test]
    fn handles_decimal_rolls() {
        assert_eq!(
            normalize_mod_text("+3.48(3.11-3.8)% to Critical Hit Chance"),
            "#% to Critical Hit Chance"
        );
    }
}
