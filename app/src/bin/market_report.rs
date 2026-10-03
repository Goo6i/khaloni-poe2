//! Prints the market model as JSON, from the cached overviews, offline:
//! `market_report --cache <dir> --league <name> [--early-league]`.
//!
//! The overviews go through `prices::market_from_cache` and
//! `core::market::build`, the path the overlay and the settings window use,
//! with the default floors. The early-league suppression is off unless
//! asked for: the report describes the market as the data has it.

use khaloni_poe2_core::market::{self, Direction, Floors, Graded, Options};
use serde_json::{json, Value};

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}

fn direction(g: &Graded) -> &'static str {
    match g.trend.map(|t| t.direction) {
        Some(Direction::Rising) => "rising",
        Some(Direction::Falling) => "falling",
        Some(Direction::Unclear) => "none",
        None => "insufficient",
    }
}

fn key(g: &Graded) -> Value {
    json!({
        "category": g.item.category,
        "name": g.item.name,
        "base_type": g.item.base_type,
        "corrupted": g.item.corrupted,
    })
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cache = arg(&args, "--cache").ok_or_else(|| anyhow::anyhow!("--cache <dir> is required"))?;
    let league = arg(&args, "--league").ok_or_else(|| anyhow::anyhow!("--league <name> is required"))?;
    let loaded = khaloni_poe2::prices::market_from_cache(std::path::Path::new(&cache), &league);
    if loaded.source.is_empty() {
        anyhow::bail!("no cached overviews for {league} in {cache}");
    }
    let young = args.iter().any(|a| a == "--early-league") && {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let log = khaloni_poe2_core::market_history::HistoryLog::open(
            &std::path::Path::new(&cache).join("market-history"),
            &league,
        );
        market::league_is_young(now, None, log.first_record_time(), loaded.source.history_days())
    };
    let model = market::build(&loaded.source, &Options { floors: Floors::default(), young });

    let items: Vec<Value> = model
        .items
        .iter()
        .map(|g| {
            json!({
                "category": g.item.category,
                "name": g.item.name,
                "base_type": g.item.base_type,
                "corrupted": g.item.corrupted,
                "price_div": g.item.price_div,
                "volume_div": g.item.volume_div,
                "listings": g.item.listings,
                "grade": g.grade.word(),
                "points_used": g.item.points_used(),
                "direction": direction(g),
                "change": g.trend.map(|t| t.change),
                "band": g.trend.map(|t| t.band),
            })
        })
        .collect();
    let movers = |ix: &[usize]| ix.iter().map(|&i| key(&model.items[i])).collect::<Vec<_>>();
    let groups: Vec<Value> = model
        .groups
        .iter()
        .map(|g| {
            json!({
                "category": g.category,
                "is_mechanic": g.is_mechanic,
                "volume_div": g.volume_div,
                "volume_share": g.volume_share,
                "index_pct": g.index_pct,
                "coverage": {
                    "items_used": g.coverage.items_used,
                    "items_total": g.coverage.items_total,
                    "volume_share": g.coverage.volume_share,
                },
                "breadth": { "up": g.breadth.up, "down": g.breadth.down, "flat": g.breadth.flat, "of": g.breadth.of },
            })
        })
        .collect();
    let report = json!({
        "league": model.league,
        "currency": "divine",
        "items": items,
        "movers": { "risers": movers(&model.risers), "fallers": movers(&model.fallers) },
        "groups": groups,
    });
    println!("{report}");
    Ok(())
}
