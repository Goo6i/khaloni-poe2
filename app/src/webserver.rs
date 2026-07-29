//! Local control-panel backend. A tiny HTTP server bound to 127.0.0.1 serves
//! the settings / reference / leveling single-page app and a small JSON API
//! over the config and the EE2 reference data. The in-game overlay stays
//! native; this is only the out-of-game control surface, so a browser UI
//! (dropdowns, key capture, scrolling, resize) is the right tool. Never bound
//! to a public interface.

use std::collections::HashMap;

use poe2_lens_core::refdata::{
    search_affixes, search_keystones, search_ref_entries, search_ref_items, search_uniques, Affix,
    Keystone, LevelingAct, RefEntry, RefItem, UniqueDetail,
};

use crate::config::Config;

/// The control-panel single-page app, embedded so the binary is self-contained.
pub const INDEX_HTML: &str = include_str!("../web/index.html");
const FONTIN_REGULAR: &[u8] = include_bytes!("../assets/fonts/Fontin-Regular.ttf");
const FONTIN_SMALLCAPS: &[u8] = include_bytes!("../assets/fonts/Fontin-SmallCaps.ttf");

/// Everything a request handler reads. Reference data is loaded once.
/// `csrf_token` and `port` are filled by `start` and used by the request guards.
pub struct Ctx {
    pub affixes: Vec<Affix>,
    pub items: Vec<RefItem>,
    pub uniques: Vec<UniqueDetail>,
    pub keystones: Vec<Keystone>,
    /// Generic reference categories keyed by API slug (essences, omens, ...).
    pub categories: HashMap<String, Vec<RefEntry>>,
    pub leveling: Vec<LevelingAct>,
    pub index_html: String,
    pub csrf_token: String,
    pub port: u16,
}

/// The security-relevant request headers the guards inspect.
#[derive(Default)]
pub struct ReqHeaders {
    pub host: Option<String>,
    pub origin: Option<String>,
    pub content_type: Option<String>,
    pub csrf: Option<String>,
}

/// True when the Host header names a loopback host. The key DNS-rebinding
/// defense: a rebound attacker page carries `Host: attacker.example`, which is
/// rejected, while the real panel always sends `127.0.0.1`/`localhost`.
fn host_is_loopback(host: Option<&str>) -> bool {
    let Some(h) = host else { return false };
    let name = h.rsplit_once(':').map_or(h, |(n, _)| n);
    matches!(name, "127.0.0.1" | "localhost" | "::1" | "[::1]")
}

/// True when Origin is absent (same-origin requests may omit it) or matches
/// our own loopback origin. Defense-in-depth beside the CSRF token.
fn origin_ok(origin: Option<&str>, port: u16) -> bool {
    match origin {
        None => true,
        Some(o) => o == format!("http://127.0.0.1:{port}") || o == format!("http://localhost:{port}"),
    }
}

