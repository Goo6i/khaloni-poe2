//! The craft panel's model, geometry and wording, on the craft fixtures the
//! core plan tests use (the mod slice, the base sample and the essence
//! slice) and on real `plan` output.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use khaloni_poe2::craft_ui::{
    self, Action, FlipList, FlipRow, Fix, Ink, Layout, ObservedMargin, Panel, Picker, PlanView, Rates, Resale, Style,
    View, PAGE_ROWS,
};
use khaloni_poe2::render::Renderer;
use khaloni_poe2_core::craft::data::{CraftData, Domain};
use khaloni_poe2_core::craft::model::{Model, UNIFORM_LABEL};
use khaloni_poe2_core::craft::plan::{model_line, plan, BuyQuote, Plan};
use khaloni_poe2_core::craft::rules::{CORRUPTED_ESSENCES, PATCH, TIERED_ESSENCES};
use khaloni_poe2_core::craft::sim::SimConfig;
use khaloni_poe2_core::craft::strategy::{Target, Want};
use khaloni_poe2_core::craft::types::{AffixKind, ItemState, ModOn, Rarity, Source};

const BASE: &str = "Soldier Cuirass";
const RATES: Rates = Rates { exalted_per_divine: 459.0, chaos_per_divine: 8.4, divine_threshold: 1.0 };
const RUNS: usize = 300;

/// Fixed-advance stand-in for the glyph measurer.
fn m(s: &str) -> i32 {
    7 * s.chars().count() as i32
}

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

fn item(ilvl: u32, rarity: Rarity, mods: Vec<ModOn>) -> ItemState {
    let b = craft().base(BASE).expect("the cuirass is in the base sample");
    ItemState {
        class: b.class.clone(),
        base: b.name.clone(),
        base_tags: b.tags.clone(),
        item_level: ilvl,
        rarity,
        corrupted: false,
        mirrored: false,
        sanctified: false,
        mods,
        sockets: 0,
    }
}

fn on(id: &str) -> ModOn {
    let data = craft();
    let tags: Vec<&str> = data.base(BASE).unwrap().tags.iter().map(String::as_str).collect();
    data.candidate(data.entry(id).unwrap_or_else(|| panic!("{id} is in the slice")), &tags).to_mod(Source::Random)
}

/// The fixed price snapshot of the core plan tests, in Exalted Orbs.
fn prices() -> HashMap<String, f64> {
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

const LEFT_OUT: &str = "Omen of Dextral Exaltation";

fn target() -> Target {
    Target {
        wants: vec![
            Want { family: "IncreasedLife".into(), kind: AffixKind::Prefix, min_tier: 3 },
            Want { family: "FireResistance".into(), kind: AffixKind::Suffix, min_tier: 3 },
            Want { family: "ColdResistance".into(), kind: AffixKind::Suffix, min_tier: 3 },
        ],
    }
}

fn run(model: Option<&Model>, without: Option<&str>, buy: Option<BuyQuote>) -> Plan {
    let table = prices();
    let price = |name: &str| if Some(name) == without { None } else { table.get(name).copied() };
    plan(&item(80, Rarity::Normal, vec![]), &target(), craft(), model, &price, buy, &SimConfig { runs: RUNS, ..SimConfig::default() })
}

/// The golden cuirass case: a normal base, life and two resistances, a buy
/// quote of 250 ex.
fn full_plan() -> &'static Plan {
    static PLAN: OnceLock<Plan> = OnceLock::new();
    PLAN.get_or_init(|| run(None, None, Some(BuyQuote { price: 250.0, listings: 3 })))
}

/// The same with the Dextral omen's price missing.
fn lacking_plan() -> &'static Plan {
    static PLAN: OnceLock<Plan> = OnceLock::new();
    PLAN.get_or_init(|| run(None, Some(LEFT_OUT), Some(BuyQuote { price: 250.0, listings: 3 })))
}

/// Under an observed model that saw every eligible entry of the base once,
/// on 412 listings.
fn observed_plan() -> &'static Plan {
    static PLAN: OnceLock<Plan> = OnceLock::new();
    PLAN.get_or_init(|| {
        let tags: Vec<&str> = craft().base(BASE).unwrap().tags.iter().map(String::as_str).collect();
        let mut counts = HashMap::new();
        let mut totals = HashMap::new();
        for e in craft().entries().iter().filter(|e| e.domain == Domain::Item && e.eligible(tags.iter().copied())) {
            counts.insert(e.id.clone(), 1);
            *totals.entry(e.kind).or_insert(0) += 1;
        }
        let model = Model::Observed { class: "Body Armour".into(), counts, totals, listings: 412 };
        run(Some(&model), None, None)
    })
}

fn picker_panel(state: &ItemState) -> Panel {
    Panel::new(Arc::new(Picker::build(craft(), state)), RATES, "prices: fixed test snapshot".into())
}

fn plan_panel(p: &Plan) -> Panel {
    let mut panel = picker_panel(&item(80, Rarity::Normal, vec![]));
    panel.set_plan(PlanView::build(p, &RATES));
    panel
}

fn click(p: &mut Panel, x: i32, y: i32) -> Option<Action> {
    let lay = craft_ui::layout(p, &m);
    let action = craft_ui::hit(&lay, x, y);
    if let Some(a) = &action {
        craft_ui::apply(p, a);
    }
    action
}

fn centre(r: &khaloni_poe2::config::Rect) -> (i32, i32) {
    (r.x + r.w as i32 / 2, r.y + r.h as i32 / 2)
}

fn texts(lay: &Layout) -> Vec<&str> {
    lay.texts.iter().map(|t| t.text.as_str()).collect()
}

/// The drawn strings joined, so a sentence wrapped over lines reads whole.
fn joined(p: &Panel) -> String {
    craft_ui::all_text(p, &m).join(" ")
}

