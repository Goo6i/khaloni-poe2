use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::time::Duration;

use khaloni_poe2_core::market::Source as MarketSource;
use khaloni_poe2_core::market_history::{HistoryLog, Record};
use khaloni_poe2_core::matcher::Vocab;
use khaloni_poe2_core::ninja::{
    unique_prices, DataOrigin, ExchangeOverview, ItemOverview, NinjaClient, NinjaError, PriceTable, UniquePrices,
    EXCHANGE_TYPES, UNIQUE_TYPES,
};
use khaloni_poe2_core::scout::ScoutClient;

pub struct Snapshot {
    /// The league every price in here belongs to. Whoever converts or shows
    /// a price reads the league from the same snapshot, so the two cannot
    /// come from different leagues.
    pub league: String,
    /// No sweep for `league` has finished yet: the table is empty because
    /// it is still being fetched, not because the league has no prices.
    pub loading: bool,
    /// Why `league` has no price table, once a sweep has failed outright:
    /// a name poe.ninja does not list, or the fetch error.
    pub error: Option<String>,
    pub table: PriceTable,
    pub vocab: Vocab,
    /// Unique prices in exalted, by name, base type and corruption state
    /// (poe.ninja; poe2scout by name when ninja has nothing). Empty when
    /// neither source answered and nothing is cached: uniques then go to
    /// the trade site, never an error.
    pub uniques: UniquePrices,
    /// Some of the currency table is older than this refresh: a type was
    /// served from the disk cache, or failed and kept its previous lines.
    pub stale: bool,
    /// The same for the unique prices, which refresh on their own cadence.
    pub uniques_stale: bool,
}

/// Uniques move slowly compared to currency; refetching them every
/// currency cycle would hammer the price sites for nothing. One fetch per
/// this many currency refreshes (default interval 10 min -> uniques every
/// 30), and at every refresh while the ones held are stale.
const UNIQUES_EVERY_N_REFRESHES: u32 = 3;

/// What the market view is built from: every priced line of the league's
/// overviews, published with the snapshot of the same sweep. It sits beside
/// [`Snapshot`] and names its own league, so a reader holding both can tell
/// when they are not the same league's.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Market {
    pub source: MarketSource,
    /// When the oldest EXCHANGE overview in `source` was fetched, unix
    /// seconds. `None` when that is not known (a cached file whose time
    /// cannot be read).
    pub fetched_at: Option<i64>,
    /// The same for the unique overviews, which refresh on their own,
    /// slower cadence: the two ages are told apart on screen.
    pub uniques_fetched_at: Option<i64>,
    /// Categories whose lines are not from the latest sweep.
    pub old_categories: Vec<String>,
    /// When this install first recorded the league in its history log.
    pub first_record: Option<i64>,
}

#[derive(Clone)]
pub struct PriceService {
    inner: Arc<RwLock<Arc<Snapshot>>>,
    market: Arc<RwLock<Arc<Market>>>,
    ctl: Arc<(Mutex<Ctl>, Condvar)>,
}

/// Which league the worker fetches. `epoch` counts the changes: a sweep
/// publishes only if the epoch it started under is still the current one,
/// so a sweep of the previous league that was in flight during a change
/// can never land under the new league's name.
struct Ctl {
    league: String,
    epoch: u64,
}

/// What is served from a league change until that league's first sweep
/// lands: nothing, marked as loading. Never the previous league's numbers.
fn loading_snapshot(league: &str) -> Snapshot {
    Snapshot {
        league: league.to_string(),
        loading: true,
        error: None,
        table: PriceTable::default(),
        vocab: crate::pricing::build_vocab(&PriceTable::default()),
        uniques: UniquePrices::default(),
        stale: true,
        uniques_stale: true,
    }
}

fn empty_market(league: &str) -> Market {
    Market { source: MarketSource { league: league.to_string(), ..MarketSource::default() }, ..Market::default() }
}

/// First retry delay while the current snapshot is stale (a fetch failed or
/// came from the on-disk cache): staleness heals itself as soon as the
/// network is back instead of waiting out the full refresh interval.
/// This is what made a manual refresh key unnecessary.
const STALE_RETRY: Duration = Duration::from_secs(60);

