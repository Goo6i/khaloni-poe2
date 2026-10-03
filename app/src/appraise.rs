//! The decisions behind a trade price check, kept apart from the threads and
//! channels that carry them so each one can be tested: what a page of
//! listings supports saying, when a cached answer may be reused or must be
//! asked again, how a failed load is retried, and in which order requests
//! are served. No I/O here.
//!
//! What the panel shows comes from here too: the fetched entries become the
//! listings table, the ladder, the price-fixed strip and the closest
//! listings; an exchange body becomes the bulk view; the market model gives
//! the poe.ninja block. Every figure is a listing's own price, converted
//! through the currency table; nothing is a model's output.

use std::collections::HashMap;
use std::hash::Hash;
use std::ops::Range;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use khaloni_poe2_core::bulk::BulkView;
use khaloni_poe2_core::ee2::request::Built;
use khaloni_poe2_core::listing::{self, ListingView};
use khaloni_poe2_core::market;
use khaloni_poe2_core::ninja::{Price, PriceTable};
use khaloni_poe2_core::suggest;
use khaloni_poe2_core::trade::{Listing, Query, TradeError};
use khaloni_poe2_core::value::{format_amount, pick_unit, Unit};
use serde_json::Value;

use crate::budget::Budget;
use crate::evaluate_ui::{
    AttributionRow, BulkBlock, BulkOffer, ClosestBlock, ListingRow, NinjaBlock, Panel, PriceFixedStrip, Target,
};
use crate::pricing::Denom;

// --- what the listings support ------------------------------------------

/// How much of a search the table rests on.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Sample {
    /// Listings asked for from the fetch endpoint.
    pub requested: usize,
    /// Of those, how many converted to exalted.
    pub priced: usize,
    /// Every match of the search, when the site reported it.
    pub total: Option<u64>,
    /// Price currencies the table could not convert, for the status line.
    pub unpriced_currencies: Vec<String>,
}

/// Counts what a page of listings converts to. `dropped` is
/// `FetchOutcome::dropped` (listings that came back gone or without a
/// price); `to_exalted` converts one listing or says it cannot.
pub fn appraise(listings: &[Listing], dropped: usize, total: Option<u64>, to_exalted: &dyn Fn(&Listing) -> Option<f64>) -> Sample {
    let mut priced = 0;
    let mut unpriced: Vec<String> = Vec::new();
    for l in listings {
        match to_exalted(l).filter(|v| v.is_finite() && *v > 0.0) {
            Some(_) => priced += 1,
            None => {
                if !unpriced.contains(&l.price_currency) {
                    unpriced.push(l.price_currency.clone());
                }
            }
        }
    }
    Sample { requested: listings.len() + dropped, priced, total, unpriced_currencies: unpriced }
}

/// A price in exalted: directly, or through the currency table by the
/// display name the trade site's currency id stands for. None for an id
/// with no name or a name with no rate: nothing is guessed at.
pub fn price_exalted(amount: f64, currency: &str, currency_names: &HashMap<String, String>, table: &PriceTable) -> Option<f64> {
    if currency == "exalted" {
        return Some(amount);
    }
    currency_names.get(currency).and_then(|name| table.lookup(name)).map(|p| amount * p.exalted)
}

/// [`price_exalted`] for a priced-summary listing.
pub fn listing_exalted(l: &Listing, currency_names: &HashMap<String, String>, table: &PriceTable) -> Option<f64> {
    price_exalted(l.price_amount, &l.price_currency, currency_names, table)
}

/// "7 of 10 priced", and the size of the whole result when it is known.
pub fn sample_text(s: &Sample) -> String {
    let mut out = format!("{} of {} priced", s.priced, s.requested);
    if let Some(total) = s.total.filter(|t| *t > s.requested as u64) {
        out.push_str(&format!(", cheapest {} of {total}", s.requested));
    }
    out
}

// --- what one check fetches ----------------------------------------------

/// Listings per fetch call: the endpoint answers ten ids at a time.
pub const PAGE: usize = 10;
/// The table's size, EE2's number: two fetch calls.
pub const TABLE_LISTINGS: usize = 20;
/// The most a check fetches, for the closest-listings comparison, and
/// only when the search matched more than the table holds: four calls,
/// which the fetch policy allows within its 12 per 4 s.
pub const CLOSEST_LISTINGS: usize = 40;

/// The fetch calls one check makes, as index ranges over the search's
/// ids: the table's twenty in pages of ten, and up to forty when the
/// closest listings are wanted and the search matched more than twenty
/// (with twenty or fewer, the table already holds every match). Never a
/// page past the ids the search returned, and never an empty one.
pub fn fetch_plan(total: Option<u64>, hashes: usize, wants_closest: bool) -> Vec<Range<usize>> {
    let deep = wants_closest && total.is_some_and(|t| t > TABLE_LISTINGS as u64);
    let want = if deep { CLOSEST_LISTINGS } else { TABLE_LISTINGS }.min(hashes);
    (0..want).step_by(PAGE).map(|from| from..(from + PAGE).min(want)).collect()
}

