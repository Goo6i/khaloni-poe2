//! The shared rate limiters, offline: header handling against the pure
//! API, and the request path against a local stub that answers like the
//! trade site (rate headers, 429 with Retry-After).

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use khaloni_poe2_core::stash::StashClient;
use khaloni_poe2_core::trade::{
    set_request_logger, Endpoint, Limiters, Query, RateDecision, TradeClient, TradeData, TradeError,
};

/// Rate headers the live search answers with (verified 2026-09-18/19).
const SEARCH_RATE_HEADERS: &str = "X-Rate-Limit-Policy: trade-search-request-limit\r\nX-Rate-Limit-Rules: Ip,Account\r\n\
     X-Rate-Limit-Ip: 5:10:60,15:60:300,30:300:1800\r\nX-Rate-Limit-Ip-State: 1:10:0,3:60:0,12:300:0\r\n\
     X-Rate-Limit-Account: 45:60:120\r\nX-Rate-Limit-Account-State: 2:60:0\r\n";

/// The request log of the calling thread. The logger is process-wide and
/// the tests here run in parallel, so it is installed once and routes each
/// line to the sink of the thread that sent the request; a test reads only
/// its own traffic.
fn request_log() -> Arc<Mutex<Vec<String>>> {
    thread_local! {
        static SINK: std::cell::RefCell<Option<Arc<Mutex<Vec<String>>>>> = const { std::cell::RefCell::new(None) };
    }
    static INSTALL: std::sync::Once = std::sync::Once::new();
    INSTALL.call_once(|| {
        set_request_logger(Box::new(|line| {
            SINK.with(|s| {
                if let Some(sink) = s.borrow().as_ref() {
                    sink.lock().unwrap().push(line.to_string());
                }
            });
        }));
    });
    let sink = Arc::new(Mutex::new(Vec::new()));
    SINK.with(|s| *s.borrow_mut() = Some(sink.clone()));
    sink
}

fn headers(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let owned: Vec<(String, String)> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    move |name: &str| owned.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone())
}

/// A stub server: `respond` gets the request head and returns the full raw
/// response. Every request's first line is recorded.
fn serve(respond: impl Fn(&str) -> String + Send + 'static) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { break };
            let mut buf = [0u8; 8192];
            let n = s.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            log.lock().unwrap().push(req.lines().next().unwrap_or("").to_string());
            let _ = s.write_all(respond(&req).as_bytes());
        }
    });
    (format!("http://{addr}"), seen)
}

