//! Dumps the default trade-search body for every item text in a directory,
//! through the same `ee2::build` the app's price check uses.
//!
//! bench_query <stats.ndjson> <items.ndjson> <trade stats.json> <trade items.json> <items dir> <out dir> [all]
//!
//! With `all`, every row EE2 shows is switched on first, the way the card's
//! checkboxes switch them, to compare against EE2's `defaultAllSelected`.
use khaloni_poe2_core::ee2::{self, data};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (paths, all) = match args.last().map(String::as_str) {
        Some("all") => (&args[..args.len() - 1], true),
        _ => (&args[..], false),
    };
    let [stats, items, trade_stats, trade_items, in_dir, out_dir] = paths else {
        panic!("usage: bench_query <stats.ndjson> <items.ndjson> <trade stats.json> <trade items.json> <items dir> <out dir> [all]");
    };
    let read = |p: &String| std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{p}: {e}"));
    let mut db = ee2::Ee2Data::from_ndjson(&read(stats), &read(items)).expect("ndjson");
    db.trade_stats = Some(data::TradeStatTexts::from_json(&read(trade_stats)).expect("trade stats"));
    db.trade_items = Some(data::trade_item_names(&read(trade_items)).expect("trade items"));
    for entry in std::fs::read_dir(in_dir).unwrap().flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("txt") {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let text = std::fs::read_to_string(&path).unwrap();
        let out = match ee2::build(&text, &db) {
            Ok(mut built) => {
                if all {
                    ee2::request::select_all(&mut built);
                }
                let ui: Vec<_> = built
                    .labels
                    .iter()
                    .zip(&built.query.filters)
                    .map(|(l, f)| serde_json::json!({"text": l.text, "tag": l.tag, "id": f.id, "min": f.value.min, "max": f.value.max, "disabled": f.disabled, "hidden": l.hidden}))
                    .collect();
                serde_json::json!({"preset": built.preset, "body": built.query.to_body(), "ui": ui, "unsearchable": built.unsearchable})
            }
            Err(e) => serde_json::json!({"error": e.to_string()}),
        };
        std::fs::write(format!("{out_dir}/{name}.json"), serde_json::to_string_pretty(&out).unwrap()).unwrap();
    }
}
