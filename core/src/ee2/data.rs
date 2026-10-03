//! EE2's reference data: `stats.ndjson` (every stat translation with its
//! trade ids) and `items.ndjson` (bases, uniques, gems).
//!
//! Lookups go through the same 32-bit FNV-1a hash index EE2 builds, not a
//! plain string map. The difference only shows on the handful of matcher
//! strings two stats share and on hash collisions, and there the index
//! decides which stat wins; a string map would quietly pick another.

use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Matcher {
    pub string: String,
    #[serde(default)]
    pub advanced: Option<String>,
    #[serde(default)]
    pub negate: bool,
    #[serde(default)]
    pub value: Option<f64>,
    #[serde(default)]
    pub oils: Option<String>,
}

/// `better` in stats.ndjson: whether a higher roll is the better one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Better {
    Negative,
    NotComparable,
    Positive,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stat {
    pub ref_: String,
    pub better: Better,
    pub dp: bool,
    pub matchers: Vec<Matcher>,
    /// Trade ids per mod type ("explicit", "rune", ...), in file order.
    pub trade_ids: Vec<(String, Vec<String>)>,
    pub inverted: bool,
    pub option: bool,
}

impl Stat {
    pub fn ids(&self, mod_type: &str) -> Option<&Vec<String>> {
        self.trade_ids.iter().find(|(t, _)| t == mod_type).map(|(_, ids)| ids)
    }
}

/// A one-matcher stat that lives outside stats.ndjson: an item property,
/// or a line only the trade catalog knows.
pub fn plain_stat(ref_: &str, mod_type: &str, trade_id: &str) -> Stat {
    Stat {
        ref_: ref_.to_string(),
        better: Better::Positive,
        dp: false,
        matchers: vec![Matcher { string: ref_.to_string(), advanced: None, negate: false, value: None, oils: None }],
        trade_ids: vec![(mod_type.to_string(), vec![trade_id.to_string()])],
        inverted: false,
        option: false,
    }
}

#[derive(Deserialize)]
struct RawStat {
    #[serde(rename = "ref")]
    ref_: String,
    better: i8,
    #[serde(default)]
    dp: bool,
    matchers: Vec<Matcher>,
    trade: RawTrade,
}

#[derive(Deserialize)]
struct RawTrade {
    #[serde(default)]
    ids: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(default)]
    inverted: bool,
    #[serde(default)]
    option: bool,
}

pub fn fnv1a32(s: &str) -> u32 {
    let mut h: u32 = 2_166_136_261;
    for b in s.as_bytes() {
        h ^= *b as u32;
        h = h.wrapping_mul(16_777_619);
    }
    h
}

/// EE2's sorted `(hash, line)` index with its binary search. The sort is
/// stable and the search lands where EE2's lands, so among equal hashes the
/// same line is chosen.
#[derive(Debug, Default)]
struct HashIndex {
    rows: Vec<(u32, usize)>,
}

impl HashIndex {
    fn build(mut rows: Vec<(u32, usize)>) -> HashIndex {
        rows.sort_by_key(|(h, _)| *h);
        HashIndex { rows }
    }

    fn find(&self, key: &str) -> Option<usize> {
        let value = fnv1a32(key);
        let (mut left, mut right) = (0i64, self.rows.len() as i64 - 1);
        while left <= right {
            let mid = (left + right) / 2;
            let at = self.rows[mid as usize].0;
            if at < value {
                left = mid + 1;
            } else if at > value {
                right = mid - 1;
            } else {
                return Some(self.rows[mid as usize].1);
            }
        }
        None
    }
}

#[derive(Debug, Default)]
pub struct StatDb {
    stats: Vec<Arc<Stat>>,
    by_ref: HashIndex,
    by_matcher: HashIndex,
}

