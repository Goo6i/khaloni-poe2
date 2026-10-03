//! The price caches on disk: what may replace them, and when they are
//! served instead of what the network said. All against a local stub.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use khaloni_poe2_core::ninja::{write_cache_atomic, DataOrigin, NinjaClient, NinjaError};
use khaloni_poe2_core::scout::ScoutClient;

const CURRENCY_JSON: &str = include_str!("fixtures/ninja_currency.json");

/// Serves whatever body is currently in the slot, as a 200.
fn serve(body: Arc<Mutex<String>>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { break };
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf);
            let body = body.lock().unwrap().clone();
            let _ = write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
        }
    });
    format!("http://{addr}")
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("khalonipoe2-ninjacache-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn variant(edit: impl FnOnce(&mut serde_json::Value)) -> String {
    let mut v: serde_json::Value = serde_json::from_str(CURRENCY_JSON).unwrap();
    edit(&mut v);
    v.to_string()
}

#[test]
fn a_200_that_is_not_usable_falls_back_to_the_cache_and_leaves_it_alone() {
    let dir = temp_dir("fallback");
    let slot = Arc::new(Mutex::new(CURRENCY_JSON.to_string()));
    let client = NinjaClient::with_base(serve(slot.clone()), dir.clone());

    let (ov, origin) = client.exchange_overview("Test League", "Currency").expect("good body");
    assert_eq!(origin, DataOrigin::Fresh);
    let cache = dir.join("Test League-Currency.json");
    assert_eq!(std::fs::read_to_string(&cache).unwrap(), CURRENCY_JSON);

    let unusable = [
        "<html>Just a moment...</html>".to_string(),
        variant(|v| v["lines"] = serde_json::json!([])),
        variant(|v| v["core"]["primary"] = serde_json::json!("chaos")),
        variant(|v| v["core"]["rates"]["exalted"] = serde_json::json!(0.0)),
    ];
    for body in unusable {
        *slot.lock().unwrap() = body.clone();
        let (stale, origin) = client
            .exchange_overview("Test League", "Currency")
            .unwrap_or_else(|e| panic!("no fallback for {body:.40}: {e}"));
        assert_eq!(origin, DataOrigin::StaleCache, "{body:.40}");
        assert_eq!(stale.lines.len(), ov.lines.len());
        assert_eq!(std::fs::read_to_string(&cache).unwrap(), CURRENCY_JSON, "a bad answer replaced a good cache");
    }
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "no temp litter");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn with_nothing_cached_the_bodys_own_error_is_reported() {
    let dir = temp_dir("nocache");
    let slot = Arc::new(Mutex::new(variant(|v| v["lines"] = serde_json::json!([]))));
    let client = NinjaClient::with_base(serve(slot), dir.clone());
    assert!(matches!(client.exchange_overview("Test League", "Omens"), Err(NinjaError::EmptyResponse(_))));
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "nothing worth caching");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_torn_cache_file_is_no_data_rather_than_a_parse_error_forever() {
    let dir = temp_dir("torn");
    std::fs::write(dir.join("Test League-Currency.json"), &CURRENCY_JSON[..CURRENCY_JSON.len() / 2]).unwrap();
    let client = NinjaClient::with_base("http://127.0.0.1:1".into(), dir.clone());
    assert!(matches!(client.exchange_overview("Test League", "Currency"), Err(NinjaError::NoData(_))));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn atomic_writes_replace_whole_files_and_leave_no_temp_behind() {
    let dir = temp_dir("atomic");
    let path = dir.join("cache.json");
    write_cache_atomic(&path, b"first").unwrap();
    write_cache_atomic(&path, b"second, longer").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "second, longer");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    // A directory that does not exist yet is created.
    write_cache_atomic(&dir.join("sub/dir/cache.json"), b"x").unwrap();
    let _ = std::fs::remove_dir_all(dir);
}

/// poe2scout stub: one unique category whose single page is `items`.
fn serve_scout(items: Arc<Mutex<String>>, categories: Arc<Mutex<bool>>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { break };
            let mut buf = [0u8; 4096];
            let n = s.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let body = if req.contains("/Realms/poe2/Filters") {
                if *categories.lock().unwrap() {
                    r#"{"Filters":[{"DisplayName":"A","Category":"accessory","Identifier":"A","ItemKind":"unique"}]}"#.to_string()
                } else {
                    r#"{"Filters":[]}"#.to_string()
                }
            } else {
                format!(r#"{{"CurrentPage":1,"Pages":1,"Total":0,"Items":[{}]}}"#, items.lock().unwrap())
            };
            let _ = write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
        }
    });
    format!("http://{addr}")
}

#[test]
fn an_empty_scout_answer_never_replaces_a_cache_that_holds_prices() {
    let dir = temp_dir("scout");
    let items = Arc::new(Mutex::new(r#"{"Name":"The Gnashing Sash","CurrentPrice":415.0}"#.to_string()));
    let categories = Arc::new(Mutex::new(true));
    let client = ScoutClient::with_base(serve_scout(items.clone(), categories.clone()), dir.clone());
    let (map, stale) = client.unique_prices("Test League").unwrap();
    assert_eq!((map.get("The Gnashing Sash"), stale), (Some(&415.0), false));
    let cache = dir.join("scout-uniques-Test League.json");
    let good = std::fs::read_to_string(&cache).unwrap();

    // The API reports no categories: the answer is passed on, the cache is
    // left holding the prices.
    *categories.lock().unwrap() = false;
    let (map, _) = client.unique_prices("Test League").unwrap();
    assert!(map.is_empty());
    assert_eq!(std::fs::read_to_string(&cache).unwrap(), good);

    // So the next outage still has something to serve.
    let offline = ScoutClient::with_base("http://127.0.0.1:1".into(), dir.clone());
    let (map, stale) = offline.unique_prices("Test League").unwrap();
    assert_eq!((map.get("The Gnashing Sash"), stale), (Some(&415.0), true));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_scout_name_listed_at_two_prices_is_left_out() {
    let dir = temp_dir("scoutdup");
    let items = Arc::new(Mutex::new(
        r#"{"Name":"Alpha's Howl","CurrentPrice":100.0},{"Name":"Alpha's Howl","CurrentPrice":1275.0},{"Name":"Revered Resin","CurrentPrice":12.5}"#
            .to_string(),
    ));
    let client = ScoutClient::with_base(serve_scout(items, Arc::new(Mutex::new(true))), dir.clone());
    let (map, _) = client.unique_prices("Test League").unwrap();
    assert_eq!(map.get("Alpha's Howl"), None, "neither variant's price may stand for the name");
    assert_eq!(map.get("Revered Resin"), Some(&12.5));
    let _ = std::fs::remove_dir_all(dir);
}
