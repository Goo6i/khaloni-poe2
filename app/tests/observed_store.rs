//! The observed store on disk: the listings a price check fetched land in
//! the league's file, a listing fetched again counts once, a league change
//! reads that league's file, the file stays bounded, and a write cut short
//! loses only the line it was writing.

use khaloni_poe2::observed_store::{ObservedStore, BIAS_NOTE};
use khaloni_poe2_core::craft::data::CraftData;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../core/tests/fixtures")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn craft() -> &'static CraftData {
    static DATA: OnceLock<CraftData> = OnceLock::new();
    DATA.get_or_init(|| {
        let dir = fixtures();
        CraftData::load(
            &read(&dir.join("craft/mods_slice.json")),
            &read(&dir.join("craft_base_items_sample.json")),
            &read(&dir.join("craft/essences_slice.json")),
        )
        .expect("the craft fixtures load")
    })
}

/// The real fetch response as the price check holds it: every entry in
/// order, `None` where the API sent null.
fn fetched() -> Vec<Option<Value>> {
    let body: Value = serde_json::from_str(&read(&fixtures().join("trade_fetch_full.json"))).unwrap();
    body["result"].as_array().unwrap().iter().map(|v| (!v.is_null()).then(|| v.clone())).collect()
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("khalonipoe2-observed-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

const LEAGUE: &str = "Forbidden Rites";

#[test]
fn a_price_checks_listings_persist_per_league() {
    let dir = temp_dir("persist");
    let mut store = ObservedStore::open(&dir, LEAGUE).unwrap();
    let got = store.record_fetch(&fetched(), craft()).unwrap();
    assert_eq!(got.recorded, 14);
    assert_eq!(got.repeated, 2);
    assert_eq!(got.not_read, 2, "the vanished listing and the normal item");
    assert!(got.unjoined.is_empty(), "{:?}", got.unjoined);
    assert_eq!(got.chosen, 17);
    assert_eq!(store.path(), dir.join("Forbidden Rites.jsonl"));
    assert_eq!(read(&store.path()).lines().count(), 14);

    let reopened = ObservedStore::open(&dir, LEAGUE).unwrap();
    assert_eq!(reopened.observed().listings("Body Armour"), 14);
    assert_eq!(reopened.observed().jsonl(), store.observed().jsonl());
    // Fourteen listings are far below the minimum sample.
    assert_eq!(reopened.model("Body Armour"), None);
}

#[test]
fn a_listing_fetched_again_counts_once() {
    let dir = temp_dir("dedup");
    let mut store = ObservedStore::open(&dir, LEAGUE).unwrap();
    store.record_fetch(&fetched(), craft()).unwrap();
    let again = store.record_fetch(&fetched(), craft()).unwrap();
    assert_eq!((again.recorded, again.repeated), (0, 16));
    assert_eq!(store.observed().listings("Body Armour"), 14);
    assert_eq!(read(&store.path()).lines().count(), 14, "nothing new was written");

    // A store opened later knows the listings too.
    let mut later = ObservedStore::open(&dir, LEAGUE).unwrap();
    assert_eq!(later.record_fetch(&fetched(), craft()).unwrap().recorded, 0);
}

#[test]
fn a_league_change_reads_that_leagues_file() {
    let dir = temp_dir("league");
    let mut store = ObservedStore::open(&dir, LEAGUE).unwrap();
    store.record_fetch(&fetched(), craft()).unwrap();

    store.switch_league("Next League").unwrap();
    assert_eq!(store.observed().league(), "Next League");
    assert!(store.observed().is_empty());
    assert_eq!(store.path(), dir.join("Next League.jsonl"));
    // The same listings are new to the new league.
    assert_eq!(store.record_fetch(&fetched(), craft()).unwrap().recorded, 14);

    // The previous league's file stays as it was, and returning reads it.
    assert_eq!(read(&dir.join("Forbidden Rites.jsonl")).lines().count(), 14);
    store.switch_league(LEAGUE).unwrap();
    assert_eq!(store.observed().listings("Body Armour"), 14);
}

#[test]
fn the_file_stays_capped_oldest_first() {
    // The fetch's rare listings, each once, in order.
    let mut unique: Vec<Option<Value>> = Vec::new();
    for entry in fetched().into_iter().flatten().filter(|e| e["item"]["rarity"] == "Rare") {
        if !unique.iter().flatten().any(|u| u["id"] == entry["id"]) {
            unique.push(Some(entry));
        }
    }
    let ids: Vec<&str> = unique.iter().flatten().map(|e| e["id"].as_str().unwrap()).collect();
    assert_eq!(ids.len(), 14);

    let dir = temp_dir("cap");
    let mut store = ObservedStore::with_limits(&dir, LEAGUE, 1, 4).unwrap();
    assert_eq!(store.record_fetch(&unique, craft()).unwrap().recorded, 14);
    assert_eq!(store.observed().len(), 4);
    // Appends run a quarter past the cap at most before the file is
    // rewritten to the listings kept.
    let lines = read(&store.path()).lines().count();
    assert!((4..=5).contains(&lines), "{lines} lines");

    // The newest four survive, in the store and on disk.
    let reopened = ObservedStore::with_limits(&dir, LEAGUE, 1, 4).unwrap();
    for id in &ids[..10] {
        assert!(!reopened.observed().contains(id), "{id} is among the oldest");
    }
    for id in &ids[10..] {
        assert!(reopened.observed().contains(id), "{id} is among the newest");
    }
    assert_eq!(read(&reopened.path()).lines().count(), 4);

    // A dropped listing fetched again is a new sighting and counts anew.
    let mut store = reopened;
    assert_eq!(store.record_fetch(&unique[..1], craft()).unwrap().recorded, 1);
    assert!(store.observed().contains(ids[0]) && !store.observed().contains(ids[10]));
}

#[test]
fn a_write_cut_short_loses_only_its_own_line() {
    let dir = temp_dir("torn");
    let mut store = ObservedStore::open(&dir, LEAGUE).unwrap();
    store.record_fetch(&fetched(), craft()).unwrap();
    let text = read(&store.path());
    let last = text.lines().last().unwrap();
    let torn = &text[..text.len() - last.len() / 2 - 1];
    std::fs::write(store.path(), torn).unwrap();

    let mut reopened = ObservedStore::open(&dir, LEAGUE).unwrap();
    assert_eq!(reopened.observed().len(), 13);
    // The half line is gone from the file, so the next append starts on a
    // line of its own and every line reads back.
    let got = reopened.record_fetch(&fetched(), craft()).unwrap();
    assert_eq!(got.recorded, 1);
    let healed = read(&reopened.path());
    assert!(healed.ends_with('\n'));
    assert_eq!(healed.lines().count(), 14);
    assert_eq!(ObservedStore::open(&dir, LEAGUE).unwrap().observed().len(), 14);
}

#[test]
fn the_panel_says_the_sample_leans_toward_listed_items() {
    assert_eq!(BIAS_NOTE, "biased toward what sellers list");
}
