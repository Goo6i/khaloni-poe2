//! The price-check card must account for every modifier line of the item.
//! EE2 takes a stat out of its list once a figure (DPS, Armour) or a total
//! (resistances, life) has counted it; the search is right without it, but
//! the owner reading the card cannot tell a folded line from one that was
//! never read. These tests read the clipboard text on their own and hold
//! the card to it.

use khaloni_poe2_core::ee2::{self, data, request::Built, Ee2Data};
use serde_json::Value;
use std::collections::BTreeMap;
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

fn corpus() -> Vec<(String, String)> {
    let mut items: Vec<PathBuf> = std::fs::read_dir(parity_dir().join("items"))
        .expect("corpus directory")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "txt"))
        .collect();
    items.sort();
    assert!(items.len() >= 52, "the corpus shrank to {} items", items.len());
    items.iter().map(|p| (p.file_stem().unwrap().to_string_lossy().to_string(), read(p))).collect()
}

const MARKERS: [&str; 6] = [" (implicit)", " (rune)", " (enchant)", " (fractured)", " (desecrated)", " (crafted)"];

fn without_marker(line: &str) -> &str {
    MARKERS.iter().find_map(|m| line.strip_suffix(m)).unwrap_or(line)
}

/// The modifier lines of a clipboard text, read without any of the code
/// under test: every line under a `{ ... }` header, and in a section
/// without headers the lines that carry a type marker or grant a skill.
/// Reminder text (a bracketed line) is not a modifier.
fn modifier_lines(clipboard: &str) -> Vec<String> {
    let mut out = Vec::new();
    let text = clipboard.replace("\r\n", "\n");
    for section in text.split("\n--------\n").skip(1) {
        let lines: Vec<&str> = section.lines().map(str::trim_end).filter(|l| !l.is_empty()).collect();
        let headed = lines.first().is_some_and(|l| l.starts_with('{') && l.ends_with('}'));
        let mut reminder = false;
        for line in lines {
            if line.starts_with('{') && line.ends_with('}') {
                continue;
            }
            if line.starts_with('(') {
                reminder = true;
            }
            if reminder {
                reminder = !line.ends_with(')');
                continue;
            }
            let marked = MARKERS.iter().any(|m| line.ends_with(m));
            if headed || marked || line.starts_with("Grants Skill:") {
                out.push(without_marker(line).to_string());
            }
        }
    }
    out
}

fn tally<'a>(lines: impl IntoIterator<Item = &'a str>) -> BTreeMap<String, usize> {
    let mut t = BTreeMap::new();
    for l in lines {
        *t.entry(without_marker(l).to_string()).or_insert(0) += 1;
    }
    t
}

/// Every item line the card stands for, row by row.
fn lines_on_card(built: &Built) -> Vec<&str> {
    built
        .labels
        .iter()
        .flat_map(|l| &l.lines)
        .chain(built.unsearchable_lines.iter().flatten())
        .chain(built.counted.iter().flat_map(|c| &c.lines))
        .flat_map(|l| l.split('\n'))
        .collect()
}

/// Lines that are rightly on no row.
fn allowed_absent(line: &str) -> bool {
    // The skill every buckler, shield and spear grants is part of the base,
    // printed on all of them alike; EE2 does not treat it as a modifier and
    // neither does the trade site.
    line.starts_with("Grants Skill: ")
        && ["Parry", "Raise Shield", "Spear Throw"].iter().any(|skill| line.ends_with(skill))
}

#[test]
fn every_modifier_line_of_every_corpus_item_is_on_the_card() {
    let db = load_data();
    let mut failures = Vec::new();
    let mut checked = 0;
    for (name, text) in corpus() {
        let Ok(built) = ee2::build(&text, &db) else { continue };
        assert_eq!(built.unsearchable.len(), built.unsearchable_lines.len(), "{name}");
        let expected = tally(modifier_lines(&text).iter().map(String::as_str).filter(|l| !allowed_absent(l)));
        let on_card = tally(lines_on_card(&built));
        for (line, times) in &expected {
            checked += times;
            let seen = on_card.get(line).copied().unwrap_or(0);
            if seen != *times {
                failures.push(format!("{name}: {line:?} is on the item {times}x and on the card {seen}x"));
            }
        }
        // The card may show more than the markers reveal (a plain copy's
        // explicit lines), but never a line the item does not have.
        for line in on_card.keys().filter(|l| !expected.contains_key(*l)) {
            if !text.lines().any(|t| without_marker(t.trim_end()) == line) {
                failures.push(format!("{name}: the card shows {line:?}, which the item does not have"));
            }
        }
    }
    assert!(checked > 200, "only {checked} modifier lines were found in the corpus");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn counted_into<'a>(built: &'a Built, line_start: &str) -> &'a [String] {
    let c = built
        .counted
        .iter()
        .find(|c| c.text.starts_with(line_start))
        .unwrap_or_else(|| panic!("no counted row for {line_start:?} among {:?}", built.counted));
    &c.into
}

