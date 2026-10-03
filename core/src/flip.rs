//! Flip profiles: finding listed items worth finishing and reselling.
//!
//! A profile names a finished item: a class (or one base), an item level
//! floor and the wanted modifiers with their worst acceptable tiers. A scan
//! runs one trade search for the finished item (its listings are the
//! resale) and one relaxed search per relaxation: each wanted modifier
//! dropped in turn, each minimum tier lowered by one, and the finished
//! item with an open affix. A candidate from those searches is read into
//! an item state, the planner costs finishing it, and its margin is the
//! resale minus its price minus that cost, under the uniform model.
//!
//! Nothing here fetches anything: the searches are built as [`Query`]
//! values the trade client sends, and the fetched listings come back in
//! as the API's JSON.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::craft::data::{range_bounds, CraftData, Domain, Entry};
use crate::craft::model::Model;
use crate::craft::observed::{listing_entries_all, NotRead};
use crate::craft::plan::{plan, Plan};
use crate::craft::sim::SimConfig;
use crate::craft::strategy::{Target, Want};
use crate::craft::types::{AffixKind, ItemState, ModOn, PoolView, Rarity, Source};
use crate::ee2::data::{Better, StatDb};
use crate::ee2::parse::roll_or_minmax_avg;
use crate::ee2::request::category_trade_id;
use crate::listing::{parse_entry, SellerState};
use crate::suggest::{ladder_text, price_text};
use crate::trade::{FilterRole, FilterValue, Query, StatFilter, StatIndex};

/// The margin a profile asks for when its file names none, in percent of
/// what the flip costs (the listing plus the fix).
pub const DEFAULT_MARGIN: f64 = 30.0;

/// Listings fetched per search of a scan.
pub const LISTINGS_PER_SEARCH: u32 = 20;

/// Listings one fetch request returns; the trade API caps a fetch at ten.
pub const LISTINGS_PER_FETCH: u32 = 10;

/// The trade ids of "N empty prefix/suffix modifiers" (the trade site's
/// stats catalog, `pseudo` group).
pub const EMPTY_PREFIX_ID: &str = "pseudo.pseudo_number_of_empty_prefix_mods";
pub const EMPTY_SUFFIX_ID: &str = "pseudo.pseudo_number_of_empty_suffix_mods";

/// Besides the explicit ids, the mod types under which a wanted modifier
/// can sit on a finished item: fixed by a Fracturing Orb, added by an
/// essence, or revealed from a bone. All of them count toward a want.
const OTHER_MOD_TYPES: [&str; 3] = ["fractured", "crafted", "desecrated"];

fn default_margin() -> f64 {
    DEFAULT_MARGIN
}

/// One wanted modifier family and its worst acceptable tier.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Wanted {
    /// The mod database's family ("IncreasedLife").
    pub family: String,
    /// Trade-site numbering: 1 is the best tier, so this tier or a better
    /// one is acceptable.
    pub min_tier: u8,
}

/// A finished item worth looking for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub name: String,
    /// The item class as the base item data names it ("Body Armour").
    pub class: String,
    /// One base of the class, when the profile is for that base only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(default)]
    pub min_ilvl: u32,
    pub wants: Vec<Wanted>,
    /// The least margin worth a flip, in percent of the listing's price
    /// plus the fix.
    #[serde(default = "default_margin")]
    pub margin: f64,
}

impl Profile {
    /// What makes the profile unusable, before any data is consulted.
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("the profile has no name".to_string());
        }
        if self.class.trim().is_empty() {
            return Err("the profile names no item class".to_string());
        }
        if self.base.as_deref().is_some_and(|b| b.trim().is_empty()) {
            return Err("the base is empty; leave it out to search the whole class".to_string());
        }
        if self.wants.is_empty() {
            return Err("the profile wants no modifier".to_string());
        }
        let mut seen = HashSet::new();
        for w in &self.wants {
            if w.family.trim().is_empty() {
                return Err("a wanted modifier names no family".to_string());
            }
            if w.min_tier == 0 {
                return Err(format!("{}: min_tier is 0, and tiers count from 1", w.family));
            }
            if !seen.insert(w.family.as_str()) {
                return Err(format!("{} is wanted twice", w.family));
            }
        }
        if !self.margin.is_finite() || self.margin < 0.0 {
            return Err(format!("the margin {} is not a percentage of 0 or more", self.margin));
        }
        Ok(())
    }
}

/// A profile of the file that could not be loaded, and why.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileError {
    /// Its position among the file's profiles, from 0; `None` when the
    /// file as a whole could not be read.
    pub index: Option<usize>,
    /// Its name, when it has a readable one.
    pub name: Option<String>,
    pub reason: String,
}

impl std::fmt::Display for ProfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.index, &self.name) {
            (Some(i), Some(name)) => write!(f, "profile {} (\"{name}\"): {}", i + 1, self.reason),
            (Some(i), None) => write!(f, "profile {}: {}", i + 1, self.reason),
            (None, _) => write!(f, "the profiles file: {}", self.reason),
        }
    }
}

