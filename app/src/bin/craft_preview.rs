//! Renders the craft planner panel to PNGs, offline:
//! `craft_preview --cache <dir> --bases <base_items.json> --out <dir>`.
//!
//! The mod database and essence texts are the cache directory's
//! `repoe_mods.json` and `xile_essences.json`, or the repository's craft
//! fixture slices when the cache lacks them (run from the repository root).
//! The item is a normal Soldier Cuirass at item level 80 aiming for life
//! and two resistances; the plans are the real planner's, on a small run
//! count and a fixed price snapshot, which the panel's status line says.
//!
//! Writes `picker.png`, `plan.png`, `steps.png` (the first plan opened),
//! `flip.png` (candidates made up here, each costed by the real planner,
//! the first opened) and `unknown.png` (the snapshot without one omen's
//! price, so a plan has no total and says why), at scale 1. `labels.txt`
//! lists every string each view drew, prefixed by the view's file stem and
//! a tab. A dev aid; not shipped.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use khaloni_poe2::craft_ui::{self, Action, FlipList, FlipRow, Fix, Panel, PlanView, Picker, Rates, Resale, View};
use khaloni_poe2::render::Renderer;
use khaloni_poe2_core::craft::data::CraftData;
use khaloni_poe2_core::craft::plan::{model_line, plan, BuyQuote, Plan};
use khaloni_poe2_core::craft::rules::{CORRUPTED_ESSENCES, TIERED_ESSENCES};
use khaloni_poe2_core::craft::sim::SimConfig;
use khaloni_poe2_core::craft::strategy::Target;
use khaloni_poe2_core::craft::types::{ItemState, ModOn, Rarity, Source};
use tiny_skia::{Color, Pixmap};

/// Runs per strategy: enough for a stable ranking, few enough to render in
/// seconds.
const RUNS: usize = 500;

/// Exchange rates of the snapshot (2026-09-19): 459 Exalted Orbs and 8.4
/// Chaos Orbs to the Divine Orb.
const RATES: Rates = Rates { exalted_per_divine: 459.0, chaos_per_divine: 8.4, divine_threshold: 1.0 };

const BASE: &str = "Soldier Cuirass";

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}

fn read(path: &Path) -> anyhow::Result<String> {
    std::fs::read_to_string(path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))
}

/// The cache's file, or the fixture slice when the cache has none.
fn cached_or(cache: &Path, name: &str, fixture: &str) -> anyhow::Result<String> {
    let path = cache.join(name);
    if path.is_file() {
        read(&path)
    } else {
        println!("{} is missing; using {fixture}", path.display());
        read(Path::new(fixture))
    }
}

