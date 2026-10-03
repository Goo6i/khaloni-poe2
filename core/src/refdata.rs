//! Reference-data parsing for the lookup layer. Source is the repoe-fork
//! PoE2 export (JSON objects keyed by an index string; each value carries an
//! item's fields). Base items and uniques have clean, complete fields and
//! are parsed here. Readable affix text comes from EE2 `stats.ndjson`, not
//! from the repoe export; the repoe `mods.json` is joined onto it purely by
//! internal stat id (EE2's `id` field) to attach roll-tier ladders — where
//! that join is ambiguous, an affix simply carries no tiers.

use serde::{Deserialize, Serialize};

/// A base item type (name, class, tags), for a "what is this base" browser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseItem {
    pub name: String,
    pub item_class: String,
    pub tags: Vec<String>,
}

/// A unique item (name + class). The PoE2 export has no unique stat text, so
/// this is a browsable index only, not an effects reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UniqueItem {
    pub name: String,
    pub item_class: String,
}

#[derive(Deserialize)]
struct BaseRow {
    #[serde(default)]
    name: String,
    #[serde(default)]
    item_class: String,
    #[serde(default)]
    tags: Vec<String>,
}

#[derive(Deserialize)]
struct UniqueRow {
    #[serde(default)]
    name: String,
    #[serde(default)]
    item_class: String,
}

/// Parses base_items.json (object of index -> row). Rows with an empty name
/// (metadata/placeholder entries) are skipped. Output is sorted by name for
/// stable browsing.
pub fn parse_base_items(json: &str) -> Vec<BaseItem> {
    let map: std::collections::HashMap<String, BaseRow> =
        serde_json::from_str(json).unwrap_or_default();
    let mut out: Vec<BaseItem> = map
        .into_values()
        .filter(|r| !r.name.trim().is_empty())
        .map(|r| BaseItem { name: r.name, item_class: r.item_class, tags: r.tags })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Parses uniques.json (object of index -> row), skipping empty names,
/// sorted by name.
pub fn parse_uniques(json: &str) -> Vec<UniqueItem> {
    let map: std::collections::HashMap<String, UniqueRow> =
        serde_json::from_str(json).unwrap_or_default();
    let mut out: Vec<UniqueItem> = map
        .into_values()
        .filter(|r| !r.name.trim().is_empty())
        .map(|r| UniqueItem { name: r.name, item_class: r.item_class })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Case-insensitive substring search over base-item names.
pub fn search_bases<'a>(bases: &'a [BaseItem], query: &str) -> Vec<&'a BaseItem> {
    let q = query.to_lowercase();
    bases.iter().filter(|b| b.name.to_lowercase().contains(&q)).collect()
}

/// Upstream commits the reference downloads are pinned to, so a format
/// change upstream cannot break the parsers between our releases. Each is
/// bumped once its repository has published data for a new game patch;
/// bumping either changes [`data_pin`], which the app uses to drop cache
/// files fetched under the previous pin.
///
/// XileHUD/poe_overlay: v0.6.11 (2026-06-19). Its data still lives in a
/// `Rise of the Abyssal` directory and has not changed since.
pub const XILE_COMMIT: &str = "cdec6065f7e3240d878edb0363c5f1918e0851f4";
/// Kvan7/Exiled-Exchange-2: master of 2026-09-06, verified to parse with
/// 2526 affixes and 4058 items. This is also the commit `core::ee2` was
/// ported from and the one `tools/ee2-parity` holds it to: its data files
/// there are byte-for-byte the two this pin downloads, so the app searches
/// with the data the parity test passed on. Bump the three together.
pub const EE2_COMMIT: &str = "cca30662bf31eaf38bd711e2ec1a6b899a06c40e";

