//! A league change moves everything that is priced, or nothing: the caches
//! on the main loop's side, the trade worker's client, and what a card and
//! a notice say while the new league's table is on its way. The price
//! service's own half is tested beside it in `prices.rs`.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use khaloni_poe2::appraise::{AsyncCache, Retrying, ReuseSlot, REUSE_TTL};
use khaloni_poe2::hover::HoverState;
use khaloni_poe2::league::{self, Announcer, Current, Priced, Switch};
use khaloni_poe2::prices::{PriceService, Snapshot};
use khaloni_poe2::pricing::{Denom, Priced as PricedRow, Tier};
use khaloni_poe2::stabilize::{ScanResult, Stabilizer};
use khaloni_poe2_core::ninja::{NinjaClient, PriceTable, UniquePrices};
use khaloni_poe2_core::scout::ScoutClient;
use khaloni_poe2_core::trade::{Limiters, Query, TradeClient};

fn row(item_key: &str) -> PricedRow {
    PricedRow {
        y_top: 40,
        height: 30,
        label: "12 ex".into(),
        amount: "12".into(),
        denom: Denom::Exalted,
        tier: Tier::Decent,
        item_key: item_key.into(),
        count: 1,
        value_ex: 12.0,
        value_chaos: 12.0,
        count_explicit: false,
        locks_in_one: true,
    }
}

/// A price service that reaches nothing: both bases refuse the connection.
fn offline_prices(league: &str) -> PriceService {
    let dir = std::env::temp_dir().join(format!("khalonipoe2-league-test-{}", std::process::id()));
    PriceService::start_with_interval(
        NinjaClient::with_base("http://127.0.0.1:1".into(), dir.clone()),
        ScoutClient::with_base("http://127.0.0.1:1".into(), dir),
        league.to_string(),
        Duration::from_secs(3600),
    )
    .unwrap()
}

fn priced<'a>(
    currency: &'a Mutex<AsyncCache<String, Option<f64>>>,
    gems: &'a Mutex<AsyncCache<(String, u32), Option<f64>>>,
    stabilizer: &'a mut Stabilizer,
    hover: &'a mut HoverState,
    awaiting_exchange: &'a mut Option<(String, u32)>,
) -> Priced<'a> {
    Priced { currency, gems, stabilizer, hover, awaiting_exchange }
}

#[test]
fn a_league_change_drops_everything_priced_in_the_old_league() {
    let now = Instant::now();
    let currency: Mutex<AsyncCache<String, Option<f64>>> = Mutex::default();
    let gems: Mutex<AsyncCache<(String, u32), Option<f64>>> = Mutex::default();
    currency.lock().unwrap().store("omen of whittling".into(), Ok(Some(310.0)), now);
    gems.lock().unwrap().store(("spark".into(), 20), Ok(Some(45.0)), now);
    // A request that is out when the league changes.
    assert!(currency.lock().unwrap().lookup(&"omen of amelioration".to_string(), now).request);

    let mut stabilizer = Stabilizer::new();
    stabilizer.apply(ScanResult::rows(vec![row("exalted orb")], false));
    assert_eq!(stabilizer.rows().len(), 1);

    let mut hover = HoverState::default();
    hover.show_notice("Omen of Whittling: 310 ex");
    hover.pending_currency = Some(("Omen of Sinistral Erasure".into(), 3));
    let mut awaiting = Some(("Omen of Amelioration".to_string(), 2));

    let current = Current::new("Standard");
    let prices = offline_prices("Standard");
    let mut announcer = Announcer::default();
    // Saving Settings with the league untouched throws nothing away.
    let same = league::switch(
        &current,
        priced(&currency, &gems, &mut stabilizer, &mut hover, &mut awaiting),
        &prices,
        &mut announcer,
        "Standard",
    );
    assert_eq!(same, None);
    assert_eq!(stabilizer.rows().len(), 1);
    assert!(currency.lock().unwrap().lookup(&"omen of whittling".to_string(), now).value.is_some());

    let moved = league::switch(
        &current,
        priced(&currency, &gems, &mut stabilizer, &mut hover, &mut awaiting),
        &prices,
        &mut announcer,
        "Forbidden Rites",
    );
    assert_eq!(moved, Some(Switch { from: "Standard".into(), to: "Forbidden Rites".into() }));
    assert!(current.is("Forbidden Rites"));

    // Each of these held a Standard price a moment ago.
    let asked = currency.lock().unwrap().lookup(&"omen of whittling".to_string(), now);
    assert!(asked.value.is_none() && asked.request, "the exchange price is asked again, in the new league");
    assert!(gems.lock().unwrap().lookup(&("spark".to_string(), 20), now).value.is_none());
    assert!(stabilizer.rows().is_empty(), "reward rows keep their old amounts");
    assert!(hover.current.is_none() && hover.pending_currency.is_none() && awaiting.is_none());
    // The price service moved with them, and serves nothing meanwhile.
    let snap = prices.snapshot();
    assert_eq!((snap.league.as_str(), snap.table.is_empty()), ("Forbidden Rites", true));

    // The request that was out comes back with a Standard price: refused,
    // and the name is free to be asked again.
    let stored = league::store_if_still(
        &current,
        "Standard",
        &currency,
        "omen of amelioration".to_string(),
        Ok(Some(9.0)),
        now,
    );
    assert!(!stored);
    let again = currency.lock().unwrap().lookup(&"omen of amelioration".to_string(), now);
    assert!(again.value.is_none() && again.request);
    // An answer fetched in the league that is current is kept.
    assert!(league::store_if_still(&current, "Forbidden Rites", &gems, ("spark".into(), 20), Ok(Some(50.0)), now));
    assert_eq!(gems.lock().unwrap().lookup(&("spark".to_string(), 20), now).value, Some(Some(50.0)));
}

