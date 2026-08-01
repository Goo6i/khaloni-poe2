//! Reference-data loading: reads the on-disk cache and fetches from EE2 /
//! XileHUD once per missing file. Sole loader for `core::refdata`; consumed
//! by the in-overlay Reference and Leveling panels.

use std::collections::HashMap;

use khaloni_poe2_core::refdata::{Affix, Keystone, LevelingAct, RefEntry, RefItem, UniqueDetail};

/// Loads the reference data (affixes + catalog items), reading the on-disk
/// cache under `cache_dir` and fetching from EE2 once when a file is missing.
/// Never fails hard: a fetch error yields an empty index so the panels still
/// run (they just show nothing until the next successful fetch).
pub struct Reference {
    pub affixes: Vec<Affix>,
    pub items: Vec<RefItem>,
    pub uniques: Vec<UniqueDetail>,
    pub keystones: Vec<Keystone>,
    pub categories: HashMap<String, Vec<RefEntry>>,
    pub leveling: Vec<LevelingAct>,
}

/// Generic XileHUD reference categories: (API slug, XileHUD file name).
pub const XILE_CATEGORIES: &[(&str, &str)] = &[
    ("essences", "Essences"),
    ("omens", "Omens"),
    ("catalysts", "Catalysts"),
    ("currency", "Currency"),
    ("annoints", "Annoints"),
    ("ascendancy", "Ascendancy_Passives"),
    ("emotions", "Liquid_Emotions"),
    ("atlas", "Atlas_Nodes"),
    // Mechanic references (cached since 2026-07-25, previously unwired):
    // searchable in the F9 panel; panel-reading advisors for these
    // mechanics need live fixtures first (KHALONI_REGION_DUMP).
    ("ritual", "Ritual"),
    ("expedition", "Expedition"),
    ("breach", "Breach"),
    ("delirium", "Delirium"),
    ("strongbox", "Strongbox"),
    ("traps", "Traps"),
    ("charms", "Charms"),
];

/// A cached-or-fetched file: reads `cache_dir/name`, else runs `fetch` once and
/// caches it. Empty string on failure so the panels still run.
fn cached(
    cache_dir: &std::path::Path,
    name: &str,
    fetch: impl FnOnce() -> Result<String, String>,
) -> String {
    let path = cache_dir.join(name);
    if let Ok(s) = std::fs::read_to_string(&path) {
        if !s.trim().is_empty() {
            return s;
        }
    }
    match fetch() {
        Ok(s) => {
            let _ = std::fs::create_dir_all(cache_dir);
            let _ = std::fs::write(&path, &s);
            s
        }
        Err(e) => {
            eprintln!("reference data: {name} fetch failed: {e}");
            String::new()
        }
    }
}

pub fn reference_data(cache_dir: &std::path::Path) -> Reference {
    use khaloni_poe2_core::refdata as rd;
    let mut categories = HashMap::new();
    for (slug, file) in XILE_CATEGORIES {
        let json = cached(cache_dir, &format!("xile_{slug}.json"), || rd::fetch_xile_json(file));
        categories.insert(slug.to_string(), rd::parse_xile_category(&json));
    }
    // Affix text comes from EE2; the repoe mods export joins onto it (by
    // internal stat id) to attach roll-tier ladders. A missing/failed mods
    // file degrades to affixes without tiers, never to a missing panel.
    let ee2_stats = cached(cache_dir, "ee2_stats.ndjson", || rd::fetch_ee2_ndjson("stats"));
    let repoe_mods = cached(cache_dir, "repoe_mods.json", rd::fetch_repoe_mods);
    Reference {
        affixes: rd::parse_affixes_tiered(&ee2_stats, &repoe_mods),
        items: rd::parse_ref_items(&cached(cache_dir, "ee2_items.ndjson", || rd::fetch_ee2_ndjson("items"))),
        uniques: rd::parse_xile_uniques(&cached(cache_dir, "xile_uniques.json", || rd::fetch_xile_json("Uniques"))),
        keystones: rd::parse_keystones(&cached(cache_dir, "xile_keystones.json", || rd::fetch_xile_json("Keystones"))),
        categories,
        leveling: rd::parse_leveling(&cached(cache_dir, "xile_leveling.json", || {
            rd::fetch_xile_path("Leveling/leveling-data-v2.json")
        })),
    }
}