/// Identity of the reference-data set this build expects on disk: the
/// pinned commits above. The unpinned downloads (repoe mods, trade stats)
/// are keyed to it too, since a pin bump is exactly when a game patch has
/// changed them as well.
pub fn data_pin() -> String {
    format!("xile={XILE_COMMIT}\nee2={EE2_COMMIT}\n")
}

/// Downloads a XileHUD PoE2 data file by its path under `data/poe2/` (the
/// segment(s) after that, URL-encoded, ending in `.json`). Pinned commit.
pub fn fetch_xile_path(rel: &str) -> Result<String, String> {
    let url = format!("https://raw.githubusercontent.com/XileHUD/poe_overlay/{XILE_COMMIT}/data/poe2/{rel}");
    let http = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("Mozilla/5.0 khaloni-poe2/0.1")
        .build()
        .map_err(|e| e.to_string())?;
    let resp = http.get(&url).send().map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("{rel} status {}", resp.status()));
    }
    resp.text().map_err(|e| e.to_string())
}

/// Downloads one Exiled-Exchange-2 PoE2 data file ("stats" or "items") as
/// ndjson text. Pinned to a verified commit so the format cannot shift under
/// us; needs a browser-like User-Agent past GitHub. Callers cache the result.
pub fn fetch_ee2_ndjson(kind: &str) -> Result<String, String> {
    let url = format!(
        "https://raw.githubusercontent.com/Kvan7/Exiled-Exchange-2/{EE2_COMMIT}/renderer/public/data/en/{kind}.ndjson"
    );
    let http = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("Mozilla/5.0 khaloni-poe2/0.1")
        .build()
        .map_err(|e| e.to_string())?;
    let resp = http.get(&url).send().map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("{kind}.ndjson status {}", resp.status()));
    }
    resp.text().map_err(|e| e.to_string())
}

/// Downloads the repoe-fork PoE2 `mods.json` export (mod families with stat
/// ids, roll ranges, spawn weights, and required levels — the tier source the
/// EE2 files lack). Callers cache the result; it is a ~13 MB file.
pub fn fetch_repoe_mods() -> Result<String, String> {
    let url = "https://repoe-fork.github.io/poe2/mods.json";
    let http = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .user_agent("Mozilla/5.0 khaloni-poe2/0.1")
        .build()
        .map_err(|e| e.to_string())?;
    let resp = http.get(url).send().map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("mods.json status {}", resp.status()));
    }
    resp.text().map_err(|e| e.to_string())
}

/// Downloads the repoe-fork PoE2 `base_items.json` export (every base with
/// its item class and tags, which decide what a base can roll). Callers
/// check it with [`validate_base_items`] and cache the result.
pub fn fetch_repoe_base_items() -> Result<String, String> {
    let url = "https://repoe-fork.github.io/poe2/base_items.json";
    let http = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .user_agent("Mozilla/5.0 khaloni-poe2/0.1")
        .build()
        .map_err(|e| e.to_string())?;
    let resp = http.get(url).send().map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("base_items.json status {}", resp.status()));
    }
    resp.text().map_err(|e| e.to_string())
}

#[derive(Deserialize)]
struct BaseCheckRow {
    #[serde(default)]
    domain: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    item_class: String,
    #[serde(default)]
    tags: Option<Vec<String>>,
}

/// Whether `body` is a base item export the craft planner can use: an
/// object of rows, at least one of them an item-domain base with a name, an
/// item class and a tag list. Valid JSON of another shape (an error object,
/// the mods export saved under the wrong name) is refused, so it never
/// replaces a good cached copy.
pub fn validate_base_items(body: &str) -> Result<(), String> {
    let rows: std::collections::HashMap<String, serde_json::Value> =
        serde_json::from_str(body).map_err(|e| format!("base_items.json is not an object of rows: {e}"))?;
    let usable = rows
        .into_values()
        .filter_map(|v| serde_json::from_value::<BaseCheckRow>(v).ok())
        .filter(|r| r.domain == "item" && !r.name.trim().is_empty() && !r.item_class.trim().is_empty())
        .filter(|r| r.tags.as_ref().is_some_and(|t| !t.is_empty()))
        .count();
    if usable == 0 {
        return Err("base_items.json holds no item base with a name, an item class and tags".to_string());
    }
    Ok(())
}