/// Candidates for the flip list: two with a margin, one whose fix has no
/// cost, and one with no resale.
fn flip_rows() -> Vec<FlipRow> {
    let row = |listed: f64, seller: &str, fix: Result<Fix, String>, resale: Result<Resale, String>| FlipRow {
        listed,
        seller: seller.into(),
        online: true,
        fix,
        resale,
        observed: None,
        card: vec![format!("Rare {BASE}"), "P2  +196 to maximum Life".into()],
        steps: vec!["Exalted Orb · ~7 uses · 7 ex".into()],
        resale_listings: vec!["6 div · online".into()],
        url: format!("https://www.pathofexile.com/trade2/search/poe2/Standard/{seller}"),
    };
    let fix = |cost: f64| Ok(Fix { cost, strategy: "orb-chain".into(), median: Some(cost * 0.6) });
    let six = Ok(Resale { price: 6.0 * 459.0, listings: 3 });
    let mut observed = row(2.0 * 459.0, "Second", fix(0.9 * 459.0), six.clone());
    observed.observed = Some(ObservedMargin { margin: 2.2 * 459.0, label: "observed on 412 listings of Body Armour".into() });
    observed.online = false;
    vec![
        row(1.0 * 459.0, "Unfixable", Err("unknown: no price for Omen of Dextral Exaltation".into()), six.clone()),
        observed,
        row(1.0 * 459.0, "First", fix(0.5 * 459.0), six),
        row(1.0 * 459.0, "Unsold", fix(0.5 * 459.0), Err("no listing at or above the profile".into())),
    ]
}

fn flip_panel() -> Panel {
    let mut p = picker_panel(&item(80, Rarity::Normal, vec![]));
    p.set_flips(FlipList::new(
        "Soldier Cuirass: life P3, fire S3, cold S3".into(),
        craft_ui::scan_text(5, 10, 27, 30),
        model_line(None),
        flip_rows(),
    ));
    craft_ui::apply(&mut p, &Action::SetView(View::Flips));
    p
}

// ---------------------------------------------------------------- L1

#[test]
fn the_picker_lists_every_family_the_base_can_roll_with_current_ones_ticked() {
    let data = craft();
    let state = item(82, Rarity::Rare, vec![on("IncreasedLife13"), on("FireResist7"), on("Strength2")]);
    let picker = Picker::build(data, &state);

    // Every family the base can roll, worked out here from the raw entries:
    // item domain, not essence-only, eligible by the base's tags.
    let tags: Vec<&str> = state.base_tags.iter().map(String::as_str).collect();
    let rollable: BTreeSet<(AffixKind, String)> = data
        .entries()
        .iter()
        .filter(|e| e.domain == Domain::Item && !e.essence_only && e.eligible(tags.iter().copied()))
        .map(|e| (e.kind, e.family.clone()))
        .collect();
    let listed: Vec<(AffixKind, String)> = picker.families.iter().map(|f| (f.kind, f.family.clone())).collect();
    assert_eq!(listed.len(), listed.iter().cloned().collect::<BTreeSet<_>>().len(), "each family once");
    // The rolled section is exactly the rollable families; what follows it
    // in a column is only an essence or a desecration adds.
    let rolled: BTreeSet<(AffixKind, String)> =
        picker.families.iter().filter(|f| f.section() == 0).map(|f| (f.kind, f.family.clone())).collect();
    assert_eq!(rolled, rollable);
    assert!(listed.len() > 10, "a body armour rolls many families, got {}", listed.len());
    // Prefixes, then suffixes.
    let first_suffix = listed.iter().position(|(k, _)| *k == AffixKind::Suffix).unwrap();
    assert!(listed[..first_suffix].iter().all(|(k, _)| *k == AffixKind::Prefix));
    assert!(listed[first_suffix..].iter().all(|(k, _)| *k == AffixKind::Suffix));
    // Labels name the stat with its numbers as "#".
    let life = picker.families.iter().find(|f| f.family == "IncreasedLife").unwrap();
    assert_eq!(life.label, "+# to maximum Life");
    assert!(picker.families.iter().all(|f| !f.label.chars().any(|c| c.is_ascii_digit())), "no number left in a label");

    // The item's own families are ticked at their tier, and only those.
    let p = picker_panel(&state);
    for (f, tick) in picker.families.iter().zip(&p.ticks) {
        let on_item = state.mods.iter().find(|m| m.family == f.family && m.kind == f.kind);
        assert_eq!(f.current, on_item.and_then(|m| m.tier), "{}", f.family);
        assert_eq!(*tick, on_item.and_then(|m| m.tier), "{}", f.family);
    }
    assert_eq!(p.ticks.iter().flatten().count(), 3);
    assert_eq!(on("IncreasedLife13").tier, Some(1));
    // The header shows the item's modifiers with their tier.
    assert_eq!(picker.title, "Rare Soldier Cuirass · item level 82");
    assert!(picker.mods.iter().any(|l| l.starts_with("P1  ")));
    assert!(picker.mods.iter().any(|l| l.starts_with("S") && l.contains("Fire Resistance")));

    // Laid out: a cell per family, prefixes in the left column and
    // suffixes in the right, a tier box on exactly the ticked ones, and the
    // Plan button live.
    let lay = craft_ui::layout(&p, &m);
    assert_eq!(lay.families.len(), picker.families.len());
    let x_of = |kind: AffixKind| {
        lay.families.iter().filter(|c| picker.families[c.index].kind == kind).map(|c| c.check.x).collect::<BTreeSet<_>>()
    };
    let (left, right) = (x_of(AffixKind::Prefix), x_of(AffixKind::Suffix));
    assert_eq!((left.len(), right.len()), (1, 1), "one column each");
    assert!(left.first() < right.first());
    for c in &lay.families {
        assert_eq!(c.ticked, p.ticks[c.index].is_some());
        assert_eq!(c.tier.is_some(), c.ticked, "{}", picker.families[c.index].family);
    }
    let life_cell = lay.families.iter().find(|c| picker.families[c.index].family == "IncreasedLife").unwrap();
    assert_eq!(life_cell.tier.as_ref().unwrap().text, "P1");
    let plan_button = lay.buttons.iter().find(|b| b.action == Action::Plan).unwrap();
    assert!(plan_button.on);
    let t = texts(&lay);
    for (i, title) in [(0, "Prefixes"), (1, "Suffixes")] {
        assert!(t.contains(&title), "{i}: {title}");
    }
    // No two cells overlap.
    for a in &lay.families {
        for b in lay.families.iter().filter(|b| b.index != a.index) {
            let apart = a.toggle.y + a.toggle.h as i32 <= b.toggle.y
                || b.toggle.y + b.toggle.h as i32 <= a.toggle.y
                || a.toggle.x + a.toggle.w as i32 <= b.toggle.x
                || b.toggle.x + b.toggle.w as i32 <= a.toggle.x;
            assert!(apart, "cells {} and {} overlap", a.index, b.index);
        }
    }
    // Unticking the life and ticking it again: the click on its row.
    let mut p = p;
    let (x, y) = centre(&life_cell.toggle);
    assert_eq!(click(&mut p, x, y), Some(Action::Tick(life_cell.index)));
    assert_eq!(p.ticks[life_cell.index], None);
    click(&mut p, x, y);
    assert_eq!(p.ticks[life_cell.index], Some(1), "the family on the item comes back at its tier");

    // At item level 10 the families whose every tier needs more are listed
    // greyed with the level they need, and cannot be ticked.
    let low = item(10, Rarity::Normal, vec![]);
    let lp = picker_panel(&low);
    let needs: HashMap<(AffixKind, String), u32> = data
        .entries()
        .iter()
        .filter(|e| e.domain == Domain::Item && !e.essence_only && e.eligible(tags.iter().copied()))
        .fold(HashMap::new(), |mut acc, e| {
            let level = acc.entry((e.kind, e.family.clone())).or_insert(u32::MAX);
            *level = (*level).min(e.required_level);
            acc
        });
    let greyed: Vec<&craft_ui::FamilyRow> = lp.picker.families.iter().filter(|f| f.unavailable.is_some()).collect();
    assert!(!greyed.is_empty(), "some family needs more than item level 10");
    // A desecrated family needs the level of its entries (65 in the data).
    for f in lp.picker.families.iter().filter(|f| f.section() == 2) {
        let lowest = f.tiers.iter().map(|t| t.required_level).min().unwrap();
        assert_eq!(f.unavailable.as_deref() == Some(format!("needs item level {lowest}").as_str()), lowest > 10, "{}", f.family);
    }
    for f in lp.picker.families.iter().filter(|f| f.section() == 0) {
        let lowest = needs[&(f.kind, f.family.clone())];
        assert_eq!(f.unavailable.is_some(), lowest > 10, "{}", f.family);
        if let Some(note) = &f.unavailable {
            assert_eq!(note, &format!("needs item level {lowest}"));
        }
    }
    let lay = craft_ui::layout(&lp, &m);
    let grey = lay.families.iter().find(|c| c.greyed).unwrap();
    let note = lp.picker.families[grey.index].unavailable.clone().unwrap();
    let drawn = lay.texts.iter().find(|t| t.text == note && t.rect.y == grey.toggle.y).expect("the note is drawn on its row");
    assert_eq!(drawn.ink, Ink::Dim);
    let label = lay.texts.iter().find(|t| t.rect.y == grey.toggle.y && t.rect.x == grey.check.x + 22).unwrap();
    assert_eq!(label.ink, Ink::Dim, "the greyed family's label is dim");
    let mut lp = lp;
    let (x, y) = centre(&grey.check);
    assert_eq!(click(&mut lp, x, y), None, "a greyed family does not tick");
    assert!(lp.ticks.iter().all(Option::is_none));
    let plan_button = lay.buttons.iter().find(|b| b.action == Action::Plan).unwrap();
    assert!(!plan_button.on, "nothing ticked, nothing to plan");
}