/// What a profiles file gave: every profile that loaded, and every one
/// that did not with its reason.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LoadedProfiles {
    pub profiles: Vec<Profile>,
    pub errors: Vec<ProfileError>,
}

#[derive(Serialize)]
struct ProfilesFile<'a> {
    profile: &'a [Profile],
}

/// Reads `profiles.toml`: `[[profile]]` tables. Each profile is read on its
/// own, so one malformed profile is reported with its position and reason
/// while the others still load.
pub fn profiles_from_toml(text: &str) -> LoadedProfiles {
    let mut out = LoadedProfiles::default();
    let file_error = |reason: String| ProfileError { index: None, name: None, reason };
    let table: toml::Table = match text.parse() {
        Ok(t) => t,
        Err(e) => {
            out.errors.push(file_error(format!("it is not valid TOML: {}", e.message())));
            return out;
        }
    };
    for key in table.keys().filter(|k| k.as_str() != "profile") {
        out.errors.push(file_error(format!("the key \"{key}\" is not a profile; profiles are [[profile]] tables")));
    }
    let entries = match table.get("profile") {
        None => return out,
        Some(toml::Value::Array(a)) => a.as_slice(),
        Some(_) => {
            out.errors.push(file_error("\"profile\" is not a list of [[profile]] tables".to_string()));
            return out;
        }
    };
    let mut names: HashSet<String> = HashSet::new();
    for (i, value) in entries.iter().enumerate() {
        let name = value.get("name").and_then(toml::Value::as_str).map(str::to_string);
        let error = |reason: String| ProfileError { index: Some(i), name: name.clone(), reason };
        let profile = match Profile::deserialize(value.clone()) {
            Ok(p) => p,
            Err(e) => {
                out.errors.push(error(e.message().to_string()));
                continue;
            }
        };
        if let Err(reason) = profile.validate() {
            out.errors.push(error(reason));
            continue;
        }
        if !names.insert(profile.name.clone()) {
            out.errors.push(error("an earlier profile has the same name".to_string()));
            continue;
        }
        out.profiles.push(profile);
    }
    out
}

/// The profiles as `profiles.toml` holds them.
pub fn to_toml(profiles: &[Profile]) -> Result<String, String> {
    toml::to_string(&ProfilesFile { profile: profiles }).map_err(|e| format!("the profiles do not write as TOML: {e}"))
}

// --- from a profile to trade searches ---

/// One line of a wanted modifier as the trade site indexes it.
#[derive(Debug, Clone, PartialEq)]
pub struct StatLine {
    /// The line with its numbers as `#` ("#% to Fire Resistance").
    pub text: String,
    /// The explicit trade ids first, then the same stat's fractured,
    /// crafted and desecrated ids.
    pub ids: Vec<String>,
}

/// A wanted family resolved against the mod database and the trade stats.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedWant {
    pub family: String,
    pub kind: AffixKind,
    pub min_tier: u8,
    /// The family's worst tier on the profile's bases.
    pub lowest_tier: u8,
    /// One per line of the family's text (a hybrid family has two).
    pub lines: Vec<StatLine>,
    /// Per tier (index 0 is tier 1), per line: the least value that tier
    /// rolls on any of the profile's bases.
    floors: Vec<Vec<f64>>,
}

impl ResolvedWant {
    /// The family as a search label: its lines joined.
    pub fn label(&self) -> String {
        self.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join(" / ")
    }

    /// The least value per line of `tier`, or `None` past the lowest tier.
    pub fn floor(&self, tier: u8) -> Option<&[f64]> {
        self.floors.get(usize::from(tier).checked_sub(1)?).map(Vec::as_slice)
    }

    fn filters(&self, tier: u8) -> Vec<StatFilter> {
        let floor = self.floor(tier).unwrap_or(&[]);
        self.lines
            .iter()
            .zip(floor)
            .map(|(line, min)| StatFilter {
                id: line.ids[0].clone(),
                alt_ids: line.ids[1..].to_vec(),
                value: FilterValue { min: Some(*min), max: None },
                disabled: false,
                role: FilterRole::Stat,
                not_id: None,
            })
            .collect()
    }
}

/// A profile resolved for searching, reading listings and planning.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    pub profile: Profile,
    pub wants: Vec<ResolvedWant>,
    /// The planner's target: every want at its tier or better.
    pub target: Target,
    /// The trade category searched when the profile names no base.
    pub category: Option<String>,
    /// Whether the open-affix search can be sent, and why not.
    pub open_affix: Result<(), String>,
}