fn snapshot(league: &str, loading: bool, error: Option<&str>) -> Snapshot {
    let table = PriceTable::default();
    Snapshot {
        league: league.into(),
        loading,
        error: error.map(str::to_string),
        vocab: khaloni_poe2::pricing::build_vocab(&table),
        table,
        uniques: UniquePrices::default(),
        stale: loading,
        uniques_stale: loading,
    }
}

#[test]
fn the_user_is_told_when_the_new_league_is_ready_and_when_it_cannot_be() {
    let mut a = Announcer::default();
    assert_eq!(a.poll(&snapshot("Standard", false, None)), None, "nothing to say without a change");

    a.switched("Forbidden Rites");
    assert_eq!(league::loading_text("Forbidden Rites"), "loading Forbidden Rites prices");
    // The old league's table, were it still served, is not the news.
    assert_eq!(a.poll(&snapshot("Standard", false, None)), None);
    assert_eq!(a.poll(&snapshot("Forbidden Rites", true, None)), None);
    assert_eq!(a.poll(&snapshot("Forbidden Rites", false, None)).as_deref(), Some("league: Forbidden Rites"));
    assert_eq!(a.poll(&snapshot("Forbidden Rites", false, None)), None, "said once");

    // A name no price site knows: the reason is shown, once, and the
    // league is still announced if it does come up later.
    a.switched("Forbiden Rites");
    let why = "league \"Forbiden Rites\" is not one poe.ninja lists: check the name in Settings";
    assert_eq!(a.poll(&snapshot("Forbiden Rites", true, Some(why))).as_deref(), Some(why));
    assert_eq!(a.poll(&snapshot("Forbiden Rites", true, Some(why))), None);
    assert_eq!(a.poll(&snapshot("Forbiden Rites", false, None)).as_deref(), Some("league: Forbiden Rites"));
}

#[test]
fn nothing_is_priced_from_a_table_that_is_not_this_leagues() {
    let loading = Some("loading Forbidden Rites prices".to_string());
    assert_eq!(league::not_ready(&snapshot("Standard", false, None), "Forbidden Rites"), loading);
    assert_eq!(league::not_ready(&snapshot("Forbidden Rites", true, None), "Forbidden Rites"), loading);
    assert_eq!(league::not_ready(&snapshot("Forbidden Rites", false, None), "Forbidden Rites"), None);
    // The price site is down: listings are still shown, unconverted.
    assert_eq!(league::not_ready(&snapshot("Forbidden Rites", true, Some("no prices")), "Forbidden Rites"), None);
}

#[test]
fn an_open_card_says_which_league_its_listings_are_from() {
    assert_eq!(league::old_league_status("Standard", "10 of 84 listings"), "league Standard - 10 of 84 listings");
    assert_eq!(league::old_league_status("Standard", ""), "league Standard");
    let once = league::old_league_status("Standard", "10 listings");
    assert_eq!(league::old_league_status("Standard", &once), once, "a second change does not stack the mark");
    // The next Search from that card runs in the new league and says so.
    assert_eq!(
        league::searched_status("3 listings", Some("Standard"), "Forbidden Rites"),
        "3 listings - league Forbidden Rites"
    );
    assert_eq!(league::searched_status("3 listings", Some("Standard"), "Standard"), "3 listings");
    assert_eq!(league::searched_status("3 listings", None, "Standard"), "3 listings");
}

/// A trade site stub that answers every search with no matches and records
/// each request line.
fn trade_stub() -> (String, Arc<Mutex<Vec<String>>>) {
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
            let body = r#"{"id":"abc","result":[],"total":0}"#;
            let _ = write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    (format!("http://{addr}"), seen)
}

#[test]
fn the_trade_worker_searches_the_new_league_after_a_change() {
    let (base, seen) = trade_stub();
    let limiters = Limiters::new();
    let mut client: Retrying<TradeClient> = Retrying::default();
    let mut last_search: ReuseSlot<String, u32> = ReuseSlot::new(REUSE_TTL);
    let now = Instant::now();

    // Before the client exists there is nothing to point anywhere; the
    // worker builds it for whatever league is current when it is needed.
    assert!(!league::retarget(&mut client, &mut last_search, "Standard"));

    client
        .get_or_load(now, || TradeClient::with_limiters(&base, "Standard", limiters.clone()).map_err(|e| e.to_string()))
        .unwrap()
        .search(&Query::default())
        .expect("stub search");
    last_search.put("body".to_string(), 7, now);
    assert!(seen.lock().unwrap()[0].contains("/api/trade2/search/poe2/Standard "), "{:?}", seen.lock().unwrap());

    assert!(league::retarget(&mut client, &mut last_search, "Forbidden Rites"));
    assert_eq!(last_search.get(&"body".to_string(), now), None, "the last search's listings were Standard's");
    let c = client.get_mut().unwrap();
    assert_eq!(c.league(), "Forbidden Rites");
    c.search(&Query::default()).expect("stub search");
    let line = seen.lock().unwrap()[1].clone();
    assert!(line.contains("/api/trade2/search/poe2/Forbidden%20Rites "), "{line}");
    assert!(c.site_url("abc").ends_with("/trade2/search/poe2/Forbidden Rites/abc"));
    // Same registry as before: what the site has counted against this
    // address, and any cooldown it imposed, carry over.
    assert!(c.limiters().shares_with(&limiters));
    assert!(!c.limiters().shares_with(&Limiters::new()));

    // Already there: nothing moves.
    assert!(!league::retarget(&mut client, &mut last_search, "Forbidden Rites"));
}