/// The fixed snapshot the core plan tests use, in Exalted Orbs.
fn snapshot() -> HashMap<String, f64> {
    let mut table: HashMap<String, f64> = [
        ("Orb of Transmutation", 0.05),
        ("Greater Orb of Transmutation", 0.6),
        ("Perfect Orb of Transmutation", 4.0),
        ("Orb of Augmentation", 0.08),
        ("Greater Orb of Augmentation", 0.7),
        ("Perfect Orb of Augmentation", 4.5),
        ("Regal Orb", 0.4),
        ("Greater Regal Orb", 2.0),
        ("Perfect Regal Orb", 9.0),
        ("Exalted Orb", 1.0),
        ("Greater Exalted Orb", 3.5),
        ("Perfect Exalted Orb", 16.0),
        ("Chaos Orb", 1.3),
        ("Greater Chaos Orb", 4.0),
        ("Perfect Chaos Orb", 18.0),
        ("Orb of Annulment", 6.0),
        ("Orb of Alchemy", 0.3),
        ("Fracturing Orb", 30.0),
        ("Omen of Sinistral Exaltation", 9.0),
        ("Omen of Dextral Exaltation", 11.0),
        ("Omen of Sinistral Annulment", 7.0),
        ("Omen of Dextral Annulment", 8.0),
        ("Omen of Sinistral Erasure", 3.0),
        ("Omen of Dextral Erasure", 3.5),
        ("Omen of Whittling", 25.0),
        ("Omen of Light", 4.0),
        ("Omen of Sinistral Crystallisation", 5.0),
        ("Omen of Dextral Crystallisation", 5.0),
        ("Omen of Sinistral Necromancy", 6.0),
        ("Omen of Dextral Necromancy", 6.0),
        ("Omen of Abyssal Echoes", 10.0),
        ("Omen of the Sovereign", 12.0),
        ("Omen of the Liege", 12.0),
        ("Omen of the Blackblooded", 12.0),
        ("Preserved Collarbone", 2.0),
        ("Preserved Rib", 2.0),
        ("Preserved Jawbone", 2.0),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    for of in TIERED_ESSENCES {
        table.insert(format!("Lesser Essence of {of}"), 0.2);
        table.insert(format!("Essence of {of}"), 0.8);
        table.insert(format!("Greater Essence of {of}"), 3.0);
        table.insert(format!("Perfect Essence of {of}"), 45.0);
    }
    for of in CORRUPTED_ESSENCES {
        table.insert(format!("Essence of {of}"), 25.0);
    }
    table
}

/// The omen the unknown view's snapshot leaves out.
const LEFT_OUT: &str = "Omen of Dextral Exaltation";

fn item(data: &CraftData, rarity: Rarity, mods: Vec<ModOn>) -> anyhow::Result<ItemState> {
    let b = data.base(BASE).ok_or_else(|| anyhow::anyhow!("{BASE} is not in the base data"))?;
    Ok(ItemState {
        class: b.class.clone(),
        base: b.name.clone(),
        base_tags: b.tags.clone(),
        item_level: 80,
        rarity,
        corrupted: false,
        mirrored: false,
        sanctified: false,
        mods,
        sockets: 0,
    })
}

/// The modifier the entry `id` puts on a Soldier Cuirass, worded with a
/// roll inside its range.
fn on(data: &CraftData, id: &str, text: &str) -> anyhow::Result<ModOn> {
    let entry = data.entry(id).ok_or_else(|| anyhow::anyhow!("{id} is not in the mod data"))?;
    let b = data.base(BASE).ok_or_else(|| anyhow::anyhow!("{BASE} is not in the base data"))?;
    let tags: Vec<&str> = b.tags.iter().map(String::as_str).collect();
    let mut m = data.candidate(entry, &tags).to_mod(Source::Random);
    m.text = text.to_string();
    Ok(m)
}

fn labels(stem: &str, r: &Renderer, p: &Panel, out: &mut String) {
    for text in craft_ui::all_text(p, &|s| r.evaluate_label_width(s)) {
        if !text.is_empty() {
            out.push_str(&format!("{stem}\t{text}\n"));
        }
    }
}

fn render(r: &Renderer, p: &Panel, path: &Path, drawn: &mut String) -> anyhow::Result<()> {
    labels(&path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(), r, p, drawn);
    let lay = craft_ui::layout(p, &|s| r.evaluate_label_width(s));
    let (w, h) = ((lay.w + 40) as u32, (lay.h + 40) as u32);
    let mut pm = Pixmap::new(w, h).ok_or_else(|| anyhow::anyhow!("pixmap {w}x{h}"))?;
    // A mid-dark backdrop standing in for the game.
    pm.fill(Color::from_rgba8(0x2A, 0x2E, 0x33, 0xFF));
    r.draw_craft(&mut pm, p, &lay, (20, 20));
    pm.save_png(path)?;
    println!("{} {}x{}", path.display(), w, h);
    Ok(())
}

/// Ticks `family` in the picker and moves its minimum to `tier`.
fn want(p: &mut Panel, family: &str, tier: u8) -> anyhow::Result<()> {
    let i = p
        .picker
        .families
        .iter()
        .position(|f| f.family == family)
        .ok_or_else(|| anyhow::anyhow!("{family} is not rollable on {BASE}"))?;
    if p.ticks[i].is_none() {
        craft_ui::apply(p, &Action::Tick(i));
    }
    while p.ticks[i].is_some_and(|t| t < tier) {
        if !craft_ui::apply(p, &Action::TierWorse(i)) {
            break;
        }
    }
    while p.ticks[i].is_some_and(|t| t > tier) {
        if !craft_ui::apply(p, &Action::TierBetter(i)) {
            break;
        }
    }
    anyhow::ensure!(p.ticks[i] == Some(tier), "{family} cannot be set to tier {tier}");
    Ok(())
}

/// A made-up candidate costed by the real planner from its own item.
struct Candidate {
    listed: f64,
    seller: &'static str,
    online: bool,
    state: ItemState,
    resale: Result<Resale, String>,
    listings: Vec<String>,
    url: &'static str,
    /// Leave this omen's price out for this candidate, as a price table
    /// that lacks it would.
    without: Option<&'static str>,
}

fn flip_row(data: &CraftData, target: &Target, c: Candidate) -> FlipRow {
    let table = snapshot();
    let price = |name: &str| if Some(name) == c.without { None } else { table.get(name).copied() };
    let cfg = SimConfig { runs: RUNS, ..SimConfig::default() };
    let planned = plan(&c.state, target, data, None, &price, None, &cfg);
    let best = planned.strategies.iter().find(|r| r.costed.per_finished.is_some());
    let fix = match best {
        Some(r) => Ok(Fix { cost: r.costed.per_finished.unwrap_or_default(), strategy: r.costed.id.to_string(), median: r.costed.median }),
        None => Err(planned
            .strategies
            .iter()
            .flat_map(|r| r.costed.unknowns.first())
            .next()
            .cloned()
            .unwrap_or_else(|| "unknown: no crafting strategy applies to this item".to_string())),
    };
    let shown = best.or(planned.strategies.first());
    let mut card = vec![format!("Rare {BASE} · item level {}", c.state.item_level)];
    card.extend(c.state.mods.iter().map(|m| match m.tier {
        Some(t) => format!("{}  {}", craft_ui::tier_text(m.kind, t), m.text),
        None => m.text.clone(),
    }));
    FlipRow {
        listed: c.listed,
        seller: c.seller.to_string(),
        online: c.online,
        fix,
        resale: c.resale,
        observed: None,
        card,
        steps: shown.map(|r| craft_ui::step_lines(&r.costed, &RATES)).unwrap_or_default(),
        resale_listings: c.listings,
        url: c.url.to_string(),
    }
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cache = PathBuf::from(arg(&args, "--cache").ok_or_else(|| anyhow::anyhow!("--cache <dir> is required"))?);
    let bases = PathBuf::from(arg(&args, "--bases").ok_or_else(|| anyhow::anyhow!("--bases <file> is required"))?);
    let out = PathBuf::from(arg(&args, "--out").ok_or_else(|| anyhow::anyhow!("--out <dir> is required"))?);
    std::fs::create_dir_all(&out)?;

    let mods = cached_or(&cache, "repoe_mods.json", "core/tests/fixtures/craft/mods_slice.json")?;
    let essences = cached_or(&cache, "xile_essences.json", "core/tests/fixtures/craft/essences_slice.json")?;
    let data = CraftData::load(&mods, &read(&bases)?, &essences).map_err(anyhow::Error::msg)?;
    let r = Renderer::new()?;
    let mut drawn = String::new();

    let start = item(&data, Rarity::Normal, Vec::new())?;
    let picker = Arc::new(Picker::build(&data, &start));
    let mut p = Panel::new(picker, RATES, "prices: fixed test snapshot".to_string());
    want(&mut p, "IncreasedLife", 3)?;
    want(&mut p, "FireResistance", 3)?;
    want(&mut p, "ColdResistance", 3)?;
    render(&r, &p, &out.join("picker.png"), &mut drawn)?;

    let target = p.target();
    let table = snapshot();
    let price = |name: &str| table.get(name).copied();
    let cfg = SimConfig { runs: RUNS, ..SimConfig::default() };
    // Fifteen divines: dearer than the best plans, cheaper than the rest.
    let buy = Some(BuyQuote { price: 15.0 * RATES.exalted_per_divine, listings: 3 });
    craft_ui::apply(&mut p, &Action::Plan);
    let full: Plan = plan(&start, &target, &data, None, &price, buy, &cfg);
    p.set_plan(PlanView::build(&full, &RATES));
    render(&r, &p, &out.join("plan.png"), &mut drawn)?;
    craft_ui::apply(&mut p, &Action::ExpandRow(0));
    render(&r, &p, &out.join("steps.png"), &mut drawn)?;

    // The same item and target with one omen's price missing: every plan
    // that slams with it has no total, and its step says why.
    let mut u = p.clone();
    u.prices = format!("prices: fixed test snapshot without {LEFT_OUT}");
    let lacking = |name: &str| if name == LEFT_OUT { None } else { table.get(name).copied() };
    let partial = plan(&start, &target, &data, None, &lacking, buy, &cfg);
    let first_unknown = partial.strategies.iter().position(|s| s.costed.per_finished.is_none());
    u.set_plan(PlanView::build(&partial, &RATES));
    if let Some(i) = first_unknown {
        craft_ui::apply(&mut u, &Action::ExpandRow(i));
    }
    render(&r, &u, &out.join("unknown.png"), &mut drawn)?;

    let rare = |mods| item(&data, Rarity::Rare, mods);
    let candidates = vec![
        Candidate {
            listed: 4.0 * RATES.exalted_per_divine,
            seller: "Vessel_of_Kulemak",
            online: true,
            state: rare(vec![
                on(&data, "IncreasedLife11", "+177 to maximum Life")?,
                on(&data, "FireResist6", "+34% to Fire Resistance")?,
                on(&data, "Strength2", "+10 to Strength")?,
            ])?,
            resale: Ok(Resale { price: 30.0 * RATES.exalted_per_divine, listings: 3 }),
            listings: vec!["30 div · online".into(), "32 div · online".into(), "35 div · offline".into()],
            url: "https://www.pathofexile.com/trade2/search/poe2/Forbidden%20Rites/example-1",
            without: None,
        },
        Candidate {
            listed: 1.5 * RATES.exalted_per_divine,
            seller: "Ashen_Wanderer",
            online: false,
            state: rare(vec![
                on(&data, "IncreasedLife12", "+196 to maximum Life")?,
                on(&data, "ColdResist6", "+33% to Cold Resistance")?,
            ])?,
            resale: Ok(Resale { price: 30.0 * RATES.exalted_per_divine, listings: 3 }),
            listings: vec!["30 div · online".into(), "32 div · online".into(), "35 div · offline".into()],
            url: "https://www.pathofexile.com/trade2/search/poe2/Forbidden%20Rites/example-2",
            without: None,
        },
        Candidate {
            listed: 2.0 * RATES.exalted_per_divine,
            seller: "Trialmaster_Fan",
            online: true,
            state: rare(vec![on(&data, "IncreasedLife12", "+201 to maximum Life")?])?,
            resale: Ok(Resale { price: 30.0 * RATES.exalted_per_divine, listings: 3 }),
            listings: vec!["30 div · online".into(), "32 div · online".into(), "35 div · offline".into()],
            url: "https://www.pathofexile.com/trade2/search/poe2/Forbidden%20Rites/example-3",
            without: Some(LEFT_OUT),
        },
    ];
    let rows: Vec<FlipRow> = candidates.into_iter().map(|c| flip_row(&data, &target, c)).collect();
    let mut f = p.clone();
    f.prices = "prices: fixed test snapshot; candidates made up for this preview".to_string();
    f.set_flips(FlipList::new(
        format!("{BASE}: life P3, fire S3, cold S3"),
        craft_ui::scan_text(5, 10, 27, 30),
        model_line(None),
        rows,
    ));
    craft_ui::apply(&mut f, &Action::SetView(View::Flips));
    craft_ui::apply(&mut f, &Action::SelectCandidate(0));
    render(&r, &f, &out.join("flip.png"), &mut drawn)?;

    std::fs::write(out.join("labels.txt"), drawn)?;
    Ok(())
}