#[test]
fn a_min_tier_box_takes_trade_site_numbering() {
    let data = craft();
    // Item level 60: below what the top life and resistance tiers need.
    let state = item(60, Rarity::Normal, vec![]);
    let mut p = picker_panel(&state);
    let tags: Vec<&str> = state.base_tags.iter().map(String::as_str).collect();
    // The best tier item level 60 can roll and the worst tier, numbered as
    // the trade site does (1 is the best on the base), worked out here.
    let ladder = |family: &str| {
        let tiers: Vec<(u8, u32)> = data
            .entries()
            .iter()
            .filter(|e| e.domain == Domain::Item && !e.essence_only && e.family == family && e.eligible(tags.iter().copied()))
            .map(|e| (data.tier_on(e, &tags), e.required_level))
            .collect();
        let best = tiers.iter().filter(|(_, lvl)| *lvl <= 60).map(|(t, _)| *t).min().unwrap();
        let worst = tiers.iter().map(|(t, _)| *t).max().unwrap();
        (best, worst)
    };
    let (best, worst) = ladder("IncreasedLife");
    assert!(best > 1, "item level 60 cannot roll the top life tier on this base");
    let life = p.picker.families.iter().position(|f| f.family == "IncreasedLife").unwrap();
    assert_eq!((p.picker.families[life].best, p.picker.families[life].worst), (best, worst));

    let cell = |p: &Panel| craft_ui::layout(p, &m).families.into_iter().find(|c| c.index == life).unwrap();
    let (x, y) = centre(&cell(&p).toggle);
    assert_eq!(click(&mut p, x, y), Some(Action::Tick(life)));
    let tier = cell(&p).tier.expect("a ticked family has a tier box");
    assert_eq!(tier.text, format!("P{best}"), "a new family starts at the best tier the item level rolls");

    // The right arrow lets the minimum fall a tier; the target follows.
    let (x, y) = centre(&tier.worse);
    assert_eq!(click(&mut p, x, y), Some(Action::TierWorse(life)));
    assert_eq!(cell(&p).tier.unwrap().text, format!("P{}", best + 1));
    assert_eq!(p.target().wants, vec![Want { family: "IncreasedLife".into(), kind: AffixKind::Prefix, min_tier: best + 1 }]);
    // The left arrow raises it, never past what the item level can roll.
    let better = centre(&cell(&p).tier.unwrap().better);
    for _ in 0..3 {
        click(&mut p, better.0, better.1);
    }
    assert_eq!(cell(&p).tier.unwrap().text, format!("P{best}"));
    assert!(!craft_ui::apply(&mut p, &Action::TierBetter(life)), "no change at the best tier");
    // Nor below the family's worst tier.
    for _ in 0..(worst as usize + 3) {
        craft_ui::apply(&mut p, &Action::TierWorse(life));
    }
    assert_eq!(p.ticks[life], Some(worst));
    assert_eq!(cell(&p).tier.unwrap().text, format!("P{worst}"));

    // Suffixes are numbered S.
    let fire = p.picker.families.iter().position(|f| f.family == "FireResistance").unwrap();
    craft_ui::apply(&mut p, &Action::Tick(fire));
    let fire_cell = craft_ui::layout(&p, &m).families.into_iter().find(|c| c.index == fire).unwrap();
    assert_eq!(fire_cell.tier.unwrap().text, format!("S{}", ladder("FireResistance").0));
    // What the box says is what the planner is asked for.
    let wants = p.target().wants;
    assert_eq!(wants.len(), 2);
    assert_eq!(wants[1], Want { family: "FireResistance".into(), kind: AffixKind::Suffix, min_tier: ladder("FireResistance").0 });

    // Plan hands over to the caller: the plans view waits for its result.
    let plan_rect = craft_ui::layout(&p, &m).buttons.into_iter().find(|b| b.action == Action::Plan).unwrap().rect;
    let (x, y) = centre(&plan_rect);
    assert_eq!(click(&mut p, x, y), Some(Action::Plan));
    assert_eq!(p.view, View::Plans);
    assert_eq!(p.plan, craft_ui::PlanState::Planning);
    assert!(joined(&p).contains("Planning"));
}