fn build_named(db: &Ee2Data, name: &str) -> Built {
    let text = read(&parity_dir().join("items").join(format!("{name}.txt")));
    ee2::build(&text, db).unwrap_or_else(|e| panic!("{name}: {e}"))
}

#[test]
fn a_mod_folded_into_a_total_says_which_total() {
    let db = load_data();

    let bow = build_named(&db, "new-bow-elemental-only");
    assert_eq!(counted_into(&bow, "Adds 96 to 151 Fire Damage"), ["Total DPS", "Elemental DPS"]);
    assert!(
        !bow.labels.iter().any(|l| l.text.contains("Fire Damage")),
        "the folded line has no searchable row of its own"
    );

    let armour = build_named(&db, "ee2-ArmourHighValueRareItem");
    assert_eq!(counted_into(&armour, "+70 to Armour"), ["Armour"]);
    // A desecrated roll counts toward the figure like any explicit one,
    // and on an item that can still be crafted it also has its own row.
    let strength = counted_into(&armour, "+32 to Strength");
    assert_eq!(strength, ["Total Strength"], "no life mod on this item, so no life total");

    let belt = build_named(&db, "new-belt-life-resists-attributes");
    assert_eq!(counted_into(&belt, "+22% to Fire Resistance"), ["Total Elemental Resistance"]);
    assert_eq!(counted_into(&belt, "+26 to Strength"), ["Total Strength", "Total Life"]);
    assert_eq!(counted_into(&belt, "+142 to maximum Life"), ["Total Life"]);
    // A belt has no armour figure, so its armour mod is an ordinary row.
    assert!(belt.labels.iter().any(|l| l.lines == ["+118(103-140) to Armour"]));
    assert!(!belt.counted.iter().any(|c| c.text.contains("to Armour")));

    assert_eq!(bow.counted[0].note(), format!("counted in {}", bow.counted[0].into.join(", ")));
}

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
fn folded_lines_never_reach_the_request() {
    let db = load_data();
    let mut with_counted = 0;
    for (name, text) in corpus() {
        let Ok(mut built) = ee2::build(&text, &db) else { continue };
        let golden: Value = serde_json::from_str(&read(&parity_dir().join("golden").join(format!("{name}.json"))))
            .unwrap_or_else(|e| panic!("golden for {name}: {e}"));
        with_counted += usize::from(!built.counted.is_empty());
        // The counted rows are no part of the query: the body is EE2's with
        // them on the card, and stays EE2's with every checkbox on.
        assert_eq!(built.labels.len(), built.query.filters.len(), "{name}: a counted row took a filter slot");
        assert_eq!(canonical(&built.query.to_body()), canonical(&golden["body"]), "{name}");
        let before = built.query.to_body();
        let counted = std::mem::take(&mut built.counted);
        assert_eq!(built.query.to_body(), before, "{name}: the body depends on the counted rows");
        built.counted = counted;
        ee2::request::select_all(&mut built);
        assert_eq!(canonical(&built.query.to_body()), canonical(&golden["body_all"]), "{name}: all rows on");
    }
    assert!(with_counted >= 10, "only {with_counted} corpus items have a folded line");
}

#[test]
fn a_simple_format_copy_is_recognised_as_unpriceable() {
    // The same kind of ring copied plainly (as a chat-linked item is) and
    // with its modifier headers. Only the second gives the search anything
    // to compare; the first must be refused, not priced as "any ring".
    let db = load_data();
    assert!(build_named(&db, "edge-plain-copy-ring").reads_no_modifier());
    for name in ["ee2-RareWithImplicit", "ours-bow", "ee2-ArmourHighValueRareItem"] {
        assert!(!build_named(&db, name).reads_no_modifier(), "{name} has readable modifiers");
    }
}

// --- every line a row: the extra rows ------------------------------------

use khaloni_poe2_core::ee2::request::{ExtraRow, NO_TRADE_STAT};
use khaloni_poe2_core::trade::StatIndex;

