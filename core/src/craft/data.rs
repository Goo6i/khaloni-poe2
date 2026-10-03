//! The mod database, base tags and essence texts in one structure.
//!
//! Three files feed it: the mod database (`repoe_mods.json`, keyed by mod
//! id), the base item data (`base_items.json`, keyed by metadata path) and
//! the essence texts (`xile_essences.json`). Only what the planner acts on is
//! kept: prefixes and suffixes of the item and desecrated domains, and bases
//! of the item domain. The Alloy table (`core/data/alloys.json`) is built
//! into the crate, since no cached feed carries what each Alloy adds.
//!
//! The client carries no real spawn weights for PoE2: every weight in the
//! mod database is 0 or 1, so a weight here says whether an entry can roll,
//! never how often.

use super::types::{AffixKind, Candidate, EssenceOutcome, Lich};
use serde::de::{Deserializer, MapAccess, Visitor};
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt;

/// Where an entry lives in the mod database.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Domain {
    /// The ordinary affixes of equipment.
    Item,
    /// The lich affixes a bone reveals.
    Desecrated,
}

/// A base type: its class and its tags in the data's order.
#[derive(Debug, Clone, PartialEq)]
pub struct Base {
    pub name: String,
    pub class: String,
    pub tags: Vec<String>,
}

impl Base {
    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| t == tag)
    }
}

/// One entry of the mod database: one tier of one family.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub id: String,
    /// The affix name the item shows in its header ("Rotund").
    pub name: String,
    /// The mod database's `type` ("IncreasedLife"): every tier of a family
    /// shares it.
    pub family: String,
    pub domain: Domain,
    pub kind: AffixKind,
    pub required_level: u32,
    pub groups: Vec<String>,
    pub adds_tags: Vec<String>,
    /// The entry's text with the database's link markup removed; empty when
    /// the database has none.
    pub text: String,
    /// `(tag, weight)` in the entry's own order, which is the order the game
    /// reads them in.
    pub spawn_weights: Vec<(String, u32)>,
    pub essence_only: bool,
    /// The lich a desecrated entry belongs to, read from the lich tag its
    /// spawn weights carry.
    pub lich: Option<Lich>,
}

impl Entry {
    /// The weight of the first tag in the entry's own list that the item
    /// carries; `None` when it carries none of them. The entry's order
    /// decides, not the item's: a str-armour pair of gloves meets
    /// `gloves: 0` before `str_armour: 1` on the high armour tiers.
    pub fn deciding_weight<'a>(&self, tags: impl IntoIterator<Item = &'a str> + Clone) -> Option<u32> {
        self.spawn_weights
            .iter()
            .find(|(tag, _)| tags.clone().into_iter().any(|t| t == tag))
            .map(|(_, w)| *w)
    }

    /// Whether the entry can roll on an item carrying `tags`.
    pub fn eligible<'a>(&self, tags: impl IntoIterator<Item = &'a str> + Clone) -> bool {
        self.deciding_weight(tags).is_some_and(|w| w > 0)
    }

    /// Whether any tag at all lets this entry roll. An entry that no tag
    /// lets roll exists only to be granted: essences and other crafting put
    /// it on an item, random rolls never do.
    pub fn rollable_anywhere(&self) -> bool {
        self.spawn_weights.iter().any(|(_, w)| *w > 0)
    }
}

/// One class line of an essence: "Armour or Belt" and the modifier text.
#[derive(Debug, Clone, PartialEq)]
pub struct EssenceLine {
    pub classes: String,
    /// The modifier as the essence words it, markup removed and dashes made
    /// plain ("+(30-39) to maximum Life").
    pub text: String,
}

/// One Alloy: its item text and the modifier it adds per item class, as
/// its poe2db page lists them.
#[derive(Debug, Clone, PartialEq)]
pub struct Alloy {
    /// "Adaptive Alloy".
    pub name: String,
    /// The line that says what the Alloy does to the item.
    pub effect: String,
    /// The page the lines were copied from.
    pub source: String,
    /// Class lines worded like an essence's: "Staves, Wands" and the
    /// modifier, a two-line modifier with a line break between its lines.
    pub lines: Vec<EssenceLine>,
}

