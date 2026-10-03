//! Reference-data loading: reads the on-disk cache and fetches from EE2 /
//! XileHUD once per missing file. Sole loader for `core::refdata`; the
//! price card grades tiers from the affixes.

use std::collections::HashMap;

use khaloni_poe2_core::refdata::{Affix, RefItem};

/// Loads the reference data (affixes + catalog items), reading the on-disk
/// cache under `cache_dir` and fetching from EE2 once when a file is missing.
/// Never fails hard: a fetch error yields an empty index so the panels still
/// run (they just show nothing until the next successful fetch).
pub struct Reference {
    pub affixes: Vec<Affix>,
    pub items: Vec<RefItem>,
}

/// Cache file recording which reference-data pin the files beside it were
/// fetched under (see `refdata::data_pin`).
pub const PIN_FILE: &str = "refdata.pin";

const REPOE_MODS: &str = "repoe_mods.json";

/// The bases with their item classes and tags, from the same repoe export
/// as the mods, and like it refreshed by age (see [`REPOE_MAX_AGE`]).
const BASE_ITEMS: &str = "base_items.json";

/// The essence texts, which name the modifier each essence guarantees per
/// item class. From the pinned XileHUD commit.
const XILE_ESSENCES: &str = "xile_essences.json";

/// The repoe export follows the live game, not a pinned commit, so it is
/// refreshed by age: often enough to pick up a patch, rarely enough that a
/// 13 MB download is not a launch cost.
const REPOE_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(14 * 24 * 3600);

/// A download that just failed is not tried again for this long, so a
/// caller that re-asks on every price check while GitHub is down does not
/// spend a 30-second timeout each time.
const RETRY_GAP: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// Every cache file downloaded from a pinned upstream commit, and therefore
/// replaced when a release bumps the pin. Not here: price caches (keyed by
/// league, refreshed on a timer), and `repoe_mods.json`, `base_items.json`
/// and `trade_stats.json`, which come from unpinned live sources - a pin
/// bump says nothing about them, and dropping them with it left the price
/// check without its stat catalog whenever the re-download failed.
pub fn pinned_files() -> Vec<String> {
    vec![
        "ee2_stats.ndjson".to_string(),
        "ee2_items.ndjson".to_string(),
        XILE_ESSENCES.to_string(),
    ]
}

/// Whether the files in `cache_dir` were fetched under this build's pin.
pub fn pin_is_current(cache_dir: &std::path::Path) -> bool {
    let pin = khaloni_poe2_core::refdata::data_pin();
    std::fs::read_to_string(cache_dir.join(PIN_FILE)).ok().as_deref() == Some(pin.as_str())
}

/// Reports whether the cached reference data matches this build's pin.
/// Deletes nothing and records nothing: the files of a previous pin stay in
/// place until [`reference_data`] has downloaded and checked a replacement
/// for each, and the new pin is written only after all of them landed. The
/// earlier order - delete everything, record the pin, then download - left
/// an install with no reference data and no reason to fetch it again the
/// first time a download failed.
pub fn sync_pin(cache_dir: &std::path::Path) -> bool {
    let current = pin_is_current(cache_dir);
    if !current {
        eprintln!("reference data: pinned data changed; files are replaced as their downloads succeed");
    }
    current
}

/// Whether `body` is a complete file of the kind `name` says it is. A
/// truncated download or an HTML error page must never become the cache.
fn valid(name: &str, body: &str) -> Result<(), String> {
    if body.trim().is_empty() {
        return Err("empty body".into());
    }
    if name.ends_with(".ndjson") {
        for (i, line) in body.lines().enumerate().filter(|(_, l)| !l.trim().is_empty()) {
            serde_json::from_str::<serde::de::IgnoredAny>(line).map_err(|e| format!("line {}: {e}", i + 1))?;
        }
        Ok(())
    } else if name == BASE_ITEMS {
        khaloni_poe2_core::refdata::validate_base_items(body)
    } else if name == XILE_ESSENCES {
        // The planner reads the "essences" list; a file without one would
        // leave every essence unknown without saying why.
        let v: serde_json::Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
        match v["essences"].as_array() {
            Some(list) if !list.is_empty() => Ok(()),
            _ => Err("no \"essences\" list".into()),
        }
    } else {
        serde_json::from_str::<serde::de::IgnoredAny>(body).map(|_| ()).map_err(|e| e.to_string())
    }
}