/// Items the owner price-checked (tablets, a waystone, uniques, a jewel,
/// a charm, runes, an enchant, an unrevealed desecrated mod), kept beside
/// the EE2 corpus: they have no golden body of EE2's, only lines.
fn allmods_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/allmods")
}

fn allmods_items() -> Vec<(String, String)> {
    let mut items: Vec<PathBuf> = std::fs::read_dir(allmods_dir())
        .expect("allmods fixtures")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "txt"))
        .collect();
    items.sort();
    assert!(items.len() >= 10, "the allmods fixtures shrank to {}", items.len());
    items.iter().map(|p| (p.file_stem().unwrap().to_string_lossy().to_string(), read(p))).collect()
}

/// The trade site's catalog (`/api/trade2/data/stats`, fetched 2026-09-25)
/// cut down to the entries the corpus and the allmods items resolve to.
/// It has the crafted group EE2's pinned snapshot lacks.
fn live_catalog() -> StatIndex {
    StatIndex::from_json(&read(&allmods_dir().join("trade_stats.json"))).expect("catalog fixture parses")
}

fn built_resolved(db: &Ee2Data, catalog: &StatIndex, text: &str) -> Option<Built> {
    let mut built = ee2::build(text, db).ok()?;
    built.resolve_extra(catalog);
    Some(built)
}

fn named(db: &Ee2Data, catalog: &StatIndex, name: &str) -> Built {
    let path = [parity_dir().join("items").join(format!("{name}.txt")), allmods_dir().join(format!("{name}.txt"))]
        .into_iter()
        .find(|p| p.exists())
        .unwrap_or_else(|| panic!("no item {name}"));
    built_resolved(db, catalog, &read(&path)).unwrap_or_else(|| panic!("{name} does not build"))
}

fn extra_ids(x: &ExtraRow) -> Vec<String> {
    x.filter().map(|f| std::iter::once(f.id).chain(f.alt_ids).collect()).unwrap_or_default()
}

fn extra_starting<'a>(built: &'a Built, text: &str) -> &'a ExtraRow {
    built
        .extra
        .iter()
        .find(|x| x.text.starts_with(text))
        .unwrap_or_else(|| panic!("no extra row {text:?} among {:?}", built.extra.iter().map(|x| &x.text).collect::<Vec<_>>()))
}

/// Every modifier line of every item, in the corpus and in the owner's own
/// checks, is on one of EE2's rows or on an extra row, as many times as the
/// item has it. An extra row either searches ids the site's catalog lists,
/// or says why there are none. The only such line is the invented one. (A
/// waystone line in a wording the game no longer uses is found through the
/// older catalog text EE2 pinned, under the id the site still lists.)
#[test]
fn every_modifier_line_is_an_ee2_row_or_an_extra_row() {
    let db = load_data();
    let catalog = live_catalog();
    let mut failures = Vec::new();
    let mut without_id = Vec::new();
    let (mut checked, mut extras) = (0, 0);
    let mut items = corpus();
    items.extend(allmods_items());
    for (name, text) in items {
        let Some(built) = built_resolved(&db, &catalog, &text) else {
            assert!(!name.starts_with("tablet") && parity_dir().join("items").join(format!("{name}.txt")).exists());
            continue;
        };
        let expected = tally(modifier_lines(&text).iter().map(String::as_str).filter(|l| !allowed_absent(l)));
        let on_rows = tally(
            built
                .labels
                .iter()
                .flat_map(|l| &l.lines)
                .chain(built.extra.iter().flat_map(|x| &x.lines))
                .flat_map(|l| l.split('\n')),
        );
        for (line, times) in &expected {
            checked += times;
            let seen = on_rows.get(line).copied().unwrap_or(0);
            if seen != *times {
                failures.push(format!("{name}: {line:?} is on the item {times}x and on the rows {seen}x"));
            }
        }
        for line in on_rows.keys().filter(|l| !expected.contains_key(*l)) {
            if !text.lines().any(|t| without_marker(t.trim_end()) == line) {
                failures.push(format!("{name}: a row shows {line:?}, which the item does not have"));
            }
        }
        // The extra rows are exactly the lines EE2 counted or cannot search.
        let folded = built.counted.iter().map(|c| c.lines.len()).sum::<usize>()
            + built.unsearchable_lines.iter().map(Vec::len).sum::<usize>();
        let extra_lines: usize = built.extra.iter().map(|x| x.lines.len()).sum();
        if folded != extra_lines {
            failures.push(format!("{name}: {folded} counted or unsearchable lines, {extra_lines} on extra rows"));
        }
        for x in &built.extra {
            extras += 1;
            let ids = extra_ids(x);
            if ids.is_empty() {
                assert!(x.note().is_some_and(|n| n.ends_with(NO_TRADE_STAT)), "{name}: {x:?}");
                without_id.push(format!("{name}: {}", x.text));
            }
            for id in ids {
                let bare = id.split('|').next().unwrap();
                if catalog.entry_by_id(bare).is_none() {
                    failures.push(format!("{name}: {:?} searches {id}, which the catalog does not list", x.text));
                }
                if !bare.starts_with(&format!("{}.", x.group)) {
                    failures.push(format!("{name}: {:?} searches {id} outside its group {}", x.text, x.group));
                }
            }
        }
    }
    assert!(checked > 250, "only {checked} modifier lines were found");
    assert!(extras > 150, "only {extras} extra rows");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert_eq!(without_id, ["edge-unknown-mod-gloves: 12% increased Wombat Summoning Speed"]);
}