/// The Alloy table the planner always has.
const ALLOYS_JSON: &str = include_str!("../../data/alloys.json");

#[derive(Debug, Clone, PartialEq)]
pub struct Essence {
    /// "Lesser Essence of the Body".
    pub name: String,
    /// The line that says what the essence does to the item.
    pub effect: String,
    pub lines: Vec<EssenceLine>,
}

/// The planner's data: bases, entries and essences, with the indexes the
/// pool and the joins need.
#[derive(Debug, Clone)]
pub struct CraftData {
    bases: Vec<Base>,
    base_by_name: HashMap<String, usize>,
    entries: Vec<Entry>,
    by_id: HashMap<String, usize>,
    by_name: HashMap<String, Vec<usize>>,
    by_text: HashMap<String, Vec<usize>>,
    ladders: HashMap<(Domain, AffixKind, String), Vec<usize>>,
    essences: Vec<Essence>,
    alloys: Vec<Alloy>,
}

#[derive(Deserialize)]
struct RawWeight {
    tag: String,
    weight: u32,
}

#[derive(Deserialize)]
struct RawMod {
    #[serde(default)]
    domain: String,
    #[serde(default)]
    generation_type: String,
    #[serde(default)]
    groups: Vec<String>,
    #[serde(default)]
    required_level: Option<u32>,
    #[serde(default)]
    spawn_weights: Vec<RawWeight>,
    #[serde(default)]
    adds_tags: Vec<String>,
    #[serde(default)]
    is_essence_only: bool,
    #[serde(default)]
    text: Option<String>,
    #[serde(default, rename = "type")]
    family: String,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Deserialize)]
struct RawBase {
    #[serde(default)]
    name: String,
    #[serde(default)]
    item_class: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    domain: String,
}

#[derive(Deserialize)]
struct RawEssences {
    essences: Vec<RawEssence>,
}

#[derive(Deserialize)]
struct RawEssence {
    #[serde(default)]
    name: String,
    #[serde(default)]
    slug: String,
    #[serde(default, rename = "explicitMods")]
    explicit_mods: Vec<String>,
}

#[derive(Deserialize)]
struct RawAlloys {
    alloys: Vec<RawAlloy>,
}

#[derive(Deserialize)]
struct RawAlloy {
    name: String,
    #[serde(default)]
    effect: String,
    #[serde(default)]
    source: String,
    #[serde(default)]
    lines: Vec<RawAlloyLine>,
}

#[derive(Deserialize)]
struct RawAlloyLine {
    classes: String,
    text: String,
}

/// A JSON object read as its entries in file order. The base item data
/// repeats a few names under different paths, and the first one in the file
/// is the one kept, so the order has to survive the parse.
struct Ordered<T>(Vec<(String, T)>);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Ordered<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V<T>(std::marker::PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for V<T> {
            type Value = Ordered<T>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut out = Vec::with_capacity(map.size_hint().unwrap_or(0));
                while let Some((k, v)) = map.next_entry::<String, T>()? {
                    out.push((k, v));
                }
                Ok(Ordered(out))
            }
        }
        deserializer.deserialize_map(V(std::marker::PhantomData))
    }
}

/// The mod database's text with its link markup removed:
/// `+(191-221) to [Armour|Armour]` reads `+(191-221) to Armour`, and
/// `[Flask]` reads `Flask`.
pub fn plain_db_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(open) = rest.find('[') {
        let Some(close) = rest[open..].find(']').map(|c| open + c) else { break };
        out.push_str(&rest[..open]);
        let inner = &rest[open + 1..close];
        out.push_str(inner.rsplit_once('|').map_or(inner, |(_, shown)| shown));
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    out.trim().to_string()
}

