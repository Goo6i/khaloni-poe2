//! Reference-data parsing for the lookup layer. Source is the repoe-fork
//! PoE2 export (JSON objects keyed by an index string; each value carries an
//! item's fields). Base items and uniques have clean, complete fields and
//! are parsed here. Affix rendering is intentionally NOT done from these
//! files: the PoE2 export omits stat_translations, so mod stat-ids cannot be
//! turned into accurate readable text; that needs a translation source
//! (poe2db) before an affix panel is worth shipping.

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

/// A unique item with its full rolled effects, from the XileHUD PoE2 dataset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UniqueDetail {
    pub name: String,
    pub base: String,
    pub mods: Vec<String>,
}

/// A passive keystone (name + effect text), from the XileHUD dataset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Keystone {
    pub name: String,
    pub description: String,
}

/// Strips HTML tags and decodes the few entities XileHUD's mod strings use, so
/// effect text renders as safe plain text (no innerHTML needed on the client).
fn strip_html(s: &str) -> String {
    // Line breaks first, so multi-line descriptions keep their line structure.
    let s = s
        .replace("<br>", "\n")
        .replace("<br/>", "\n")
        .replace("<br />", "\n");
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&nbsp;", " ").replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">")
}

/// Parses XileHUD `Uniques.json` (`{uniques:{Weapon|Armour|Other:[{name,
/// typeLine, explicitMods:[html]}]}}`) into a flat, name-sorted list with the
/// effect text cleaned to plain strings.
pub fn parse_xile_uniques(json: &str) -> Vec<UniqueDetail> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Some(groups) = v.get("uniques").and_then(|u| u.as_object()) {
        for arr in groups.values() {
            for it in arr.as_array().into_iter().flatten() {
                let Some(name) = it.get("name").and_then(|n| n.as_str()) else { continue };
                let base = it.get("typeLine").and_then(|t| t.as_str()).unwrap_or("").to_string();
                let mods = it
                    .get("explicitMods")
                    .and_then(|m| m.as_array())
                    .map(|a| a.iter().filter_map(|m| m.as_str()).map(strip_html).collect())
                    .unwrap_or_default();
                out.push(UniqueDetail { name: name.to_string(), base, mods });
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Parses XileHUD `Keystones.json` (`{keystones:[{name,description}]}`).
pub fn parse_keystones(json: &str) -> Vec<Keystone> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Some(arr) = v.get("keystones").and_then(|k| k.as_array()) {
        for it in arr {
            if let Some(name) = it.get("name").and_then(|n| n.as_str()) {
                out.push(Keystone {
                    name: name.to_string(),
                    description: strip_html(it.get("description").and_then(|d| d.as_str()).unwrap_or("")),
                });
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Case-insensitive search over uniques by name, base, or any mod text.
pub fn search_uniques<'a>(uniques: &'a [UniqueDetail], query: &str) -> Vec<&'a UniqueDetail> {
    let q = query.to_lowercase();
    uniques
        .iter()
        .filter(|u| {
            u.name.to_lowercase().contains(&q)
                || u.base.to_lowercase().contains(&q)
                || u.mods.iter().any(|m| m.to_lowercase().contains(&q))
        })
        .collect()
}

/// Case-insensitive search over keystones by name or description.
pub fn search_keystones<'a>(keystones: &'a [Keystone], query: &str) -> Vec<&'a Keystone> {
    let q = query.to_lowercase();
    keystones
        .iter()
        .filter(|k| k.name.to_lowercase().contains(&q) || k.description.to_lowercase().contains(&q))
        .collect()
}

/// A generic reference catalog entry (name + effect/description lines), used
/// for the XileHUD categories that share the `{name, explicitMods|description}`
/// shape: essences, omens, catalysts, currency, annoints, and the like.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RefEntry {
    pub name: String,
    pub lines: Vec<String>,
}

fn pretty_slug(s: &str) -> String {
    s.rsplit('/').next().unwrap_or(s).replace('_', " ").trim().to_string()
}

fn entry_lines(v: &serde_json::Value) -> Vec<String> {
    let mut lines = Vec::new();
    // Any of the mod arrays a XileHUD entry may carry; a mod string can itself
    // hold <br>-separated lines, so split those too.
    for field in ["explicitMods", "enchantMods", "implicitMods"] {
        if let Some(mods) = v.get(field).and_then(|m| m.as_array()) {
            for s in mods.iter().filter_map(|m| m.as_str()) {
                lines.extend(strip_html(s).lines().map(str::to_string).filter(|l| !l.trim().is_empty()));
            }
        }
    }
    if lines.is_empty() {
        if let Some(desc) = v.get("description").and_then(|d| d.as_str()) {
            lines.extend(strip_html(desc).lines().map(str::to_string).filter(|l| !l.trim().is_empty()));
        }
    }
    lines
}

/// Generic parser for a XileHUD reference file whose data is one array (or a
/// dict of arrays) under the single non-`slug` key, each element `{name,
/// explicitMods|description, ...}`. Empty-name entries fall back to a
/// prettified slug; `[DNT]` (dev/untranslated) and duplicate names are dropped.
pub fn parse_xile_category(json: &str) -> Vec<RefEntry> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let mut raw: Vec<&serde_json::Value> = Vec::new();
    if let Some(arr) = v.as_array() {
        // Some files are a bare top-level array of entries.
        raw.extend(arr.iter());
    } else if let Some(obj) = v.as_object() {
        // Others wrap the data (array, or dict-of-arrays) under one key.
        for (k, val) in obj {
            if k == "slug" {
                continue;
            }
            match val {
                serde_json::Value::Array(a) => raw.extend(a.iter()),
                serde_json::Value::Object(groups) => {
                    for gv in groups.values() {
                        if let Some(a) = gv.as_array() {
                            raw.extend(a.iter());
                        }
                    }
                }
                _ => {}
            }
        }
    } else {
        return Vec::new();
    }
    let mut out = Vec::new();
    for e in raw {
        let Some(o) = e.as_object() else { continue };
        let name = o
            .get("name")
            .and_then(|n| n.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(String::from)
            .or_else(|| o.get("slug").and_then(|s| s.as_str()).map(pretty_slug))
            .unwrap_or_default();
        if name.is_empty() || name.contains("[DNT]") {
            continue;
        }
        out.push(RefEntry { name, lines: entry_lines(e) });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out.dedup_by(|a, b| a.name == b.name);
    out
}

/// Case-insensitive search over generic entries by name or any line.
pub fn search_ref_entries<'a>(entries: &'a [RefEntry], query: &str) -> Vec<&'a RefEntry> {
    let q = query.to_lowercase();
    entries
        .iter()
        .filter(|e| e.name.to_lowercase().contains(&q) || e.lines.iter().any(|l| l.to_lowercase().contains(&q)))
        .collect()
}

