//! The craft planner's path through the app, without a window: a copied
//! item becomes the planner panel, the ticked target becomes a plan beside
//! the price of buying one, a calibration feeds the observed store, and a
//! flip profile becomes a ranked list of candidates.
//!
//! Nothing here sends a request or reads a clock on its own. Every trade
//! search goes through a fetch function the caller passes in (the overlay's
//! worker passes its trade client, the tests pass fixtures), and every
//! search is weighed against the five-minute budget first: a calibration
//! or a scan states what it costs before it runs, and is refused while it
//! would leave fewer than [`USER_RESERVE`] searches for the user's own
//! price checks.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use khaloni_poe2_core::craft::data::CraftData;
use khaloni_poe2_core::craft::model::Model;
use khaloni_poe2_core::craft::plan::{model_line, plan, BuyQuote, Plan};
use khaloni_poe2_core::craft::sim::SimConfig;
use khaloni_poe2_core::craft::strategy::Target;
use khaloni_poe2_core::craft::types::{ItemState, Rarity};
use khaloni_poe2_core::ee2::data::StatDb;
use khaloni_poe2_core::ee2::request::category_trade_id;
use khaloni_poe2_core::ee2::Ee2Data;
use khaloni_poe2_core::flip::{
    self, relaxations, resale, resolve, scan_cost, LoadedProfiles, Market, Planner, Profile, Relaxations, RequestCost,
    Resolved, Wanted, DEFAULT_MARGIN, LISTINGS_PER_FETCH, LISTINGS_PER_SEARCH,
};
use khaloni_poe2_core::ninja::PriceTable;
use khaloni_poe2_core::trade::{Query, StatIndex};
use serde_json::Value;

use crate::craft_ui::{self, FlipList, FlipRow, ObservedMargin, Panel, Picker, Rates};
use crate::observed_store::{ObservedStore, BIAS_NOTE};

// ------------------------------------------------------------------ the item

/// A copied item read for the planner.
#[derive(Debug, Clone, PartialEq)]
pub struct Opened {
    pub state: ItemState,
    pub picker: Picker,
}

/// Reads a copied item into the planner's item state and its picker. Fails,
/// with the reason the panel shows, when the copy cannot be read (no
/// modifier headers, a base the data does not know) or the item takes no
/// crafting.
pub fn read_item(text: &str, ee2: &Ee2Data, data: &CraftData) -> Result<Opened, String> {
    let built = khaloni_poe2_core::ee2::build(text, ee2).map_err(|e| format!("the item could not be read: {e}"))?;
    let state = ItemState::from_built(&built, data)?;
    if state.rarity == Rarity::Unique {
        return Err("a unique item rolls no modifiers from the pool, so there is nothing to plan".to_string());
    }
    let locked = [(state.corrupted, "corrupted"), (state.mirrored, "mirrored"), (state.sanctified, "sanctified")];
    if let Some((_, what)) = locked.iter().find(|(on, _)| *on) {
        return Err(format!("the item is {what} and takes no further crafting"));
    }
    let picker = Picker::build(data, &state);
    Ok(Opened { state, picker })
}

/// The panel for a read item, its current families ticked.
pub fn open_panel(opened: &Opened, rates: Rates, prices: String, observed: String) -> Panel {
    let mut p = Panel::new(Arc::new(opened.picker.clone()), rates, prices);
    p.observed = observed;
    p
}

/// A panel with no item, for a scan asked for from the settings window
/// while no item has been read.
pub fn empty_panel(rates: Rates, prices: String) -> Panel {
    let picker = Picker {
        title: "No item read: hover an item and press the craft key".to_string(),
        ..Picker::default()
    };
    Panel::new(Arc::new(picker), rates, prices)
}

// ------------------------------------------------------------------ money

/// The price panel's units from the price table: what a divine is worth in
/// exalted and in chaos. Without a divine line every amount stays in
/// exalted.
pub fn rates(table: &PriceTable, divine_threshold: f64) -> Rates {
    match table.lookup("Divine Orb") {
        Some(d) if d.exalted > 0.0 => {
            Rates { exalted_per_divine: d.exalted, chaos_per_divine: d.chaos, divine_threshold }
        }
        _ => Rates { exalted_per_divine: 0.0, chaos_per_divine: 0.0, divine_threshold },
    }
}

