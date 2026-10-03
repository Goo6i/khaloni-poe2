//! The craft planner's wiring, driven through the real pipeline over
//! fixtures: a copied item becomes the planner panel, a ticked target
//! becomes a plan beside the price of buying one, a calibration feeds the
//! observed store and the next plan heads with it, and a flip profile
//! becomes ranked candidates. Every trade search goes through an injected
//! fetch that answers from the real fetch fixture; nothing leaves the
//! machine. Calibrations and scans state their cost and are refused by the
//! budget before anything is sent.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use khaloni_poe2::craft_flow::{
    self, allow, calibrate, calibration_bands, calibration_cost, calibration_search, calibration_statement, open_panel, plan_item,
    read_item, run_scan, scan_plan, scan_statement, window_from, Opened, PlanInputs, ScanInputs, SearchWindow,
    USER_RESERVE,
};
use khaloni_poe2::craft_ui::{self, Action, Ask, FlipRow, Panel, PlanState, PlanView, Prompt, Rates, View};
use khaloni_poe2::observed_store::ObservedStore;
use khaloni_poe2_core::craft::data::CraftData;
use khaloni_poe2_core::craft::observed::MAX_LISTINGS;
use khaloni_poe2_core::craft::sim::SimConfig;
use khaloni_poe2_core::craft::strategy::Target;
use khaloni_poe2_core::ee2::{data, Ee2Data};
use khaloni_poe2_core::flip::{Market, Planner, Profile, RequestCost, Wanted, DEFAULT_MARGIN};
use khaloni_poe2_core::ninja::PriceTable;
use khaloni_poe2_core::trade::{Query, StatIndex};
use serde_json::Value;

const LEAGUE: &str = "Forbidden Rites";
const RUNS: usize = 300;

/// Fixed-advance stand-in for the glyph measurer.
fn m(s: &str) -> i32 {
    7 * s.chars().count() as i32
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../core/tests/fixtures")
}

fn ee2_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../tools/ee2-parity/data")
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

/// EE2's data the way the craft worker loads it: the pinned ndjson with
/// the trade catalogs it matches lines against.
fn ee2() -> &'static Ee2Data {
    static DATA: OnceLock<Ee2Data> = OnceLock::new();
    DATA.get_or_init(|| {
        let dir = ee2_dir();
        let mut d = Ee2Data::from_ndjson(&read(&dir.join("stats.ndjson")), &read(&dir.join("items.ndjson")))
            .expect("pinned ndjson parses");
        d.trade_stats = Some(data::TradeStatTexts::from_json(&read(&dir.join("trade-stats.json"))).expect("trade stats"));
        d.trade_items = Some(data::trade_item_names(&read(&dir.join("trade-items.json"))).expect("trade items"));
        d
    })
}

fn catalog() -> &'static StatIndex {
    static INDEX: OnceLock<StatIndex> = OnceLock::new();
    INDEX.get_or_init(|| StatIndex::from_json(&read(&ee2_dir().join("trade-stats.json"))).expect("the trade catalog parses"))
}

/// The poe.ninja fixture: divine, chaos, exalted and the Orb of Annulment.
fn table() -> &'static PriceTable {
    static TABLE: OnceLock<PriceTable> = OnceLock::new();
    TABLE.get_or_init(|| {
        let ov: khaloni_poe2_core::ninja::ExchangeOverview =
            serde_json::from_str(&read(&fixtures().join("ninja_currency.json"))).unwrap();
        PriceTable::build(&[ov])
    })
}