/// Constant-time token comparison, so a timing side channel can't reveal it.
fn csrf_ok(got: Option<&str>, want: &str) -> bool {
    let Some(got) = got else { return false };
    if got.len() != want.len() {
        return false;
    }
    got.bytes().zip(want.bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

pub struct Resp {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

impl Resp {
    fn json(status: u16, s: String) -> Self {
        Resp { status, content_type: "application/json", body: s.into_bytes() }
    }
    fn html(s: &str) -> Self {
        Resp { status: 200, content_type: "text/html; charset=utf-8", body: s.as_bytes().to_vec() }
    }
    fn not_found() -> Self {
        Resp { status: 404, content_type: "text/plain; charset=utf-8", body: b"not found".to_vec() }
    }
    fn font(bytes: &[u8]) -> Self {
        Resp { status: 200, content_type: "font/ttf", body: bytes.to_vec() }
    }
}

fn urldecode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                if let Some(b) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    out.push(b);
                    i += 2;
                } else {
                    out.push(b'%');
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn query_param(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then(|| urldecode(v))
    })
}

/// Resolves one request to a response. Pure over `ctx` plus the config file, so
/// it is unit-testable without a socket. `/api/config` POST persists to
/// config.toml (the only side effect); the reference and whisper routes are
/// read-only.
pub fn route(method: &str, path: &str, query: &str, body: &str, h: &ReqHeaders, ctx: &Ctx) -> Resp {
    // DNS-rebinding guard on EVERY request (including GET, so a rebound page
    // can't even fetch the token-bearing index).
    if !host_is_loopback(h.host.as_deref()) {
        return Resp { status: 403, content_type: "text/plain; charset=utf-8", body: b"forbidden host".to_vec() };
    }
    match (method, path) {
        ("GET", "/") | ("GET", "/index.html") => {
            // Stamp the per-run CSRF token into the page the JS reads.
            Resp::html(&ctx.index_html.replace("%%CSRF%%", &ctx.csrf_token))
        }
        ("GET", "/fonts/Fontin-Regular.ttf") => Resp::font(FONTIN_REGULAR),
        ("GET", "/fonts/Fontin-SmallCaps.ttf") => Resp::font(FONTIN_SMALLCAPS),
        ("GET", "/api/config") => match Config::load() {
            Ok(c) => Resp::json(200, serde_json::to_string(&c).unwrap_or_default()),
            Err(e) => Resp::json(500, serde_json::json!({"error": e.to_string()}).to_string()),
        },
        ("POST", "/api/config") => {
            // Mutating: require same-origin, a JSON body, and the CSRF token,
            // so no cross-site page can rewrite config (which drives process
            // spawns like tesseract_cmd).
            if !origin_ok(h.origin.as_deref(), ctx.port) {
                return Resp::json(403, r#"{"error":"bad origin"}"#.into());
            }
            if h.content_type.as_deref().map(|c| c.starts_with("application/json")) != Some(true) {
                return Resp::json(415, r#"{"error":"expected application/json"}"#.into());
            }
            if !csrf_ok(h.csrf.as_deref(), &ctx.csrf_token) {
                return Resp::json(403, r#"{"error":"bad csrf token"}"#.into());
            }
            match serde_json::from_str::<Config>(body) {
                Ok(c) => match c.save() {
                    Ok(()) => Resp::json(200, r#"{"ok":true}"#.into()),
                    Err(e) => Resp::json(500, serde_json::json!({"error": e.to_string()}).to_string()),
                },
                Err(e) => Resp::json(400, serde_json::json!({"error": e.to_string()}).to_string()),
            }
        }
        ("GET", "/api/affixes") => {
            let q = query_param(query, "q").unwrap_or_default();
            let hits: Vec<&Affix> = search_affixes(&ctx.affixes, &q).into_iter().take(300).collect();
            Resp::json(200, serde_json::to_string(&hits).unwrap_or_else(|_| "[]".into()))
        }
        ("GET", "/api/items") => {
            let q = query_param(query, "q").unwrap_or_default();
            let ns = query_param(query, "ns");
            let hits: Vec<&RefItem> =
                search_ref_items(&ctx.items, &q, ns.as_deref()).into_iter().take(300).collect();
            Resp::json(200, serde_json::to_string(&hits).unwrap_or_else(|_| "[]".into()))
        }
        ("GET", "/api/uniques") => {
            let q = query_param(query, "q").unwrap_or_default();
            let hits: Vec<&UniqueDetail> = search_uniques(&ctx.uniques, &q).into_iter().take(300).collect();
            Resp::json(200, serde_json::to_string(&hits).unwrap_or_else(|_| "[]".into()))
        }
        ("GET", "/api/keystones") => {
            let q = query_param(query, "q").unwrap_or_default();
            let hits: Vec<&Keystone> = search_keystones(&ctx.keystones, &q).into_iter().take(300).collect();
            Resp::json(200, serde_json::to_string(&hits).unwrap_or_else(|_| "[]".into()))
        }
        ("GET", "/api/ref") => {
            let cat = query_param(query, "cat").unwrap_or_default();
            let q = query_param(query, "q").unwrap_or_default();
            let hits: Vec<&RefEntry> = ctx
                .categories
                .get(&cat)
                .map(|entries| search_ref_entries(entries, &q).into_iter().take(300).collect())
                .unwrap_or_default();
            Resp::json(200, serde_json::to_string(&hits).unwrap_or_else(|_| "[]".into()))
        }
        ("GET", "/api/leveling") => {
            Resp::json(200, serde_json::to_string(&ctx.leveling).unwrap_or_else(|_| "[]".into()))
        }
        _ => Resp::not_found(),
    }
}

pub use crate::refcache::{reference_data, Reference};

/// Binds the control panel to a loopback port (first free in a small range) and
/// serves it on a background thread. Returns the chosen port for opening the
/// browser. 127.0.0.1 only: never reachable off the machine.
pub fn start(mut ctx: Ctx) -> anyhow::Result<u16> {
    let mut bound = None;
    for p in 7997u16..8020 {
        if let Ok(s) = tiny_http::Server::http(("127.0.0.1", p)) {
            bound = Some((s, p));
            break;
        }
    }
    let (server, port) = bound.ok_or_else(|| anyhow::anyhow!("no free loopback port for control panel"))?;
    ctx.port = port;
    ctx.csrf_token = random_token();
    std::thread::spawn(move || {
        for mut req in server.incoming_requests() {
            let mut h = ReqHeaders::default();
            for hdr in req.headers() {
                let field = hdr.field.as_str().as_str().to_ascii_lowercase();
                let val = hdr.value.as_str().to_string();
                match field.as_str() {
                    "host" => h.host = Some(val),
                    "origin" => h.origin = Some(val),
                    "content-type" => h.content_type = Some(val),
                    "x-csrf-token" => h.csrf = Some(val),
                    _ => {}
                }
            }
            let method = req.method().as_str().to_string();
            let url = req.url().to_string();
            let (path, query) = url.split_once('?').unwrap_or((url.as_str(), ""));
            let mut body = String::new();
            let _ = req.as_reader().read_to_string(&mut body);
            let resp = route(&method, path, query, &body, &h, &ctx);
            let header = tiny_http::Header::from_bytes(&b"Content-Type"[..], resp.content_type.as_bytes())
                .expect("valid header");
            let response = tiny_http::Response::from_data(resp.body)
                .with_status_code(resp.status)
                .with_header(header);
            let _ = req.respond(response);
        }
    });
    Ok(port)
}

/// A 128-bit random token as hex, from the OS CSPRNG (/dev/urandom). Falls
/// back to a time/pid mix only if urandom is unreadable (still unguessable
/// enough for a loopback-only, per-run token; the Host guard is the main line).
fn random_token() -> String {
    let mut buf = [0u8; 16];
    if std::fs::File::open("/dev/urandom")
        .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut buf))
        .is_ok()
    {
        return buf.iter().map(|b| format!("{b:02x}")).collect();
    }
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
        ^ u128::from(std::process::id());
    format!("{seed:032x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> Ctx {
        Ctx {
            affixes: vec![
                Affix { text: "#% increased Attack Speed".into(), trade_ids: vec!["explicit.stat_1".into()] },
                Affix { text: "+# to maximum Life".into(), trade_ids: vec!["explicit.stat_2".into()] },
            ],
            items: vec![
                RefItem { name: "Wanderlust".into(), namespace: "UNIQUE".into(), category: Some("Boots".into()) },
                RefItem { name: "Emerald Ring".into(), namespace: "ITEM".into(), category: Some("Ring".into()) },
            ],
            uniques: vec![UniqueDetail {
                name: "Brynhand's Mark".into(),
                base: "Wooden Club".into(),
                mods: vec!["Causes Double Stun Buildup".into()],
            }],
            keystones: vec![Keystone {
                name: "Resolute Technique".into(),
                description: "Never deal Critical Hits".into(),
            }],
            categories: HashMap::from([(
                "omens".to_string(),
                vec![RefEntry { name: "Omen of Foo".into(), lines: vec!["do a thing".into()] }],
            )]),
            leveling: vec![LevelingAct {
                act: 1,
                name: "Grelwood".into(),
                steps: vec![poe2_lens_core::refdata::LevelingStep {
                    id: "a1".into(), kind: "kill_boss".into(), zone: "Riverbank".into(),
                    description: "Kill the Miller".into(), hint: "".into(),
                }],
            }],
            index_html: "<!doctype html><meta name=csrf content=%%CSRF%%><title>panel</title>".into(),
            csrf_token: "secret-token".into(),
            port: 7997,
        }
    }
    // Loopback GET headers (passes the host guard).
    fn h() -> ReqHeaders {
        ReqHeaders { host: Some("127.0.0.1:7997".into()), ..Default::default() }
    }

    #[test]
    fn serves_index_with_token_and_404s_unknown() {
        let c = ctx();
        let idx = route("GET", "/", "", "", &h(), &c);
        assert_eq!(idx.status, 200);
        assert!(String::from_utf8_lossy(&idx.body).contains("secret-token"), "token stamped in");
        assert_eq!(route("GET", "/nope", "", "", &h(), &c).status, 404);
    }

    #[test]
    fn affix_and_item_routes_work() {
        let c = ctx();
        let a: serde_json::Value = serde_json::from_slice(&route("GET", "/api/affixes", "q=attack", "", &h(), &c).body).unwrap();
        assert_eq!(a.as_array().unwrap().len(), 1);
        let i: serde_json::Value = serde_json::from_slice(&route("GET", "/api/items", "q=&ns=UNIQUE", "", &h(), &c).body).unwrap();
        assert_eq!(i[0]["name"], "Wanderlust");
        let u: serde_json::Value = serde_json::from_slice(&route("GET", "/api/uniques", "q=double+stun", "", &h(), &c).body).unwrap();
        assert_eq!(u[0]["name"], "Brynhand's Mark");
        let k: serde_json::Value = serde_json::from_slice(&route("GET", "/api/keystones", "q=critical", "", &h(), &c).body).unwrap();
        assert_eq!(k[0]["name"], "Resolute Technique");
        let g: serde_json::Value = serde_json::from_slice(&route("GET", "/api/ref", "cat=omens&q=thing", "", &h(), &c).body).unwrap();
        assert_eq!(g[0]["name"], "Omen of Foo");
        // Unknown category -> empty, not an error.
        let none: serde_json::Value = serde_json::from_slice(&route("GET", "/api/ref", "cat=nope&q=", "", &h(), &c).body).unwrap();
        assert_eq!(none.as_array().unwrap().len(), 0);
    }

    #[test]
    fn rejects_non_loopback_host_dns_rebinding() {
        let c = ctx();
        let mut bad = h();
        bad.host = Some("attacker.example".into());
        assert_eq!(route("GET", "/", "", "", &bad, &c).status, 403);
        assert_eq!(route("GET", "/api/config", "", "", &bad, &c).status, 403);
    }

    #[test]
    fn config_post_requires_csrf_json_and_same_origin() {
        let c = ctx();
        let body = serde_json::to_string(&crate::config::Config::default()).unwrap();
        // Missing token -> rejected.
        let mut hh = h(); hh.content_type = Some("application/json".into());
        assert_eq!(route("POST", "/api/config", "", &body, &hh, &c).status, 403);
        // Wrong content-type -> 415.
        let mut hh2 = h(); hh2.csrf = Some("secret-token".into()); hh2.content_type = Some("text/plain".into());
        assert_eq!(route("POST", "/api/config", "", &body, &hh2, &c).status, 415);
        // Cross-origin -> rejected.
        let mut hh3 = h(); hh3.csrf = Some("secret-token".into()); hh3.content_type = Some("application/json".into());
        hh3.origin = Some("http://evil.example".into());
        assert_eq!(route("POST", "/api/config", "", &body, &hh3, &c).status, 403);
        // Wrong token -> rejected (constant-time compare still 403).
        let mut hh4 = h(); hh4.csrf = Some("nope".into()); hh4.content_type = Some("application/json".into());
        assert_eq!(route("POST", "/api/config", "", &body, &hh4, &c).status, 403);
    }

    #[test]
    fn guards_helpers() {
        assert!(host_is_loopback(Some("127.0.0.1:7997")));
        assert!(host_is_loopback(Some("localhost:7997")));
        assert!(!host_is_loopback(Some("evil.example")));
        assert!(!host_is_loopback(None));
        assert!(origin_ok(None, 7997));
        assert!(origin_ok(Some("http://127.0.0.1:7997"), 7997));
        assert!(!origin_ok(Some("http://evil.example"), 7997));
        assert!(csrf_ok(Some("abc"), "abc"));
        assert!(!csrf_ok(Some("abd"), "abc"));
        assert!(!csrf_ok(None, "abc"));
    }

    #[test]
    fn urldecode_handles_percent_and_plus() {
        assert_eq!(urldecode("attack+speed"), "attack speed");
        assert_eq!(urldecode("%2B%23%20life"), "+# life");
    }
}