/// One step of the leveling guide.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LevelingStep {
    pub id: String,
    pub kind: String,
    pub zone: String,
    pub description: String,
    pub hint: String,
}

/// An act with its ordered leveling steps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LevelingAct {
    pub act: u32,
    pub name: String,
    pub steps: Vec<LevelingStep>,
}

/// Parses XileHUD `leveling-data-v2.json` (`{acts:[{actNumber,actName,steps:
/// [{id,type,zone,description,hint}]}]}`) into acts of steps.
pub fn parse_leveling(json: &str) -> Vec<LevelingAct> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let s = |o: &serde_json::Value, k: &str| o.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let mut out = Vec::new();
    for a in v.get("acts").and_then(|x| x.as_array()).into_iter().flatten() {
        let steps = a
            .get("steps")
            .and_then(|x| x.as_array())
            .into_iter()
            .flatten()
            .map(|st| LevelingStep {
                id: s(st, "id"),
                kind: s(st, "type"),
                zone: s(st, "zone"),
                description: s(st, "description"),
                hint: s(st, "hint"),
            })
            .collect();
        out.push(LevelingAct {
            act: a.get("actNumber").and_then(|n| n.as_u64()).unwrap_or(0) as u32,
            name: s(a, "actName"),
            steps,
        });
    }
    out
}

