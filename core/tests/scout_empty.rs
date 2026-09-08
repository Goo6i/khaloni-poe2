//! poe2scout answering "no unique data for this league" (live 2026-09-08 for
//! Forbidden Rites: every category page comes back empty) is a definitive
//! answer, not a failure: the map is empty and the caller routes uniques to
//! the trade site. It must not be reported as an error on every refresh.

use std::io::{Read, Write};
use std::net::TcpListener;

use khaloni_poe2_core::scout::ScoutClient;

fn serve_empty_league() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { break };
            let mut buf = [0u8; 2048];
            let n = s.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            let body = if req.contains("/Realms/poe2/Filters") {
                r#"{"Filters":[{"DisplayName":"A","Category":"accessory","Identifier":"A","ItemKind":"unique"}]}"#
            } else {
                r#"{"CurrentPage":1,"Pages":0,"Total":0,"Items":[]}"#
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
fn a_league_without_unique_data_is_an_empty_map_not_an_error() {
    let dir = std::env::temp_dir().join(format!("scout-empty-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let client = ScoutClient::with_base(serve_empty_league(), dir.clone());
    let (map, stale) = client.unique_prices("Forbidden Rites").expect("empty is not an error");
    assert!(map.is_empty());
    assert!(!stale, "a fresh, definitive answer is not stale data");
    let _ = std::fs::remove_dir_all(dir);
}
