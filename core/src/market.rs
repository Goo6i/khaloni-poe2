//! The market view: which items are moving, whether the move is real, and
//! where value trades among the league mechanics. Pure; no I/O.
//!
//! One rule governs everything here: when the data cannot support a number,
//! the model carries no number. A missing day is `None`, never zero; an
//! item too thinly traded to rank carries a grade that keeps it out of
//! every ranking and index; a group whose usable items hold too little of
//! its volume has no index, and says how much they held.
//!
//! All trends and indices are in divines, the overviews' primary currency:
//! the exalted orb loses value across a league, so a price pinned in
//! exalted reads as a fall in divines and the other way round.

use crate::ninja::{ExchangeOverview, ItemOverview, Price, Sparkline};

/// Days in poe.ninja's sparkline.
pub const DAYS: usize = 7;
/// Fewest days a trend may be read from. Not 7: a source outage can take
/// the same days from every line of a category (2026-09: all 695 uniques
/// carried 5 of 7), and a rule asking for all of them silences the lot.
pub const MIN_POINTS: usize = 5;
/// An index is a statement about a group; one or two items are not one.
pub const INDEX_MIN_ITEMS: usize = 3;
/// Share of the group's traded volume the basket must hold for its index
/// to speak for the group.
pub const INDEX_MIN_COVERAGE: f64 = 0.6;
/// No rankings in a league's first days: prices have not formed yet.
pub const YOUNG_LEAGUE_SECS: i64 = 72 * 3600;
/// A price table older than this is drawn greyed.
pub const GREY_AFTER_SECS: i64 = 3 * 3600;
/// An index inside this many percent of zero is called flat.
pub const FLAT_INDEX_PCT: f64 = 1.0;
/// Prices arrive rounded to four significant digits, so exactly one
/// exalted can come back as 1.0001 exalted.
const FLOOR_ROUNDING: f64 = 5e-4;

/// Categories whose items drop from one piece of league content. The other
/// exchange categories are markets, not mechanics.
pub const MECHANICS: [&str; 9] =
    ["Abyss", "Breach", "Delirium", "Essences", "Expedition", "Fragments", "Idols", "Ritual", "Verisium"];

pub fn is_mechanic(category: &str) -> bool {
    MECHANICS.contains(&category)
}