/// Whether a check compares the listings' mods with the item's: a rare or
/// magic with mods the search reads (EE2's pseudo preset), which is what
/// the closest-listings block is about. A unique, a currency or a gem is
/// priced by name and the comparison would say nothing.
pub fn wants_closest(built: &Built) -> bool {
    built.preset == "filters.preset_pseudo" && !suggest::ours_from_built(built).is_empty()
}

// --- the budget line ------------------------------------------------------

/// Within this many of the cap the budget text turns red.
pub const BUDGET_LOW_WITHIN: u32 = 5;
/// The attribution button needs this many free search slots: three
/// searches with their fetches, and the user's next check after them.
pub const ATTRIBUTE_MIN_FREE: u32 = 8;
/// The rule window the budget line reads: the five-minute rule of the
/// search policy (`30:300:1800` live), the one that says something about
/// the next few minutes rather than a ten-second burst.
pub const BUDGET_WINDOW_S: u32 = 300;

/// The five-minute rule's counters as one response line of the request
/// log reports them ("... policy=trade-search-request-limit ip 1/5 (10s),
/// 3/15 (60s), 12/30 (300s); account 2/45 (300s)"): the used and max of
/// every family's rule over [`BUDGET_WINDOW_S`], the fullest family
/// counting. None for a line of another endpoint, or one without such a
/// rule. The log line is the server's own state, which is what the panel
/// is meant to show.
pub fn search_counters(line: &str) -> Option<(u32, u32)> {
    let rest = line.strip_prefix("trade response: ")?;
    let (endpoint, rest) = rest.split_once(' ')?;
    if endpoint != "search" {
        return None;
    }
    let window = format!("({BUDGET_WINDOW_S}s)");
    let mut best: Option<(u32, u32)> = None;
    for cell in rest.split([',', ';']) {
        let cell = cell.trim();
        // A family's first cell carries its name: "ip 1/5 (10s)".
        let cell = cell.rsplit(' ').take(2).collect::<Vec<_>>();
        let [w, pair] = cell[..] else { continue };
        if w != window {
            continue;
        }
        let (used, max) = pair.split_once('/')?;
        let (used, max) = (used.parse::<u32>().ok()?, max.parse::<u32>().ok()?);
        if best.is_none_or(|(u, m)| max.saturating_sub(used) < m.saturating_sub(u)) {
            best = Some((used, max));
        }
    }
    best
}

/// "searches 4/30 (5 min)", and whether it is within [`BUDGET_LOW_WITHIN`]
/// of the cap.
pub fn budget_line(used: u32, max: u32) -> (String, bool) {
    (format!("searches {used}/{max} (5 min)"), max.saturating_sub(used) <= BUDGET_LOW_WITHIN)
}

/// Whether the attribution button may be pressed.
pub fn attribute_enabled(free_slots: u32) -> bool {
    free_slots >= ATTRIBUTE_MIN_FREE
}

/// The five-minute counters of the last search response this process
/// saw, kept by [`note_request_line`] from the request log.
static LAST_SEARCH: std::sync::Mutex<Option<(u32, u32)>> = std::sync::Mutex::new(None);

/// Reads a request-log line: a search response's counters are kept for
/// the panel's budget line. Every line goes to the caller's log as well.
pub fn note_request_line(line: &str) {
    if let Some(counters) = search_counters(line) {
        *LAST_SEARCH.lock().unwrap_or_else(|e| e.into_inner()) = Some(counters);
    }
}

/// The budget line from the last search response, or the limiter's own
/// reading (`fallback`, "search 0/5 (10s)") before any search has
/// answered.
pub fn budget_text(fallback: &str) -> (String, bool) {
    match *LAST_SEARCH.lock().unwrap_or_else(|e| e.into_inner()) {
        Some((used, max)) => budget_line(used, max),
        None => (fallback.to_string(), false),
    }
}

// --- the price-fixed re-search and what each mod is worth ---------------

/// The trade site's price option that keeps only listings priced in
/// exalted or divine orbs, the way EE2's price-fixed button re-searches.
pub const HONEST_PRICES: &str = "exalted_divine";

/// The same search, priced in exalted and divine only: what the
/// price-fixed strip's button runs.
pub fn price_fixed_query(q: &Query) -> Query {
    Query { price_option: Some(HONEST_PRICES.to_string()), ..q.clone() }
}

