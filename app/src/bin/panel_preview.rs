//! Renders the price-check panel to PNGs from a fetch fixture, offline:
//! `panel_preview --fixture <fetch.json> --exchange <exchange.json>
//! --cache <dir> --league <name> --out <dir>`. Writes `table.png` (the
//! listings table), `hover.png` (row 1 hovered, its card beside the panel),
//! `ninja.png` (the poe.ninja block), `closest.png` (the closest listings
//! and what each mod is worth), `bulk.png` (the exchange offers) and
//! `price-fixed.png` (a 16-row bait ladder tripping the strip),
//! `extra.png` (the lines EE2 gives no row of their own, as rows, one of
//! them ticked, and a line no catalog lists), plus
//! `labels.txt`: every string each view drew, prefixed by the view's file
//! stem and a tab. A dev aid; not shipped.
//!
//! Every block goes through `appraise`, the path the overlay's trade
//! worker takes: the rows, the ladder and the strip through
//! `appraise::blocks` over the fixture, the bulk view through
//! `appraise::bulk_block` over the exchange body, the poe.ninja block from
//! the cached overviews of `--league`, at a fixed clock and with "seller3"
//! as the configured account. The closest listings compare the fixture's
//! rows with the parity corpus's Soldier Cuirass, built the way the price
//! check builds it; the attribution rows are worded from the fixture's own
//! prices, since no second search runs offline (`closest\tsample data`
//! says so in labels.txt), and so is the bulk block when the exchange
//! fixture is missing (`bulk\tsample data`).

use khaloni_poe2::appraise::{self, Fetched};
use khaloni_poe2::evaluate_ui::{self as ev, ListingRow, SellerState};
use khaloni_poe2::render::Renderer;
use khaloni_poe2_core::bulk;
use khaloni_poe2_core::ee2::request::Built;
use khaloni_poe2_core::ee2::{data, Ee2Data};
use khaloni_poe2_core::market::{self, Floors, Grade};
use khaloni_poe2_core::ninja::PriceTable;
use serde_json::{json, Value};
use std::collections::HashMap;
use tiny_skia::{Color, Pixmap};

const NOW: i64 = 1_790_000_000;
const ME: &str = "seller3";

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}

fn read_json(path: &str) -> anyhow::Result<Value> {
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}

/// The trade currency ids the fixtures use, by the names the price table
/// knows them under.
fn currency_names() -> HashMap<String, String> {
    HashMap::from([
        ("exalted".to_string(), "Exalted Orb".to_string()),
        ("divine".to_string(), "Divine Orb".to_string()),
        ("chaos".to_string(), "Chaos Orb".to_string()),
        ("omen-of-whittling".to_string(), "Omen of Whittling".to_string()),
    ])
}

/// Thirteen sellers asking one alchemy or chance orb, then three real
/// prices: the shape of a price-fixed table, as `listing_report` builds it.
fn bait_ladder() -> Value {
    let result: Vec<Value> = (0..16)
        .map(|i| {
            let (amount, currency) = match i {
                13 => (2, "exalted"),
                14 => (3, "exalted"),
                15 => (1, "divine"),
                _ if i % 2 == 0 => (1, "alch"),
                _ => (1, "chance"),
            };
            json!({
                "id": format!("bait{i}"),
                "listing": {
                    "indexed": "2026-09-19T08:35:28Z",
                    "price": { "type": "~b/o", "amount": amount, "currency": currency },
                    "account": { "name": format!("bait{i}"), "online": { "status": "online" } },
                },
                "item": { "name": "", "typeLine": "Warlord Cuirass", "rarity": "Rare", "ilvl": 80 },
            })
        })
        .collect();
    json!({ "result": result })
}

/// The corpus's Soldier Cuirass, built the way the price check builds it,
/// when the parity corpus and the EE2 data are beside the crate.
fn built_cuirass() -> Option<Built> {
    built_corpus_item("ee2-ArmourHighValueRareItem")
}

/// A parity-corpus item built the way the price check builds it.
fn built_corpus_item(name: &str) -> Option<Built> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tools/ee2-parity");
    let read = |p: &std::path::Path| std::fs::read_to_string(p).ok();
    let stats = read(&dir.join("data/stats.ndjson"))?;
    let items = read(&dir.join("data/items.ndjson"))?;
    let mut d = Ee2Data::from_ndjson(&stats, &items).ok()?;
    d.trade_stats = data::TradeStatTexts::from_json(&read(&dir.join("data/trade-stats.json"))?).ok();
    d.trade_items = data::trade_item_names(&read(&dir.join("data/trade-items.json"))?).ok();
    let text = read(&dir.join(format!("items/{name}.txt")))?;
    khaloni_poe2_core::ee2::request::build(&text, &d).ok()
}