/// Category names as a reader expects them ("SoulCores" -> "Soul Cores").
pub fn category_label(category: &str) -> String {
    let mut out = String::with_capacity(category.len() + 2);
    for (i, c) in category.chars().enumerate() {
        if i > 0 && c.is_uppercase() {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Traded on the currency exchange: carries a traded volume.
    Exchange,
    /// Listed on the trade site: carries a listing count.
    Listed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MarketItem {
    /// The overview it came from ("Breach", "UniqueWeapons").
    pub category: String,
    /// Stable key inside the category, for the history log: the exchange
    /// id, or name, base type and corruption state of a listed item.
    pub id: String,
    pub name: String,
    pub base_type: Option<String>,
    pub corrupted: bool,
    pub kind: Kind,
    pub price_div: f64,
    /// Traded volume in divines. poe.ninja does not say over what period,
    /// so nothing here depends on the period: it ranks, it weighs, and it
    /// is compared with a floor in its own unit.
    pub volume_div: Option<f64>,
    pub listings: Option<u32>,
    /// Cumulative percent change against the first day, oldest first.
    pub points: [Option<f64>; DAYS],
    /// Exalted per divine in the overview this line came from. The unique
    /// overviews are computed apart from the exchange ones and carry a
    /// different rate (444.1 against 474.2 on 2026-09-19).
    pub own_exalted_rate: f64,
    /// The chaos rate of the item's own overview, like `own_exalted_rate`.
    pub own_chaos_rate: f64,
}

impl MarketItem {
    pub fn points_used(&self) -> usize {
        self.points.iter().flatten().count()
    }
}

/// What parsing left out, for the settings tab to show.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Dropped {
    /// Lines without a positive finite price. They are not items.
    pub prices: usize,
    /// Volumes that were not positive finite numbers. The item stays, with
    /// no volume.
    pub volumes: usize,
    /// Sparkline points that were not finite, or at or under -100% (a
    /// price cannot fall by more than all of it). They become gaps.
    pub points: usize,
}

/// Every priced line of one league's overviews, with the rates they were
/// priced at.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Source {
    pub league: String,
    pub items: Vec<MarketItem>,
    /// Exalted and chaos per divine of the exchange table: the last
    /// exchange overview carrying one, as `PriceTable::build` takes it.
    pub exalted_rate: f64,
    pub chaos_rate: f64,
    pub dropped: Dropped,
}

fn points_of(spark: Option<&Sparkline>, dropped: &mut Dropped) -> [Option<f64>; DAYS] {
    let mut out = [None; DAYS];
    let Some(spark) = spark else { return out };
    // A series of another length has no day six to anchor on; its days
    // cannot be placed, so none of them is used.
    if spark.data.len() != DAYS {
        return out;
    }
    for (slot, v) in out.iter_mut().zip(&spark.data) {
        match v {
            Some(x) if x.is_finite() && *x > -100.0 => *slot = Some(*x),
            Some(_) => dropped.points += 1,
            None => {}
        }
    }
    out
}

fn positive(x: f64) -> bool {
    x.is_finite() && x > 0.0
}

impl Source {
    /// From the overviews by category name. Exchange item names come from
    /// the overview's own id -> name lists; a line without a name there
    /// keeps its id, since it is still a priced, traded thing.
    pub fn from_overviews(
        league: &str,
        exchange: &[(&str, &ExchangeOverview)],
        listed: &[(&str, &ItemOverview)],
    ) -> Source {
        let mut src = Source { league: league.to_string(), ..Source::default() };
        for (category, ov) in exchange {
            let own = ov.core.rates.get("exalted").copied().filter(|r| positive(*r)).unwrap_or(0.0);
            let own_chaos = ov.core.rates.get("chaos").copied().filter(|r| positive(*r)).unwrap_or(0.0);
            if own > 0.0 {
                src.exalted_rate = own;
            }
            if let Some(ch) = ov.core.rates.get("chaos").copied().filter(|r| positive(*r)) {
                src.chaos_rate = ch;
            }
            let name_of = |id: &str| {
                ov.items.iter().chain(ov.core.items.iter()).rfind(|it| it.id == id).map(|it| it.name.clone())
            };
            for line in &ov.lines {
                if !positive(line.primary_value) {
                    src.dropped.prices += 1;
                    continue;
                }
                let volume_div = match line.volume_primary_value {
                    Some(v) if positive(v) => Some(v),
                    Some(_) => {
                        src.dropped.volumes += 1;
                        None
                    }
                    None => None,
                };
                src.items.push(MarketItem {
                    category: category.to_string(),
                    id: line.id.clone(),
                    name: name_of(&line.id).unwrap_or_else(|| line.id.clone()),
                    base_type: None,
                    corrupted: false,
                    kind: Kind::Exchange,
                    price_div: line.primary_value,
                    volume_div,
                    listings: None,
                    points: points_of(line.sparkline.as_ref(), &mut src.dropped),
                    own_exalted_rate: own,
                    own_chaos_rate: own_chaos,
                });
            }
        }
        for (category, ov) in listed {
            let own = ov.core.rates.get("exalted").copied().filter(|r| positive(*r)).unwrap_or(0.0);
            let own_chaos = ov.core.rates.get("chaos").copied().filter(|r| positive(*r)).unwrap_or(0.0);
            for line in &ov.lines {
                if !positive(line.primary_value) {
                    src.dropped.prices += 1;
                    continue;
                }
                let base_type = line.base_type.clone().filter(|b| !b.trim().is_empty());
                let corrupted = line.corrupted.unwrap_or(false);
                src.items.push(MarketItem {
                    category: category.to_string(),
                    id: format!(
                        "{}|{}|{}",
                        line.name,
                        base_type.as_deref().unwrap_or(""),
                        if corrupted { "c" } else { "" }
                    ),
                    name: line.name.clone(),
                    base_type,
                    corrupted,
                    kind: Kind::Listed,
                    price_div: line.primary_value,
                    volume_div: None,
                    listings: line.listing_count,
                    points: points_of(line.spark_line.as_ref(), &mut src.dropped),
                    own_exalted_rate: own,
                    own_chaos_rate: own_chaos,
                });
            }
        }
        src
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The most days of history any line carries, counted back from today.
    /// A league younger than that many days cannot have produced them.
    pub fn history_days(&self) -> usize {
        self.items
            .iter()
            .filter_map(|it| it.points.iter().position(Option::is_some))
            .map(|first| DAYS - first)
            .max()
            .unwrap_or(0)
    }
}

/// The liquidity floors, editable in Settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Floors {
    /// Traded volume from which an exchange item is `Liquid`, in the
    /// volume field's own unit (divines, period unpublished).
    pub volume_div: f64,
    /// Listings from which a listed item is `Liquid`.
    pub listings_rank: u32,
    /// Listings under which a listed item is `Untrusted`.
    pub listings_min: u32,
}

impl Default for Floors {
    fn default() -> Floors {
        Floors { volume_div: 5.0, listings_rank: 20, listings_min: 10 }
    }
}

impl Floors {
    /// Why these floors cannot be used, if they cannot.
    pub fn problem(&self) -> Option<&'static str> {
        if !(self.volume_div.is_finite() && self.volume_div >= 0.0) {
            Some("the volume floor must be zero or more")
        } else if self.listings_min < 1 || self.listings_rank < 1 {
            Some("the listing floors must be at least 1")
        } else if self.listings_min > self.listings_rank {
            Some("the minimum listings cannot exceed the listings needed to rank")
        } else {
            None
        }
    }

    /// These floors if usable, the defaults otherwise.
    pub fn or_default(self) -> Floors {
        if self.problem().is_some() {
            Floors::default()
        } else {
            self
        }
    }
}