/// When each file's download last failed, process-wide (see [`RETRY_GAP`]).
fn recent_failures() -> &'static std::sync::Mutex<HashMap<std::path::PathBuf, std::time::Instant>> {
    static FAILED: std::sync::OnceLock<std::sync::Mutex<HashMap<std::path::PathBuf, std::time::Instant>>> =
        std::sync::OnceLock::new();
    FAILED.get_or_init(Default::default)
}

/// One cache file's content. The copy on disk is served when it is valid
/// and `replace` is false; otherwise `fetch` runs, and its body replaces the
/// file only after passing [`valid`] (written to a temp file and renamed,
/// so the old copy survives a crash too). When the download fails the old
/// valid copy is still returned - data from the previous pin beats none -
/// with `Err` carrying it so the caller knows the file is not current.
/// `Err(None)` means there is nothing usable at all.
fn cached(
    cache_dir: &std::path::Path,
    name: &str,
    replace: bool,
    fetch: &dyn Fn(&str) -> Result<String, String>,
) -> Result<String, Option<String>> {
    let path = cache_dir.join(name);
    let on_disk = std::fs::read_to_string(&path).ok().filter(|s| valid(name, s).is_ok());
    if let (Some(s), false) = (&on_disk, replace) {
        return Ok(s.clone());
    }
    let failed_recently = recent_failures()
        .lock()
        .ok()
        .and_then(|f| f.get(&path).copied())
        .is_some_and(|at| at.elapsed() < RETRY_GAP);
    if failed_recently {
        return Err(on_disk);
    }
    match fetch(name).and_then(|s| valid(name, &s).map(|()| s)) {
        Ok(s) => {
            if let Err(e) = khaloni_poe2_core::ninja::write_cache_atomic(&path, s.as_bytes()) {
                eprintln!("reference data: {name} not cached: {e}");
            }
            Ok(s)
        }
        Err(e) => {
            eprintln!("reference data: {name} fetch failed: {e}");
            if let Ok(mut f) = recent_failures().lock() {
                f.insert(path, std::time::Instant::now());
            }
            Err(on_disk)
        }
    }
}

/// The upstream download for a cache file name.
fn upstream(name: &str) -> Result<String, String> {
    use khaloni_poe2_core::refdata as rd;
    match name {
        "ee2_stats.ndjson" => rd::fetch_ee2_ndjson("stats"),
        "ee2_items.ndjson" => rd::fetch_ee2_ndjson("items"),
        REPOE_MODS => rd::fetch_repoe_mods(),
        BASE_ITEMS => rd::fetch_repoe_base_items(),
        XILE_ESSENCES => rd::fetch_xile_path("Rise%20of%20the%20Abyssal/Essences.json"),
        other => Err(format!("no upstream source for {other}")),
    }
}

/// Brings every pinned file up to this build's pin and returns their
/// contents by name (empty for a file with nothing usable). The pin is
/// recorded last, and only when every file is current: any failure leaves
/// it unrecorded, so the next launch - or the next call - tries again.
fn load_pinned(cache_dir: &std::path::Path, fetch: &dyn Fn(&str) -> Result<String, String>) -> HashMap<String, String> {
    let replace = !pin_is_current(cache_dir);
    let mut all_current = true;
    let mut out = HashMap::new();
    for name in pinned_files() {
        let body = cached(cache_dir, &name, replace, fetch).unwrap_or_else(|old| {
            all_current = false;
            old.unwrap_or_default()
        });
        out.insert(name, body);
    }
    if replace && all_current {
        let pin = khaloni_poe2_core::refdata::data_pin();
        if let Err(e) = khaloni_poe2_core::ninja::write_cache_atomic(&cache_dir.join(PIN_FILE), pin.as_bytes()) {
            eprintln!("reference data: could not record pin: {e}");
        }
    }
    out
}

/// The repoe mods export: the cached copy while it is younger than
/// [`REPOE_MAX_AGE`], else a fresh download, else the old copy.
fn load_repoe(cache_dir: &std::path::Path, fetch: &dyn Fn(&str) -> Result<String, String>) -> String {
    load_aged(cache_dir, REPOE_MODS, fetch).unwrap_or_else(|old| old.unwrap_or_default())
}