/// How long to wait before the next sweep. A healthy table waits the full
/// `interval`. An unhealthy one retries after [`STALE_RETRY`], doubling
/// with every further unhealthy sweep in a row up to `interval`: a full
/// sweep is two dozen requests, and repeating it every minute for as long
/// as one type stays down is how a client gets itself blocked. Never
/// shorter than `STALE_RETRY`, so a zero interval cannot spin.
pub fn next_wait(interval: Duration, unhealthy_in_a_row: u32) -> Duration {
    let interval = interval.max(STALE_RETRY);
    if unhealthy_in_a_row == 0 {
        return interval;
    }
    let doublings = (unhealthy_in_a_row - 1).min(16);
    STALE_RETRY.saturating_mul(1u32 << doublings).min(interval)
}

/// The overviews each type last answered with. A refresh replaces the types
/// that answered and keeps the rest, so a sweep in which three types failed
/// yields the previous table with fifteen types updated - not a table that
/// has silently lost three types' worth of names yet calls itself fresh.
#[derive(Default)]
pub struct LastGood {
    exchange: HashMap<&'static str, ExchangeOverview>,
    uniques: HashMap<&'static str, ItemOverview>,
    /// When each type's overview was fetched, unix seconds: now for a fresh
    /// answer, the cache file's time for one served from disk.
    fetched: HashMap<&'static str, i64>,
    /// Types the latest sweep did not answer fresh.
    old: std::collections::BTreeSet<&'static str>,
}

/// Seconds since the unix epoch; zero if the clock reads before it.
pub fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn unix_of(t: std::time::SystemTime) -> Option<i64> {
    t.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs() as i64)
}

impl LastGood {
    fn answered(&mut self, client: &NinjaClient, league: &str, typ: &'static str, origin: DataOrigin) {
        match origin {
            DataOrigin::Fresh => {
                self.fetched.insert(typ, unix_now());
                self.old.remove(typ);
            }
            DataOrigin::StaleCache => {
                match client.cached_at(league, typ).and_then(unix_of) {
                    Some(t) => self.fetched.insert(typ, t),
                    None => self.fetched.remove(typ),
                };
                self.old.insert(typ);
            }
        }
    }

    /// The market view's input from the overviews held, in the fixed type
    /// order so the same inputs give the same source.
    pub fn market(&self, league: &str) -> Market {
        let exchange: Vec<(&str, &ExchangeOverview)> =
            EXCHANGE_TYPES.iter().filter_map(|t| Some((*t, self.exchange.get(t)?))).collect();
        let listed: Vec<(&str, &ItemOverview)> =
            UNIQUE_TYPES.iter().filter_map(|t| Some((*t, self.uniques.get(t)?))).collect();
        // One type without a known time makes the age of its part unknown.
        let oldest = |types: &mut dyn Iterator<Item = &str>| types.map(|t| self.fetched.get(t).copied()).min().flatten();
        Market {
            source: MarketSource::from_overviews(league, &exchange, &listed),
            fetched_at: oldest(&mut exchange.iter().map(|(t, _)| *t)),
            uniques_fetched_at: oldest(&mut listed.iter().map(|(t, _)| *t)),
            old_categories: self.old.iter().map(|t| t.to_string()).collect(),
            first_record: None,
        }
    }
}

/// The cached overviews of `league` as the market view's input, without a
/// request: the settings window and the offline tools read the files the
/// overlay's refresh wrote, through the same [`MarketSource::from_overviews`].
pub fn market_from_cache(cache_dir: &std::path::Path, league: &str) -> Market {
    use khaloni_poe2_core::ninja::{cached_at, cached_exchange_overview, cached_item_overview};
    let mut last = LastGood::default();
    for typ in EXCHANGE_TYPES {
        if let Some(ov) = cached_exchange_overview(cache_dir, league, typ) {
            last.exchange.insert(typ, ov);
        }
    }
    for typ in UNIQUE_TYPES {
        if let Some(ov) = cached_item_overview(cache_dir, league, typ) {
            last.uniques.insert(typ, ov);
        }
    }
    let held: Vec<&'static str> = last.exchange.keys().chain(last.uniques.keys()).copied().collect();
    for typ in held {
        if let Some(t) = cached_at(cache_dir, league, typ).and_then(unix_of) {
            last.fetched.insert(typ, t);
        }
    }
    last.market(league)
}

