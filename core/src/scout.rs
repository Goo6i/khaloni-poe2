//! poe2scout client for unique item prices (poe.ninja's PoE2 economy API
//! carries currency only; uniques live on poe2scout). Same resilience
//! pattern as ninja.rs: fresh fetch cached to disk, stale cache served
//! when the network fails.
//!
//! API shape (live-verified 2026-07-23 against api.poe2scout.com):
//! - GET /Realms/poe2/Filters -> {"Filters": [{"DisplayName", "Category",
//!   "Identifier", "ItemKind"}]} where ItemKind "unique" rows carry the
//!   category apiIds worth paging.
//! - GET /poe2/Leagues/{league}/Uniques/ByCategory?category={c}&page={n}
//!   &perPage=250 -> {"CurrentPage", "Pages", "Total", "Items": [{"Name",
//!   "CurrentPrice", ...}]} with CurrentPrice denominated in exalted.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

pub const DEFAULT_BASE: &str = "https://api.poe2scout.com";

#[derive(Debug, thiserror::Error)]
pub enum ScoutError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("bad json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("no unique data: {0}")]
    NoData(String),
}

#[derive(Debug, Deserialize)]
struct FiltersResponse {
    #[serde(rename = "Filters")]
    filters: Vec<Filter>,
}

#[derive(Debug, Deserialize)]
struct Filter {
    #[serde(rename = "Category")]
    category: String,
    #[serde(rename = "ItemKind")]
    item_kind: String,
}

#[derive(Debug, Deserialize)]
struct UniquePage {
    #[serde(rename = "Pages")]
    pages: u32,
    #[serde(rename = "Items")]
    items: Vec<UniqueItem>,
}

#[derive(Debug, Deserialize)]
struct UniqueItem {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "CurrentPrice")]
    current_price: Option<f64>,
}

/// Parses one ByCategory page into (name -> exalted price) entries plus
/// the total page count. Items without a current price are skipped, not
/// zeroed: a missing price must surface as "?" downstream, never "0 ex".
pub fn parse_unique_page(body: &str) -> Result<(Vec<(String, f64)>, u32), ScoutError> {
    let page: UniquePage = serde_json::from_str(body)?;
    let entries = page
        .items
        .into_iter()
        .filter_map(|i| match i.current_price {
            Some(p) if p > 0.0 => Some((i.name, p)),
            _ => None,
        })
        .collect();
    Ok((entries, page.pages))
}

/// Parses the Filters response into the distinct unique-item categories.
pub fn parse_unique_categories(body: &str) -> Result<Vec<String>, ScoutError> {
    let resp: FiltersResponse = serde_json::from_str(body)?;
    let mut cats: Vec<String> = Vec::new();
    for f in resp.filters {
        if f.item_kind == "unique" && !cats.contains(&f.category) {
            cats.push(f.category);
        }
    }
    Ok(cats)
}

pub struct ScoutClient {
    http: reqwest::blocking::Client,
    base: String,
    cache_dir: PathBuf,
}

impl ScoutClient {
    pub fn new(cache_dir: PathBuf) -> ScoutClient {
        Self::with_base(DEFAULT_BASE.to_string(), cache_dir)
    }

    pub fn with_base(base: String, cache_dir: PathBuf) -> ScoutClient {
        let http = reqwest::blocking::Client::builder()
            .user_agent(crate::ninja::USER_AGENT)
            .timeout(Duration::from_secs(15))
            .build()
            .expect("client build");
        ScoutClient { http, base, cache_dir }
    }

    fn cache_path(&self, league: &str) -> PathBuf {
        let safe: String = league
            .chars()
            .map(|c| if c == '/' || c == '\\' { '_' } else { c })
            .collect();
        self.cache_dir.join(format!("scout-uniques-{safe}.json"))
    }