/// A line of the mod database's text as a trade matcher: each number or
/// `(lo-hi)` range, with its sign, becomes `#`, and the value the least
/// roll of the line gives (the low ends, averaged the way EE2 averages a
/// line's rolls). `None` when the line holds no number.
fn matcher_of(line: &str) -> Option<(String, f64)> {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len());
    let mut lows: Vec<f64> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let after_word = i > 0 && (chars[i - 1].is_alphanumeric() || chars[i - 1] == ')');
        let mut j = i;
        let mut sign = 1.0;
        if !after_word && matches!(chars[j], '+' | '-') && j + 1 < chars.len() {
            if chars[j] == '-' {
                sign = -1.0;
            }
            j += 1;
        }
        if !after_word && chars[j] == '(' {
            if let Some(close) = chars[j..].iter().position(|c| *c == ')').map(|n| j + n) {
                let inner: String = chars[j + 1..close].iter().collect();
                if let Some((a, b)) = range_bounds(&inner) {
                    if let (Ok(a), Ok(b)) = (a.parse::<f64>(), b.parse::<f64>()) {
                        lows.push((sign * a).min(sign * b));
                        out.push('#');
                        i = close + 1;
                        continue;
                    }
                }
            }
        }
        if !after_word && chars[j].is_ascii_digit() {
            let start = j;
            while j < chars.len() && (chars[j].is_ascii_digit() || (chars[j] == '.' && j > start)) {
                j += 1;
            }
            let number: String = chars[start..j].iter().collect();
            if let Ok(n) = number.trim_end_matches('.').parse::<f64>() {
                lows.push(sign * n);
                out.push('#');
                i = j;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    let value = roll_or_minmax_avg(&lows)?;
    Some((out, value))
}

/// The trade line behind one matcher, or why it cannot be searched as "at
/// least this much". Only ids the trade site's own catalog lists are kept:
/// EE2's stat list names a few the site does not index (a crafted twin of
/// a local stat), and one unknown id makes the site refuse the search.
fn stat_line(matcher: &str, stats: &StatDb, catalog: &StatIndex) -> Result<StatLine, String> {
    let (stat, m) = stats.by_match_str(matcher).ok_or_else(|| format!("no trade stat reads \"{matcher}\""))?;
    let explicit = stat
        .ids("explicit")
        .filter(|ids| !ids.is_empty())
        .ok_or_else(|| format!("the trade stat \"{matcher}\" has no explicit id"))?;
    if m.negate || stat.inverted {
        return Err(format!(
            "\"{matcher}\" is indexed with mirrored bounds, which a profile search does not send"
        ));
    }
    if stat.better != Better::Positive || m.value.is_some() || stat.option {
        return Err(format!("\"{matcher}\" is not a figure where more is better"));
    }
    let listed = |id: &&String| catalog.entry_by_id(id).is_some();
    let mut ids: Vec<String> = explicit.iter().filter(listed).cloned().collect();
    if ids.is_empty() {
        return Err(format!(
            "the trade site's catalog lists none of the explicit ids of \"{matcher}\" ({})",
            explicit.join(", ")
        ));
    }
    for t in OTHER_MOD_TYPES {
        for id in stat.ids(t).into_iter().flatten().filter(listed) {
            if !ids.contains(id) {
                ids.push(id.clone());
            }
        }
    }
    Ok(StatLine { text: matcher.to_string(), ids })
}

/// Resolves one wanted family on `bases`.
fn resolve_want(
    want: &Wanted,
    bases: &[&crate::craft::data::Base],
    data: &CraftData,
    (stats, catalog): (&StatDb, &StatIndex),
    on: &str,
) -> Result<ResolvedWant, String> {
    let family = &want.family;
    let entries: Vec<&Entry> = data.entries().iter().filter(|e| &e.family == family && !e.essence_only).collect();
    if entries.is_empty() {
        return Err(format!("{family} is not a family of the mod database"));
    }
    let rolls_on = |e: &Entry| bases.iter().any(|b| e.eligible(b.tags.iter().map(String::as_str)));
    // A family orbs roll is searched by its rolled tiers. One only a
    // desecration or an essence adds is searched by those: a listing's card
    // joins its desecrated and crafted lines as well, so such a listing is
    // counted when it meets the profile.
    let rolled: Vec<&Entry> = entries.iter().copied().filter(|e| e.domain == Domain::Item && rolls_on(e)).collect();
    let entries: Vec<&Entry> = if rolled.is_empty() {
        entries
            .into_iter()
            .filter(|e| (e.domain == Domain::Desecrated && rolls_on(e)) || (e.domain == Domain::Item && !e.rollable_anywhere()))
            .collect()
    } else {
        rolled
    };
    if entries.is_empty() {
        return Err(format!("{family} does not roll on {on}"));
    }
    let kind = entries[0].kind;
    if entries.iter().any(|e| e.kind != kind) {
        return Err(format!("{family} is both a prefix and a suffix on {on}"));
    }

    let parse = |e: &Entry| -> Result<Vec<(String, f64)>, String> {
        e.text
            .lines()
            .map(|l| matcher_of(l).ok_or_else(|| format!("{family}: the line \"{l}\" of {} holds no number", e.id)))
            .collect()
    };
    let first = parse(entries[0])?;
    let lines: Vec<StatLine> = first
        .iter()
        .map(|(m, _)| stat_line(m, stats, catalog))
        .collect::<Result<_, _>>()
        .map_err(|r| format!("{family}: {r}"))?;

    let mut floors: Vec<Vec<f64>> = Vec::new();
    for base in bases {
        let tags: Vec<&str> = base.tags.iter().map(String::as_str).collect();
        for e in entries.iter().filter(|e| e.eligible(tags.iter().copied()) || !e.rollable_anywhere()) {
            let parsed = parse(e)?;
            if parsed.len() != lines.len() || parsed.iter().zip(&lines).any(|((m, _), l)| *m != l.text) {
                return Err(format!("{family}: its tiers are worded differently ({} against {})", e.id, entries[0].id));
            }
            let tier = usize::from(data.tier_on(e, &tags));
            if floors.len() < tier {
                floors.resize(tier, vec![f64::INFINITY; lines.len()]);
            }
            for (slot, (_, v)) in floors[tier - 1].iter_mut().zip(&parsed) {
                *slot = slot.min(*v);
            }
        }
    }
    let lowest_tier = u8::try_from(floors.len()).unwrap_or(u8::MAX);
    if want.min_tier > lowest_tier {
        return Err(format!("{family} has {lowest_tier} tiers on {on}; tier {} does not exist", want.min_tier));
    }
    Ok(ResolvedWant { family: family.clone(), kind, min_tier: want.min_tier, lowest_tier, lines, floors })
}

/// Resolves every want of `profile` against the mod database, EE2's stat
/// list (text to trade ids) and the trade site's stats catalog (the ids it
/// indexes). Every family that cannot be searched is reported with its
/// reason; no trade id is ever guessed.
pub fn resolve(profile: &Profile, data: &CraftData, stats: &StatDb, catalog: &StatIndex) -> Result<Resolved, Vec<String>> {
    profile.validate().map_err(|r| vec![r])?;
    let (bases, on, category) = match &profile.base {
        Some(name) => {
            let base = data.base(name).ok_or_else(|| vec![format!("{name} is not a base of the base item data")])?;
            if base.class != profile.class {
                return Err(vec![format!("{name} is a {}, not a {}", base.class, profile.class)]);
            }
            (vec![base], name.clone(), None)
        }
        None => {
            let category = category_trade_id(&profile.class)
                .ok_or_else(|| vec![format!("{} has no trade category to search", profile.class)])?;
            let bases: Vec<_> = data.bases().iter().filter(|b| b.class == profile.class).collect();
            if bases.is_empty() {
                return Err(vec![format!("no base of the base item data is a {}", profile.class)]);
            }
            (bases, profile.class.clone(), Some(category.to_string()))
        }
    };
    let mut wants = Vec::new();
    let mut reasons = Vec::new();
    for w in &profile.wants {
        match resolve_want(w, &bases, data, (stats, catalog), &on) {
            Ok(r) => wants.push(r),
            Err(reason) => reasons.push(reason),
        }
    }
    if !reasons.is_empty() {
        return Err(reasons);
    }
    let target = Target {
        wants: wants.iter().map(|w| Want { family: w.family.clone(), kind: w.kind, min_tier: w.min_tier }).collect(),
    };
    let open_affix = [EMPTY_PREFIX_ID, EMPTY_SUFFIX_ID]
        .into_iter()
        .find(|id| catalog.entry_by_id(id).is_none())
        .map_or(Ok(()), |id| Err(format!("the trade site's catalog does not list {id}, so no open affix is searched")));
    Ok(Resolved { profile: profile.clone(), wants, target, category, open_affix })
}

/// What one search of a scan stands for.
#[derive(Debug, Clone, PartialEq)]
pub enum Relaxation {
    /// The finished profile: its listings are the resale.
    Resale,
    /// The profile without this family.
    Dropped(String),
    /// The profile with this family's minimum tier one worse.
    Lowered { family: String, tier: u8 },
    /// The profile with an open prefix or suffix required.
    OpenAffix,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RelaxedQuery {
    pub relaxation: Relaxation,
    pub label: String,
    pub query: Query,
}

/// The searches of one scan: the resale search first, then one per
/// relaxation.
#[derive(Debug, Clone, PartialEq)]
pub struct Relaxations {
    pub queries: Vec<RelaxedQuery>,
    /// Relaxations left out, each with the reason.
    pub skipped: Vec<String>,
}

/// The search every query of a profile shares, in the shape EE2's request
/// builder gives a rare of the same class: category or base (never both),
/// non-unique, clean, one listing per seller.
fn base_query(resolved: &Resolved) -> Query {
    let p = &resolved.profile;
    Query {
        category: resolved.category.clone(),
        category_enabled: resolved.category.is_some(),
        category_replaces_type: true,
        type_name: p.base.clone(),
        rarity: Some("nonunique".to_string()),
        ilvl_min: (p.min_ilvl > 0).then_some(p.min_ilvl),
        corrupted: Some(false),
        mirrored: Some(false),
        sanctified: Some(false),
        collapse: true,
        ..Default::default()
    }
}

/// The resale search and one relaxed search per relaxation. A want already
/// at its family's lowest tier is not lowered, and `skipped` says so.
pub fn relaxations(resolved: &Resolved) -> Relaxations {
    let with = |tiers: &[Option<u8>], open: bool| {
        let mut q = base_query(resolved);
        for (w, tier) in resolved.wants.iter().zip(tiers) {
            if let Some(t) = tier {
                q.filters.extend(w.filters(*t));
            }
        }
        if open {
            q.filters.push(StatFilter {
                id: EMPTY_PREFIX_ID.to_string(),
                alt_ids: vec![EMPTY_SUFFIX_ID.to_string()],
                value: FilterValue { min: Some(1.0), max: None },
                disabled: false,
                role: FilterRole::Stat,
                not_id: None,
            });
        }
        q
    };
    let full: Vec<Option<u8>> = resolved.wants.iter().map(|w| Some(w.min_tier)).collect();
    let on = resolved.profile.base.as_deref().unwrap_or(&resolved.profile.class);
    let mut queries = vec![RelaxedQuery {
        relaxation: Relaxation::Resale,
        label: "resale: the finished profile".to_string(),
        query: with(&full, false),
    }];
    let mut skipped = Vec::new();
    for (i, w) in resolved.wants.iter().enumerate() {
        let mut tiers = full.clone();
        tiers[i] = None;
        queries.push(RelaxedQuery {
            relaxation: Relaxation::Dropped(w.family.clone()),
            label: format!("without {}", w.label()),
            query: with(&tiers, false),
        });
    }
    for (i, w) in resolved.wants.iter().enumerate() {
        if w.min_tier >= w.lowest_tier {
            skipped.push(format!(
                "{} is already at its lowest tier (T{}) on {on}, so it is not lowered",
                w.label(),
                w.lowest_tier
            ));
            continue;
        }
        let mut tiers = full.clone();
        tiers[i] = Some(w.min_tier + 1);
        queries.push(RelaxedQuery {
            relaxation: Relaxation::Lowered { family: w.family.clone(), tier: w.min_tier + 1 },
            label: format!("{} at T{} instead of T{}", w.label(), w.min_tier + 1, w.min_tier),
            query: with(&tiers, false),
        });
    }
    match &resolved.open_affix {
        Ok(()) => queries.push(RelaxedQuery {
            relaxation: Relaxation::OpenAffix,
            label: "with an open prefix or suffix".to_string(),
            query: with(&full, true),
        }),
        Err(reason) => skipped.push(reason.clone()),
    }
    Relaxations { queries, skipped }
}

/// The trade requests a scan makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestCost {
    pub searches: u32,
    pub fetches: u32,
}

impl RequestCost {
    /// `searches` searches, each followed by the fetches that bring back
    /// [`LISTINGS_PER_SEARCH`] listings.
    pub fn of(searches: u32) -> RequestCost {
        RequestCost { searches, fetches: searches * LISTINGS_PER_SEARCH.div_ceil(LISTINGS_PER_FETCH) }
    }

    /// "this scan: 5 searches, 10 fetches; budget 27/30 free".
    pub fn statement(&self, free: u32, total: u32) -> String {
        let count = |n: u32, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        format!(
            "this scan: {}, {}; budget {free}/{total} free",
            count(self.searches, "search", "searches"),
            count(self.fetches, "fetch", "fetches")
        )
    }
}

/// What a scan of these searches costs.
pub fn scan_cost(relaxations: &Relaxations) -> RequestCost {
    RequestCost::of(u32::try_from(relaxations.queries.len()).unwrap_or(u32::MAX))
}

// --- reading listings ---

/// Prices in the caller's display unit.
pub struct Market<'a> {
    /// A listing's (amount, currency id) in the unit; `None` when the
    /// currency has no rate.
    pub convert: &'a dyn Fn(f64, &str) -> Option<f64>,
    /// The unit's short name ("div").
    pub unit: &'a str,
}