/// The strongest ticked mods of the card, up to three, each with the
/// search that goes without it. A mod is a ticked row that drives a filter
/// of `searched`, the query the card's listings came from: one of EE2's
/// rows, or a line EE2 gave no row that was ticked into the search. Rows
/// with a tier badge rank first, best tier then best roll, named as the
/// attribution block names them ("T1 life"); rows without one follow in the
/// card's order, named by their stat. Without one of
/// EE2's rows is `searched` with that filter switched off; without a ticked
/// line is `searched` with that line's filter taken out, so every search
/// differs from the one on the card by exactly one filter. `ee2_filters` is
/// how many of `searched`'s filters are EE2's; the ticked lines follow them.
pub fn strongest_mods(panel: &Panel, searched: &Query, ee2_filters: usize) -> Vec<(String, Query)> {
    // Every ticked row is a candidate: those with a tier badge first, best
    // tier then best roll, then the rest (totals, implicits, a unique's
    // lines) in the card's order, since nothing ranks them against each
    // other; the three-search cap holds either way.
    let mut rows: Vec<(u8, f32, String, usize)> = panel
        .rows
        .iter()
        .filter(|r| r.enabled)
        .filter_map(|r| {
            let at = match r.target? {
                Target::Stat(fi) => fi,
                // The line's filter as the search carries it: the same stat,
                // its bounds perhaps relaxed. A line ticked after the search
                // is not in it, and has nothing to go without.
                Target::Extra(xi) => {
                    let f = panel.extras.get(xi)?;
                    let tail = searched.filters.get(ee2_filters..)?;
                    ee2_filters + tail.iter().position(|s| !s.disabled && s.id == f.id && s.alt_ids == f.alt_ids)?
                }
                Target::Equipment(_) => return None,
            };
            let name = suggest::short_name(&suggest::mod_key(&r.label));
            Some(match r.badge {
                Some(badge) => (badge.tier, r.score.unwrap_or(0.0), format!("T{} {name}", badge.tier), at),
                None => (u8::MAX, 0.0, name, at),
            })
        })
        .collect();
    // A stable sort keeps the unbadged rows in the card's order.
    rows.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.total_cmp(&a.1)));
    // Two unrevealed lines of one kind are one filter in the search, and
    // are searched without once.
    let mut chosen: Vec<(String, usize)> = Vec::new();
    for (_, _, label, at) in rows {
        if chosen.len() < 3 && !chosen.iter().any(|(_, c)| *c == at) {
            chosen.push((label, at));
        }
    }
    chosen
        .into_iter()
        .map(|(label, at)| {
            let without = if at < ee2_filters {
                without_filter(searched, at)
            } else {
                let mut q = searched.clone();
                q.filters.remove(at);
                q
            };
            (label, without)
        })
        .collect()
}

/// The search with one filter switched off: what a mod's absence costs.
pub fn without_filter(q: &Query, filter: usize) -> Query {
    let mut out = q.clone();
    if let Some(f) = out.filters.get_mut(filter) {
        f.disabled = true;
    }
    out
}

/// The attribution rows from the baseline's cheapest listing and each
/// dropped search's: `dropped` is (mod label, the cheapest price and its
/// seller when that search matched something).
pub fn attribution_rows(
    baseline: (f64, &str),
    dropped: &[(String, Option<(f64, String)>)],
    unit: &str,
) -> Vec<AttributionRow> {
    let figures: Vec<(String, Option<f64>)> = dropped.iter().map(|(l, w)| (l.clone(), w.as_ref().map(|(p, _)| *p))).collect();
    suggest::attribution(baseline.0, &figures, unit)
        .into_iter()
        .zip(dropped)
        .map(|(a, (_, without))| AttributionRow {
            label: a.label,
            with: format!("{} {unit}", suggest::price_text(a.with)),
            without: match without {
                Some((p, _)) => format!("{} {unit}", suggest::price_text(*p)),
                None => "nothing matched".to_string(),
            },
            text: match without {
                Some((p, seller)) => format!(
                    "with: {} {} {unit}; without: {seller} {} {unit}",
                    baseline.1,
                    suggest::price_text(a.with),
                    suggest::price_text(*p)
                ),
                None => format!("with: {} {} {unit}; without it nothing matched", baseline.1, suggest::price_text(a.with)),
            },
        })
        .collect()
}

// --- the blocks under the card -------------------------------------------

/// The currency the ladder and the closest listings are read in: the
/// panel's display rule applied to one price, with the factor that takes
/// an exalted figure into it.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayUnit {
    pub name: &'static str,
    /// Multiplies a price in exalted.
    pub per_exalted: f64,
}

impl DisplayUnit {
    /// The unit `denom_amount` would show `exalted` in.
    pub fn for_exalted(exalted: f64, table: &PriceTable, divine_threshold: f64) -> DisplayUnit {
        let (unit, _, _) = pick_unit(&table.price_from_exalted(exalted), 1, divine_threshold);
        match unit {
            Unit::Divine if table.exalted_per_divine > 0.0 => DisplayUnit { name: "div", per_exalted: 1.0 / table.exalted_per_divine },
            Unit::Chaos if table.exalted_per_divine > 0.0 => {
                DisplayUnit { name: "chaos", per_exalted: table.chaos_per_divine / table.exalted_per_divine }
            }
            _ => DisplayUnit { name: "ex", per_exalted: 1.0 },
        }
    }

    pub fn convert(&self, exalted: f64) -> f64 {
        exalted * self.per_exalted
    }
}

