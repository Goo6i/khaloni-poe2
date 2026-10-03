//! What the stash gained between two snapshots, set against the map time
//! spent in between.
//!
//! The overlay reads which rewards a panel offers, not which one was taken,
//! so panels are not income; the stash is. A **block** is the span between
//! two consecutive itemised snapshots A and B of one league:
//!
//! - `income = sum((qB - qA) x priceB)`: both ends valued with B's prices,
//!   so a price move is never income;
//! - `revaluation = sum(qA x (priceB - priceA))`, reported apart.
//!
//! The two add up to the change in the priced total. A stack that falls to
//! zero counts only once the following snapshot agrees: the stash API drops
//! tabs now and then, and a net-worth delta turns each such gap into a
//! loss. Until then the block is pending and feeds nothing.
//!
//! A rate is never returned without the sample it rests on, and a
//! per-mechanic rate only from blocks where every run saw that mechanic.
//! Times are the naive local seconds `runs` uses, so the caller shifts a
//! snapshot's epoch time by the log's offset before it gets here.

use std::collections::{BTreeMap, BTreeSet};

use crate::runs::{civil_from_days, Mechanic, Run, Totals};

/// Pure blocks and maps a per-mechanic rate needs before it is printed.
pub const MIN_PURE_BLOCKS: usize = 3;
pub const MIN_PURE_MAPS: usize = 10;

#[derive(Debug, Clone, PartialEq)]
pub struct Holding {
    pub qty: u64,
    /// Price of one, in divines, from the table the snapshot was valued
    /// with; `None` when that table did not price the item.
    pub price_div: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    /// Naive local seconds (see the module doc).
    pub at: i64,
    pub league: String,
    /// By item name. A name held at the previous snapshot and gone now is
    /// listed with quantity 0, so that its price at this end is known.
    pub items: BTreeMap<String, Holding>,
}

