use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use khaloni_poe2_core::matcher::Vocab;
use khaloni_poe2_core::ninja::{unique_prices, DataOrigin, NinjaClient, PriceTable, EXCHANGE_TYPES, UNIQUE_TYPES};
use khaloni_poe2_core::scout::ScoutClient;

pub struct Snapshot {
    pub table: PriceTable,
    pub vocab: Vocab,
    /// Unique item name -> exalted price (poe2scout). Empty when the
    /// fetch failed with no cache: uniques then price as "?", exactly the
    /// pre-feature behavior, never an error.
    pub uniques: HashMap<String, f64>,
    pub stale: bool,
}

/// Uniques move slowly compared to currency; refetching them every
/// currency cycle would hammer poe2scout for nothing. One fetch per this
/// many currency refreshes (default interval 10 min -> uniques every 30).
const UNIQUES_EVERY_N_REFRESHES: u32 = 3;

#[derive(Clone)]
pub struct PriceService {
    inner: Arc<RwLock<Arc<Snapshot>>>,
}

/// Retry cadence while the current snapshot is stale (a fetch failed or
/// came from the on-disk cache): staleness heals itself as soon as the
/// network is back instead of waiting out the full refresh interval.
/// This is what made a manual refresh key unnecessary.
const STALE_RETRY: Duration = Duration::from_secs(60);

fn fetch(client: &NinjaClient, league: &str) -> anyhow::Result<(PriceTable, bool)> {
    let mut overviews = Vec::new();
    let mut any_stale = false;
    let mut last_err = None;
    for typ in EXCHANGE_TYPES {
        match client.exchange_overview(league, typ) {
            Ok((ov, origin)) => {
                any_stale |= origin == DataOrigin::StaleCache;
                overviews.push(ov);
            }
            Err(e) => last_err = Some(e),
        }
    }
    if overviews.is_empty() {
        anyhow::bail!("no price data: {last_err:?}");
    }
    Ok((PriceTable::build(&overviews), any_stale))
}

/// Unique name -> exalted, from poe.ninja's unique item overviews first
/// (the same host and cadence as the currency table, live for every league
/// it lists), falling back to poe2scout only when ninja has nothing. Never
/// an error: uniques neither source prices go to the trade site on demand.
fn fetch_uniques(client: &NinjaClient, scout: &ScoutClient, league: &str) -> (HashMap<String, f64>, bool) {
    let mut overviews = Vec::new();
    let mut stale = false;
    for typ in UNIQUE_TYPES {
        match client.item_overview(league, typ) {
            Ok((ov, origin)) => {
                stale |= origin == DataOrigin::StaleCache;
                overviews.push(ov);
            }
            Err(e) => eprintln!("uniques: {typ} unavailable: {e}"),
        }
    }
    let map = unique_prices(&overviews);
    if !map.is_empty() {
        return (map, stale);
    }
    match scout.unique_prices(league) {
        Ok((map, scout_stale)) => (map, scout_stale),
        Err(e) => {
            eprintln!("uniques: poe2scout unavailable too: {e}");
            (HashMap::new(), true)
        }
    }
}

impl PriceService {
    /// Blocking initial fetch, then a background refresh every
    /// `refresh_minutes`, dropping to STALE_RETRY while the snapshot is
    /// stale. On refresh failure the previous snapshot is kept (never a
    /// zeroed table).
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
        let inner = Arc::new(RwLock::new(Arc::new(Snapshot {
            table: PriceTable::default(),
            vocab: crate::pricing::build_vocab(&PriceTable::default()),
            uniques: HashMap::new(),
            stale: true,
        })));
        let svc = PriceService { inner: inner.clone() };
        std::thread::spawn(move || {
            // Initial fetch, off the startup path. Uniques are best-effort
            // here and on every refetch: a failure logs and prices uniques
            // as "?" instead of failing the service.
            match fetch(&client, &league) {
                Ok((table, stale)) => {
                    let vocab = crate::pricing::build_vocab(&table);
                    let (uniques, uniques_stale) = fetch_uniques(&client, &scout, &league);
                    if uniques.is_empty() {
                        // No bulk source has this league: the hover check
                        // prices uniques through the trade site instead.
                        eprintln!("no unique price data for {league}; uniques go to the trade site");
                    } else {
                        eprintln!("uniques loaded: {} items (stale={uniques_stale})", uniques.len());
                    }
                    eprintln!("price table ready ({} names, stale={stale})", table.len());
                    *inner.write().unwrap() = Arc::new(Snapshot { table, vocab, uniques, stale });
                }
                Err(e) => eprintln!("initial price fetch failed (retrying on the stale cadence): {e}"),
            }
            // Tracks outright fetch failures, which keep the old snapshot
            // (whose stale flag then understates the data's age): either
            // signal arms the fast retry.
            let mut last_failed = false;
            let mut cycles = 0u32;
            loop {
                let started = std::time::Instant::now();
                loop {
                    let wait = if last_failed || inner.read().unwrap().stale {
                        STALE_RETRY.min(interval)
                    } else {
                        interval
                    };
                    if started.elapsed() >= wait {
                        break;
                    }
                    std::thread::sleep(Duration::from_secs(1));
                }
                cycles = cycles.wrapping_add(1);
                let uniques = if cycles.is_multiple_of(UNIQUES_EVERY_N_REFRESHES) {
                    // An empty refetch keeps the last map: a transient blank
                    // must not blank every unique until the next cycle.
                    Some(fetch_uniques(&client, &scout, &league).0).filter(|m| !m.is_empty())
                } else {
                    None
                };
                match fetch(&client, &league) {
                    Ok((table, stale)) => {
                        let recovered = last_failed;
                        last_failed = false;
                        let vocab = crate::pricing::build_vocab(&table);
                        let prev = inner.read().unwrap().clone();
                        let uniques = uniques.unwrap_or_else(|| prev.uniques.clone());
                        let was_stale = prev.stale;
                        *inner.write().unwrap() = Arc::new(Snapshot { table, vocab, uniques, stale });
                        // Routine refreshes are silent; log only state
                        // changes (a long session otherwise fills the log
                        // with one line per interval — live finding).
                        if recovered || was_stale != stale {
                            eprintln!("prices refreshed (stale={stale})");
                        }
                    }
                    Err(e) => {
                        last_failed = true;
                        eprintln!("price refresh failed, keeping last table: {e}");
                    }
                }
            }
        });
        Ok(svc)
    }

    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.inner.read().unwrap().clone()
    }
}