/// One item's price in exalted by its in-game name: the poe.ninja table
/// first, then exchange answers the overlay already holds for what the
/// table lacks. `None` for anything neither has: the plan then says which
/// price it is missing.
pub fn price_of(table: &PriceTable, exchange: &HashMap<String, f64>, name: &str) -> Option<f64> {
    if name == "Exalted Orb" {
        return Some(1.0);
    }
    table
        .lookup(name)
        .map(|p| p.exalted)
        .filter(|p| p.is_finite() && *p > 0.0)
        .or_else(|| exchange.get(name).copied().filter(|p| p.is_finite() && *p > 0.0))
}

/// The items a plan could not price, from its "unknown: no price for X"
/// lines, for the exchange to be asked about through the overlay's own
/// background path.
pub fn missing_prices(plan: &Plan) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for u in &plan.unknowns {
        if let Some(name) = u.strip_prefix("unknown: no price for ") {
            if !out.iter().any(|n| n == name) {
                out.push(name.to_string());
            }
        }
    }
    out
}

/// A listing's price in exalted, through the trade site's currency ids and
/// the price table, the way the price check converts its listings.
pub fn listing_exalted(amount: f64, currency: &str, names: &HashMap<String, String>, table: &PriceTable) -> Option<f64> {
    crate::appraise::price_exalted(amount, currency, names, table)
}

// ------------------------------------------------------------------ the budget

/// Searches a calibration or a scan must leave free in the five-minute
/// window: the point at which the price check's budget line turns to
/// "near the cap", so the user's next checks never wait on the planner.
pub const USER_RESERVE: u32 = crate::appraise::BUDGET_LOW_WITHIN;

/// The size of the five-minute search rule before the site has answered a
/// search this session: the limiter's seed rule (`30:300:1800`).
pub const SEED_WINDOW_MAX: u32 = 30;

/// The window the counters describe.
const WINDOW: Duration = Duration::from_secs(300);

/// The five-minute search window as the site last reported it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchWindow {
    pub used: u32,
    pub max: u32,
    /// Whether the figures are the site's own; false before the first
    /// search of the session has answered.
    pub seen: bool,
}

impl SearchWindow {
    pub fn free(&self) -> u32 {
        self.max.saturating_sub(self.used)
    }
}

/// The five-minute counters of the last search response, and when it came.
static COUNTERS: std::sync::Mutex<Option<(u32, u32, Instant)>> = std::sync::Mutex::new(None);

/// Reads a request-log line: a search response's five-minute counters are
/// kept for the planner's budget checks.
pub fn note_request_line(line: &str) {
    if let Some((used, max)) = crate::appraise::search_counters(line) {
        *COUNTERS.lock().unwrap_or_else(|e| e.into_inner()) = Some((used, max, Instant::now()));
    }
}

/// The window from counters read at `at`: once a whole window has passed
/// since, every search it counted has left it.
pub fn window_from(counters: Option<(u32, u32, Instant)>, now: Instant) -> SearchWindow {
    match counters {
        Some((used, max, at)) => {
            let used = if now.saturating_duration_since(at) >= WINDOW { 0 } else { used };
            SearchWindow { used, max, seen: true }
        }
        None => SearchWindow { used: 0, max: SEED_WINDOW_MAX, seen: false },
    }
}

/// The window as it stands now.
pub fn search_window(now: Instant) -> SearchWindow {
    window_from(*COUNTERS.lock().unwrap_or_else(|e| e.into_inner()), now)
}