#[test]
fn plans_rank_by_the_headline_model_and_expand_into_steps() {
    let plan = full_plan();
    assert!(plan.strategies.len() > PAGE_ROWS, "enough strategies to page, got {}", plan.strategies.len());
    let view = PlanView::build(plan, &RATES);
    // Rows in the plan's order: cheapest finished item first.
    let names: Vec<&str> = view.rows.iter().map(|r| r.name.as_str()).collect();
    let want: Vec<&str> = plan.strategies.iter().map(|r| r.costed.strategy.as_str()).collect();
    assert_eq!(names, want);
    let totals: Vec<f64> = plan.strategies.iter().filter_map(|r| r.costed.per_finished).collect();
    assert!(totals.windows(2).all(|w| w[0] <= w[1]), "{totals:?}");
    // Each row's figures are its own, in the price panel's units.
    for (row, r) in view.rows.iter().zip(&plan.strategies) {
        let c = &r.costed;
        let figures = row.figures.as_deref().expect("every plan here has a total");
        assert!(figures.starts_with(&format!("expected {}", RATES.money(c.per_finished.unwrap()))), "{figures}");
        assert!(figures.contains(" · half under ") && figures.contains(" · one in ten over ") && figures.contains(" · gives up "));
        assert_eq!(row.steps.len(), c.steps.len());
        assert_eq!(row.second, None, "no observed model, no second figure");
    }

    let mut p = plan_panel(plan);
    let lay = craft_ui::layout(&p, &m);
    assert_eq!(lay.rows.len(), PAGE_ROWS, "one page of rows");
    assert!(lay.rows.windows(2).all(|w| w[0].0.y + w[0].0.h as i32 <= w[1].0.y), "rows stack without overlap");
    // Opening the first row lists its steps with their figures, the
    // currency it uses, and how a run gives up.
    let (x, y) = centre(&lay.rows[0].0);
    assert_eq!(click(&mut p, x, y), Some(Action::ExpandRow(0)));
    let lay = craft_ui::layout(&p, &m);
    assert!(lay.rows[0].2, "the first row is open");
    let c = &plan.strategies[0].costed;
    let drawn = texts(&lay);
    for (step, line) in c.steps.iter().zip(&view.rows[0].steps) {
        assert!(line.starts_with(&step.name), "{line}");
        assert!(line.ends_with(&RATES.money(step.cost.unwrap())), "{line}");
        assert!(drawn.contains(&line.as_str()), "step not drawn: {line}");
    }
    let body = joined(&p);
    for (name, _) in c.currency() {
        assert!(body.contains(&name), "currency {name} not listed");
    }
    assert!(body.contains("currency: "));
    assert!(body.contains(&c.cap));
    // The steps sit inside the open row, and the next row is below them.
    let step_y = lay.texts.iter().find(|t| t.text == view.rows[0].steps[0]).unwrap().rect.y;
    assert!(step_y > lay.rows[0].0.y && step_y < lay.rows[0].0.y + lay.rows[0].0.h as i32);
    click(&mut p, x, y);
    assert_eq!(p.expanded, None, "a second click closes it");

    // The next page shows the next rows in rank order.
    let next = craft_ui::layout(&p, &m).buttons.into_iter().find(|b| b.action == Action::NextPage).unwrap();
    assert!(next.on);
    let (x, y) = centre(&next.rect);
    assert_eq!(click(&mut p, x, y), Some(Action::NextPage));
    let lay = craft_ui::layout(&p, &m);
    assert_eq!(lay.rows[0].1, PAGE_ROWS);
    assert!(texts(&lay).contains(&view.rows[PAGE_ROWS].name.as_str()));

    // Under an observed model the figures are the observed ones and the
    // row carries the sample and the uniform figure beside them.
    let observed = observed_plan();
    let ov = PlanView::build(observed, &RATES);
    assert!(observed.model_line.starts_with("observed on 412 listings of Body Armour"));
    for (row, r) in ov.rows.iter().zip(&observed.strategies) {
        let second = row.second.as_deref().expect("an observed headline has a uniform figure beside it");
        assert!(second.starts_with("observed on 412 listings of Body Armour · uniform model: "), "{second}");
        if let Some(u) = r.uniform.as_ref().and_then(|u| u.per_finished) {
            assert!(second.contains(&format!("expected {}", RATES.money(u))), "{second}");
        }
    }
    let op = plan_panel(observed);
    assert!(joined(&op).contains("uniform model: expected"));
}

#[test]
fn the_buy_line_sits_above_the_plans() {
    let plan = full_plan();
    let p = plan_panel(plan);
    let lay = craft_ui::layout(&p, &m);
    let buy = lay.texts.iter().find(|t| t.text.starts_with("Buy one instead: ")).expect("the buy line is drawn");
    assert_eq!(buy.text, format!("Buy one instead: cheapest {} (3 listings)", RATES.money(250.0)));
    let first_row = lay.rows.iter().map(|(r, _, _)| r.y).min().unwrap();
    assert!(buy.rect.y + buy.rect.h as i32 <= first_row, "the buy line is above every plan");
    // Rows dearer than buying carry the mark, and only those.
    let view = PlanView::build(plan, &RATES);
    for ((rect, i, _), r) in lay.rows.iter().zip(&plan.strategies) {
        let marked = lay.texts.iter().any(|t| {
            t.text == craft_ui::NOT_WORTH && t.rect.y >= rect.y && t.rect.y < rect.y + rect.h as i32
        });
        assert_eq!(marked, r.not_worth_crafting, "{}", view.rows[*i].name);
        if marked {
            let mark = lay.texts.iter().find(|t| t.text == craft_ui::NOT_WORTH && t.rect.y >= rect.y).unwrap();
            assert_eq!(mark.ink, Ink::Warn);
            assert!(mark.rect.x + mark.rect.w as i32 <= lay.w - 12, "the mark stays inside the panel");
        }
    }
    assert!(plan.strategies.iter().any(|r| r.not_worth_crafting), "the 250 ex quote is below some plan");

    // Without a trade search the line is still first, with its reason.
    let blind = observed_plan();
    let p = plan_panel(blind);
    let lay = craft_ui::layout(&p, &m);
    let buy = lay.texts.iter().find(|t| t.text.starts_with("Buy one instead: ")).unwrap();
    assert!(buy.text.starts_with("Buy one instead: unknown: "), "{}", buy.text);
    assert_eq!(buy.ink, Ink::Unknown);
    assert!(buy.rect.y < lay.rows.iter().map(|(r, _, _)| r.y).min().unwrap());
    assert!(!lay.texts.iter().any(|t| t.text == craft_ui::NOT_WORTH), "nothing marked against a price nobody has");
}