// --- Exiled-Exchange-2 data (the richer PoE2 source with readable affix text
// that repoe-fork lacks). Both files are newline-delimited JSON (ndjson), one
// object per line: stats.ndjson for affixes, items.ndjson for bases/uniques/
// gems. ---

/// Which affix family a mod rolls in, from the repoe mods export's
/// `generation_type` field. Only `"prefix"` and `"suffix"` map to a family;
/// every other generation type the export carries (`unique`, `corrupted`,
/// `essence`, `torment`, …) — and any affix with no reliable mod join at all —
/// is [`AffixKind::Other`], never a guess.
///
/// This is core's own type. The app has a separate UI-side `AffixKind` for its
/// tiering badge and maps between them; core cannot depend on the app crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize)]
pub enum AffixKind {
    Prefix,
    Suffix,
    /// Not a rollable prefix/suffix, or not known to be one.
    #[default]
    Other,
}

/// Maps a repoe `generation_type` string to a family. Anything unmapped is
/// [`AffixKind::Other`].
fn affix_kind(generation_type: &str) -> AffixKind {
    match generation_type {
        "prefix" => AffixKind::Prefix,
        "suffix" => AffixKind::Suffix,
        _ => AffixKind::Other,
    }
}

/// The family of a whole ladder: the one its tiers agree on. An empty ladder,
/// or one whose tiers disagree (a mod key group that mixes a prefix and a
/// suffix — not present in the real export, but cheap to refuse), is `Other`
/// rather than an arbitrary pick.
fn ladder_kind(tiers: &[AffixTier]) -> AffixKind {
    let mut it = tiers.iter().map(|t| t.kind);
    match it.next() {
        Some(first) if it.all(|k| k == first) => first,
        _ => AffixKind::Other,
    }
}

/// A modifier the game can roll, with its in-game readable text (`#` marks the
/// rolled value) and the trade stat ids it maps to. From EE2 `stats.ndjson`.
/// `tiers` (from the repoe-fork mods export, joined on internal stat id) is
/// the roll ladder where a reliable join exists, empty otherwise. `kind` is
/// the family the joined ladder rolls in — `Other` whenever there is no
/// ladder, or (never observed in the real export) a ladder disagrees with
/// itself about its generation type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Affix {
    pub text: String,
    pub trade_ids: Vec<String>,
    pub tiers: Vec<AffixTier>,
    pub kind: AffixKind,
    /// Spawn weight, with exactly poe2db's provenance: the weight of the
    /// FIRST POSITIVE `spawn_weights` entry on the dominant ladder's base
    /// (lowest `required_level`) mod — the number poe2db displays for the
    /// family. `None` whenever no ladder joined; never a guess.
    pub weight: Option<u32>,
    /// The dominant ladder's lowest rung's repoe `required_level` — the item
    /// level the affix starts existing at. Named so callers need not know
    /// `tiers` is ilvl-ascending. `None` without a ladder.
    pub required_level: Option<u32>,
}

/// One tier of an affix ladder: the item level it starts rolling at, its
/// value range (per-stat ranges joined with ", " for e.g. added-damage mods),
/// and the family the mod behind it rolls in. Sorted ascending by `ilvl`
/// inside `Affix::tiers`, so the last entry is the top tier (T1 as players
/// count them — see [`crate::rollquality`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AffixTier {
    pub ilvl: u32,
    pub range: String,
    pub kind: AffixKind,
}

/// A catalog item (base, unique, or gem) from EE2 `items.ndjson`. `namespace`
/// distinguishes them ("ITEM"/"UNIQUE"/"GEM"/...); `category` is the craftable
/// class where present (e.g. "Support Skill Gem", "Body Armour").
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RefItem {
    pub name: String,
    pub namespace: String,
    pub category: Option<String>,
}