/// A fetched listing's card as an item state. Every modifier of the card
/// goes through the same join the observed store uses; crafted and
/// desecrated lines are joined too, since they hold slots and can meet a
/// want. A rolled modifier that does not join makes the card unreadable
/// (the state would be wrong); a chosen one that does not join is kept as
/// a slot of unknown family.
pub fn card_state(entry: &Value, data: &CraftData) -> Result<ItemState, String> {
    struct CardMod {
        name: String,
        tier: String,
        level: Option<u64>,
        source: Source,
    }
    let mut mods: Vec<CardMod> = Vec::new();
    if let Some(lines) = entry["item"]["explicitMods"].as_array() {
        for line in lines {
            let source = match line["domain"].as_str() {
                Some("crafted") => Source::Crafted,
                Some("desecrated") => Source::Desecrated,
                Some("fractured") => Source::Fractured,
                _ => Source::Random,
            };
            for m in line["mods"].as_array().into_iter().flatten() {
                mods.push(CardMod {
                    name: m["name"].as_str().unwrap_or("").to_string(),
                    tier: m["tier"].as_str().unwrap_or("").to_string(),
                    level: m["level"].as_u64(),
                    source,
                });
            }
        }
    }
    let read = listing_entries_all(entry, data).map_err(|e| match e {
        NotRead::Gone => "the listing is gone".to_string(),
        NotRead::NotRolled(reason) => reason,
        NotRead::UnknownBase(base) => format!("the base \"{base}\" is not in the base item data"),
    })?;
    let item = &entry["item"];
    let base_name = [&item["baseType"], &item["typeLine"]]
        .into_iter()
        .filter_map(Value::as_str)
        .find(|s| !s.is_empty())
        .unwrap_or("");
    let base = data.base(base_name).ok_or_else(|| format!("the base \"{base_name}\" is not in the base item data"))?;
    let tags: Vec<&str> = base.tags.iter().map(String::as_str).collect();
    let badge_kind = |tier: &str| match tier.get(..1) {
        Some("P") => Some(AffixKind::Prefix),
        Some("S") => Some(AffixKind::Suffix),
        _ => None,
    };

    let mut on_item: Vec<ModOn> = Vec::new();
    for (id, kind) in &read.listing.entries {
        let entry = data.entry(id).ok_or_else(|| format!("{id} joined but is not in the mod database"))?;
        let source = mods
            .iter()
            .find(|m| {
                m.name == entry.name
                    && m.level == Some(u64::from(entry.required_level))
                    && badge_kind(&m.tier) == Some(*kind)
            })
            .map_or(Source::Random, |m| m.source);
        on_item.push(data.candidate(entry, &tags).to_mod(source));
    }
    for u in &read.unjoined {
        let chosen = mods
            .iter()
            .find(|m| m.name == u.name && m.tier == u.tier && matches!(m.source, Source::Crafted | Source::Desecrated));
        let Some(m) = chosen else {
            let tier = if u.tier.is_empty() { String::new() } else { format!(" {}", u.tier) };
            return Err(format!("the modifier \"{}\"{tier} does not join the mod database: {}", u.name, u.reason));
        };
        let kind = badge_kind(&m.tier)
            .ok_or_else(|| format!("the modifier \"{}\" has no prefix or suffix badge", m.name))?;
        on_item.push(ModOn {
            entry_id: None,
            family: m.name.clone(),
            groups: Vec::new(),
            kind,
            tier: m.tier.get(1..).and_then(|t| t.parse().ok()),
            required_level: m.level.and_then(|l| u32::try_from(l).ok()),
            adds_tags: Vec::new(),
            source: m.source,
            text: m.name.clone(),
        });
    }
    let rarity = match item["rarity"].as_str() {
        Some("Magic") => Rarity::Magic,
        _ => Rarity::Rare,
    };
    Ok(ItemState {
        class: base.class.clone(),
        base: base.name.clone(),
        base_tags: base.tags.clone(),
        item_level: item["ilvl"].as_u64().and_then(|l| u32::try_from(l).ok()).unwrap_or(0),
        rarity,
        corrupted: item["corrupted"].as_bool().unwrap_or(false),
        // The searches ask for neither, so a listing they return is
        // neither.
        mirrored: false,
        sanctified: false,
        mods: on_item,
        sockets: item["sockets"].as_array().map_or(0, |s| u32::try_from(s.len()).unwrap_or(0)),
    })
}