/// One sweep over the exchange types. The bool is true when any type is
/// not from this sweep. A type the API answers with no lines, and that has
/// never had any, is a league without that market rather than a failure.
pub fn fetch(client: &NinjaClient, league: &str, last: &mut LastGood) -> anyhow::Result<(PriceTable, bool)> {
    let mut stale = false;
    let mut last_err = None;
    for typ in EXCHANGE_TYPES {
        match client.exchange_overview(league, typ) {
            Ok((ov, origin)) => {
                stale |= origin == DataOrigin::StaleCache;
                last.exchange.insert(typ, ov);
                last.answered(client, league, typ, origin);
            }
            Err(NinjaError::EmptyResponse(_)) if !last.exchange.contains_key(typ) => {}
            Err(e) => {
                stale = true;
                if last.exchange.contains_key(typ) {
                    last.old.insert(typ);
                }
                last_err = Some(e);
            }
        }
    }
    if last.exchange.is_empty() {
        anyhow::bail!("no price data: {last_err:?}");
    }
    // Built in the fixed type order so the same inputs give the same table.
    let overviews: Vec<ExchangeOverview> =
        EXCHANGE_TYPES.iter().filter_map(|t| last.exchange.get(t).cloned()).collect();
    Ok((PriceTable::build(&overviews), stale))
}

/// Unique prices from poe.ninja's unique item overviews first (the same
/// host and cadence as the currency table, live for every league it
/// lists), falling back to poe2scout only when ninja has nothing. Never an
/// error: uniques neither source prices go to the trade site on demand.
/// The bool is true when any of it is not from this sweep.
pub fn fetch_uniques(
    client: &NinjaClient,
    scout: &ScoutClient,
    league: &str,
    last: &mut LastGood,
) -> (UniquePrices, bool) {
    let mut stale = false;
    for typ in UNIQUE_TYPES {
        match client.item_overview(league, typ) {
            Ok((ov, origin)) => {
                stale |= origin == DataOrigin::StaleCache;
                last.uniques.insert(typ, ov);
                last.answered(client, league, typ, origin);
            }
            Err(e) => {
                stale = true;
                if last.uniques.contains_key(typ) {
                    last.old.insert(typ);
                }
                eprintln!("uniques: {typ} unavailable: {e}");
            }
        }
    }
    let overviews: Vec<ItemOverview> = UNIQUE_TYPES.iter().filter_map(|t| last.uniques.get(t).cloned()).collect();
    let prices = unique_prices(&overviews);
    if !prices.is_empty() {
        return (prices, stale);
    }
    match scout.unique_prices(league) {
        Ok((map, scout_stale)) => (UniquePrices::from_names(map), stale || scout_stale),
        Err(e) => {
            eprintln!("uniques: poe2scout unavailable too: {e}");
            (UniquePrices::default(), true)
        }
    }
}

impl PriceService {
    /// Non-blocking start, then a background refresh every
    /// `refresh_minutes`, retrying sooner (see [`next_wait`]) while the
    /// snapshot is stale. On refresh failure the previous snapshot is kept
    /// (never a zeroed table).
    pub fn start(client: NinjaClient, scout: ScoutClient, league: String) -> anyhow::Result<PriceService> {
        Self::start_with_interval(client, scout, league, Duration::from_secs(30 * 60))
    }

