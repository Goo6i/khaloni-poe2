//! The observed craft model: each fetched listing's random modifiers join
//! their mod-database entries by affix name, affix kind, required level and
//! trade-site tier; the counts are kept per item class, per league and
//! bounded; a class yields a model only once enough listings back it.

use khaloni_poe2_core::craft::data::CraftData;
use khaloni_poe2_core::craft::model::Model;
use khaloni_poe2_core::craft::observed::{
    listing_entries, Listing, NotRead, Observed, MAX_LISTINGS, MIN_LISTINGS,
};
use khaloni_poe2_core::craft::types::{AffixKind, EntryId};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
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

/// The real fetch response: body armours, three copies of one listing, a
/// normal item and a listing that vanished.
fn fetched() -> Vec<Value> {
    let body: Value = serde_json::from_str(&read(&fixtures().join("trade_fetch_full.json"))).unwrap();
    body["result"].as_array().unwrap().clone()
}

fn prefix(id: &str) -> (EntryId, AffixKind) {
    (id.to_string(), AffixKind::Prefix)
}

fn suffix(id: &str) -> (EntryId, AffixKind) {
    (id.to_string(), AffixKind::Suffix)
}

fn listing(id: &str, class: &str, entries: &[(EntryId, AffixKind)]) -> Listing {
    Listing { id: id.to_string(), class: class.to_string(), entries: entries.to_vec() }
}

fn store_of(observed: &mut Observed, fetched: &[Value]) -> (usize, usize, usize) {
    let (mut recorded, mut repeated, mut unjoined) = (0, 0, 0);
    for entry in fetched {
        let Ok(read) = listing_entries(entry, craft()) else { continue };
        unjoined += read.unjoined.len();
        if observed.record(read.listing) {
            recorded += 1;
        } else {
            repeated += 1;
        }
    }
    (recorded, repeated, unjoined)
}

#[test]
fn a_listing_card_records_its_entries_per_class() {
    let fetched = fetched();

    // The first listing: two prefixes and two suffixes rolled at random.
    // "Octopus'" is one hybrid modifier behind two card lines and counts
    // once; "of the Troll" names two entries at level 35 and only the
    // life regeneration one rolls on body armour; the essence's cold
    // resistance and the bone's armour were chosen, not rolled.
    let first = listing_entries(&fetched[0], craft()).expect("a rare listing reads");
    assert_eq!(first.listing.id, "7e41d18eb53e91a2a3434bbf4400515928d81445fbc32ce9abb59d01bc32cc84");
    assert_eq!(first.listing.class, "Body Armour");
    assert_eq!(
        first.listing.entries,
        vec![
            prefix("LocalIncreasedPhysicalDamageReductionRatingPercent8_"),
            prefix("LocalIncreasedArmourAndLife5"),
            suffix("LifeRegeneration6"),
            suffix("ReducedPoisonDuration5"),
        ]
    );
    assert!(first.unjoined.is_empty(), "{:?}", first.unjoined);
    assert_eq!(first.chosen, 2, "one crafted and one desecrated line");

    // A fractured modifier was rolled before it was fixed in place, so it
    // is recorded like any random one.
    let fractured = fetched.iter().find(|e| e["id"].as_str().is_some_and(|id| id.starts_with("4d6e6f9c"))).unwrap();
    let read = listing_entries(fractured, craft()).unwrap();
    assert!(read.listing.entries.contains(&suffix("ReducedLocalAttributeRequirements4")));

    // The vanished listing and the normal item are nothing to record.
    assert!(matches!(listing_entries(&Value::Null, craft()), Err(NotRead::Gone)));
    let normal = fetched.iter().find(|e| e["item"]["rarity"] == "Normal").unwrap();
    assert!(matches!(listing_entries(normal, craft()), Err(NotRead::NotRolled(_))));

    let mut observed = Observed::new("Forbidden Rites");
    let (recorded, repeated, unjoined) = store_of(&mut observed, &fetched);
    assert_eq!((recorded, repeated, unjoined), (14, 2, 0), "fourteen distinct rare listings, one of them sent three times");
    assert_eq!(observed.listings("Body Armour"), 14);
    assert_eq!(observed.listings("Boots"), 0);

    let mut fourteen = Observed::with_limits("Forbidden Rites", 14, MAX_LISTINGS);
    store_of(&mut fourteen, &fetched);
    let Model::Observed { class, counts, totals, listings } =
        fourteen.model("Body Armour").expect("fourteen listings meet a minimum of fourteen")
    else {
        panic!("an observed model")
    };
    assert_eq!(class, "Body Armour");
    assert_eq!(listings, 14);
    assert_eq!(totals.get(&AffixKind::Prefix), Some(&30));
    assert_eq!(totals.get(&AffixKind::Suffix), Some(&33));
    assert_eq!(counts.get("LocalIncreasedPhysicalDamageReductionRatingPercent8_"), Some(&6));
    assert_eq!(counts.get("LocalIncreasedPhysicalDamageReductionRating11"), Some(&4));
    assert_eq!(counts.get("LifeRegeneration6"), Some(&1));
    assert_eq!(counts.get("ColdResist6"), Some(&1), "rolled once on a Warlord Cuirass; the essence copies do not count");
    assert_eq!(counts.values().sum::<u32>(), 63);
}