/// Whether `state` is at or above the profile; the reason when it is not.
pub fn meets(resolved: &Resolved, state: &ItemState) -> Result<(), String> {
    let p = &resolved.profile;
    if state.class != p.class {
        return Err(format!("a {}, not a {}", state.class, p.class));
    }
    if let Some(base) = p.base.as_deref().filter(|b| *b != state.base) {
        return Err(format!("a {}, not a {base}", state.base));
    }
    if state.item_level < p.min_ilvl {
        return Err(format!("item level {} is under {}", state.item_level, p.min_ilvl));
    }
    if state.corrupted {
        return Err("corrupted".to_string());
    }
    let missing: Vec<String> =
        resolved.target.missing(state).iter().map(|w| format!("{} at T{} or better", w.family, w.min_tier)).collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!("lacks {}", missing.join(", ")))
    }
}

/// The resale: the finished search's listings that meet the profile.
#[derive(Debug, Clone, PartialEq)]
pub struct Resale {
    /// The cheapest counted listing, in the market's unit.
    pub cheapest: Option<f64>,
    /// Every counted listing's price, cheapest first.
    pub prices: Vec<f64>,
    pub listings_counted: usize,
    /// Listings read that fall short of the profile.
    pub below_profile: usize,
    /// Listings that meet the profile but whose price cannot be read in
    /// the unit, and listings whose card could not be read.
    pub not_counted: usize,
    pub unit: String,
    pub text: String,
}