/// One search's fetched entries and everything needed to read them.
pub struct Fetched<'a> {
    /// The entries as received, in the search's order (`FetchOutcome::raw`
    /// over every page), `None` where the API sent null.
    pub raw: &'a [Option<Value>],
    pub total: Option<u64>,
    pub now_unix: i64,
    pub my_account: &'a str,
    /// Trade currency id -> the display name the price table knows.
    pub currency_names: &'a HashMap<String, String>,
    pub table: &'a PriceTable,
    pub divine_threshold: f64,
    /// The checked item as the search was built from it, for the
    /// closest-listings comparison; None prices by name (a unique, a gem).
    pub built: Option<&'a Built>,
}

/// What one search puts under the card.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Blocks {
    /// The table: the first [`TABLE_LISTINGS`] entries, folded.
    pub listings: Vec<ListingRow>,
    pub ladder: String,
    pub price_fixed: Option<PriceFixedStrip>,
    pub closest: Option<ClosestBlock>,
    /// The unit the ladder and the closest listings are in, and what
    /// multiplies an exalted figure into it.
    pub unit: String,
    pub unit_per_exalted: f64,
    /// The cheapest table listing in that unit, and its seller: the
    /// baseline the attribution searches compare against.
    pub cheapest: Option<(f64, String)>,
    /// Table rows shown, and entries that came back gone or unpriced.
    pub shown: usize,
    pub dropped: usize,
    pub total: Option<u64>,
}

fn view_exalted(v: &ListingView, f: &Fetched) -> Option<f64> {
    v.price.as_ref().and_then(|(amount, currency)| price_exalted(*amount, currency, f.currency_names, f.table))
}

/// The blocks for one search, from its fetched entries.
pub fn blocks(f: &Fetched) -> Blocks {
    let mut dropped = 0;
    let views: Vec<ListingView> = f
        .raw
        .iter()
        .filter_map(|entry| {
            let view = entry.as_ref().and_then(|v| listing::parse_entry(v, f.now_unix, f.my_account));
            if view.as_ref().is_none_or(|v| v.price.is_none()) {
                dropped += 1;
            }
            view
        })
        .collect();
    let table_views: Vec<ListingView> = views.iter().take(TABLE_LISTINGS).cloned().collect();
    let grouped = listing::group(table_views);
    // The unit follows the cheapest listing the table can convert.
    let unit = grouped
        .iter()
        .filter_map(|g| view_exalted(&g.view, f))
        .min_by(f64::total_cmp)
        .map(|ex| DisplayUnit::for_exalted(ex, f.table, f.divine_threshold))
        .unwrap_or(DisplayUnit { name: "ex", per_exalted: 1.0 });
    let listings: Vec<ListingRow> = grouped
        .iter()
        .map(|g| ListingRow::priced(g, view_exalted(&g.view, f), f.table, f.divine_threshold))
        .collect();
    let priced: Vec<(f64, String)> =
        grouped.iter().filter_map(|g| Some((unit.convert(view_exalted(&g.view, f)?), g.view.seller.clone()))).collect();
    let prices: Vec<f64> = priced.iter().map(|(p, _)| *p).collect();
    let ladder = suggest::ladder_text(&prices, f.total, unit.name);
    let cheapest = priced.iter().cloned().min_by(|a, b| a.0.total_cmp(&b.0));
    let price_fixed = listing::price_fixed(&grouped).map(|p| PriceFixedStrip {
        text: format!(
            "likely price-fixed: {} listings under {}, next at {}",
            p.count,
            p.under_currency.as_deref().unwrap_or("?"),
            p.next_price.as_ref().map(|(a, c)| format!("{} {c}", format_amount(*a))).unwrap_or_else(|| "none".into())
        ),
        button: "ex/div only".to_string(),
    });
    let closest = f.built.filter(|b| wants_closest(b)).map(|built| {
        let ours = suggest::ours_from_built(built);
        let candidates: Vec<(ListingView, f64)> =
            views.iter().filter_map(|v| Some((v.clone(), unit.convert(view_exalted(v, f)?)))).collect();
        let c = suggest::closest(&ours, &candidates, unit.name);
        ClosestBlock {
            lines: vec![c.text],
            nearest: c.nearest.map(|(i, _, _)| {
                let (v, price) = &candidates[i];
                format!("nearest listing: {} {} by {}, {}", suggest::price_text(*price), unit.name, v.seller, v.age_text)
            }),
        }
    });
    Blocks {
        shown: listings.len(),
        listings,
        ladder,
        price_fixed,
        closest,
        unit: unit.name.to_string(),
        unit_per_exalted: unit.per_exalted,
        cheapest,
        dropped,
        total: f.total,
    }
}

