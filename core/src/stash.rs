//! Legacy stash-tab endpoint client for the wealth tracker:
//! `GET /character-window/get-stash-items` on www.pathofexile.com, which
//! still serves PoE2 stashes when given the league name. Needs the
//! POESESSID cookie (it is an account endpoint) and answers with the same
//! `x-rate-limit-*` header family as the trade API, which drives the
//! limiter here exactly like `TradeClient` does - no pathofexile.com call
//! leaves without passing a `RateLimiter` first.

use serde::Deserialize;

use crate::trade::{RateDecision, RateLimiter};

/// Hard cap on tabs fetched per snapshot. A snapshot is a trend line, not
/// an audit: 20 tabs bounds the request burst (and the wait spent inside
/// the rate limiter) no matter how large the account's stash is.
pub const TAB_CAP: u32 = 20;

/// One stash item as the wealth tracker sees it: the display base name and
/// how many are stacked (non-stackables report 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StashItem {
    pub type_line: String,
    pub stack_size: u32,
}

/// One tab's parsed payload: the account's total tab count (the endpoint
/// repeats it on every response) and the items in the requested tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StashTab {
    pub num_tabs: u32,
    pub items: Vec<StashItem>,
}

#[derive(Debug, Deserialize)]
struct RawStash {
    #[serde(rename = "numTabs", default)]
    num_tabs: u32,
    #[serde(default)]
    items: Vec<RawItem>,
}

#[derive(Debug, Deserialize)]
struct RawItem {
    #[serde(rename = "typeLine", default)]
    type_line: String,
    /// Absent on non-stackables (gear, jewels); treated as a stack of 1.
    #[serde(rename = "stackSize")]
    stack_size: Option<u32>,
}

/// Parses one get-stash-items response body. Pure, so the shape is testable
/// without the network.
pub fn parse_stash_tab(json: &str) -> Result<StashTab, String> {
    let raw: RawStash = serde_json::from_str(json).map_err(|e| format!("stash json: {e}"))?;
    Ok(StashTab {
        num_tabs: raw.num_tabs,
        items: raw
            .items
            .into_iter()
            .filter(|i| !i.type_line.is_empty())
            .map(|i| StashItem {
                type_line: i.type_line,
                stack_size: i.stack_size.unwrap_or(1).max(1),
            })
            .collect(),
    })
}

/// Blocking client for the legacy stash endpoint. Owns its reqwest client
/// (the endpoint lives outside the trade API base) and its own limiter,
/// seeded conservatively and corrected by every response's rate headers -
/// keep one instance alive across snapshots so the request history carries
/// over.
pub struct StashClient {
    http: reqwest::blocking::Client,
    limiter: RateLimiter,
}

impl Default for StashClient {
    fn default() -> Self {
        StashClient::new()
    }
}

impl StashClient {
    pub fn new() -> StashClient {
        let http = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .user_agent(
                "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) \
                 Chrome/126.0 Safari/537.36 khaloni-poe2/0.1",
            )
            .build()
            .expect("reqwest client with static config builds");
        StashClient {
            http,
            // Conservative until the first response's x-rate-limit headers
            // replace it; the documented legacy limit is far looser.
            limiter: RateLimiter::from_header("20:60:60"),
        }
    }

    /// Fetches one tab, sleeping out short limiter waits (this only ever
    /// runs on the wealth worker thread, 30 minutes apart, so a pause is
    /// the polite choice); a wait past 2 minutes means an active ban and
    /// becomes an error instead of a hang.
    fn get_tab(
        &mut self,
        account: &str,
        league: &str,
        poesessid: &str,
        tab_index: u32,
    ) -> Result<StashTab, String> {
        if let RateDecision::Wait(d) = self.limiter.check() {
            if d > std::time::Duration::from_secs(120) {
                return Err(format!("stash rate limited; retry in {d:?}"));
            }
            std::thread::sleep(d);
        }
        self.limiter.record();
        let resp = self
            .http
            .get("https://www.pathofexile.com/character-window/get-stash-items")
            .query(&[
                ("accountName", account),
                ("league", league),
                ("tabs", "1"),
                ("tabIndex", &tab_index.to_string()),
            ])
            .header(reqwest::header::COOKIE, format!("POESESSID={poesessid}"))
            .send()
            .map_err(|e| format!("stash http: {e}"))?;
        // The legacy endpoint reports limits under -account (session-keyed)
        // where trade uses -ip; absorb whichever appears.
        for prefix in ["x-rate-limit-account", "x-rate-limit-ip"] {
            if let Some(rules) = resp.headers().get(prefix).and_then(|v| v.to_str().ok()) {
                self.limiter = RateLimiter::from_header(rules);
            }
            let state = format!("{prefix}-state");
            if let Some(state) = resp.headers().get(&state).and_then(|v| v.to_str().ok()) {
                self.limiter.apply_state(state);
            }
        }
        match resp.status().as_u16() {
            429 => return Err("stash rate limited (429)".into()),
            401 | 403 => return Err("stash auth failed: POESESSID invalid or expired".into()),
            s if !(200..300).contains(&s) => return Err(format!("stash status {s}")),
            _ => {}
        }
        parse_stash_tab(&resp.text().map_err(|e| format!("stash body: {e}"))?)
    }
}

/// Total stash value in whatever unit `price` returns (the app passes an
/// exalted-valued table lookup): iterates tabs 0..numTabs capped at
/// [`TAB_CAP`], summing `price(type_line, stack_size)` over every item.
/// Unknown items are the callback's problem by design - the app prices them
/// at 0 so a snapshot is always a lower bound, never an error.
pub fn fetch_stash_value(
    client: &mut StashClient,
    account: &str,
    league: &str,
    poesessid: &str,
    price: &dyn Fn(&str, u32) -> f64,
) -> Result<f64, String> {
    if account.is_empty() || poesessid.is_empty() {
        return Err("stash fetch needs an account name and POESESSID".into());
    }
    let mut total = 0.0;
    let mut num_tabs = 1u32; // corrected by the first response
    let mut tab = 0u32;
    while tab < num_tabs.min(TAB_CAP) {
        let t = client.get_tab(account, league, poesessid, tab)?;
        if tab == 0 {
            num_tabs = t.num_tabs.max(1);
        }
        for item in &t.items {
            total += price(&item.type_line, item.stack_size);
        }
        tab += 1;
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::parse_stash_tab;

    #[test]
    fn parses_the_top_shape() {
        let t = parse_stash_tab(
            r#"{"numTabs":4,"items":[
                {"typeLine":"Exalted Orb","stackSize":23},
                {"typeLine":"Stellar Amulet"}
            ]}"#,
        )
        .expect("parses");
        assert_eq!(t.num_tabs, 4);
        assert_eq!(t.items.len(), 2);
        assert_eq!(t.items[0].type_line, "Exalted Orb");
        assert_eq!(t.items[0].stack_size, 23);
        // Non-stackables carry no stackSize; they count once.
        assert_eq!(t.items[1].stack_size, 1);
    }

    #[test]
    fn bad_json_is_an_error_not_a_panic() {
        assert!(parse_stash_tab("not json").is_err());
    }
}
