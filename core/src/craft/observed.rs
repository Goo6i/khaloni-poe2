//! Entry frequencies recorded from the listings price checks fetched.
//!
//! Each fetched listing carries, for every modifier on its card, the affix
//! name, the trade-site tier ("P1", "S3") and the modifier's required
//! level. Name, kind and level pick the mod-database entry; the base's tags
//! settle the few names that two families share at one level, and the tier
//! the listing shows must be the tier the data gives that entry on the
//! base, or the modifier is not counted. A modifier that does not join
//! exactly is reported as unjoined, never matched to a near miss; the
//! listing still counts with the modifiers that did join, so an unjoined
//! modifier is missing from the shares' denominators, not guessed into
//! them.
//!
//! Only modifiers the game rolled are counted: explicit and fractured ones.
//! A crafted line (essence or alloy) and a desecrated line (one of three
//! reveals the owner picked) were chosen, so their frequencies measure the
//! seller's choices and would skew the pool's shares.
//!
//! The counts are per league and per item class, bounded oldest-first, and
//! a class yields a model only once enough listings back it. Pure: the text
//! form is built and read here, the file is the app's.

use std::collections::{HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::data::{CraftData, Domain, Entry};
use super::model::Model;
use super::types::{AffixKind, EntryId};

/// Listings of a class needed before its observed model exists.
pub const MIN_LISTINGS: u32 = 200;

/// Listings kept across every class of a league. A calibration gathers 200
/// listings of one class, and about twenty equipment classes are crafted,
/// so this holds a sample several times the minimum for each of them. A
/// line runs near 200 bytes, which keeps the league's file around 4 MB and
/// its load well under a second. Dropping the oldest first keeps the sample
/// on the market as it is now rather than as it opened.
pub const MAX_LISTINGS: usize = 20_000;

/// The random modifiers of one listing, joined to their entries.
#[derive(Debug, Clone, PartialEq)]
pub struct Listing {
    /// The trade listing's id, so a listing fetched twice counts once.
    pub id: String,
    /// The item class of the listing's base ("Body Armour").
    pub class: String,
    /// Each joined modifier once, in card order.
    pub entries: Vec<(EntryId, AffixKind)>,
}

/// A modifier of a listing that joined no entry, and why.
#[derive(Debug, Clone, PartialEq)]
pub struct Unjoined {
    pub name: String,
    pub tier: String,
    pub reason: String,
}

/// What one fetched listing gave.
#[derive(Debug, Clone, PartialEq)]
pub struct ListingRead {
    pub listing: Listing,
    /// Rolled modifiers that joined no entry; left out of the counts.
    pub unjoined: Vec<Unjoined>,
    /// Crafted and desecrated modifiers, left out because they were chosen.
    pub chosen: usize,
}

/// Why a fetched entry gives nothing to record.
#[derive(Debug, Clone, PartialEq)]
pub enum NotRead {
    /// The null the API sends for a listing that vanished, or an entry
    /// without an id or an item.
    Gone,
    /// Normal, unique and unidentified items show no rolled affixes.
    NotRolled(String),
    /// The base is not in the base item data, so its class is unknown.
    UnknownBase(String),
}

fn kind_word(kind: AffixKind) -> &'static str {
    match kind {
        AffixKind::Prefix => "prefix",
        AffixKind::Suffix => "suffix",
    }
}

/// "P2" -> (Prefix, 2).
fn tier_badge(tier: &str) -> Option<(AffixKind, u8)> {
    let kind = match tier.get(..1)? {
        "P" => AffixKind::Prefix,
        "S" => AffixKind::Suffix,
        _ => return None,
    };
    Some((kind, tier[1..].parse().ok()?))
}