    fn fetch_all(&self, league: &str) -> Result<HashMap<String, f64>, ScoutError> {
        let filters_body = self
            .http
            .get(format!("{}/Realms/poe2/Filters", self.base))
            .send()?
            .error_for_status()?
            .text()?;
        let categories = parse_unique_categories(&filters_body)?;
        if categories.is_empty() {
            return Err(ScoutError::NoData("no unique categories".into()));
        }
        let mut out = HashMap::new();
        for cat in &categories {
            let mut page = 1u32;
            loop {
                let url = format!(
                    "{}/poe2/Leagues/{}/Uniques/ByCategory?category={}&page={}&perPage=250",
                    self.base,
                    league.replace(' ', "%20"),
                    cat,
                    page
                );
                let body = self.http.get(&url).send()?.error_for_status()?.text()?;
                let (entries, pages) = parse_unique_page(&body)?;
                out.extend(entries);
                if page >= pages {
                    break;
                }
                page += 1;
            }
        }
        if out.is_empty() {
            return Err(ScoutError::NoData(format!("no priced uniques for {league}")));
        }
        Ok(out)
    }

    /// All unique item prices (name -> exalted) for a league: fresh from
    /// the API when reachable (and cached to disk), the last cached map
    /// otherwise. The bool is true when the data came from stale cache.
    pub fn unique_prices(&self, league: &str) -> Result<(HashMap<String, f64>, bool), ScoutError> {
        match self.fetch_all(league) {
            Ok(map) => {
                std::fs::create_dir_all(&self.cache_dir)?;
                std::fs::write(self.cache_path(league), serde_json::to_string(&map)?)?;
                Ok((map, false))
            }
            Err(fetch_err) => match std::fs::read_to_string(self.cache_path(league)) {
                Ok(body) => Ok((serde_json::from_str(&body)?, true)),
                Err(_) => Err(ScoutError::NoData(format!("{league}: {fetch_err}"))),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_names_and_prices_and_page_count() {
        let body = r#"{"CurrentPage":1,"Pages":2,"Total":3,"Items":[
            {"Name":"The Gnashing Sash","CurrentPrice":1615873.53,"Type":"Wide Belt"},
            {"Name":"Revered Resin","CurrentPrice":12.5}
        ]}"#;
        let (entries, pages) = parse_unique_page(body).unwrap();
        assert_eq!(pages, 2);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].0, "The Gnashing Sash");
        assert!((entries[1].1 - 12.5).abs() < f64::EPSILON);
    }

    #[test]
    fn missing_or_zero_price_is_skipped_not_zeroed() {
        let body = r#"{"CurrentPage":1,"Pages":1,"Total":3,"Items":[
            {"Name":"Unpriced","CurrentPrice":null},
            {"Name":"Zeroed","CurrentPrice":0.0},
            {"Name":"Priced","CurrentPrice":5.0}
        ]}"#;
        let (entries, _) = parse_unique_page(body).unwrap();
        assert_eq!(entries, vec![("Priced".to_string(), 5.0)]);
    }

    #[test]
    fn bad_json_is_an_error_not_a_panic() {
        assert!(parse_unique_page("<!DOCTYPE html>").is_err());
    }

    #[test]
    fn categories_come_from_unique_kind_rows_deduped() {
        let body = r#"{"Filters":[
            {"DisplayName":"A","Category":"accessory","Identifier":"A","ItemKind":"unique"},
            {"DisplayName":"B","Category":"accessory","Identifier":"B","ItemKind":"unique"},
            {"DisplayName":"C","Category":"weapon","Identifier":"C","ItemKind":"unique"},
            {"DisplayName":"D","Category":"currency","Identifier":"D","ItemKind":"currency"}
        ]}"#;
        assert_eq!(parse_unique_categories(body).unwrap(), vec!["accessory", "weapon"]);
    }

    #[test]
    fn stale_cache_serves_when_fetch_fails() {
        let dir = std::env::temp_dir().join(format!("scout-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Unroutable base: fetch fails fast.
        let client = ScoutClient::with_base("http://127.0.0.1:1".into(), dir.clone());
        std::fs::write(
            client.cache_path("TestLeague"),
            r#"{"The Gnashing Sash": 100.0}"#,
        )
        .unwrap();
        let (map, stale) = client.unique_prices("TestLeague").unwrap();
        assert!(stale);
        assert_eq!(map.get("The Gnashing Sash"), Some(&100.0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_cache_and_no_network_is_an_error() {
        let dir = std::env::temp_dir().join(format!("scout-test-none-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let client = ScoutClient::with_base("http://127.0.0.1:1".into(), dir);
        assert!(client.unique_prices("TestLeague").is_err());
    }
}