// ---------------------------------------------------------------- L2

#[test]
fn the_model_line_and_assumptions_are_always_shown() {
    let plan = full_plan();
    assert!(plan.model_line.contains(UNIFORM_LABEL));
    assert!(!plan.assumptions.is_empty());
    let mut p = plan_panel(plan);
    let body = joined(&p);
    assert!(body.contains(&format!("rulebook {PATCH}")), "the patch pin");
    assert!(body.contains(&plan.model_line), "the model line");
    assert!(body.contains("Assumptions of the cheapest plan"));
    for a in &plan.assumptions {
        assert!(body.contains(a.as_str()), "assumption not shown: {a}");
    }
    // Opening a row shows that plan's assumptions instead.
    let last = PAGE_ROWS - 1;
    craft_ui::apply(&mut p, &Action::ExpandRow(last));
    let body = joined(&p);
    let r = &plan.strategies[last].costed;
    assert!(body.contains(&format!("Assumptions of {}", r.strategy)));
    for a in &r.assumptions {
        assert!(body.contains(a.as_str()), "assumption not shown: {a}");
    }
    assert!(body.contains(&plan.model_line));

    // The picker, the waiting plans view and the flip list show the pin and
    // the model line too.
    let picker = picker_panel(&item(80, Rarity::Normal, vec![]));
    let mut waiting = picker.clone();
    waiting.view = View::Plans;
    let flips = flip_panel();
    for (name, panel) in [("picker", &picker), ("waiting", &waiting), ("flips", &flips)] {
        let body = joined(panel);
        assert!(body.contains(&format!("rulebook {PATCH}")), "{name}: the patch pin");
        assert!(body.contains(&model_line(None)), "{name}: the model line");
    }
    // The flip list's header states the scan's request cost.
    assert!(joined(&flips).contains("this scan: 5 searches, 10 fetches; budget 27/30 free"));
}

#[test]
fn an_unknown_step_shows_its_reason_in_place_of_a_figure() {
    let plan = lacking_plan();
    let reason = format!("unknown: no price for {LEFT_OUT}");
    let idx = plan.strategies.iter().position(|r| r.costed.per_finished.is_none()).expect("a plan that slams with the omen");
    let c = &plan.strategies[idx].costed;
    assert!(c.unknowns.contains(&reason));
    let view = PlanView::build(plan, &RATES);
    let row = &view.rows[idx];
    assert_eq!(row.figures, None, "no total, so no figures");
    assert!(row.unknowns.contains(&reason));

    let mut p = plan_panel(plan);
    // Page to the row and open it.
    for _ in 0..idx / PAGE_ROWS {
        craft_ui::apply(&mut p, &Action::NextPage);
    }
    craft_ui::apply(&mut p, &Action::ExpandRow(idx));
    let lay = craft_ui::layout(&p, &m);
    let (rect, _, open) = lay.rows.iter().find(|(_, i, _)| *i == idx).unwrap();
    assert!(open);
    let inside: Vec<&craft_ui::Text> =
        lay.texts.iter().filter(|t| t.rect.y >= rect.y && t.rect.y < rect.y + rect.h as i32).collect();
    assert!(!inside.iter().any(|t| t.text.starts_with("expected ")), "no figure line for a plan without a total");
    let reason_line = inside.iter().find(|t| t.text == reason).expect("the reason is drawn in the row");
    assert_eq!(reason_line.ink, Ink::Unknown);
    // Its known steps keep their figures; the step needing the omen shows
    // the reason where its cost would be.
    for (step, line) in c.steps.iter().zip(&row.steps) {
        assert!(inside.iter().any(|t| t.text == *line), "step not drawn: {line}");
        match step.cost {
            Some(cost) => assert!(line.ends_with(&RATES.money(cost)), "{line}"),
            None => {
                assert!(step.name.contains(LEFT_OUT));
                assert!(line.ends_with(&reason), "{line}");
                for unit in [" ex", " div", " chaos"] {
                    assert!(!line.contains(unit), "a figure for an unpriced step: {line}");
                }
                assert_eq!(inside.iter().find(|t| t.text == *line).unwrap().ink, Ink::Unknown);
            }
        }
    }
    assert!(c.steps.iter().any(|s| s.cost.is_some()) && c.steps.iter().any(|s| s.cost.is_none()));

    // In the flip list, a candidate whose fix has no cost says so, has no
    // margin, and is listed after every candidate with one.
    let flips = flip_panel();
    let list = flips.flips.as_ref().unwrap();
    let sellers: Vec<&str> = list.rows.iter().map(|r| r.seller.as_str()).collect();
    assert_eq!(sellers, vec!["First", "Second", "Unfixable", "Unsold"], "ranked by margin, unknowns last");
    let (first, second) = list.rows[0].lines(&RATES);
    assert_eq!(first, format!("listed 1 div by First (online) · fix ~{} (orb-chain, half under 2.5)", RATES.money(0.5 * 459.0)));
    assert_eq!(second, "resale cheapest 6 div (3 listings) · margin 4.5 div under uniform");
    let (_, observed) = list.rows[1].lines(&RATES);
    assert_eq!(observed, "resale cheapest 6 div (3 listings) · margin 3.1 div under uniform · 2.2 div observed on 412 listings of Body Armour");
    let (unfixable, resale) = list.rows[2].lines(&RATES);
    assert!(unfixable.ends_with(&format!("fix cost unknown: no price for {LEFT_OUT}")), "{unfixable}");
    assert!(!resale.contains("margin"), "{resale}");
    let (_, unsold) = list.rows[3].lines(&RATES);
    assert_eq!(unsold, "resale unknown: no listing at or above the profile");
    let lay = craft_ui::layout(&flips, &m);
    let drawn = lay.texts.iter().find(|t| t.text == unfixable).unwrap();
    assert_eq!(drawn.ink, Ink::Unknown);

    // Clicking a candidate opens its card, its plan and its listings, and
    // "Open site" hands its listing to the caller.
    let mut flips = flips;
    let (x, y) = centre(&lay.rows[0].0);
    assert_eq!(click(&mut flips, x, y), Some(Action::SelectCandidate(0)));
    let lay = craft_ui::layout(&flips, &m);
    let body = texts(&lay);
    assert!(body.contains(&"P2  +196 to maximum Life") && body.contains(&"its plan") && body.contains(&"6 div · online"));
    let site = lay.buttons.iter().find(|b| b.label == craft_ui::OPEN_SITE).unwrap();
    let (x, y) = centre(&site.rect);
    assert_eq!(
        click(&mut flips, x, y),
        Some(Action::OpenSite("https://www.pathofexile.com/trade2/search/poe2/Standard/First".into()))
    );
    assert_eq!(flips.selected, Some(0), "opening the site leaves the candidate open");
}