fn response(status: &str, extra_headers: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n{extra_headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

#[test]
fn every_named_family_must_have_room() {
    let l = Limiters::new();
    // Ip has plenty of room; the account family is full. Reading only the
    // ip headers, or letting the ip state overwrite the account state in
    // one slot, called this Ready.
    l.absorb_with(
        Endpoint::Stash,
        true,
        &headers(&[
            ("x-rate-limit-policy", "backend-stash-request-limit"),
            ("x-rate-limit-rules", "Account,Ip"),
            ("x-rate-limit-account", "3:60:120"),
            ("x-rate-limit-account-state", "3:60:0"),
            ("x-rate-limit-ip", "50:60:120"),
            ("x-rate-limit-ip-state", "1:60:0"),
        ]),
    );
    assert!(matches!(l.check(Endpoint::Stash, true), RateDecision::Wait(_)));
    // The account family counts signed-in requests only.
    assert_eq!(l.check(Endpoint::Stash, false), RateDecision::Ready);
}

#[test]
fn a_client_family_is_absorbed_like_the_others() {
    let l = Limiters::new();
    l.absorb_with(
        Endpoint::Search,
        false,
        &headers(&[
            ("x-rate-limit-policy", "trade-search-request-limit"),
            ("x-rate-limit-rules", "Ip,Client"),
            ("x-rate-limit-ip", "5:10:60"),
            ("x-rate-limit-ip-state", "1:10:0"),
            ("x-rate-limit-client", "2:10:60"),
            ("x-rate-limit-client-state", "2:10:0"),
        ]),
    );
    assert!(matches!(l.check(Endpoint::Search, false), RateDecision::Wait(_)));
}

#[test]
fn endpoints_under_one_server_policy_share_one_budget() {
    let l = Limiters::new();
    let policy = |state: &'static str| {
        headers(&[
            ("x-rate-limit-policy", "trade-search-request-limit"),
            ("x-rate-limit-rules", "Ip"),
            ("x-rate-limit-ip", "5:10:60"),
            ("x-rate-limit-ip-state", state),
        ])
    };
    l.absorb_with(Endpoint::Search, false, &policy("1:10:0"));
    l.absorb_with(Endpoint::Exchange, false, &policy("2:10:0"));
    // Three more exchanges fill the window the searches draw on too.
    for _ in 0..3 {
        l.take_turn(Endpoint::Exchange, false, Duration::ZERO).expect("room left");
    }
    assert!(matches!(l.check(Endpoint::Search, false), RateDecision::Wait(_)));
    // A different policy is a different budget, with its own rules: the
    // fetch response cannot redefine what the search limiter enforces.
    l.absorb_with(
        Endpoint::Fetch,
        false,
        &headers(&[
            ("x-rate-limit-policy", "trade-fetch-request-limit"),
            ("x-rate-limit-rules", "Ip"),
            ("x-rate-limit-ip", "12:4:10"),
            ("x-rate-limit-ip-state", "1:4:0"),
        ]),
    );
    assert_eq!(l.check(Endpoint::Fetch, false), RateDecision::Ready);
    assert!(matches!(l.check(Endpoint::Search, false), RateDecision::Wait(_)));
}

#[test]
fn two_clients_on_the_same_limiters_cannot_both_take_the_last_slot() {
    let l = Limiters::new();
    l.absorb_with(
        Endpoint::Search,
        false,
        &headers(&[("x-rate-limit-rules", "Ip"), ("x-rate-limit-ip", "8:60:60"), ("x-rate-limit-ip-state", "0:60:0")]),
    );
    let taken: usize = (0..4)
        .map(|_| {
            let l = l.clone();
            std::thread::spawn(move || (0..8).filter(|_| l.take_turn(Endpoint::Search, false, Duration::ZERO).is_ok()).count())
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|t| t.join().unwrap())
        .sum();
    assert_eq!(taken, 8, "32 attempts from 4 threads, 8 slots");
}

#[test]
fn a_429_locks_the_limiter_for_the_servers_retry_after() {
    let (base, seen) = serve(|_| {
        response(
            "429 Too Many Requests",
            "Retry-After: 1800\r\nX-Rate-Limit-Policy: trade-search-request-limit\r\nX-Rate-Limit-Rules: Ip\r\n\
             X-Rate-Limit-Ip: 5:10:60,15:60:300,30:300:1800\r\nX-Rate-Limit-Ip-State: 6:10:60,16:60:300,31:300:1800\r\n",
            r#"{"error":{"code":3,"message":"Rate limit exceeded"}}"#,
        )
    });
    let limiters = Limiters::new();
    let mut c = TradeClient::with_limiters(&base, "Standard", limiters.clone()).unwrap();
    match c.search(&Query::default()) {
        Err(TradeError::Cooldown(d)) => assert!(d.as_secs() >= 1790, "the half-hour ban, not a fixed minute: {d:?}"),
        other => panic!("expected Cooldown, got {other:?}"),
    }
    // The ban lives in the limiter: the next search, from any client on
    // these limiters, is refused before a request leaves.
    let mut other = TradeClient::with_limiters(&base, "Standard", limiters).unwrap();
    assert!(matches!(other.search(&Query::default()), Err(TradeError::Cooldown(d)) if d.as_secs() >= 1780));
    assert_eq!(seen.lock().unwrap().len(), 1);
}

#[test]
fn a_429_without_a_retry_after_still_locks_the_limiter() {
    let (base, seen) = serve(|_| response("429 Too Many Requests", "", "{}"));
    let mut c = TradeClient::with_limiters(&base, "Standard", Limiters::new()).unwrap();
    assert!(matches!(c.search(&Query::default()), Err(TradeError::Cooldown(d)) if d.as_secs() >= 59));
    assert!(matches!(c.search(&Query::default()), Err(TradeError::Cooldown(_))));
    assert_eq!(seen.lock().unwrap().len(), 1);
}

const STATIC_BODY: &str =
    r#"{"result":[{"id":"Currency","entries":[{"id":"exalted","text":"Exalted Orb"},{"id":"divine","text":"Divine Orb"}]}]}"#;

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("khalonipoe2-tradedata-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_data_catalog_is_downloaded_once_and_then_read_from_disk() {
    let (base, seen) = serve(|_| response("200 OK", "", STATIC_BODY));
    let dir = temp_dir("once");
    let path = dir.join("trade_static.json");
    let c = TradeClient::with_limiters(&base, "Standard", Limiters::new()).unwrap();
    let day = Duration::from_secs(24 * 3600);
    assert_eq!(c.cached_data(TradeData::Static, &path, day).unwrap(), STATIC_BODY);
    assert_eq!(c.cached_data(TradeData::Static, &path, day).unwrap(), STATIC_BODY);
    assert_eq!(seen.lock().unwrap().len(), 1, "the second launch costs no request");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "no temp litter");
    // Past its age the copy is refreshed.
    assert!(c.cached_data(TradeData::Static, &path, Duration::ZERO).is_ok());
    assert_eq!(seen.lock().unwrap().len(), 2);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_unusable_catalog_is_an_error_and_never_reaches_the_cache() {
    let (base, _) = serve(|_| response("200 OK", "", "<html>Just a moment...</html>"));
    let dir = temp_dir("bad");
    let path = dir.join("trade_static.json");
    let c = TradeClient::with_limiters(&base, "Standard", Limiters::new()).unwrap();
    assert!(matches!(c.cached_data(TradeData::Static, &path, Duration::ZERO), Err(TradeError::BadData(..))));
    assert!(!path.exists());
    assert!(c.static_currency_ids().is_err(), "a failure is reported, not defaulted to an empty map");
    // An empty catalog is no better than a challenge page.
    assert!(TradeData::Items.validate(r#"{"result":[]}"#).is_err());

    // With a good copy on disk, the bad answer leaves it alone and it is
    // what the caller gets.
    std::fs::write(&path, STATIC_BODY).unwrap();
    assert_eq!(c.cached_data(TradeData::Static, &path, Duration::ZERO).unwrap(), STATIC_BODY);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), STATIC_BODY);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn data_requests_go_through_the_limiter_and_honor_a_429() {
    let (base, seen) = serve(|_| response("429 Too Many Requests", "Retry-After: 120\r\n", "{}"));
    let c = TradeClient::with_limiters(&base, "Standard", Limiters::new()).unwrap();
    assert!(matches!(c.items_json(), Err(TradeError::Cooldown(d)) if d.as_secs() >= 110));
    // Stats and static draw on the same data policy: both are refused
    // without a request.
    assert!(matches!(c.data_json(TradeData::Stats), Err(TradeError::Cooldown(_))));
    assert!(matches!(c.static_currency_ids(), Err(TradeError::Cooldown(_))));
    assert_eq!(seen.lock().unwrap().len(), 1);
}

#[test]
fn the_stash_client_tracks_both_families_and_records_a_429_ban() {
    let hits = Arc::new(Mutex::new(0u32));
    let count = hits.clone();
    let (base, seen) = serve(move |_| {
        let mut n = count.lock().unwrap();
        *n += 1;
        if *n == 1 {
            response(
                "200 OK",
                "X-Rate-Limit-Policy: backend-stash-request-limit\r\nX-Rate-Limit-Rules: Account,Ip\r\n\
                 X-Rate-Limit-Account: 30:60:60\r\nX-Rate-Limit-Account-State: 1:60:0\r\n\
                 X-Rate-Limit-Ip: 45:60:120\r\nX-Rate-Limit-Ip-State: 1:60:0\r\n",
                r#"{"numTabs":2,"items":[{"typeLine":"Exalted Orb","stackSize":3}]}"#,
            )
        } else {
            response("429 Too Many Requests", "Retry-After: 600\r\n", "{}")
        }
    });
    let limiters = Limiters::new();
    let mut c = StashClient::with_base(&base, limiters.clone());
    let tab = c.get_tab("acct#1", "Standard", "sess", 0).expect("first tab");
    assert_eq!(tab.items[0].stack_size, 3);
    let err = c.get_tab("acct#1", "Standard", "sess", 1).expect_err("429");
    assert!(err.contains("429"), "{err}");
    // The ban is in the limiter now, for as long as the server asked.
    match limiters.check(Endpoint::Stash, true) {
        RateDecision::Wait(d) => assert!(d.as_secs() >= 590, "{d:?}"),
        RateDecision::Ready => panic!("a 429 must lock the stash limiter"),
    }
    assert!(c.get_tab("acct#1", "Standard", "sess", 1).is_err());
    assert_eq!(seen.lock().unwrap().len(), 2, "nothing leaves during the ban");
    let _ = hits;
}

// --- request accounting and the shared exchange budget (2026-09-26) ---

#[test]
fn every_request_logs_the_servers_counters() {
    let log = request_log();
    let (base, _) = serve(|_| response("200 OK", SEARCH_RATE_HEADERS, r#"{"id":"abc","result":[],"total":0}"#));
    let mut c = TradeClient::with_limiters(&base, "Standard", Limiters::new()).unwrap();
    c.search(&Query::default()).expect("first search");
    c.search(&Query::default()).expect("second search");
    let lines = log.lock().unwrap().clone();
    assert_eq!(lines.len(), 4, "one line before and one after each request: {lines:#?}");
    // Before the first response nothing has named the policy, so the seed
    // is what the request is accounted under.
    assert_eq!(lines[0], "trade request: search Standard policy=seed:search");
    // After it, every family the server counts is read back as the server
    // reported it: used/max per rule, with the rule's window.
    assert_eq!(
        lines[1],
        "trade response: search 200 policy=trade-search-request-limit \
         ip 1/5 (10s), 3/15 (60s), 12/30 (300s); account 2/45 (60s)"
    );
    assert_eq!(lines[2], "trade request: search Standard policy=trade-search-request-limit");
    assert!(lines[3].starts_with("trade response: search 200 "), "{}", lines[3]);
}

#[test]
fn a_429_logs_the_ban_and_the_request_that_hit_it() {
    let log = request_log();
    let (base, _) = serve(|_| {
        response(
            "429 Too Many Requests",
            "Retry-After: 1800\r\nX-Rate-Limit-Policy: trade-search-request-limit\r\nX-Rate-Limit-Rules: Ip\r\n\
             X-Rate-Limit-Ip: 5:10:60,15:60:300,30:300:1800\r\nX-Rate-Limit-Ip-State: 6:10:60,16:60:300,31:300:1800\r\n",
            r#"{"error":{"code":3,"message":"Rate limit exceeded"}}"#,
        )
    });
    let mut c = TradeClient::with_limiters(&base, "Runes of Aldur", Limiters::new()).unwrap();
    assert!(matches!(c.search(&Query::default()), Err(TradeError::Cooldown(_))));
    let lines = log.lock().unwrap().clone();
    assert_eq!(lines.len(), 3, "{lines:#?}");
    assert_eq!(lines[0], "trade request: search Runes of Aldur policy=seed:search");
    assert_eq!(
        lines[1],
        "trade response: search 429 policy=trade-search-request-limit ip 6/5 (10s), 16/15 (60s), 31/30 (300s)"
    );
    // The ban line is what the owner reads after the fact: how long, what
    // the server said, and which request it was.
    assert_eq!(
        lines[2],
        "trade 429: search banned 1800s (retry-after 1800, state ip 6:10:60,16:60:300,31:300:1800) \
         after request search Runes of Aldur"
    );
}

#[test]
fn the_exchange_body_is_returned_raw() {
    const EXCHANGE: &str = include_str!("fixtures/trade_exchange.json");
    let (base, seen) = serve(|_| response("200 OK", SEARCH_RATE_HEADERS, EXCHANGE));
    let mut c = TradeClient::with_limiters(&base, "Standard", Limiters::new()).unwrap();
    let body = c.exchange_raw("divine", "exalted").expect("exchange body");
    assert_eq!(body["id"].as_str(), Some("QLZEYpzmuw"));
    let offers = body["result"].as_object().expect("the offers keyed by listing id");
    assert!(!offers.is_empty());
    // The bulk view reads stock and seller from the same body the rate
    // came from; the rate is still the summary of it.
    let listing = &offers["80423ee2677a86a6f6eff4f06213da49c75157241cbf3ffdc4e453"];
    assert_eq!(listing["listing"]["offers"][0]["item"]["stock"].as_u64(), Some(3));
    assert_eq!(listing["listing"]["account"]["name"].as_str(), Some("unclefanch#5816"));
    // Median of the five cheapest offers: 10, 460, 499, 500, 560.
    assert_eq!(c.exchange("divine", "exalted").expect("rate"), Some(499.0));
    assert_eq!(seen.lock().unwrap().len(), 2);
    assert!(seen.lock().unwrap()[0].starts_with("POST /api/trade2/exchange/Standard "));
}

#[test]
fn the_exchange_spends_the_search_budget_until_a_policy_says_otherwise() {
    let l = Limiters::new();
    // The search's own policy is named; the exchange has not answered yet.
    l.absorb_with(
        Endpoint::Search,
        false,
        &headers(&[
            ("x-rate-limit-policy", "trade-search-request-limit"),
            ("x-rate-limit-rules", "Ip"),
            ("x-rate-limit-ip", "5:10:60"),
            ("x-rate-limit-ip-state", "1:10:0"),
        ]),
    );
    assert_eq!(l.free_slots(Endpoint::Exchange), 4, "the exchange reads the search's counters");
    // Exchanges spend search slots until the window is full.
    for _ in 0..4 {
        l.take_turn(Endpoint::Exchange, false, Duration::ZERO).expect("room left");
    }
    assert!(matches!(l.check(Endpoint::Search, false), RateDecision::Wait(_)));
    assert!(matches!(l.check(Endpoint::Exchange, false), RateDecision::Wait(_)));
    // An exchange response under the search's own policy name changes
    // nothing: the two keep one budget.
    l.absorb_with(
        Endpoint::Exchange,
        false,
        &headers(&[
            ("x-rate-limit-policy", "trade-search-request-limit"),
            ("x-rate-limit-rules", "Ip"),
            ("x-rate-limit-ip", "5:10:60"),
            ("x-rate-limit-ip-state", "5:10:0"),
        ]),
    );
    assert!(matches!(l.check(Endpoint::Search, false), RateDecision::Wait(_)));
    // Once a response names a policy of its own, the exchange moves to it
    // and the search budget stops paying for exchanges.
    l.absorb_with(
        Endpoint::Exchange,
        false,
        &headers(&[
            ("x-rate-limit-policy", "trade-exchange-request-limit"),
            ("x-rate-limit-rules", "Ip"),
            ("x-rate-limit-ip", "20:10:60"),
            ("x-rate-limit-ip-state", "6:10:0"),
        ]),
    );
    assert_eq!(l.check(Endpoint::Exchange, false), RateDecision::Ready);
    for _ in 0..3 {
        l.take_turn(Endpoint::Exchange, false, Duration::ZERO).expect("the exchange's own room");
    }
    assert_eq!(l.free_slots(Endpoint::Exchange), 11);
    assert_eq!(l.free_slots(Endpoint::Search), 0, "the search window is still the one the exchanges filled");
    assert_eq!(l.budget_text(Endpoint::Exchange), "exchange 9/20 (10s)");
}

#[test]
fn free_slots_follow_the_servers_last_state() {
    let l = Limiters::new();
    // Nothing seen yet: the seed's tightest rule is the whole budget.
    assert_eq!(l.free_slots(Endpoint::Search), 5);
    assert_eq!(l.free_slots(Endpoint::Fetch), 12);
    let state = |ip: &'static str, account: &'static str| {
        headers(&[
            ("x-rate-limit-policy", "trade-search-request-limit"),
            ("x-rate-limit-rules", "Ip,Account"),
            ("x-rate-limit-ip", "5:10:60,15:60:300,30:300:1800,600:21600:3600"),
            ("x-rate-limit-ip-state", ip),
            ("x-rate-limit-account", "45:60:120"),
            ("x-rate-limit-account-state", account),
        ])
    };
    // The five-minute rule is the tightest: 30 - 26.
    l.absorb_with(Endpoint::Search, true, &state("0:10:0,0:60:0,26:300:0,47:21600:0", "2:60:0"));
    assert_eq!(l.free_slots(Endpoint::Search), 4);
    // A request sent since the last response is counted, so the number
    // never reads higher than the server will.
    l.take_turn(Endpoint::Search, true, Duration::ZERO).expect("room");
    assert_eq!(l.free_slots(Endpoint::Search), 3);
    // The next response is the truth again, whichever family is fullest.
    l.absorb_with(Endpoint::Search, true, &state("2:10:0,4:60:0,20:300:0,48:21600:0", "44:60:0"));
    assert_eq!(l.free_slots(Endpoint::Search), 1, "the account family has one slot left");
    // Past the cap is zero, never negative.
    l.absorb_with(Endpoint::Search, true, &state("6:10:60,16:60:300,31:300:1800,49:21600:0", "44:60:0"));
    assert_eq!(l.free_slots(Endpoint::Search), 0);
    // Another policy's counters are its own.
    assert_eq!(l.free_slots(Endpoint::Fetch), 12);
}

#[test]
fn the_budget_text_reads_from_the_server_counters() {
    let l = Limiters::new();
    assert_eq!(l.budget_text(Endpoint::Search), "search 0/5 (10s)");
    l.absorb_with(
        Endpoint::Search,
        false,
        &headers(&[
            ("x-rate-limit-policy", "trade-search-request-limit"),
            ("x-rate-limit-rules", "Ip"),
            ("x-rate-limit-ip", "5:10:60,15:60:300,30:300:1800,600:21600:3600"),
            ("x-rate-limit-ip-state", "0:10:0,4:60:0,26:300:0,47:21600:0"),
        ]),
    );
    // The rule with the fewest free slots is the one named: 4 of 30 left
    // in the five-minute window, against 5 of 5, 11 of 15 and 553 of 600.
    assert_eq!(l.budget_text(Endpoint::Search), "search 26/30 (5 min)");
    l.absorb_with(
        Endpoint::Search,
        false,
        &headers(&[
            ("x-rate-limit-policy", "trade-search-request-limit"),
            ("x-rate-limit-rules", "Ip"),
            ("x-rate-limit-ip", "5:10:60,15:60:300,30:300:1800,600:21600:3600"),
            ("x-rate-limit-ip-state", "0:10:0,4:60:0,26:300:0,598:21600:0"),
        ]),
    );
    assert_eq!(l.budget_text(Endpoint::Search), "search 598/600 (6 h)");
    assert_eq!(l.budget_text(Endpoint::Fetch), "fetch 0/12 (4s)");
}

#[test]
fn the_budget_reads_the_minute_and_longer_rules_and_the_burst_rules_apart() {
    let l = Limiters::new();
    let state = |ip: &'static str| {
        headers(&[
            ("x-rate-limit-policy", "trade-search-request-limit"),
            ("x-rate-limit-rules", "Ip"),
            ("x-rate-limit-ip", "5:10:60,15:60:300,30:300:1800,600:21600:3600"),
            ("x-rate-limit-ip-state", ip),
        ])
    };
    // Idle: the ten-second rule caps the tightest reading at five, which is
    // why the budget cannot be read from it; the minute rule leaves 15.
    l.absorb_with(Endpoint::Search, true, &state("0:10:0,0:60:0,0:300:0,0:21600:0"));
    assert_eq!(l.free_slots(Endpoint::Search), 5);
    assert_eq!(l.budget_free(Endpoint::Search), 15);
    assert_eq!(l.burst_free(Endpoint::Search), 5);
    // The five-minute rule is the fullest long rule: 30 - 22.
    l.absorb_with(Endpoint::Search, true, &state("1:10:0,3:60:0,22:300:0,40:21600:0"));
    assert_eq!(l.budget_free(Endpoint::Search), 8);
    assert_eq!(l.burst_free(Endpoint::Search), 4);
    // A full burst window: the budget is fine, but a request now would wait.
    // (A fresh limiter: one never reads fewer requests than it has seen.)
    let l = Limiters::new();
    l.absorb_with(Endpoint::Search, true, &state("5:10:0,5:60:0,10:300:0,40:21600:0"));
    assert_eq!(l.budget_free(Endpoint::Search), 10);
    assert_eq!(l.burst_free(Endpoint::Search), 0);
}