/// A crafted line searches the crafted id and a desecrated line the
/// desecrated one, the ids of their own type, never the explicit ones.
#[test]
fn crafted_and_desecrated_lines_search_their_own_type() {
    let db = load_data();
    let catalog = live_catalog();
    let bow = named(&db, &catalog, "ee2-BowThreeAugments");
    let crafted = extra_starting(&bow, "+3.45% to Critical Hit Chance");
    assert_eq!((crafted.tag, extra_ids(crafted)), ("crafted", vec!["crafted.stat_518292764".to_string()]));
    assert_eq!(crafted.value.min, Some(3.1), "ten percent under the roll, as EE2 bounds its rows");
    let desecrated = extra_starting(&bow, "Adds 21 to 39 Physical Damage");
    assert_eq!((desecrated.tag, extra_ids(desecrated)), ("desecrated", vec!["desecrated.stat_1940865751".to_string()]));

    let armour = named(&db, &catalog, "ee2-ArmourHighValueRareItem");
    let desecrated = extra_starting(&armour, "+256 to Armour");
    assert_eq!(extra_ids(desecrated), ["desecrated.stat_3484657501", "desecrated.stat_809229260"]);
    assert_eq!(desecrated.into, ["Armour"], "the note still says the Armour figure counts it");
}

/// An unrevealed desecrated mod has no stat yet; the site counts them by
/// kind, and the row asks for at least as many as the item has.
#[test]
fn an_unrevealed_mod_searches_the_count_of_its_kind() {
    let db = load_data();
    let catalog = live_catalog();
    for (name, text, id) in [
        ("amulet-rare-unrevealed-desecrated", "Desecrated Suffix", "pseudo.pseudo_number_of_unrevealed_suffix_mods"),
        ("edge-body-unrevealed-desecrated", "Desecrated Prefix", "pseudo.pseudo_number_of_unrevealed_prefix_mods"),
    ] {
        let built = named(&db, &catalog, name);
        let row = extra_starting(&built, text);
        assert_eq!(extra_ids(row), [id], "{name}");
        assert_eq!(row.value.min, Some(1.0), "{name}");
    }
}

/// Tablets, uniques and waystones: the implicit a tablet's uses total
/// counts, a unique's constant line searched from its own number, a line
/// only the site's text knows, and a waystone whose lines are all EE2's.
#[test]
fn tablets_uniques_and_waystones_have_every_line_searchable() {
    let db = load_data();
    let catalog = live_catalog();
    let tablet = named(&db, &catalog, "tablet-rare-abyss");
    let implicit = extra_starting(&tablet, "Adds Abysses to a Map");
    assert_eq!(extra_ids(implicit), ["implicit.stat_2369421690"]);
    assert_eq!((implicit.value.min, implicit.into.as_slice()), (Some(10.0), &["uses remaining (Tablets)".to_string()][..]));

    // EE2 searches a unique's constant roll by the unique's name and sends
    // no bound; the row the user adds is bounded by the number it shows.
    let unique = named(&db, &catalog, "tablet-unique-irradiated");
    let implicit = extra_starting(&unique, "Adds Irradiated to a Map");
    assert_eq!((extra_ids(implicit), implicit.value.min), (vec!["implicit.stat_4041853756".to_string()], Some(1.0)));

    let ring = named(&db, &catalog, "ring-unique-right-ring-slot");
    let chain = extra_starting(&ring, "Right ring slot: Projectiles from Spells Chain");
    assert_eq!((extra_ids(chain), chain.value.min), (vec!["explicit.stat_1555918911".to_string()], Some(1.0)));

    let waystone = named(&db, &catalog, "waystone-rare-corrupted");
    assert!(waystone.extra.is_empty(), "every waystone line is one of EE2's rows: {:?}", waystone.extra);
    assert!(waystone.labels.iter().filter(|l| !l.lines.is_empty()).count() >= 8);
}