/// Parses EE2 `stats.ndjson` into affixes: each line's `ref` (readable text)
/// plus every trade id under `trade.ids.*`. Lines without a `ref` are skipped.
/// All tiers are empty; use [`parse_affixes_tiered`] to also join the repoe
/// mods export.
pub fn parse_affixes(ndjson: &str) -> Vec<Affix> {
    parse_affixes_tiered(ndjson, "")
}

/// Like [`parse_affixes`], but joins the repoe-fork PoE2 `mods.json` export to
/// attach roll-tier ladders. The join key is the internal stat id: EE2 lines
/// carry it as `id`, repoe mods reference it in `stats[].id`. Only mods that
/// join unambiguously get tiers (see [`affix_tiers`]); everything else keeps
/// an empty ladder rather than a guessed one.
pub fn parse_affixes_tiered(ndjson: &str, mods_json: &str) -> Vec<Affix> {
    // First pass: collect (text, trade_ids, internal id) per EE2 line.
    let mut rows: Vec<(String, Vec<String>, Option<String>)> = Vec::new();
    for line in ndjson.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(text) = v.get("ref").and_then(|r| r.as_str()) else {
            continue;
        };
        let mut trade_ids = Vec::new();
        if let Some(types) = v.pointer("/trade/ids").and_then(|x| x.as_object()) {
            for arr in types.values() {
                if let Some(a) = arr.as_array() {
                    trade_ids.extend(a.iter().filter_map(|id| id.as_str().map(String::from)));
                }
            }
        }
        let id = v.get("id").and_then(|i| i.as_str()).map(String::from);
        rows.push((text.to_string(), trade_ids, id));
    }
    let known: std::collections::HashSet<&str> =
        rows.iter().filter_map(|(_, _, id)| id.as_deref()).collect();
    let mut tiers_by_id = affix_tiers(mods_json, &known);
    let mut out: Vec<Affix> = rows
        .into_iter()
        .map(|(text, trade_ids, id)| {
            let (tiers, weight) =
                id.and_then(|i| tiers_by_id.remove(i.as_str())).unwrap_or_default();
            Affix {
                kind: ladder_kind(&tiers),
                required_level: tiers.first().map(|t| t.ilvl),
                weight,
                text,
                trade_ids,
                tiers,
            }
        })
        .collect();
    out.sort_by(|a, b| a.text.cmp(&b.text));
    out
}

#[derive(Deserialize)]
struct ModStat {
    id: String,
    #[serde(default)]
    min: i64,
    #[serde(default)]
    max: i64,
}

#[derive(Deserialize)]
struct SpawnWeight {
    #[serde(default)]
    tag: String,
    #[serde(default)]
    weight: i64,
}

#[derive(Deserialize)]
struct ModRow {
    #[serde(default)]
    domain: String,
    #[serde(default)]
    generation_type: String,
    #[serde(default)]
    is_essence_only: bool,
    #[serde(default)]
    required_level: u32,
    #[serde(default)]
    spawn_weights: Vec<SpawnWeight>,
    #[serde(default)]
    stats: Vec<ModStat>,
    /// Nullable in the export (internal-only mods have no display text). A
    /// mod without text cannot be verified as a single display line, so it
    /// never contributes tiers.
    #[serde(default)]
    text: Option<String>,
}