/// Downloads a XileHUD PoE2 data file by its path under `data/poe2/` (the
/// segment(s) after that, URL-encoded, ending in `.json`). Pinned commit.
pub fn fetch_xile_path(rel: &str) -> Result<String, String> {
    const COMMIT: &str = "cdec6065f7e3240d878edb0363c5f1918e0851f4";
    let url = format!("https://raw.githubusercontent.com/XileHUD/poe_overlay/{COMMIT}/data/poe2/{rel}");
    let http = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("Mozilla/5.0 poe2-lens/0.1")
        .build()
        .map_err(|e| e.to_string())?;
    let resp = http.get(&url).send().map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("{rel} status {}", resp.status()));
    }
    resp.text().map_err(|e| e.to_string())
}

/// Downloads one XileHUD PoE2 reference file (e.g. "Uniques", "Keystones")
/// from the current league dir. Pinned to a verified commit. Caller caches it.
pub fn fetch_xile_json(file: &str) -> Result<String, String> {
    const COMMIT: &str = "cdec6065f7e3240d878edb0363c5f1918e0851f4";
    let url = format!(
        "https://raw.githubusercontent.com/XileHUD/poe_overlay/{COMMIT}/data/poe2/Rise%20of%20the%20Abyssal/{file}.json"
    );
    let http = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("Mozilla/5.0 poe2-lens/0.1")
        .build()
        .map_err(|e| e.to_string())?;
    let resp = http.get(&url).send().map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("{file}.json status {}", resp.status()));
    }
    resp.text().map_err(|e| e.to_string())
}

/// Downloads one Exiled-Exchange-2 PoE2 data file ("stats" or "items") as
/// ndjson text. Pinned to a verified commit so the format cannot shift under
/// us; needs a browser-like User-Agent past GitHub. Callers cache the result.
pub fn fetch_ee2_ndjson(kind: &str) -> Result<String, String> {
    const COMMIT: &str = "acc7653f05629228f12e273ab1b8da3e46d6bcd1";
    let url = format!(
        "https://raw.githubusercontent.com/Kvan7/Exiled-Exchange-2/{COMMIT}/renderer/public/data/en/{kind}.ndjson"
    );
    let http = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("Mozilla/5.0 poe2-lens/0.1")
        .build()
        .map_err(|e| e.to_string())?;
    let resp = http.get(&url).send().map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("{kind}.ndjson status {}", resp.status()));
    }
    resp.text().map_err(|e| e.to_string())
}

// --- Exiled-Exchange-2 data (the richer PoE2 source with readable affix text
// that repoe-fork lacks). Both files are newline-delimited JSON (ndjson), one
// object per line: stats.ndjson for affixes, items.ndjson for bases/uniques/
// gems. ---