impl StatDb {
    pub fn from_ndjson(text: &str) -> Result<StatDb, serde_json::Error> {
        let mut stats = Vec::new();
        let (mut refs, mut matchers) = (Vec::new(), Vec::new());
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let raw: RawStat = serde_json::from_str(line)?;
            let idx = stats.len();
            refs.push((fnv1a32(&raw.ref_), idx));
            for m in &raw.matchers {
                matchers.push((fnv1a32(&m.string), idx));
                if let Some(a) = &m.advanced {
                    matchers.push((fnv1a32(a), idx));
                }
            }
            let trade_ids = raw
                .trade
                .ids
                .unwrap_or_default()
                .into_iter()
                .map(|(k, v)| {
                    let ids = v
                        .as_array()
                        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
                        .unwrap_or_default();
                    (k, ids)
                })
                .collect();
            stats.push(Arc::new(Stat {
                ref_: raw.ref_,
                better: match raw.better {
                    1 => Better::Positive,
                    -1 => Better::Negative,
                    _ => Better::NotComparable,
                },
                dp: raw.dp,
                matchers: raw.matchers,
                trade_ids,
                inverted: raw.trade.inverted,
                option: raw.trade.option,
            }));
        }
        Ok(StatDb { stats, by_ref: HashIndex::build(refs), by_matcher: HashIndex::build(matchers) })
    }

    pub fn is_empty(&self) -> bool {
        self.stats.is_empty()
    }

    /// The stat a line translates to, with the matcher that matched.
    pub fn by_match_str(&self, s: &str) -> Option<(Arc<Stat>, Matcher)> {
        let stat = &self.stats[self.by_matcher.find(s)?];
        let m = stat
            .matchers
            .iter()
            .find(|m| m.string == s || m.advanced.as_deref() == Some(s))?;
        Some((stat.clone(), m.clone()))
    }

    pub fn by_ref(&self, r: &str) -> Option<Arc<Stat>> {
        self.by_ref.find(r).map(|i| self.stats[i].clone())
    }
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct Craftable {
    pub category: String,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct UniqueInfo {
    pub base: String,
    #[serde(default, rename = "fixedStats")]
    pub fixed_stats: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct GemInfo {
    #[serde(default)]
    pub awakened: bool,
    #[serde(default)]
    pub transfigured: bool,
    #[serde(default, rename = "normalVariant")]
    pub normal_variant: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct ArmourInfo {
    #[serde(default)]
    pub ar: Option<[f64; 2]>,
    #[serde(default)]
    pub ev: Option<[f64; 2]>,
    #[serde(default)]
    pub es: Option<[f64; 2]>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct RefOnly {
    #[serde(rename = "ref")]
    pub ref_: String,
}

/// What tells two bases with one name apart.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct Disc {
    #[serde(default, rename = "propAR")]
    pub prop_ar: bool,
    #[serde(default, rename = "propEV")]
    pub prop_ev: bool,
    #[serde(default, rename = "propES")]
    pub prop_es: bool,
    #[serde(default, rename = "mapTier")]
    pub map_tier: Option<String>,
    #[serde(default, rename = "hasImplicit")]
    pub has_implicit: Option<RefOnly>,
    #[serde(default, rename = "hasExplicit")]
    pub has_explicit: Option<RefOnly>,
    #[serde(default, rename = "sectionText")]
    pub section_text: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct MapInfo {
    #[serde(default)]
    pub tier: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct BaseType {
    #[serde(default)]
    pub name: String,
    #[serde(default, rename = "refName")]
    pub ref_name: String,
    pub namespace: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub craftable: Option<Craftable>,
    #[serde(default)]
    pub unique: Option<UniqueInfo>,
    #[serde(default)]
    pub gem: Option<GemInfo>,
    #[serde(default)]
    pub armour: Option<ArmourInfo>,
    #[serde(default)]
    pub disc: Option<Disc>,
    #[serde(default)]
    pub map: Option<MapInfo>,
    #[serde(default, rename = "tradeTag")]
    pub trade_tag: Option<String>,
    #[serde(default, rename = "tradeDisc")]
    pub trade_disc: Option<String>,
}

#[derive(Debug, Default)]
pub struct ItemDb {
    items: Vec<BaseType>,
    by_name: HashIndex,
    by_ref: HashIndex,
}

impl ItemDb {
    pub fn from_ndjson(text: &str) -> Result<ItemDb, serde_json::Error> {
        let mut items: Vec<BaseType> = Vec::new();
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            items.push(serde_json::from_str(line)?);
        }
        // One index row per `namespace::refName`, at its first line; the
        // variants of a name follow it directly in the file.
        let mut seen: HashMap<String, ()> = HashMap::new();
        let (mut names, mut refs) = (Vec::new(), Vec::new());
        for (idx, it) in items.iter().enumerate() {
            let key = format!("{}::{}", it.namespace, it.ref_name);
            if seen.insert(key.clone(), ()).is_none() {
                names.push((fnv1a32(&format!("{}::{}", it.namespace, it.name)), idx));
                refs.push((fnv1a32(&key), idx));
            }
        }
        Ok(ItemDb { items, by_name: HashIndex::build(names), by_ref: HashIndex::build(refs) })
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    fn find(&self, index: &HashIndex, by_ref: bool, ns: &str, name: &str) -> Vec<&BaseType> {
        let mut out = Vec::new();
        let Some(start) = index.find(&format!("{ns}::{name}")) else { return out };
        for rec in &self.items[start..] {
            let field = if by_ref { &rec.ref_name } else { &rec.name };
            if rec.namespace != ns || field != name {
                break;
            }
            out.push(rec);
            if rec.disc.is_none() && rec.unique.is_none() {
                break;
            }
        }
        out
    }

    pub fn by_ref(&self, ns: &str, name: &str) -> Vec<&BaseType> {
        self.find(&self.by_ref, true, ns, name)
    }

    pub fn by_translated(&self, ns: &str, name: &str) -> Vec<&BaseType> {
        self.find(&self.by_name, false, ns, name)
    }
}

/// The trade site's own stat catalog keyed by stat text, for the lines
/// stats.ndjson does not cover: text -> mod type -> ids.
#[derive(Debug, Default)]
pub struct TradeStatTexts {
    by_text: HashMap<String, Vec<(String, Vec<String>)>>,
}

impl TradeStatTexts {
    pub fn from_json(json: &str) -> Result<TradeStatTexts, serde_json::Error> {
        #[derive(Deserialize)]
        struct Root {
            result: Vec<Group>,
        }
        #[derive(Deserialize)]
        struct Group {
            id: String,
            entries: Vec<Entry>,
        }
        #[derive(Deserialize)]
        struct Entry {
            id: String,
            text: String,
        }
        let root: Root = serde_json::from_str(json)?;
        let mut by_text: HashMap<String, Vec<(String, Vec<String>)>> = HashMap::new();
        for g in root.result {
            for e in g.entries {
                let types = by_text.entry(e.text).or_default();
                match types.iter_mut().find(|(t, _)| *t == g.id) {
                    Some((_, ids)) => ids.push(e.id),
                    None => types.push((g.id.clone(), vec![e.id])),
                }
            }
        }
        Ok(TradeStatTexts { by_text })
    }

    pub fn get(&self, text: &str) -> Option<&Vec<(String, Vec<String>)>> {
        self.by_text.get(text)
    }
}

/// Everything the EE2 path reads. `trade_stats` is optional: without it a
/// line stats.ndjson does not know is reported unknown instead of being
/// looked up in the trade catalog by its text.
#[derive(Debug, Default)]
pub struct Ee2Data {
    pub stats: StatDb,
    pub items: ItemDb,
    pub trade_stats: Option<TradeStatTexts>,
    /// Every name the trade site lists (`/api/trade2/data/items`), for
    /// items newer than items.ndjson.
    pub trade_items: Option<std::collections::HashSet<String>>,
}

impl Ee2Data {
    pub fn from_ndjson(stats: &str, items: &str) -> Result<Ee2Data, serde_json::Error> {
        Ok(Ee2Data {
            stats: StatDb::from_ndjson(stats)?,
            items: ItemDb::from_ndjson(items)?,
            trade_stats: None,
            trade_items: None,
        })
    }
}

/// The names in a `/api/trade2/data/items` response: each entry's base
/// type, and its full text where it has one (uniques).
pub fn trade_item_names(json: &str) -> Result<std::collections::HashSet<String>, serde_json::Error> {
    #[derive(Deserialize)]
    struct Root {
        result: Vec<Group>,
    }
    #[derive(Deserialize)]
    struct Group {
        entries: Vec<Entry>,
    }
    #[derive(Deserialize)]
    struct Entry {
        #[serde(default, rename = "type")]
        type_: Option<String>,
        #[serde(default)]
        text: Option<String>,
    }
    let root: Root = serde_json::from_str(json)?;
    let mut out = std::collections::HashSet::new();
    for e in root.result.into_iter().flat_map(|g| g.entries) {
        out.extend(e.type_.filter(|t| !t.is_empty()));
        out.extend(e.text.filter(|t| !t.is_empty()));
    }
    Ok(out)
}