fn count(n: u32, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// "this scan: 5 searches, 10 fetches; budget 27/30 free", the flip
/// finder's wording, for any request of `cost` named `what`.
pub fn statement(what: &str, cost: RequestCost, window: SearchWindow) -> String {
    let mut out = format!(
        "{what}: {}, {}; budget {}/{} free",
        count(cost.searches, "search", "searches"),
        count(cost.fetches, "fetch", "fetches"),
        window.free(),
        window.max
    );
    if !window.seen {
        out.push_str(" (no search has answered yet this session)");
    }
    out
}

/// Whether a request of `cost` may go now: it must leave [`USER_RESERVE`]
/// searches free in the five-minute window. The refusal says why and what
/// would let it through.
pub fn allow(cost: RequestCost, window: SearchWindow) -> Result<(), String> {
    let need = cost.searches + USER_RESERVE;
    let free = window.free();
    if free >= need {
        return Ok(());
    }
    Err(format!(
        "refused by the budget: {} need {need} free searches of the five-minute window ({USER_RESERVE} are kept for your price checks), and {free} are free; try again in a few minutes",
        count(cost.searches, "search", "searches")
    ))
}

// ------------------------------------------------------------------ fetching

/// Runs one search and fetches up to `listings` of its results, cheapest
/// first; the entries come back as the API sent them (`None` for a
/// listing that vanished). The overlay passes its trade client here.
pub type FetchFn<'a> = dyn FnMut(&Query, usize) -> Result<Vec<Option<Value>>, String> + 'a;

/// The fetch pages that bring back `listings` of `hashes`, ten at a time.
pub fn fetch_pages(hashes: usize, listings: usize) -> Vec<std::ops::Range<usize>> {
    let want = listings.min(hashes);
    let page = LISTINGS_PER_FETCH as usize;
    (0..want).step_by(page).map(|from| from..(from + page).min(want)).collect()
}

// ------------------------------------------------------------------ the buy line

/// The finished item as a search: the target's families at their tiers on
/// the item's own base, the listings of which are what buying one costs.
pub fn buy_profile(state: &ItemState, target: &Target) -> Profile {
    Profile {
        name: "buy one".to_string(),
        class: state.class.clone(),
        base: Some(state.base.clone()),
        min_ilvl: 0,
        wants: target.wants.iter().map(|w| Wanted { family: w.family.clone(), min_tier: w.min_tier }).collect(),
        margin: DEFAULT_MARGIN,
    }
}

/// The search for the finished item, or why it cannot be searched.
pub fn buy_search(
    state: &ItemState,
    target: &Target,
    data: &CraftData,
    stats: &StatDb,
    catalog: &StatIndex,
) -> Result<(Resolved, Query), String> {
    let resolved = resolve(&buy_profile(state, target), data, stats, catalog)
        .map_err(|reasons| format!("the finished item cannot be searched: {}", reasons.join("; ")))?;
    let query = relaxations(&resolved)
        .queries
        .into_iter()
        .next()
        .map(|q| q.query)
        .ok_or_else(|| "the finished item has no search".to_string())?;
    Ok((resolved, query))
}

/// The price of buying one: the cheapest of the fetched listings that meet
/// the target, in exalted, with how many listings the price rests on; or
/// why there is no price.
pub fn buy_quote(fetched: &[Value], resolved: &Resolved, data: &CraftData, market: &Market) -> Result<BuyQuote, String> {
    let r = resale(fetched, resolved, data, market);
    match r.cheapest {
        Some(price) => Ok(BuyQuote { price, listings: r.listings_counted }),
        None => Err(format!("the search for the finished item found no listing that meets it ({})", r.text)),
    }
}

// ------------------------------------------------------------------ the plan

/// What a plan is run with.
pub struct PlanInputs<'a> {
    pub state: &'a ItemState,
    pub target: &'a Target,
    pub data: &'a CraftData,
    /// The observed model of the item's class, when its sample is large
    /// enough.
    pub observed: Option<&'a Model>,
    pub prices: &'a (dyn Fn(&str) -> Option<f64> + Sync),
    /// The price of buying one, or why there is none.
    pub buy: Result<BuyQuote, String>,
    pub config: &'a SimConfig,
}

/// Costs every applicable strategy to the target. When no buy price could
/// be had, the plan's buy line carries the real reason (a refused budget, a
/// search that found nothing) in place of the planner's generic one.
pub fn plan_item(inputs: PlanInputs) -> Plan {
    let buy = inputs.buy.as_ref().ok().copied();
    let mut p = plan(inputs.state, inputs.target, inputs.data, inputs.observed, inputs.prices, buy, inputs.config);
    if let Err(reason) = &inputs.buy {
        let line = format!("unknown: {reason}");
        if let Some(old) = p.buy.unknown.replace(line.clone()) {
            for u in &mut p.unknowns {
                if *u == old {
                    *u = line.clone();
                }
            }
        }
    }
    p
}

// ------------------------------------------------------------------ the observed model

