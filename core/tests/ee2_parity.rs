//! Holds `ee2::build` + `Query::to_body` to the request Exiled Exchange 2
//! builds for the same clipboard text, over the corpus in
//! `tools/ee2-parity`. The golden bodies were produced by EE2's own parser
//! and request builder (see that directory's README); the data files are
//! the ones that run read, so the test needs no network and no cache.

use khaloni_poe2_core::ee2::{self, data, Ee2Data};
use serde_json::Value;
use std::path::{Path, PathBuf};

fn parity_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../tools/ee2-parity")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn load_data() -> Ee2Data {
    let dir = parity_dir().join("data");
    let mut db = Ee2Data::from_ndjson(&read(&dir.join("stats.ndjson")), &read(&dir.join("items.ndjson")))
        .expect("pinned ndjson parses");
    db.trade_stats = Some(data::TradeStatTexts::from_json(&read(&dir.join("trade-stats.json"))).expect("trade stats"));
    db.trade_items = Some(data::trade_item_names(&read(&dir.join("trade-items.json"))).expect("trade items"));
    db
}

/// A body as the trade API reads it: the order of stat groups, and of the
/// filters inside one, carries no meaning. Everything else must be equal.
fn canonical(body: &Value) -> Value {
    let mut body = body.clone();
    if let Some(groups) = body["query"]["stats"].as_array_mut() {
        for g in groups.iter_mut() {
            if let Some(filters) = g["filters"].as_array_mut() {
                filters.sort_by_key(|f| f.to_string());
            }
        }
        groups.sort_by_key(|g| g.to_string());
    }
    body
}

#[test]
fn every_corpus_item_builds_the_request_ee2_builds() {
    let db = load_data();
    let dir = parity_dir();
    let mut items: Vec<PathBuf> = std::fs::read_dir(dir.join("items"))
        .expect("corpus directory")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "txt"))
        .collect();
    items.sort();
    assert!(items.len() >= 52, "the corpus shrank to {} items", items.len());

    let mut failures = Vec::new();
    for path in &items {
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        let golden: Value = serde_json::from_str(&read(&dir.join("golden").join(format!("{name}.json"))))
            .unwrap_or_else(|e| panic!("golden for {name}: {e}"));
        let mut built = match ee2::build(&read(path), &db) {
            Ok(b) => b,
            Err(e) => {
                failures.push(format!("{name}: {e}"));
                continue;
            }
        };
        assert_eq!(built.labels.len(), built.query.filters.len(), "{name}: labels follow the filters");
        if golden["preset"] != built.preset {
            failures.push(format!("{name}: preset {} instead of {}", built.preset, golden["preset"]));
        }
        if canonical(&built.query.to_body()) != canonical(&golden["body"]) {
            failures.push(format!("{name}: default selection differs"));
        }
        // The same rows switched on the way the card's checkboxes do it.
        ee2::request::select_all(&mut built);
        if canonical(&built.query.to_body()) != canonical(&golden["body_all"]) {
            failures.push(format!("{name}: selection with every row on differs"));
        }
    }
    assert!(failures.is_empty(), "{} of {} items differ:\n{}", failures.len(), items.len(), failures.join("\n"));
}

#[test]
fn a_line_without_a_known_stat_is_reported_and_not_searched() {
    let db = load_data();
    let text = read(&parity_dir().join("items/edge-unknown-mod-gloves.txt"));
    let built = ee2::build(&text, &db).expect("builds");
    // Worded like every other row of the card, with the item's own text
    // kept beside it.
    assert_eq!(built.unsearchable, ["12% increased Wombat Summoning Speed"]);
    assert_eq!(built.unsearchable_lines, [["12(10-15)% increased Wombat Summoning Speed"]]);
    assert!(!built.query.to_body().to_string().contains("Wombat"));
}

#[test]
fn an_item_the_data_does_not_know_is_an_error_not_a_looser_search() {
    let db = load_data();
    let text = "Item Class: Rings\nRarity: Rare\nGrim Turn\nRing Of No Such Base\n--------\nItem Level: 55\n";
    assert!(matches!(ee2::build(text, &db), Err(ee2::parse::ParseError::UnknownItem(_))));
}

/// The reference panels read the same two files (`refdata`); a pin bump
/// must keep them parsing, not only the price check.
#[test]
fn the_pinned_data_still_feeds_the_reference_panels() {
    use khaloni_poe2_core::refdata;
    let dir = parity_dir().join("data");
    let affixes = refdata::parse_affixes_tiered(&read(&dir.join("stats.ndjson")), "");
    let items = refdata::parse_ref_items(&read(&dir.join("items.ndjson")));
    eprintln!("pinned data: {} affixes, {} items", affixes.len(), items.len());
    assert!(affixes.len() > 2000, "{} affixes", affixes.len());
    assert!(items.len() > 4000, "{} items", items.len());
}

/// EE2 itself fails on this item ("Cannot read properties of undefined"):
/// the line is only known through the trade catalog's text, which lists it
/// under explicit ids and not under the implicit one the item needs. There
/// is no request of EE2's to match, so the row is reported, not searched.
#[test]
fn a_stat_with_no_id_for_its_mod_type_is_reported_and_not_searched() {
    let db = load_data();
    let text = "Item Class: Belts\nRarity: Rare\nGrim Cord\nRawhide Belt\n--------\nItem Level: 60\n--------\n\
        { Implicit Modifier }\n24(20-30)% increased Flask Life Recovery rate (implicit)\n--------\n\
        { Prefix Modifier \"Robust\" (Tier: 5) — Life }\n+72(70-84) to maximum Life\n";
    let built = ee2::build(text, &db).expect("builds");
    assert_eq!(built.unsearchable.len(), 1, "{:?}", built.unsearchable);
    assert!(built.unsearchable[0].contains("Flask Life Recovery rate"));
    assert_eq!(built.labels.len(), built.query.filters.len());
}