#[test]
fn under_two_hundred_listings_there_is_no_observed_model() {
    assert_eq!(MIN_LISTINGS, 200);
    let mut observed = Observed::new("Forbidden Rites");
    for i in 0..199 {
        assert!(observed.record(listing(&format!("b{i}"), "Boots", &[prefix("MovementVelocity3")])));
    }
    // Listings of another class do not lift this one over the line.
    assert!(observed.record(listing("g0", "Gloves", &[prefix("IncreasedLife5")])));
    assert_eq!(observed.listings("Boots"), 199);
    assert_eq!(observed.model("Boots"), None);

    assert!(observed.record(listing("b199", "Boots", &[suffix("FireResist4")])));
    let model = observed.model("Boots").expect("two hundred listings back a model");
    assert_eq!(model.label(), "observed on 200 listings of Boots");
    assert_eq!(observed.model("Gloves"), None);

    // The minimum is the user's to change.
    let mut small = Observed::with_limits("Forbidden Rites", 2, MAX_LISTINGS);
    small.record(listing("a", "Rings", &[]));
    assert_eq!(small.model("Rings"), None);
    small.record(listing("b", "Rings", &[]));
    assert!(small.model("Rings").is_some());
}

#[test]
fn the_observed_probability_is_the_entrys_share_of_its_kind() {
    let mut observed = Observed::with_limits("Forbidden Rites", 1, MAX_LISTINGS);
    observed.record(listing("1", "Boots", &[prefix("Life7"), prefix("Speed3"), suffix("Fire4")]));
    observed.record(listing("2", "Boots", &[prefix("Life7"), suffix("Fire4"), suffix("Cold4")]));
    observed.record(listing("3", "Boots", &[prefix("Life6"), suffix("Cold4")]));
    // A listing of another class adds nothing to Boots' shares.
    observed.record(listing("4", "Gloves", &[prefix("Life7"), suffix("Fire4")]));

    let model = observed.model("Boots").unwrap();
    // Prefixes seen on Boots: Life7 twice, Speed3 once, Life6 once.
    assert_eq!(model.share("Life7", AffixKind::Prefix), Some(2.0 / 4.0));
    assert_eq!(model.share("Speed3", AffixKind::Prefix), Some(1.0 / 4.0));
    assert_eq!(model.share("Life6", AffixKind::Prefix), Some(1.0 / 4.0));
    // Suffixes: Fire4 twice, Cold4 twice.
    assert_eq!(model.share("Fire4", AffixKind::Suffix), Some(2.0 / 4.0));
    assert_eq!(model.share("Cold4", AffixKind::Suffix), Some(2.0 / 4.0));
    // An entry never seen has a share of nothing, not of an even split.
    assert_eq!(model.share("Life5", AffixKind::Prefix), Some(0.0));
    let Model::Observed { listings, .. } = &model else { panic!("an observed model") };
    assert_eq!(*listings, 3);

    let gloves = observed.model("Gloves").unwrap();
    assert_eq!(gloves.share("Life7", AffixKind::Prefix), Some(1.0));
}