/// The line beside the Calibrate button: the class's observed sample.
pub fn observed_line(store: &ObservedStore, class: &str) -> String {
    let n = store.observed().listings(class);
    let min = store.observed().min_listings();
    if store.model(class).is_some() {
        format!("observed on {n} listings of {class}, {BIAS_NOTE}")
    } else {
        format!("observed model: {n} of {min} listings of {class} recorded; Calibrate gathers more")
    }
}

/// Listings a calibration fetches per search: four fetch pages.
pub const CALIBRATION_PAGES: u32 = 4;
/// Searches of one calibration.
pub const CALIBRATION_SEARCHES: u32 = 5;

/// The requests of one calibration: five searches of four fetch pages.
pub fn calibration_cost() -> RequestCost {
    RequestCost { searches: CALIBRATION_SEARCHES, fetches: CALIBRATION_SEARCHES * CALIBRATION_PAGES }
}

/// The price bands of the calibration's last four searches, as multiples
/// of the class's cheapest asking price the first search found: each search
/// starts at its band and reads the forty cheapest listings from there up,
/// so the sample climbs the market instead of stopping at the junk end.
pub const CALIBRATION_BANDS: [f64; 4] = [3.0, 10.0, 30.0, 100.0];

/// One search of a calibration.
#[derive(Debug, Clone, PartialEq)]
pub struct CalibrationSearch {
    pub label: String,
    pub query: Query,
}

/// A calibration search of `class`: rare items of the class, clean, one
/// listing per seller, cheapest first, from `price_min` (exalted orb
/// equivalent) up when given.
pub fn calibration_search(class: &str, price_min: Option<f64>) -> Result<CalibrationSearch, String> {
    let category =
        category_trade_id(class).ok_or_else(|| format!("{class} has no trade category, so it cannot be calibrated"))?;
    Ok(CalibrationSearch {
        label: match price_min {
            None => format!("cheapest {class}"),
            Some(min) => format!("{class} from {} ex up", round_price(min)),
        },
        query: Query {
            category: Some(category.to_string()),
            category_enabled: true,
            category_replaces_type: true,
            rarity: Some("rare".to_string()),
            corrupted: Some(false),
            mirrored: Some(false),
            sanctified: Some(false),
            collapse: true,
            price_min: price_min.map(round_price),
            ..Default::default()
        },
    })
}

/// The four banded searches that follow a first search whose cheapest
/// listing asked `cheapest` exalted.
pub fn calibration_bands(class: &str, cheapest: f64) -> Result<Vec<CalibrationSearch>, String> {
    CALIBRATION_BANDS.iter().map(|k| calibration_search(class, Some(cheapest * k))).collect()
}

/// Two significant decimals at most: a bound the site reads the same way
/// the label shows it.
fn round_price(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// A fetched listing's asking price in exalted, when it has one the
/// caller's conversion knows.
pub fn entry_exalted(entry: &serde_json::Value, convert: &dyn Fn(f64, &str) -> Option<f64>) -> Option<f64> {
    let price = &entry["listing"]["price"];
    let amount = price["amount"].as_f64().filter(|a| *a > 0.0)?;
    convert(amount, price["currency"].as_str()?)
}

/// What a calibration recorded.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Calibrated {
    /// Searches that ran.
    pub searches: usize,
    pub recorded: usize,
    pub repeated: usize,
    pub not_read: usize,
    /// Why the calibration stopped before its last search, if it did.
    pub stopped: Option<String>,
    /// The class's listings after it.
    pub listings: u32,
}

impl Calibrated {
    /// "calibration of Body Armour: 187 new listings recorded (13 already
    /// held, 0 unreadable); 230 listings of Body Armour kept".
    pub fn text(&self, class: &str) -> String {
        let mut out = format!(
            "calibration of {class}: {} new listings recorded ({} already held, {} without rolled modifiers or unreadable); {} listings of {class} kept",
            self.recorded, self.repeated, self.not_read, self.listings
        );
        if let Some(why) = &self.stopped {
            out.push_str(&format!("; stopped after {} of {CALIBRATION_SEARCHES} searches: {why}", self.searches));
        }
        out
    }
}

/// "this calibration of Body Armour: 5 searches, 20 fetches; budget 27/30
/// free".
pub fn calibration_statement(class: &str, window: SearchWindow) -> String {
    statement(&format!("this calibration of {class}"), calibration_cost(), window)
}