/// Every view the panel has, with rows open where rows can open.
fn every_view() -> Vec<(&'static str, Panel)> {
    let rare = item(82, Rarity::Rare, vec![on("IncreasedLife13"), on("FireResist7"), on("Strength2")]);
    let low = item(10, Rarity::Magic, vec![on("Strength2")]);
    let mut out = vec![("picker", picker_panel(&rare)), ("low picker", picker_panel(&low))];
    let mut waiting = picker_panel(&rare);
    craft_ui::apply(&mut waiting, &Action::Plan);
    out.push(("planning", waiting));
    let mut empty = picker_panel(&rare);
    empty.view = View::Plans;
    out.push(("no plan", empty));
    for (name, plan) in [("plan", full_plan()), ("lacking", lacking_plan()), ("observed", observed_plan())] {
        let p = plan_panel(plan);
        for i in 0..p.pages() {
            let mut page = p.clone();
            for _ in 0..i {
                craft_ui::apply(&mut page, &Action::NextPage);
            }
            let first = i * PAGE_ROWS;
            craft_ui::apply(&mut page, &Action::ExpandRow(first));
            out.push((name, page));
        }
    }
    let mut flips = flip_panel();
    out.push(("flips", flips.clone()));
    for i in 0..4 {
        craft_ui::apply(&mut flips, &Action::SelectCandidate(i));
        out.push(("flip open", flips.clone()));
    }
    let mut none = picker_panel(&rare);
    none.view = View::Flips;
    out.push(("no flips", none));
    out
}

#[test]
fn every_craft_label_can_be_drawn_by_the_overlay_font() {
    let r = Renderer::new().unwrap();
    let measure = |s: &str| r.evaluate_label_width(s);
    let mut count = 0;
    for (name, p) in every_view() {
        let text = craft_ui::all_text(&p, &measure);
        for s in &text {
            assert!(r.can_draw(s), "{name}: not drawable by the overlay font: {s:?}");
        }
        count += text.len();
        // And every line fits inside the panel at the overlay's own metrics.
        let lay = craft_ui::layout(&p, &measure);
        for t in &lay.texts {
            let w = match t.style {
                Style::Small => (measure(&t.text) as f32 * craft_ui::SMALL_RATIO).ceil() as i32,
                _ => measure(&t.text),
            };
            assert!(t.rect.x + w <= lay.w - 12 + 1, "{name}: runs past the edge: {:?}", t.text);
        }
    }
    assert!(count > 400, "the views draw many strings, got {count}");
    // The characters the panel leans on.
    for s in ["~", "·", "…", "#", "%", "<", ">", "+", "/"] {
        assert!(r.can_draw(s), "{s}");
    }
}

#[test]
fn no_plan_label_says_estimate_or_prediction() {
    let mut count = 0;
    for (name, p) in every_view() {
        for s in craft_ui::all_text(&p, &m) {
            let lower = s.to_lowercase();
            assert!(!lower.contains("estimat") && !lower.contains("predict"), "{name}: {s}");
            count += 1;
        }
    }
    assert!(count > 400);
}

#[test]
fn a_family_sharing_a_group_with_a_ticked_one_cannot_be_ticked() {
    // Bleeding, Ignite and Poison duration share one mod group on body
    // armour: an item holds one of them, so wanting two is impossible.
    let mut p = picker_panel(&item(80, Rarity::Normal, Vec::new()));
    let at = |p: &Panel, family: &str| p.picker.families.iter().position(|f| f.family == family).unwrap_or_else(|| panic!("{family} is in the picker"));
    let (bleed, poison) = (at(&p, "ReducedBleedDuration"), at(&p, "ReducedPoisonDuration"));
    craft_ui::apply(&mut p, &Action::Tick(bleed));
    assert!(p.ticks[bleed].is_some());
    assert!(p.group_taken(poison));
    craft_ui::apply(&mut p, &Action::Tick(poison));
    assert!(p.ticks[poison].is_none(), "the second family of the group stays unticked");
    let lay = craft_ui::layout(&p, &m);
    assert!(texts(&lay).contains(&craft_ui::SAME_GROUP_NOTE), "the reason is drawn beside it");
    // Unticking the first frees the group.
    craft_ui::apply(&mut p, &Action::Tick(bleed));
    craft_ui::apply(&mut p, &Action::Tick(poison));
    assert!(p.ticks[poison].is_some());
}

#[test]
fn the_cheapest_line_claims_only_what_the_totals_show() {
    for plan in [full_plan(), lacking_plan(), observed_plan()] {
        let view = PlanView::build(plan, &RATES);
        let costed = plan.strategies.iter().filter(|r| r.costed.per_finished.is_some()).count();
        let line = view.cheapest.clone().unwrap_or_default();
        if costed == 0 {
            // Nothing shows buying is cheaper than a craft without a total.
            assert_eq!(line, "no plan has a total, so the cheapest way is unknown");
        } else if costed < plan.strategies.len() {
            assert!(line.starts_with("cheapest of the costed ways to the finished item: "), "{line}");
        } else {
            assert!(line.starts_with("cheapest way to the finished item: "), "{line}");
        }
    }
}

