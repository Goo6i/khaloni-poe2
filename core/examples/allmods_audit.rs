//! Accounts for every modifier line of every item text in a directory: on
//! one of EE2's rows, on an extra row with a trade id the catalog lists, or
//! on an extra row that says why it has none. The same `ee2::build` and
//! `Built::resolve_extra` the price check runs.
//!
//! allmods_audit <stats.ndjson> <items.ndjson> <trade stats.json> <trade items.json> <items dir>
//!
//! Run it on the app's cache to audit the items the owner checked:
//! `C=~/.cache/khaloni-poe2; allmods_audit $C/ee2_stats.ndjson $C/ee2_items.ndjson
//! $C/trade_stats.json $C/trade_items.json $C/checked-items`
use khaloni_poe2_core::ee2::{self, data};
use khaloni_poe2_core::trade::StatIndex;
use std::collections::BTreeMap;

const MARKERS: [&str; 6] = [" (implicit)", " (rune)", " (enchant)", " (fractured)", " (desecrated)", " (crafted)"];

fn without_marker(line: &str) -> &str {
    MARKERS.iter().find_map(|m| line.strip_suffix(m)).unwrap_or(line)
}

/// The modifier lines of a clipboard text, read without the parser: every
/// line under a `{ ... }` header, and in a section without headers the
/// lines that carry a type marker or grant a skill. Reminder text is not a
/// modifier, and neither is the skill every shield, buckler or spear grants.
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
            let builtin = ["Grants Skill: Parry", "Grants Skill: Raise Shield", "Grants Skill: Spear Throw"]
                .iter()
                .any(|s| line.starts_with(s));
            let marked = MARKERS.iter().any(|m| line.ends_with(m));
            if (headed || marked || line.starts_with("Grants Skill:")) && !builtin {
                out.push(without_marker(line).to_string());
            }
        }
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [stats, items, trade_stats, trade_items, in_dir] = &args[..] else {
        panic!("usage: allmods_audit <stats.ndjson> <items.ndjson> <trade stats.json> <trade items.json> <items dir>");
    };
    let read = |p: &String| std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{p}: {e}"));
    let mut db = ee2::Ee2Data::from_ndjson(&read(stats), &read(items)).expect("ndjson");
    let catalog_json = read(trade_stats);
    db.trade_stats = Some(data::TradeStatTexts::from_json(&catalog_json).expect("trade stats"));
    db.trade_items = Some(data::trade_item_names(&read(trade_items)).expect("trade items"));
    let catalog = StatIndex::from_json(&catalog_json).expect("trade stats index");

    // Per item class: items, lines on EE2 rows, extra rows with ids, extra
    // rows with a reason, lines on no row.
    let mut per_kind: BTreeMap<String, [usize; 5]> = BTreeMap::new();
    let mut paths: Vec<_> = std::fs::read_dir(in_dir).expect("items dir").flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths.iter().filter(|p| p.extension().is_some_and(|e| e == "txt")) {
        let text = std::fs::read_to_string(path).expect("item text");
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let class = text.lines().next().unwrap_or("").trim_start_matches("Item Class: ").to_string();
        let mut built = match ee2::build(&text, &db) {
            Ok(b) => b,
            Err(e) => {
                println!("{name} [{class}] not built: {e}");
                continue;
            }
        };
        built.resolve_extra(&catalog);
        let kind = per_kind.entry(class.clone()).or_default();
        kind[0] += 1;
        let mut on_rows: Vec<String> = Vec::new();
        for l in &built.labels {
            kind[1] += l.lines.len();
            on_rows.extend(l.lines.iter().flat_map(|l| l.split('\n')).map(|l| without_marker(l).to_string()));
        }
        for x in &built.extra {
            on_rows.extend(x.lines.iter().flat_map(|l| l.split('\n')).map(|l| without_marker(l).to_string()));
            match x.filter() {
                Some(f) => {
                    kind[2] += 1;
                    let ids: Vec<&str> = std::iter::once(&f.id).chain(&f.alt_ids).map(String::as_str).collect();
                    println!("{name} [{class}] EXTRA {} {:?} -> {} {:?}", x.tag, x.text, ids.join(" "), f.value);
                }
                None => {
                    kind[3] += 1;
                    println!("{name} [{class}] NO ID {} {:?}: {}", x.tag, x.text, x.note().unwrap_or_default());
                }
            }
        }
        for line in modifier_lines(&text) {
            match on_rows.iter().position(|l| *l == line) {
                Some(i) => {
                    on_rows.remove(i);
                }
                None => {
                    kind[4] += 1;
                    println!("{name} [{class}] ON NO ROW {line:?}");
                }
            }
        }
    }
    let mut total = [0usize; 5];
    for (k, v) in &per_kind {
        println!("KIND {k}: items {}, EE2 rows {}, extra with id {}, extra with reason {}, on no row {}", v[0], v[1], v[2], v[3], v[4]);
        for (t, n) in total.iter_mut().zip(v) {
            *t += n;
        }
    }
    println!(
        "TOTAL: items {}, EE2 rows {}, extra with id {}, extra with reason {}, on no row {}",
        total[0], total[1], total[2], total[3], total[4]
    );
}