/// The entry behind one modifier of a listing on `base`.
fn join(
    data: &CraftData,
    (base, tags): (&str, &[&str]),
    domain: Domain,
    name: &str,
    tier: &str,
    level: u32,
) -> Result<(EntryId, AffixKind), String> {
    let (kind, number) =
        tier_badge(tier).ok_or_else(|| format!("the tier badge \"{tier}\" names no prefix or suffix tier"))?;
    let named: Vec<&Entry> = data
        .entries_named(name)
        .filter(|e| e.domain == domain && e.kind == kind && e.required_level == level)
        .collect();
    if named.is_empty() {
        return Err(format!("no {} named {name} at level {level} in the mod database", kind_word(kind)));
    }
    // A crafted modifier's entry is one only crafting grants, so no tag of
    // the base lets it roll; it is taken when the name, side and level
    // single it out.
    let here: Vec<&Entry> =
        named.iter().copied().filter(|e| e.eligible(tags.iter().copied()) || !e.rollable_anywhere()).collect();
    let entry = match here.as_slice() {
        [only] => *only,
        [] => {
            return Err(format!(
                "{} {name} at level {level} is in the mod database but does not roll on {base}",
                kind_word(kind)
            ))
        }
        several => {
            let ids: Vec<&str> = several.iter().map(|e| e.id.as_str()).collect();
            return Err(format!("{} entries roll on {base} as {name} at level {level}: {}", ids.len(), ids.join(", ")));
        }
    };
    let on_base = data.tier_on(entry, tags);
    if on_base != number {
        return Err(format!("{} is tier {on_base} on {base}, the listing shows {tier}", entry.id));
    }
    Ok((entry.id.clone(), entry.kind))
}

/// One fetch entry's rolled modifiers, joined to the mod database.
pub fn listing_entries(entry: &Value, data: &CraftData) -> Result<ListingRead, NotRead> {
    read_listing(entry, data, false)
}

/// Every modifier of a fetch entry joined to the mod database, the chosen
/// ones too (a crafted line to its granted entry, a desecrated line to its
/// desecrated entry): what an item state needs, where the observed counts
/// need only the rolled ones.
pub fn listing_entries_all(entry: &Value, data: &CraftData) -> Result<ListingRead, NotRead> {
    read_listing(entry, data, true)
}

fn read_listing(entry: &Value, data: &CraftData, join_chosen: bool) -> Result<ListingRead, NotRead> {
    let id = entry["id"].as_str().filter(|id| !id.is_empty()).ok_or(NotRead::Gone)?;
    let item = &entry["item"];
    if !item.is_object() {
        return Err(NotRead::Gone);
    }
    let rarity = item["rarity"].as_str().unwrap_or("");
    if !matches!(rarity, "Magic" | "Rare") {
        let shown = if rarity.is_empty() { "an item without a rarity".to_string() } else { format!("a {rarity} item") };
        return Err(NotRead::NotRolled(format!("{shown} rolls no affixes from the pool")));
    }
    if item["identified"].as_bool() == Some(false) {
        return Err(NotRead::NotRolled("the item is unidentified".to_string()));
    }
    // A magic item's typeLine carries its affix names; baseType is the base.
    let base_name = [&item["baseType"], &item["typeLine"]]
        .into_iter()
        .filter_map(Value::as_str)
        .find(|s| !s.is_empty())
        .unwrap_or("");
    let base = data.base(base_name).ok_or_else(|| NotRead::UnknownBase(base_name.to_string()))?;
    let tags: Vec<&str> = base.tags.iter().map(String::as_str).collect();

    let mut entries = Vec::new();
    let mut unjoined = Vec::new();
    let mut chosen = 0;
    // A hybrid modifier stands behind two card lines; it is one roll.
    let mut seen: HashSet<(&str, &str, Option<u64>)> = HashSet::new();
    for line in item["explicitMods"].as_array().into_iter().flatten() {
        let domain = line["domain"].as_str().unwrap_or("explicit");
        let Some(mods) = line["mods"].as_array() else {
            unjoined.push(Unjoined {
                name: line["description"].as_str().or(line.as_str()).unwrap_or("").to_string(),
                tier: String::new(),
                reason: "the line names no modifier".to_string(),
            });
            continue;
        };
        for m in mods {
            let name = m["name"].as_str().unwrap_or("");
            let tier = m["tier"].as_str().unwrap_or("");
            let level = m["level"].as_u64();
            if !name.is_empty() && !seen.insert((name, tier, level)) {
                continue;
            }
            let pool = match domain {
                "crafted" | "desecrated" if !join_chosen => {
                    chosen += 1;
                    continue;
                }
                "desecrated" => Domain::Desecrated,
                "explicit" | "fractured" | "crafted" => Domain::Item,
                other => {
                    unjoined.push(Unjoined {
                        name: name.to_string(),
                        tier: tier.to_string(),
                        reason: format!("a {other} modifier is not an affix roll"),
                    });
                    continue;
                }
            };
            let joined = match level.and_then(|l| u32::try_from(l).ok()) {
                _ if name.is_empty() => Err("the modifier has no affix name".to_string()),
                None => Err("the modifier has no required level".to_string()),
                // A desecrated line joins the desecrated entries first; the
                // site also marks desecrated some modifiers whose entry is an
                // ordinary affix (no desecrated entry bears the name), and
                // those join where the name, side and level are found.
                Some(level) => join(data, (&base.name, &tags), pool, name, tier, level).or_else(|first| {
                    if pool == Domain::Desecrated {
                        join(data, (&base.name, &tags), Domain::Item, name, tier, level).map_err(|_| first)
                    } else {
                        Err(first)
                    }
                }),
            };
            match joined {
                Ok(joined) => entries.push(joined),
                Err(reason) => unjoined.push(Unjoined { name: name.to_string(), tier: tier.to_string(), reason }),
            }
        }
    }
    Ok(ListingRead { listing: Listing { id: id.to_string(), class: base.class.clone(), entries }, unjoined, chosen })
}