/// Reads the finished search's fetched listings: only those at or above
/// the profile count toward the resale.
pub fn resale(fetched: &[Value], resolved: &Resolved, data: &CraftData, market: &Market) -> Resale {
    let mut prices = Vec::new();
    let (mut below, mut not_counted) = (0, 0);
    for entry in fetched {
        let Some(view) = parse_entry(entry, 0, "") else { continue };
        // A normal or unique item rolls no affixes, so it holds no want.
        if !matches!(entry["item"]["rarity"].as_str(), Some("Magic" | "Rare")) {
            below += 1;
            continue;
        }
        match card_state(entry, data) {
            Err(_) => not_counted += 1,
            Ok(state) if meets(resolved, &state).is_err() => below += 1,
            Ok(_) => match view.price.and_then(|(amount, currency)| (market.convert)(amount, &currency)) {
                Some(p) => prices.push(p),
                None => not_counted += 1,
            },
        }
    }
    prices.sort_by(f64::total_cmp);
    let mut text = if prices.is_empty() {
        "no listing of the finished search meets the profile".to_string()
    } else {
        ladder_text(&prices, None, market.unit)
    };
    if below > 0 {
        text.push_str(&format!("; {below} below the profile not counted"));
    }
    if not_counted > 0 {
        text.push_str(&format!("; {not_counted} unreadable or unpriced not counted"));
    }
    Resale {
        cheapest: prices.first().copied(),
        listings_counted: prices.len(),
        prices,
        below_profile: below,
        not_counted,
        unit: market.unit.to_string(),
        text,
    }
}

