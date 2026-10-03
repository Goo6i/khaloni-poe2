use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;
use thiserror::Error;

use crate::matcher::normalize;

pub const DEFAULT_BASE: &str = "https://poe.ninja";
pub const USER_AGENT: &str = concat!("khaloni-poe2/", env!("CARGO_PKG_VERSION"));

/// Exchange types verified to return data on the PoE2 API (2026-07-21).
// Every poe.ninja PoE2 exchange type that trades in the in-game currency
// exchange. Named skill/support gems ("Uruk's Smelting"), idols, and
// verisium are traded here just like orbs, so they must be pulled or they
// price as "?" (live-verified 2026-07-23: LineageSupportGems 75 items,
// Idols 32, Verisium 24, all divine-denominated). Types that come back
// empty this league (Omens, Catalysts, Artifacts right now) are harmless:
// prices::fetch skips an empty/malformed type and keeps the rest.
pub const EXCHANGE_TYPES: [&str; 18] = [
    "Currency", "Fragments", "Essences", "Runes", "UncutGems", "LineageSupportGems", "Omens",
    "Catalysts", "Artifacts", "SoulCores", "Talismans", "Expedition", "Ritual", "Breach",
    "Delirium", "Abyss", "Idols", "Verisium",
];

#[derive(Debug, Error)]
pub enum NinjaError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("bad json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("empty response for type {0} (unknown type or no data)")]
    EmptyResponse(String),
    #[error("malformed response for type {0}: {1}")]
    MalformedResponse(String, &'static str),
    #[error("network failed and no cache available: {0}")]
    NoData(String),
}

#[derive(Debug, Clone, Deserialize)]
pub struct League {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogItem {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub details_id: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreBlock {
    pub items: Vec<CatalogItem>,
    pub rates: HashMap<String, f64>,
    pub primary: String,
    #[serde(default)]
    pub secondary: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExchangeLine {
    pub id: String,
    pub primary_value: f64,
    #[serde(default)]
    pub volume_primary_value: Option<f64>,
    #[serde(default)]
    pub max_volume_currency: Option<String>,
    #[serde(default)]
    pub max_volume_rate: Option<f64>,
    /// Seven daily points, see [`Sparkline`].
    #[serde(default)]
    pub sparkline: Option<Sparkline>,
}

/// poe.ninja's week of history for one line: `data[i]` is the cumulative
/// percent change against the first day, oldest first, `null` where the
/// source has no figure for that day. The exchange overview spells the key
/// `sparkline`, the item overview `sparkLine`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sparkline {
    #[serde(default)]
    pub data: Vec<Option<f64>>,
    #[serde(default)]
    pub total_change: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExchangeOverview {
    pub core: CoreBlock,
    pub lines: Vec<ExchangeLine>,
    #[serde(default)]
    pub items: Vec<CatalogItem>,
}

/// poe.ninja's PoE2 unique item categories on the stash item overview
/// (`/poe2/api/economy/stash/current/item/overview`, documented at
/// poe.ninja/docs/api; all eight answered with data for Forbidden Rites on
/// 2026-09-08). PrecursorTablets is the same endpoint but rare tablets, not
/// uniques, so it is deliberately absent.
pub const UNIQUE_TYPES: [&str; 8] = [
    "UniqueWeapons", "UniqueArmours", "UniqueAccessories", "UniqueFlasks", "UniqueCharms",
    "UniqueJewels", "UniqueSanctumRelics", "UniqueTablets",
];

/// One priced unique on the item overview. A name can appear on several
/// lines: one per base type it exists on, and corrupted variants.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemLine {
    pub name: String,
    #[serde(default)]
    pub base_type: Option<String>,
    /// Price in the overview's primary currency (divine on PoE2).
    pub primary_value: f64,
    #[serde(default)]
    pub listing_count: Option<u32>,
    #[serde(default)]
    pub corrupted: Option<bool>,
    #[serde(default)]
    pub spark_line: Option<Sparkline>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ItemOverview {
    pub core: CoreBlock,
    pub lines: Vec<ItemLine>,
}

/// What the unique price data says about one item.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UniqueMatch {
    /// A line for exactly this name, base type and corruption state, in
    /// exalted.
    Exact(f64),
    /// The name is priced, but not as this variant: on other bases, in the
    /// other corruption state, or on lines that disagree. Those prices are
    /// for different items (live: "Alpha's Howl" at 0.235 div on one base
    /// and 3.0 div on another), so none of them is offered.
    Ambiguous,
    /// No line carries the name.
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
struct UniqueVariant {
    /// `None` when the source does not say which base it priced.
    base: Option<String>,
    corrupted: bool,
    exalted: f64,
    /// Two lines claimed this same variant at different prices.
    conflicted: bool,
}

/// Unique prices in exalted, answerable per (name, base type, corrupted).
/// A unique's name is shared by every base it drops on and by its corrupted
/// copies, each its own market; keying by name alone let one of them speak
/// for all.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UniquePrices {
    by_name: HashMap<String, Vec<UniqueVariant>>,
}

fn same_base(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

impl UniquePrices {
    /// From a source that prices by name only (poe2scout). Such an entry
    /// stands for the ordinary uncorrupted item on whatever base, which is
    /// all a name-only source can mean; a corrupted item never matches it.
    pub fn from_names(names: HashMap<String, f64>) -> UniquePrices {
        let mut out = UniquePrices::default();
        for (name, exalted) in names {
            out.insert(&name, None, false, exalted);
        }
        out
    }

    fn insert(&mut self, name: &str, base: Option<&str>, corrupted: bool, exalted: f64) {
        let variants = self.by_name.entry(name.to_string()).or_default();
        let same = variants.iter_mut().find(|v| {
            v.corrupted == corrupted
                && match (&v.base, base) {
                    (Some(a), Some(b)) => same_base(a, b),
                    (None, None) => true,
                    _ => false,
                }
        });
        match same {
            Some(v) if (v.exalted - exalted).abs() > f64::EPSILON * v.exalted.abs().max(1.0) => v.conflicted = true,
            Some(_) => {}
            None => variants.push(UniqueVariant {
                base: base.map(str::to_string),
                corrupted,
                exalted,
                conflicted: false,
            }),
        }
    }

    /// The price of exactly this item. `base` is the item's base type when
    /// the caller knows it; without one, a name answers only if it has a
    /// single variant in that corruption state, so there is nothing it
    /// could be confused with.
    pub fn lookup(&self, name: &str, base: Option<&str>, corrupted: bool) -> UniqueMatch {
        let Some(variants) = self.by_name.get(name) else {
            return UniqueMatch::Unknown;
        };
        let mut candidates = variants.iter().filter(|v| {
            v.corrupted == corrupted
                && match (&v.base, base) {
                    (Some(have), Some(want)) => same_base(have, want),
                    // A name-only line, or an item whose base is not known.
                    _ => true,
                }
        });
        match (candidates.next(), candidates.next()) {
            (Some(v), None) if !v.conflicted => UniqueMatch::Exact(v.exalted),
            _ => UniqueMatch::Ambiguous,
        }
    }

    /// Distinct unique names carried.
    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    /// Adds every name of `other` this set does not carry yet. A name
    /// already here keeps its variants untouched: two sources' lines for
    /// one name are not mixed.
    pub fn fill_from(&mut self, other: &UniquePrices) {
        for (name, variants) in &other.by_name {
            self.by_name.entry(name.clone()).or_insert_with(|| variants.clone());
        }
    }

    /// Replaces the names `newer` carries and keeps the rest, so a refresh
    /// in which one category failed does not drop that category's prices.
    pub fn merged_with(&self, newer: &UniquePrices) -> UniquePrices {
        let mut out = newer.clone();
        out.fill_from(self);
        out
    }
}

/// Unique prices in exalted across item overviews, one entry per (name,
/// base type, corrupted) line. An overview whose primary currency is not
/// divine, or which carries no exalted rate, is skipped entirely rather
/// than mis-scaled.
pub fn unique_prices(overviews: &[ItemOverview]) -> UniquePrices {
    let mut out = UniquePrices::default();
    for ov in overviews {
        if ov.core.primary != "divine" {
            continue;
        }
        let Some(&ex) = ov.core.rates.get("exalted").filter(|r| **r > 0.0) else {
            continue;
        };
        for line in &ov.lines {
            if !(line.primary_value.is_finite() && line.primary_value > 0.0) {
                continue;
            }
            out.insert(
                &line.name,
                line.base_type.as_deref().filter(|b| !b.trim().is_empty()),
                line.corrupted.unwrap_or(false),
                line.primary_value * ex,
            );
        }
    }
    out
}

/// Writes `bytes` to `path` so that a reader sees the old file or the new
/// one, never part of each: a uniquely named temp file beside it, synced,
/// then renamed over it. A cache written in place and torn by a crash
/// parses as garbage on every later launch.
pub fn write_cache_atomic(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(std::path::Path::new("."));
    std::fs::create_dir_all(dir)?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("cache");
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = dir.join(format!(".{name}.{}.{seq}.tmp", std::process::id()));
    let written = (|| {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

/// Where the overview of (league, typ) is cached under `cache_dir`.
pub fn cache_file(cache_dir: &std::path::Path, league: &str, typ: &str) -> PathBuf {
    let safe: String = format!("{league}-{typ}")
        .chars()
        .map(|c| if c == '/' || c == '\\' { '_' } else { c })
        .collect();
    cache_dir.join(format!("{safe}.json"))
}

/// When the cached overview of (league, typ) was written, which is when its
/// figures were fetched.
pub fn cached_at(cache_dir: &std::path::Path, league: &str, typ: &str) -> Option<std::time::SystemTime> {
    std::fs::metadata(cache_file(cache_dir, league, typ)).and_then(|m| m.modified()).ok()
}

/// The cached exchange overview of (league, typ), held to the same checks a
/// fetched one passes. For readers that must not touch the network: the
/// settings window and the offline tools.
pub fn cached_exchange_overview(cache_dir: &std::path::Path, league: &str, typ: &str) -> Option<ExchangeOverview> {
    let body = std::fs::read_to_string(cache_file(cache_dir, league, typ)).ok()?;
    let ov: ExchangeOverview = serde_json::from_str(&body).ok()?;
    NinjaClient::validate(&ov, typ).ok()?;
    Some(ov)
}

/// The cached item overview of (league, typ); see
/// [`cached_exchange_overview`].
pub fn cached_item_overview(cache_dir: &std::path::Path, league: &str, typ: &str) -> Option<ItemOverview> {
    let body = std::fs::read_to_string(cache_file(cache_dir, league, typ)).ok()?;
    let ov: ItemOverview = serde_json::from_str(&body).ok()?;
    (ov.core.primary == "divine").then_some(ov)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataOrigin {
    Fresh,
    StaleCache,
}

pub struct NinjaClient {
    http: reqwest::blocking::Client,
    base: String,
    cache_dir: PathBuf,
}

impl NinjaClient {
    pub fn new(cache_dir: PathBuf) -> NinjaClient {
        Self::with_base(DEFAULT_BASE.to_string(), cache_dir)
    }

    pub fn with_base(base: String, cache_dir: PathBuf) -> NinjaClient {
        let http = reqwest::blocking::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(10))
            .build()
            .expect("client build");
        NinjaClient {
            http,
            base,
            cache_dir,
        }
    }

    pub fn leagues(&self) -> Result<Vec<League>, NinjaError> {
        let url = format!("{}/poe2/api/economy/leagues", self.base);
        Ok(self.http.get(url).send()?.error_for_status()?.json()?)
    }

    pub fn validate(ov: &ExchangeOverview, typ: &str) -> Result<(), NinjaError> {
        if ov.lines.is_empty() {
            return Err(NinjaError::EmptyResponse(typ.to_string()));
        }
        // PriceTable::build assumes primary values are denominated in divine;
        // any other primary would silently mis-scale every price.
        if ov.core.primary != "divine" {
            return Err(NinjaError::MalformedResponse(
                typ.to_string(),
                "core.primary is not \"divine\"",
            ));
        }
        // a missing or non-positive exalted rate makes every Price.exalted 0.0
        // and renders as "0.00 ex" instead of surfacing the "?" fallback.
        match ov.core.rates.get("exalted") {
            Some(rate) if *rate > 0.0 => {}
            _ => {
                return Err(NinjaError::MalformedResponse(
                    typ.to_string(),
                    "core.rates[\"exalted\"] is missing or not positive",
                ));
            }
        }
        Ok(())
    }

    fn cache_path(&self, league: &str, typ: &str) -> PathBuf {
        cache_file(&self.cache_dir, league, typ)
    }

    /// When this client's cached overview of (league, typ) was written.
    pub fn cached_at(&self, league: &str, typ: &str) -> Option<std::time::SystemTime> {
        cached_at(&self.cache_dir, league, typ)
    }

    /// Where this client caches; the market history log lives under it.
    pub fn cache_dir(&self) -> &std::path::Path {
        &self.cache_dir
    }

    pub fn exchange_overview(
        &self,
        league: &str,
        typ: &str,
    ) -> Result<(ExchangeOverview, DataOrigin), NinjaError> {
        let url = format!(
            "{}/poe2/api/economy/exchange/current/overview?league={}&type={}",
            self.base,
            urlencode(league),
            typ
        );
        self.fetch_cached(&url, league, typ, |ov| Self::validate(ov, typ))
    }

    /// One unique item category (see [`UNIQUE_TYPES`]), same fresh-then-
    /// stale-cache contract as `exchange_overview`. An empty `lines` is
    /// valid here: a category can legitimately have nothing listed early in
    /// a league.
    pub fn item_overview(
        &self,
        league: &str,
        typ: &str,
    ) -> Result<(ItemOverview, DataOrigin), NinjaError> {
        let url = format!(
            "{}/poe2/api/economy/stash/current/item/overview?league={}&type={}",
            self.base,
            urlencode(league),
            typ
        );
        self.fetch_cached(&url, league, typ, |ov: &ItemOverview| {
            if ov.core.primary != "divine" {
                return Err(NinjaError::MalformedResponse(
                    typ.to_string(),
                    "core.primary is not \"divine\"",
                ));
            }
            Ok(())
        })
    }

    /// Fetches `url`; a body that parses and validates is cached under
    /// (league, typ) and returned as fresh. Anything else - a transport
    /// failure, an error status, a 200 whose body does not parse or fails
    /// validation (a challenge page, an emptied overview) - serves the last
    /// good cached body as stale data, and is an error only when there is
    /// none.
    /// The cache is replaced only by a body that passed, so a bad answer
    /// can never push out a good one.
    fn fetch_cached<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        league: &str,
        typ: &str,
        validate: impl Fn(&T) -> Result<(), NinjaError>,
    ) -> Result<(T, DataOrigin), NinjaError> {
        let parse = |body: &str| -> Result<T, NinjaError> {
            let ov: T = serde_json::from_str(body)?;
            validate(&ov)?;
            Ok(ov)
        };
        let path = self.cache_path(league, typ);
        let fetched: Result<T, NinjaError> = (|| {
            let body = self.http.get(url).send()?.error_for_status()?.text()?;
            let ov = parse(&body)?;
            // Losing the cache write costs offline resilience later, not
            // the prices in hand.
            let _ = write_cache_atomic(&path, body.as_bytes());
            Ok(ov)
        })();
        match fetched {
            Ok(ov) => Ok((ov, DataOrigin::Fresh)),
            Err(fetch_err) => match std::fs::read_to_string(&path).map_err(NinjaError::from).and_then(|b| parse(&b)) {
                Ok(ov) => Ok((ov, DataOrigin::StaleCache)),
                // With nothing cached, a transport failure is "no data";
                // a body the API did send keeps its own, more telling error
                // (an empty type is a definitive answer, not an outage).
                Err(_) => Err(match fetch_err {
                    NinjaError::Http(e) => NinjaError::NoData(format!("{typ} for {league}: {e}")),
                    other => other,
                }),
            },
        }
    }
}

fn urlencode(s: &str) -> String {
    s.replace(' ', "%20")
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Price {
    pub divine: f64,
    pub exalted: f64,
    pub chaos: f64,
}

#[derive(Default)]
pub struct PriceTable {
    by_name: HashMap<String, Price>,
    pub exalted_per_divine: f64,
    pub chaos_per_divine: f64,
}

impl PriceTable {
    pub fn build(overviews: &[ExchangeOverview]) -> PriceTable {
        let mut by_name = HashMap::new();
        let mut exalted_per_divine = 0.0;
        let mut chaos_per_divine = 0.0;

        for ov in overviews {
            let ex = ov.core.rates.get("exalted").copied().unwrap_or(0.0);
            let ch = ov.core.rates.get("chaos").copied().unwrap_or(0.0);
            if ex > 0.0 {
                exalted_per_divine = ex;
            }
            if ch > 0.0 {
                chaos_per_divine = ch;
            }

            let mut names: HashMap<&str, &str> = HashMap::new();
            for it in ov.items.iter().chain(ov.core.items.iter()) {
                names.insert(it.id.as_str(), it.name.as_str());
            }
            for line in &ov.lines {
                if let Some(display) = names.get(line.id.as_str()) {
                    by_name.insert(
                        normalize(display),
                        Price {
                            divine: line.primary_value,
                            exalted: line.primary_value * ex,
                            chaos: line.primary_value * ch,
                        },
                    );
                }
            }
            // the primary currency itself has no line; synthesize it
            if ov.core.primary == "divine" {
                by_name.entry(normalize("Divine Orb")).or_insert(Price {
                    divine: 1.0,
                    exalted: ex,
                    chaos: ch,
                });
            }
        }

        PriceTable {
            by_name,
            exalted_per_divine,
            chaos_per_divine,
        }
    }

    /// A value known only in exalted (a trade listing, a poe2scout unique,
    /// an exchange rate) in all three currencies, through this table's
    /// rates. A rate the table does not carry leaves that side at zero,
    /// which the display reads as "not available in this currency".
    pub fn price_from_exalted(&self, exalted: f64) -> Price {
        let divine = if self.exalted_per_divine > 0.0 { exalted / self.exalted_per_divine } else { 0.0 };
        Price { divine, exalted, chaos: divine * self.chaos_per_divine }
    }

    pub fn lookup(&self, name: &str) -> Option<&Price> {
        self.by_name.get(&normalize(name))
    }

    /// Normalized display names of every priced entry, for building a matcher Vocab.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.by_name.keys().map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }
}