/// Exchange answers the overlay already holds for what the table lacks.
fn exchange() -> HashMap<String, f64> {
    let mut held: HashMap<String, f64> = [
        ("Orb of Transmutation", 0.05),
        ("Greater Orb of Transmutation", 0.6),
        ("Perfect Orb of Transmutation", 4.0),
        ("Orb of Augmentation", 0.08),
        ("Greater Orb of Augmentation", 0.7),
        ("Perfect Orb of Augmentation", 4.5),
        ("Regal Orb", 0.4),
        ("Greater Regal Orb", 2.0),
        ("Perfect Regal Orb", 9.0),
        ("Greater Exalted Orb", 3.5),
        ("Perfect Exalted Orb", 16.0),
        ("Greater Chaos Orb", 4.0),
        ("Perfect Chaos Orb", 18.0),
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
    for of in khaloni_poe2_core::craft::rules::TIERED_ESSENCES {
        held.insert(format!("Lesser Essence of {of}"), 0.2);
        held.insert(format!("Essence of {of}"), 0.8);
        held.insert(format!("Greater Essence of {of}"), 3.0);
        held.insert(format!("Perfect Essence of {of}"), 45.0);
    }
    held
}

/// The trade site's currency ids the fixture's listings are priced in.
fn names() -> HashMap<String, String> {
    HashMap::from([
        ("exalted".to_string(), "Exalted Orb".to_string()),
        ("divine".to_string(), "Divine Orb".to_string()),
        ("chaos".to_string(), "Chaos Orb".to_string()),
    ])
}

fn rates() -> Rates {
    craft_flow::rates(table(), 1.0)
}

fn small() -> SimConfig {
    SimConfig { runs: RUNS, ..SimConfig::default() }
}

/// The real fetch response: body armours, mostly priced in divines.
fn fetched() -> Vec<Option<Value>> {
    let body: Value = serde_json::from_str(&read(&fixtures().join("trade_fetch_full.json"))).unwrap();
    body["result"].as_array().unwrap().iter().map(|v| (!v.is_null()).then(|| v.clone())).collect()
}

/// A fetch that answers every search from the fixture, recording what was
/// asked; `distinct` gives each search's listings their own ids, as five
/// different searches would.
struct FixtureFetch {
    asked: RefCell<Vec<(Query, usize)>>,
    distinct: bool,
}

impl FixtureFetch {
    fn new(distinct: bool) -> FixtureFetch {
        FixtureFetch { asked: RefCell::new(Vec::new()), distinct }
    }

    fn call(&self, q: &Query, listings: usize) -> Result<Vec<Option<Value>>, String> {
        let n = self.asked.borrow().len();
        self.asked.borrow_mut().push((q.clone(), listings));
        Ok(fetched()
            .into_iter()
            .take(listings)
            .map(|e| {
                e.map(|mut v| {
                    if self.distinct {
                        let id = format!("{}-{n}", v["id"].as_str().unwrap_or(""));
                        v["id"] = Value::from(id);
                    }
                    v
                })
            })
            .collect())
    }
}

fn never(_: &Query, _: usize) -> Result<Vec<Option<Value>>, String> {
    panic!("a refused request sent a search")
}

fn window(used: u32) -> SearchWindow {
    SearchWindow { used, max: 30, seen: true }
}

fn the_item() -> Opened {
    read_item(&read(&fixtures().join("craft/rare-soldier-cuirass.txt")), ee2(), craft()).expect("the rare reads")
}

fn texts(p: &Panel) -> Vec<String> {
    craft_ui::all_text(p, &m)
}

fn joined(p: &Panel) -> String {
    texts(p).join("\n")
}

fn button<'a>(lay: &'a craft_ui::Layout, label: &str) -> Option<&'a craft_ui::Button> {
    lay.buttons.iter().find(|b| b.label == label)
}

fn centre(r: &khaloni_poe2::config::Rect) -> (i32, i32) {
    (r.x + r.w as i32 / 2, r.y + r.h as i32 / 2)
}

/// Clicks the button labelled `label`, carrying the action out as the
/// panel does; the caller's actions come back.
fn press(p: &mut Panel, label: &str) -> Option<Action> {
    let lay = craft_ui::layout(p, &m);
    let b = button(&lay, label).unwrap_or_else(|| panic!("no {label} button in {:?}", texts(p)));
    let (x, y) = centre(&b.rect);
    let action = craft_ui::hit(&lay, x, y)?;
    craft_ui::apply(p, &action);
    Some(action)
}

/// The family of the item's armour modifier ("+# to Armour").
fn armour_family(opened: &Opened) -> String {
    opened
        .state
        .mods
        .iter()
        .find(|m| m.entry_id.as_deref().is_some_and(|id| id.starts_with("LocalIncreasedPhysicalDamageReductionRating")))
        .map(|m| m.family.clone())
        .expect("the item has increased armour")
}

/// Ticks the item's panel to armour at P3 and fire at S3 or better (the
/// item has them at P4 and S5), through the picker's own controls; every
/// other family unticked.
fn tick_armour_and_fire(p: &mut Panel, armour: &str) {
    let families = p.picker.families.clone();
    for (i, f) in families.iter().enumerate() {
        let want = (f.family == armour || f.family == "FireResistance").then_some(3);
        match (p.ticks[i], want) {
            (Some(_), None) | (None, Some(_)) => {
                craft_ui::apply(p, &Action::Tick(i));
            }
            _ => {}
        }
        if let Some(t) = want {
            while p.ticks[i].is_some_and(|x| x > t) {
                assert!(craft_ui::apply(p, &Action::TierBetter(i)), "{} cannot reach tier {t}", f.family);
            }
        }
    }
}

/// The price of buying one from the fixture's listings, through the real
/// buy search and resale reading.
fn buy_from_fixture(opened: &Opened, target: &Target, fetch: &FixtureFetch) -> Result<khaloni_poe2_core::craft::plan::BuyQuote, String> {
    let (resolved, query) = craft_flow::buy_search(&opened.state, target, craft(), &ee2().stats, catalog())?;
    let raw = fetch.call(&query, 20)?;
    let entries: Vec<Value> = raw.into_iter().flatten().collect();
    let names = names();
    let convert = |amount: f64, currency: &str| craft_flow::listing_exalted(amount, currency, &names, table());
    craft_flow::buy_quote(&entries, &resolved, craft(), &Market { convert: &convert, unit: "ex" })
}

fn cuirass() -> Profile {
    Profile {
        name: "Armour and fire cuirass".to_string(),
        class: "Body Armour".to_string(),
        base: None,
        min_ilvl: 75,
        wants: vec![
            Wanted { family: "LocalPhysicalDamageReductionRatingPercent".to_string(), min_tier: 2 },
            Wanted { family: "FireResistance".to_string(), min_tier: 3 },
        ],
        margin: DEFAULT_MARGIN,
    }
}