/// The margin of a flip: the resale's cheapest listing minus the
/// listing's price minus the fix. `fix` is the uniform model's figure; the
/// observed one is worked out separately and never blended in.
pub fn margin(listing_price: f64, fix: Option<f64>, resale: &Resale) -> Result<f64, String> {
    let fix = fix.ok_or_else(|| "fix cost unknown".to_string())?;
    let sell = resale
        .cheapest
        .ok_or_else(|| "resale unknown: no listing of the finished search meets the profile".to_string())?;
    Ok(sell - listing_price - fix)
}

// --- candidates ---

/// What finishing a candidate costs under one model.
#[derive(Debug, Clone, PartialEq)]
pub struct Fix {
    /// The cost of one finished item by the cheapest strategy.
    pub cost: f64,
    /// Half the runs of that strategy spend under this.
    pub median: Option<f64>,
    /// The strategy's library id ("orb-chain"); "none" when the item
    /// already meets the profile.
    pub strategy: String,
}

/// What the planner needs from the caller.
pub struct Planner<'a> {
    pub pool: &'a (dyn PoolView + Sync),
    /// The observed model for the profile's class, when its sample is
    /// large enough.
    pub observed: Option<&'a Model>,
    /// The price of one item by its in-game name, in the market's unit.
    pub prices: &'a (dyn Fn(&str) -> Option<f64> + Sync),
    pub config: &'a SimConfig,
}

/// One listing of a relaxed search, costed.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub listing_id: String,
    pub seller: String,
    pub seller_state: SellerState,
    /// The listing's price in the market's unit.
    pub price: Result<f64, String>,
    pub item: Result<ItemState, String>,
    /// Under the uniform model.
    pub fix: Result<Fix, String>,
    /// Under the observed model, when the caller has one.
    pub fix_observed: Option<Result<Fix, String>>,
    /// Resale minus price minus the uniform fix: the figure candidates are
    /// ranked by.
    pub margin: Result<f64, String>,
    pub margin_observed: Option<Result<f64, String>>,
    /// The observed model's label, beside its figure.
    pub observed_label: Option<String>,
    /// The margin reaches the profile's percentage of price plus fix.
    pub meets_margin: bool,
    pub plan: Option<Plan>,
    pub text: String,
}

fn strip_unknown(reason: &str) -> &str {
    reason.strip_prefix("unknown: ").unwrap_or(reason)
}

/// The cheapest strategy's figure under one model, from the plan's ranked
/// strategies (`uniform` picks the uniform figures kept beside an observed
/// headline).
fn cheapest_fix(plan: &Plan, uniform_beside: bool) -> Result<Fix, String> {
    let costed = |r: &crate::craft::plan::Ranked| if uniform_beside { r.uniform.clone() } else { Some(r.costed.clone()) };
    let all: Vec<_> = plan.strategies.iter().filter_map(costed).collect();
    let best = all
        .iter()
        .filter_map(|c| c.per_finished.map(|cost| (cost, c)))
        .min_by(|a, b| a.0.total_cmp(&b.0));
    match best {
        Some((cost, c)) => Ok(Fix { cost, median: c.median, strategy: c.id.to_string() }),
        None => Err(all
            .iter()
            .flat_map(|c| c.unknowns.iter())
            .next()
            .map(|u| strip_unknown(u).to_string())
            .unwrap_or_else(|| "no crafting strategy reaches the profile from this item".to_string())),
    }
}

/// Why no strategy can finish `state` when a missing want's family cannot
/// roll on its base at all.
fn cannot_roll(resolved: &Resolved, state: &ItemState, data: &CraftData) -> Option<String> {
    let tags: Vec<&str> = state.base_tags.iter().map(String::as_str).collect();
    resolved.target.missing(state).into_iter().find_map(|w| {
        let rolls = data
            .entries()
            .iter()
            .any(|e| e.family == w.family && e.kind == w.kind && e.eligible(tags.iter().copied()));
        let label = resolved.wants.iter().find(|r| r.family == w.family).map_or(w.family.clone(), ResolvedWant::label);
        (!rolls).then(|| format!("{label} does not roll on {}", state.base))
    })
}