/// The poe.ninja block as the panel words it, from the market model's
/// line: the price in the panel's currency, the band and direction in the
/// market view's words (with a plain "+/-": the overlay face has no
/// plus-minus glyph), the traded volume, and the note.
pub fn ninja_view(block: &market::NinjaBlock, table: &PriceTable, divine_threshold: f64) -> NinjaBlock {
    let price = Price {
        divine: block.price_div,
        exalted: block.price_div * table.exalted_per_divine,
        chaos: block.price_div * table.chaos_per_divine,
    };
    let (denom, amount) = crate::pricing::denom_amount(&price, 1, divine_threshold);
    let unit = match denom {
        Denom::Divine => "div",
        Denom::Chaos => "chaos",
        _ => "ex",
    };
    let direction = market::direction_text(block.trend.as_ref()).to_string();
    // The market view's note opens with the direction word for a trusted
    // item; the price line already carries it here, so the note keeps the
    // rest: how many days it rests on, "thin market", the young-league
    // caveat, why an item is untrusted.
    let note: Vec<&str> = block.note.split(" · ").filter(|part| *part != direction && !part.is_empty()).collect();
    NinjaBlock {
        price: format!("{amount} {unit}"),
        band: block.trend.map(|t| market::band_text_ascii(t.band)).unwrap_or_default(),
        direction,
        volume: block.volume_text.clone(),
        note: note.join(" · "),
    }
}

/// The market model for a check: the price service's overviews under the
/// same floors and young-league rule the market view applies.
pub fn market_model(m: &crate::prices::Market, floors: market::Floors, now: i64) -> market::Model {
    let young = market::league_is_young(now, None, m.first_record, m.source.history_days());
    market::build(&m.source, &market::Options { floors, young })
}

/// The exchange offers as the bulk view shows them: what the seller gives
/// and wants, by the currency table's names when the catalog knows them,
/// stock, seller; and the going rate the popup quotes, so the two agree.
pub fn bulk_block(view: &BulkView, currency_names: &HashMap<String, String>) -> BulkBlock {
    let name = |id: &str| currency_names.get(id).cloned().unwrap_or_else(|| id.to_string());
    let offers = view
        .offers
        .iter()
        .map(|o| BulkOffer {
            have: format!("{} {}", format_amount(o.have.1), name(&o.have.0)),
            want: format!("{} {}", format_amount(o.want.1), name(&o.want.0)),
            stock: o.stock.map(|n| n.to_string()).unwrap_or_else(|| "?".into()),
            seller: o.seller.clone(),
            state: o.state,
        })
        .collect();
    let note = match (view.median_rate(), view.offers.first()) {
        (Some(rate), Some(first)) => format!(
            "{} offers, cheapest first; median of the cheapest {}: {} {} each",
            view.offers.len(),
            view.offers.len().min(5),
            format_amount(rate),
            name(&first.want.0)
        ),
        _ => "no offers right now".to_string(),
    };
    BulkBlock { offers, note }
}

/// "37 x 1.8 chaos = 67 chaos": a stack's worth from the per-unit price in
/// exalted, both figures in the unit the total is shown in.
pub fn stack_value_text(count: u32, per_unit_exalted: f64, table: &PriceTable, divine_threshold: f64) -> String {
    let (unit, total, each) = pick_unit(&table.price_from_exalted(per_unit_exalted), count, divine_threshold);
    format!("{count} x {} {} = {} {}", format_amount(each), unit.suffix(), format_amount(total), unit.suffix())
}

// --- exchange quotes -----------------------------------------------------

/// The `have` currencies an exchange price is asked in, in order, with the
/// price-table name that converts each to exalted. Most stackables are
/// offered for exalted; the expensive ones only for chaos or divine, and
/// asking in exalted alone reported them as having no price.
pub const EXCHANGE_HAVE: [(&str, &str); 3] =
    [("exalted", "Exalted Orb"), ("chaos", "Chaos Orb"), ("divine", "Divine Orb")];

/// Whether a currency row must go to the exchange at all. `table_price` is
/// what the poe.ninja table says one unit is worth in exalted. A positive
/// price is the answer, and the exchange is not asked: it costs a search
/// slot and says nothing the table did not. Only a currency the table
/// lacks, or prices at nothing, is worth a request.
pub fn needs_exchange(table_price_exalted: Option<f64>) -> bool {
    !table_price_exalted.is_some_and(|p| p.is_finite() && p > 0.0)
}

/// Exalted per unit from the first `have` currency with offers. `ask`
/// performs one exchange query; `exalted_per` gives how many exalted one
/// unit of a table name is worth. An error stops the walk: falling through
/// to the next currency after a cooldown would spend another request just
/// to be refused again.
pub fn exchange_with_fallback<E>(
    ask: &mut dyn FnMut(&str) -> Result<Option<f64>, E>,
    exalted_per: &dyn Fn(&str) -> Option<f64>,
) -> Result<Option<f64>, E> {
    for (have, name) in EXCHANGE_HAVE {
        let rate = if have == "exalted" { Some(1.0) } else { exalted_per(name) };
        // No conversion for this currency: its offers could not be shown in
        // exalted anyway, so the request is not made.
        let Some(rate) = rate.filter(|r| r.is_finite() && *r > 0.0) else { continue };
        if let Some(per_unit) = ask(have)? {
            return Ok(Some(per_unit * rate));
        }
    }
    Ok(None)
}