#[test]
fn a_league_change_starts_a_new_store() {
    let mut observed = Observed::with_limits("Forbidden Rites", 1, MAX_LISTINGS);
    observed.record(listing("1", "Boots", &[prefix("Life7")]));
    assert!(!observed.switch_league("Forbidden Rites"), "the same league keeps the store");
    assert_eq!(observed.listings("Boots"), 1);

    assert!(observed.switch_league("Next League"));
    assert_eq!(observed.league(), "Next League");
    assert_eq!(observed.listings("Boots"), 0);
    assert_eq!(observed.model("Boots"), None);
    assert!(observed.is_empty());
    // A listing seen last league is new to this one.
    assert!(observed.record(listing("1", "Boots", &[prefix("Life7")])));

    // Every line names its league, and a line from another league is not
    // read into this one's store.
    let mut old = Observed::new("Forbidden Rites");
    old.record(listing("a", "Boots", &[prefix("Life7"), suffix("Fire4")]));
    let mut text = old.jsonl();
    text.push_str(&observed.jsonl());
    let mut loaded = Observed::new("Next League");
    let report = loaded.load_jsonl(&text);
    assert_eq!((report.kept, report.other_league), (1, 1));
    assert!(loaded.contains("1"));
    assert!(!loaded.contains("a"));
}

#[test]
fn the_store_is_capped_oldest_first() {
    let mut observed = Observed::with_limits("Forbidden Rites", 1, 3);
    observed.record(listing("1", "Boots", &[prefix("Life7"), suffix("Fire4")]));
    observed.record(listing("2", "Boots", &[prefix("Life7")]));
    observed.record(listing("3", "Gloves", &[prefix("Life6")]));
    observed.record(listing("4", "Boots", &[prefix("Speed3")]));

    assert_eq!(observed.len(), 3);
    assert!(!observed.contains("1"), "the oldest listing went first");
    assert!(observed.contains("2") && observed.contains("3") && observed.contains("4"));
    assert_eq!(observed.listings("Boots"), 2);

    // The dropped listing's entries leave the counts with it.
    let model = observed.model("Boots").unwrap();
    assert_eq!(model.share("Life7", AffixKind::Prefix), Some(0.5));
    assert_eq!(model.share("Speed3", AffixKind::Prefix), Some(0.5));
    assert_eq!(model.share("Fire4", AffixKind::Suffix), None, "no suffix left on Boots");
    let Model::Observed { counts, totals, .. } = &model else { panic!("an observed model") };
    assert!(!counts.contains_key("Fire4"));
    assert!(!totals.contains_key(&AffixKind::Suffix));

    // Reading the lines back applies the same cap in the same order.
    let mut text = String::new();
    for i in 0..5 {
        text.push_str(&Observed::line("Forbidden Rites", &listing(&i.to_string(), "Boots", &[prefix("Life7")])));
        text.push('\n');
    }
    let mut loaded = Observed::with_limits("Forbidden Rites", 1, 3);
    assert_eq!(loaded.load_jsonl(&text).kept, 5);
    assert_eq!(loaded.len(), 3);
    assert!(!loaded.contains("0") && !loaded.contains("1") && loaded.contains("4"));
    assert_eq!(loaded.jsonl().lines().count(), 3);
}

#[test]
fn the_lines_read_back_to_the_same_store_and_skip_what_is_broken() {
    let mut observed = Observed::new("Forbidden Rites");
    store_of(&mut observed, &fetched());
    let text = observed.jsonl();
    assert_eq!(text.lines().count(), 14);

    let mut again = Observed::new("Forbidden Rites");
    let report = again.load_jsonl(&text);
    assert_eq!((report.kept, report.unreadable, report.repeated), (14, 0, 0));
    assert_eq!(again.jsonl(), text);

    // A write cut short leaves half a line at the end; the lines before it
    // still count and a repeated line counts once.
    let first = text.lines().next().unwrap();
    let broken = format!("{text}{first}\n{}", &first[..first.len() / 2]);
    let mut cut = Observed::new("Forbidden Rites");
    let report = cut.load_jsonl(&broken);
    assert_eq!((report.kept, report.unreadable, report.repeated), (14, 1, 1));
    assert_eq!(cut.listings("Body Armour"), 14);
}