    pub fn start_with_interval(
        client: NinjaClient,
        scout: ScoutClient,
        league: String,
        interval: Duration,
    ) -> anyhow::Result<PriceService> {
        // Startup must never block on the network (measured 8s on a live
        // machine): the service starts EMPTY and stale, the overlay comes up
        // immediately, and the first worker-thread fetch below fills prices
        // seconds later. Until then priced rows simply do not appear, the
        // same behavior as any other stale window.
        let inner = Arc::new(RwLock::new(Arc::new(loading_snapshot(&league))));
        let market = Arc::new(RwLock::new(Arc::new(empty_market(&league))));
        // The history log is written from this worker only, after a sweep
        // that brought an exchange table: never from the UI thread.
        let mut history = HistoryLog::open(&client.cache_dir().join("market-history"), &league);
        let ctl = Arc::new((Mutex::new(Ctl { league, epoch: 0 }), Condvar::new()));
        let svc = PriceService { inner: inner.clone(), market: market.clone(), ctl: ctl.clone() };
        std::thread::spawn(move || {
            // Per league: a type that fails keeps its previous lines, and
            // those must be the same league's.
            let mut last: HashMap<String, LastGood> = HashMap::new();
            // Sweeps in a row that failed or left something stale; drives
            // the retry backoff.
            let mut unhealthy = 0u32;
            let mut cycles = 0u32;
            let mut seen_epoch = 0u64;
            loop {
                let (league, epoch) = {
                    let c = ctl.0.lock().unwrap_or_else(|e| e.into_inner());
                    (c.league.clone(), c.epoch)
                };
                if epoch != seen_epoch {
                    seen_epoch = epoch;
                    unhealthy = 0;
                    cycles = 0;
                }
                let prev = inner.read().unwrap().clone();
                let (prev_uniques, prev_uniques_stale) = if prev.league == league {
                    (prev.uniques.clone(), prev.uniques_stale)
                } else {
                    (UniquePrices::default(), true)
                };
                let last = last.entry(league.clone()).or_default();
                let first = cycles == 0;
                // A league name nobody lists would otherwise look like a
                // league with no prices. Asked once per league; when the
                // list cannot be had, the fetch error speaks instead.
                let unlisted = first
                    && client.leagues().is_ok_and(|ls| !ls.iter().any(|l| l.name == league || l.id == league));
                // Uniques are best-effort on every sweep: a failure logs
                // and leaves them to the trade site instead of failing the
                // service. An empty refetch keeps the last set: a transient
                // blank must not blank every unique until the next cycle.
                let due = cycles.is_multiple_of(UNIQUES_EVERY_N_REFRESHES) || prev_uniques_stale;
                let (uniques, uniques_stale) = if due {
                    let (fresh, stale) = fetch_uniques(&client, &scout, &league, last);
                    if fresh.is_empty() && !prev_uniques.is_empty() {
                        (prev_uniques, true)
                    } else {
                        (fresh, stale)
                    }
                } else {
                    (prev_uniques, prev_uniques_stale)
                };
                cycles = cycles.wrapping_add(1);
                let snapshot = match fetch(&client, &league, last) {
                    Ok((table, stale)) => {
                        let recovered = unhealthy > 0 && !stale;
                        unhealthy = if stale || uniques_stale { unhealthy.saturating_add(1) } else { 0 };
                        let vocab = crate::pricing::build_vocab(&table);
                        if first {
                            if uniques.is_empty() {
                                // No bulk source has this league: the hover
                                // check prices uniques through the trade site.
                                eprintln!("no unique price data for {league}; uniques go to the trade site");
                            } else {
                                eprintln!("uniques loaded: {} items (stale={uniques_stale})", uniques.len());
                            }
                            eprintln!("price table ready for {league} ({} names, stale={stale})", table.len());
                        } else if recovered || prev.stale != stale {
                            // Routine refreshes are silent; log only state
                            // changes (a long session otherwise fills the
                            // log with one line per interval — live finding).
                            eprintln!("prices refreshed (stale={stale})");
                        }
                        Snapshot {
                            league: league.clone(),
                            loading: false,
                            error: None,
                            table,
                            vocab,
                            uniques,
                            stale,
                            uniques_stale,
                        }
                    }
                    Err(e) => {
                        // `fetch` fails only while no type has ever
                        // answered, so there is no earlier table to keep:
                        // the snapshot stays empty and stale, and carries
                        // whatever unique prices did arrive.
                        unhealthy = unhealthy.saturating_add(1);
                        eprintln!("price fetch for {league} failed (retrying with backoff): {e}");
                        let error = if unlisted {
                            format!("league \"{league}\" is not one poe.ninja lists: check the name in Settings")
                        } else {
                            format!("no prices for {league}: {e}")
                        };
                        // The reason a league was called unlisted outlives
                        // the sweep that asked.
                        let error = prev.error.clone().filter(|_| prev.league == league && !first).unwrap_or(error);
                        let table = PriceTable::default();
                        let vocab = crate::pricing::build_vocab(&table);
                        Snapshot {
                            league: league.clone(),
                            loading: true,
                            error: Some(error),
                            table,
                            vocab,
                            uniques,
                            stale: true,
                            uniques_stale,
                        }
                    }
                };
                // The market view's input and its history record, from the
                // same overviews the table was built from. A sweep that
                // failed outright has no table and records nothing.
                let swept = if snapshot.loading {
                    empty_market(&league)
                } else {
                    let mut m = last.market(&league);
                    if history.league() != league {
                        // No league list here carries a start date, so
                        // nothing is pruned on the way in.
                        if let Err(e) = history.switch_league(&league, None) {
                            eprintln!("market history: {e}");
                        }
                    }
                    let now = unix_now();
                    let source_t = [m.fetched_at, m.uniques_fetched_at].into_iter().flatten().min().unwrap_or(now);
                    let record = Record::from_source(&m.source, now, source_t);
                    if let Err(e) = history.append(&record) {
                        eprintln!("market history: not written: {e}");
                    }
                    m.first_record = history.first_record_time();
                    m
                };
                let wait = next_wait(interval, unhealthy);
                let mut c = ctl.0.lock().unwrap_or_else(|e| e.into_inner());
                // Published under the lock `set_league` takes: either this
                // lands first and is replaced by the loading snapshot, or
                // the epoch has moved and it is dropped.
                if c.epoch == epoch {
                    *inner.write().unwrap() = Arc::new(snapshot);
                    *market.write().unwrap() = Arc::new(swept);
                }
                let started = std::time::Instant::now();
                while c.epoch == epoch && started.elapsed() < wait {
                    let left = wait.saturating_sub(started.elapsed());
                    c = ctl.1.wait_timeout(c, left).unwrap_or_else(|e| e.into_inner()).0;
                }
            }
        });
        Ok(svc)
    }

