use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;
use thiserror::Error;

use crate::matcher::normalize;

pub const DEFAULT_BASE: &str = "https://poe.ninja";
pub const USER_AGENT: &str = concat!("poe2-lens/", env!("CARGO_PKG_VERSION"));

/// Exchange types verified to return data on the PoE2 API (2026-07-21).
pub const EXCHANGE_TYPES: [&str; 15] = [
    "Currency", "Fragments", "Essences", "Runes", "UncutGems", "Omens", "Catalysts",
    "Artifacts", "SoulCores", "Talismans", "Expedition", "Ritual", "Breach", "Delirium",
    "Abyss",
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
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExchangeOverview {
    pub core: CoreBlock,
    pub lines: Vec<ExchangeLine>,
    #[serde(default)]
    pub items: Vec<CatalogItem>,
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
        let safe: String = format!("{league}-{typ}")
            .chars()
            .map(|c| if c == '/' || c == '\\' { '_' } else { c })
            .collect();
        self.cache_dir.join(format!("{safe}.json"))
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
        let fetched: Result<String, NinjaError> = (|| {
            let body = self.http.get(&url).send()?.error_for_status()?.text()?;
            Ok(body)
        })();
        match fetched {
            Ok(body) => {
                let ov: ExchangeOverview = serde_json::from_str(&body)?;
                Self::validate(&ov, typ)?;
                std::fs::create_dir_all(&self.cache_dir)?;
                std::fs::write(self.cache_path(league, typ), &body)?;
                Ok((ov, DataOrigin::Fresh))
            }
            Err(fetch_err) => {
                let path = self.cache_path(league, typ);
                match std::fs::read_to_string(&path) {
                    Ok(body) => {
                        let ov: ExchangeOverview = serde_json::from_str(&body)?;
                        Ok((ov, DataOrigin::StaleCache))
                    }
                    Err(_) => Err(NinjaError::NoData(format!(
                        "{typ} for {league}: {fetch_err}"
                    ))),
                }
            }
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