/// Reads one listing of a relaxed search as a candidate: its card becomes
/// an item state, the planner runs from it to the profile, and its margin
/// is worked out against `resale`. `None` for the null the API sends for a
/// listing that vanished.
pub fn candidate(
    entry: &Value,
    resolved: &Resolved,
    resale: &Resale,
    data: &CraftData,
    planner: &Planner,
    market: &Market,
) -> Option<Candidate> {
    let view = parse_entry(entry, 0, "")?;
    let listing_id = entry["id"].as_str().unwrap_or("").to_string();
    let price = match &view.price {
        None => Err("the listing has no price".to_string()),
        Some((amount, currency)) => (market.convert)(*amount, currency)
            .ok_or_else(|| format!("its price in {currency} cannot be read in {}", market.unit)),
    };
    let item = card_state(entry, data);

    let mut the_plan = None;
    let (fix, fix_observed) = match &item {
        Err(reason) => {
            let reason = format!("the card cannot be read: {reason}");
            (Err(reason.clone()), planner.observed.map(|_| Err(reason)))
        }
        Ok(state) if state.locked() => {
            let reason = "the item is corrupted and takes no further crafting".to_string();
            (Err(reason.clone()), planner.observed.map(|_| Err(reason)))
        }
        Ok(state) if resolved.target.holds(state) => {
            let none = Fix { cost: 0.0, median: Some(0.0), strategy: "none".to_string() };
            (Ok(none.clone()), planner.observed.map(|_| Ok(none)))
        }
        Ok(state) => match cannot_roll(resolved, state, data) {
            Some(reason) => (Err(reason.clone()), planner.observed.map(|_| Err(reason))),
            None => {
                let p =
                    plan(state, &resolved.target, planner.pool, planner.observed, planner.prices, None, planner.config);
                let uniform = cheapest_fix(&p, planner.observed.is_some());
                let observed = planner.observed.map(|_| cheapest_fix(&p, false));
                the_plan = Some(p);
                (uniform, observed)
            }
        },
    };

    let margin_under = |fix: &Result<Fix, String>| -> Result<f64, String> {
        let listed = price.clone()?;
        let fix = match fix {
            Ok(f) => Some(f.cost),
            Err(reason) => return Err(format!("fix cost unknown: {reason}")),
        };
        margin(listed, fix, resale)
    };
    let margin_uniform = margin_under(&fix);
    let margin_observed = fix_observed.as_ref().map(margin_under);
    let meets_margin = match (&margin_uniform, &price, &fix) {
        (Ok(m), Ok(p), Ok(f)) => *m >= resolved.profile.margin / 100.0 * (p + f.cost),
        _ => false,
    };
    let observed_label = planner.observed.map(Model::label);

    let unit = market.unit;
    let mut parts = Vec::new();
    parts.push(match &price {
        Ok(p) => format!("listed {} {unit} by {} ({})", price_text(*p), view.seller, view.state.as_str()),
        Err(reason) => format!("price unknown: {reason}, by {} ({})", view.seller, view.state.as_str()),
    });
    parts.push(match &fix {
        Ok(f) if f.strategy == "none" => "no fix: it meets the profile".to_string(),
        Ok(f) => match f.median {
            Some(m) => format!("fix ~{} {unit} ({}, half under {})", price_text(f.cost), f.strategy, price_text(m)),
            None => format!("fix ~{} {unit} ({})", price_text(f.cost), f.strategy),
        },
        Err(reason) => format!("fix cost unknown: {reason}"),
    });
    parts.push(match resale.cheapest {
        Some(c) => {
            let n = resale.listings_counted;
            format!("resale cheapest {} {unit} ({n} listing{})", price_text(c), if n == 1 { "" } else { "s" })
        }
        None => "resale unknown: no listing of the finished search meets the profile".to_string(),
    });
    if let Ok(m) = &margin_uniform {
        let mut line = format!("margin {} {unit} under uniform", price_text(*m));
        if let (Some(Ok(o)), Some(label)) = (&margin_observed, &observed_label) {
            line.push_str(&format!(", {} {unit} {label}", price_text(*o)));
        }
        if !meets_margin {
            line.push_str(&format!(", under the profile's {}%", price_text(resolved.profile.margin)));
        }
        parts.push(line);
    }

    Some(Candidate {
        listing_id,
        seller: view.seller.clone(),
        seller_state: view.state,
        price,
        item,
        fix,
        fix_observed,
        margin: margin_uniform,
        margin_observed,
        observed_label,
        meets_margin,
        plan: the_plan,
        text: parts.join(" · "),
    })
}

/// Candidates ranked by their uniform margin, largest first; those without
/// a margin (an unknown fix, price or resale) last, in the order given,
/// each with its reason in its line.
pub fn rank(mut candidates: Vec<Candidate>) -> Vec<Candidate> {
    candidates.sort_by(|a, b| match (&a.margin, &b.margin) {
        (Ok(x), Ok(y)) => y.total_cmp(x),
        (Ok(_), Err(_)) => std::cmp::Ordering::Less,
        (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
        (Err(_), Err(_)) => std::cmp::Ordering::Equal,
    });
    candidates
}