/// Builds roll-tier ladders from the repoe-fork `mods.json` export, keyed by
/// internal stat id, restricted to ids in `known` (the ids EE2 can display).
/// Each tier also records its mod's `generation_type` as an [`AffixKind`], so
/// a ladder carries the prefix/suffix classification the eligibility filter
/// below already had to establish. The ladder's spawn weight rides along
/// (see [`Affix::weight`] for the exact provenance).
///
/// The join is deliberately conservative — a mod contributes a tier only when
/// it is:
/// - an `item`-domain prefix/suffix that can actually spawn (some positive
///   spawn weight, not essence-only);
/// - a single display line (`text` without '\n'), so every stat on the mod
///   belongs to the one readable affix line (multi-line hybrids like
///   "+Armour / +ES" would otherwise leak their tiers into the pure ladders);
/// - matched to exactly one known stat id (a mod whose stats hit two known
///   ids is a hybrid of two displayable affixes; its ladder belongs to
///   neither).
///
/// A stat often has parallel ladders for different item classes (e.g.
/// `IncreasedMana1..13` on jewellery vs `IncreasedManaTwoHandWeapon1..11` on
/// staves) that share a `type`; they split cleanly on the mod key with the
/// trailing digits/underscores removed. One compact suffix can only show one
/// ladder, so the one spanning the most item classes (most distinct positive
/// spawn tags; ties: more tiers, then lexicographic key) is kept.
fn affix_tiers(
    mods_json: &str,
    known: &std::collections::HashSet<&str>,
) -> std::collections::HashMap<String, (Vec<AffixTier>, Option<u32>)> {
    use std::collections::{BTreeMap, HashMap};
    let Ok(mods) = serde_json::from_str::<HashMap<String, ModRow>>(mods_json) else {
        return HashMap::new();
    };
    // affix id -> ladder prefix -> mods. BTreeMap for the deterministic
    // lexicographic tie-break.
    let mut grouped: HashMap<&str, BTreeMap<&str, Vec<&ModRow>>> = HashMap::new();
    for (key, m) in &mods {
        let spawnable = m.spawn_weights.iter().any(|w| w.weight > 0);
        let single_line = m.text.as_deref().is_some_and(|t| !t.contains('\n'));
        if m.domain != "item"
            || !(m.generation_type == "prefix" || m.generation_type == "suffix")
            || m.is_essence_only
            || !spawnable
            || m.stats.is_empty()
            || !single_line
        {
            continue;
        }
        let mut hits = m.stats.iter().filter(|s| known.contains(s.id.as_str()));
        let (Some(hit), None) = (hits.next(), hits.next()) else {
            continue;
        };
        let ladder = key.trim_end_matches(|c: char| c.is_ascii_digit() || c == '_');
        grouped.entry(hit.id.as_str()).or_default().entry(ladder).or_default().push(m);
    }
    let mut out = HashMap::new();
    for (affix_id, ladders) in grouped {
        let best = ladders.into_iter().max_by_key(|(prefix, ms)| {
            let tags: std::collections::HashSet<&str> = ms
                .iter()
                .flat_map(|m| &m.spawn_weights)
                .filter(|w| w.weight > 0)
                .map(|w| w.tag.as_str())
                .collect();
            // Reverse the name so max_by_key's "last wins on ties" picks the
            // lexicographically smallest prefix deterministically.
            (tags.len(), ms.len(), std::cmp::Reverse(*prefix))
        });
        let Some((_, mut ms)) = best else { continue };
        ms.sort_by_key(|m| m.required_level);
        // Weight provenance (what poe2db shows): the first POSITIVE
        // spawn_weights entry of the base (lowest required_level) rung. The
        // eligibility filter above already required some positive entry, so
        // this is Some for every kept ladder unless the base rung alone has
        // none.
        let weight = ms.first().and_then(|m| {
            m.spawn_weights.iter().find(|w| w.weight > 0).and_then(|w| u32::try_from(w.weight).ok())
        });
        let tiers = ms
            .into_iter()
            .map(|m| AffixTier {
                ilvl: m.required_level,
                range: format_ranges(&m.stats),
                kind: affix_kind(&m.generation_type),
            })
            .collect();
        out.insert(affix_id.to_string(), (tiers, weight));
    }
    out
}

