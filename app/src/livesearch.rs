//! Live-search alert worker: polls the user's saved trade searches in the
//! background and raises an [`Alert`] whenever a search gains listings it
//! has not seen before. Every request funnels through one shared
//! `TradeClient` so a config full of searches still respects the site's
//! search rate limits as a single well-behaved client.

use std::collections::HashSet;
use std::sync::mpsc::Sender;
use std::time::Duration;

use khaloni_poe2_core::trade::{parse_search_url, TradeClient, TradeError};

use crate::config::LiveSearch;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Alert {
    NewListings { search: String, count: usize },
}

/// Poll cadence per full pass over the saved searches. 120s keeps a
/// handful of searches to a few requests per minute - well under the
/// 15-per-60s search rule even before the limiter gets involved.
const POLL_INTERVAL: Duration = Duration::from_secs(120);

/// Gap between consecutive searches inside one pass: each search costs two
/// requests (saved-query GET + re-POST) against the burst rule (5 per 10s),
/// so spacing passes them through without ever tripping a cooldown.
const BETWEEN_SEARCHES: Duration = Duration::from_secs(5);

/// Ceiling on remembered result ids per search. The union grows as
/// listings churn; past this it resets to the current page, trading a
/// (rare) repeat alert for bounded memory over a long session.
const SEEN_CAP: usize = 2000;

struct Watched {
    name: String,
    league: String,
    id: String,
    /// `None` until the first successful poll: the priming poll only
    /// records what exists, silently - alerting on startup would flag every
    /// pre-existing listing as "new".
    seen: Option<HashSet<String>>,
}

/// Spawns the poll worker. No session cookie or no searches means the
/// feature is off: nothing is spawned and no request is ever made. The
/// thread exits when the alert receiver is dropped.
pub fn spawn(cfg_searches: Vec<LiveSearch>, poesessid: String, tx: Sender<Alert>) {
    if poesessid.trim().is_empty() || cfg_searches.is_empty() {
        return;
    }
    std::thread::spawn(move || run(cfg_searches, poesessid, tx));
}

fn run(cfg_searches: Vec<LiveSearch>, poesessid: String, tx: Sender<Alert>) {
    let mut watched: Vec<Watched> = cfg_searches
        .iter()
        .filter_map(|s| match parse_search_url(&s.url) {
            Some((league, id)) => Some(Watched {
                name: if s.name.is_empty() { id.clone() } else { s.name.clone() },
                league,
                id,
                seen: None,
            }),
            None => {
                // The settings UI flags these red; a stale config entry
                // just drops out of the poll set instead of erroring forever.
                eprintln!("live-search {:?}: unparseable trade URL {:?}", s.name, s.url);
                None
            }
        })
        .collect();
    if watched.is_empty() {
        return;
    }
    // One client for every search: the league is passed per call, so all
    // polls share this client's rate limiter - the non-negotiable part.
    let mut client = match TradeClient::new("https://www.pathofexile.com", "") {
        Ok(c) => c,
        Err(e) => {
            eprintln!("live-search: trade client failed to build: {e}");
            return;
        }
    };
    client.set_session(&poesessid);

    loop {
        for w in &mut watched {
            match client.saved_search_ids(&w.league, &w.id) {
                Ok(ids) => {
                    let now: HashSet<String> = ids.into_iter().collect();
                    match w.seen.take() {
                        None => w.seen = Some(now), // priming poll: record silently
                        Some(prev) => {
                            let fresh = now.difference(&prev).count();
                            if fresh > 0
                                && tx
                                    .send(Alert::NewListings {
                                        search: w.name.clone(),
                                        count: fresh,
                                    })
                                    .is_err()
                            {
                                return; // receiver gone: the app is shutting down
                            }
                            // Union, not replace: a listing that rotates off
                            // page one and back must not re-alert.
                            let mut merged: HashSet<String> = prev;
                            merged.extend(now.iter().cloned());
                            w.seen = Some(if merged.len() > SEEN_CAP { now } else { merged });
                        }
                    }
                }
                Err(TradeError::Cooldown(d)) => {
                    // The limiter (or the server) said stop; honor it and
                    // let the next pass retry this search.
                    eprintln!("live-search {}: rate limited, backing off {d:?}", w.name);
                    std::thread::sleep(d.min(Duration::from_secs(60)));
                }
                Err(e) => eprintln!("live-search {}: {e}", w.name),
            }
            std::thread::sleep(BETWEEN_SEARCHES);
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}
