//! Legacy stash-tab endpoint client for the wealth tracker:
//! `GET /character-window/get-stash-items` on www.pathofexile.com, which
//! still serves PoE2 stashes when given the league name. Needs the
//! POESESSID cookie (it is an account endpoint) and answers with the same
//! `x-rate-limit-*` header family as the trade API, which drives the shared
//! limiters here exactly like `TradeClient` does - no pathofexile.com call
//! leaves without claiming a slot first.

use serde::Deserialize;

use crate::trade::{Endpoint, Limiters, TRADE_BASE};

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

/// The longest a tab request waits for its turn. This only ever runs on the
/// wealth worker thread, 30 minutes apart, so a pause is the polite choice;
/// a wait past this means an active ban and becomes an error, not a hang.
const MAX_TAB_WAIT: std::time::Duration = std::time::Duration::from_secs(120);

/// Blocking client for the legacy stash endpoint. Owns its reqwest client
/// (the endpoint lives outside the trade API paths) and draws on the
/// process-wide limiters, so its request history outlives any one instance
/// and its account and ip rule families are each tracked in their own slot.
pub struct StashClient {
    http: reqwest::blocking::Client,
    base: String,
    limiters: Limiters,
}

impl Default for StashClient {
    fn default() -> Self {
        StashClient::new()
    }
}

impl StashClient {
    pub fn new() -> StashClient {
        StashClient::with_base(TRADE_BASE, Limiters::global())
    }

    /// A client for another host with its own limiters: a stub server.
    pub fn with_base(base: &str, limiters: Limiters) -> StashClient {
        let http = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .user_agent(
                "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) \
                 Chrome/126.0 Safari/537.36 khaloni-poe2/0.1",
            )
            .build()
            .expect("reqwest client with static config builds");
        StashClient { http, base: base.trim_end_matches('/').to_string(), limiters }
    }

    /// Fetches one tab. The slot is claimed under the limiter's lock after
    /// any wait, so a sleep is always followed by a fresh check, and a 429
    /// locks the limiter for as long as the server asked.
    pub fn get_tab(
        &mut self,
        account: &str,
        league: &str,
        poesessid: &str,
        tab_index: u32,
    ) -> Result<StashTab, String> {
        self.limiters
            .take_turn(Endpoint::Stash, true, MAX_TAB_WAIT)
            .map_err(|d| format!("stash rate limited; retry in {d:?}"))?;
        let resp = self
            .http
            .get(format!("{}/character-window/get-stash-items", self.base))
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
        // where trade uses -ip, and may carry both: each family the
        // response names keeps its own rules and history.
        self.limiters.absorb(Endpoint::Stash, true, resp.headers());
        match resp.status().as_u16() {
            429 => {
                let retry_after =
                    resp.headers().get(reqwest::header::RETRY_AFTER).and_then(|v| v.to_str().ok());
                let ban = self.limiters.on_429(Endpoint::Stash, true, retry_after);
                return Err(format!("stash rate limited (429); retry in {}s", ban.as_secs().max(1)));
            }
            401 | 403 => return Err("stash auth failed: POESESSID invalid or expired".into()),
            s if !(200..300).contains(&s) => return Err(format!("stash status {s}")),
            _ => {}
        }
        parse_stash_tab(&resp.text().map_err(|e| format!("stash body: {e}"))?)
    }

    pub fn limiters(&self) -> &Limiters {
        &self.limiters
    }
}

/// Every item in the account's stash: iterates tabs 0..numTabs capped at
/// [`TAB_CAP`]. Downloading is kept apart from pricing so the caller can
/// price against the table as it stands once the (slow, rate-limited) walk
/// is over, not as it stood before it began.
pub fn fetch_stash_items(
    client: &mut StashClient,
    account: &str,
    league: &str,
    poesessid: &str,
) -> Result<Vec<StashItem>, String> {
    if account.is_empty() || poesessid.is_empty() {
        return Err("stash fetch needs an account name and POESESSID".into());
    }
    let mut items = Vec::new();
    let mut num_tabs = 1u32; // corrected by the first response
    let mut tab = 0u32;
    while tab < num_tabs.min(TAB_CAP) {
        let t = client.get_tab(account, league, poesessid, tab)?;
        if tab == 0 {
            num_tabs = t.num_tabs.max(1);
        }
        items.extend(t.items);
        tab += 1;
    }
    Ok(items)
}

/// Total value of `items` in whatever unit `price` returns (the app passes
/// an exalted-valued table lookup). Unknown items are the callback's
/// problem by design - the app prices them at 0 so a snapshot is always a
/// lower bound, never an error.
pub fn stash_value(items: &[StashItem], price: &dyn Fn(&str, u32) -> f64) -> f64 {
    items.iter().map(|i| price(&i.type_line, i.stack_size)).sum()
}

/// [`fetch_stash_items`] then [`stash_value`].
pub fn fetch_stash_value(
    client: &mut StashClient,
    account: &str,
    league: &str,
    poesessid: &str,
    price: &dyn Fn(&str, u32) -> f64,
) -> Result<f64, String> {
    Ok(stash_value(&fetch_stash_items(client, account, league, poesessid)?, price))
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