impl Snapshot {
    fn qty(&self, name: &str) -> u64 {
        self.items.get(name).map(|h| h.qty).unwrap_or(0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Figures {
    pub income_div: f64,
    pub revaluation_div: f64,
    /// Items whose quantity changed and that B's price set does not price:
    /// they are in neither figure.
    pub unpriced_changes: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub from: i64,
    pub to: i64,
    /// `None` while a vanished stack waits for the next snapshot.
    pub figures: Option<Figures>,
    /// Indexes into the runs the blocks were built with.
    pub runs: Vec<usize>,
    pub maps: usize,
    /// `None` when any run in the block has no known duration.
    pub map_seconds: Option<i64>,
    /// Seen in at least one run of the block.
    pub mechanics: BTreeSet<Mechanic>,
    /// Seen in every run of the block (empty for a block without runs).
    pub pure_for: BTreeSet<Mechanic>,
}

impl Block {
    pub fn pending(&self) -> bool {
        self.figures.is_none()
    }

    /// Fit for an hourly figure: settled, with maps, all of known length.
    pub fn complete(&self) -> bool {
        self.figures.is_some() && self.maps > 0 && self.map_seconds.is_some_and(|s| s > 0)
    }
}

/// The blocks between consecutive snapshots, oldest first. `snapshots` are
/// one league's, in time order; `runs` are finished runs. A run belongs to
/// the block its last interval ended in: that is when its loot can first be
/// in the stash, and its whole map time goes with it.
pub fn blocks(snapshots: &[Snapshot], runs: &[Run]) -> Vec<Block> {
    let mut out = Vec::new();
    let Some(first) = snapshots.first() else { return out };
    let mut qty_a: BTreeMap<String, u64> = first.items.iter().map(|(n, h)| (n.clone(), h.qty)).collect();
    for (i, pair) in snapshots.windows(2).enumerate() {
        let (a, b) = (&pair[0], &pair[1]);
        let next = snapshots.get(i + 2);
        let mut qty_b: BTreeMap<String, u64> = b.items.iter().map(|(n, h)| (n.clone(), h.qty)).collect();
        let mut pending = false;
        for (name, &qa) in &qty_a {
            if qa == 0 || b.qty(name) > 0 {
                continue;
            }
            match next {
                None => pending = true,
                // Back at the following snapshot: B missed it, nothing left
                // the stash. Its quantity carries over unchanged.
                Some(c) if c.qty(name) > 0 => {
                    qty_b.insert(name.clone(), qa);
                }
                Some(_) => {}
            }
        }
        let figures = (!pending).then(|| {
            let mut f = Figures { income_div: 0.0, revaluation_div: 0.0, unpriced_changes: 0 };
            let names: BTreeSet<&String> = qty_a.keys().chain(qty_b.keys()).collect();
            for name in names {
                let qa = qty_a.get(name).copied().unwrap_or(0);
                let qb = qty_b.get(name).copied().unwrap_or(0);
                let price_a = a.items.get(name).and_then(|h| h.price_div);
                let price_b = b.items.get(name).and_then(|h| h.price_div);
                match price_b {
                    Some(pb) => {
                        f.income_div += (qb as f64 - qa as f64) * pb;
                        if let Some(pa) = price_a {
                            f.revaluation_div += qa as f64 * (pb - pa);
                        }
                    }
                    None if qa != qb => f.unpriced_changes += 1,
                    None => {}
                }
            }
            f
        });
        let inside: Vec<usize> =
            runs.iter().enumerate().filter(|(_, r)| r.ended > a.at && r.ended <= b.at).map(|(i, _)| i).collect();
        let map_seconds = inside.iter().try_fold(0i64, |acc, &i| runs[i].seconds().map(|s| acc + s));
        let mechanics: BTreeSet<Mechanic> = inside.iter().flat_map(|&i| runs[i].mechanics.iter().copied()).collect();
        let pure_for =
            mechanics.iter().copied().filter(|m| inside.iter().all(|&i| runs[i].saw(*m))).collect::<BTreeSet<_>>();
        out.push(Block {
            from: a.at,
            to: b.at,
            figures,
            maps: inside.len(),
            runs: inside,
            map_seconds,
            mechanics,
            pure_for,
        });
        qty_a = qty_b;
    }
    out
}

/// What a rate rests on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Sample {
    pub blocks: usize,
    pub maps: usize,
    pub map_seconds: i64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rate {
    pub div_per_map_hour: f64,
    pub sample: Sample,
}

fn rate_over<'a>(blocks: impl Iterator<Item = &'a Block>) -> Option<Rate> {
    let (mut income, mut sample) = (0.0, Sample::default());
    for b in blocks.filter(|b| b.complete()) {
        income += b.figures.map(|f| f.income_div).unwrap_or(0.0);
        sample.blocks += 1;
        sample.maps += b.maps;
        sample.map_seconds += b.map_seconds.unwrap_or(0);
    }
    (sample.map_seconds > 0).then(|| Rate { div_per_map_hour: income / (sample.map_seconds as f64 / 3600.0), sample })
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MechanicIncome {
    Rate(Rate),
    /// Under a minimum: the pure blocks and their maps so far.
    NotEnough { blocks: usize, maps: usize },
}

/// A mechanic's rate from the blocks pure for it, or how far the sample is
/// from the minimums. A mixed block never gets here.
pub fn mechanic_income(blocks: &[Block], m: Mechanic) -> MechanicIncome {
    let pure = || blocks.iter().filter(move |b| b.complete() && b.pure_for.contains(&m));
    let (n, maps) = pure().fold((0, 0), |(n, maps), b| (n + 1, maps + b.maps));
    match rate_over(pure()) {
        Some(rate) if n >= MIN_PURE_BLOCKS && maps >= MIN_PURE_MAPS => MechanicIncome::Rate(rate),
        _ => MechanicIncome::NotEnough { blocks: n, maps },
    }
}

/// Whether the stash can be read at all.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum StashAccess {
    #[default]
    Ready,
    /// What the settings lack, in the words the settings use.
    Missing(Vec<String>),
}

impl StashAccess {
    pub fn from_credentials(account_name: &str, poesessid: &str) -> StashAccess {
        let mut missing = Vec::new();
        if account_name.trim().is_empty() {
            missing.push("account name".to_string());
        }
        if poesessid.trim().is_empty() {
            missing.push("POESESSID".to_string());
        }
        if missing.is_empty() {
            StashAccess::Ready
        } else {
            StashAccess::Missing(missing)
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MechanicLine {
    pub mechanic: Mechanic,
    /// Maps the log showed it in, of `Summary::all.maps`.
    pub seen_in: usize,
    /// `None` without stash access: there is nothing to say about income.
    pub income: Option<MechanicIncome>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Summary {
    pub today: Totals,
    /// Every finished run handed in.
    pub all: Totals,
    /// First entry of the oldest of them.
    pub since: Option<i64>,
    pub access: StashAccess,
    pub overall: Option<Rate>,
    pub mechanics: Vec<MechanicLine>,
    /// Oldest first; empty without stash access.
    pub blocks: Vec<Block>,
}

/// Everything the "My runs" view says. Without stash access it carries the
/// maps and the hours and nothing about income, whatever snapshots exist.
pub fn summarize(runs: &[Run], snapshots: &[Snapshot], now_local: i64, access: StashAccess) -> Summary {
    let midnight = now_local.div_euclid(86_400) * 86_400;
    let ready = access == StashAccess::Ready;
    let blocks = if ready { blocks(snapshots, runs) } else { Vec::new() };
    let mechanics = Mechanic::ALL
        .into_iter()
        .filter_map(|m| {
            let seen_in = runs.iter().filter(|r| r.saw(m)).count();
            (seen_in > 0).then(|| MechanicLine {
                mechanic: m,
                seen_in,
                income: ready.then(|| mechanic_income(&blocks, m)),
            })
        })
        .collect();
    Summary {
        today: Totals::of(runs.iter().filter(|r| r.started >= midnight)),
        all: Totals::of(runs),
        since: runs.iter().map(|r| r.started).min(),
        access,
        overall: rate_over(blocks.iter()),
        mechanics,
        blocks,
    }
}

// Wording, shared by the overlay tab, the settings table and the export.

fn plural(n: usize, one: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {one}s")
    }
}

/// "5.2 h", or "12 min" under an hour.
pub fn hours_text(seconds: i64) -> String {
    // Under an hour a tenth of an hour is too coarse: a three-minute map
    // would read "0.0 h".
    if seconds < 3600 {
        return format!("{} min", (seconds + 30) / 60);
    }
    format!("{:.1} h", seconds as f64 / 3600.0)
}

fn amount(x: f64) -> String {
    let a = x.abs();
    if a >= 100.0 {
        format!("{x:.0}")
    } else if a >= 10.0 {
        format!("{x:.1}")
    } else if a >= 1.0 {
        format!("{x:.2}")
    } else {
        format!("{x:.3}")
    }
}

/// "+3.20 div" / "-0.412 div": a block's income or revaluation.
pub fn signed_div_text(x: f64) -> String {
    let body = amount(x);
    // Rounded away entirely: no sign, since neither would be true.
    if body.chars().all(|c| matches!(c, '-' | '0' | '.')) {
        return "0 div".to_string();
    }
    if x > 0.0 {
        format!("+{body} div")
    } else {
        format!("{body} div")
    }
}

/// "2.10 div per map hour". Map hours, because hideout time is in no
/// figure here: this is not what an hour of play brings.
pub fn rate_text(rate: &Rate) -> String {
    format!("{} div per map hour", amount(rate.div_per_map_hour))
}

/// "measured over 6 blocks, 41 maps, 5.2 h".
pub fn sample_text(s: &Sample) -> String {
    format!("measured over {}, {}, {}", plural(s.blocks, "block"), plural(s.maps, "map"), hours_text(s.map_seconds))
}

pub const NOT_ENOUGH: &str = "not enough runs yet";

/// "not enough runs yet (1 of 3 pure blocks, 4 of 10 maps)".
pub fn not_enough_text(blocks: usize, maps: usize) -> String {
    format!("{NOT_ENOUGH} ({blocks} of {MIN_PURE_BLOCKS} pure blocks, {maps} of {MIN_PURE_MAPS} maps)")
}

/// "seen in 41 of 120 maps": the log only shows a mechanic when the engine
/// happened to complain about it, so this is a floor and says "seen".
pub fn seen_text(seen_in: usize, maps: usize) -> String {
    format!("seen in {seen_in} of {}", plural(maps, "map"))
}

/// What the view says in place of income when the stash cannot be read.
pub fn missing_text(missing: &[String]) -> String {
    format!("income needs the {} in Settings, Account: maps and hours only", missing.join(" and "))
}

pub const NO_BLOCKS: &str = "no income yet: it takes two stash snapshots of this league";
pub const NO_COMPLETE_BLOCKS: &str = "no income per map hour yet: no settled block holds maps of known length";
pub const PENDING: &str = "pending";
/// What the income figure is, under the table.
pub const INCOME_NOTE: &str =
    "income: change in the exchange-priced items of the first 20 stash tabs; gear is not valued, trades and purchases count";

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// "19 Sep" of naive seconds.
pub fn date_text(naive: i64) -> String {
    let (_, m, d) = civil_from_days(naive.div_euclid(86_400));
    format!("{d} {}", MONTHS[(m - 1) as usize])
}

/// "19 Sep 14:05".
pub fn time_text(naive: i64) -> String {
    let s = naive.rem_euclid(86_400);
    format!("{} {:02}:{:02}", date_text(naive), s / 3600, s % 3600 / 60)
}

/// "2026-09-19 14:05:00", for the export.
pub fn stamp_text(naive: i64) -> String {
    let (y, m, d) = civil_from_days(naive.div_euclid(86_400));
    let s = naive.rem_euclid(86_400);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
}

pub fn mechanics_text<'a>(mechanics: impl IntoIterator<Item = &'a Mechanic>) -> String {
    mechanics.into_iter().map(|m| m.name()).collect::<Vec<_>>().join(", ")
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// The runs as CSV. An unknown duration is an empty cell, never a zero.
pub fn runs_csv(runs: &[Run]) -> String {
    let mut out = String::from("area,seed,started,ended,map_seconds,portals,mechanics_seen\n");
    for r in runs {
        out.push_str(&format!(
            "{},{},{},{},{},{},{}\n",
            csv_field(&r.area),
            r.seed,
            stamp_text(r.started),
            stamp_text(r.ended),
            r.seconds().map(|s| s.to_string()).unwrap_or_default(),
            r.portals,
            csv_field(&mechanics_text(&r.mechanics).replace(", ", " ")),
        ));
    }
    out
}

/// The blocks as CSV; a pending block has empty figures.
pub fn blocks_csv(blocks: &[Block]) -> String {
    let mut out = String::from(
        "from,to,status,maps,map_seconds,mechanics_seen,pure_for,income_div,revaluation_div,unpriced_changes\n",
    );
    for b in blocks {
        let fig = |f: fn(&Figures) -> String| b.figures.as_ref().map(f).unwrap_or_default();
        out.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{}\n",
            stamp_text(b.from),
            stamp_text(b.to),
            if b.pending() { PENDING } else { "settled" },
            b.maps,
            b.map_seconds.map(|s| s.to_string()).unwrap_or_default(),
            csv_field(&mechanics_text(&b.mechanics).replace(", ", " ")),
            csv_field(&mechanics_text(&b.pure_for).replace(", ", " ")),
            fig(|f| format!("{:.4}", f.income_div)),
            fig(|f| format!("{:.4}", f.revaluation_div)),
            fig(|f| f.unpriced_changes.to_string()),
        ));
    }
    out
}