    /// Moves the service to another league. From this call on no snapshot
    /// carries the previous league's prices: an empty one marked `loading`
    /// is served until the worker, woken here, has swept the new league.
    /// The disk caches and the per-type memory are keyed by league, so
    /// neither can answer for the wrong one. False when `league` is
    /// already the one being priced.
    pub fn set_league(&self, league: &str) -> bool {
        let mut c = self.ctl.0.lock().unwrap_or_else(|e| e.into_inner());
        if c.league == league {
            return false;
        }
        c.league = league.to_string();
        c.epoch += 1;
        *self.inner.write().unwrap() = Arc::new(loading_snapshot(league));
        *self.market.write().unwrap() = Arc::new(empty_market(league));
        self.ctl.1.notify_all();
        true
    }

    /// The market view's input for the league being priced: empty from a
    /// league change until that league's first sweep lands, like the table.
    pub fn market(&self) -> Arc<Market> {
        self.market.read().unwrap().clone()
    }

    pub fn league(&self) -> String {
        self.ctl.0.lock().unwrap_or_else(|e| e.into_inner()).league.clone()
    }

    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.inner.read().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::sync::Mutex;

    const CURRENCY: &str = include_str!("../../core/tests/fixtures/ninja_currency.json");

    fn fragments() -> String {
        let mut v: serde_json::Value = serde_json::from_str(CURRENCY).unwrap();
        v["items"][0]["name"] = serde_json::json!("Test Fragment");
        v.to_string()
    }

    /// A poe.ninja stub: exchange type -> body, a missing type answers an
    /// overview with no lines, and a type listed in `down` answers 503.
    fn serve(bodies: Arc<Mutex<HashMap<String, String>>>, down: Arc<Mutex<Vec<String>>>) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { break };
                let mut buf = [0u8; 4096];
                let n = s.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let typ = req.split("type=").nth(1).and_then(|r| r.split([' ', '&']).next()).unwrap_or("").to_string();
                let (status, body) = if down.lock().unwrap().contains(&typ) {
                    ("503 Service Unavailable", String::new())
                } else {
                    let empty = r#"{"core":{"items":[],"rates":{"exalted":410.0},"primary":"divine"},"lines":[]}"#;
                    ("200 OK", bodies.lock().unwrap().get(&typ).cloned().unwrap_or_else(|| empty.to_string()))
                };
                let _ = write!(
                    s,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        format!("http://{addr}")
    }