/// A modifier the game can roll, with its in-game readable text (`#` marks the
/// rolled value) and the trade stat ids it maps to. From EE2 `stats.ndjson`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Affix {
    pub text: String,
    pub trade_ids: Vec<String>,
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
pub fn parse_affixes(ndjson: &str) -> Vec<Affix> {
    let mut out = Vec::new();
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
        out.push(Affix { text: text.to_string(), trade_ids });
    }
    out.sort_by(|a, b| a.text.cmp(&b.text));
    out
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
pub fn search_affixes<'a>(affixes: &'a [Affix], query: &str) -> Vec<&'a Affix> {
    let q = query.to_lowercase();
    affixes.iter().filter(|a| a.text.to_lowercase().contains(&q)).collect()
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
    fn parses_xile_uniques_and_keystones_stripping_html() {
        let uniques = concat!(
            r##"{"slug":"Uniques","uniques":{"Weapon":["##,
            r##"{"name":"Brynhand's Mark","typeLine":"Wooden Club","explicitMods":["##,
            r##""Adds <span class=\"mod-value\">(10-14)</span> Physical Damage","Causes Double Stun"]}"##,
            r##"],"Armour":[],"Other":[]}}"##,
        );
        let u = parse_xile_uniques(uniques);
        assert_eq!(u.len(), 1);
        assert_eq!(u[0].name, "Brynhand's Mark");
        assert_eq!(u[0].base, "Wooden Club");
        assert_eq!(u[0].mods[0], "Adds (10-14) Physical Damage", "html stripped");
        assert_eq!(search_uniques(&u, "double stun").len(), 1, "searches mod text");

        let ks = r##"{"slug":"Keystones","keystones":[{"name":"Resolute Technique","description":"Accuracy is Doubled<br>Never deal Critical Hits"}]}"##;
        let k = parse_keystones(ks);
        assert_eq!(k.len(), 1);
        assert_eq!(k[0].name, "Resolute Technique");
        assert!(k[0].description.contains("Never deal Critical Hits"));
        assert_eq!(search_keystones(&k, "critical").len(), 1);
    }

    #[test]
    fn generic_category_handles_explicitmods_slug_fallback_and_dnt() {
        let json = concat!(
            r##"{"slug":"essences","essences":["##,
            r##"{"name":"","slug":"Lesser_Essence_of_the_Body","explicitMods":["Armour: <span>+30</span> to maximum Life","  "]},"##,
            r##"{"name":"Omen of Foo","explicitMods":["Line one<br>line two"]},"##,
            r##"{"name":"[DNT] dev","explicitMods":["x"]}"##,
            r##"]}"##,
        );
        let e = parse_xile_category(json);
        assert_eq!(e.len(), 2, "DNT dropped");
        let body = e.iter().find(|x| x.name == "Lesser Essence of the Body").expect("slug fallback name");
        assert_eq!(body.lines, vec!["Armour: +30 to maximum Life"], "html stripped, blanks dropped");
        let omen = e.iter().find(|x| x.name == "Omen of Foo").unwrap();
        assert_eq!(omen.lines, vec!["Line one", "line two"], "<br> split into lines");
        assert_eq!(search_ref_entries(&e, "maximum life").len(), 1);
    }

    #[test]
    fn parses_leveling_acts_and_steps() {
        let json = r##"{"acts":[{"actNumber":1,"actName":"Grelwood","steps":[
            {"id":"a1_1","type":"kill_boss","zone":"The Riverbank","description":"Kill The Bloated Miller","hint":"use skill point"},
            {"id":"a1_2","type":"waypoint","zone":"Clearfell","description":"Take the waypoint"}
        ]}]}"##;
        let acts = parse_leveling(json);
        assert_eq!(acts.len(), 1);
        assert_eq!(acts[0].act, 1);
        assert_eq!(acts[0].name, "Grelwood");
        assert_eq!(acts[0].steps.len(), 2);
        assert_eq!(acts[0].steps[0].kind, "kill_boss");
        assert_eq!(acts[0].steps[0].description, "Kill The Bloated Miller");
        assert_eq!(acts[0].steps[1].hint, "", "missing hint -> empty");
    }

    #[test]
    fn generic_category_handles_top_level_array_and_enchant_mods() {
        let json = r##"[{"name":"Trap A","explicitMods":["deals damage"]},{"name":"Emotion","enchantMods":["Allocates Point Blank"]}]"##;
        let e = parse_xile_category(json);
        assert_eq!(e.len(), 2);
        assert_eq!(e.iter().find(|x| x.name == "Emotion").unwrap().lines, vec!["Allocates Point Blank"]);
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