#[test]
fn the_picker_offers_desecrated_and_essence_modifiers_with_each_tiers_rolls() {
    let state = item(80, Rarity::Normal, Vec::new());
    let picker = Picker::build(craft(), &state);
    use craft_ui::Origin;
    // A desecrated family a bone can reveal on body armour, with its lich.
    let desecrated = picker
        .families
        .iter()
        .find(|f| matches!(f.origin, Origin::Desecrated(Some(_))))
        .expect("body armour has desecrated families");
    assert!(desecrated.shown().contains(": "), "the lich leads the label: {}", desecrated.shown());
    // A modifier only an essence adds, named by the essence on its tier.
    let only = picker.families.iter().find(|f| f.origin == Origin::Essence).expect("an essence-only modifier on body armour");
    assert!(only.tiers.iter().all(|t| t.essence.is_some()), "every tier of an essence-only family names its essence");
    // A rolled family an essence can also guarantee says so.
    let life = picker.families.iter().find(|f| f.family == "IncreasedLife").unwrap();
    assert_eq!(life.origin, Origin::Rolled { essence: true, alloy: false });
    assert!(life.shown().ends_with(" · essence"));
    // Every tier states what it rolls and the item level it needs, best first.
    assert!(life.tiers.windows(2).all(|w| w[0].tier < w[1].tier));
    let detail = life.tier_detail(life.best);
    assert!(detail.contains(" · ilvl "), "{detail}");
    assert!(detail.chars().next().unwrap().is_ascii_digit(), "the rolls lead: {detail}");

    // Ticked, the desecrated family draws its tier's rolls under the row
    // and plans through a desecration.
    let mut p = picker_panel(&item(80, Rarity::Rare, Vec::new()));
    let i = p.picker.families.iter().position(|f| f.family == desecrated.family && f.kind == desecrated.kind).unwrap();
    craft_ui::apply(&mut p, &Action::Tick(i));
    let t = p.ticks[i].expect("a desecrated family ticks");
    let lay = craft_ui::layout(&p, &m);
    let line = format!("{}: {}", craft_ui::tier_text(desecrated.kind, t), desecrated.tier_detail(t));
    assert!(texts(&lay).iter().any(|s| *s == line), "the detail line is drawn: {line}");
    assert!(texts(&lay).contains(&craft_ui::DESECRATED_SECTION));
    let target = p.target();
    let plan = plan(&p_state(&p), &target, craft(), None, &|n: &str| prices().get(n).copied(), None, &SimConfig { runs: 200, seed: 3, ..SimConfig::default() });
    assert!(plan.strategies.iter().any(|r| r.costed.id == "desecrate"), "a desecrate plan: {:?}", plan.strategies.iter().map(|r| r.costed.strategy.clone()).collect::<Vec<_>>());
}

#[test]
fn the_picker_offers_alloy_modifiers_in_their_own_section() {
    use craft_ui::Origin;
    let state = item(80, Rarity::Rare, Vec::new());
    let picker = Picker::build(craft(), &state);
    // Body armour takes three Alloys' modifiers no orb rolls (Sovereign,
    // Expansive and Cyclonic): each its own row, every tier naming its Alloy.
    let alloy_rows: Vec<&craft_ui::FamilyRow> = picker.families.iter().filter(|f| f.origin == Origin::Alloy).collect();
    let mut named: Vec<&str> = alloy_rows.iter().flat_map(|f| f.tiers.iter().filter_map(|t| t.alloy.as_deref())).collect();
    named.sort_unstable();
    named.dedup();
    assert_eq!(named, vec!["Cyclonic Alloy", "Expansive Alloy", "Sovereign Alloy"]);
    let presence = alloy_rows.iter().find(|f| f.label == "#% increased Presence Area of Effect").expect("Expansive Alloy's row");
    assert_eq!(presence.kind, AffixKind::Suffix);
    assert_eq!(presence.shown(), "#% increased Presence Area of Effect");
    assert_eq!(presence.tier_detail(presence.best), "35-50 · ilvl 25 · Expansive Alloy");
    // In each column the alloy rows follow the essence-only ones and come
    // before the desecrated ones.
    for kind in [AffixKind::Prefix, AffixKind::Suffix] {
        let places: Vec<u8> = picker.families.iter().filter(|f| f.kind == kind).map(|f| f.place()).collect();
        assert!(places.windows(2).all(|w| w[0] <= w[1]), "{kind:?}: {places:?}");
    }
    // A rolled family an Alloy also guarantees carries the marker, beside
    // the essence one when both do.
    let rolled = picker.families.iter().find(|f| f.origin == Origin::Rolled { essence: false, alloy: false }).unwrap();
    let marked = craft_ui::FamilyRow { origin: Origin::Rolled { essence: false, alloy: true }, ..rolled.clone() };
    assert_eq!(marked.shown(), format!("{} · alloy", rolled.label));
    let both = craft_ui::FamilyRow { origin: Origin::Rolled { essence: true, alloy: true }, ..rolled.clone() };
    assert_eq!(both.shown(), format!("{} · essence · alloy", rolled.label));

    // Ticked, the alloy row draws under its own heading with the Alloy on
    // its tier line, and the target wants it.
    let mut p = picker_panel(&state);
    let at = |p: &Panel, label: &str| p.picker.families.iter().position(|f| f.origin == Origin::Alloy && f.label == label).unwrap();
    let i = at(&p, "#% increased Presence Area of Effect");
    craft_ui::apply(&mut p, &Action::Tick(i));
    let t = p.ticks[i].expect("an alloy family ticks");
    let lay = craft_ui::layout(&p, &m);
    assert!(texts(&lay).contains(&craft_ui::ALLOY_SECTION));
    let line = format!("{}: 35-50 · ilvl 25 · Expansive Alloy", craft_ui::tier_text(AffixKind::Suffix, t));
    assert!(texts(&lay).iter().any(|s| *s == line), "the tier line names the Alloy: {line}");
    assert!(p.target().wants.iter().any(|w| w.family == presence.family && w.kind == AffixKind::Suffix));

    // Both kinds take the one crafted slot: a second alloy-only tick and an
    // essence-only tick are refused with the crafted-slot note.
    let other = at(&p, "#% increased Runic Ward");
    assert_eq!(p.blocked(other), Some(craft_ui::ONE_CRAFTED_NOTE));
    craft_ui::apply(&mut p, &Action::Tick(other));
    assert!(p.ticks[other].is_none(), "one crafted modifier per item");
    let essence_only = p.picker.families.iter().position(|f| f.origin == Origin::Essence && f.unavailable.is_none()).expect("an essence-only row");
    assert_eq!(p.blocked(essence_only), Some(craft_ui::ONE_CRAFTED_NOTE));
    assert!(texts(&craft_ui::layout(&p, &m)).contains(&craft_ui::ONE_CRAFTED_NOTE));
    // And the other way round: an essence-only tick holds every alloy row.
    craft_ui::apply(&mut p, &Action::Tick(i));
    assert!(p.ticks[i].is_none());
    craft_ui::apply(&mut p, &Action::Tick(essence_only));
    assert!(p.ticks[essence_only].is_some());
    assert_eq!(p.blocked(i), Some(craft_ui::ONE_CRAFTED_NOTE));
    // A rolled family is held back by neither.
    let rolled_at = p.picker.families.iter().position(|f| f.section() == 0 && !f.groups.iter().any(|g| p.picker.families[essence_only].groups.contains(g))).unwrap();
    assert_eq!(p.blocked(rolled_at), None);
}

