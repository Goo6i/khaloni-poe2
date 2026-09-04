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

/// Cache file recording which reference-data pin the files beside it were
/// fetched under (see `refdata::data_pin`).
pub const PIN_FILE: &str = "refdata.pin";

/// Every cache file that holds reference data from an upstream source, and
/// therefore goes stale when the game patches. Price caches are not here:
/// they are keyed by league and refreshed on a timer.
pub fn pinned_files() -> Vec<String> {
    let mut names = vec![
        "ee2_stats.ndjson".to_string(),
        "ee2_items.ndjson".to_string(),
        "repoe_mods.json".to_string(),
        "trade_stats.json".to_string(),
        "xile_uniques.json".to_string(),
        "xile_keystones.json".to_string(),
        "xile_leveling.json".to_string(),
    ];
    names.extend(XILE_CATEGORIES.iter().map(|(slug, _)| format!("xile_{slug}.json")));
    names
}

/// Drops reference files fetched under a previous data pin, so a release
/// that bumps the pinned upstream commits reaches installs that already
/// hold a cache. Runs once per process before anything reads the cache;
/// a matching pin costs one small file read.
pub fn sync_pin(cache_dir: &std::path::Path) {
    let pin = khaloni_poe2_core::refdata::data_pin();
    let pin_path = cache_dir.join(PIN_FILE);
    if std::fs::read_to_string(&pin_path).ok().as_deref() == Some(pin.as_str()) {
        return;
    }
    for name in pinned_files() {
        let path = cache_dir.join(&name);
        match std::fs::remove_file(&path) {
            Ok(()) => eprintln!("reference data: dropped {name} (pinned data changed)"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => eprintln!("reference data: could not drop {name}: {e}"),
        }
    }
    let _ = std::fs::create_dir_all(cache_dir);
    if let Err(e) = std::fs::write(&pin_path, &pin) {
        eprintln!("reference data: could not record pin: {e}");
    }
}

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
    sync_pin(cache_dir);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("khalonipoe2-pin-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn stale_pin_drops_reference_files_and_records_the_current_pin() {
        let dir = temp_dir("stale");
        for name in pinned_files() {
            std::fs::write(dir.join(name), "old").unwrap();
        }
        std::fs::write(dir.join("Forbidden Rites-Currency.json"), "prices").unwrap();
        std::fs::write(dir.join(PIN_FILE), "ee2=previous\n").unwrap();
        sync_pin(&dir);
        for name in pinned_files() {
            assert!(!dir.join(&name).exists(), "{name} survived a pin change");
        }
        assert!(dir.join("Forbidden Rites-Currency.json").exists(), "price cache is not pinned");
        assert_eq!(
            std::fs::read_to_string(dir.join(PIN_FILE)).unwrap(),
            khaloni_poe2_core::refdata::data_pin()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn matching_pin_leaves_the_cache_alone() {
        let dir = temp_dir("match");
        std::fs::write(dir.join(PIN_FILE), khaloni_poe2_core::refdata::data_pin()).unwrap();
        std::fs::write(dir.join("ee2_stats.ndjson"), "current").unwrap();
        sync_pin(&dir);
        assert_eq!(std::fs::read_to_string(dir.join("ee2_stats.ndjson")).unwrap(), "current");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_pin_is_treated_as_stale() {
        let dir = temp_dir("missing");
        std::fs::write(dir.join("trade_stats.json"), "from before pins existed").unwrap();
        sync_pin(&dir);
        assert!(!dir.join("trade_stats.json").exists());
        assert!(dir.join(PIN_FILE).exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