#[derive(Debug, Clone, Default)]
struct Tally {
    counts: HashMap<EntryId, u32>,
    totals: HashMap<AffixKind, u32>,
    listings: u32,
}

impl Tally {
    fn add(&mut self, listing: &Listing) {
        self.listings += 1;
        for (id, kind) in &listing.entries {
            *self.counts.entry(id.clone()).or_default() += 1;
            *self.totals.entry(*kind).or_default() += 1;
        }
    }

    fn remove(&mut self, listing: &Listing) {
        self.listings = self.listings.saturating_sub(1);
        for (id, kind) in &listing.entries {
            if let Some(n) = self.counts.get_mut(id) {
                *n -= 1;
                if *n == 0 {
                    self.counts.remove(id);
                }
            }
            if let Some(n) = self.totals.get_mut(kind) {
                *n -= 1;
                if *n == 0 {
                    self.totals.remove(kind);
                }
            }
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Prefix,
    Suffix,
}

impl From<AffixKind> for Kind {
    fn from(k: AffixKind) -> Kind {
        match k {
            AffixKind::Prefix => Kind::Prefix,
            AffixKind::Suffix => Kind::Suffix,
        }
    }
}

impl From<Kind> for AffixKind {
    fn from(k: Kind) -> AffixKind {
        match k {
            Kind::Prefix => AffixKind::Prefix,
            Kind::Suffix => AffixKind::Suffix,
        }
    }
}

/// One listing as a line of the league's file.
#[derive(Serialize, Deserialize)]
struct Line {
    league: String,
    id: String,
    class: String,
    entries: Vec<(String, Kind)>,
}

/// What reading a league's lines found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LoadReport {
    /// Lines of this league taken into the store, in file order (the cap
    /// may since have dropped the oldest of them).
    pub kept: usize,
    /// Lines naming another league.
    pub other_league: usize,
    /// Lines that do not parse, such as a write cut short.
    pub unreadable: usize,
    /// Lines for a listing already in the store.
    pub repeated: usize,
}

/// The recorded listings of one league, oldest first, with their counts
/// per item class.
#[derive(Debug, Clone)]
pub struct Observed {
    league: String,
    min_listings: u32,
    cap: usize,
    order: VecDeque<Listing>,
    ids: HashSet<String>,
    classes: HashMap<String, Tally>,
}

impl Observed {
    pub fn new(league: &str) -> Observed {
        Observed::with_limits(league, MIN_LISTINGS, MAX_LISTINGS)
    }