/// Runs the five searches of a calibration of `class` through `fetch` and
/// records every fetched listing into the store: the class's cheapest
/// listings first, then one search per price band above the cheapest price
/// they asked (`convert` turns a listing's price into exalted). Nothing is
/// sent when the budget refuses it. A failed search, or a first search with
/// no priced listing to set the bands from, ends the calibration with what
/// was recorded kept.
pub fn calibrate(
    class: &str,
    window: SearchWindow,
    fetch: &mut FetchFn,
    store: &mut ObservedStore,
    data: &CraftData,
    convert: &dyn Fn(f64, &str) -> Option<f64>,
) -> Result<Calibrated, String> {
    let first = calibration_search(class, None)?;
    allow(calibration_cost(), window)?;
    let per_search = (CALIBRATION_PAGES * LISTINGS_PER_FETCH) as usize;
    let mut got = Calibrated::default();
    let mut queue = vec![first];
    let mut banded = false;
    while !queue.is_empty() {
        let s = queue.remove(0);
        let raw = match fetch(&s.query, per_search) {
            Ok(raw) => raw,
            Err(why) => {
                got.stopped = Some(format!("{}: {why}", s.label));
                break;
            }
        };
        got.searches += 1;
        let r = store.record_fetch(&raw, data).map_err(|e| format!("the observed store could not be written: {e}"))?;
        got.recorded += r.recorded;
        got.repeated += r.repeated;
        got.not_read += r.not_read;
        if !banded {
            banded = true;
            let cheapest = raw.iter().flatten().filter_map(|e| entry_exalted(e, convert)).min_by(f64::total_cmp);
            match cheapest {
                Some(c) => queue = calibration_bands(class, c)?,
                None => {
                    got.stopped = Some(format!(
                        "{}: no listing with a price in a known currency, so the price bands above it cannot be set",
                        s.label
                    ));
                    break;
                }
            }
        }
    }
    got.listings = store.observed().listings(class);
    Ok(got)
}

// ------------------------------------------------------------------ flip scans

/// Runs per strategy when a scan costs its candidates. A scan plans every
/// candidate it finds (up to twenty per search), so at the full run count a
/// scan would take many minutes; this many runs keeps the ranking steady
/// and the scan to a minute or two. The list's model line says so.
pub const SCAN_RUNS: usize = 2_000;

/// A profile ready to scan: its searches and what they cost.
#[derive(Debug, Clone, PartialEq)]
pub struct ScanPlan {
    pub resolved: Resolved,
    pub relaxations: Relaxations,
    pub cost: RequestCost,
}

/// Resolves a profile into its scan: the resale search and one per
/// relaxation. Every family that cannot be searched is named.
pub fn scan_plan(profile: &Profile, data: &CraftData, stats: &StatDb, catalog: &StatIndex) -> Result<ScanPlan, String> {
    let resolved = resolve(profile, data, stats, catalog)
        .map_err(|reasons| format!("profile \"{}\" cannot be scanned: {}", profile.name, reasons.join("; ")))?;
    let relaxations = relaxations(&resolved);
    let cost = scan_cost(&relaxations);
    Ok(ScanPlan { resolved, relaxations, cost })
}

/// "Armour and fire cuirass: Body Armour, item level 75+, wants
/// LocalPhysicalDamageReductionRatingPercent T2, FireResistance T3; margin
/// 30%".
pub fn profile_label(profile: &Profile) -> String {
    let on = profile.base.clone().unwrap_or_else(|| profile.class.clone());
    let wants: Vec<String> = profile.wants.iter().map(|w| format!("{} T{}", w.family, w.min_tier)).collect();
    let mut out = format!("{}: {on}", profile.name);
    if profile.min_ilvl > 0 {
        out.push_str(&format!(", item level {}+", profile.min_ilvl));
    }
    out.push_str(&format!(", wants {}; margin {}%", wants.join(", "), profile.margin));
    out
}

/// The trade site's search behind a candidate, narrowed to its seller: the
/// site has no address for one listing, so this opens the search it was
/// found in with only that seller's listings.
pub fn listing_url(league: &str, query: &Query, seller: &str) -> String {
    let mut body = query.to_body();
    if !seller.is_empty() {
        body["query"]["filters"]["trade_filters"]["filters"]["account"] = serde_json::json!({ "input": seller });
    }
    format!("https://www.pathofexile.com/trade2/search/poe2/{}?q={}", urlencode(league), urlencode(&body.to_string()))
}

