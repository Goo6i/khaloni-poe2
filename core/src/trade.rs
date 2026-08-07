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
    #[error("http: {0}")]
    Http(String),
    #[error("rate limited; retry in {0:?}")]
    Cooldown(std::time::Duration),
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
    /// Every entry keyed by its stat id, for the lookups that start from an
    /// id rather than mod text (pseudo aggregates, which no single mod line
    /// spells out).
    by_id: HashMap<String, StatEntry>,
}

impl StatIndex {
    pub fn from_json(s: &str) -> Result<StatIndex, TradeError> {
        let parsed: StatsResponse = serde_json::from_str(s)?;
        let mut groups = HashMap::new();
        let mut by_id = HashMap::new();
        for g in parsed.result {
            let mut by_text = HashMap::new();
            for e in g.entries {
                let entry = StatEntry {
                    id: e.id,
                    text: e.text,
                };
                by_id.insert(entry.id.clone(), entry.clone());
                by_text.insert(entry.text.clone(), entry);
            }
            groups.insert(g.id, by_text);
        }
        Ok(StatIndex { groups, by_id })
    }

    /// Looks a stat up by its trade id (e.g.
    /// "pseudo.pseudo_total_elemental_resistance"). `None` when the catalog
    /// this index was built from has no such stat, so a caller can never
    /// search an id the site does not know.
    pub fn entry_by_id(&self, id: &str) -> Option<&StatEntry> {
        self.by_id.get(id)
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
    /// Search bound. Float so decimal rolls (attack speed 1.35, crit 3.5%)
    /// are searchable; whole values serialize back as integers (see
    /// `ser_num`), so the body still reads `155`, not `155.0`.
    #[serde(serialize_with = "ser_num")]
    pub min: f64,
    /// Optional upper bound; omitted from the request when `None`.
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "ser_opt_num")]
    pub max: Option<f64>,
}

/// A search bound as JSON: an integer when whole (`155`), else a float
/// (`3.5`). The trade site sends whole values as integers, and the verified
/// body shape asserts on integers, so we preserve that.
fn num_json(v: f64) -> serde_json::Value {
    if v.is_finite() && v.fract() == 0.0 {
        serde_json::json!(v as i64)
    } else {
        serde_json::json!(v)
    }
}

fn ser_num<S: serde::Serializer>(v: &f64, s: S) -> Result<S::Ok, S::Error> {
    if v.is_finite() && v.fract() == 0.0 {
        s.serialize_i64(*v as i64)
    } else {
        s.serialize_f64(*v)
    }
}

fn ser_opt_num<S: serde::Serializer>(v: &Option<f64>, s: S) -> Result<S::Ok, S::Error> {
    match v {
        Some(x) => ser_num(x, s),
        None => s.serialize_none(),
    }
}

/// Weapon damage bounds, each a MINIMUM (`None` = unset). These are the
/// trade site's own computed weapon numbers - total DPS, physical DPS,
/// elemental DPS, critical hit chance, attacks per second - not item mods,
/// so they live in their own filter section rather than in `stats`.
///
/// Serialized into `filters.equipment_filters`, NOT `filters.weapon_filters`:
/// verified live 2026-08-07 against `/api/trade2/data/filters`, whose only
/// group carrying `dps`/`pdps`/`edps`/`crit`/`aps` is `equipment_filters`.
/// A probe POST with a `weapon_filters` group is rejected outright with
/// `{"error":{"code":2,"message":"Unknown filter group: weapon_filters"}}`
/// (that is the PoE1 trade name; trade2 renamed the section), while the same
/// body under `equipment_filters` returns results.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct WeaponFilters {
    /// Total damage per second.
    pub dps: Option<f64>,
    /// Physical damage per second.
    pub pdps: Option<f64>,
    /// Elemental damage per second.
    pub edps: Option<f64>,
    /// Critical hit chance, in percent (decimal, e.g. 6.5).
    pub crit: Option<f64>,
    /// Attacks per second (decimal, e.g. 1.45).
    pub aps: Option<f64>,
}

