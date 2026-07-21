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

// --- rate limiting, search, fetch (verified live 2026-07-21; see the
// phase-4 plan's "Verified trade API facts" and the recorded fixtures) ---

/// One `max:window_seconds:ban_seconds` rule from `x-rate-limit-ip`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateRule {
    pub max: u32,
    pub window_s: u32,
    pub ban_s: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RateDecision {
    Ready,
    Wait(std::time::Duration),
}

/// Client-side mirror of the server's sliding-window limits, driven by the
/// real response headers: `x-rate-limit-ip` declares the rules,
/// `x-rate-limit-ip-state` (`used:window:banned_for`) reports the server's
/// own view after every response and overrides local bookkeeping.
#[derive(Debug)]
pub struct RateLimiter {
    rules: Vec<RateRule>,
    /// Locally recorded request instants, pruned to the largest window.
    sent: Vec<std::time::Instant>,
    banned_until: Option<std::time::Instant>,
}

impl RateLimiter {
    pub fn from_header(h: &str) -> RateLimiter {
        let rules = h
            .split(',')
            .filter_map(|t| {
                let mut it = t.trim().split(':');
                Some(RateRule {
                    max: it.next()?.parse().ok()?,
                    window_s: it.next()?.parse().ok()?,
                    ban_s: it.next()?.parse().ok()?,
                })
            })
            .collect();
        RateLimiter { rules, sent: Vec::new(), banned_until: None }
    }

    /// Applies a fresh `x-rate-limit-ip-state` header: an active ban
    /// (third field nonzero) locks the limiter for that long.
    pub fn apply_state(&mut self, state: &str) {
        for t in state.split(',') {
            let mut it = t.trim().split(':');
            let (Some(_used), Some(_win), Some(ban)) = (it.next(), it.next(), it.next()) else {
                continue;
            };
            if let Ok(ban_s) = ban.parse::<u64>() {
                if ban_s > 0 {
                    let until = std::time::Instant::now() + std::time::Duration::from_secs(ban_s);
                    self.banned_until = Some(match self.banned_until {
                        Some(b) if b > until => b,
                        _ => until,
                    });
                }
            }
        }
    }

    pub fn check(&mut self) -> RateDecision {
        let now = std::time::Instant::now();
        if let Some(until) = self.banned_until {
            if until > now {
                return RateDecision::Wait(until - now);
            }
            self.banned_until = None;
        }
        let max_window = self.rules.iter().map(|r| r.window_s).max().unwrap_or(0);
        self.sent
            .retain(|t| now.duration_since(*t).as_secs() < u64::from(max_window));
        let mut wait = std::time::Duration::ZERO;
        for r in &self.rules {
            let in_window = self
                .sent
                .iter()
                .filter(|t| now.duration_since(**t).as_secs() < u64::from(r.window_s))
                .count() as u32;
            if in_window >= r.max {
                // Free again when the oldest in-window request expires.
                if let Some(oldest) = self
                    .sent
                    .iter()
                    .filter(|t| now.duration_since(**t).as_secs() < u64::from(r.window_s))
                    .min()
                {
                    let free_in = std::time::Duration::from_secs(u64::from(r.window_s))
                        .saturating_sub(now.duration_since(*oldest));
                    wait = wait.max(free_in);
                }
            }
        }
        if wait.is_zero() {
            RateDecision::Ready
        } else {
            RateDecision::Wait(wait)
        }
    }

    /// Records a request the caller is about to send.
    pub fn record(&mut self) {
        self.sent.push(std::time::Instant::now());
    }
}

// --- query building (body shape byte-verified against the live probe) ---

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct StatFilter {
    pub id: String,
    pub value: FilterValue,
    pub disabled: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct FilterValue {
    pub min: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub category: Option<String>,
    pub filters: Vec<StatFilter>,
}

impl Query {
    /// Serializes to the exact body shape verified live.
    pub fn to_body(&self) -> serde_json::Value {
        let mut query = serde_json::json!({
            "status": {"option": "online"},
            "stats": [{
                "type": "and",
                "filters": self.filters,
            }],
        });
        if let Some(cat) = &self.category {
            query["filters"] = serde_json::json!({
                "type_filters": {"filters": {"category": {"option": cat}}}
            });
        }
        serde_json::json!({"query": query, "sort": {"price": "asc"}})
    }
}

/// Item classes verified against live category options; extended only as
/// new classes appear in fixtures.
fn category_for(item_class: &str) -> Option<String> {
    let c = item_class.to_ascii_lowercase();
    let cat = match c.as_str() {
        "bows" => "weapon.bow",
        "amulets" => "accessory.amulet",
        "rings" => "accessory.ring",
        "belts" => "accessory.belt",
        "jewels" => "jewel",
        _ => return None,
    };
    Some(cat.to_string())
}

/// Mods worth preselecting: the stats that dominate rare pricing.
fn preselect(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    ["resistance", "maximum life", "maximum mana", "attributes", "dexterity", "strength",
     "intelligence", "damage", "level of all", "skill"]
        .iter()
        .any(|k| t.contains(k))
}

/// First number in a mod line (the rolled value), if any.
fn first_number(text: &str) -> Option<f64> {
    let mut cur = String::new();
    for ch in text.chars() {
        if ch.is_ascii_digit() || (ch == '.' && !cur.is_empty()) {
            cur.push(ch);
        } else if !cur.is_empty() {
            break;
        }
    }
    if cur.is_empty() {
        None
    } else {
        cur.parse().ok()
    }
}

/// Builds the default appraisal query for a parsed rare: every resolvable
/// explicit mod becomes a stat filter with min = floor(rolled * 0.9)
/// (undershot to widen matches, per the established price-checker
/// pattern); pricing-dominant mods start enabled, the rest disabled.
pub fn build_query(item: &crate::item::Item, stats: &StatIndex) -> Query {
    let mut filters: Vec<StatFilter> = Vec::new();
    for m in &item.explicits {
        // Rune-socket mods are gear, not the item's own explicits; the
        // trade site treats them separately, and mapping one onto an
        // explicit filter (same stat id, lower roll) both duplicates the
        // filter and drags the min value down.
        if m.header.as_ref().is_some_and(|h| h.kind == crate::item::ModKind::Rune) {
            continue;
        }
        let Some(entry) = stats.resolve(&m.text) else { continue };
        if filters.iter().any(|f| f.id == entry.id) {
            continue;
        }
        let Some(v) = first_number(&m.text) else { continue };
        filters.push(StatFilter {
            id: entry.id.clone(),
            value: FilterValue { min: (v * 0.9).floor() as i64 },
            disabled: !preselect(&m.text),
        });
    }
    Query { category: category_for(&item.item_class), filters }
}