fn p_state(_p: &Panel) -> ItemState {
    item(80, Rarity::Rare, Vec::new())
}

#[test]
fn an_absent_amulet_offers_the_kurgal_skill_quality_desecration_and_its_essences() {
    let base = craft().base("Absent Amulet").expect("Absent Amulet is in the base sample");
    let state = ItemState {
        class: base.class.clone(),
        base: base.name.clone(),
        base_tags: base.tags.clone(),
        item_level: 82,
        rarity: Rarity::Rare,
        corrupted: false,
        mirrored: false,
        sanctified: false,
        mods: Vec::new(),
        sockets: 0,
    };
    let picker = Picker::build(craft(), &state);
    let quality = picker
        .families
        .iter()
        .find(|f| f.label == "+#% to Quality of all Skills")
        .unwrap_or_else(|| panic!("no skill quality row: {:?}", picker.families.iter().map(|f| f.shown()).collect::<Vec<_>>()));
    assert_eq!(quality.origin, craft_ui::Origin::Desecrated(Some(khaloni_poe2_core::craft::types::Lich::Kurgal)));
    assert_eq!(quality.shown(), "Kurgal: +#% to Quality of all Skills");
    assert_eq!(quality.kind, AffixKind::Suffix);
    assert!(quality.unavailable.is_none(), "item level 82 reaches it");
    assert!(quality.tier_detail(quality.best).starts_with("3-5 · ilvl 65"), "{}", quality.tier_detail(quality.best));
    // Essences name jewellery modifiers on an amulet.
    let essence_rows: Vec<String> =
        picker.families.iter().filter(|f| f.tiers.iter().any(|t| t.essence.is_some())).map(|f| f.shown()).collect();
    assert!(!essence_rows.is_empty(), "essences guarantee something on an amulet");
}

#[test]
fn only_one_desecrated_and_one_essence_only_modifier_can_be_ticked() {
    let base = craft().base("Absent Amulet").unwrap();
    let state = ItemState {
        class: base.class.clone(),
        base: base.name.clone(),
        base_tags: base.tags.clone(),
        item_level: 82,
        rarity: Rarity::Rare,
        corrupted: false,
        mirrored: false,
        sanctified: false,
        mods: vec![ModOn {
            entry_id: None,
            family: "Desecrated Suffix".into(),
            groups: Vec::new(),
            kind: AffixKind::Suffix,
            tier: None,
            required_level: None,
            adds_tags: Vec::new(),
            source: Source::Desecrated,
            text: "Desecrated Suffix".into(),
        }],
        sockets: 0,
    };
    let mut p = picker_panel(&state);
    // The unrevealed suffix reads as what it is.
    assert!(p.picker.mods.iter().any(|l| l.starts_with("unrevealed desecrated suffix")), "{:?}", p.picker.mods);
    let in_section = |p: &Panel, section: u8| -> Vec<usize> {
        p.picker.families.iter().enumerate().filter(|(_, f)| f.section() == section && f.unavailable.is_none()).map(|(i, _)| i).collect()
    };
    // Two desecrated families that share no group: the second is refused.
    let desecrated = in_section(&p, 2);
    let first = desecrated[0];
    let second = *desecrated
        .iter()
        .find(|&&j| !p.picker.families[j].groups.iter().any(|g| p.picker.families[first].groups.contains(g)))
        .expect("two desecrated families of different groups");
    craft_ui::apply(&mut p, &Action::Tick(first));
    assert!(p.ticks[first].is_some());
    assert_eq!(p.blocked(second), Some(craft_ui::ONE_DESECRATED_NOTE));
    craft_ui::apply(&mut p, &Action::Tick(second));
    assert!(p.ticks[second].is_none(), "one desecrated modifier per item");
    assert!(texts(&craft_ui::layout(&p, &m)).contains(&craft_ui::ONE_DESECRATED_NOTE));
    // A rolled family is not held back by the desecrated tick.
    let rolled = in_section(&p, 0)[0];
    assert_eq!(p.blocked(rolled), None);
    // The same for the essence-only section, when the base has two.
    let only = in_section(&p, 1);
    if only.len() >= 2 {
        craft_ui::apply(&mut p, &Action::Tick(only[0]));
        assert_eq!(p.blocked(only[1]), Some(craft_ui::ONE_CRAFTED_NOTE));
    }
}

#[test]
fn an_opened_plan_lists_its_omens_with_their_odds() {
    let plan = full_plan();
    let (i, row) = plan.strategies.iter().enumerate().find(|(_, r)| !r.omens.is_empty()).expect("a plan that uses an omen");
    let mut p = plan_panel(plan);
    for _ in 0..i / PAGE_ROWS {
        craft_ui::apply(&mut p, &Action::NextPage);
    }
    // Closed, the row names its omens; opened, each gets its line.
    let closed = texts(&craft_ui::layout(&p, &m)).join("\n");
    assert!(closed.contains("omens: "), "{closed}");
    craft_ui::apply(&mut p, &Action::ExpandRow(i));
    let opened = texts(&craft_ui::layout(&p, &m)).join("\n");
    assert!(opened.contains(craft_ui::OMENS_HEADING));
    for o in &row.omens {
        let head: String = o.chars().take(40).collect();
        assert!(opened.contains(&head), "{o} is drawn");
    }
}