/// Decided in this order: `Untrusted`, then `Liquid`, else `Thin`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grade {
    /// A listed item under the minimum listings or at the price floor
    /// (see [`at_price_floor`]). Hidden unless asked for; never a trend.
    Untrusted,
    /// Enough volume or listings to rank and to enter an index.
    Liquid,
    /// Shown greyed as "thin market"; never ranked, never in an index.
    Thin,
}

impl Grade {
    pub fn word(self) -> &'static str {
        match self {
            Grade::Untrusted => "untrusted",
            Grade::Liquid => "liquid",
            Grade::Thin => "thin",
        }
    }
}

pub fn grade(item: &MarketItem, floors: &Floors, table_exalted_rate: f64) -> Grade {
    let too_few = item.listings.is_some_and(|l| l < floors.listings_min);
    if too_few || at_price_floor(item, table_exalted_rate) {
        return Grade::Untrusted;
    }
    let liquid = match item.listings {
        Some(l) => l >= floors.listings_rank && !is_coarsely_priced(item, table_exalted_rate),
        None => item.volume_div.unwrap_or(0.0) >= floors.volume_div,
    };
    if liquid {
        Grade::Liquid
    } else {
        Grade::Thin
    }
}

/// Whether a LISTED item sits on the one-exalted price floor. A seller
/// cannot list below one orb, so a unique at 1 ex cannot fall, and its week
/// in divines is the exalted orb's week: measured with one global rate, 202
/// one-exalted uniques stayed trusted on 2026-09-19 and 130 of them ranked
/// as fallers at an identical -33.80%.
///
/// The rate is the item's OWN overview's (0.00225 div x 444.1 = 0.999 ex
/// there, 1.07 ex at the exchange table's 474.2); the table's stands in only
/// for an overview that carries none. The tolerance absorbs the source's
/// four-digit rounding.
///
/// Never an exchange item: those trade at executed ratios, a price under
/// one exalted there is a real price, and the Exalted Orb's own move
/// against the divine is the most important trend in the table. Thin ones
/// are held back by the volume floor.
/// Listed prices under this many exalted move in whole-orb steps.
pub const COARSE_PRICE_EX: f64 = 10.0;
/// Listed prices under this many chaos that fall on whole chaos are on the
/// chaos grid.
pub const COARSE_PRICE_CHAOS: f64 = 10.0;

/// Whether a LISTED item is priced where sellers price in whole orbs. Under
/// ten exalted one step of the grid is a tenth of the price or more, and
/// the divine-denominated history of such an item mostly mirrors the
/// exalted orb's own drift: on 2026-09-20 half the ranked uniques (67 of
/// 134) sat here, in clusters sharing one move to the hundredth of a percent
/// (seven at -46.82%, four at -33.76%). A percentage on that grid says
/// nothing about the item, so it keeps its price and loses its rank.
pub fn is_coarsely_priced(item: &MarketItem, table_exalted_rate: f64) -> bool {
    if item.kind != Kind::Listed {
        return false;
    }
    let own = if item.own_exalted_rate > 0.0 { item.own_exalted_rate } else { table_exalted_rate };
    if item.price_div * own < COARSE_PRICE_EX {
        return true;
    }
    // The same grid one currency up: a unique listed at "1 chaos" or "3
    // chaos" sits on a step a third of its price wide, and its divine
    // history is the chaos orb's drift (eight uniques at 0.1279 div shared
    // one move of +8.85% on 2026-09-26, the chaos orb's own). A price
    // within half a percent of a whole number of chaos, under ten chaos,
    // is on that grid.
    if item.own_chaos_rate > 0.0 {
        let chaos = item.price_div * item.own_chaos_rate;
        let whole = chaos.round();
        if chaos < COARSE_PRICE_CHAOS && whole >= 1.0 && (chaos - whole).abs() <= chaos * 0.005 {
            return true;
        }
    }
    false
}