/// "min-max" per stat ("min" alone when the roll is fixed), joined with ", "
/// for multi-stat single-line mods (added min/max damage).
fn format_ranges(stats: &[ModStat]) -> String {
    stats
        .iter()
        .map(|s| {
            if s.min == s.max {
                s.min.to_string()
            } else {
                format!("{}-{}", s.min, s.max)
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Parses EE2 `items.ndjson` into catalog items, skipping empty names, sorted
/// by name.
pub fn parse_ref_items(ndjson: &str) -> Vec<RefItem> {
    let mut out = Vec::new();
    for line in ndjson.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let name = v.get("name").and_then(|n| n.as_str()).unwrap_or("").trim();
        if name.is_empty() {
            continue;
        }
        out.push(RefItem {
            name: name.to_string(),
            namespace: v.get("namespace").and_then(|n| n.as_str()).unwrap_or("").to_string(),
            category: v
                .pointer("/craftable/category")
                .and_then(|c| c.as_str())
                .map(String::from),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Case-insensitive substring search over affix readable text.
/// Canonical form for matching a rolled mod line against an affix's
/// template: lowercased, sign-stripped, every number run replaced by the
/// `#` placeholder the templates use. "+23 to Accuracy Rating" and
/// "+# to Accuracy Rating" both become "# to accuracy rating".
pub fn normalize_mod_text(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_ascii_digit() {
            out.push('#');
            // Consume the whole number, including interior decimal points
            // ("1.45"), but not a trailing sentence period.
            while i < chars.len()
                && (chars[i].is_ascii_digit()
                    || (chars[i] == '.' && chars.get(i + 1).is_some_and(char::is_ascii_digit)))
            {
                i += 1;
            }
            continue;
        }
        match c {
            // Signs are template noise: "+#" and "#" must agree.
            '+' => {}
            '#' => out.push('#'),
            _ => out.extend(c.to_lowercase()),
        }
        i += 1;
    }
    // Collapse runs of whitespace so spacing differences cannot miss.
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Affixes keyed by `normalize_mod_text`, for looking up a rolled line's
/// tier ladder. First entry wins on collision, which keeps the mapping
/// deterministic across runs.
pub fn affix_index(affixes: &[Affix]) -> std::collections::HashMap<String, &Affix> {
    let mut map = std::collections::HashMap::new();
    for a in affixes {
        map.entry(normalize_mod_text(&a.text)).or_insert(a);
    }
    map
}

pub fn search_affixes<'a>(affixes: &'a [Affix], query: &str) -> Vec<&'a Affix> {
    let q = query.to_lowercase();
    affixes.iter().filter(|a| a.text.to_lowercase().contains(&q)).collect()
}

/// The base type inside a magic item's name. Magic items copy as a single
/// name line ("Kraken Grip Sapphire Ring of the Whelpling"), so the base is
/// recovered the way Exiled Exchange 2 does it: the longest craftable base
/// (namespace ITEM) that appears in the name on word boundaries. `None`
/// when no base fits, which the caller treats as "search mods only".
pub fn magic_base<'a>(items: &'a [RefItem], name: &str) -> Option<&'a str> {
    let padded = format!(" {name} ");
    items
        .iter()
        .filter(|i| i.namespace == "ITEM" && !i.name.is_empty())
        .filter(|i| padded.contains(&format!(" {} ", i.name)))
        .map(|i| i.name.as_str())
        .max_by_key(|n| n.len())
}

/// Case-insensitive substring search over catalog item names, optionally
/// restricted to one namespace (e.g. "UNIQUE" for a unique browser).
pub fn search_ref_items<'a>(items: &'a [RefItem], query: &str, namespace: Option<&str>) -> Vec<&'a RefItem> {
    let q = query.to_lowercase();
    items
        .iter()
        .filter(|i| namespace.is_none_or(|ns| i.namespace == ns))
        .filter(|i| i.name.to_lowercase().contains(&q))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATS: &str = concat!(
        r##"{"ref": "# Charm Slots", "trade": {"ids": {"explicit": ["explicit.stat_2582079000"], "rune": ["rune.stat_554899692"]}}, "id": "num_charm_slots"}"##,
        "\n",
        r##"{"ref": "#% increased Attack Speed", "trade": {"ids": {"explicit": ["explicit.stat_681332047"]}}}"##,
        "\n",
        r##"{"matchers": [{"string": "no ref here"}]}"##,
    );

    const ITEMS: &str = concat!(
        r##"{"name": "Abiding Hex", "namespace": "GEM", "craftable": {"category": "Support Skill Gem"}}"##,
        "\n",
        r##"{"name": "Wanderlust", "namespace": "UNIQUE", "craftable": {"category": "Boots"}}"##,
        "\n",
        r##"{"name": "Emerald Ring", "namespace": "ITEM", "craftable": {"category": "Ring"}}"##,
        "\n",
        r##"{"name": "", "namespace": "ITEM"}"##,
    );

    #[test]
    fn parses_affixes_with_readable_text_and_trade_ids() {
        let a = parse_affixes(STATS);
        // The ref-less line is skipped; results sorted by text.
        assert_eq!(a.len(), 2);
        let atk = a.iter().find(|x| x.text.contains("Attack Speed")).unwrap();
        assert!(atk.trade_ids.contains(&"explicit.stat_681332047".to_string()));
        let charm = a.iter().find(|x| x.text.contains("Charm")).unwrap();
        assert!(charm.trade_ids.contains(&"rune.stat_554899692".to_string()));
        assert_eq!(search_affixes(&a, "attack").len(), 1);
    }

    #[test]
    fn parses_ref_items_by_namespace() {
        let items = parse_ref_items(ITEMS);
        assert_eq!(items.len(), 3, "empty name skipped");
        assert_eq!(search_ref_items(&items, "", Some("UNIQUE")).len(), 1);
        assert_eq!(search_ref_items(&items, "ring", Some("ITEM"))[0].name, "Emerald Ring");
        assert_eq!(search_ref_items(&items, "", Some("GEM"))[0].category.as_deref(), Some("Support Skill Gem"));
    }

    const BASES: &str = r#"{
        "0": {"name": "Bramblejack Placeholder", "item_class": "", "tags": []},
        "1": {"name": "", "item_class": "Body Armour", "tags": ["str_armour"]},
        "2": {"name": "Advanced Plate Vest", "item_class": "Body Armour", "tags": ["armour","str_armour","default"]},
        "3": {"name": "Emerald Ring", "item_class": "Ring", "tags": ["ring","default"]}
    }"#;

    const UNIQUES: &str = r#"{
        "0": {"id":"Bramblejack","name":"Bramblejack","item_class":"Body Armour"},
        "1": {"id":"x","name":"","item_class":"Ring"},
        "2": {"id":"Wanderlust","name":"Wanderlust","item_class":"Boots"}
    }"#;

    #[test]
    fn parse_base_items_skips_empty_and_sorts() {
        let bases = parse_base_items(BASES);
        let names: Vec<&str> = bases.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, ["Advanced Plate Vest", "Bramblejack Placeholder", "Emerald Ring"]);
        let ring = bases.iter().find(|b| b.name == "Emerald Ring").unwrap();
        assert_eq!(ring.item_class, "Ring");
        assert!(ring.tags.contains(&"ring".to_string()));
    }

    #[test]
    fn parse_uniques_skips_empty_and_sorts() {
        let u = parse_uniques(UNIQUES);
        let names: Vec<&str> = u.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, ["Bramblejack", "Wanderlust"]);
    }

    #[test]
    fn search_bases_is_case_insensitive_substring() {
        let bases = parse_base_items(BASES);
        let hits = search_bases(&bases, "ring");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "Emerald Ring");
    }
}