/// Percent-encodes a string for a URL query (the unreserved set literal).
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// What a scan is run with besides its searches.
pub struct ScanInputs<'a> {
    pub data: &'a CraftData,
    pub planner: &'a Planner<'a>,
    /// Converts listing prices to exalted ("ex").
    pub market: &'a Market<'a>,
    pub rates: &'a Rates,
    pub league: &'a str,
    /// The cost statement shown before the scan ran.
    pub statement: String,
}

/// A scan's result: the list for the panel, and what it left out and why.
#[derive(Debug, Clone, PartialEq)]
pub struct Scanned {
    pub list: FlipList,
    pub notes: Vec<String>,
}

/// One candidate as the panel shows it; `None` when its price cannot be
/// read in exalted (the row's figures all rest on it).
fn flip_row(c: &flip::Candidate, resale: &flip::Resale, rates: &Rates, url: String) -> Option<FlipRow> {
    let listed = c.price.clone().ok()?;
    let fix = match &c.fix {
        Ok(f) => Ok(craft_ui::Fix { cost: f.cost, strategy: f.strategy.clone(), median: f.median }),
        Err(reason) => Err(reason.clone()),
    };
    let resale_price = match resale.cheapest {
        Some(price) => Ok(craft_ui::Resale { price, listings: resale.listings_counted }),
        None => Err("no listing of the finished search meets the profile".to_string()),
    };
    let observed = match (&c.margin_observed, &c.observed_label) {
        (Some(Ok(margin)), Some(label)) => Some(ObservedMargin { margin: *margin, label: label.clone() }),
        _ => None,
    };
    let card = match &c.item {
        Ok(state) => {
            let mut lines = vec![format!("{} · item level {}", state.base, state.item_level)];
            lines.extend(state.mods.iter().map(|m| match m.tier {
                Some(t) => format!("{}  {}", craft_ui::tier_text(m.kind, t), m.text),
                None => m.text.clone(),
            }));
            lines
        }
        Err(reason) => vec![format!("card unreadable: {reason}")],
    };
    // The steps of the strategy the fix figure came from.
    let steps = c
        .plan
        .as_ref()
        .and_then(|p| {
            let best = p.strategies.iter().filter(|r| r.costed.per_finished.is_some()).min_by(|a, b| {
                a.costed.per_finished.unwrap_or(f64::INFINITY).total_cmp(&b.costed.per_finished.unwrap_or(f64::INFINITY))
            });
            best.or(p.strategies.first()).map(|r| craft_ui::step_lines(&r.costed, rates))
        })
        .unwrap_or_default();
    Some(FlipRow {
        listed,
        seller: c.seller.clone(),
        online: matches!(c.seller_state, khaloni_poe2_core::listing::SellerState::Online),
        fix,
        resale: resale_price,
        observed,
        card,
        steps,
        resale_listings: resale.prices.iter().map(|p| rates.money(*p)).collect(),
        url,
    })
}

/// "this scan: 5 searches, 10 fetches; budget 27/30 free".
pub fn scan_statement(plan: &ScanPlan, window: SearchWindow) -> String {
    statement("this scan", plan.cost, window)
}