/// A trade error in words fit for a popup line. The cooldown gets whole
/// seconds instead of the Debug duration ("retry in 32.847s").
pub fn error_text(e: &TradeError) -> String {
    match e {
        TradeError::Cooldown(d) => format!("trade cooldown {}s", d.as_secs().max(1)),
        other => other.to_string(),
    }
}

/// How long a failed lookup waits before it may be asked again. A refusal
/// waits for [`UNTIL_INPUTS_CHANGE`]: the site answers the same request
/// the same way, however long it waits.
pub fn retry_after(e: &TradeError) -> Duration {
    match e {
        TradeError::Cooldown(d) => *d + Duration::from_secs(1),
        TradeError::Refused(_) => UNTIL_INPUTS_CHANGE,
        _ => ERROR_RETRY,
    }
}

// --- answers that are asked for once and read many times ----------------

/// A good answer is served for this long, then asked again: a rune priced
/// at session start is not its price four hours later.
pub const GOOD_TTL: Duration = Duration::from_secs(10 * 60);
/// A failed lookup is asked again after this long (a cooldown names its
/// own wait, see [`retry_after`]).
pub const ERROR_RETRY: Duration = Duration::from_secs(30);
/// The wait of a failure no clock ends, only new inputs: the cache keeps
/// its reason and asks again after [`AsyncCache::inputs_changed`] or
/// [`AsyncCache::clear`].
pub const UNTIL_INPUTS_CHANGE: Duration = Duration::MAX;
/// A request nobody answered (the worker was busy past this, or is gone)
/// is sent again.
pub const PENDING_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Debug, Clone, PartialEq)]
enum Entry<V> {
    Pending { since: Instant, last: Option<(V, Instant)> },
    Good { value: V, at: Instant },
    /// `retry_at` is None for a failure that waits for new inputs.
    Failed { why: String, retry_at: Option<Instant>, last: Option<(V, Instant)> },
}

/// What a lookup found.
#[derive(Debug, Clone, PartialEq)]
pub struct Found<V> {
    /// The newest good answer, if there is one young enough to show. It
    /// keeps being served while a refresh is in flight so a row does not
    /// blink to "...", but only up to twice the TTL: refreshes that keep
    /// failing must not leave an hours-old price looking current.
    pub value: Option<V>,
    /// The caller must send a request now; the entry is marked pending.
    pub request: bool,
    /// Why the last attempt failed, while it has not been retried yet.
    pub error: Option<String>,
}

/// Cache for answers that arrive later from a worker. Errors are never an
/// answer: they are remembered only long enough not to hammer the API.
#[derive(Debug)]
pub struct AsyncCache<K, V> {
    map: HashMap<K, Entry<V>>,
    ttl: Duration,
}

impl<K: Eq + Hash + Clone, V: Clone> Default for AsyncCache<K, V> {
    fn default() -> Self {
        AsyncCache::new(GOOD_TTL)
    }
}

impl<K: Eq + Hash + Clone, V: Clone> AsyncCache<K, V> {
    pub fn new(ttl: Duration) -> Self {
        AsyncCache { map: HashMap::new(), ttl }
    }

    pub fn lookup(&mut self, key: &K, now: Instant) -> Found<V> {
        let (last, request, error) = match self.map.get(key) {
            None => (None, true, None),
            Some(Entry::Good { value, at }) => (Some((value.clone(), *at)), now >= *at + self.ttl, None),
            Some(Entry::Pending { since, last }) => (last.clone(), now >= *since + PENDING_TIMEOUT, None),
            Some(Entry::Failed { why, retry_at, last }) => {
                let due = retry_at.is_some_and(|at| now >= at);
                (last.clone(), due, (!due).then(|| why.clone()))
            }
        };
        if request {
            self.map.insert(key.clone(), Entry::Pending { since: now, last: last.clone() });
        }
        let value = last.filter(|(_, at)| now < *at + self.ttl * 2).map(|(v, _)| v);
        Found { value, request, error }
    }

    /// A request could not be sent: forget that it is pending so the next
    /// lookup asks again.
    /// Forgets every answer and every request in flight: what was asked
    /// in one league says nothing about another.
    pub fn clear(&mut self) {
        self.map.clear();
    }

    pub fn unsent(&mut self, key: &K) {
        if matches!(self.map.get(key), Some(Entry::Pending { .. })) {
            self.map.remove(key);
        }
    }

    /// What the requests were built from changed (a price table refresh,
    /// a catalog reload): a request the site refused may now be a
    /// different request, so refusals are asked again. Answers and
    /// ordinary failures keep their own clocks.
    pub fn inputs_changed(&mut self) {
        self.map.retain(|_, e| !matches!(e, Entry::Failed { retry_at: None, .. }));
    }

