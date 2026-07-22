use std::sync::{Arc, RwLock};
use std::time::Duration;

use poe2_lens_core::matcher::Vocab;
use poe2_lens_core::ninja::{DataOrigin, NinjaClient, PriceTable, EXCHANGE_TYPES};

pub struct Snapshot {
    pub table: PriceTable,
    pub vocab: Vocab,
    pub stale: bool,
}

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

impl PriceService {
    /// Blocking initial fetch, then a background refresh every
    /// `refresh_minutes`, dropping to STALE_RETRY while the snapshot is
    /// stale. On refresh failure the previous snapshot is kept (never a
    /// zeroed table).
    pub fn start(client: NinjaClient, league: String) -> anyhow::Result<PriceService> {
        Self::start_with_interval(client, league, Duration::from_secs(30 * 60))
    }

    pub fn start_with_interval(
        client: NinjaClient,
        league: String,
        interval: Duration,
    ) -> anyhow::Result<PriceService> {
        let (table, stale) = fetch(&client, &league)?;
        let vocab = crate::pricing::build_vocab(&table);
        let inner = Arc::new(RwLock::new(Arc::new(Snapshot { table, vocab, stale })));
        let svc = PriceService { inner: inner.clone() };
        std::thread::spawn(move || {
            // Tracks outright fetch failures, which keep the old snapshot
            // (whose stale flag then understates the data's age): either
            // signal arms the fast retry.
            let mut last_failed = false;
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
                match fetch(&client, &league) {
                    Ok((table, stale)) => {
                        last_failed = false;
                        let vocab = crate::pricing::build_vocab(&table);
                        *inner.write().unwrap() = Arc::new(Snapshot { table, vocab, stale });
                        eprintln!("prices refreshed (stale={stale})");
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