    /// A store needing `min_listings` of a class for a model and keeping at
    /// most `cap` listings (at least one).
    pub fn with_limits(league: &str, min_listings: u32, cap: usize) -> Observed {
        Observed {
            league: league.to_string(),
            min_listings,
            cap: cap.max(1),
            order: VecDeque::new(),
            ids: HashSet::new(),
            classes: HashMap::new(),
        }
    }

    pub fn league(&self) -> &str {
        &self.league
    }

    pub fn min_listings(&self) -> u32 {
        self.min_listings
    }

    pub fn set_min_listings(&mut self, n: u32) {
        self.min_listings = n;
    }

    /// Moves the store to `league`, emptied: one league's market says
    /// nothing about the next one's. True when the league changed.
    pub fn switch_league(&mut self, league: &str) -> bool {
        if self.league == league {
            return false;
        }
        self.league = league.to_string();
        self.order.clear();
        self.ids.clear();
        self.classes.clear();
        true
    }

    /// Adds one listing unless a listing with its id is already kept, then
    /// drops the oldest listings past the cap. True when it was added.
    pub fn record(&mut self, listing: Listing) -> bool {
        if self.ids.contains(&listing.id) {
            return false;
        }
        self.classes.entry(listing.class.clone()).or_default().add(&listing);
        self.ids.insert(listing.id.clone());
        self.order.push_back(listing);
        while self.order.len() > self.cap {
            let Some(old) = self.order.pop_front() else { break };
            self.ids.remove(&old.id);
            if let Some(tally) = self.classes.get_mut(&old.class) {
                tally.remove(&old);
                if tally.listings == 0 {
                    self.classes.remove(&old.class);
                }
            }
        }
        true
    }

    pub fn contains(&self, id: &str) -> bool {
        self.ids.contains(id)
    }

    /// Listings kept, across every class.
    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// Listings of `class` kept.
    pub fn listings(&self, class: &str) -> u32 {
        self.classes.get(class).map_or(0, |t| t.listings)
    }

    /// The observed model of `class`, once at least the minimum number of
    /// its listings is kept.
    pub fn model(&self, class: &str) -> Option<Model> {
        let tally = self.classes.get(class).filter(|t| t.listings >= self.min_listings)?;
        Some(Model::Observed {
            class: class.to_string(),
            counts: tally.counts.clone(),
            totals: tally.totals.clone(),
            listings: tally.listings,
        })
    }

    /// One listing as a line of `league`'s file, without the newline.
    pub fn line(league: &str, listing: &Listing) -> String {
        let line = Line {
            league: league.to_string(),
            id: listing.id.clone(),
            class: listing.class.clone(),
            entries: listing.entries.iter().map(|(id, kind)| (id.clone(), Kind::from(*kind))).collect(),
        };
        serde_json::to_string(&line).expect("a line of strings serializes")
    }

    /// Every kept listing as lines, oldest first, each ending in a newline.
    pub fn jsonl(&self) -> String {
        let mut out = String::new();
        for listing in &self.order {
            out.push_str(&Observed::line(&self.league, listing));
            out.push('\n');
        }
        out
    }

    /// Records the listings in `text`, one line each, in order, so the cap
    /// keeps the newest. Lines of another league and lines that do not
    /// parse are counted and skipped.
    pub fn load_jsonl(&mut self, text: &str) -> LoadReport {
        let mut report = LoadReport::default();
        for raw in text.lines().filter(|l| !l.trim().is_empty()) {
            let Ok(line) = serde_json::from_str::<Line>(raw) else {
                report.unreadable += 1;
                continue;
            };
            if line.id.is_empty() {
                report.unreadable += 1;
                continue;
            }
            if line.league != self.league {
                report.other_league += 1;
                continue;
            }
            let listing = Listing {
                id: line.id,
                class: line.class,
                entries: line.entries.into_iter().map(|(id, kind)| (id, AffixKind::from(kind))).collect(),
            };
            if self.record(listing) {
                report.kept += 1;
            } else {
                report.repeated += 1;
            }
        }
        report
    }
}