fn sample_bulk() -> Vec<ev::BulkOffer> {
    let offer = |have: &str, want: &str, stock: &str, seller: &str, state| ev::BulkOffer {
        have: have.into(),
        want: want.into(),
        stock: stock.into(),
        seller: seller.into(),
        state,
    };
    vec![
        offer("1 Omen of Whittling", "3 Exalted Orb", "40", "sellerB#1002", SellerState::Afk),
        offer("1 Omen of Whittling", "4 Exalted Orb", "12", "sellerA#1001", SellerState::Online),
        offer("1 Omen of Whittling", "5 Exalted Orb", "1", "sellerC#1003", SellerState::Offline),
    ]
}

/// The poe.ninja block for the first liquid item with a trend in the
/// cached market, worded by `core::market` as the market view words it.
fn ninja_block(cache: &str, league: &str, table: &PriceTable) -> Option<(String, ev::NinjaBlock)> {
    let m = khaloni_poe2::prices::market_from_cache(std::path::Path::new(cache), league);
    if m.source.items.is_empty() {
        return None;
    }
    let model = appraise::market_model(&m, Floors::default(), NOW);
    let pick = model
        .items
        .iter()
        .find(|g| g.grade == Grade::Liquid && g.trend.is_some_and(|t| t.direction != market::Direction::Unclear))
        .or_else(|| model.items.iter().find(|g| g.grade != Grade::Untrusted))?;
    let block =
        market::ninja_block(&model, &pick.item.name, pick.item.base_type.as_deref(), pick.item.corrupted, None)?;
    Some((pick.item.name.clone(), appraise::ninja_view(&block, table, 1.0)))
}

fn panel(name: &str, rarity: &str, rows: Vec<ev::StatRow>, status: &str) -> ev::Panel {
    ev::Panel {
        header: ev::ItemHeader {
            name: name.into(),
            rarity: rarity.into(),
            item_level: Some(81),
            requires_level: Some(65),
            base: Some(ev::BaseToggle { label: "Category: Body Armour".into(), enabled: true }),
        },
        rows,
        status: status.into(),
        search_id: Some("preview".into()),
        ..ev::Panel::default()
    }
}

fn mod_row(label: &str, kind: ev::AffixKind, tier: u8, score: f32, min: f64, i: usize) -> ev::StatRow {
    ev::StatRow {
        label: label.into(),
        badge: Some(ev::TierBadge { kind, tier }),
        score: Some(score),
        min: Some(min),
        max: None,
        enabled: true,
        target: Some(ev::Target::Stat(i)),
        hidden: false,
        group: ev::RowGroup::Explicit,
        note: None,
    }
}

fn armour_rows() -> Vec<ev::StatRow> {
    vec![
        ev::StatRow {
            label: "Armour: 2972".into(),
            badge: None,
            score: None,
            min: Some(2600.0),
            max: None,
            enabled: true,
            target: Some(ev::Target::Equipment(ev::EquipKey::Armour)),
            hidden: false,
            group: ev::RowGroup::Property,
            note: None,
        },
        mod_row("+348 to Armour", ev::AffixKind::Prefix, 1, 4.2, 348.0, 0),
        mod_row("+60 to maximum Life", ev::AffixKind::Prefix, 3, 3.0, 60.0, 1),
        mod_row("+45% to Lightning Resistance", ev::AffixKind::Suffix, 1, 4.8, 45.0, 2),
        ev::StatRow { enabled: false, ..mod_row("35% reduced Attribute Requirements", ev::AffixKind::Suffix, 1, 4.0, 35.0, 3) },
    ]
}

fn labels(stem: &str, r: &Renderer, p: &ev::Panel, out: &mut String) {
    for text in ev::all_text(p, &|s| r.evaluate_label_width(s)) {
        out.push_str(&format!("{stem}\t{text}\n"));
    }
}

