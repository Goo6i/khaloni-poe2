//! Wealth tracker worker: every 30 minutes it walks the account's stash
//! tabs (via the core legacy-stash client, which owns the rate limiting),
//! prices every item through the live poe.ninja table, appends the total to
//! an on-disk jsonl history, and sends the fresh snapshot to the UI.
//!
//! Each snapshot also stores what it counted: the quantity of every item
//! and the price it was valued at. Income is worked out between two such
//! lists (`core::income`); a total alone cannot tell loot from a price
//! move. A snapshot is also taken once the player has sat in a hideout for
//! a minute after a map, so that blocks end where loot is stashed.

use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use khaloni_poe2_core::income;
use khaloni_poe2_core::stash::{fetch_stash_items, stash_value, StashClient, StashItem};
use serde::{Deserialize, Serialize};

use crate::prices::{PriceService, Snapshot};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WealthSnapshot {
    pub at_epoch_s: u64,
    pub total_ex: f64,
    /// The league whose stash was walked and whose prices valued it. Lines
    /// written before this existed have none and belong to no series: the
    /// file does not say which league they were.
    #[serde(default)]
    pub league: String,
    /// Seconds the game log's clock ran ahead of UTC when this was taken
    /// (see `myruns`): what places the snapshot among the log's runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_offset_s: Option<i64>,
    /// What was counted: `[name, quantity, price of one in divines]`, the
    /// price null when the table had none. A name held at the previous
    /// snapshot and gone now is listed with quantity 0. Lines from before
    /// this existed carry a total only and can form no income block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<Vec<ItemLine>>,
}

pub type ItemLine = (String, u64, Option<f64>);

impl WealthSnapshot {
    /// The snapshot as `core::income` reads it, placed on the log's clock.
    /// `None` for a total-only line.
    pub fn itemised(&self, fallback_offset_s: i64) -> Option<income::Snapshot> {
        let items = self.items.as_ref()?;
        Some(income::Snapshot {
            at: self.at_epoch_s as i64 + self.local_offset_s.unwrap_or(fallback_offset_s),
            league: self.league.clone(),
            items: items
                .iter()
                .map(|(name, qty, price)| (name.clone(), income::Holding { qty: *qty, price_div: *price }))
                .collect(),
        })
    }
}

/// The stash as one line per item name: quantities summed across tabs,
/// each with the table's divine price. `held_before` are the names of the
/// previous snapshot; one that is gone is kept at quantity 0 with today's
/// price, which is the price its loss is valued at.
pub fn item_lines(items: &[StashItem], prices: &Snapshot, held_before: &[String]) -> Vec<ItemLine> {
    let mut qty: std::collections::BTreeMap<&str, u64> = std::collections::BTreeMap::new();
    for i in items {
        *qty.entry(i.type_line.as_str()).or_default() += u64::from(i.stack_size);
    }
    for name in held_before {
        qty.entry(name.as_str()).or_default();
    }
    qty.into_iter()
        .map(|(name, q)| {
            let price = prices.table.lookup(name).map(|p| p.divine).filter(|p| p.is_finite() && *p > 0.0);
            (name.to_string(), q, price)
        })
        .collect()
}

/// A hideout visit this long after a map is taken to be a stashing stop.
pub const HIDEOUT_SETTLE: Duration = Duration::from_secs(60);
/// The extra snapshot is not taken sooner than this after the last attempt.
pub const EXTRA_SNAPSHOT_MIN_AGE: Duration = Duration::from_secs(10 * 60);

/// Whether the hideout snapshot is due. It needs a map to have ended since
/// the last snapshot: without one there is no loot to bound, and sitting
/// in the hideout would otherwise triple the requests for nothing.
pub fn extra_snapshot_due(in_hideout_for: Option<Duration>, since_last_attempt: Duration, map_ended_since: bool) -> bool {
    map_ended_since
        && since_last_attempt > EXTRA_SNAPSHOT_MIN_AGE
        && in_hideout_for.is_some_and(|d| d >= HIDEOUT_SETTLE)
}