/// With the extra rows on the card, EE2's body is untouched; ticking one
/// adds exactly its filter, as the builder sends such a stat: into the
/// "and" group, or as an any-of group when the stat has a local twin.
#[test]
fn ticking_an_extra_row_adds_exactly_its_filter() {
    let db = load_data();
    let catalog = live_catalog();
    let mut ticked = 0;
    for (name, text) in corpus() {
        let Some(built) = built_resolved(&db, &catalog, &text) else { continue };
        let golden: Value = serde_json::from_str(&read(&parity_dir().join("golden").join(format!("{name}.json"))))
            .unwrap_or_else(|e| panic!("golden for {name}: {e}"));
        assert_eq!(canonical(&built.query.to_body()), canonical(&golden["body"]), "{name}: unticked is EE2's");
        for x in &built.extra {
            let Some(f) = x.filter() else { continue };
            assert!(f.disabled, "{name}: {:?} starts unticked", x.text);
            let on = khaloni_poe2_core::trade::StatFilter { disabled: false, ..f.clone() };
            let mut q = built.query.clone();
            q.filters.push(on.clone());
            let member = |id: &str| serde_json::json!({"id": id, "value": on.value.to_json(), "disabled": false});
            let mut expected = golden["body"].clone();
            let groups = expected["query"]["stats"].as_array_mut().expect("stat groups");
            if on.alt_ids.is_empty() {
                let and = groups.iter_mut().find(|g| g["type"] == "and").expect("an and group");
                and["filters"].as_array_mut().unwrap().push(member(&on.id));
            } else {
                let members: Vec<Value> = std::iter::once(&on.id).chain(&on.alt_ids).map(|id| member(id)).collect();
                groups.push(serde_json::json!({"type": "count", "value": {"min": 1}, "disabled": false, "filters": members}));
            }
            assert_eq!(canonical(&q.to_body()), canonical(&expected), "{name}: {:?}", x.text);
            ticked += 1;
        }
    }
    assert!(ticked > 150, "only {ticked} extra rows were ticked");
}

/// Where the pinned data has no id under a line's type, the site's catalog
/// decides: the stat's key under the line's own group when the catalog
/// lists it (the site keys a stat alike in every group), else the line's
/// text within that group. An id the catalog does not list is dropped, and
/// a line it lists nowhere under its type keeps no id and says so.
#[test]
fn a_type_the_pinned_data_lacks_is_taken_from_the_catalog_or_not_at_all() {
    let catalog = live_catalog();
    let row = |ids: &[&str], stat_keys: &[&str], lookup: &[&str]| ExtraRow {
        text: "+3.45% to Critical Hit Chance".into(),
        tag: "crafted",
        rolled: Some(3.45),
        lines: vec!["+3.45(3.11-3.8)% to Critical Hit Chance".into()],
        into: Vec::new(),
        group: "crafted",
        ids: ids.iter().map(|s| s.to_string()).collect(),
        option: None,
        value: khaloni_poe2_core::trade::FilterValue { min: Some(3.1), max: None },
        lookup: lookup.iter().map(|s| s.to_string()).collect(),
        stat_keys: stat_keys.iter().map(|s| s.to_string()).collect(),
    };
    let resolved = |mut r: ExtraRow| {
        r.resolve(&catalog);
        r
    };
    // By the stat's key: explicit.stat_518292764 in the data, crafted in the catalog.
    let by_key = resolved(row(&[], &["stat_518292764"], &[]));
    assert_eq!(by_key.ids, ["crafted.stat_518292764"]);
    // An id the catalog does not list is not searched; the text finds the right one.
    let by_text = resolved(row(&["crafted.stat_1"], &["stat_2"], &["#% to Critical Hit Chance"]));
    assert_eq!(by_text.ids, ["crafted.stat_518292764"]);
    // The explicit group lists the text too; a crafted line does not take it.
    let explicit_only = resolved(ExtraRow { group: "crafted", ..row(&[], &[], &["# to maximum Life"]) });
    assert!(explicit_only.ids.is_empty(), "{:?}", explicit_only.ids);
    assert_eq!(explicit_only.note().as_deref(), Some(NO_TRADE_STAT));
    assert!(explicit_only.filter().is_none());
}