fn render(r: &Renderer, p: &ev::Panel, path: &std::path::Path, drawn: &mut String) -> anyhow::Result<()> {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    labels(&stem, r, p, drawn);
    let lay = ev::layout(p, &|s| r.evaluate_label_width(s));
    // The card can sit to either side: give the canvas room for it.
    let (mut x0, mut x1, mut y1) = (0, lay.size.0, lay.size.1);
    if let Some(c) = &lay.card {
        x0 = x0.min(c.rect.x);
        x1 = x1.max(c.rect.x + c.rect.w as i32);
        y1 = y1.max(c.rect.y + c.rect.h as i32);
    }
    let (w, h) = ((x1 - x0 + 40) as u32, (y1 + 40) as u32);
    let mut pm = Pixmap::new(w, h).ok_or_else(|| anyhow::anyhow!("pixmap {w}x{h}"))?;
    // A mid-dark backdrop standing in for the game, so the panel's own
    // translucent fill reads the way it does in play.
    pm.fill(Color::from_rgba8(0x2A, 0x2E, 0x33, 0xFF));
    r.draw_evaluate(&mut pm, p, &lay, (20 - x0, 20), None, "");
    pm.save_png(path)?;
    println!("{} {}x{}", path.display(), w, h);
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let need = |flag: &str| arg(&args, flag).ok_or_else(|| anyhow::anyhow!("{flag} <value> is required"));
    let fixture = need("--fixture")?;
    let exchange = need("--exchange")?;
    let cache = need("--cache")?;
    let league = need("--league")?;
    let out = std::path::PathBuf::from(need("--out")?);
    std::fs::create_dir_all(&out)?;

    // The currency table: the cached currency overview of `--league` when
    // there is one (its Divine Orb and Chaos Orb lines are what a listing
    // in those converts through), else the poe.ninja fixture's.
    let table = khaloni_poe2_core::ninja::cached_exchange_overview(std::path::Path::new(&cache), &league, "Currency")
        .map(|ov| PriceTable::build(&[ov]))
        .unwrap_or_else(|| {
            let ov: khaloni_poe2_core::ninja::ExchangeOverview =
                serde_json::from_str(include_str!("../../../core/tests/fixtures/ninja_currency.json"))
                    .expect("the fixture parses");
            PriceTable::build(&[ov])
        });
    let names = currency_names();

    let body = read_json(&fixture)?;
    let raw: Vec<Option<Value>> = body["result"].as_array().cloned().unwrap_or_default().into_iter().map(Some).collect();
    let built = built_cuirass();
    let fetched = Fetched {
        raw: &raw,
        total: Some(1934),
        now_unix: NOW,
        my_account: ME,
        currency_names: &names,
        table: &table,
        divine_threshold: 1.0,
        built: built.as_ref(),
    };
    let blocks = appraise::blocks(&fetched);
    let rows: Vec<ListingRow> = blocks.listings.clone();
    let status = format!("{} of 1,934 shown ({} gone or unpriced)", blocks.shown, blocks.dropped);
    let r = Renderer::new()?;
    let mut drawn = String::new();

    let base = ev::Panel {
        listings: rows.clone(),
        ladder: blocks.ladder.clone(),
        budget_text: "searches 4/30 (5 min)".into(),
        ..panel("Sol Wrap", "Rare", armour_rows(), &status)
    };

    // The table alone, then the same with row 1 hovered.
    render(&r, &base, &out.join("table.png"), &mut drawn)?;
    let hover = ev::Panel { hover: Some(1), ..base.clone() };
    render(&r, &hover, &out.join("hover.png"), &mut drawn)?;

    // poe.ninja: a tracked item's line from the cached market. A unique
    // with the fixture's rows under it, the budget near its cap.
    let (tracked, ninja) = match ninja_block(&cache, &league, &table) {
        Some(found) => found,
        None => {
            drawn.push_str("ninja\tsample data\n");
            let sample = ev::NinjaBlock {
                price: "4.6 div".into(),
                band: market::band_text_ascii(12),
                direction: "rising".into(),
                volume: market::volume_text(120.0),
                note: String::new(),
            };
            ("Orb of Alchemy".to_string(), sample)
        }
    };
    let mut unique = panel(&tracked, "Currency", Vec::new(), &status);
    unique.header.base = None;
    unique.header.item_level = None;
    unique.header.requires_level = None;
    let with_ninja = ev::Panel {
        ninja: Some(ninja),
        budget_text: "searches 27/30 (5 min)".into(),
        budget_low: true,
        listings: rows.clone(),
        ladder: blocks.ladder.clone(),
        ..unique
    };
    render(&r, &with_ninja, &out.join("ninja.png"), &mut drawn)?;

    // Closest listings from the comparison itself when the corpus is
    // beside the crate, and what each mod is worth worded from the
    // fixture's own prices: no second search runs offline.
    drawn.push_str("closest\tsample data\n");
    let closest = blocks.closest.clone().unwrap_or_else(|| ev::ClosestBlock {
        lines: vec![format!("3 listings within one tier: {}, {}, {}", rows[3].price, rows[4].price, rows[5].price)],
        nearest: Some(format!("nearest listing: {} by {}, {}", rows[3].price, rows[3].seller, rows[3].age)),
    });
    let baseline = blocks.cheapest.clone().unwrap_or((2.0, "seller1".into()));
    let attribution = appraise::attribution_rows(
        (baseline.0, &baseline.1),
        &[
            ("T1 armour".to_string(), Some((baseline.0 * 0.4, rows[0].seller.clone()))),
            ("T1 lightning res".to_string(), Some((baseline.0 * 0.7, rows[2].seller.clone()))),
        ],
        &blocks.unit,
    );
    let with_closest = ev::Panel { closest: Some(closest), attribution, attribute_enabled: true, ..base.clone() };
    render(&r, &with_closest, &out.join("closest.png"), &mut drawn)?;

    // Bulk offers for a stackable, with the stack's worth on the header.
    let bulk_block = match read_json(&exchange) {
        Ok(body) => appraise::bulk_block(&bulk::parse_exchange(&body), &names),
        Err(_) => {
            drawn.push_str("bulk\tsample data\n");
            let offers = sample_bulk();
            let note = format!("{} offers, cheapest first", offers.len());
            ev::BulkBlock { offers, note }
        }
    };
    let status = format!("{} exchange offers", bulk_block.offers.len());
    let mut stack = panel("Omen of Whittling", "Currency", Vec::new(), &status);
    stack.header.item_level = None;
    stack.header.requires_level = None;
    stack.header.base = None;
    let with_bulk = ev::Panel {
        bulk: Some(bulk_block),
        stack_value: Some(appraise::stack_value_text(37, 3.0, &table, 1.0)),
        budget_text: "searches 4/30 (5 min)".into(),
        ..stack
    };
    render(&r, &with_bulk, &out.join("bulk.png"), &mut drawn)?;

    // A bait ladder: 15 rows in augmentation orbs and one honest price.
    let bait = bait_ladder();
    let bait_raw: Vec<Option<Value>> = bait["result"].as_array().cloned().unwrap_or_default().into_iter().map(Some).collect();
    let bait_blocks = appraise::blocks(&Fetched { raw: &bait_raw, total: Some(220), built: None, ..fetched });
    if bait_blocks.price_fixed.is_none() {
        anyhow::bail!("the bait ladder must trip the rule");
    }
    let with_strip = ev::Panel {
        listings: bait_blocks.listings,
        ladder: bait_blocks.ladder,
        price_fixed: bait_blocks.price_fixed,
        ..base.clone()
    };
    render(&r, &with_strip, &out.join("price-fixed.png"), &mut drawn)?;

    // The extra rows: the cuirass's lines EE2 folded into its figures and
    // totals, their ids from the cached trade catalog when there is one,
    // the first of them ticked; and a line no catalog lists.
    let catalog = std::fs::read_to_string(std::path::Path::new(&cache).join("trade_stats.json"))
        .ok()
        .and_then(|body| khaloni_poe2_core::trade::StatIndex::from_json(&body).ok());
    if catalog.is_none() {
        drawn.push_str("extra\tpinned ids\n");
    }
    let mut lines = Vec::new();
    for name in ["ee2-ArmourHighValueRareItem", "edge-unknown-mod-gloves"] {
        let mut b = built_corpus_item(name).ok_or_else(|| anyhow::anyhow!("{name} does not build"))?;
        if let Some(c) = &catalog {
            b.resolve_extra(c);
        }
        lines.extend(b.extra.into_iter().filter(|x| name != "edge-unknown-mod-gloves" || x.ids.is_empty()));
    }
    let mut extras = Vec::new();
    let mut rows = armour_rows();
    rows.extend(ev::extra_rows(&lines, &mut extras));
    let mut with_extra = ev::Panel { extras, ..panel("Soldier Cuirass", "Rare", rows, &status) };
    let first = with_extra.rows.iter().position(|r| matches!(r.target, Some(ev::Target::Extra(_))));
    if let Some(i) = first {
        let mut q = khaloni_poe2_core::trade::Query::default();
        ev::toggle_row(&mut with_extra, &mut q, i);
    }
    render(&r, &with_extra, &out.join("extra.png"), &mut drawn)?;

    std::fs::write(out.join("labels.txt"), drawn)?;
    Ok(())
}