/// Stash contents move on the hours scale and each snapshot costs up to 20
/// account-endpoint requests; 30 minutes is frequent enough for a trend
/// line and polite to the endpoint.
const SNAPSHOT_INTERVAL: Duration = Duration::from_secs(30 * 60);

/// Snapshot history lives beside the other caches: one JSON object per
/// line, append-only, so a crash can lose at most the line being written.
pub fn history_path() -> PathBuf {
    directories::ProjectDirs::from("", "", "khaloni-poe2")
        .map(|d| d.cache_dir().to_path_buf())
        .unwrap_or_else(std::env::temp_dir)
        .join("wealth.jsonl")
}

/// Last `limit` snapshots, oldest first. Any unreadable line (torn write,
/// old format) is skipped: history is a nicety, never an error source.
/// The newest `limit` snapshots of one league. Totals from two leagues are
/// two different stashes valued at two different sets of prices; drawn as
/// one series they would show a fortune made or lost at the switch.
pub fn load_history(league: &str, limit: usize) -> Vec<WealthSnapshot> {
    let Ok(text) = std::fs::read_to_string(history_path()) else {
        return Vec::new();
    };
    series(&text, league, limit)
}

/// Every itemised snapshot of `league` in the file, oldest first.
pub fn load_itemised(league: &str, fallback_offset_s: i64) -> Vec<income::Snapshot> {
    let Ok(text) = std::fs::read_to_string(history_path()) else {
        return Vec::new();
    };
    itemised_series(&text, league, fallback_offset_s)
}

pub fn itemised_series(jsonl: &str, league: &str, fallback_offset_s: i64) -> Vec<income::Snapshot> {
    // File order is the order they were taken in. It is not re-sorted by
    // the log's clock: across a clock change that would pair snapshots the
    // wrong way round.
    jsonl
        .lines()
        .filter_map(|l| serde_json::from_str::<WealthSnapshot>(l).ok())
        .filter(|s| s.league == league)
        .filter_map(|s| s.itemised(fallback_offset_s))
        .collect()
}

pub fn series(jsonl: &str, league: &str, limit: usize) -> Vec<WealthSnapshot> {
    let all: Vec<WealthSnapshot> = jsonl
        .lines()
        .filter_map(|l| serde_json::from_str::<WealthSnapshot>(l).ok())
        .filter(|s| s.league == league)
        .collect();
    let skip = all.len().saturating_sub(limit);
    all.into_iter().skip(skip).collect()
}

/// Itemised lines are tens of kilobytes each; past this the oldest go.
pub const HISTORY_MAX_BYTES: u64 = 32 * 1024 * 1024;

fn append_history(snap: &WealthSnapshot) -> std::io::Result<()> {
    append_to(&history_path(), snap, HISTORY_MAX_BYTES)
}

/// Appends one line; a file past `max_bytes` is first cut to its newer
/// half, whole lines only, through a temp file so a crash leaves either
/// the old history or the new.
pub fn append_to(path: &std::path::Path, snap: &WealthSnapshot, max_bytes: u64) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if std::fs::metadata(path).map(|m| m.len()).unwrap_or(0) > max_bytes {
        let text = std::fs::read_to_string(path)?;
        let from = text.len() / 2;
        let cut = text[from..].find('\n').map(|i| from + i + 1).unwrap_or(text.len());
        let tmp = path.with_extension("jsonl.tmp");
        std::fs::write(&tmp, &text.as_bytes()[cut..])?;
        std::fs::rename(&tmp, path)?;
    }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(f, "{}", serde_json::json!(snap))
}

/// Spawns the snapshot worker. Missing account name or session cookie means
/// the feature is off: nothing is spawned, no request is ever made. The
/// thread exits when the snapshot receiver is dropped.
pub fn spawn(
    account: String,
    poesessid: String,
    svc: PriceService,
    tx: Sender<WealthSnapshot>,
    hub: std::sync::Arc<crate::myruns::Hub>,
) {
    if account.trim().is_empty() || poesessid.trim().is_empty() {
        return;
    }
    std::thread::spawn(move || run(account, poesessid, svc, tx, hub));
}