/// A file of the live repoe export: the cached copy while it is younger
/// than [`REPOE_MAX_AGE`], else a fresh download that passed [`valid`],
/// else the old copy (as `Err(Some)`, so the caller knows it is not
/// current). `Err(None)` when there is nothing usable at all.
fn load_aged(
    cache_dir: &std::path::Path,
    name: &str,
    fetch: &dyn Fn(&str) -> Result<String, String>,
) -> Result<String, Option<String>> {
    let aged = std::fs::metadata(cache_dir.join(name))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|at| at.elapsed().ok())
        .is_none_or(|age| age > REPOE_MAX_AGE);
    cached(cache_dir, name, aged, fetch)
}

/// The three files the craft planner is built from.
pub struct CraftFiles {
    pub mods: String,
    pub bases: String,
    pub essences: String,
}

/// The craft planner's files from the cache under `cache_dir`, each
/// downloaded when missing (the repoe ones also when older than
/// [`REPOE_MAX_AGE`]) and replaced only by a download that passed its
/// check. A file with no usable copy is an error naming it, so the panel
/// can say what it is waiting for; an `Err` is not final, a later call
/// tries the download again (no sooner than [`RETRY_GAP`]).
pub fn craft_files(cache_dir: &std::path::Path) -> Result<CraftFiles, String> {
    craft_files_with(cache_dir, &upstream)
}

fn craft_files_with(
    cache_dir: &std::path::Path,
    fetch: &dyn Fn(&str) -> Result<String, String>,
) -> Result<CraftFiles, String> {
    let usable = |got: Result<String, Option<String>>, name: &str| {
        got.or_else(|old| old.ok_or_else(|| format!("{name} is not downloaded yet")))
    };
    Ok(CraftFiles {
        mods: usable(load_aged(cache_dir, REPOE_MODS, fetch), REPOE_MODS)?,
        bases: usable(load_aged(cache_dir, BASE_ITEMS, fetch), BASE_ITEMS)?,
        // Replacing a previous pin's copy is `reference_data`'s job, which
        // also records the pin.
        essences: usable(cached(cache_dir, XILE_ESSENCES, false, fetch), XILE_ESSENCES)?,
    })
}

/// Files only a removed feature read, by the directory they were kept in:
/// the leveling guide's cached route and its record of ticked steps.
pub const OBSOLETE_CACHE_FILES: [&str; 1] = ["xile_leveling.json"];
pub const OBSOLETE_CONFIG_FILES: [&str; 1] = ["leveling_done.txt"];

/// Deletes the files only a removed feature used, from the cache and the
/// config directories; nothing else is touched. Returns the paths it
/// removed, for the log.
pub fn remove_obsolete_files(cache_dir: &std::path::Path, config_dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let named = OBSOLETE_CACHE_FILES
        .iter()
        .map(|n| cache_dir.join(n))
        .chain(OBSOLETE_CONFIG_FILES.iter().map(|n| config_dir.join(n)));
    named.filter(|p| p.is_file() && std::fs::remove_file(p).is_ok()).collect()
}

pub fn reference_data(cache_dir: &std::path::Path) -> Reference {
    reference_data_with(cache_dir, &upstream)
}

fn reference_data_with(cache_dir: &std::path::Path, fetch: &dyn Fn(&str) -> Result<String, String>) -> Reference {
    use khaloni_poe2_core::refdata as rd;
    let files = load_pinned(cache_dir, fetch);
    let file = |name: &str| files.get(name).map(String::as_str).unwrap_or("");
    // Affix text comes from EE2; the repoe mods export joins onto it (by
    // internal stat id) to attach roll-tier ladders. A missing/failed mods
    // file degrades to affixes without tiers, never to a missing panel.
    let repoe_mods = load_repoe(cache_dir, fetch);
    Reference {
        affixes: rd::parse_affixes_tiered(file("ee2_stats.ndjson"), &repoe_mods),
        items: rd::parse_ref_items(file("ee2_items.ndjson")),
    }
}

impl Reference {
    /// True when a core part came up empty - a download failed with nothing
    /// cached. Calling [`reference_data`] again re-attempts exactly the
    /// missing files (no sooner than [`RETRY_GAP`] after the failure).
    pub fn is_incomplete(&self) -> bool {
        self.affixes.is_empty() || self.items.is_empty()
    }
}

