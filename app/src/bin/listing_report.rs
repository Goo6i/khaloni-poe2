//! Prints a fetch body as the listings table would show it, as JSON,
//! offline: `listing_report --fixture <path> --now <unix> --account <name>`.
//!
//! The rows, grouping and price-fixed verdict go through `core::listing`,
//! the path the price-check panel uses. A built-in 16-row bait ladder is
//! judged alongside so the verdict over the fixture can be read against a
//! table that must trip the rule.

use khaloni_poe2_core::listing::{group, group_indices, parse_fetch_body, price_fixed, ListingView, PriceFixed};
use serde_json::{json, Value};

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}

fn row_json(r: &ListingView) -> Value {
    json!({
        "price": r.price.as_ref().map(|(amount, currency)| json!([amount, currency])),
        "indexed_unix": r.indexed_unix,
        "age_text": r.age_text,
        "seller": r.seller,
        "state": r.state.as_str(),
        "is_mine": r.is_mine,
        "stack": r.stack,
        "ilvl": r.ilvl,
        "quality": r.quality,
        "gem_level": r.gem_level,
        "corrupted": r.corrupted,
        "has_note": r.has_note,
        "instant_buyout": r.instant_buyout,
        "card": {
            "name": r.card.name,
            "base": r.card.base,
            "rarity": r.card.rarity,
            "lines": r.card.lines.iter().map(|l| json!({
                "text": l.text,
                "tiers": l.tiers,
                "kind": l.kind.as_str(),
            })).collect::<Vec<Value>>(),
        },
    })
}

fn fixed_json(f: &PriceFixed) -> Value {
    json!({
        "count": f.count,
        "under_currency": f.under_currency,
        "next_price": f.next_price.as_ref().map(|(amount, currency)| json!([amount, currency])),
    })
}

/// Thirteen sellers asking one alchemy or chance orb, then three real
/// prices: the shape of a price-fixed table.
fn bait_ladder(now: i64) -> Vec<ListingView> {
    let entries: Vec<Value> = (0..16)
        .map(|i| {
            let (amount, currency) = match i {
                13 => (2, "exalted"),
                14 => (3, "exalted"),
                15 => (1, "divine"),
                _ if i % 2 == 0 => (1, "alch"),
                _ => (1, "chance"),
            };
            json!({
                "listing": {
                    "indexed": "2026-09-19T08:35:28Z",
                    "price": { "type": "~b/o", "amount": amount, "currency": currency },
                    "account": { "name": format!("bait{i}") },
                },
                "item": { "name": "", "typeLine": "Soldier Cuirass", "rarity": "Rare" },
            })
        })
        .collect();
    parse_fetch_body(&json!({ "result": entries }), now, "").0
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let fixture = arg(&args, "--fixture").ok_or_else(|| anyhow::anyhow!("--fixture <path> is required"))?;
    let now: i64 = arg(&args, "--now").ok_or_else(|| anyhow::anyhow!("--now <unix> is required"))?.parse()?;
    let account = arg(&args, "--account").ok_or_else(|| anyhow::anyhow!("--account <name> is required"))?;

    let body: Value = serde_json::from_str(&std::fs::read_to_string(&fixture)?)?;
    let (rows, dropped) = parse_fetch_body(&body, now, &account);
    let grouped: Vec<Value> = group_indices(&rows).iter().map(|(row, times)| json!({ "row": row, "times": times })).collect();
    let fixed = price_fixed(&group(rows.clone())).map(|f| fixed_json(&f));
    let synthetic = price_fixed(&group(bait_ladder(now)))
        .map(|f| fixed_json(&f))
        .ok_or_else(|| anyhow::anyhow!("the built-in bait ladder did not trip the price-fixed rule"))?;

    let report = json!({
        "rows": rows.iter().map(row_json).collect::<Vec<Value>>(),
        "grouped": grouped,
        "price_fixed": fixed,
        "dropped": dropped,
        "synthetic_price_fixed": synthetic,
    });
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