/// Runs a scan's searches through `fetch`: the resale search first, then
/// each relaxed search; reads the resale, costs every candidate from the
/// relaxed searches once (a listing two searches return is one candidate),
/// and ranks them by margin under the uniform model. Nothing is sent when
/// the budget refuses the scan. A relaxed search that fails is noted and
/// the scan goes on with the others; a failed resale search ends it, since
/// no margin exists without the resale.
pub fn run_scan(plan: &ScanPlan, window: SearchWindow, fetch: &mut FetchFn, inputs: &ScanInputs) -> Result<Scanned, String> {
    allow(plan.cost, window)?;
    let per_search = LISTINGS_PER_SEARCH as usize;
    let mut notes: Vec<String> = plan.relaxations.skipped.clone();
    let mut queries = plan.relaxations.queries.iter();
    let first = queries.next().ok_or_else(|| "the profile has no resale search".to_string())?;
    let resale_entries: Vec<Value> = fetch(&first.query, per_search)
        .map_err(|why| format!("the resale search failed: {why}"))?
        .into_iter()
        .flatten()
        .collect();
    let resale = resale(&resale_entries, &plan.resolved, inputs.data, inputs.market);

    let mut seen: HashSet<String> = HashSet::new();
    let mut found: Vec<(Value, &Query)> = Vec::new();
    for q in queries {
        match fetch(&q.query, per_search) {
            Ok(raw) => {
                for entry in raw.into_iter().flatten() {
                    let id = entry["id"].as_str().unwrap_or("").to_string();
                    if id.is_empty() || seen.insert(id) {
                        found.push((entry, &q.query));
                    }
                }
            }
            Err(why) => notes.push(format!("{} failed: {why}", q.label)),
        }
    }

    let mut unpriced = 0;
    let mut costed: Vec<(flip::Candidate, &Query)> = Vec::new();
    for (entry, query) in &found {
        if let Some(c) = flip::candidate(entry, &plan.resolved, &resale, inputs.data, inputs.planner, inputs.market) {
            costed.push((c, query));
        }
    }
    // Ranked as the flip finder ranks them; FlipList sorts the same way.
    let order: Vec<String> = flip::rank(costed.iter().map(|(c, _)| c.clone()).collect())
        .into_iter()
        .map(|c| c.listing_id)
        .collect();
    costed.sort_by_key(|(c, _)| order.iter().position(|id| *id == c.listing_id).unwrap_or(usize::MAX));
    let mut rows = Vec::new();
    for (c, query) in &costed {
        let url = listing_url(inputs.league, query, &c.seller);
        match flip_row(c, &resale, inputs.rates, url) {
            Some(row) => rows.push(row),
            None => unpriced += 1,
        }
    }
    if unpriced > 0 {
        notes.push(format!("{unpriced} listings left out: their price cannot be read in exalted"));
    }
    let model = match inputs.planner.observed {
        Some(m) => format!("{}; each candidate costed on {} runs", model_line(Some(m)), inputs.planner.config.runs),
        None => format!("{}; each candidate costed on {} runs", model_line(None), inputs.planner.config.runs),
    };
    let list = FlipList::new(profile_label(&plan.resolved.profile), inputs.statement.clone(), model, rows);
    Ok(Scanned { list, notes })
}

// ------------------------------------------------------------------ files

/// The flip profiles live beside the config.
pub const PROFILES_FILE: &str = "profiles.toml";

/// Where the settings window leaves a scan for the overlay to state and
/// run: the overlay owns the trade client and its rate limiter, so a scan
/// never leaves from the settings process.
pub const SCAN_REQUEST_FILE: &str = "craft-scan.request";

pub fn profiles_path(config_dir: &Path) -> PathBuf {
    config_dir.join(PROFILES_FILE)
}

/// The profiles in `path`: every one that loads, and every one that does
/// not with its reason. A missing file is no profiles and no error.
pub fn load_profiles(path: &Path) -> LoadedProfiles {
    match std::fs::read_to_string(path) {
        Ok(text) => flip::profiles_from_toml(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => LoadedProfiles::default(),
        Err(e) => LoadedProfiles {
            profiles: Vec::new(),
            errors: vec![flip::ProfileError { index: None, name: None, reason: format!("it cannot be read: {e}") }],
        },
    }
}

/// Writes the profiles, atomically, after checking each one.
pub fn save_profiles(path: &Path, profiles: &[Profile]) -> Result<(), String> {
    for (i, p) in profiles.iter().enumerate() {
        p.validate().map_err(|reason| format!("profile {} (\"{}\"): {reason}", i + 1, p.name))?;
    }
    let text = flip::to_toml(profiles)?;
    crate::config::write_atomic(path, &text).map_err(|e| format!("{} not written: {e}", path.display()))
}

/// Asks the overlay to state the cost of scanning the profile `name`.
pub fn request_scan(config_dir: &Path, name: &str) -> Result<(), String> {
    crate::config::write_atomic(&config_dir.join(SCAN_REQUEST_FILE), name)
        .map_err(|e| format!("the scan request was not written: {e}"))
}

/// The scan the settings window asked for, taken so it is acted on once.
pub fn take_scan_request(config_dir: &Path) -> Option<String> {
    let path = config_dir.join(SCAN_REQUEST_FILE);
    let text = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    let name = text.trim();
    (!name.is_empty()).then(|| name.to_string())
}