/// EE2's stat and item data for the price-check search, from the same
/// cache files `reference_data` fills. A failed fetch yields empty data,
/// which the caller reports instead of searching. Prefer
/// [`try_ee2_data`], which lets the caller tell that case apart and ask
/// again later instead of holding on to the empty result.
pub fn ee2_data(cache_dir: &std::path::Path) -> khaloni_poe2_core::ee2::Ee2Data {
    try_ee2_data(cache_dir).unwrap_or_else(|e| {
        eprintln!("reference data: {e}");
        Default::default()
    })
}

/// EE2's stat and item data, or why there is none. An `Err` is not final:
/// each call re-attempts the missing downloads (no sooner than
/// [`RETRY_GAP`] after a failure), so a caller should keep only an `Ok`.
pub fn try_ee2_data(cache_dir: &std::path::Path) -> Result<khaloni_poe2_core::ee2::Ee2Data, String> {
    try_ee2_data_with(cache_dir, &upstream)
}

fn try_ee2_data_with(
    cache_dir: &std::path::Path,
    fetch: &dyn Fn(&str) -> Result<String, String>,
) -> Result<khaloni_poe2_core::ee2::Ee2Data, String> {
    // Whatever is on disk and valid is used as it is: replacing files of a
    // previous pin is `reference_data`'s job, which also records the pin.
    let get = |name: &str| cached(cache_dir, name, false, fetch).map_err(|_| format!("{name} is not downloaded"));
    let (stats, items) = (get("ee2_stats.ndjson")?, get("ee2_items.ndjson")?);
    let data = khaloni_poe2_core::ee2::Ee2Data::from_ndjson(&stats, &items)
        .map_err(|e| format!("EE2 ndjson unreadable: {e}"))?;
    if data.stats.is_empty() || data.items.is_empty() {
        return Err("EE2 reference data is empty".into());
    }
    Ok(data)
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
    fn obsolete_leveling_files_are_removed_and_nothing_else_is() {
        let cache = temp_dir("obsolete-cache");
        let config = temp_dir("obsolete-config");
        for (dir, name) in [(&cache, "xile_leveling.json"), (&config, "leveling_done.txt")] {
            std::fs::write(dir.join(name), "old").unwrap();
        }
        // Everything else in both directories stays, the live files included.
        let keep = [cache.join(XILE_ESSENCES), cache.join(REPOE_MODS), config.join("config.toml"), config.join("profiles.toml")];
        for k in &keep {
            std::fs::write(k, "keep").unwrap();
        }
        // A directory under an obsolete name is not a file to delete.
        std::fs::create_dir_all(config.join("leveling_done.txt.d")).unwrap();

        let mut removed = remove_obsolete_files(&cache, &config);
        removed.sort();
        let mut want = vec![cache.join("xile_leveling.json"), config.join("leveling_done.txt")];
        want.sort();
        assert_eq!(removed, want);
        assert!(!cache.join("xile_leveling.json").exists() && !config.join("leveling_done.txt").exists());
        assert!(keep.iter().all(|k| k.exists()), "only the obsolete files go");
        assert!(config.join("leveling_done.txt.d").is_dir());
        // Once gone, a second start removes nothing.
        assert!(remove_obsolete_files(&cache, &config).is_empty());
        let _ = std::fs::remove_dir_all(&cache);
        let _ = std::fs::remove_dir_all(&config);
    }

    fn old_body(name: &str) -> &'static str {
        if name.ends_with(".ndjson") {
            "{\"old\":1}\n"
        } else {
            "{\"essences\":[{\"old\":true}]}"
        }
    }

    fn new_body(name: &str) -> Result<String, String> {
        Ok(if name.ends_with(".ndjson") {
            "{\"new\":1}\n{\"new\":2}\n".into()
        } else {
            "{\"essences\":[{\"new\":true}]}".into()
        })
    }

    fn seed_previous_pin(dir: &std::path::Path) {
        for name in pinned_files() {
            std::fs::write(dir.join(&name), old_body(&name)).unwrap();
        }
        std::fs::write(dir.join(PIN_FILE), "ee2=previous\n").unwrap();
    }

    #[test]
    fn a_pin_change_deletes_nothing_by_itself() {
        let dir = temp_dir("sync");
        seed_previous_pin(&dir);
        std::fs::write(dir.join("trade_stats.json"), "{}").unwrap();
        assert!(!sync_pin(&dir));
        for name in pinned_files() {
            assert!(dir.join(&name).exists(), "{name} was dropped before its replacement existed");
        }
        assert!(dir.join("trade_stats.json").exists());
        assert_eq!(std::fs::read_to_string(dir.join(PIN_FILE)).unwrap(), "ee2=previous\n");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_failed_download_keeps_the_old_file_and_leaves_the_pin_unrecorded() {
        let dir = temp_dir("fail");
        seed_previous_pin(&dir);
        let fetch = |name: &str| {
            if name == "ee2_items.ndjson" { Err("status 503".to_string()) } else { new_body(name) }
        };
        let files = load_pinned(&dir, &fetch);
        // The failed file still serves its previous content, on disk and in
        // the result; the ones that downloaded are replaced.
        assert_eq!(files["ee2_items.ndjson"], old_body("ee2_items.ndjson"));
        assert_eq!(std::fs::read_to_string(dir.join("ee2_items.ndjson")).unwrap(), old_body("ee2_items.ndjson"));
        assert_eq!(std::fs::read_to_string(dir.join("ee2_stats.ndjson")).unwrap(), new_body("ee2_stats.ndjson").unwrap());
        // No pin, so the next run knows there is still a file to replace.
        assert!(!pin_is_current(&dir));

        // The re-attempt (once the retry gap has passed) finishes the job,
        // and only then is the pin written.
        recent_failures().lock().unwrap().retain(|path, _| !path.starts_with(&dir));
        let files = load_pinned(&dir, &|name: &str| new_body(name));
        assert_eq!(files["ee2_items.ndjson"], new_body("ee2_items.ndjson").unwrap());
        assert!(pin_is_current(&dir));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_body_that_does_not_parse_never_replaces_the_cache() {
        let dir = temp_dir("invalid");
        seed_previous_pin(&dir);
        let fetch = |name: &str| {
            if name == XILE_ESSENCES { Ok("<html>rate limited</html>".to_string()) } else { new_body(name) }
        };
        load_pinned(&dir, &fetch);
        assert_eq!(std::fs::read_to_string(dir.join(XILE_ESSENCES)).unwrap(), old_body(XILE_ESSENCES));
        assert!(!pin_is_current(&dir));
        // A truncated ndjson (last line cut mid-object) is refused too.
        assert!(valid("ee2_stats.ndjson", "{\"a\":1}\n{\"b\":").is_err());
        assert!(valid("ee2_stats.ndjson", "{\"a\":1}\n{\"b\":2}\n").is_ok());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_matching_pin_downloads_nothing() {
        let dir = temp_dir("match");
        seed_previous_pin(&dir);
        std::fs::write(dir.join(PIN_FILE), khaloni_poe2_core::refdata::data_pin()).unwrap();
        let files = load_pinned(&dir, &|name: &str| panic!("{name} fetched under a current pin"));
        assert_eq!(files["ee2_stats.ndjson"], old_body("ee2_stats.ndjson"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn unpinned_files_are_not_part_of_the_pin() {
        let pinned = pinned_files();
        assert!(!pinned.iter().any(|n| n == "trade_stats.json" || n == REPOE_MODS));
        // A fresh repoe copy is served without a download whatever the pin.
        let dir = temp_dir("repoe");
        std::fs::write(dir.join(REPOE_MODS), "{\"mods\":1}").unwrap();
        assert_eq!(load_repoe(&dir, &|n: &str| panic!("{n} fetched while fresh")), "{\"mods\":1}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_ee2_data_is_an_error_that_a_later_call_can_clear() {
        let dir = temp_dir("ee2");
        let down = |_: &str| Err("offline".to_string());
        assert!(try_ee2_data_with(&dir, &down).is_err());
        // Inside the retry gap nothing is fetched again.
        assert!(try_ee2_data_with(&dir, &|n: &str| panic!("{n} refetched inside the retry gap")).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A base row the planner can use, in the export's shape.
    const BASES_OK: &str = r#"{"Metadata/Items/Armours/BodyArmours/FourBodyStr1":{"domain":"item","name":"Soldier Cuirass","item_class":"Body Armour","tags":["str_armour","body_armour","armour","default"]}}"#;
    const BASES_NEW: &str = r#"{"Metadata/Items/Armours/Boots/FourBootsStr1":{"domain":"item","name":"Rough Greaves","item_class":"Boots","tags":["str_armour","boots","armour","default"]}}"#;

    fn age(path: &std::path::Path, days: u64) {
        let then = std::time::SystemTime::now() - std::time::Duration::from_secs(days * 24 * 3600);
        std::fs::File::options().write(true).open(path).unwrap().set_modified(then).unwrap();
    }

    #[test]
    fn base_items_is_a_pinned_download_with_validation() {
        // The essence texts ride the pinned XileHUD commit; the bases follow
        // the live repoe export by age, like the mods.
        assert!(pinned_files().iter().any(|n| n == XILE_ESSENCES));
        assert!(!pinned_files().iter().any(|n| n == BASE_ITEMS));
        assert_eq!(upstream("no-such-file.json").unwrap_err(), "no upstream source for no-such-file.json");

        // A cold cache downloads all three, and each lands on disk.
        let dir = temp_dir("craft-cold");
        let fetch = |name: &str| match name {
            BASE_ITEMS => Ok(BASES_OK.to_string()),
            REPOE_MODS => Ok("{\"IncreasedLife1\":{}}".to_string()),
            XILE_ESSENCES => Ok("{\"essences\":[{\"slug\":\"Essence_of_the_Body\"}]}".to_string()),
            other => Err(format!("unexpected {other}")),
        };
        let files = craft_files_with(&dir, &fetch).expect("every file downloads");
        assert_eq!(files.bases, BASES_OK);
        assert_eq!(std::fs::read_to_string(dir.join(BASE_ITEMS)).unwrap(), BASES_OK);
        assert!(dir.join(XILE_ESSENCES).exists() && dir.join(REPOE_MODS).exists());
        // Fresh copies are served without a request.
        let again = craft_files_with(&dir, &|n: &str| panic!("{n} fetched while fresh")).unwrap();
        assert_eq!(again.bases, BASES_OK);

        // Past its age the bases are downloaded again, and a body that is
        // JSON but not a base export never replaces the good copy.
        age(&dir.join(BASE_ITEMS), 15);
        for bad in [
            "{\"error\":{\"code\":3,\"message\":\"rate limited\"}}",
            // Rows without an item domain, a class or tags.
            r#"{"a":{"domain":"misc","name":"Gold","item_class":"Currency","tags":["currency"]}}"#,
            r#"{"a":{"domain":"item","name":"Soldier Cuirass","item_class":"","tags":["armour"]}}"#,
            r#"{"a":{"domain":"item","name":"Soldier Cuirass","item_class":"Body Armour"}}"#,
            "[1,2,3]",
            "<html>challenge</html>",
        ] {
            assert!(khaloni_poe2_core::refdata::validate_base_items(bad).is_err(), "{bad}");
            recent_failures().lock().unwrap().retain(|path, _| !path.starts_with(&dir));
            let files = craft_files_with(&dir, &|n: &str| match n {
                BASE_ITEMS => Ok(bad.to_string()),
                other => panic!("{other} fetched while fresh"),
            })
            .expect("the old copy still serves");
            assert_eq!(files.bases, BASES_OK, "{bad} replaced the cache");
            assert_eq!(std::fs::read_to_string(dir.join(BASE_ITEMS)).unwrap(), BASES_OK);
        }
        // A good download replaces it.
        recent_failures().lock().unwrap().retain(|path, _| !path.starts_with(&dir));
        let files = craft_files_with(&dir, &|n: &str| match n {
            BASE_ITEMS => Ok(BASES_NEW.to_string()),
            other => panic!("{other} fetched while fresh"),
        })
        .unwrap();
        assert_eq!(files.bases, BASES_NEW);
        assert_eq!(std::fs::read_to_string(dir.join(BASE_ITEMS)).unwrap(), BASES_NEW);
        let _ = std::fs::remove_dir_all(&dir);

        // With nothing cached and the download down, the error names the
        // file the planner waits for.
        let dir = temp_dir("craft-down");
        let err = craft_files_with(&dir, &|_: &str| Err("offline".to_string())).err().unwrap();
        assert!(err.contains(REPOE_MODS), "{err}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