    pub fn store(&mut self, key: K, outcome: Result<V, (String, Duration)>, now: Instant) {
        let last = match self.map.get(&key) {
            Some(Entry::Pending { last, .. }) | Some(Entry::Failed { last, .. }) => last.clone(),
            Some(Entry::Good { value, at }) => Some((value.clone(), *at)),
            None => None,
        };
        let entry = match outcome {
            Ok(value) => Entry::Good { value, at: now },
            // A wait past what an Instant can hold is one no clock ends.
            Err((why, wait)) => Entry::Failed { why, retry_at: now.checked_add(wait), last },
        };
        self.map.insert(key, entry);
    }
}

/// The last answer to one kind of question, reusable for a short while when
/// the very same question comes again (F7 twice on one item, Search twice
/// on unchanged boxes). Only good answers are kept.
#[derive(Debug)]
pub struct ReuseSlot<K, V> {
    held: Option<(K, V, Instant)>,
    ttl: Duration,
}

/// How long an identical check reuses the previous result.
pub const REUSE_TTL: Duration = Duration::from_secs(30);

impl<K: PartialEq, V: Clone> ReuseSlot<K, V> {
    pub fn new(ttl: Duration) -> Self {
        ReuseSlot { held: None, ttl }
    }

    pub fn get(&self, key: &K, now: Instant) -> Option<(V, Duration)> {
        let (k, v, at) = self.held.as_ref()?;
        let age = now.saturating_duration_since(*at);
        (k == key && age < self.ttl).then(|| (v.clone(), age))
    }

    pub fn put(&mut self, key: K, value: V, now: Instant) {
        self.held = Some((key, value, now));
    }

    pub fn clear(&mut self) {
        self.held = None;
    }
}

// --- loads that may fail and are tried again -----------------------------

/// Waits between attempts at loading something a price check needs.
const LOAD_BACKOFF_S: [u64; 5] = [0, 5, 15, 60, 300];

/// A value loaded on first use and kept once it loads. A failed load is
/// retried on a later use, after a growing wait, instead of being final for
/// the run: the first check after a network blip used to disable trade
/// pricing until restart.
#[derive(Debug)]
pub struct Retrying<T> {
    value: Option<T>,
    failures: usize,
    next_attempt: Option<Instant>,
    last_error: String,
}

impl<T> Default for Retrying<T> {
    fn default() -> Self {
        Retrying { value: None, failures: 0, next_attempt: None, last_error: String::new() }
    }
}

impl<T> Retrying<T> {
    pub fn get(&self) -> Option<&T> {
        self.value.as_ref()
    }

    pub fn get_mut(&mut self) -> Option<&mut T> {
        self.value.as_mut()
    }

    pub fn is_loaded(&self) -> bool {
        self.value.is_some()
    }

    /// Why the last load failed; empty before the first failure.
    pub fn last_error(&self) -> &str {
        &self.last_error
    }

    /// True when the value is missing and a load may be tried now. Callers
    /// that load in the background check this first, so waiting out a
    /// failure is silent instead of one log line per look.
    pub fn attempt_due(&self, now: Instant) -> bool {
        self.value.is_none() && self.next_attempt.is_none_or(|at| now >= at)
    }

    /// The value, loading it when it is missing and an attempt is due.
    /// While waiting out a failure the error names the wait.
    pub fn get_or_load(
        &mut self,
        now: Instant,
        load: impl FnOnce() -> Result<T, String>,
    ) -> Result<&mut T, String> {
        if self.value.is_none() {
            if let Some(at) = self.next_attempt.filter(|at| now < *at) {
                let wait = at.saturating_duration_since(now).as_secs().max(1);
                return Err(format!("{} (next try in {wait}s)", self.last_error));
            }
            match load() {
                Ok(v) => {
                    self.value = Some(v);
                    self.failures = 0;
                    self.next_attempt = None;
                }
                Err(e) => {
                    self.failures += 1;
                    let step = LOAD_BACKOFF_S[self.failures.min(LOAD_BACKOFF_S.len() - 1)];
                    self.next_attempt = Some(now + Duration::from_secs(step));
                    self.last_error = e.clone();
                    return Err(e);
                }
            }
        }
        Ok(self.value.as_mut().expect("loaded above"))
    }
}

/// The trade client for the next request, loaded by `load` on first use,
/// carrying `session` as saved now: a POESESSID entered or cleared in
/// settings takes effect on the next search, with no restart.
pub fn session_client<'a>(
    slot: &'a mut Retrying<khaloni_poe2_core::trade::TradeClient>,
    session: &str,
    now: Instant,
    load: impl FnOnce() -> Result<khaloni_poe2_core::trade::TradeClient, String>,
) -> Result<&'a mut khaloni_poe2_core::trade::TradeClient, String> {
    let client = slot.get_or_load(now, load)?;
    client.set_session(session);
    Ok(client)
}

// --- which reward rows may ask at all -------------------------------------

/// What a reward row would need a trade request for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    /// A currency the poe.ninja table lacks: one exchange query.
    Currency,
    /// A specific gem: one item search.
    Gem,
}