// ------------------------------------------------------------------ opening

#[test]
fn the_planner_opens_from_a_hovered_item_state() {
    let opened = the_item();
    assert_eq!(opened.state.class, "Body Armour");
    assert_eq!(opened.state.base, "Soldier Cuirass");
    assert_eq!(opened.state.item_level, 82);
    assert_eq!(opened.picker.title, "Rare Soldier Cuirass · item level 82");
    assert_eq!(opened.picker.class, "Body Armour");

    let p = open_panel(&opened, rates(), "prices: poe.ninja, Forbidden Rites".into(), "observed model: 0 of 200 listings of Body Armour recorded; Calibrate gathers more".into());
    assert_eq!(p.view, View::Picker);
    // Every modifier the item has is ticked at the tier it has.
    let ticked: Vec<(String, u8)> = p
        .picker
        .families
        .iter()
        .zip(&p.ticks)
        .filter_map(|(f, t)| t.map(|t| (f.family.clone(), t)))
        .collect();
    assert_eq!(ticked.len(), 5, "{ticked:?}");
    assert!(ticked.contains(&("IncreasedLife".to_string(), 6)), "{ticked:?}");
    assert!(ticked.contains(&("FireResistance".to_string(), 5)), "{ticked:?}");
    assert_eq!(p.target().wants.len(), 5);
    // The item's lines, the observed sample and the Calibrate button are on
    // the panel, under the rulebook's patch pin.
    let all = joined(&p);
    assert!(all.contains("+104(100-119) to maximum Life"), "{all}");
    assert!(all.contains("observed model: 0 of 200 listings of Body Armour"), "{all}");
    assert!(all.contains("rulebook 0.5.5"), "{all}");
    let lay = craft_ui::layout(&p, &m);
    assert!(button(&lay, craft_ui::CALIBRATE).is_some_and(|b| b.on));
    assert!(button(&lay, craft_ui::PLAN).is_some_and(|b| b.on));

    // What the planner cannot work on says why instead of opening.
    let text = read(&fixtures().join("craft/rare-soldier-cuirass.txt"));
    let corrupted = read_item(&format!("{text}--------\nCorrupted\n"), ee2(), craft()).unwrap_err();
    assert!(corrupted.contains("corrupted") && corrupted.contains("no further crafting"), "{corrupted}");
    let headless: String = text.lines().filter(|l| !l.starts_with('{')).collect::<Vec<_>>().join("\n");
    let why = read_item(&headless, ee2(), craft()).unwrap_err();
    assert!(why.contains("Ctrl+Alt+C"), "{why}");

    // A panel opened for a scan with no item offers no calibration.
    let empty = craft_flow::empty_panel(rates(), String::new());
    assert!(!empty.can_calibrate());
    assert!(button(&craft_ui::layout(&empty, &m), craft_ui::CALIBRATE).is_none());
}

// ------------------------------------------------------------------ budget