/// How long one cycle waits for usable prices before giving up on it, and
/// how soon a cycle that gave up is tried again.
const PRICE_WAIT: Duration = Duration::from_secs(10 * 60);
const RETRY_AFTER_SKIP: Duration = Duration::from_secs(5 * 60);

/// Whether a total priced from `snap` belongs in the history. The price
/// service starts with an empty table and fills it seconds later; a
/// snapshot taken in that gap priced every item at zero and wrote a total
/// of 0 into the trend line. A stale table is refused for the same reason
/// in milder form: the point plotted would mix today's stash with prices of
/// unknown age.
pub fn prices_usable(snap: &Snapshot) -> bool {
    !snap.table.is_empty() && !snap.stale
}

fn wait_for_prices(svc: &PriceService, max: Duration) -> Option<std::sync::Arc<Snapshot>> {
    let started = std::time::Instant::now();
    loop {
        let snap = svc.snapshot();
        if prices_usable(&snap) {
            return Some(snap);
        }
        if started.elapsed() >= max {
            return None;
        }
        std::thread::sleep(Duration::from_secs(5));
    }
}

/// The stash total in exalted. Unknown names price at 0 by design - the
/// total is a lower bound, and gear the exchange table cannot price would
/// otherwise poison the trend.
pub fn total_ex(items: &[khaloni_poe2_core::stash::StashItem], prices: &Snapshot) -> f64 {
    stash_value(items, &|type_line: &str, stack: u32| {
        prices.table.lookup(type_line).map(|p| p.exalted * f64::from(stack)).unwrap_or(0.0)
    })
}

/// The names the newest itemised snapshot of `league` on disk held.
fn names_last_held(league: &str) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(history_path()) else { return Vec::new() };
    text.lines()
        .rev()
        .filter_map(|l| serde_json::from_str::<WealthSnapshot>(l).ok())
        .find(|s| s.league == league && s.items.is_some())
        .and_then(|s| s.items)
        .map(|items| items.into_iter().filter(|(_, q, _)| *q > 0).map(|(n, _, _)| n).collect())
        .unwrap_or_default()
}

/// Sleeps out `pause`, waking early when the hideout snapshot falls due.
/// Every request that follows still waits its turn at the stash client's
/// limiters; this only decides when to ask.
fn wait_for_next(pause: Duration, hub: &crate::myruns::Hub) {
    let started = std::time::Instant::now();
    while started.elapsed() < pause {
        std::thread::sleep(Duration::from_secs(5).min(pause));
        if extra_snapshot_due(hub.in_hideout_for(), started.elapsed(), hub.map_ended_since_snapshot()) {
            return;
        }
    }
}