pub fn at_price_floor(item: &MarketItem, table_exalted_rate: f64) -> bool {
    if item.kind != Kind::Listed {
        return false;
    }
    let own = if item.own_exalted_rate > 0.0 { item.own_exalted_rate } else { table_exalted_rate };
    item.price_div * own <= 1.0 + FLOOR_ROUNDING
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Rising,
    Falling,
    /// Enough history, no clear direction in it.
    Unclear,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Trend {
    /// Percent move from the first day that has a figure to today.
    pub change: f64,
    /// Twice the sample standard deviation of the week's points, rounded:
    /// how far the price swings, not a promise.
    pub band: u32,
    pub direction: Direction,
    pub points_used: usize,
}

impl Trend {
    /// Inside its own band: not a move.
    pub fn is_flat(&self) -> bool {
        self.change.abs() <= f64::from(self.band)
    }
}

/// The trend the points support, or `None` for "not enough history":
/// fewer than [`MIN_POINTS`] days, or no figure for today.
pub fn trend(points: &[Option<f64>; DAYS]) -> Option<Trend> {
    points[DAYS - 1]?;
    let vals: Vec<f64> = points.iter().flatten().copied().collect();
    if vals.len() < MIN_POINTS {
        return None;
    }
    let (base, last, rest) = (vals[0], vals[vals.len() - 1], &vals[1..]);
    let change = ((1.0 + last / 100.0) / (1.0 + base / 100.0) - 1.0) * 100.0;
    let mean = vals.iter().sum::<f64>() / vals.len() as f64;
    let variance = vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (vals.len() - 1) as f64;
    let band = (2.0 * variance.sqrt()).round();
    if !change.is_finite() || !band.is_finite() {
        return None;
    }
    let above = rest.iter().filter(|v| **v > base).count();
    let below = rest.iter().filter(|v| **v < base).count();
    let last_three = &rest[rest.len().saturating_sub(3)..];
    let all_last = |f: &dyn Fn(f64) -> bool| last_three.len() == 3 && last_three.iter().all(|v| f(*v));
    let direction = if change > 0.0 && (above >= 4 || all_last(&|v| v > base)) {
        Direction::Rising
    } else if change < 0.0 && (below >= 4 || all_last(&|v| v < base)) {
        Direction::Falling
    } else {
        Direction::Unclear
    };
    Some(Trend { change, band: band as u32, direction, points_used: vals.len() })
}

#[derive(Debug, Clone, PartialEq)]
pub struct Graded {
    pub item: MarketItem,
    pub grade: Grade,
    /// `None` for an untrusted item whatever its history, for an item
    /// without enough history, and for every item of a young league.
    pub trend: Option<Trend>,
    /// See [`at_price_floor`]. Such an item is always `Untrusted`.
    pub at_floor: bool,
}

impl Graded {
    /// Liquid, with a direction, moved by more than its own band. What
    /// breadth counts as flat is not a mover: "falling -0.7%" inside a band
    /// of 55 ranked before this, and +1346% inside a band of 2884 led the
    /// risers.
    pub fn is_mover(&self) -> bool {
        self.grade == Grade::Liquid
            && self.trend.is_some_and(|t| t.direction != Direction::Unclear && !t.is_flat())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Coverage {
    pub items_used: usize,
    pub items_total: usize,
    /// The basket's share of the group's traded volume, 0..=1.
    pub volume_share: f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Breadth {
    pub up: usize,
    pub down: usize,
    pub flat: usize,
    pub of: usize,
}

/// What may be said about a group as a whole.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// No index: nothing is said.
    Unindexed,
    /// Index and breadth agree: see `verdict`.
    Rising,
    Falling,
    /// The index is within [`FLAT_INDEX_PCT`] of zero.
    Flat,
    /// The index moved but most items did not move with it: these items
    /// carry it, heaviest first.
    CarriedBy(Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub category: String,
    pub is_mechanic: bool,
    /// Every item's traded volume, summed.
    pub volume_div: f64,
    /// Share of all exchange volume, 0..=1.
    pub volume_share: f64,
    /// Volume-weighted geometric mean move of the basket, in percent.
    /// `None` is "too thin to index" (or a young league).
    pub index_pct: Option<f64>,
    pub coverage: Coverage,
    pub breadth: Breadth,
    pub verdict: Verdict,
    /// The index day by day on the basis of `index_pct` (each item from
    /// its first existing day), so the last point IS the figure. A day on
    /// which the basket items with a figure hold under the coverage share
    /// of the basket's volume is a gap.
    pub index_points: [Option<f64>; DAYS],
    /// Indices into `Model::items`: liquid by traded volume, then thin,
    /// then untrusted. By name in a young league.
    pub items: Vec<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Options {
    pub floors: Floors,
    /// See [`league_is_young`].
    pub young: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Model {
    pub league: String,
    pub items: Vec<Graded>,
    /// Exchange categories, by traded volume; in the fixed mechanic order
    /// for a young league, which has no rankings.
    pub groups: Vec<Group>,
    /// Indices into `items`, by |change| descending.
    pub risers: Vec<usize>,
    pub fallers: Vec<usize>,
    pub young: bool,
    pub exalted_rate: f64,
    pub chaos_rate: f64,
    pub dropped: Dropped,
    /// Listed items left untrusted by the price floor, for the settings
    /// tab's data-quality line.
    pub floor_pinned: usize,
}

impl Model {
    /// The item's price in all three currencies at the table's rates, for
    /// the overlay's display rule.
    pub fn price(&self, item: &MarketItem) -> Price {
        Price {
            divine: item.price_div,
            exalted: item.price_div * self.exalted_rate,
            chaos: item.price_div * self.chaos_rate,
        }
    }

    pub fn group(&self, category: &str) -> Option<&Group> {
        self.groups.iter().find(|g| g.category == category)
    }
}

/// Volume-weighted geometric mean of percent moves, in percent. Geometric
/// so one item going tenfold cannot carry its group: +900% weighs ln(10),
/// not 900.
fn geometric_index(moves: &[(f64, f64)]) -> Option<f64> {
    let weight: f64 = moves.iter().map(|(w, _)| w).sum();
    // No volume to weigh by, or one that is not a number.
    if weight.is_nan() || weight <= 0.0 {
        return None;
    }
    let log_sum: f64 = moves.iter().map(|(w, pct)| w * (1.0 + pct / 100.0).ln()).sum();
    let index = ((log_sum / weight).exp() - 1.0) * 100.0;
    index.is_finite().then_some(index)
}

fn verdict(index: Option<f64>, breadth: &Breadth, basket: &[&Graded]) -> Verdict {
    let Some(index) = index else { return Verdict::Unindexed };
    if index.abs() < FLAT_INDEX_PCT {
        return Verdict::Flat;
    }
    // Breadth agrees when the items that moved the index's way are the
    // largest camp: more than moved against it, and no fewer than stayed
    // inside their bands.
    if index > 0.0 && breadth.up > breadth.down && breadth.up >= breadth.flat {
        return Verdict::Rising;
    }
    if index < 0.0 && breadth.down > breadth.up && breadth.down >= breadth.flat {
        return Verdict::Falling;
    }
    // The items pulling the index its way, by how hard they pull.
    let mut pulls: Vec<(f64, &str)> = basket
        .iter()
        .filter_map(|g| {
            let t = g.trend?;
            let pull = g.item.volume_div.unwrap_or(0.0) * (1.0 + t.change / 100.0).ln();
            (pull.is_finite() && pull * index > 0.0).then_some((pull.abs(), g.item.name.as_str()))
        })
        .collect();
    pulls.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(b.1)));
    Verdict::CarriedBy(pulls.into_iter().take(3).map(|(_, n)| n.to_string()).collect())
}

pub fn build(source: &Source, opts: &Options) -> Model {
    let floors = opts.floors.or_default();
    let rate = source.exalted_rate;
    let items: Vec<Graded> = source
        .items
        .iter()
        .map(|item| {
            let grade = grade(item, &floors, rate);
            let trend = if grade == Grade::Untrusted || opts.young { None } else { trend(&item.points) };
            Graded { item: item.clone(), grade, trend, at_floor: at_price_floor(item, rate) }
        })
        .collect();

    let mut movers: Vec<usize> = (0..items.len()).filter(|&i| items[i].is_mover()).collect();
    let change = |i: usize| items[i].trend.map(|t| t.change).unwrap_or(0.0);
    movers.sort_by(|&a, &b| {
        change(b).abs().total_cmp(&change(a).abs()).then_with(|| items[a].item.name.cmp(&items[b].item.name))
    });
    let (risers, fallers) = movers.iter().partition(|&&i| change(i) > 0.0);
    let floor_pinned = items.iter().filter(|g| g.at_floor).count();

    let mut categories: Vec<&str> = Vec::new();
    for g in items.iter().filter(|g| g.item.kind == Kind::Exchange) {
        if !categories.contains(&g.item.category.as_str()) {
            categories.push(&g.item.category);
        }
    }
    let total_volume: f64 =
        items.iter().filter(|g| g.item.kind == Kind::Exchange).filter_map(|g| g.item.volume_div).sum();
    let mut groups: Vec<Group> = categories
        .iter()
        .map(|category| group_of(category, &items, total_volume, opts.young))
        .collect();
    if opts.young {
        let order = |g: &Group| MECHANICS.iter().position(|m| *m == g.category).unwrap_or(MECHANICS.len());
        groups.sort_by(|a, b| order(a).cmp(&order(b)).then_with(|| a.category.cmp(&b.category)));
    } else {
        groups.sort_by(|a, b| b.volume_div.total_cmp(&a.volume_div).then_with(|| a.category.cmp(&b.category)));
    }

    Model {
        league: source.league.clone(),
        items,
        groups,
        risers,
        fallers,
        young: opts.young,
        exalted_rate: rate,
        chaos_rate: source.chaos_rate,
        dropped: source.dropped,
        floor_pinned,
    }
}

fn group_of(category: &str, items: &[Graded], total_volume: f64, young: bool) -> Group {
    let members: Vec<usize> = (0..items.len())
        .filter(|&i| items[i].item.kind == Kind::Exchange && items[i].item.category == category)
        .collect();
    let volume_of = |i: usize| items[i].item.volume_div.unwrap_or(0.0);
    let volume_div: f64 = members.iter().map(|&i| volume_of(i)).sum();
    let basket: Vec<&Graded> =
        members.iter().map(|&i| &items[i]).filter(|g| g.grade == Grade::Liquid && g.trend.is_some()).collect();
    let basket_volume: f64 = basket.iter().filter_map(|g| g.item.volume_div).sum();
    let coverage = Coverage {
        items_used: basket.len(),
        items_total: members.len(),
        volume_share: if volume_div > 0.0 { basket_volume / volume_div } else { 0.0 },
    };
    let indexable = basket.len() >= INDEX_MIN_ITEMS && coverage.volume_share >= INDEX_MIN_COVERAGE;
    let index_pct = if indexable {
        let moves: Vec<(f64, f64)> =
            basket.iter().filter_map(|g| Some((g.item.volume_div?, g.trend?.change))).collect();
        geometric_index(&moves)
    } else {
        None
    };
    let mut index_points = [None; DAYS];
    if index_pct.is_some() {
        for (day, slot) in index_points.iter_mut().enumerate() {
            // Each item from its first existing day, as its `change` is: on
            // the last day this is the index figure itself.
            let moves: Vec<(f64, f64)> = basket
                .iter()
                .filter_map(|g| {
                    let base = g.item.points.iter().flatten().next()?;
                    let rebased = ((1.0 + g.item.points[day]? / 100.0) / (1.0 + base / 100.0) - 1.0) * 100.0;
                    Some((g.item.volume_div?, rebased))
                })
                .collect();
            let held: f64 = moves.iter().map(|(w, _)| w).sum();
            if basket_volume > 0.0 && held / basket_volume >= INDEX_MIN_COVERAGE {
                *slot = geometric_index(&moves);
            }
        }
    }
    let moved = |g: &&&Graded, up: bool| {
        g.trend.is_some_and(|t| !t.is_flat() && (t.change > 0.0) == up)
    };
    let up = basket.iter().filter(|g| moved(g, true)).count();
    let down = basket.iter().filter(|g| moved(g, false)).count();
    let breadth = Breadth { up, down, flat: basket.len() - up - down, of: basket.len() };
    let verdict = verdict(index_pct, &breadth, &basket);

    let mut ordered = members;
    if young {
        ordered.sort_by(|&a, &b| items[a].item.name.cmp(&items[b].item.name));
    } else {
        let rank = |g: Grade| match g {
            Grade::Liquid => 0,
            Grade::Thin => 1,
            Grade::Untrusted => 2,
        };
        ordered.sort_by(|&a, &b| {
            rank(items[a].grade)
                .cmp(&rank(items[b].grade))
                .then_with(|| volume_of(b).total_cmp(&volume_of(a)))
                .then_with(|| items[a].item.name.cmp(&items[b].item.name))
        });
    }
    Group {
        category: category.to_string(),
        is_mechanic: is_mechanic(category),
        volume_div,
        volume_share: if total_volume > 0.0 { volume_div / total_volume } else { 0.0 },
        index_pct,
        coverage,
        breadth,
        verdict,
        index_points,
        items: ordered,
    }
}

/// Whether the league is inside its first [`YOUNG_LEAGUE_SECS`].
///
/// `start` is the league's start when a league list gave one. Without it
/// the history log's first record stands in - but that record is only when
/// THIS install first saw the league, so a league joined in its third
/// month would be called young for three days. The overviews settle that:
/// a line carrying a figure from `history_days` ago proves the league is
/// at least that old.
pub fn league_is_young(now: i64, start: Option<i64>, first_record: Option<i64>, history_days: usize) -> bool {
    if let Some(start) = start {
        return now - start < YOUNG_LEAGUE_SECS;
    }
    // Day six is today, so a figure from `n` days of history is n-1 days
    // old at least.
    let proven = (history_days.saturating_sub(1) as i64) * 86_400;
    if proven >= YOUNG_LEAGUE_SECS {
        return false;
    }
    match first_record {
        Some(first) => now - first < YOUNG_LEAGUE_SECS,
        // Nothing says how old the league is, and nothing in the
        // overviews is old enough to rank on.
        None => true,
    }
}

/// "just now", "12 min", "3 h 05 min", "2 d 4 h": the age of the price
/// table.
pub fn age_text(secs: i64) -> String {
    let secs = secs.max(0);
    let (d, h, m) = (secs / 86_400, (secs % 86_400) / 3600, (secs % 3600) / 60);
    if d > 0 {
        format!("{d} d {h} h")
    } else if h > 0 {
        format!("{h} h {m:02} min")
    } else if m > 0 {
        format!("{m} min")
    } else {
        "just now".to_string()
    }
}

// The words both the overlay panel and the settings tab use, kept beside
// the numbers they describe so the two views cannot word a figure
// differently.

/// "5 of 7 days" when the trend rests on fewer than all seven.
pub fn days_note(points_used: usize) -> Option<String> {
    (points_used < DAYS).then(|| format!("{points_used} of {DAYS} days"))
}

/// The band as the reader sees it: how far the price swung this week.
pub fn band_text(band: u32) -> String {
    format!("±{band}% over {DAYS} days")
}

/// The same for a face without a plus-minus sign. The overlay's Fontin maps
/// U+00B1 to a glyph with an advance and no outline, so "±58%" drew as
/// " 58%" there, which reads as a gain.
pub fn band_text_ascii(band: u32) -> String {
    format!("+/-{band}% over {DAYS} days")
}

/// A signed percent: one decimal under 10, none from there, so "+0.4%" is
/// not rounded into "+0%" and "+1234%" carries no false precision.
pub fn percent_text(pct: f64) -> String {
    if pct.abs() < 10.0 {
        format!("{pct:+.1}%")
    } else {
        format!("{pct:+.0}%")
    }
}

pub fn direction_text(trend: Option<&Trend>) -> &'static str {
    match trend.map(|t| t.direction) {
        Some(Direction::Rising) => "rising",
        Some(Direction::Falling) => "falling",
        Some(Direction::Unclear) => "no clear direction",
        None => "not enough history",
    }
}

/// Traded volume with its unit and nothing else: the period it covers is
/// not published, so none is named.
pub fn volume_text(volume_div: f64) -> String {
    format!("{} div", crate::value::format_amount(volume_div))
}

pub fn share_text(share: f64) -> String {
    let pct = share * 100.0;
    if pct > 0.0 && pct < 1.0 {
        "<1%".to_string()
    } else {
        format!("{pct:.0}%")
    }
}

/// What the index rests on, or why there is none.
pub fn coverage_text(group: &Group) -> String {
    let c = &group.coverage;
    let held = share_text(c.volume_share);
    if group.index_pct.is_some() {
        format!("index from {} of {} items ({held} of the group's volume)", c.items_used, c.items_total)
    } else {
        format!("too thin to index: {} of {} items hold {held} of the group's volume", c.items_used, c.items_total)
    }
}

pub fn breadth_text(b: &Breadth) -> String {
    format!("{} up / {} down / {} flat of {}", b.up, b.down, b.flat, b.of)
}

pub fn verdict_text(verdict: &Verdict) -> String {
    match verdict {
        Verdict::Unindexed => String::new(),
        Verdict::Rising => "rising".to_string(),
        Verdict::Falling => "falling".to_string(),
        Verdict::Flat => "flat".to_string(),
        Verdict::CarriedBy(names) if names.is_empty() => "mixed".to_string(),
        Verdict::CarriedBy(names) => format!("carried by {}", names.join(", ")),
    }
}

pub const TOO_YOUNG: &str = "league too young for trends";
pub const THIN_MARKET: &str = "thin market";
/// The note for a listed item held back by its price grid, not its depth.
pub const COARSE_PRICE: &str = "priced in whole orbs";

/// The note beside an item, as the market view words it: an untrusted
/// item gets the reason it is untrusted and no trend words ("not enough
/// history" would be wrong about why there is none); any other gets its
/// direction, how many days that rests on when fewer than the week, and
/// "thin market" or "priced in whole orbs" when it is held back. Empty in a
/// young league, which says so once for the whole table.
pub fn note_text(g: &Graded, young: bool) -> String {
    let mut notes: Vec<String> = Vec::new();
    if g.grade == Grade::Untrusted {
        notes.push(if g.at_floor {
            "untrusted: listed at the 1 ex floor".to_string()
        } else {
            format!("untrusted: {} listings", g.item.listings.unwrap_or(0))
        });
    } else if !young {
        notes.push(direction_text(g.trend.as_ref()).to_string());
        match g.trend {
            Some(t) => notes.extend(days_note(t.points_used)),
            // Says how little there was, never a zero.
            None => notes.push(format!("{} of {DAYS} days", g.item.points_used())),
        }
        if g.grade == Grade::Thin {
            // Held back by its price grid or by its depth: the note says which.
            let coarse = is_coarsely_priced(&g.item, 0.0);
            notes.push(if coarse { COARSE_PRICE } else { THIN_MARKET }.to_string());
        }
    }
    notes.join(" · ")
}

/// The depth column as the market view words it: traded volume for an
/// exchange item, the listing count for a listed one, nothing otherwise.
pub fn depth_text(item: &MarketItem) -> String {
    match (item.volume_div, item.listings) {
        (Some(v), _) => volume_text(v),
        (None, Some(l)) => format!("{l} listed"),
        (None, None) => String::new(),
    }
}

/// What poe.ninja says about a checked item, under the market view's own
/// trust rules and in its words.
#[derive(Debug, Clone, PartialEq)]
pub struct NinjaBlock {
    pub price_div: f64,
    pub grade: Grade,
    /// None for an untrusted item, one without enough history, and every
    /// item of a young league, as in the table.
    pub trend: Option<Trend>,
    /// The band as the market view words it; empty without a trend.
    pub band_text: String,
    /// The market view's note for the item, or [`TOO_YOUNG`] in a young
    /// league.
    pub note: String,
    pub volume_text: String,
}

fn same_name(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// The market model's line for this item, by name, base type and
/// corruption. A unique's name is shared by every base it drops on and by
/// its corrupted copies, each its own market: the base narrows when the
/// caller knows it, and a name that still fits several lines is no
/// answer. `category_hint` (an overview name, "UniqueArmours") narrows
/// the search when given. None for an item poe.ninja does not track.
pub fn ninja_block(
    model: &Model,
    name: &str,
    base: Option<&str>,
    corrupted: bool,
    category_hint: Option<&str>,
) -> Option<NinjaBlock> {
    if name.trim().is_empty() {
        return None;
    }
    let mut candidates = model.items.iter().filter(|g| {
        let it = &g.item;
        same_name(&it.name, name)
            && it.corrupted == corrupted
            && category_hint.is_none_or(|c| it.category == c)
            && match (&it.base_type, base) {
                (Some(have), Some(want)) => same_name(have, want),
                _ => true,
            }
    });
    let g = match (candidates.next(), candidates.next()) {
        (Some(g), None) => g,
        _ => return None,
    };
    Some(NinjaBlock {
        price_div: g.item.price_div,
        grade: g.grade,
        trend: g.trend,
        band_text: g.trend.map(|t| band_text(t.band)).unwrap_or_default(),
        note: if model.young { TOO_YOUNG.to_string() } else { note_text(g, false) },
        volume_text: depth_text(&g.item),
    })
}