    /// A poe.ninja stub that knows two leagues. "Alpha" prices the pinned
    /// currency fixture; "Beta" prices the same fixture with its first item
    /// renamed, so each league has a name the other lacks. Every request's
    /// league is recorded; Alpha answers slowly while `slow_alpha` is set.
    fn serve_two_leagues(seen: Arc<Mutex<Vec<String>>>, slow_alpha: Arc<std::sync::atomic::AtomicBool>) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { break };
                let mut buf = [0u8; 4096];
                let n = s.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let param = |key: &str| {
                    req.split(key).nth(1).and_then(|r| r.split([' ', '&']).next()).unwrap_or("").to_string()
                };
                let (league, typ) = (param("league="), param("type="));
                let body = if req.contains("/economy/leagues") {
                    r#"[{"id":"alpha","name":"Alpha"},{"id":"beta","name":"Beta"}]"#.to_string()
                } else {
                    seen.lock().unwrap().push(league.clone());
                    if league == "Alpha" && slow_alpha.load(std::sync::atomic::Ordering::Acquire) {
                        std::thread::sleep(Duration::from_millis(40));
                    }
                    match (league.as_str(), typ.as_str()) {
                        ("Alpha", "Currency") => CURRENCY.to_string(),
                        ("Beta", "Currency") => fragments(),
                        _ => r#"{"core":{"items":[],"rates":{"exalted":410.0},"primary":"divine"},"lines":[]}"#.into(),
                    }
                };
                let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            }
        });
        format!("http://{addr}")
    }

    fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
        let started = std::time::Instant::now();
        while !done() {
            assert!(started.elapsed() < Duration::from_secs(20), "timed out waiting until {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_league_change_moves_the_price_service_to_the_new_league() {
        let alpha_only = {
            let v: serde_json::Value = serde_json::from_str(CURRENCY).unwrap();
            v["items"][0]["name"].as_str().unwrap().to_string()
        };
        let dir = std::env::temp_dir().join(format!("khalonipoe2-league-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let slow_alpha = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let base = serve_two_leagues(seen.clone(), slow_alpha.clone());
        let start = |league: &str| {
            PriceService::start_with_interval(
                NinjaClient::with_base(base.clone(), dir.clone()),
                ScoutClient::with_base("http://127.0.0.1:1".into(), dir.clone()),
                league.to_string(),
                Duration::from_secs(3600),
            )
            .unwrap()
        };
        // A snapshot that says Beta must never hold a price only Alpha has.
        let never_mixed = |snap: &Snapshot| {
            if snap.league == "Beta" {
                assert!(snap.table.lookup(&alpha_only).is_none(), "Alpha's {alpha_only} is served as Beta's");
            } else {
                assert!(snap.table.lookup("Test Fragment").is_none(), "Beta's price is served as {}'s", snap.league);
            }
        };

        let svc = start("Alpha");
        let first = svc.snapshot();
        assert!(first.loading && first.league == "Alpha" && first.table.is_empty());
        wait_until("Alpha has loaded", || !svc.snapshot().loading);
        assert!(svc.snapshot().table.lookup(&alpha_only).is_some());
        assert!(!seen.lock().unwrap().iter().any(|l| l == "Beta"));

        assert!(svc.set_league("Beta"));
        assert!(!svc.set_league("Beta"), "the same league again is no change");
        // Served from the very next read: nothing, and it says so.
        let between = svc.snapshot();
        assert_eq!((between.league.as_str(), between.loading), ("Beta", true));
        assert!(between.table.is_empty() && between.uniques.is_empty() && between.stale);
        let asked_before = seen.lock().unwrap().len();
        wait_until("Beta has loaded", || {
            let snap = svc.snapshot();
            never_mixed(&snap);
            !snap.loading
        });
        let loaded = svc.snapshot();
        assert_eq!(loaded.league, "Beta");
        assert!(loaded.table.lookup("Test Fragment").is_some() && loaded.error.is_none());
        // The sweep that followed asked for the new league only, without
        // waiting out the hour-long refresh interval.
        let after: Vec<String> = seen.lock().unwrap()[asked_before..].to_vec();
        assert!(!after.is_empty() && after.iter().all(|l| l == "Beta"), "asked for {after:?}");

        // A change while the old league's sweep is still on the wire: that
        // sweep finishes later and must not land under the new name.
        slow_alpha.store(true, std::sync::atomic::Ordering::Release);
        let _ = std::fs::remove_dir_all(&dir);
        let racing = start("Alpha");
        wait_until("the Alpha sweep is under way", || seen.lock().unwrap().last().is_some_and(|l| l == "Alpha"));
        assert!(racing.set_league("Beta"));
        let watch = std::time::Instant::now();
        while watch.elapsed() < Duration::from_millis(2500) {
            let snap = racing.snapshot();
            assert_eq!(snap.league, "Beta", "the abandoned sweep took the service back");
            never_mixed(&snap);
            std::thread::sleep(Duration::from_millis(5));
        }
        // Beta's own sweep may still be running: on Windows each refused
        // connection to the unreachable scout costs about two seconds.
        wait_until("Beta has loaded after the race", || {
            let snap = racing.snapshot();
            assert_eq!(snap.league, "Beta", "the abandoned sweep took the service back");
            never_mixed(&snap);
            !snap.loading
        });
        assert!(racing.snapshot().table.lookup("Test Fragment").is_some());

        // A league nobody lists is an error the user can read, not an
        // empty table.
        assert!(racing.set_league("Gamma"));
        wait_until("Gamma has failed", || racing.snapshot().error.is_some());
        let failed = racing.snapshot();
        assert!(failed.loading && failed.table.is_empty());
        assert!(failed.error.as_deref().unwrap().contains("\"Gamma\" is not one poe.ninja lists"), "{:?}", failed.error);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_failed_type_keeps_its_previous_prices_and_marks_the_table_stale() {
        let dir = std::env::temp_dir().join(format!("khalonipoe2-prices-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let bodies = Arc::new(Mutex::new(HashMap::from([
            ("Currency".to_string(), CURRENCY.to_string()),
            ("Fragments".to_string(), fragments()),
        ])));
        let down = Arc::new(Mutex::new(Vec::new()));
        let client = NinjaClient::with_base(serve(bodies, down.clone()), dir.clone());
        let mut last = LastGood::default();

        // Sixteen types have no market in this league; that is not staleness.
        let (table, stale) = fetch(&client, "Test League", &mut last).expect("first sweep");
        assert!(!stale);
        assert!(table.lookup("Orb of Annulment").is_some() && table.lookup("Test Fragment").is_some());

        // Fragments goes down, and its disk cache with it: the sweep used
        // to rebuild the table from the types that answered, dropping every
        // fragment while reporting fresh prices.
        down.lock().unwrap().push("Fragments".to_string());
        std::fs::remove_file(dir.join("Test League-Fragments.json")).unwrap();
        let (table, stale) = fetch(&client, "Test League", &mut last).expect("second sweep");
        assert!(stale, "a failed type must show");
        assert!(table.lookup("Test Fragment").is_some(), "the failed type keeps its last prices");
        assert!(table.lookup("Orb of Annulment").is_some());

        // Back up: fresh again.
        down.lock().unwrap().clear();
        let (_, stale) = fetch(&client, "Test League", &mut last).expect("third sweep");
        assert!(!stale);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn stale_unique_overviews_are_reported_stale() {
        let dir = std::env::temp_dir().join(format!("khalonipoe2-uniques-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Only the disk cache can answer: everything is unreachable.
        std::fs::write(
            dir.join("Test League-UniqueWeapons.json"),
            include_str!("../../core/tests/fixtures/ninja_unique_weapons.json"),
        )
        .unwrap();
        let client = NinjaClient::with_base("http://127.0.0.1:1".into(), dir.clone());
        let scout = ScoutClient::with_base("http://127.0.0.1:1".into(), dir.clone());
        let (uniques, stale) = fetch_uniques(&client, &scout, "Test League", &mut LastGood::default());
        assert!(!uniques.is_empty());
        assert!(stale, "prices served from the cache are not this sweep's");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn retries_back_off_up_to_the_interval_and_never_spin() {
        let ten_min = Duration::from_secs(600);
        assert_eq!(next_wait(ten_min, 0), ten_min);
        assert_eq!(next_wait(ten_min, 1), Duration::from_secs(60));
        assert_eq!(next_wait(ten_min, 2), Duration::from_secs(120));
        assert_eq!(next_wait(ten_min, 3), Duration::from_secs(240));
        assert_eq!(next_wait(ten_min, 4), Duration::from_secs(480));
        assert_eq!(next_wait(ten_min, 5), ten_min, "capped at the interval");
        assert_eq!(next_wait(ten_min, u32::MAX), ten_min);
        // A zero interval (refresh_minutes = 0) must not become a busy loop.
        assert_eq!(next_wait(Duration::ZERO, 0), STALE_RETRY);
        assert_eq!(next_wait(Duration::ZERO, 7), STALE_RETRY);
    }
}