/// An essence line with its HTML removed and its dashes made plain:
/// `<span class="mod-value">+(30—39)</span> to maximum Life` reads
/// `+(30-39) to maximum Life`.
pub fn plain_essence_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut in_tag = false;
    for c in raw.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if in_tag => {}
            '\u{2014}' | '\u{2013}' => out.push('-'),
            _ => out.push(c),
        }
    }
    out.trim().to_string()
}

/// Every `(a-b)` range written low end first, so a text that words a
/// "reduced" range high-to-low still meets the same text written the other
/// way.
pub fn canonical_ranges(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('(') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find(')').and_then(|close| range_bounds(&after[..close]).map(|b| (close, b))) {
            Some((close, (lo, hi))) => {
                let (a, b) = if hi.parse::<f64>().ok() < lo.parse::<f64>().ok() { (hi, lo) } else { (lo, hi) };
                out.push('(');
                out.push_str(a);
                out.push('-');
                out.push_str(b);
                out.push(')');
                rest = &after[close + 1..];
            }
            None => {
                out.push('(');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// `"30-39"` → `("30", "39")`, `"-10--5"` → `("-10", "-5")`; `None` for
/// anything that is not two numbers.
pub(crate) fn range_bounds(inner: &str) -> Option<(&str, &str)> {
    let start = usize::from(inner.starts_with('-'));
    let dash = inner[start..].find('-')? + start;
    let (lo, hi) = (&inner[..dash], &inner[dash + 1..]);
    let number = |s: &str| !s.is_empty() && s.parse::<f64>().is_ok();
    (number(lo) && number(hi)).then_some((lo, hi))
}

/// Whether one class phrase of an essence line names this base. `None`
/// when the phrase is not one this code knows, so the caller can say so
/// instead of guessing. Group phrases read the base's tags: the data tags
/// every martial weapon `weapon` (wands, staves and sceptres are not), bows
/// and crossbows `ranged`, and every piece of armour `armour`.
fn class_phrase_names(phrase: &str, base: &Base) -> Option<bool> {
    let has = |t: &str| base.has_tag(t);
    let phrase = phrase.trim().to_lowercase();
    let named = match phrase.as_str() {
        "armour" => has("armour"),
        "jewellery" => has("amulet") || has("ring"),
        "equipment" => ["armour", "weapon", "amulet", "ring", "belt", "quiver", "focus", "wand", "sceptre", "staff"]
            .iter()
            .any(|t| has(t)),
        "martial weapon" => has("weapon"),
        "melee weapon" => has("weapon") && !has("ranged"),
        "one handed melee weapon" => has("weapon") && has("onehand") && !has("ranged"),
        "two handed melee weapon" => has("weapon") && has("twohand") && !has("ranged"),
        "belt" | "boots" | "gloves" | "helmet" | "body armour" | "shield" | "amulet" | "ring" | "quiver" | "focus"
        | "wand" | "staff" | "sceptre" | "bow" | "crossbow" => base.class.to_lowercase() == phrase,
        _ => {
            let (_, class) = CLASS_PLURALS.iter().find(|(plural, _)| *plural == phrase)?;
            base.class.to_lowercase() == *class
        }
    };
    Some(named)
}

/// The plural class names the Alloy pages use, each with the item class it
/// means as the base item data writes it (lower case). Quarterstaves are
/// "Warstaff" there.
const CLASS_PLURALS: [(&str, &str); 25] = [
    ("belts", "belt"),
    ("helmets", "helmet"),
    ("body armours", "body armour"),
    ("shields", "shield"),
    ("bucklers", "buckler"),
    ("foci", "focus"),
    ("amulets", "amulet"),
    ("rings", "ring"),
    ("quivers", "quiver"),
    ("wands", "wand"),
    ("staves", "staff"),
    ("quarterstaves", "warstaff"),
    ("sceptres", "sceptre"),
    ("bows", "bow"),
    ("crossbows", "crossbow"),
    ("spears", "spear"),
    ("daggers", "dagger"),
    ("flails", "flail"),
    ("talismans", "talisman"),
    ("one hand swords", "one hand sword"),
    ("one hand axes", "one hand axe"),
    ("one hand maces", "one hand mace"),
    ("two hand swords", "two hand sword"),
    ("two hand axes", "two hand axe"),
    ("two hand maces", "two hand mace"),
];

/// The one line of `lines` whose classes name `base`; the error says why
/// there is none, or that more than one names it. `name` is the essence or
/// Alloy the lines belong to.
fn line_for<'a>(name: &str, lines: &'a [EssenceLine], base: &Base) -> Result<&'a EssenceLine, String> {
    let mut unplaced = Vec::new();
    let mut naming = Vec::new();
    for line in lines {
        let mut names_base = false;
        for phrase in class_phrases(&line.classes) {
            match class_phrase_names(phrase, base) {
                Some(true) => names_base = true,
                Some(false) => {}
                None => unplaced.push(phrase.to_string()),
            }
        }
        if names_base {
            naming.push(line);
        }
    }
    match naming.as_slice() {
        [line] => Ok(*line),
        [] if !unplaced.is_empty() => Err(format!(
            "{name} names classes this data cannot place ({}), and none of the others is {}",
            unplaced.join(", "),
            base.class
        )),
        [] => Err(format!("{name} has no modifier for {}", base.class)),
        _ => Err(format!("{name} words more than one modifier for {}", base.class)),
    }
}

/// A modifier's lines, ranges written low end first, in sorted order: an
/// Alloy's page and the mod database word a two-line modifier's lines in
/// either order.
fn line_set(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = text.lines().map(|l| canonical_ranges(l.trim())).filter(|l| !l.is_empty()).collect();
    lines.sort_unstable();
    lines
}

/// "Belt, Boots, Gloves, Helmet or Jewellery" → its five phrases.
fn class_phrases(classes: &str) -> Vec<&str> {
    classes.split(", ").flat_map(|p| p.split(" or ")).map(str::trim).filter(|p| !p.is_empty()).collect()
}

impl CraftData {
    /// Reads the three files; the Alloy table comes from the crate itself.
    pub fn load(mods_json: &str, base_items_json: &str, essences_json: &str) -> Result<CraftData, String> {
        let mods: HashMap<String, RawMod> =
            serde_json::from_str(mods_json).map_err(|e| format!("the mod database does not parse: {e}"))?;
        let bases: Ordered<RawBase> =
            serde_json::from_str(base_items_json).map_err(|e| format!("the base item data does not parse: {e}"))?;
        let essences: RawEssences =
            serde_json::from_str(essences_json).map_err(|e| format!("the essence data does not parse: {e}"))?;

        let mut entries: Vec<Entry> = mods
            .into_iter()
            .filter_map(|(id, m)| {
                let domain = match m.domain.as_str() {
                    "item" => Domain::Item,
                    "desecrated" => Domain::Desecrated,
                    _ => return None,
                };
                let kind = match m.generation_type.as_str() {
                    "prefix" => AffixKind::Prefix,
                    "suffix" => AffixKind::Suffix,
                    _ => return None,
                };
                let spawn_weights: Vec<(String, u32)> = m.spawn_weights.into_iter().map(|w| (w.tag, w.weight)).collect();
                let lich = [Lich::Ulaman, Lich::Amanamu, Lich::Kurgal]
                    .into_iter()
                    .find(|l| spawn_weights.iter().any(|(t, w)| t == l.tag() && *w > 0));
                Some(Entry {
                    id,
                    name: m.name.unwrap_or_default(),
                    family: m.family,
                    domain,
                    kind,
                    required_level: m.required_level.unwrap_or(0),
                    groups: m.groups,
                    adds_tags: m.adds_tags,
                    text: m.text.as_deref().map(plain_db_text).unwrap_or_default(),
                    spawn_weights,
                    essence_only: m.is_essence_only,
                    lich,
                })
            })
            .collect();
        if entries.is_empty() {
            return Err("the mod database holds no item or desecrated prefixes or suffixes".to_string());
        }
        entries.sort_by(|a, b| a.id.cmp(&b.id));

        let mut by_id = HashMap::new();
        let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
        let mut by_text: HashMap<String, Vec<usize>> = HashMap::new();
        let mut ladders: HashMap<(Domain, AffixKind, String), Vec<usize>> = HashMap::new();
        for (i, e) in entries.iter().enumerate() {
            by_id.insert(e.id.clone(), i);
            if !e.name.is_empty() {
                by_name.entry(e.name.clone()).or_default().push(i);
            }
            if !e.text.is_empty() {
                by_text.entry(canonical_ranges(&e.text)).or_default().push(i);
            }
            if !e.essence_only {
                ladders.entry((e.domain, e.kind, e.family.clone())).or_default().push(i);
            }
        }

        let mut base_list = Vec::new();
        let mut base_by_name = HashMap::new();
        for (_, b) in bases.0 {
            if b.domain != "item" || b.name.is_empty() || base_by_name.contains_key(&b.name) {
                continue;
            }
            base_by_name.insert(b.name.clone(), base_list.len());
            base_list.push(Base { name: b.name, class: b.item_class, tags: b.tags });
        }
        if base_list.is_empty() {
            return Err("the base item data holds no item bases".to_string());
        }

        let essences = essences
            .essences
            .into_iter()
            .map(|e| {
                let name = if e.name.trim().is_empty() { e.slug.replace('_', " ") } else { e.name };
                let mut effect = String::new();
                let mut lines = Vec::new();
                for raw in &e.explicit_mods {
                    let text = plain_essence_text(raw);
                    match text.split_once(": ") {
                        Some((classes, modifier)) => {
                            lines.push(EssenceLine { classes: classes.to_string(), text: modifier.to_string() })
                        }
                        None if effect.is_empty() => effect = text,
                        None => {}
                    }
                }
                Essence { name, effect, lines }
            })
            .collect();

        let alloys: RawAlloys =
            serde_json::from_str(ALLOYS_JSON).map_err(|e| format!("the Alloy table does not parse: {e}"))?;
        let alloys = alloys
            .alloys
            .into_iter()
            .map(|a| Alloy {
                name: a.name,
                effect: a.effect,
                source: a.source,
                lines: a
                    .lines
                    .into_iter()
                    .map(|l| EssenceLine { classes: l.classes, text: plain_essence_text(&l.text) })
                    .collect(),
            })
            .collect();

        Ok(CraftData { bases: base_list, base_by_name, entries, by_id, by_name, by_text, ladders, essences, alloys })
    }

    pub fn base(&self, name: &str) -> Option<&Base> {
        self.base_by_name.get(name).map(|&i| &self.bases[i])
    }

    pub fn bases(&self) -> &[Base] {
        &self.bases
    }

    pub fn entry(&self, id: &str) -> Option<&Entry> {
        self.by_id.get(id).map(|&i| &self.entries[i])
    }

    /// Every entry, ordered by id.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Entries whose header name is `name`.
    pub fn entries_named(&self, name: &str) -> impl Iterator<Item = &Entry> {
        self.by_name.get(name).into_iter().flatten().map(|&i| &self.entries[i])
    }

    /// Entries whose plain text is `text` (see [`plain_db_text`]), ranges
    /// compared low end first.
    pub fn entries_with_text(&self, text: &str) -> impl Iterator<Item = &Entry> {
        self.by_text.get(&canonical_ranges(text)).into_iter().flatten().map(|&i| &self.entries[i])
    }

    pub fn essences(&self) -> &[Essence] {
        &self.essences
    }

    /// An essence by name; underscores read as spaces and case is ignored,
    /// so the slug works too.
    pub fn essence_named(&self, name: &str) -> Option<&Essence> {
        let wanted = name.replace('_', " ").trim().to_lowercase();
        self.essences.iter().find(|e| e.name.to_lowercase() == wanted)
    }

    /// Every Alloy of the table, in its order.
    pub fn alloys(&self) -> &[Alloy] {
        &self.alloys
    }

    /// An Alloy by name, case ignored.
    pub fn alloy_named(&self, name: &str) -> Option<&Alloy> {
        let wanted = name.trim().to_lowercase();
        self.alloys.iter().find(|a| a.name.to_lowercase() == wanted)
    }

    /// Trade-site numbering of `entry` on an item carrying `tags`: 1 is the
    /// family's best tier there. The ladder is every entry of the same
    /// family, kind and domain that can roll on those tags at any item level;
    /// the tier is one more than the number of distinct required levels on
    /// it above this entry's.
    pub fn tier_on(&self, entry: &Entry, tags: &[&str]) -> u8 {
        let mut higher: Vec<u32> = self
            .ladders
            .get(&(entry.domain, entry.kind, entry.family.clone()))
            .into_iter()
            .flatten()
            .map(|&i| &self.entries[i])
            .filter(|e| e.required_level > entry.required_level && e.eligible(tags.iter().copied()))
            .map(|e| e.required_level)
            .collect();
        higher.sort_unstable();
        higher.dedup();
        u8::try_from(higher.len() + 1).unwrap_or(u8::MAX)
    }

    /// The candidate `entry` makes on an item carrying `tags`.
    pub fn candidate(&self, entry: &Entry, tags: &[&str]) -> Candidate {
        Candidate {
            entry_id: entry.id.clone(),
            family: entry.family.clone(),
            groups: entry.groups.clone(),
            kind: entry.kind,
            tier: self.tier_on(entry, tags),
            required_level: entry.required_level,
            adds_tags: entry.adds_tags.clone(),
            text: entry.text.clone(),
            desecrated: match entry.domain {
                Domain::Desecrated => entry.lich,
                Domain::Item => None,
            },
        }
    }

    /// The entry an essence line means when no entry reads its exact text.
    /// The essence cards word some modifiers their own way: "Global
    /// Defences" for the database's "Global Armour, Evasion and Energy
    /// Shield", "Armour, Evasion or Energy Shield" for whichever of the three
    /// the base can roll, "Socketed Items" for "Socketed Augment Items". So
    /// an entry with the same rolls whose words are all among the line's (or
    /// the line's all among its) is the one meant, narrowed to what only
    /// crafting grants or what rolls on this base. When the rolls differ
    /// (the card predates a patch that changed them, as Perfect Essence of
    /// Battle's "+5" became "+3" in 0.5.0), an entry only crafting grants
    /// with exactly the line's words is taken. Anything left open is an
    /// error saying which entries stayed in the running.
    fn essence_by_meaning(&self, text: &str, tags: &[&str], hand: Option<Hand>) -> Result<&Entry, String> {
        let (rolls, words) = (rolls_of(text), meaning_words(text));
        let items = || self.entries.iter().filter(|e| e.domain == Domain::Item && !e.text.is_empty() && hand_fits(e, hand));
        let usable = |e: &&Entry| !e.rollable_anywhere() || e.eligible(tags.iter().copied());
        let same: Vec<&Entry> = items()
            .filter(|e| rolls_of(&e.text) == rolls)
            .filter(|e| {
                let theirs = meaning_words(&e.text);
                !theirs.is_empty() && (theirs.is_subset(&words) || words.is_subset(&theirs))
            })
            .filter(usable)
            .collect();
        let granted: Vec<&Entry> = same.iter().copied().filter(|e| !e.rollable_anywhere()).collect();
        match (granted.as_slice(), same.as_slice()) {
            ([only], _) | ([], [only]) => return Ok(only),
            ([], []) => {}
            (_, several) => {
                let mut texts: Vec<&str> = several.iter().map(|e| e.text.as_str()).collect();
                texts.sort_unstable();
                texts.dedup();
                return Err(format!(", and it reads as any of {}; which one it gives is not stated", texts.join(" / ")));
            }
        }
        let renumbered: Vec<&Entry> =
            items().filter(|e| !e.rollable_anywhere() && meaning_words(&e.text) == words).collect();
        match renumbered.as_slice() {
            [only] => Ok(only),
            _ => Err(String::new()),
        }
    }

    /// The entry an essence guarantees on this base, found by matching the
    /// essence's text for the base's class against the mod database's
    /// texts. Whenever the text names no entry, or the essence has no line
    /// for this class, the answer is `Unknown` with the reason.
    pub fn essence_outcome(&self, essence: &str, base: &Base) -> EssenceOutcome {
        let Some(found) = self.essence_named(essence) else {
            return EssenceOutcome::Unknown(format!("no essence named {essence} in the essence data"));
        };
        let line = match line_for(&found.name, &found.lines, base) {
            Ok(line) => line,
            Err(why) => return EssenceOutcome::Unknown(why),
        };

        let tags: Vec<&str> = base.tags.iter().map(String::as_str).collect();
        // An essence that words one-handed and two-handed lines apart grants
        // entries the database marks "1H" and "2H"; the card's numbers can
        // predate a patch, so the one-handed line's text may read exactly as
        // the two-handed entry does. The line's own hand decides.
        let hand = line_hand(&line.classes);
        let matches: Vec<&Entry> = self
            .entries_with_text(&line.text)
            .filter(|e| e.domain == Domain::Item && hand_fits(e, hand))
            .collect();
        let chosen = match matches.as_slice() {
            [] => match self.essence_by_meaning(&line.text, &tags, hand) {
                Ok(e) => e,
                Err(why) => {
                    return EssenceOutcome::Unknown(format!(
                        "no entry of the mod database reads \"{}\"{why} ({} on {})",
                        line.text, found.name, base.class
                    ))
                }
            },
            [only] => *only,
            several => {
                // An entry no tag lets roll exists only to be granted, so when
                // exactly one of the entries sharing the text is such an entry
                // it is the essence's own. Otherwise the one that can roll on
                // this base is the one the essence adds there.
                let granted: Vec<&Entry> = several.iter().copied().filter(|e| !e.rollable_anywhere()).collect();
                let here: Vec<&Entry> = several.iter().copied().filter(|e| e.eligible(tags.iter().copied())).collect();
                match (granted.as_slice(), here.as_slice()) {
                    ([only], _) => *only,
                    ([], [only]) => *only,
                    _ => {
                        return EssenceOutcome::Unknown(format!(
                            "{} entries of the mod database read \"{}\" and none of them is singled out on {}",
                            several.len(),
                            line.text,
                            base.class
                        ))
                    }
                }
            }
        };
        EssenceOutcome::Entry(self.candidate(chosen, &tags))
    }

    /// The entry an Alloy adds on this base: its line for the base's class
    /// (as its poe2db page words it) joined to the mod database by text,
    /// the lines of a two-line modifier in either order. When several
    /// entries read the same, the one only crafting grants is taken, and
    /// among those the database's own Alloy entry (its id starts "Alloy").
    /// A text no entry reads falls back to the essences' match by meaning.
    /// Anything left open, or a class the Alloy has no line for, is
    /// `Unknown` with the reason.
    pub fn alloy_outcome(&self, alloy: &str, base: &Base) -> EssenceOutcome {
        let Some(found) = self.alloy_named(alloy) else {
            return EssenceOutcome::Unknown(format!("no Alloy named {alloy} in the Alloy table"));
        };
        let line = match line_for(&found.name, &found.lines, base) {
            Ok(line) => line,
            Err(why) => return EssenceOutcome::Unknown(why),
        };
        let tags: Vec<&str> = base.tags.iter().map(String::as_str).collect();
        let mut matches: Vec<&Entry> = self.entries_with_text(&line.text).filter(|e| e.domain == Domain::Item).collect();
        if matches.is_empty() && line.text.contains('\n') {
            let wanted = line_set(&line.text);
            matches = self
                .entries
                .iter()
                .filter(|e| e.domain == Domain::Item && e.text.contains('\n') && line_set(&e.text) == wanted)
                .collect();
        }
        let granted: Vec<&Entry> = matches.iter().copied().filter(|e| !e.rollable_anywhere()).collect();
        let own: Vec<&Entry> = granted.iter().copied().filter(|e| e.id.starts_with("Alloy")).collect();
        let chosen = match (matches.as_slice(), granted.as_slice(), own.as_slice()) {
            ([], _, _) => match self.essence_by_meaning(&line.text, &tags, None) {
                Ok(e) => e,
                Err(why) => {
                    return EssenceOutcome::Unknown(format!(
                        "no entry of the mod database reads \"{}\"{why} ({} on {})",
                        line.text.replace('\n', " / "),
                        found.name,
                        base.class
                    ))
                }
            },
            ([only], _, _) | (_, [only], _) | (_, _, [only]) => *only,
            (several, _, _) => {
                return EssenceOutcome::Unknown(format!(
                    "{} entries of the mod database read \"{}\" and none of them is singled out as the Alloy's on {}",
                    several.len(),
                    line.text.replace('\n', " / "),
                    base.class
                ))
            }
        };
        EssenceOutcome::Entry(self.candidate(chosen, &tags))
    }
}

/// A text's numbers, ranges written low end first: "+(9-12) to Strength"
/// reads "9-12", "Adds (5-8) to (12-15)" reads "5-8 12-15".
fn rolls_of(text: &str) -> String {
    let text = canonical_ranges(text);
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for c in text.chars() {
        if c.is_ascii_digit() || c == '.' || (c == '-' && !cur.is_empty()) {
            cur.push(c);
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out.join(" ")
}

/// The words that carry a modifier's meaning, lower case: numbers, signs
/// and the joining words dropped, "Rating" dropped ("Evasion" and "Evasion
/// Rating" are one stat), and "Defences" read as the three it stands for.
fn meaning_words(text: &str) -> std::collections::BTreeSet<String> {
    const JOINING: [&str; 9] = ["or", "and", "to", "of", "the", "a", "an", "rating", "you"];
    let mut out = std::collections::BTreeSet::new();
    for w in text.split(|c: char| !c.is_alphabetic()).filter(|w| !w.is_empty()) {
        let w = w.to_lowercase();
        if JOINING.contains(&w.as_str()) {
            continue;
        }
        if w == "defences" {
            out.extend(["armour", "evasion", "energy", "shield"].map(String::from));
        } else {
            out.insert(w);
        }
    }
    out
}

/// Which hand an essence's class line is for, when it names one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hand {
    One,
    Two,
}

/// "One Handed Melee Weapon or Bow" is one-handed, "Two Handed Melee Weapon
/// or Crossbow" two-handed; a line naming neither has no hand.
fn line_hand(classes: &str) -> Option<Hand> {
    let lower = classes.to_lowercase();
    if lower.contains("two handed") || lower.contains("crossbow") {
        Some(Hand::Two)
    } else if lower.contains("one handed") || lower.split(|c: char| !c.is_alphabetic()).any(|w| w == "bow") {
        Some(Hand::One)
    } else {
        None
    }
}

/// Whether an entry suits the line's hand: the database marks
/// hand-specific granted entries with "1H" or "2H" in their id.
fn hand_fits(entry: &Entry, hand: Option<Hand>) -> bool {
    match hand {
        Some(Hand::One) => !entry.id.contains("2H"),
        Some(Hand::Two) => !entry.id.contains("1H"),
        None => true,
    }
}