#[test]
fn calibrate_states_its_cost_and_is_refused_under_the_budget() {
    assert_eq!(calibration_cost(), RequestCost { searches: 5, fetches: 20 });
    assert_eq!(
        calibration_statement("Body Armour", window(3)),
        "this calibration of Body Armour: 5 searches, 20 fetches; budget 27/30 free"
    );
    // Five searches and the reserve for the user's own checks.
    assert_eq!(USER_RESERVE, 5);
    assert!(allow(calibration_cost(), window(20)).is_ok(), "ten free is exactly enough");
    let refused = allow(calibration_cost(), window(21)).unwrap_err();
    assert!(refused.contains("refused by the budget") && refused.contains("9 are free"), "{refused}");

    // Before any search has answered, the seed's five-minute rule, said so.
    let now = Instant::now();
    let unseen = window_from(None, now);
    assert_eq!((unseen.used, unseen.max, unseen.seen), (0, 30, false));
    assert!(calibration_statement("Boots", unseen).ends_with("(no search has answered yet this session)"));
    // Counters a whole window old count nothing any more.
    let old = window_from(Some((29, 30, now)), now + Duration::from_secs(301));
    assert_eq!(old.used, 0);
    assert_eq!(window_from(Some((29, 30, now)), now + Duration::from_secs(10)).used, 29);

    // The first search: rare items of the class, clean, cheapest first, no
    // price bound; the four after it start at 3, 10, 30 and 100 times the
    // cheapest price it found.
    let first = calibration_search("Body Armour", None).unwrap();
    let bands = calibration_bands("Body Armour", 2.5).unwrap();
    assert_eq!(bands.len(), 4);
    for s in std::iter::once(&first).chain(&bands) {
        let body = s.query.to_body();
        assert_eq!(body["query"]["filters"]["type_filters"]["filters"]["category"]["option"], "armour.chest", "{body}");
        assert_eq!(body["query"]["filters"]["type_filters"]["filters"]["rarity"]["option"], "rare", "{body}");
    }
    assert!(first.query.to_body()["query"]["filters"]["trade_filters"]["filters"].get("price").is_none());
    let mins: Vec<f64> = bands.iter().map(|b| b.query.to_body()["query"]["filters"]["trade_filters"]["filters"]["price"]["min"].as_f64().unwrap()).collect();
    assert_eq!(mins, vec![7.5, 25.0, 75.0, 250.0]);
    assert_eq!(bands[0].label, "Body Armour from 7.5 ex up");
    assert!(calibration_search("Wombat", None).is_err());

    // Refused, it sends nothing and records nothing.
    let dir = std::env::temp_dir().join(format!("khalonipoe2-calib-refused-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut store = ObservedStore::with_limits(&dir, LEAGUE, 50, MAX_LISTINGS).unwrap();
    let err = calibrate("Body Armour", window(25), &mut never, &mut store, craft(), &|_, _| panic!("a refused calibration priced a listing")).unwrap_err();
    assert!(err.contains("refused by the budget"), "{err}");
    assert!(store.observed().is_empty());
    assert!(!store.path().exists());

    // On the panel: the statement first, and Run is off while refused.
    let mut p = open_panel(&the_item(), rates(), String::new(), String::new());
    assert_eq!(press(&mut p, craft_ui::CALIBRATE), Some(Action::Calibrate));
    p.ask(Prompt {
        ask: Ask::Calibrate,
        statement: calibration_statement("Body Armour", window(25)),
        refused: allow(calibration_cost(), window(25)).err(),
    });
    let lay = craft_ui::layout(&p, &m);
    assert!(joined(&p).contains("this calibration of Body Armour: 5 searches, 20 fetches; budget 5/30 free"));
    assert!(joined(&p).contains("refused by the budget"));
    assert!(button(&lay, craft_ui::RUN_CALIBRATION).is_some_and(|b| !b.on));
    assert!(button(&lay, craft_ui::CALIBRATE).is_none(), "the prompt replaces the button");
    // Allowed, Run is live; Cancel drops the prompt.
    p.ask(Prompt { ask: Ask::Calibrate, statement: calibration_statement("Body Armour", window(3)), refused: None });
    assert!(button(&craft_ui::layout(&p, &m), craft_ui::RUN_CALIBRATION).is_some_and(|b| b.on));
    assert_eq!(press(&mut p, craft_ui::CANCEL), Some(Action::Cancel));
    assert!(p.prompt.is_none());
    // While something runs, a stated request cannot be started.
    p.busy = Some("planning".into());
    assert!(!p.can_calibrate());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_scan_states_its_cost_and_is_refused_under_the_budget() {
    let plan = scan_plan(&cuirass(), craft(), &ee2().stats, catalog()).expect("the profile resolves");
    let searches = plan.relaxations.queries.len() as u32;
    assert_eq!(plan.cost, RequestCost { searches, fetches: searches * 2 });
    // The flip finder's own wording, with the live budget.
    assert_eq!(scan_statement(&plan, window(3)), plan.cost.statement(27, 30));
    assert!(scan_statement(&plan, window(3)).starts_with(&format!("this scan: {searches} searches, {} fetches", searches * 2)));
    let free_needed = searches + USER_RESERVE;
    assert!(allow(plan.cost, window(30 - free_needed)).is_ok());
    let refused = allow(plan.cost, window(30 - free_needed + 1)).unwrap_err();
    assert!(refused.contains("refused by the budget"), "{refused}");

    // A profile that cannot be searched says why before any cost.
    let odd = Profile { wants: vec![Wanted { family: "WombatHandling".into(), min_tier: 1 }], ..cuirass() };
    let err = scan_plan(&odd, craft(), &ee2().stats, catalog()).unwrap_err();
    assert!(err.contains("WombatHandling"), "{err}");

    // Refused, nothing is sent.
    let names = names();
    let convert = |amount: f64, currency: &str| craft_flow::listing_exalted(amount, currency, &names, table());
    let market = Market { convert: &convert, unit: "ex" };
    let held = exchange();
    let prices = |n: &str| craft_flow::price_of(table(), &held, n);
    let config = small();
    let planner = Planner { pool: craft(), observed: None, prices: &prices, config: &config };
    let r = rates();
    let inputs = ScanInputs { data: craft(), planner: &planner, market: &market, rates: &r, league: LEAGUE, statement: String::new() };
    let err = run_scan(&plan, window(29), &mut never, &inputs).unwrap_err();
    assert!(err.contains("refused by the budget"), "{err}");

    // On the panel the scan's prompt opens the Flips view, statement first.
    let mut p = craft_flow::empty_panel(rates(), String::new());
    p.ask(Prompt {
        ask: Ask::Scan(cuirass().name),
        statement: scan_statement(&plan, window(29)),
        refused: allow(plan.cost, window(29)).err(),
    });
    assert_eq!(p.view, View::Flips);
    let all = joined(&p);
    assert!(all.contains("this scan: ") && all.contains("budget 1/30 free"), "{all}");
    assert!(button(&craft_ui::layout(&p, &m), craft_ui::RUN_SCAN).is_some_and(|b| !b.on));
    p.ask(Prompt { ask: Ask::Scan(cuirass().name), statement: scan_statement(&plan, window(0)), refused: None });
    assert_eq!(press(&mut p, craft_ui::RUN_SCAN), Some(Action::Run));
}

// ------------------------------------------------------------------ plans

#[test]
fn the_planner_panel_shows_a_real_plan_from_fixtures() {
    let opened = the_item();
    let armour = armour_family(&opened);
    let mut p = open_panel(&opened, rates(), "prices: poe.ninja, Forbidden Rites".into(), String::new());
    tick_armour_and_fire(&mut p, &armour);
    let target = p.target();
    assert_eq!(target.wants.len(), 2, "{target:?}");
    assert_eq!(press(&mut p, craft_ui::PLAN), Some(Action::Plan));
    assert_eq!(p.plan, PlanState::Planning);
    assert!(joined(&p).contains("Planning"));

    let fetch = FixtureFetch::new(false);
    let buy = buy_from_fixture(&opened, &target, &fetch);
    let held = exchange();
    let prices = |n: &str| craft_flow::price_of(table(), &held, n);
    let plan = plan_item(PlanInputs {
        state: &opened.state,
        target: &target,
        data: craft(),
        observed: None,
        prices: &prices,
        buy,
        config: &small(),
    });
    assert!(!plan.strategies.is_empty());
    assert_eq!(plan.patch, "0.5.5");
    p.set_plan(PlanView::build(&plan, &rates()));
    assert_eq!(p.view, View::Plans);

    let all = joined(&p);
    assert!(all.contains("Buy one instead: cheapest 7 div (2 listings)"), "{all}");
    assert!(all.contains("uniform model: PoE2 does not publish weights"), "{all}");
    assert!(all.contains("optimistic floor: assumes every tier equally likely"), "{all}");
    assert!(all.contains("rulebook 0.5.5 · prices: poe.ninja, Forbidden Rites"), "{all}");
    assert!(all.contains("Assumptions of the cheapest plan"), "{all}");
    let costed = plan.strategies.iter().filter(|r| r.costed.per_finished.is_some()).count();
    assert!(costed > 0, "no strategy has a total: {:?}", plan.unknowns);
    assert!(all.contains("expected "), "{all}");
    // Opening a row lists its steps.
    let lay = craft_ui::layout(&p, &m);
    let (rect, i, _) = lay.rows[0];
    let (x, y) = centre(&rect);
    let hit = craft_ui::hit(&lay, x, y).unwrap();
    assert_eq!(hit, Action::ExpandRow(i));
    craft_ui::apply(&mut p, &hit);
    let steps = &plan.strategies[0].costed.steps;
    assert!(!steps.is_empty());
    assert!(joined(&p).contains(&steps[0].name), "{}", joined(&p));
    let lower = all.to_lowercase();
    assert!(!lower.contains("estimate") && !lower.contains("prediction"));
}

#[test]
fn a_hovered_item_becomes_a_plan_with_the_headline_model_and_buy_line() {
    let opened = the_item();
    let armour = armour_family(&opened);
    let mut p = open_panel(&opened, rates(), String::new(), String::new());
    tick_armour_and_fire(&mut p, &armour);
    let target = p.target();

    // One search for the finished item, on the item's own base: two
    // Soldier Cuirasses of the fixture meet it, cheapest 7 divines.
    let fetch = FixtureFetch::new(false);
    let buy = buy_from_fixture(&opened, &target, &fetch).expect("the fixture prices the finished item");
    assert_eq!(buy.listings, 2);
    assert!((buy.price - 7.0 * 410.0).abs() < 1e-6, "{}", buy.price);
    let asked = fetch.asked.borrow();
    assert_eq!(asked.len(), 1);
    let body = asked[0].0.to_body();
    assert_eq!(body["query"]["type"], "Soldier Cuirass", "{body}");
    drop(asked);

    let held = exchange();
    let prices = |n: &str| craft_flow::price_of(table(), &held, n);
    let plan = plan_item(PlanInputs {
        state: &opened.state,
        target: &target,
        data: craft(),
        observed: None,
        prices: &prices,
        buy: Ok(buy),
        config: &small(),
    });
    // Uniform headline: the only model without a sample.
    assert!(plan.model_line.starts_with("uniform model: PoE2 does not publish weights"), "{}", plan.model_line);
    assert!(plan.strategies.iter().all(|r| r.uniform.is_none()));
    assert_eq!(plan.buy.price, Some(buy.price));
    // A strategy dearer than buying is marked, the cheaper ones are not.
    for r in &plan.strategies {
        assert_eq!(r.not_worth_crafting, r.costed.per_finished.is_some_and(|c| c > buy.price));
    }
    let view = PlanView::build(&plan, &rates());
    assert_eq!(view.buy, "Buy one instead: cheapest 7 div (2 listings)");

    // Without a buy price the line says why, in the planner's place.
    let plan = plan_item(PlanInputs {
        state: &opened.state,
        target: &target,
        data: craft(),
        observed: None,
        prices: &prices,
        buy: Err("the finished item was not searched: refused by the budget".into()),
        config: &small(),
    });
    let view = PlanView::build(&plan, &rates());
    assert_eq!(view.buy, "Buy one instead: unknown: the finished item was not searched: refused by the budget");
    assert!(plan.unknowns.iter().any(|u| u.contains("refused by the budget")));
    assert!(!plan.unknowns.iter().any(|u| u.contains("no trade search for the finished item has run")));

    // Prices the table lacks come from the exchange answers held; the Orb
    // of Annulment comes from the table itself.
    let annul = craft_flow::price_of(table(), &held, "Orb of Annulment").unwrap();
    assert!((annul - 13.3).abs() < 0.05, "{annul}");
    assert_eq!(craft_flow::price_of(table(), &held, "Regal Orb"), Some(0.4));
    assert_eq!(craft_flow::price_of(table(), &held, "Exalted Orb"), Some(1.0));
    let bare = |n: &str| craft_flow::price_of(table(), &HashMap::new(), n);
    let plan = plan_item(PlanInputs {
        state: &opened.state,
        target: &target,
        data: craft(),
        observed: None,
        prices: &bare,
        buy: Ok(buy),
        config: &small(),
    });
    let missing = craft_flow::missing_prices(&plan);
    assert!(!missing.is_empty() && missing.iter().all(|n| bare(n).is_none()), "{missing:?}");
}

#[test]
fn a_calibration_feeds_the_observed_store_and_the_next_plan_uses_it() {
    let dir = std::env::temp_dir().join(format!("khalonipoe2-calib-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut store = ObservedStore::with_limits(&dir, LEAGUE, 50, MAX_LISTINGS).unwrap();
    let opened = the_item();
    let class = opened.state.class.clone();
    assert!(store.model(&class).is_none());
    assert!(craft_flow::observed_line(&store, &class).starts_with("observed model: 0 of 50 listings of Body Armour"));

    let names = names();
    let convert = |amount: f64, currency: &str| craft_flow::listing_exalted(amount, currency, &names, table());
    let fetch = FixtureFetch::new(true);
    let mut f = |q: &Query, n: usize| fetch.call(q, n);
    let got = calibrate(&class, window(0), &mut f, &mut store, craft(), &convert).expect("the calibration runs");
    // Five searches of forty listings each.
    let asked = fetch.asked.borrow();
    assert_eq!(asked.len(), 5);
    assert!(asked.iter().all(|(_, n)| *n == 40));
    // The first is unbounded; the others start at 3, 10, 30 and 100 times
    // the cheapest price the first returned, read here from the fixture.
    let cheapest = fetched()
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let p = &e["listing"]["price"];
            convert(p["amount"].as_f64()?, p["currency"].as_str()?)
        })
        .fold(f64::INFINITY, f64::min);
    assert!(cheapest.is_finite() && cheapest > 0.0);
    assert_eq!(asked[0].0.price_min, None);
    for (k, (q, _)) in [3.0, 10.0, 30.0, 100.0].iter().zip(&asked[1..]) {
        let want = (cheapest * k * 100.0).round() / 100.0;
        assert_eq!(q.price_min, Some(want), "band x{k}");
    }
    drop(asked);
    assert_eq!(got.searches, 5);
    assert!(got.stopped.is_none());
    // The vanished listing and the normal Soldier Cuirass give nothing, a
    // listing a search returns three times counts once, and every other
    // rare is kept.
    assert_eq!(got.not_read, 10);
    assert_eq!((got.recorded, got.repeated), (70, 10));
    assert_eq!(got.listings, 70);
    assert!(got.text(&class).starts_with("calibration of Body Armour: 70 new listings recorded"), "{}", got.text(&class));
    // Every recorded listing is a line of the league's file.
    let lines = std::fs::read_to_string(store.path()).unwrap().lines().count();
    assert_eq!(lines, 70);

    let model = store.model(&class).expect("the sample is past the minimum");
    assert!(craft_flow::observed_line(&store, &class).starts_with("observed on 70 listings of Body Armour, biased toward what sellers list"));

    // The next plan heads with the observed model and keeps the uniform one
    // beside it.
    let armour = armour_family(&opened);
    let mut p = open_panel(&opened, rates(), String::new(), craft_flow::observed_line(&store, &class));
    tick_armour_and_fire(&mut p, &armour);
    let target = p.target();
    let held = exchange();
    let prices = |n: &str| craft_flow::price_of(table(), &held, n);
    let plan = plan_item(PlanInputs {
        state: &opened.state,
        target: &target,
        data: craft(),
        observed: Some(&model),
        prices: &prices,
        buy: Err("not searched in this test".into()),
        config: &small(),
    });
    assert!(plan.model_line.starts_with("observed on 70 listings of Body Armour"), "{}", plan.model_line);
    assert!(plan.strategies.iter().all(|r| r.uniform.is_some()));
    p.set_plan(PlanView::build(&plan, &rates()));
    let all = joined(&p);
    assert!(all.contains("observed on 70 listings of Body Armour"), "{all}");
    assert!(all.contains("uniform model: "), "{all}");

    // A second calibration of the same listings adds nothing twice.
    let again = FixtureFetch::new(false);
    let mut f = |q: &Query, n: usize| again.call(q, n);
    let second = calibrate(&class, window(0), &mut f, &mut store, craft(), &convert).unwrap();
    assert_eq!(second.recorded, 14, "the undecorated ids are new once");
    let third = calibrate(&class, window(0), &mut f, &mut store, craft(), &convert).unwrap();
    assert_eq!((third.recorded, third.repeated), (0, 80));

    // A search that fails ends the calibration and keeps what came before.
    let calls = RefCell::new(0);
    let mut failing = |q: &Query, n: usize| {
        *calls.borrow_mut() += 1;
        if *calls.borrow() == 3 {
            Err("trade cooldown 12s".to_string())
        } else {
            FixtureFetch::new(true).call(q, n).map(|v| {
                v.into_iter()
                    .map(|e| {
                        e.map(|mut x| {
                            x["id"] = Value::from(format!("{}-late{}", x["id"].as_str().unwrap_or(""), calls.borrow()));
                            x
                        })
                    })
                    .collect()
            })
        }
    };
    let partial = calibrate(&class, window(0), &mut failing, &mut store, craft(), &convert).unwrap();
    assert_eq!(partial.searches, 2);
    assert!(partial.text(&class).contains("stopped after 2 of 5 searches"), "{}", partial.text(&class));
    assert!(partial.text(&class).contains("trade cooldown 12s"));

    // A first search with no priced listing leaves nothing to band from:
    // the calibration stops after it and says why.
    let unpriced = |q: &Query, n: usize| {
        FixtureFetch::new(true).call(q, n).map(|v| {
            v.into_iter()
                .map(|e| {
                    e.map(|mut x| {
                        x["listing"]["price"] = Value::Null;
                        x["id"] = Value::from(format!("{}-unpriced", x["id"].as_str().unwrap_or("")));
                        x
                    })
                })
                .collect()
        })
    };
    let mut unpriced = unpriced;
    let stopped = calibrate(&class, window(0), &mut unpriced, &mut store, craft(), &convert).unwrap();
    assert_eq!(stopped.searches, 1);
    assert!(stopped.text(&class).contains("the price bands above it cannot be set"), "{}", stopped.text(&class));
    let _ = std::fs::remove_dir_all(dir);
}

// ------------------------------------------------------------------ flips

fn scan_fixture() -> (craft_flow::Scanned, Vec<(Query, usize)>) {
    let plan = scan_plan(&cuirass(), craft(), &ee2().stats, catalog()).unwrap();
    let names = names();
    let convert = |amount: f64, currency: &str| craft_flow::listing_exalted(amount, currency, &names, table());
    let market = Market { convert: &convert, unit: "ex" };
    let held = exchange();
    let prices = |n: &str| craft_flow::price_of(table(), &held, n);
    let config = SimConfig { runs: 200, ..SimConfig::default() };
    let planner = Planner { pool: craft(), observed: None, prices: &prices, config: &config };
    let r = rates();
    let statement = scan_statement(&plan, window(3));
    let inputs = ScanInputs { data: craft(), planner: &planner, market: &market, rates: &r, league: LEAGUE, statement };
    let fetch = FixtureFetch::new(false);
    let mut f = |q: &Query, n: usize| fetch.call(q, n);
    let scanned = run_scan(&plan, window(3), &mut f, &inputs).expect("the scan runs");
    let asked = fetch.asked.borrow().clone();
    (scanned, asked)
}

#[test]
fn the_flip_list_shows_a_scan_from_fixtures() {
    let (scanned, asked) = scan_fixture();
    let plan = scan_plan(&cuirass(), craft(), &ee2().stats, catalog()).unwrap();
    // One search per query of the scan, twenty listings each.
    assert_eq!(asked.len(), plan.relaxations.queries.len());
    assert!(asked.iter().all(|(_, n)| *n == 20));
    assert_eq!(asked[0].0, plan.relaxations.queries[0].query, "the resale search runs first");

    let list = scanned.list.clone();
    assert!(!list.rows.is_empty());
    assert!(list.profile.starts_with("Armour and fire cuirass: Body Armour, item level 75+"), "{}", list.profile);
    assert_eq!(list.scan, scan_statement(&plan, window(3)));
    assert!(list.model.contains("optimistic floor") && list.model.contains("each candidate costed on 200 runs"), "{}", list.model);
    // The fixture's normal base and unpriced listing are candidates of
    // neither kind: the one without a price is left out and said so.
    assert!(scanned.notes.iter().any(|n| n.contains("listings left out: their price cannot be read in exalted")), "{:?}", scanned.notes);

    let mut p = craft_flow::empty_panel(rates(), "prices: poe.ninja, Forbidden Rites".into());
    p.set_flips(list.clone());
    craft_ui::apply(&mut p, &Action::SetView(View::Flips));
    let all = joined(&p);
    assert!(all.contains("Armour and fire cuirass"), "{all}");
    assert!(all.contains("this scan: "), "{all}");
    assert!(all.contains("resale cheapest"), "{all}");
    assert!(all.contains("margin "), "{all}");
    assert!(all.contains("Candidates"), "{all}");

    // Opening a candidate shows its card, its plan and the resale listings,
    // and Open site carries the search it came from, narrowed to its seller.
    craft_ui::apply(&mut p, &Action::SelectCandidate(0));
    let all = joined(&p);
    assert!(all.contains("its plan") && all.contains("resale listings"), "{all}");
    let lay = craft_ui::layout(&p, &m);
    let open = button(&lay, craft_ui::OPEN_SITE).expect("an Open site button");
    let Action::OpenSite(url) = &open.action else { panic!("{:?}", open.action) };
    assert!(url.starts_with("https://www.pathofexile.com/trade2/search/poe2/Forbidden%20Rites?q="), "{url}");
    let seller = &list.rows[0].seller;
    assert!(url.contains("account") && url.contains(seller.as_str()), "{url}");
    let lower = all.to_lowercase();
    assert!(!lower.contains("estimate") && !lower.contains("prediction"));
}

#[test]
fn a_scan_produces_ranked_candidates_with_visible_arithmetic() {
    let (scanned, _) = scan_fixture();
    let rows: &[FlipRow] = &scanned.list.rows;
    let r = rates();
    // Ranked by margin, largest first; rows without one after every row
    // with one.
    let margins: Vec<Option<f64>> = rows.iter().map(FlipRow::margin).collect();
    assert!(margins.iter().any(Option::is_some), "no candidate has a margin");
    let first_none = margins.iter().position(Option::is_none).unwrap_or(margins.len());
    assert!(margins[first_none..].iter().all(Option::is_none));
    let known: Vec<f64> = margins[..first_none].iter().map(|m| m.unwrap()).collect();
    assert!(known.windows(2).all(|w| w[0] >= w[1]), "{known:?}");

    // Every figure the row draws is the one its margin is made of.
    for row in rows {
        let (first, second) = row.lines(&r);
        assert!(first.contains(&format!("listed {} by {}", r.money(row.listed), row.seller)), "{first}");
        match (&row.fix, &row.resale, row.margin()) {
            (Ok(fix), Ok(resale), Some(m)) => {
                assert!((m - (resale.price - row.listed - fix.cost)).abs() < 1e-9);
                assert!(first.contains(&format!("fix ~{}", r.money(fix.cost))), "{first}");
                assert!(second.contains(&format!("resale cheapest {}", r.money(resale.price))), "{second}");
                let shown = if m < 0.0 { format!("-{}", r.money(-m)) } else { r.money(m) };
                assert!(second.contains(&format!("margin {shown} under uniform")), "{second}");
            }
            (Err(_), _, None) => assert!(first.contains("fix cost unknown: "), "{first}"),
            (_, Err(_), None) => assert!(second.contains("resale unknown: "), "{second}"),
            other => panic!("inconsistent row {other:?}"),
        }
        // The resale listings behind the price are listed, cheapest first.
        if let Ok(resale) = &row.resale {
            assert_eq!(row.resale_listings.first(), Some(&r.money(resale.price)));
            assert_eq!(row.resale_listings.len(), resale.listings);
        }
    }
    // A listing two relaxed searches both return is costed once: every
    // search here answers with the fixture's fifteen distinct listings (one
    // id appears three times), and the one without a price is left out.
    assert_eq!(rows.len(), 14);
}


/// The request shapes the trade site's own filter catalog
/// (`/api/trade2/data/filters`, read 2026-09-26) defines: rarity ids
/// normal/magic/rare/unique/uniquefoil/nonunique; a price filter with
/// min/max whose unset option means "Exalted Orb Equivalent"; an account
/// filter taking an `input` string.
#[test]
fn calibration_and_open_site_bodies_use_the_trade_sites_filter_shapes() {
    let first = calibration_search("Body Armour", None).unwrap().query.to_body();
    let trade = &first["query"]["filters"]["trade_filters"]["filters"];
    assert_eq!(first["query"]["filters"]["type_filters"]["filters"]["rarity"], serde_json::json!({ "option": "rare" }));
    assert!(trade.get("price").is_none(), "the first search is unbounded");
    let band = calibration_bands("Body Armour", 4.0).unwrap().remove(0).query.to_body();
    // A bare min, no option: the site reads it in exalted orb equivalent,
    // the unit the bands are computed in.
    assert_eq!(band["query"]["filters"]["trade_filters"]["filters"]["price"], serde_json::json!({ "min": 12.0 }));
    let url = craft_flow::listing_url("Forbidden Rites", &calibration_search("Body Armour", None).unwrap().query, "seller#1234");
    let q = url.split("?q=").nth(1).expect("the url carries the query");
    let decoded = percent_decode(q);
    let body: serde_json::Value = serde_json::from_str(&decoded).expect("the query is JSON");
    assert_eq!(body["query"]["filters"]["trade_filters"]["filters"]["account"], serde_json::json!({ "input": "seller#1234" }));
    assert!(url.starts_with("https://www.pathofexile.com/trade2/search/poe2/Forbidden%20Rites?q="), "{url}");
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            out.push(u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap(), 16).unwrap());
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap()
}