impl WeaponFilters {
    /// The `(trade key, bound)` pairs in the site's own filter order.
    fn pairs(&self) -> [(&'static str, Option<f64>); 5] {
        [
            ("dps", self.dps),
            ("pdps", self.pdps),
            ("edps", self.edps),
            ("crit", self.crit),
            ("aps", self.aps),
        ]
    }

    /// True when no bound is set, so the whole block is omitted from the body.
    pub fn is_empty(&self) -> bool {
        self.pairs().iter().all(|(_, v)| v.is_none())
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Query {
    pub category: Option<String>,
    /// Whether the gear `category` constraint is applied. Off means a
    /// mods-only search across every base (the common "what do these rolls
    /// sell for" check); the category rides along dormant so it can be
    /// re-enabled without rebuilding the query.
    pub category_enabled: bool,
    /// Exact base type to search (`query.type`), e.g. "Waystone" - used for
    /// items priced by base + a map filter rather than by gear category.
    pub type_name: Option<String>,
    /// Waystone/map tier -> `filters.map_filters.filters.map_tier` (searched
    /// as an exact tier: `{min: t, max: t}`), the dominant price driver.
    pub map_tier: Option<i64>,
    /// Skill/support gem level -> `filters.misc_filters.filters.gem_level`
    /// (exact: `{min: n, max: n}`). Set with category "gem.activegem" and the
    /// gem's skill name as `type_name` to price a specific cut gem.
    pub gem_level: Option<i64>,
    /// Weapon damage bounds -> `filters.equipment_filters.filters.{dps,pdps,
    /// edps,crit,aps}`. `None` (or an all-unset `WeaponFilters`) omits the
    /// whole section.
    pub weapon: Option<WeaponFilters>,
    pub filters: Vec<StatFilter>,
}

impl Query {
    /// Serializes to the exact trade-search body shape (verified live, and
    /// against EE2's builder for the map_filters/type paths).
    pub fn to_body(&self) -> serde_json::Value {
        // Filters whose id starts with "map_" (waystone rarity/pack-size/
        // effectiveness/etc.) go in the map_filters section, not stats; and
        // only enabled ones (map filters have no disabled flag). Everything
        // else is a stat filter (disabled ones ride along with disabled:true).
        let stat_filters: Vec<&StatFilter> =
            self.filters.iter().filter(|f| !f.id.starts_with("map_")).collect();
        let mut query = serde_json::json!({
            // "any" includes offline sellers: with the 0.5 asynchronous
            // marketplace their items are securable without them being online,
            // so they belong in the price just like online listings.
            "status": {"option": "any"},
            "stats": [{"type": "and", "filters": stat_filters}],
        });
        if let Some(t) = &self.type_name {
            query["type"] = serde_json::json!(t);
        }
        let mut filters = serde_json::Map::new();
        if let (true, Some(cat)) = (self.category_enabled, &self.category) {
            filters.insert(
                "type_filters".into(),
                serde_json::json!({"filters": {"category": {"option": cat}}}),
            );
        }
        let mut mapf = serde_json::Map::new();
        if let Some(tier) = self.map_tier {
            mapf.insert("map_tier".into(), serde_json::json!({"min": tier, "max": tier}));
        }
        for f in self.filters.iter().filter(|f| f.id.starts_with("map_") && !f.disabled) {
            let mut val = serde_json::json!({"min": num_json(f.value.min)});
            if let Some(mx) = f.value.max {
                val["max"] = num_json(mx);
            }
            mapf.insert(f.id.clone(), val);
        }
        if !mapf.is_empty() {
            filters.insert("map_filters".into(), serde_json::json!({"filters": mapf}));
        }
        if let Some(level) = self.gem_level {
            filters.insert(
                "misc_filters".into(),
                serde_json::json!({"filters": {"gem_level": {"min": level, "max": level}}}),
            );
        }
        // Weapon damage bounds are open-ended minimums ("this fast or faster",
        // "this much DPS or more"), so each carries a `min` and no `max`.
        if let Some(w) = &self.weapon {
            let mut wf = serde_json::Map::new();
            for (key, bound) in w.pairs() {
                if let Some(v) = bound {
                    wf.insert(key.into(), serde_json::json!({"min": num_json(v)}));
                }
            }
            if !wf.is_empty() {
                filters.insert(
                    "equipment_filters".into(),
                    serde_json::json!({"disabled": false, "filters": wf}),
                );
            }
        }
        if !filters.is_empty() {
            query["filters"] = serde_json::Value::Object(filters);
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
/// explicit mod becomes a stat filter with min = the mod's tier floor
/// (see `tier_floor`), so the search is by tier range rather than the
/// exact roll; pricing-dominant mods start enabled, the rest disabled.
/// A filter's human-facing description, index-aligned with
/// Query::filters: the cleaned mod text, its tier (when annotated), and
/// the search floor. This is what an interactive panel renders next to
/// each checkbox.
#[derive(Debug, Clone, PartialEq)]
pub struct FilterLabel {
    pub text: String,
    pub tier: Option<u8>,
    pub min: i64,
    /// Mod group for panel grouping/tagging: "implicit", "explicit", "map".
    pub tag: &'static str,
}

pub fn build_query(item: &crate::item::Item, stats: &StatIndex) -> Query {
    build_query_with_labels(item, stats).0
}

/// The value to search a mod on: the low end of its tier's roll range (from
/// the item text's `157(155-169)%` annotation), matching every item of that
/// tier or better - what a price check wants. A mod with no range annotation
/// (a fixed implicit) is searched on its own value.
fn tier_floor(text: &str) -> Option<i64> {
    if let Some(open) = text.find('(') {
        let rest = &text[open + 1..];
        if let Some(close) = rest.find(')') {
            let inside = &rest[..close];
            if let Some(low) = inside.split('-').next() {
                if let Ok(v) = low.trim().parse::<f64>() {
                    return Some(v.floor() as i64);
                }
            }
        }
    }
    first_number(text).map(|v| v.floor() as i64)
}

/// Builds a trade query covering EVERY modifier on the item, the way EE2
/// does: one stat filter per explicit AND implicit mod (so a waystone's
/// implicit rarity/pack-size/etc. and a rare's affixes are all searchable),
/// each with a value-based minimum. Explicit mods are enabled by default
/// only for the high-signal stats (`preselect`); implicit mods, which are an
/// item's defining stats, are enabled by default. `search_relaxed` drops
/// enabled filters until listings appear, so a fully-filtered query never
/// dead-ends.
/// Relaxes every enabled minimum by `pct` (0.10 = the panel's "Broad
/// (-10%)"), which surfaces the comparable items an exact-roll search
/// misses. Disabled filters and open minimums are left alone, and values
/// are floored at zero so a relaxation can never invert a bound.
/// A search filter on a pseudo aggregate stat ("+# total to Fire
/// Resistance" summed across every mod that grants it). Resolved through
/// the live stat catalog: an id the site does not list yields `None`
/// rather than a filter the search API would reject.
pub fn pseudo_filter(stats: &StatIndex, pseudo_id: &str, min: f64) -> Option<StatFilter> {
    let entry = stats.entry_by_id(pseudo_id)?;
    Some(StatFilter {
        id: entry.id.clone(),
        value: FilterValue { min, max: None },
        disabled: false,
    })
}

pub fn relax_query(query: &Query, pct: f64) -> Query {
    let mut out = query.clone();
    for f in &mut out.filters {
        if !f.disabled && f.value.min > 0.0 {
            f.value.min = (f.value.min * (1.0 - pct)).max(0.0);
        }
    }
    // Weapon bounds are minimums like any other; a Broad search must not
    // hold DPS to the exact roll while relaxing every mod around it.
    if let Some(w) = &mut out.weapon {
        for v in [&mut w.dps, &mut w.pdps, &mut w.edps, &mut w.crit, &mut w.aps]
            .into_iter()
            .flatten()
        {
            *v *= 1.0 - pct;
        }
    }
    out
}

pub fn build_query_with_labels(
    item: &crate::item::Item,
    stats: &StatIndex,
) -> (Query, Vec<FilterLabel>) {
    let mut filters: Vec<StatFilter> = Vec::new();
    let mut labels: Vec<FilterLabel> = Vec::new();
    for m in &item.explicits {
        // Rune-socket mods are gear, not the item's own explicits; the trade
        // site treats them separately, and mapping one onto an explicit
        // filter duplicates it and drags the min down.
        if m.header.as_ref().is_some_and(|h| h.kind == crate::item::ModKind::Rune) {
            continue;
        }
        push_filter(m, stats, !preselect(&m.text), "explicit", &mut filters, &mut labels);
    }
    for m in &item.implicits {
        push_filter(m, stats, false, "implicit", &mut filters, &mut labels);
    }
    // Waystones/maps are priced by base type + tier (their danger mods above
    // ride along disabled). Their reward properties (Item Rarity, Pack Size,
    // Monster Effectiveness, etc.) become pickable map_filters, disabled by
    // default so the user chooses which to search by.
    let (type_name, map_tier) = if item.item_class.eq_ignore_ascii_case("waystones") {
        for (id, label, min) in waystone_reward_filters(item) {
            filters.push(StatFilter {
                id,
                value: FilterValue { min: min as f64, max: None },
                disabled: true,
            });
            labels.push(FilterLabel { text: label, tier: None, min, tag: "map" });
        }
        // The trade catalog names each waystone base with its tier baked
        // in - "Waystone (Tier 15)" - and rejects the bare "Waystone" base
        // ("Unknown item base type", observed live 2026-08-07). The full
        // base line the game wrote is what gets searched; the tier is
        // still parsed out for the map_tier filter beside it.
        let tier = item.base_type.as_deref().and_then(|b| split_waystone(b).1);
        (item.base_type.clone().filter(|b| !b.is_empty()), tier)
    } else {
        (None, None)
    };
    (
        Query {
            category: category_for(&item.item_class),
            category_enabled: true,
            type_name,
            map_tier,
            gem_level: None,
            weapon: None,
            filters,
        },
        labels,
    )
}

/// Builds the gear-upgrade query for an equipped item: strictly-better
/// pieces of the same gear class. The category constraint is applied the
/// same way `build_query` applies it (`category_for` on the item class,
/// enabled), and every resolvable explicit mod with a numeric roll becomes
/// an ENABLED stat filter whose min is the item's CURRENT rolled value -
/// not the tier floor `build_query` searches on - so a match must
/// meet-or-beat the equipped item on every kept mod. Mods that fail stat
/// resolution or carry no numeric value are skipped, never guessed.
/// Rune-socket mods are skipped for the same reason `build_query` skips
/// them: they are swappable gear, not the item's own explicits.
///
/// Results come back cheapest-first without any extra step here:
/// `Query::to_body` always emits `"sort": {"price": "asc"}`.
pub fn build_upgrade_query(item: &crate::item::Item, stats: &StatIndex) -> Query {
    let mut filters: Vec<StatFilter> = Vec::new();
    for m in &item.explicits {
        if m.header.as_ref().is_some_and(|h| h.kind == crate::item::ModKind::Rune) {
            continue;
        }
        let Some(entry) = stats.resolve(&m.text) else { continue };
        if filters.iter().any(|f| f.id == entry.id) {
            continue;
        }
        let Some(min) = first_number(&m.text) else { continue };
        filters.push(StatFilter {
            id: entry.id.clone(),
            value: FilterValue { min, max: None },
            disabled: false,
        });
    }
    Query {
        category: category_for(&item.item_class),
        category_enabled: true,
        type_name: None,
        map_tier: None,
        gem_level: None,
        weapon: None,
        filters,
    }
}

/// Panel/window title for an upgrade search, e.g. "upgrades: Bows".
pub fn upgrade_title(item: &crate::item::Item) -> String {
    format!("upgrades: {}", item.item_class)
}

/// Query for a specific cut skill gem at an exact level, e.g. "Detonate
/// Living" (Level 20): category `gem.activegem`, the skill name as the base
/// `type`, and an exact `gem_level` filter. This is how the reward panel
/// prices its "Skill Level N: <name>" rows individually instead of collapsing
/// them all to the fungible Uncut Skill Gem price.
pub fn build_gem_query(skill: &str, level: i64) -> Query {
    Query {
        category: Some("gem.activegem".into()),
        category_enabled: true,
        type_name: Some(skill.to_string()),
        map_tier: None,
        gem_level: Some(level),
        weapon: None,
        filters: Vec::new(),
    }
}

/// Parses the currency-exchange response into the cheapest rate: how many
/// units of the `have` currency one unit of the `want` currency costs
/// (`exchange.amount / item.amount`, minimized across all offers). `None`
/// when there are no offers. This is how EE2 prices stackable currency
/// (omens, essences, catalysts) that poe.ninja does not track for PoE2.
pub fn parse_exchange_rate(body: &str) -> Option<f64> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let result = v.get("result")?.as_object()?;
    let mut best: Option<f64> = None;
    for listing in result.values() {
        let Some(offers) = listing.pointer("/listing/offers").and_then(|o| o.as_array()) else {
            continue;
        };
        for offer in offers {
            let item_amt = offer.pointer("/item/amount").and_then(serde_json::Value::as_f64);
            let exch_amt = offer.pointer("/exchange/amount").and_then(serde_json::Value::as_f64);
            if let (Some(item_amt), Some(exch_amt)) = (item_amt, exch_amt) {
                if item_amt > 0.0 {
                    let rate = exch_amt / item_amt;
                    best = Some(best.map_or(rate, |b: f64| b.min(rate)));
                }
            }
        }
    }
    best
}

/// Maps each currency item's display name (lowercased) to its trade currency
/// id, from the `/api/trade2/data/static` response (e.g. "omen of whittling"
/// -> "omen-of-whittling", "exalted orb" -> "exalted").
pub fn parse_static_currency_ids(body: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return map;
    };
    let Some(groups) = v.get("result").and_then(|r| r.as_array()) else {
        return map;
    };
    for g in groups {
        let Some(entries) = g.get("entries").and_then(|e| e.as_array()) else {
            continue;
        };
        for e in entries {
            let id = e.get("id").and_then(|i| i.as_str());
            let text = e.get("text").and_then(|t| t.as_str());
            if let (Some(id), Some(text)) = (id, text) {
                map.insert(text.to_lowercase(), id.to_string());
            }
        }
    }
    map
}

/// Exact gem base-type names from the `/api/trade2/data/items` "Gems" group
/// (e.g. "Detonate Living", "Fragments Of The Past"). The trade `type` field
/// is case-sensitive, so these are matched against OCR text to recover the
/// exact spelling before searching.
pub fn parse_gem_types(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return out;
    };
    let Some(groups) = v.get("result").and_then(|r| r.as_array()) else {
        return out;
    };
    for g in groups {
        if g.get("label").and_then(|l| l.as_str()) != Some("Gems") {
            continue;
        }
        if let Some(entries) = g.get("entries").and_then(|e| e.as_array()) {
            for e in entries {
                if let Some(t) = e.get("type").and_then(|t| t.as_str()) {
                    out.push(t.to_string());
                }
            }
        }
    }
    out
}

/// Recovers the exact gem name for OCR text (lowercased, whitespace-collapsed)
/// by matching against `gems`: an exact case-insensitive hit first, else the
/// closest fuzzy match above a confidence floor (tolerates minor OCR slips).
/// `None` when nothing is close enough, so a misread never prices the wrong gem.
pub fn match_gem_name(ocr: &str, gems: &[String]) -> Option<String> {
    let norm = |s: &str| s.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ");
    let q = norm(ocr);
    if q.is_empty() {
        return None;
    }
    if let Some(g) = gems.iter().find(|g| norm(g) == q) {
        return Some(g.clone());
    }
    let mut best: Option<(f64, &String)> = None;
    for g in gems {
        let sim = strsim::normalized_levenshtein(&q, &norm(g));
        if best.as_ref().is_none_or(|(b, _)| sim > *b) {
            best = Some((sim, g));
        }
    }
    best.filter(|(s, _)| *s >= 0.82).map(|(_, g)| g.clone())
}

/// Parses a waystone's reward property lines into (trade map_filter id,
/// display label, min value). Keys verified live against trade2 data/filters:
/// "Item Rarity: +24%" -> ("map_iir","Item Rarity",24); Monster Effectiveness
/// is the trade key `map_magic_monsters`.
fn waystone_reward_filters(item: &crate::item::Item) -> Vec<(String, String, i64)> {
    const MAP: [(&str, &str); 6] = [
        ("Item Rarity", "map_iir"),
        ("Pack Size", "map_packsize"),
        ("Monster Effectiveness", "map_magic_monsters"),
        ("Monster Rarity", "map_rare_monsters"),
        ("Waystone Drop Chance", "map_bonus"),
        ("Revives Available", "map_revives"),
    ];
    let mut out = Vec::new();
    for sec in &item.sections {
        for line in sec {
            for (prop, id) in MAP {
                if let Some(rest) = line.strip_prefix(&format!("{prop}: ")) {
                    if let Some(v) = first_number(rest) {
                        out.push((id.to_string(), prop.to_string(), v.floor() as i64));
                    }
                }
            }
        }
    }
    out
}

/// The API's own explanation when it has one: a trade error body carries
/// `{"error":{"message":…}}`, and "Unknown item base type" diagnoses a
/// problem that a bare "status 400" hides.
fn api_error(what: &str, status: u16, body: &str) -> String {
    let msg = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| Some(v.get("error")?.get("message")?.as_str()?.to_string()));
    match msg {
        Some(m) => format!("{what}: {m} ({status})"),
        None => format!("{what} status {status}"),
    }
}

/// Splits a waystone base type into its bare base and tier:
/// "Waystone (Tier 15)" -> ("Waystone", Some(15)).
fn split_waystone(base_type: &str) -> (String, Option<i64>) {
    if let Some(open) = base_type.find(" (Tier ") {
        let base = base_type[..open].trim().to_string();
        let tier = base_type[open..]
            .chars()
            .filter(char::is_ascii_digit)
            .collect::<String>()
            .parse()
            .ok();
        (base, tier)
    } else {
        (base_type.trim().to_string(), None)
    }
}

/// Resolves `m` to a trade stat and appends a filter + label, unless the stat
/// is unknown, already filtered, or has no numeric roll. `disabled` sets the
/// filter's default-enabled state.
fn push_filter(
    m: &crate::item::ItemMod,
    stats: &StatIndex,
    disabled: bool,
    tag: &'static str,
    filters: &mut Vec<StatFilter>,
    labels: &mut Vec<FilterLabel>,
) {
    let Some(entry) = stats.resolve(&m.text) else { return };
    if filters.iter().any(|f| f.id == entry.id) {
        return;
    }
    let Some(min) = tier_floor(&m.text) else { return };
    filters.push(StatFilter {
        id: entry.id.clone(),
        value: FilterValue { min: min as f64, max: None },
        disabled,
    });
    labels.push(FilterLabel {
        text: strip_range_annotations(&m.text),
        tier: m.header.as_ref().and_then(|h| h.tier),
        min,
        tag,
    });
}

/// Removes the advanced-format "(min-max)" roll annotations for display:
/// "+45(40-49) to maximum Life" reads as "+45 to maximum Life".
fn strip_range_annotations(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut depth = 0u32;
    for c in text.chars() {
        match c {
            '(' => depth += 1,
            ')' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

impl TradeClient {
    /// Searches with auto-relaxation: an exact multi-mod query on a rare
    /// usually matches nothing, so if the full enabled filter set returns
    /// no listings, drop the least-impactful enabled filter (the one with
    /// the smallest min, so the biggest mods are kept longest) and retry,
    /// down to the single strongest mod. Returns the first non-empty
    /// result with how many filters it took, or the last (empty) result.
    pub fn search_relaxed(&mut self, query: &Query) -> Result<(SearchResult, usize), TradeError> {
        // Order enabled filters by min descending; disabled ones ride along
        // untouched (they never constrain the search).
        let mut enabled: Vec<usize> = query
            .filters
            .iter()
            .enumerate()
            .filter(|(_, f)| !f.disabled)
            .map(|(i, _)| i)
            .collect();
        // Keep the highest-floor mods longest (they discriminate price most).
        // f64 has no total Ord, so compare directly; NaN never occurs here.
        enabled.sort_by(|&a, &b| {
            query.filters[b].value.min
                .partial_cmp(&query.filters[a].value.min)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut last: Option<SearchResult> = None;
        for keep in (1..=enabled.len()).rev() {
            let keep_set: std::collections::HashSet<usize> =
                enabled.iter().take(keep).copied().collect();
            let filters = query
                .filters
                .iter()
                .enumerate()
                .map(|(i, f)| {
                    let mut f = f.clone();
                    // Disable enabled filters we are dropping this round.
                    if !f.disabled && !keep_set.contains(&i) {
                        f.disabled = true;
                    }
                    f
                })
                .collect();
            let trimmed = Query {
                category: query.category.clone(),
                category_enabled: query.category_enabled,
                type_name: query.type_name.clone(),
                map_tier: query.map_tier,
                gem_level: query.gem_level,
                weapon: query.weapon,
                filters,
            };
            let result = self.search(&trimmed)?;
            if !result.hashes.is_empty() {
                return Ok((result, keep));
            }
            last = Some(result);
        }
        // No enabled filters, or every relaxation was empty: one plain
        // search on category alone.
        if enabled.is_empty() {
            let result = self.search(query)?;
            return Ok((result, 0));
        }
        Ok((last.expect("loop ran at least once"), 0))
    }
}

// --- HTTP client (endpoints and shapes verified live 2026-07-21) ---

#[derive(Debug, Deserialize)]
pub struct SearchResult {
    pub id: String,
    #[serde(rename = "result")]
    pub hashes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Listing {
    pub price_amount: f64,
    pub price_currency: String,
    pub account: String,
    pub indexed: String,
    pub item_name: String,
}

#[derive(Debug, Deserialize)]
struct FetchResponse {
    result: Vec<FetchEntry>,
}

#[derive(Debug, Deserialize)]
struct FetchEntry {
    listing: RawListing,
    item: Option<RawItem>,
}

#[derive(Debug, Deserialize)]
struct RawListing {
    price: Option<RawPrice>,
    indexed: Option<String>,
    account: Option<RawAccount>,
}

#[derive(Debug, Deserialize)]
struct RawPrice {
    amount: f64,
    currency: String,
}

#[derive(Debug, Deserialize)]
struct RawAccount {
    name: String,
}

#[derive(Debug, Deserialize)]
struct RawItem {
    name: Option<String>,
    #[serde(rename = "baseType")]
    base_type: Option<String>,
}

pub fn parse_search(json: &str) -> Result<SearchResult, TradeError> {
    Ok(serde_json::from_str(json)?)
}

pub fn parse_fetch(json: &str) -> Result<Vec<Listing>, TradeError> {
    let r: FetchResponse = serde_json::from_str(json)?;
    Ok(r.result
        .into_iter()
        .filter_map(|e| {
            let price = e.listing.price?;
            Some(Listing {
                price_amount: price.amount,
                price_currency: price.currency,
                account: e.listing.account.map(|a| a.name).unwrap_or_default(),
                indexed: e.listing.indexed.unwrap_or_default(),
                item_name: e
                    .item
                    .and_then(|i| i.name.or(i.base_type))
                    .unwrap_or_default(),
            })
        })
        .collect())
}

/// Trade-site search client. All calls flow through per-endpoint rate
/// limiters seeded with the live-verified default rules and continuously
/// corrected by every response's state headers; an active ban surfaces as
/// `TradeError::Cooldown` and no request leaves while one is pending.
pub struct TradeClient {
    base: String,
    league: String,
    http: reqwest::blocking::Client,
    /// POESESSID cookie value; empty = anonymous. Only ever sent to `base`
    /// (pathofexile.com in production), never logged.
    session: String,
    pub search_limiter: RateLimiter,
    pub fetch_limiter: RateLimiter,
}

impl TradeClient {
    pub fn new(base: &str, league: &str) -> Result<TradeClient, TradeError> {
        let http = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .user_agent(
                "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) \
                 Chrome/126.0 Safari/537.36 khaloni-poe2/0.1",
            )
            .build()
            .map_err(|e| TradeError::Http(e.to_string()))?;
        Ok(TradeClient {
            base: base.trim_end_matches('/').to_string(),
            league: league.to_string(),
            http,
            session: String::new(),
            search_limiter: RateLimiter::from_header("5:10:60,15:60:300,30:300:1800"),
            fetch_limiter: RateLimiter::from_header("12:4:10,16:12:300"),
        })
    }

    /// Sets the POESESSID session cookie for the account-backed endpoints
    /// (saved searches need the owning account). Empty clears it; every
    /// request goes back to anonymous.
    pub fn set_session(&mut self, poesessid: &str) {
        self.session = poesessid.trim().to_string();
    }

    /// Attaches the session cookie when one is set; a no-op otherwise, so
    /// the anonymous paths keep their exact pre-session request shape.
    fn authed(&self, req: reqwest::blocking::RequestBuilder) -> reqwest::blocking::RequestBuilder {
        if self.session.is_empty() {
            req
        } else {
            req.header(reqwest::header::COOKIE, format!("POESESSID={}", self.session))
        }
    }

    pub fn site_url(&self, search_id: &str) -> String {
        format!(
            "{}/trade2/search/poe2/{}/{}",
            self.base.replace("/api", ""),
            self.league,
            search_id
        )
    }

    fn absorb_headers(limiter: &mut RateLimiter, resp: &reqwest::blocking::Response) {
        if let Some(rules) = resp
            .headers()
            .get("x-rate-limit-ip")
            .and_then(|v| v.to_str().ok())
        {
            *limiter = RateLimiter::from_header(rules);
        }
        if let Some(state) = resp
            .headers()
            .get("x-rate-limit-ip-state")
            .and_then(|v| v.to_str().ok())
        {
            limiter.apply_state(state);
        }
    }

    pub fn search(&mut self, query: &Query) -> Result<SearchResult, TradeError> {
        if let RateDecision::Wait(d) = self.search_limiter.check() {
            return Err(TradeError::Cooldown(d));
        }
        self.search_limiter.record();
        let url = format!("{}/api/trade2/search/poe2/{}", self.base, self.league);
        let resp = self
            .authed(self.http.post(&url).json(&query.to_body()))
            .send()
            .map_err(|e| TradeError::Http(e.to_string()))?;
        Self::absorb_headers(&mut self.search_limiter, &resp);
        if resp.status().as_u16() == 429 {
            return Err(TradeError::Cooldown(std::time::Duration::from_secs(60)));
        }
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().unwrap_or_default();
            return Err(TradeError::Http(api_error("search", status, &body)));
        }
        parse_search(&resp.text().map_err(|e| TradeError::Http(e.to_string()))?)
    }

    /// Fetches the trade static-data currency map (display name -> currency
    /// id) once, for resolving an item's name to its exchange `want` id.
    pub fn static_currency_ids(&self) -> Result<HashMap<String, String>, TradeError> {
        let url = format!("{}/api/trade2/data/static", self.base);
        let resp = self
            .http
            .get(&url)
            .send()
            .map_err(|e| TradeError::Http(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(TradeError::Http(format!("static status {}", resp.status())));
        }
        let text = resp.text().map_err(|e| TradeError::Http(e.to_string()))?;
        Ok(parse_static_currency_ids(&text))
    }

    /// Prices a stackable currency (e.g. an omen) via the trade exchange:
    /// returns how many `have` currency one `want` unit costs, or `None` when
    /// there are no live offers. Covers the currency-exchange items poe.ninja
    /// does not track for PoE2.
    pub fn exchange(&mut self, want_id: &str, have_id: &str) -> Result<Option<f64>, TradeError> {
        if let RateDecision::Wait(d) = self.search_limiter.check() {
            return Err(TradeError::Cooldown(d));
        }
        self.search_limiter.record();
        let url = format!("{}/api/trade2/exchange/{}", self.base, self.league);
        let body = serde_json::json!({
            "engine": "new",
            "query": {"status": {"option": "online"}, "have": [have_id], "want": [want_id]},
            "sort": {"have": "asc"},
        });
        let resp = self
            .authed(self.http.post(&url).json(&body))
            .send()
            .map_err(|e| TradeError::Http(e.to_string()))?;
        Self::absorb_headers(&mut self.search_limiter, &resp);
        if resp.status().as_u16() == 429 {
            return Err(TradeError::Cooldown(std::time::Duration::from_secs(60)));
        }
        if !resp.status().is_success() {
            return Err(TradeError::Http(format!("exchange status {}", resp.status())));
        }
        let text = resp.text().map_err(|e| TradeError::Http(e.to_string()))?;
        Ok(parse_exchange_rate(&text))
    }

    /// Fetches the exact gem base-type names once (from data/items), for
    /// matching OCR'd skill names to a searchable `type`.
    pub fn gem_types(&self) -> Result<Vec<String>, TradeError> {
        let url = format!("{}/api/trade2/data/items", self.base);
        let resp = self
            .http
            .get(&url)
            .send()
            .map_err(|e| TradeError::Http(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(TradeError::Http(format!("items status {}", resp.status())));
        }
        let text = resp.text().map_err(|e| TradeError::Http(e.to_string()))?;
        Ok(parse_gem_types(&text))
    }

    /// Prices a specific cut skill gem at an exact level: searches by gem name
    /// + level, then fetches the cheapest listings. Returns them (price amount
    /// + currency); the caller converts to exalted via its currency table.
    pub fn price_gem(&mut self, skill: &str, level: i64) -> Result<Vec<Listing>, TradeError> {
        let sr = self.search(&build_gem_query(skill, level))?;
        if sr.hashes.is_empty() {
            return Ok(Vec::new());
        }
        self.fetch(&sr.id, &sr.hashes)
    }

    pub fn fetch(&mut self, search_id: &str, hashes: &[String]) -> Result<Vec<Listing>, TradeError> {
        if let RateDecision::Wait(d) = self.fetch_limiter.check() {
            return Err(TradeError::Cooldown(d));
        }
        self.fetch_limiter.record();
        let ids: Vec<&str> = hashes.iter().take(10).map(String::as_str).collect();
        let url = format!(
            "{}/api/trade2/fetch/{}?query={}",
            self.base,
            ids.join(","),
            search_id
        );
        let resp = self
            .authed(self.http.get(&url))
            .send()
            .map_err(|e| TradeError::Http(e.to_string()))?;
        Self::absorb_headers(&mut self.fetch_limiter, &resp);
        if resp.status().as_u16() == 429 {
            return Err(TradeError::Cooldown(std::time::Duration::from_secs(10)));
        }
        if !resp.status().is_success() {
            return Err(TradeError::Http(format!("fetch status {}", resp.status())));
        }
        parse_fetch(&resp.text().map_err(|e| TradeError::Http(e.to_string()))?)
    }

    /// Result-id list of a saved trade search: GET the saved query json from
    /// `/api/trade2/search/poe2/{league}/{id}` (needs the session cookie -
    /// saved searches belong to an account), then re-POST that query through
    /// the normal search endpoint. The first page of ids is enough for the
    /// live-search differ: a poll only needs to see what is new at the top.
    /// Both requests count against the search limiter - the GET hits the same
    /// rate-limit policy as the POST (verified: the trade site serves both
    /// from the same `/api/trade2/search` family).
    pub fn saved_search_ids(&mut self, league: &str, id: &str) -> Result<Vec<String>, TradeError> {
        if let RateDecision::Wait(d) = self.search_limiter.check() {
            return Err(TradeError::Cooldown(d));
        }
        self.search_limiter.record();
        let url = format!("{}/api/trade2/search/poe2/{}/{}", self.base, league, id);
        let resp = self
            .authed(self.http.get(&url))
            .send()
            .map_err(|e| TradeError::Http(e.to_string()))?;
        Self::absorb_headers(&mut self.search_limiter, &resp);
        if resp.status().as_u16() == 429 {
            return Err(TradeError::Cooldown(std::time::Duration::from_secs(60)));
        }
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().unwrap_or_default();
            return Err(TradeError::Http(api_error("saved search", status, &body)));
        }
        let text = resp.text().map_err(|e| TradeError::Http(e.to_string()))?;
        let body = parse_saved_query(&text)?;

        if let RateDecision::Wait(d) = self.search_limiter.check() {
            return Err(TradeError::Cooldown(d));
        }
        self.search_limiter.record();
        let post_url = format!("{}/api/trade2/search/poe2/{}", self.base, league);
        let resp = self
            .authed(self.http.post(&post_url).json(&body))
            .send()
            .map_err(|e| TradeError::Http(e.to_string()))?;
        Self::absorb_headers(&mut self.search_limiter, &resp);
        if resp.status().as_u16() == 429 {
            return Err(TradeError::Cooldown(std::time::Duration::from_secs(60)));
        }
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().unwrap_or_default();
            return Err(TradeError::Http(api_error("search", status, &body)));
        }
        let sr = parse_search(&resp.text().map_err(|e| TradeError::Http(e.to_string()))?)?;
        Ok(sr.hashes)
    }
}

/// Extracts the re-POSTable search body from a saved-search GET response:
/// `{"query": ..., "sort": ...}`, with the default price sort filled in when
/// the saved search stored none (the POST endpoint requires a sort).
pub fn parse_saved_query(body: &str) -> Result<serde_json::Value, TradeError> {
    let v: serde_json::Value = serde_json::from_str(body)?;
    let query = v
        .get("query")
        .cloned()
        .ok_or_else(|| TradeError::Http("saved search response has no query".into()))?;
    let sort = v
        .get("sort")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({"price": "asc"}));
    Ok(serde_json::json!({"query": query, "sort": sort}))
}

/// Decodes `%XX` percent-escapes byte-wise (league names carry `%20`s in
/// trade URLs). Malformed escapes pass through literally rather than erroring:
/// a pasted URL should parse as far as it can.
fn percent_decode(s: &str) -> String {
    fn hex(b: u8) -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        }
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(hi * 16 + lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Parses a pasted trade-site search URL,
/// `https://www.pathofexile.com/trade2/search/poe2/{league}/{id}`, into
/// (league, search id), URL-decoding the league ("Runes%20of%20Aldur" ->
/// "Runes of Aldur"). Tolerates a trailing slash and query/fragment suffixes;
/// anything not shaped like a two-segment poe2 search path is `None`, which
/// the settings UI surfaces as a bad-URL hint.
pub fn parse_search_url(url: &str) -> Option<(String, String)> {
    let rest = url.trim().split_once("/trade2/search/poe2/")?.1;
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    let mut parts = rest.split('/').filter(|p| !p.is_empty());
    let league = percent_decode(parts.next()?);
    let id = parts.next()?.to_string();
    if parts.next().is_some() || league.is_empty() || id.is_empty() {
        return None;
    }
    Some((league, id))
}

#[cfg(test)]
mod search_url_tests {
    use super::parse_search_url;

    #[test]
    fn parses_the_canonical_search_url_and_decodes_the_league() {
        assert_eq!(
            parse_search_url(
                "https://www.pathofexile.com/trade2/search/poe2/Runes%20of%20Aldur/AbCd12eF"
            ),
            Some(("Runes of Aldur".to_string(), "AbCd12eF".to_string()))
        );
    }

    #[test]
    fn tolerates_trailing_slash_and_query_suffix() {
        assert_eq!(
            parse_search_url("https://www.pathofexile.com/trade2/search/poe2/Standard/xYz9/"),
            Some(("Standard".to_string(), "xYz9".to_string()))
        );
        assert_eq!(
            parse_search_url("https://www.pathofexile.com/trade2/search/poe2/Standard/xYz9?a=1"),
            Some(("Standard".to_string(), "xYz9".to_string()))
        );
    }

    #[test]
    fn rejects_non_search_urls() {
        // Missing id, wrong path family, extra segments, and plain junk all
        // fail closed - the UI marks these red instead of silently polling
        // a nonsense endpoint.
        assert_eq!(parse_search_url("https://www.pathofexile.com/trade2/search/poe2/Standard"), None);
        assert_eq!(parse_search_url("https://www.pathofexile.com/trade/search/League/abc"), None);
        assert_eq!(
            parse_search_url("https://www.pathofexile.com/trade2/search/poe2/a/b/c"),
            None
        );
        assert_eq!(parse_search_url("not a url"), None);
        assert_eq!(parse_search_url(""), None);
    }
}

/// Fetches the live stats catalog (no session needed; requires a
/// browser-like User-Agent past Cloudflare). Callers cache the returned
/// JSON on disk.
pub fn fetch_stats_json() -> Result<String, TradeError> {
    let http = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent(
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) \
             Chrome/126.0 Safari/537.36 khaloni-poe2/0.1",
        )
        .build()
        .map_err(|e| TradeError::Http(e.to_string()))?;
    let resp = http
        .get("https://www.pathofexile.com/api/trade2/data/stats")
        .send()
        .map_err(|e| TradeError::Http(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(TradeError::Http(format!("stats status {}", resp.status())));
    }
    resp.text().map_err(|e| TradeError::Http(e.to_string()))
}