fn run(
    account: String,
    poesessid: String,
    svc: PriceService,
    tx: Sender<WealthSnapshot>,
    hub: std::sync::Arc<crate::myruns::Hub>,
) {
    let mut client = StashClient::new();
    // (league, names): the zero lines of the next snapshot.
    let mut held: Option<(String, Vec<String>)> = None;
    loop {
        // The league is read off the price table, not held here: the stash
        // that is walked is then always the league the prices are for, and
        // after a league change the next snapshot waits for the new table.
        let Some(league) = wait_for_prices(&svc, PRICE_WAIT).map(|p| p.league.clone()) else {
            eprintln!("wealth: no fresh price table; snapshot skipped");
            std::thread::sleep(RETRY_AFTER_SKIP);
            continue;
        };
        // Maps that end during the walk belong to the next snapshot.
        let maps_before_walk = hub.intervals_closed();
        let pause = match fetch_stash_items(&mut client, &account, &league, &poesessid) {
            Ok(items) => match wait_for_prices(&svc, PRICE_WAIT).filter(|p| p.league == league) {
                Some(prices) => {
                    let at_epoch_s = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    let before = match &held {
                        Some((l, names)) if *l == league => names.clone(),
                        _ => names_last_held(&league),
                    };
                    let lines = item_lines(&items, &prices, &before);
                    held = Some((
                        league.clone(),
                        lines.iter().filter(|(_, q, _)| *q > 0).map(|(n, _, _)| n.clone()).collect(),
                    ));
                    let snap = WealthSnapshot {
                        at_epoch_s,
                        total_ex: total_ex(&items, &prices),
                        league,
                        local_offset_s: hub.offset(),
                        items: Some(lines),
                    };
                    if let Err(e) = append_history(&snap) {
                        eprintln!("wealth: history append failed: {e}");
                    }
                    hub.mark_snapshot(maps_before_walk);
                    if tx.send(snap).is_err() {
                        return; // receiver gone: the app is shutting down
                    }
                    SNAPSHOT_INTERVAL
                }
                None => {
                    eprintln!("wealth: the price table went stale or changed league during the stash walk; snapshot skipped");
                    RETRY_AFTER_SKIP
                }
            },
            Err(e) => {
                eprintln!("wealth snapshot failed: {e}");
                SNAPSHOT_INTERVAL
            }
        };
        wait_for_next(pause, &hub);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use khaloni_poe2_core::ninja::{ExchangeOverview, PriceTable, UniquePrices};
    use khaloni_poe2_core::stash::StashItem;

    fn snapshot(table: PriceTable, stale: bool) -> Snapshot {
        let vocab = crate::pricing::build_vocab(&table);
        Snapshot {
            league: "Test League".into(),
            loading: false,
            error: None,
            table,
            vocab,
            uniques: UniquePrices::default(),
            stale,
            uniques_stale: false,
        }
    }

    fn filled() -> PriceTable {
        let ov: ExchangeOverview =
            serde_json::from_str(include_str!("../../core/tests/fixtures/ninja_currency.json")).unwrap();
        PriceTable::build(&[ov])
    }

    #[test]
    fn the_startup_table_is_not_priced_against() {
        // What the price service holds until its first fetch lands: every
        // item would price at zero.
        assert!(!prices_usable(&snapshot(PriceTable::default(), true)));
        assert!(!prices_usable(&snapshot(PriceTable::default(), false)));
        assert!(!prices_usable(&snapshot(filled(), true)), "prices of unknown age are refused too");
        assert!(prices_usable(&snapshot(filled(), false)));
    }

    #[test]
    fn each_league_is_its_own_series() {
        let jsonl = concat!(
            r#"{"at_epoch_s":1,"total_ex":900.0}"#,
            "\n",
            r#"{"at_epoch_s":2,"total_ex":1000.0,"league":"Standard"}"#,
            "\n",
            r#"{"at_epoch_s":3,"total_ex":12.0,"league":"Forbidden Rites"}"#,
            "\n",
            r#"{"at_epoch_s":4,"total_ex":1010.0,"league":"Standard"}"#,
            "\n",
        );
        let totals = |league: &str| series(jsonl, league, 10).iter().map(|s| s.total_ex).collect::<Vec<_>>();
        assert_eq!(totals("Standard"), [1000.0, 1010.0]);
        assert_eq!(totals("Forbidden Rites"), [12.0], "a new league starts its own line, not a crash from 1000");
        assert_eq!(series(jsonl, "Standard", 1)[0].at_epoch_s, 4, "the newest are kept");
    }

    #[test]
    fn items_are_priced_by_stack_with_unknowns_at_zero() {
        let items = vec![
            StashItem { type_line: "Orb of Annulment".into(), stack_size: 4 },
            StashItem { type_line: "Stellar Amulet".into(), stack_size: 1 },
        ];
        let total = total_ex(&items, &snapshot(filled(), false));
        assert!((total - 4.0 * 0.0325 * 410.0).abs() < 1e-6, "got {total}");
    }
}