/// Whether reward rows may price gems through a trade search. It is the
/// `price_gem_rows` setting, off by default: a panel of gem rewards is a
/// dozen searches nobody asked for.
pub fn gem_rows_enabled(cfg_flag: bool) -> bool {
    cfg_flag
}

/// Whether a reward row's request may be sent. A gem row is refused
/// outright while gem rows are off, before the budget is even consulted, so
/// it neither spends nor counts; anything else is the budget's call
/// (`allow` is `Budget::allow_background` with the caller's clock and slot
/// count). A refusal drops the request: the row shows "..." and a later
/// scan asks again.
pub fn background_request(kind: RowKind, cfg_gem_rows: bool, allow: impl FnOnce() -> bool) -> bool {
    if kind == RowKind::Gem && !gem_rows_enabled(cfg_gem_rows) {
        return false;
    }
    allow()
}

/// [`background_request`] with the budget's own answer: whether a reward
/// row's request goes now, given the free search slots the limiter
/// reports. A refusal counts nothing; the row stays unpriced.
pub fn row_request(kind: RowKind, cfg_gem_rows: bool, budget: &mut Budget, now: Instant, free_slots: u32) -> bool {
    row_decision(kind, cfg_gem_rows, budget, now, free_slots).0
}

/// [`row_request`] with the line the log gets either way: what the row
/// wanted, whether it goes, and the numbers the decision rested on. A
/// refused row would otherwise leave nothing to read behind a "..." that
/// never fills.
pub fn row_decision(
    kind: RowKind,
    cfg_gem_rows: bool,
    budget: &mut Budget,
    now: Instant,
    free_slots: u32,
) -> (bool, String) {
    let allowed = background_request(kind, cfg_gem_rows, || budget.allow_background(now, free_slots, None));
    let what = match kind {
        RowKind::Currency => "currency row (exchange)",
        RowKind::Gem => "gem row (search)",
    };
    let verdict = match (allowed, kind) {
        (true, _) => "sent",
        (false, RowKind::Gem) if !gem_rows_enabled(cfg_gem_rows) => "not sent: gem rows are off",
        (false, _) => "not sent: the budget has no room",
    };
    let line = format!(
        "reward {what} {verdict}; {free_slots} free search slots, {} background sent in 5 min",
        budget.sent_in_window(now)
    );
    (allowed, line)
}

// --- whose request goes first --------------------------------------------

/// Sender half of [`priority_channel`].
pub struct PrioritySender<T> {
    user: mpsc::Sender<T>,
    background: mpsc::Sender<T>,
}

impl<T> Clone for PrioritySender<T> {
    fn clone(&self) -> Self {
        PrioritySender { user: self.user.clone(), background: self.background.clone() }
    }
}

impl<T> PrioritySender<T> {
    /// Something the user is waiting on (a price check, a Search press).
    pub fn send_user(&self, t: T) -> Result<(), mpsc::SendError<T>> {
        self.user.send(t)
    }

    /// Something a scan asked for (a reward row's gem or currency).
    pub fn send_background(&self, t: T) -> Result<(), mpsc::SendError<T>> {
        self.background.send(t)
    }
}

pub struct PriorityReceiver<T> {
    user: mpsc::Receiver<T>,
    background: mpsc::Receiver<T>,
}

/// Two queues served as one: everything the user asked for goes before
/// anything a scan asked for. A reward panel full of unknown gems used to
/// queue a dozen rate-limited searches ahead of the user's own F7.
pub fn priority_channel<T>() -> (PrioritySender<T>, PriorityReceiver<T>) {
    let (user, user_rx) = mpsc::channel();
    let (background, background_rx) = mpsc::channel();
    (PrioritySender { user, background }, PriorityReceiver { user: user_rx, background: background_rx })
}

impl<T> PriorityReceiver<T> {
    /// The next request, or `None` once every sender is gone.
    pub fn recv(&self) -> Option<T> {
        loop {
            match self.recv_timeout(Duration::from_secs(3600)) {
                Ok(t) => return Some(t),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return None,
            }
        }
    }

    /// The next request, waiting at most `max` for one: `Timeout` lets the
    /// worker use a quiet moment (retrying a failed load), `Disconnected`
    /// means every sender is gone.
    pub fn recv_timeout(&self, max: Duration) -> Result<T, mpsc::RecvTimeoutError> {
        const POLL: Duration = Duration::from_millis(50);
        let deadline = Instant::now() + max;
        loop {
            match self.user.try_recv() {
                Ok(t) => return Ok(t),
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    return self.background.try_recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected);
                }
            }
            if let Ok(t) = self.background.try_recv() {
                return Ok(t);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(mpsc::RecvTimeoutError::Timeout);
            }
            // Nothing queued: sleep on the user queue, so a press is
            // picked up the moment it arrives.
            match self.user.recv_timeout(POLL.min(left)) {
                Ok(t) => return Ok(t),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return self.background.try_recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected);
                }
            }
        }
    }
}
