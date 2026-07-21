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
    force: Arc<std::sync::atomic::AtomicBool>,
}

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
    /// `refresh_minutes` (checked each second so `refresh_now` takes
    /// effect immediately). On refresh failure the previous snapshot is
    /// kept (never a zeroed table).
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
        let force = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let force_bg = force.clone();
        let svc = PriceService {
            inner: inner.clone(),
            force,
        };
        std::thread::spawn(move || loop {
            let started = std::time::Instant::now();
            while started.elapsed() < interval
                && !force_bg.swap(false, std::sync::atomic::Ordering::Relaxed)
            {
                std::thread::sleep(Duration::from_secs(1));
            }
            match fetch(&client, &league) {
                Ok((table, stale)) => {
                    let vocab = crate::pricing::build_vocab(&table);
                    *inner.write().unwrap() = Arc::new(Snapshot { table, vocab, stale });
                    eprintln!("prices refreshed (stale={stale})");
                }
                Err(e) => eprintln!("price refresh failed, keeping last table: {e}"),
            }
        });
        Ok(svc)
    }

    /// Requests an immediate refresh; the background thread notices
    /// within a second.
    pub fn refresh_now(&self) {
        self.force.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.inner.read().unwrap().clone()
    }
}
