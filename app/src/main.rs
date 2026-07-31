// On non-Linux targets the overlay/headless pipelines are compiled out
// (they need the Linux OCR stack; see platform/windows/mod.rs), which
// leaves their helpers and imports dead there. Linux lints are unaffected.
#![cfg_attr(not(ocr), allow(dead_code, unused_imports))]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use khaloni_poe2::{
    config::{Config, Rect},
    coord::CoordMap,
    hover, ocr,
    platform::{capture, inject},
    pricing, prices,
};
use khaloni_poe2_core::ninja::NinjaClient;

fn main() -> anyhow::Result<()> {
    migrate_legacy_dirs();
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str).unwrap_or("") {
        "--headless" => headless(),
        "--settings" => khaloni_poe2::settings_ui::run(),
        _ => overlay_mode(),
    }
}

fn game_window_logical() -> Rect {
    // Stage A shortcut: the reference game window is the fullscreen gamescope
    // window on DP-2. Stage B replaces this with the live KWin geometry feed.
    Rect {
        x: 2560,
        y: 0,
        w: 2560,
        h: 1440,
    }
}

/// Percent-encodes a string for use in a URL query (RFC 3986 unreserved
/// set kept literal; everything else percent-encoded, space as %20).
/// Prices one specific cut skill gem: resolve the OCR'd name to an exact gem
/// type, item-search it at the given level, and convert the cheapest listing
/// to exalted via the currency table. `Unpriced` when the name doesn't resolve
/// or there are no listings; leaves it for the caller to cache.
fn price_one_gem(
    client: &mut khaloni_poe2_core::trade::TradeClient,
    skill_lower: &str,
    level: u32,
    gem_types: &[String],
    cur_id_to_name: &std::collections::HashMap<String, String>,
    table: &khaloni_poe2_core::ninja::PriceTable,
) -> khaloni_poe2::pricing::GemState {
    use khaloni_poe2::pricing::GemState;
    let Some(name) = khaloni_poe2_core::trade::match_gem_name(skill_lower, gem_types) else {
        return GemState::Unpriced;
    };
    let listings = match client.price_gem(&name, i64::from(level)) {
        Ok(l) => l,
        // A transient error (rate limit, network): stay Pending so the next
        // scan re-requests, rather than caching a wrong "unpriced".
        Err(_) => return GemState::Pending,
    };
    // Cheapest listing that converts to exalted (search is price-asc, so the
    // first convertible one is the floor).
    for l in &listings {
        let ex = if l.price_currency == "exalted" {
            Some(l.price_amount)
        } else {
            cur_id_to_name
                .get(&l.price_currency)
                .and_then(|n| table.lookup(n))
                .map(|p| l.price_amount * p.exalted)
        };
        if let Some(ex) = ex {
            return GemState::Priced(ex);
        }
    }
    GemState::Unpriced
}

/// Turns a trade category id ("weapon.bow", "armour.helmet") into a readable
/// label ("Bow", "Helmet") for the panel's base-type toggle.
fn pretty_category(cat: &str) -> String {
    let leaf = cat.rsplit('.').next().unwrap_or(cat);
    let mut out = String::with_capacity(leaf.len());
    let mut start = true;
    for ch in leaf.chars() {
        if ch == '_' {
            out.push(' ');
            start = true;
        } else if start {
            out.extend(ch.to_uppercase());
            start = false;
        } else {
            out.push(ch);
        }
    }
    out
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Opens `url_template` (with `{name}` replaced by the copied item's name,
/// URL-encoded) in the default browser. Uses the base type when the item has
/// no distinct name (magic/normal). No-ops on an unparseable/empty item.
fn open_resource(url_template: &str, item_text: &str) {
    let name = khaloni_poe2_core::item::parse_item(item_text)
        .ok()
        .and_then(|it| {
            if it.name.trim().is_empty() {
                it.base_type
            } else {
                Some(it.name)
            }
        })
        .unwrap_or_default();
    if name.trim().is_empty() {
        return;
    }
    let url = url_template.replace("{name}", &urlencode(name.trim()));
    open_url(&url);
}

/// Opens a URL in the default browser, per-OS. Detached spawn: the overlay
/// must never block on a browser starting up.
fn open_url(url: &str) {
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    // `start` is a cmd builtin; the empty "" is its window-title slot so a
    // URL containing spaces is not mistaken for the title.
    let _ = std::process::Command::new("cmd").args(["/C", "start", "", url]).spawn();
}

/// Sets the overlay's pointer input region to the union bounding box of
/// every open interactive panel (appraisal, reference, leveling), or clears
/// it when none is open. One region because the layer surface supports a
/// single rect; the union is slightly generous when panels are far apart,
/// but clicks between them still fall through to nothing (hit() misses).
fn sync_input_region(
    overlay: &mut khaloni_poe2::platform::overlay::Overlay,
    renderer: &khaloni_poe2::render::Renderer,
    apanel: &Option<(khaloni_poe2::appraise_ui::Panel, khaloni_poe2_core::trade::Query, (i32, i32))>,
    ref_panel: &Option<(khaloni_poe2::reference_ui::Panel, (i32, i32))>,
    lvl_panel: &Option<(khaloni_poe2::leveling_ui::Panel, (i32, i32))>,
) -> anyhow::Result<()> {
    let out = overlay.output_pos();
    let measure = |s: &str| renderer.appraisal_label_width(s);
    let mut boxes: Vec<(i32, i32, i32, i32)> = Vec::new();
    if let Some((p, _, pos)) = apanel {
        let lay = khaloni_poe2::appraise_ui::layout(p, &measure);
        boxes.push((pos.0 - out.0, pos.1 - out.1, lay.size.0, lay.size.1));
    }
    if let Some((p, pos)) = ref_panel {
        let lay = khaloni_poe2::reference_ui::layout(p, &measure);
        boxes.push((pos.0 - out.0, pos.1 - out.1, lay.w, lay.h));
    }
    if let Some((p, pos)) = lvl_panel {
        let lay = khaloni_poe2::leveling_ui::layout(p, &measure);
        boxes.push((pos.0 - out.0, pos.1 - out.1, lay.w, lay.h));
    }
    let union = boxes.into_iter().fold(None, |acc: Option<(i32, i32, i32, i32)>, (x, y, w, h)| {
        Some(match acc {
            None => (x, y, x + w, y + h),
            Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x + w), y1.max(y + h)),
        })
    });
    overlay.set_interactive(
        union.map(|(x0, y0, x1, y1)| (x0, y0, (x1 - x0).max(0) as u32, (y1 - y0).max(0) as u32)),
    )
}

/// Built-in map-mod seed rules plus the config's extra needles, lowercased.
fn build_map_rules(cfg: &Config) -> Vec<khaloni_poe2_core::mapmods::ModRule> {
    let mut r = khaloni_poe2_core::mapmods::default_rules();
    for n in &cfg.map_danger_needles {
        r.push(khaloni_poe2_core::mapmods::ModRule {
            needle: n.to_lowercase(),
            kind: khaloni_poe2_core::mapmods::ModKind::Danger,
        });
    }
    for n in &cfg.map_good_needles {
        r.push(khaloni_poe2_core::mapmods::ModRule {
            needle: n.to_lowercase(),
            kind: khaloni_poe2_core::mapmods::ModKind::Good,
        });
    }
    r
}

/// One-time rename of the pre-rename "poe2-lens" config/cache dirs to the
/// "khaloni-poe2" locations, so calibration, tokens, rumours.csv, and the
/// reference cache survive the project rename. Only fires when the old dir
/// exists and the new one does not; best-effort on every mode's startup.
fn migrate_legacy_dirs() {
    let (Some(old), Some(new)) = (
        directories::ProjectDirs::from("", "", "poe2-lens"),
        directories::ProjectDirs::from("", "", "khaloni-poe2"),
    ) else {
        return;
    };
    for (o, n) in [
        (old.config_dir(), new.config_dir()),
        (old.cache_dir(), new.cache_dir()),
    ] {
        if o.exists() && !n.exists() {
            if let Some(parent) = n.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match std::fs::rename(o, n) {
                Ok(()) => eprintln!("migrated {} -> {}", o.display(), n.display()),
                Err(e) => eprintln!("dir migration failed ({} -> {}): {e}", o.display(), n.display()),
            }
        }
    }
}

/// Launches the native settings window as its own process; the overlay keeps
/// running and picks config changes up via the mtime watcher, so no IPC.
fn open_settings() {
    match std::env::current_exe() {
        Ok(exe) => {
            let _ = std::process::Command::new(exe).arg("--settings").spawn();
        }
        Err(e) => eprintln!("settings window: cannot find own binary: {e}"),
    }
}

/// Headless one-shot needs the Linux capture + OCR stack; the Windows
/// backend lands in SP3 (see platform/windows/mod.rs).
#[cfg(not(ocr))]
fn headless() -> anyhow::Result<()> {
    anyhow::bail!("this build has no OCR (windows-gnu check target); the shipped Windows build is MSVC with vcpkg tesseract")
}

#[cfg(ocr)]
fn headless() -> anyhow::Result<()> {
    let mut cfg = Config::load()?;

    eprintln!("fetching prices for {}...", cfg.league);
    let cache = directories::ProjectDirs::from("", "", "khaloni-poe2")
        .unwrap()
        .cache_dir()
        .to_path_buf();
    let svc = prices::PriceService::start_with_interval(
        NinjaClient::new(cache.clone()),
        khaloni_poe2_core::scout::ScoutClient::new(cache),
        cfg.league.clone(),
        std::time::Duration::from_secs(cfg.refresh_minutes * 60),
    )?;
    eprintln!("price table ready ({} names)", svc.snapshot().table.len());

    let rt = tokio::runtime::Runtime::new()?;
    let start = rt.block_on(capture::portal_session(cfg.restore_token.as_deref()))?;
    if let Some(tok) = &start.new_token {
        cfg.restore_token = Some(tok.clone());
        cfg.save()?;
    }

    // Headless works from full frames only: detect the reward panel on
    // each frame (zero calibration), crop in-process, and scan the crop.
    // The region channel is unused; the region path's throttling doesn't
    // matter because the dummy region below is never OCR'd.
    let (ftx, frx) = mpsc::sync_channel(1);
    let (_rtx, rrx) = mpsc::channel::<Rect>();
    let (full_tx, full_rx) = mpsc::sync_channel::<image::GrayImage>(1);
    let panel_open = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    std::thread::spawn(move || {
        let dummy = Rect { x: 0, y: 0, w: 64, h: 64 };
        if let Err(e) = capture::consume(start, rrx, dummy, ftx, panel_open, Some(full_tx)) {
            eprintln!("capture thread died: {e}");
        }
    });
    // Keep the region channel drained so capture's try_send never backs up.
    std::thread::spawn(move || for _ in frx {});

    eprintln!("headless pipeline running; open a Runeshape panel. Ctrl+C to quit.");
    let game = game_window_logical();
    let mut engine = ocr::OcrEngine::new()?;
    for frame in full_rx {
        let Some(region) = khaloni_poe2::autoregion::detect_reward_region(&frame) else {
            continue;
        };
        let crop = image::imageops::crop_imm(
            &frame,
            region.x0,
            region.y0,
            region.x1 - region.x0,
            region.y1 - region.y0,
        )
        .to_image();
        let map = CoordMap::new(
            game,
            (frame.width(), frame.height()),
            Rect {
                x: region.x0 as i32,
                y: region.y0 as i32,
                w: region.x1 - region.x0,
                h: region.y1 - region.y0,
            },
        );
        let lines = ocr::ocr_scan(&mut engine, &crop);
        let snap = svc.snapshot();
        let (rows, total) = pricing::price_lines(&snap.table, &snap.vocab, &lines, &cfg);
        println!(
            "--- scan region {}x{}@({},{}) ({} lines, {} priced){}",
            region.x1 - region.x0,
            region.y1 - region.y0,
            region.x0,
            region.y0,
            lines.len(),
            rows.len(),
            if snap.stale { " [STALE PRICES]" } else { "" }
        );
        for r in &rows {
            let (lx, ly) = map.label_pos_centered(r.y_top, r.height);
            println!("  y={:>4} ({lx},{ly})  {:?}  {}", r.y_top, r.tier, r.label);
        }
        if !total.is_empty() {
            println!("  {total}");
        }
    }
    Ok(())
}

/// What one tick draws+presents: the row labels, the header line (the
/// div=>ex rate), whether prices are stale, the hover popup (with its
/// anchor), and the interactive appraisal panel (with its anchor).
type FrameState = (
    Vec<khaloni_poe2::render::Placed>,
    String,
    bool,
    Option<(hover::Popup, (i32, i32))>,
    Option<(khaloni_poe2::appraise_ui::Panel, (i32, i32))>,
    Vec<khaloni_poe2::render::RumourBadge>,
    // Focused value box (filter index, field, live edit buffer), so typed
    // digits repaint even though the committed panel values are unchanged.
    Option<(usize, khaloni_poe2::appraise_ui::Field, String)>,
    // In-overlay reference search and leveling checklist panels.
    Option<(khaloni_poe2::reference_ui::Panel, (i32, i32))>,
    Option<(khaloni_poe2::leveling_ui::Panel, (i32, i32))>,
);

/// Shared placement geometry from the full-frame worker: (capture frame
/// dims, detected reward region in capture px), both None until first seen.
type ScanGeom = std::sync::Arc<std::sync::Mutex<(Option<(u32, u32)>, Option<Rect>)>>;

/// What an in-flight copy-hovered request (other than a price check) should
/// do with the copied item text once it arrives.
enum PendingAction {
    /// Open the item in the browser via `Config::resource_shortcuts[i]`.
    Shortcut(usize),
}

/// Appraisal worker requests: Auto = fresh item, build the query and
/// relax until listings appear; Exact = the user's checkbox state, run
/// verbatim with no relaxation (their toggle IS the intent).
enum AppraiseReq {
    Auto(khaloni_poe2_core::item::Item),
    Exact { title: String, query: khaloni_poe2_core::trade::Query },
    /// Price a stackable currency (e.g. an omen) by its display name via the
    /// trade exchange; the result comes back on the exchange channel.
    Currency { name: String },
    /// Price a specific cut skill gem (reward-panel "Skill Level N: <name>")
    /// by name + level via item search; the result is written to the shared
    /// gem cache the OCR pricer reads.
    Gem { skill: String, level: u32 },
}

/// Shared cache of specific-gem prices, written by the trade worker and read
/// (with lazy request) by the reward-panel pricer.
type GemMap = std::sync::Arc<std::sync::Mutex<std::collections::HashMap<(String, u32), khaloni_poe2::pricing::GemState>>>;

/// Reads a gem's cached price and, on a miss, marks it pending and asks the
/// trade worker to price it.
struct GemCache {
    map: GemMap,
    req_tx: mpsc::Sender<AppraiseReq>,
}

impl khaloni_poe2::pricing::GemPricer for GemCache {
    fn lookup(&self, skill_lower: &str, level: u32) -> khaloni_poe2::pricing::GemState {
        let key = (skill_lower.to_string(), level);
        let mut m = self.map.lock().unwrap();
        if let Some(state) = m.get(&key) {
            return *state;
        }
        m.insert(key.clone(), khaloni_poe2::pricing::GemState::Pending);
        drop(m);
        let _ = self.req_tx.send(AppraiseReq::Gem { skill: skill_lower.to_string(), level });
        khaloni_poe2::pricing::GemState::Pending
    }
}

struct AppraiseDone {
    title: String,
    outcome: Result<Vec<khaloni_poe2_core::trade::Listing>, String>,
    /// Query + labels only on Auto responses (they seed the panel); an
    /// Exact response updates listings on the panel the user already has.
    query: Option<khaloni_poe2_core::trade::Query>,
    labels: Vec<khaloni_poe2_core::trade::FilterLabel>,
    search_id: Option<String>,
}

/// The live overlay drives the Linux backends (and the Linux OCR stack)
/// directly; the Windows backend lands in SP3 (see platform/windows/mod.rs).
#[cfg(not(ocr))]
fn overlay_mode() -> anyhow::Result<()> {
    anyhow::bail!("this build has no OCR (windows-gnu check target); the shipped Windows build is MSVC with vcpkg tesseract")
}

#[cfg(ocr)]
fn overlay_mode() -> anyhow::Result<()> {
    // Startup phase timing, permanently logged: cold-start stalls are only
    // diagnosable from user reports, and eight lines of log are cheap.
    let boot = std::time::Instant::now();
    let phase = move |name: &str| eprintln!("t+{:>5}ms {}", boot.elapsed().as_millis(), name);
    let mut cfg = Config::load()?;

    let cache = directories::ProjectDirs::from("", "", "khaloni-poe2").unwrap().cache_dir().to_path_buf();
    let svc = prices::PriceService::start_with_interval(
        NinjaClient::new(cache.clone()),
        khaloni_poe2_core::scout::ScoutClient::new(cache),
        cfg.league.clone(),
        std::time::Duration::from_secs(cfg.refresh_minutes * 60),
    )?;

    phase("price service up");
    let kwin = khaloni_poe2::platform::gamewin::start()?;
    phase("window tracker up");
    // First geometry fixes the output; 0,0,0,0 means no game yet.
    let mut game = Rect { x: 2560, y: 0, w: 2560, h: 1440 };
    let geometry_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let remaining = geometry_deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            // Deadline expired with no Geometry event seen; keep the fallback rect.
            break;
        }
        match kwin.rx.recv_timeout(remaining) {
            Ok(khaloni_poe2::platform::GameWindowEvent::Geometry(g)) => {
                game = g;
                break;
            }
            Ok(_) => continue, // ignore Active/GameGone while waiting for the real geometry
            Err(_) => break,   // channel closed or timed out
        }
    }

    phase("game geometry known");
    let rt = tokio::runtime::Runtime::new()?;
    // Identify ourselves to xdg-desktop-portal BEFORE any other portal call.
    // ashpd shares one session-bus connection across all its proxies, and the
    // FIRST portal request (ScreenCast below) permanently binds that
    // connection to an app id: for a terminal-launched app that id is empty,
    // and KDE's GlobalShortcuts portal then refuses it ("An app id is
    // required"). Registering here claims a real id first, so hotkeys bind.
    // Best-effort: logged, never fatal. Portal machinery is Linux-only;
    // Windows hotkeys (RegisterHotKey) need no identity.
    #[cfg(target_os = "linux")]
    rt.block_on(async {
        match "dev.goo6i.khalonipoe2".parse::<ashpd::AppID>() {
            Ok(app_id) => {
                if let Err(e) = ashpd::register_host_app(app_id).await {
                    eprintln!("app-id registration failed (hotkeys may not bind): {e}");
                }
            }
            Err(e) => eprintln!("invalid app id: {e}"),
        }
    });
    phase("app id registered");
    let start = rt.block_on(capture::portal_session(cfg.restore_token.as_deref()))?;
    phase("capture session ready");
    if let Some(tok) = &start.new_token {
        cfg.restore_token = Some(tok.clone());
        cfg.save()?;
    }
    let (hk_tx, hk_rx) = mpsc::channel();
    {
        let (check, overlay) = (cfg.hotkey_price_check.clone(), cfg.hotkey_overlay.clone());
        // (id, trigger) for every dynamic action: chat macros as "macro-N",
        // resource shortcuts as "url-N". The main loop routes by id prefix.
        let mut extra: Vec<(String, String)> = cfg
            .macros
            .iter()
            .enumerate()
            .map(|(i, m)| (format!("macro-{i}"), m.key.clone()))
            .collect();
        extra.extend(
            cfg.resource_shortcuts
                .iter()
                .enumerate()
                .map(|(i, s)| (format!("url-{i}"), s.key.clone())),
        );
        // The settings window opens on its own shortcut (id "settings"); the
        // reference and leveling panels toggle on theirs.
        if !cfg.hotkey_settings.is_empty() {
            extra.push(("settings".to_string(), cfg.hotkey_settings.clone()));
        }
        if !cfg.hotkey_reference.is_empty() {
            extra.push(("reference".to_string(), cfg.hotkey_reference.clone()));
        }
        if !cfg.hotkey_leveling.is_empty() {
            extra.push(("leveling".to_string(), cfg.hotkey_leveling.clone()));
        }
        let hk_tx = hk_tx.clone();
        rt.spawn(async move {
            if let Err(e) = khaloni_poe2::platform::hotkeys::listen(hk_tx, check, overlay, extra).await {
                eprintln!("hotkeys unavailable: {e}");
            }
        });
    }

    // System tray: quick actions without a hotkey. A missing tray host
    // (no StatusNotifier) is not fatal — everything works without it.
    let (tray_tx, tray_rx) = mpsc::channel();
    if let Err(e) = khaloni_poe2::tray::spawn(tray_tx) {
        eprintln!("tray unavailable: {e}");
    }

    // Hover price check: the Injector runs a uinput virtual keyboard on
    // its own dedicated thread (see inject.rs for why the injection must
    // stay on one long-lived thread). A missing /dev/uinput permission is
    // not fatal: F7 just does nothing, logged once at startup.
    phase("hotkeys spawning");
    let injector: Option<inject::Injector> = match inject::Injector::new() {
        Ok(i) => Some(i),
        Err(e) => {
            eprintln!("price check unavailable: {e}");
            None
        }
    };
    // Set true while a price check is running on the injector thread so a
    // second F7 does not queue another; reset when its result is drained.
    let price_check_in_flight = Arc::new(AtomicBool::new(false));
    let (clip_tx, clip_rx) = mpsc::channel::<anyhow::Result<String>>();
    // Copy-hovered actions that are not price checks (resource shortcuts,
    // map analysis) share one reply channel; `pending_action` says what the
    // in-flight copy was for.
    let (action_tx, action_rx) = mpsc::channel::<anyhow::Result<String>>();
    let mut pending_action: Option<PendingAction> = None;
    // Map-mod rules: built-in seed plus any config-added needles. Rebuilt
    // on config hot-reload so settings edits apply without a relaunch.
    let mut map_rules = build_map_rules(&cfg);
    // Reference data for the in-overlay panels loads (cached, fetched once)
    // on a background thread so a cold fetch never blocks startup; the
    // panels show a loading row until the OnceLock fills.
    let reference: std::sync::Arc<std::sync::OnceLock<khaloni_poe2::refcache::Reference>> =
        std::sync::Arc::new(std::sync::OnceLock::new());
    {
        let reference = reference.clone();
        std::thread::spawn(move || {
            let cache = directories::ProjectDirs::from("", "", "khaloni-poe2")
                .map(|d| d.cache_dir().to_path_buf())
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            let r = khaloni_poe2::refcache::reference_data(&cache);
            eprintln!(
                "reference data ready: {} affixes, {} items, {} uniques",
                r.affixes.len(),
                r.items.len(),
                r.uniques.len()
            );
            let _ = reference.set(r);
        });
    }
    // Trade appraisal worker: rare items parsed from the clipboard get a
    // background search+fetch against the official trade API (strictly
    // rate limited inside TradeClient); results return on this channel.
    let (appraise_tx, appraise_rx) = mpsc::channel::<AppraiseDone>();
    let (appraise_req_tx, appraise_req_rx) = mpsc::channel::<AppraiseReq>();
    // Currency-exchange results: (display name, price in exalted or None).
    let (exch_tx, exch_rx) = mpsc::channel::<(String, Option<f64>)>();
    // Specific-gem price cache, shared with the OCR pricer.
    let gem_map: GemMap = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    {
        let tx = appraise_tx.clone();
        let exch_tx = exch_tx.clone();
        let league = cfg.league.clone();
        let gem_map = gem_map.clone();
        let svc_gem = svc.clone();
        std::thread::spawn(move || {
            let stats_path = directories::ProjectDirs::from("", "", "khaloni-poe2")
                .map(|d| d.cache_dir().join("trade_stats.json"));
            let mut client = match khaloni_poe2_core::trade::TradeClient::new("https://www.pathofexile.com", &league) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("trade client unavailable: {e}");
                    return;
                }
            };
            // Stats index: disk cache first, else fetched once, cached.
            let stats_json: Option<String> = stats_path
                .as_deref()
                .and_then(|p| std::fs::read_to_string(p).ok())
                .or_else(|| {
                    let got = khaloni_poe2_core::trade::fetch_stats_json().ok()?;
                    if let Some(p) = stats_path.as_deref() {
                        if let Some(dir) = p.parent() {
                            let _ = std::fs::create_dir_all(dir);
                        }
                        let _ = std::fs::write(p, &got);
                    }
                    Some(got)
                });
            let stats = stats_json.and_then(|j| khaloni_poe2_core::trade::StatIndex::from_json(&j).ok());
            let Some(stats) = stats else {
                eprintln!("trade stats index unavailable; rare appraisal disabled");
                return;
            };
            // Currency name -> trade exchange id (for pricing omens etc. that
            // poe.ninja doesn't track). Best-effort; empty disables exchange.
            let currency_ids = client.static_currency_ids().unwrap_or_default();
            // Reverse map (trade currency id -> display name), for converting a
            // gem listing's price currency to exalted via the poe.ninja table.
            let cur_id_to_name: std::collections::HashMap<String, String> =
                currency_ids.iter().map(|(name, id)| (id.clone(), name.clone())).collect();
            // Exact gem base-type names, for resolving OCR'd skill names.
            let gem_types = client.gem_types().unwrap_or_default();
            for req in appraise_req_rx {
                // Currency exchange is priced separately from item search.
                if let AppraiseReq::Currency { name } = &req {
                    let rate = currency_ids
                        .get(&name.to_lowercase())
                        .and_then(|id| client.exchange(id, "exalted").ok().flatten());
                    let _ = exch_tx.send((name.clone(), rate));
                    continue;
                }
                // Specific cut skill gem: resolve name, item-search by level,
                // convert the cheapest listing to exalted, write the cache.
                if let AppraiseReq::Gem { skill, level } = &req {
                    let state = price_one_gem(
                        &mut client,
                        skill,
                        *level,
                        &gem_types,
                        &cur_id_to_name,
                        &svc_gem.snapshot().table,
                    );
                    if let Ok(mut m) = gem_map.lock() {
                        m.insert((skill.clone(), *level), state);
                    }
                    continue;
                }
                let (title, q, labels, relaxed) = match req {
                    AppraiseReq::Auto(item) => {
                        let title = if item.name.is_empty() {
                            item.base_type.clone().unwrap_or_default()
                        } else {
                            item.name.clone()
                        };
                        let (q, labels) =
                            khaloni_poe2_core::trade::build_query_with_labels(&item, &stats);
                        (title, q, labels, true)
                    }
                    AppraiseReq::Exact { title, query } => (title, query, Vec::new(), false),
                    AppraiseReq::Currency { .. } | AppraiseReq::Gem { .. } => continue, // handled above
                };
                let searched = if relaxed {
                    client.search_relaxed(&q).map(|(s, _kept)| s)
                } else {
                    client.search(&q)
                };
                let mut search_id = None;
                let outcome = searched.and_then(|s| {
                    search_id = Some(s.id.clone());
                    // Empty even after relaxing to the strongest mod:
                    // fetching an empty id list 404s, so report none.
                    let take = s.hashes.len().min(10);
                    if take == 0 {
                        Ok(Vec::new())
                    } else {
                        client.fetch(&s.id, &s.hashes[..take])
                    }
                });
                // Cooldown gets a human line with whole seconds instead
                // of the Debug duration ("rate limited; retry in
                // 32.847s"); other errors keep their Display text.
                let outcome = outcome.map_err(|e| match e {
                    khaloni_poe2_core::trade::TradeError::Cooldown(d) => {
                        format!("trade cooldown, retry in {}s", d.as_secs().max(1))
                    }
                    other => other.to_string(),
                });
                let _ = tx.send(AppraiseDone {
                    title,
                    outcome,
                    query: relaxed.then_some(q),
                    labels,
                    search_id,
                });
            }
        });
    }

    // Zero calibration: the reward-panel region is DETECTED on the full
    // frames (autoregion, inside the rumour worker below) and shipped to
    // the capture thread through region_tx. Until the first detection the
    // capture crops a harmless dummy corner that the OCR worker ignores
    // (region_ready gate). `scan_geom` carries (frame dims, region) to the
    // main loop for label/badge placement, replacing the old CoordMap-from-
    // calibration path and its hardcoded 3840x2160 capture assumption.
    let scan_geom: ScanGeom = std::sync::Arc::new(std::sync::Mutex::new((None, None)));
    let region_ready = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    // Capacity 1: only the latest frame is ever wanted; see capture::consume.
    let (ftx, frx) = mpsc::sync_channel(1);
    // Full-frame channel for the rumour recognizer (latest-only, capacity 1).
    let (full_tx, full_rx) = mpsc::sync_channel::<image::GrayImage>(1);
    // Recognized rumours flow back to the render loop here (every scan,
    // including empty, so stale badges clear when the panel closes).
    let (rumour_tx, rumour_rx) = mpsc::channel::<Vec<khaloni_poe2::rumours::RumourHit>>();
    let (region_tx, region_rx) = mpsc::channel::<Rect>();
    let region = Rect { x: 0, y: 0, w: 64, h: 64 };
    // Shared with the OCR worker below: it owns the BrightnessGate and
    // stores whether it's currently open here every pass; the capture
    // thread only reads it, to pick its 120ms/300ms throttle. An atomic is
    // the simplest correct way to move this one bit across the thread
    // boundary without a second channel (see capture::consume's doc comment).
    let panel_open = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let panel_open_capture = panel_open.clone();
    std::thread::spawn(move || {
        let _ = capture::consume(start, region_rx, region, ftx, panel_open_capture, Some(full_tx));
    });

    // OCR worker: frames in, priced rows out. `pipeline_paused` is toggled by the
    // main loop on focus loss / scan toggle, so we stop feeding tesseract without
    // touching the capture thread (which keeps running regardless, now that it
    // emits every throttle tick rather than only on pixel change).
    let pipeline_paused = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (rows_tx, rows_rx) = mpsc::channel();
    let svc_ocr = svc.clone();
    let ocr_cfg = cfg.clone();
    // Full-frame worker: reward-region detection + rumour recognition, one
    // thread because the full-frame channel has a single consumer. Region
    // detection is pure image math and runs even when the rumour dataset or
    // tesseract is missing; rumour recognition is best-effort on top.
    {
        let rumour_csv = Config::path().parent().map(|d| d.join("rumours.csv"));
        let paused_rumour = pipeline_paused.clone();
        let scan_geom = scan_geom.clone();
        let region_ready = region_ready.clone();
        let panel_open_det = panel_open.clone();
        std::thread::spawn(move || {
            let dbg = std::env::var("KHALONI_DEBUG").is_ok();
            // Rumour half of the worker: optional. None = detection only.
            let idx = rumour_csv
                .and_then(|p| std::fs::read_to_string(p).ok())
                .map(|csv| {
                    khaloni_poe2_core::rumour::RumourIndex::new(
                        khaloni_poe2_core::rumour::parse_csv(&csv),
                    )
                });
            let mut engine = match &idx {
                Some(idx) => match ocr::OcrEngine::new() {
                    Ok(e) => {
                        eprintln!("rumour worker: ready ({} entries)", idx.len());
                        Some(e)
                    }
                    Err(_) => {
                        eprintln!("rumour worker: tesseract init failed; rumour overlay off");
                        None
                    }
                },
                None => {
                    eprintln!("rumour worker: no rumours.csv; rumour overlay off");
                    None
                }
            };
            let mut last_region: Option<Rect> = None;
            for frame in full_rx {
                // Reward-region detection. While the brightness gate is
                // open the region is LOCKED: the stabilizer's scroll origin
                // must not move under it. Redetect only when closed.
                {
                    // Live-debug: keep the latest full frame on disk so a
                    // detection miss can be reproduced offline against the
                    // exact pixels (overwritten each ~700ms frame).
                    if std::env::var("KHALONI_REGION_DUMP").is_ok() {
                        let _ = frame.save("/tmp/khaloni-frame.png");
                    }
                    let mut geom = scan_geom.lock().unwrap();
                    geom.0 = Some((frame.width(), frame.height()));
                    if !panel_open_det.load(Ordering::Relaxed) {
                        let found = khaloni_poe2::autoregion::detect_reward_region(&frame).map(|r| Rect {
                            x: r.x0 as i32,
                            y: r.y0 as i32,
                            w: r.x1 - r.x0,
                            h: r.y1 - r.y0,
                        });
                        if dbg && found != last_region {
                            eprintln!("auto-region: {found:?}");
                        }
                        if let Some(r) = found {
                            if last_region != Some(r) {
                                let _ = region_tx.send(r);
                                last_region = Some(r);
                            }
                            geom.1 = Some(r);
                            region_ready.store(true, Ordering::Relaxed);
                        }
                        // A vanished panel keeps the last region: the gate
                        // is closed anyway, and reusing it makes reopening
                        // in the same spot (the common case) instant.
                    }
                }
                if paused_rumour.load(std::sync::atomic::Ordering::Relaxed) {
                    continue;
                }
                let (Some(idx), Some(engine)) = (&idx, engine.as_mut()) else {
                    continue;
                };
                let t = std::time::Instant::now();
                // Debug: dump the exact frame a panel was seen in, so live
                // misses can be analyzed offline at the true capture resolution.
                if std::env::var("KHALONI_RUMOUR_DUMP").is_ok()
                    && khaloni_poe2::rumours::find_panel(&frame).is_some()
                {
                    let _ = frame.save("/tmp/poe2-live-frame.png");
                }
                let hits = khaloni_poe2::rumours::recognize(engine, &frame, idx);
                if !hits.is_empty() {
                    eprintln!(
                        "RUMOURS {} in {}ms: {}",
                        hits.len(),
                        t.elapsed().as_millis(),
                        hits.iter()
                            .map(|h| format!(
                                "{} [{}] @({},{})",
                                h.entry.rumour, h.entry.rating, h.line.x0, h.line.y0
                            ))
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                } else if dbg {
                    eprintln!(
                        "rumour scan: {}x{} none in {}ms",
                        frame.width(),
                        frame.height(),
                        t.elapsed().as_millis()
                    );
                }
                // Always forward (even empty) so the render loop clears
                // badges the instant the tooltip leaves the screen.
                if rumour_tx.send(hits).is_err() {
                    break; // main loop gone
                }
            }
        });
    }

    let paused_ocr = pipeline_paused.clone();
    // The reward-panel pricer's handle to the specific-gem cache + trade worker.
    let gem_cache = GemCache { map: gem_map.clone(), req_tx: appraise_req_tx.clone() };
    let region_ready_ocr = region_ready.clone();
    std::thread::spawn(move || {
        let dbg = std::env::var("KHALONI_DEBUG").is_ok();
        let t0 = std::time::Instant::now();
        let Ok(mut engine) = ocr::OcrEngine::new() else {
            eprintln!("tesseract init failed; OCR disabled");
            return;
        };
        let mut last_profile: Option<Vec<u16>> = None;
        let mut post_scroll_fast = false;
        // Tesseract cadence floor: at 16ms capture the per-frame work is
        // profile+templates only; the expensive OCR paths keep the old
        // 120ms rhythm regardless of capture rate.
        let mut last_heavy = std::time::Instant::now() - Duration::from_secs(1);
        let mut gate = khaloni_poe2::brightness::BrightnessGate::new(
            ocr_cfg.panel_open_brightness,
            ocr_cfg.panel_close_brightness,
        );
        // Learned-template store: identifies previously seen reward bands
        // in well under a millisecond, bypassing tesseract; OCR remains
        // the teacher for first encounters. Persisted across sessions.
        // Rumour annotations: optional dataset at config_dir/rumours.csv
        // (community sheet snapshot). Absent file = feature off; rumour
        // lines then render nothing, exactly as before the wiring.
        let rumours = Config::path()
            .parent()
            .map(|d| d.join("rumours.csv"))
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|csv| {
                let idx = khaloni_poe2_core::rumour::RumourIndex::new(
                    khaloni_poe2_core::rumour::parse_csv(&csv),
                );
                eprintln!("rumour dataset loaded: {} entries", idx.len());
                idx
            });
        if rumours.is_none() {
            eprintln!("no rumours.csv in config dir; rumour annotations off");
        }
        let tpl_path = directories::ProjectDirs::from("", "", "khaloni-poe2")
            .map(|d| d.cache_dir().join("templates.bin"));
        let mut tstore = tpl_path
            .as_deref()
            .map(khaloni_poe2::template::TemplateStore::load)
            .unwrap_or_default();
        let mut tpl_saved_at = std::time::Instant::now();
        // The frame channel is capacity-1 with try_send-and-drop on the
        // capture side (see capture::consume), so a backlog here is
        // structurally impossible: this is always the latest frame, and a
        // plain blocking recv (via the Receiver iterator) is enough.
        for frame in frx {
            // Until the detector has found a reward region, capture is
            // cropping the startup dummy rect — never OCR that.
            if !region_ready_ocr.load(std::sync::atomic::Ordering::Relaxed) {
                continue;
            }
            let mean = mean_gray_brightness(&frame.gray);
            if dbg {
                eprintln!("DBG ocr-worker: frame {}x{} mean_brightness={mean}", frame.gray.width(), frame.gray.height());
            }
            if paused_ocr.load(std::sync::atomic::Ordering::Relaxed) {
                // Drop the frame cheaply; no OCR/pricing work while paused.
                continue;
            }
            let t_frame = std::time::Instant::now();
            let open = gate.observe(mean);
            panel_open.store(open, std::sync::atomic::Ordering::Relaxed);
            if dbg {
                eprintln!("TRACE {:>8.2}s mean={mean} gate_open={open}", t0.elapsed().as_secs_f32());
            }
            if !open {
                // Gate closed: too dark to be the parchment panel (game
                // world, not the list). Skip tesseract entirely (this check
                // costs microseconds) and report gated-empty so the overlay
                // can drop stale rows instead of holding them.
                let _ = rows_tx.send(khaloni_poe2::stabilize::ScanResult::GateEmpty);
                continue;
            }
            let profile = ocr::row_profile(&frame.gray);
            let motion = match last_profile.replace(profile.clone()) {
                Some(prev) => ocr::track_motion(&prev, &profile),
                None => ocr::Motion::Still,
            };
            match motion {
                ocr::Motion::Scrolled(dy) => {
                    // Content is scrolling: move labels instantly and
                    // skip OCR (mid-scroll frames are motion blur);
                    // the next stable frame rescans normally.
                    let dy_pre = i64::from(dy) * i64::from(ocr::UPSCALE);
                    post_scroll_fast = true;
                    let _ = rows_tx.send(khaloni_poe2::stabilize::ScanResult::Scrolled(dy_pre));
                    if dbg {
                        eprintln!("TRACE {:>8.2}s scroll dy={dy}", t0.elapsed().as_secs_f32());
                    }
                    continue;
                }
                ocr::Motion::Lost => {
                    // Flick faster than correlation can follow, or a
                    // panel-scale change mid-scroll: the stabilizer
                    // hides rather than showing prices on rows they no
                    // longer belong to. Skip OCR on this frame (it is
                    // blur/transition); the next Still frame re-anchors
                    // everything from scratch.
                    post_scroll_fast = true;
                    let _ = rows_tx.send(khaloni_poe2::stabilize::ScanResult::TrackingLost);
                    if dbg {
                        eprintln!("TRACE {:>8.2}s tracking lost", t0.elapsed().as_secs_f32());
                    }
                    continue;
                }
                ocr::Motion::Still => {}
            }
            let bands = ocr::detect_bands_from_profile(&profile);
            if dbg {
                eprintln!("TRACE {:>8.2}s bands={}", t0.elapsed().as_secs_f32(), bands.len());
            }
            // Fast-close: a band-less frame IS the close signal; skip all
            // OCR (band detection costs ~2 ms) so the hide confirmation
            // arrives at capture cadence, not OCR cadence. Live-verified:
            // 114/116 panel scans banded (no under-threshold panel seen);
            // if a panel style ever defeats band detection, this is the
            // line to revisit.
            if bands.is_empty() {
                let _ = rows_tx.send(khaloni_poe2::stabilize::ScanResult::NoBands);
                continue;
            }
            // Template pass first: every band already learned resolves in
            // ~0.7 ms (measured on the live corpus) with no tesseract.
            let snap = svc_ocr.snapshot();
            let mut resolved: Vec<pricing::Priced> = Vec::new();
            let mut any_unresolved = false;
            for &(y0, y1) in &bands {
                let row = ocr::band_crop(&frame.gray, y0, y1)
                    .and_then(|crop| {
                        tstore.match_band(&crop).map(|(hit, _)| {
                            (hit.item_key.clone(), hit.count, hit.count_explicit)
                        })
                    })
                    .and_then(|(key, count, explicit)| {
                        pricing::price_resolved(
                            &snap.table,
                            &key,
                            count,
                            explicit,
                            y0 * ocr::UPSCALE,
                            (y1 - y0) * ocr::UPSCALE,
                            &ocr_cfg,
                        )
                    });
                match row {
                    Some(r) => resolved.push(r),
                    None => any_unresolved = true,
                }
            }
            if !any_unresolved && !resolved.is_empty() {
                if dbg {
                    eprintln!(
                        "TRACE {:>8.2}s tpl_done in {:?}: {} rows",
                        t0.elapsed().as_secs_f32(),
                        t_frame.elapsed(),
                        resolved.len()
                    );
                }
                let _ = rows_tx.send(khaloni_poe2::stabilize::ScanResult::Rows(resolved, snap.stale));
                continue;
            }
            // Unresolved bands wait for the next tesseract slot (120ms
            // rhythm); nothing is sent for a gated frame, so slot
            // miss-counting does not advance and the next slot's scan
            // sees a fresher frame anyway.
            if last_heavy.elapsed() < Duration::from_millis(120) {
                continue;
            }
            last_heavy = std::time::Instant::now();
            // First scan after a scroll burst: bands only, no whole-panel
            // union pass, so newly revealed rows appear ~3x sooner; the
            // union tops up on the following scan.
            let lines = if std::mem::take(&mut post_scroll_fast) {
                ocr::ocr_bands(&mut engine, &frame.gray, &bands)
            } else {
                ocr::ocr_scan(&mut engine, &frame.gray)
            };
            if dbg {
                let d = std::path::Path::new("/tmp/khalonipoe2-frames");
                let _ = std::fs::create_dir_all(d);
                let _ = frame.gray.save(d.join(format!(
                    "t{:06.2}_bands{}_lines{}.png",
                    t0.elapsed().as_secs_f32(),
                    bands.len(),
                    lines.len()
                )));
            }
            let out = pricing::price_lines_with_rumours(
                &snap.table,
                &snap.vocab,
                &lines,
                &ocr_cfg,
                rumours.as_ref(),
                Some(&gem_cache),
            );
            // Teach the template store from confidently identified OCR
            // rows aligned to a band (OCR-taught templates then take over
            // for every later encounter of the same reward).
            for r in &out.0 {
                if !r.locks_in_one
                    || r.item_key == "unpriceable"
                    || r.item_key == "ambiguous"
                    || r.item_key.starts_with("gem-unleveled")
                    // Specific gems are priced asynchronously via trade and
                    // must re-OCR each scan to pick up the arriving price, so
                    // they are never templated (a template would freeze the
                    // provisional "…" or an early price).
                    || r.item_key.starts_with("gemx:")
                {
                    continue;
                }
                if let Some(&(y0, y1)) = bands
                    .iter()
                    .find(|&&(y0, _)| y0 * ocr::UPSCALE == r.y_top)
                {
                    if let Some(crop) = ocr::band_crop(&frame.gray, y0, y1) {
                        tstore.learn(&r.item_key, r.count, r.count_explicit, &crop);
                    }
                }
            }
            if tstore.dirty && tpl_saved_at.elapsed().as_secs() >= 30 {
                if let Some(p) = tpl_path.as_deref() {
                    let _ = tstore.save(p);
                }
                tpl_saved_at = std::time::Instant::now();
            }
            // Merge template-resolved rows with the OCR pass: a resolved
            // row wins over any OCR row overlapping its y range.
            let mut merged = resolved;
            for r in out.0 {
                let clash = merged.iter().any(|m| {
                    let (a0, a1) = (i64::from(m.y_top), i64::from(m.y_top) + i64::from(m.height));
                    let (b0, b1) = (i64::from(r.y_top), i64::from(r.y_top) + i64::from(r.height));
                    a0.max(b0) < a1.min(b1)
                });
                if !clash {
                    merged.push(r);
                }
            }
            merged.sort_by_key(|r| r.y_top);
            let out = (merged, out.1);
            // Bands were present but nothing priced (tooltip occlusion,
            // mid-transition frame): plain empty Rows, which the
            // stabilizer rides out with its occlusion tolerance. The
            // band-less case already exited above.
            if dbg {
                eprintln!(
                    "TRACE {:>8.2}s ocr_done in {:?}: {} lines -> {} rows [{}]",
                    t0.elapsed().as_secs_f32(),
                    t_frame.elapsed(),
                    lines.len(),
                    out.0.len(),
                    out.0.iter().map(|r| format!("{}@y{}", r.item_key, r.y_top)).collect::<Vec<_>>().join(", ")
                );
            }
            let _ = rows_tx.send(khaloni_poe2::stabilize::ScanResult::Rows(out.0, snap.stale));
        }
    });

    let center = (game.x + game.w as i32 / 2, game.y + game.h as i32 / 2);
    let mut overlay = khaloni_poe2::platform::overlay::Overlay::new(center)?;
    phase("overlay surface up");
    let mut first_present_logged = false;
    // Overlay opacity live-applies from config; a change must force a
    // repaint because an idle overlay keeps its last presented buffer.
    let mut last_opacity = f64::NAN;
    let renderer = khaloni_poe2::render::Renderer::new()?;

    let mut scanning = true;
    let mut game_focused = true;
    // On-screen state from the tracker (minimized/covered detection);
    // optimistic until the first Visible event arrives.
    let mut game_visible = true;
    let mut game_present = true;
    let mut stabilizer = khaloni_poe2::stabilize::Stabilizer::new();
    let mut hover = hover::HoverState::default();
    let mut game_pos = (game.x, game.y);
    // Live pointer position (global logical), fed by the KWin script's
    // cursor timer. Falls back to the game center until the first move.
    let mut cursor_pos = center;
    // Where the cursor was when the current popup fired, and the placed
    // popup rect: move-away dismissal measures against these. None while
    // no popup is up.
    let mut popup_at: Option<((i32, i32), Rect)> = None;
    // Interactive appraisal panel: model + the query its checkboxes edit
    // + placed top-left (global logical). While Some, the overlay's input
    // region covers the panel and clicks resolve through appraise_ui.
    let mut apanel: Option<(
        khaloni_poe2::appraise_ui::Panel,
        khaloni_poe2_core::trade::Query,
        (i32, i32),
    )> = None;
    // Which filter box is being typed into, and the digits typed so far.
    let mut editing: Option<(usize, khaloni_poe2::appraise_ui::Field)> = None;
    let mut edit_buf = String::new();
    // In-overlay reference search panel (F9) and leveling checklist (F10),
    // each with its placed top-left in global logical coordinates. While
    // open they join the overlay's input region and take keyboard focus
    // for search typing / scrolling.
    let mut ref_panel: Option<(khaloni_poe2::reference_ui::Panel, (i32, i32))> = None;
    let mut lvl_panel: Option<(khaloni_poe2::leveling_ui::Panel, (i32, i32))> = None;
    // An in-progress panel drag: (grab point in surface px, panel's global
    // position when the grab began). Deliberately NOT persisted anywhere, so
    // each new price check reopens the panel at its freshly-placed spot.
    let mut panel_drag: Option<((i32, i32), (i32, i32))> = None;
    let mut pixmap: Option<tiny_skia::Pixmap> = None;
    // What was actually drawn+presented last tick: `Some((placed, stale,
    // popup))` while visible, `None` while hidden/blank. Compared each tick
    // so an unchanged stabilized row set (the common case at 10 ticks/sec,
    // since OCR scans land far less often) skips both the redraw and the
    // Wayland present entirely instead of repainting identical content
    // every 100ms. The popup slot is part of the same equality so its 6s
    // expiry (which changes nothing else about the frame) still forces the
    // repaint that clears it.
    let mut last_frame: Option<FrameState> = None;
    // Latest rumours from the recognizer worker (capture-physical px boxes).
    let mut latest_rumours: Vec<khaloni_poe2::rumours::RumourHit> = Vec::new();
    let dbg = std::env::var("KHALONI_DEBUG").is_ok();
    // Live config reload: the web control panel writes config.toml; polling its
    // mtime (once a second) lets main-loop-read settings (pause-when-unfocused,
    // divine threshold) take effect without a relaunch. Worker-thread settings
    // still need a restart (they hold clones), as the panel notes.
    let mut cfg_mtime = std::fs::metadata(Config::path()).and_then(|m| m.modified()).ok();
    let mut last_cfg_poll = std::time::Instant::now();

    loop {
        overlay.pump()?;

        // Exact != on purpose: both sides come from the same config value,
        // and the NAN sentinel compares unequal to everything, so the first
        // tick always applies (a subtraction-epsilon test is always-false
        // against NAN and would never fire — the bug this replaces).
        if cfg.overlay_opacity != last_opacity {
            last_opacity = cfg.overlay_opacity;
            // Same 10% floor as the settings slider, so a hand-edited config
            // cannot make the overlay silently invisible either.
            overlay.set_opacity(cfg.overlay_opacity.max(0.1));
            last_frame = None;
        }
        if last_cfg_poll.elapsed() >= Duration::from_secs(1) {
            last_cfg_poll = std::time::Instant::now();
            if let Ok(m) = std::fs::metadata(Config::path()).and_then(|md| md.modified()) {
                if cfg_mtime != Some(m) {
                    cfg_mtime = Some(m);
                    if let Ok(new_cfg) = Config::load() {
                        cfg = new_cfg;
                        map_rules = build_map_rules(&cfg);
                    }
                }
            }
        }

        // Latest rumour scan wins; empty vec clears badges when the panel closes.
        while let Ok(r) = rumour_rx.try_recv() {
            latest_rumours = r;
        }

        while let Ok(ev) = kwin.rx.try_recv() {
            match ev {
                khaloni_poe2::platform::GameWindowEvent::Geometry(g) => {
                    // The scan region is capture-space and auto-detected, so
                    // a window move needs no region update; label placement
                    // reads the live game position every paint.
                    game_pos = (g.x, g.y);
                    game = g;
                    game_present = true;
                }
                khaloni_poe2::platform::GameWindowEvent::Active(is_game) => game_focused = is_game,
                khaloni_poe2::platform::GameWindowEvent::Visible(v) => game_visible = v,
                khaloni_poe2::platform::GameWindowEvent::GameGone => {
                    stabilizer.clear();
                    game_present = false;
                    let any_panel = apanel.take().is_some()
                        | ref_panel.take().is_some()
                        | lvl_panel.take().is_some();
                    if any_panel {
                        overlay.set_keyboard(false)?;
                        overlay.set_interactive(None)?;
                    }
                    overlay.hide()?;
                }
                khaloni_poe2::platform::GameWindowEvent::Cursor(x, y) => cursor_pos = (x, y),
            }
        }
        // Tray menu actions reuse the hotkey paths where one exists, so the
        // two entry points cannot drift apart.
        while let Ok(ev) = tray_rx.try_recv() {
            match ev {
                khaloni_poe2::tray::TrayEvent::OpenSettings => open_settings(),
                khaloni_poe2::tray::TrayEvent::ToggleOverlay => {
                    let _ = hk_tx.send(khaloni_poe2::platform::Hotkey::OverlayToggle);
                }
                khaloni_poe2::tray::TrayEvent::TogglePause => {
                    let v = !pipeline_paused.load(Ordering::Relaxed);
                    pipeline_paused.store(v, Ordering::Relaxed);
                    hover.show_note(if v { "pricing paused" } else { "pricing resumed" });
                }
                khaloni_poe2::tray::TrayEvent::Quit => return Ok(()),
            }
        }
        while let Ok(hk) = hk_rx.try_recv() {
            match hk {
                khaloni_poe2::platform::Hotkey::OverlayToggle => {
                    scanning = !scanning;
                    if !scanning {
                        stabilizer.clear();
                    }
                    // No forced rescan needed either way: capture emits a
                    // frame on every throttle tick regardless of pause
                    // state, so toggling back on picks up the next one
                    // within one tick on its own.
                    eprintln!("overlay toggled {}", if scanning { "on" } else { "off" });
                    let any_panel = apanel.take().is_some()
                        | ref_panel.take().is_some()
                        | lvl_panel.take().is_some();
                    if any_panel {
                        editing = None;
                        overlay.set_keyboard(false)?;
                        overlay.set_interactive(None)?;
                    }
                    hover.show_note(if scanning { "overlay on" } else { "overlay off" });
                    let game_rect =
                        Rect { x: game_pos.0, y: game_pos.1, w: game.w, h: game.h };
                    popup_at = hover.current.as_ref().map(|p| {
                        let size = khaloni_poe2::render::Renderer::popup_size(p);
                        let (px, py) =
                            khaloni_poe2::popup_pos::place(cursor_pos, size, game_rect);
                        (cursor_pos, Rect { x: px, y: py, w: size.0 as u32, h: size.1 as u32 })
                    });
                }
                khaloni_poe2::platform::Hotkey::PriceCheck => {
                    if let Some(inj) = &injector {
                        // game_focused-gated so a press over some other
                        // window never sends Ctrl+C into it; the swap keeps
                        // a second press from queueing another copy while
                        // one is running on the injector thread.
                        if game_focused && !price_check_in_flight.swap(true, Ordering::AcqRel) {
                            inj.submit(clip_tx.clone(), 0);
                        }
                    }
                }
                khaloni_poe2::platform::Hotkey::Extra(id) => {
                    // The settings hotkey opens the native settings window in
                    // its own process. No focus gate: it's an out-of-game
                    // window; config changes flow back via the mtime watcher.
                    if id == "settings" {
                        open_settings();
                        continue;
                    }
                    // Reference and leveling panels toggle without a focus
                    // gate: consulting them from the game menus is the point.
                    if id == "reference" {
                        if ref_panel.take().is_none() {
                            let mut p = khaloni_poe2::reference_ui::Panel::default();
                            if let Some(r) = reference.get() {
                                khaloni_poe2::reference_ui::refresh(&mut p, r);
                            }
                            let pos = (game_pos.0 + (game.w as i32) / 2 - 300, game_pos.1 + 140);
                            ref_panel = Some((p, pos));
                        }
                        overlay.set_keyboard(
                            editing.is_some() || ref_panel.is_some() || lvl_panel.is_some(),
                        )?;
                        sync_input_region(&mut overlay, &renderer, &apanel, &ref_panel, &lvl_panel)?;
                        continue;
                    }
                    if id == "leveling" {
                        if lvl_panel.take().is_none() {
                            let acts = reference.get().map(|r| r.leveling.clone()).unwrap_or_default();
                            let done = Config::path()
                                .parent()
                                .map(khaloni_poe2::leveling_ui::load_done)
                                .unwrap_or_default();
                            let p = khaloni_poe2::leveling_ui::Panel { acts, act: 0, done, scroll: 0 };
                            let pos = (game_pos.0 + (game.w as i32) / 2 + 40, game_pos.1 + 140);
                            lvl_panel = Some((p, pos));
                        }
                        overlay.set_keyboard(
                            editing.is_some() || ref_panel.is_some() || lvl_panel.is_some(),
                        )?;
                        sync_input_region(&mut overlay, &renderer, &apanel, &ref_panel, &lvl_panel)?;
                        continue;
                    }
                    // Only act while the game is focused, never into another
                    // window. "macro-N" types a chat message; "url-N" copies
                    // the hovered item and opens it in a browser.
                    if !game_focused {
                        continue;
                    }
                    if let Some(i) = id.strip_prefix("macro-").and_then(|n| n.parse::<usize>().ok()) {
                        if let (Some(inj), Some(m)) = (&injector, cfg.macros.get(i)) {
                            inj.type_text(m.message.clone(), cfg.macro_open_delay_ms);
                        }
                    } else if let Some(i) =
                        id.strip_prefix("url-").and_then(|n| n.parse::<usize>().ok())
                    {
                        if let Some(inj) = &injector {
                            if i < cfg.resource_shortcuts.len() && pending_action.is_none() {
                                pending_action = Some(PendingAction::Shortcut(i));
                                inj.submit(action_tx.clone(), 300);
                            }
                        }
                    }
                }
            }
        }

        // Drain copy-hovered action results (resource shortcuts, map analysis).
        while let Ok(result) = action_rx.try_recv() {
            let action = pending_action.take();
            if let (Some(PendingAction::Shortcut(i)), Ok(text)) = (action, result) {
                if let Some(sc) = cfg.resource_shortcuts.get(i) {
                    open_resource(&sc.url, &text);
                }
            }
        }

        // Drain injected clipboard text: reprice against whatever the price
        // table looks like right now (not at the moment F7 was pressed).
        let game_rect = Rect { x: game_pos.0, y: game_pos.1, w: game.w, h: game.h };
        while let Ok(result) = clip_rx.try_recv() {
            price_check_in_flight.store(false, Ordering::Release);
            match result {
                Ok(text) if text.trim().is_empty() => {
                    hover.show_no_item();
                }
                Ok(text) => {
                    let snap = svc.snapshot();
                    hover.trigger(&text, &snap.table, &snap.uniques, cfg.divine_threshold);
                    // Waystone hovered: flag dangerous and rewarding mods in
                    // the overlay popup itself (a desktop notification is
                    // invisible over a fullscreen game). No clipboard write
                    // here so F7's copy is never clobbered.
                    if text.to_lowercase().contains("waystone") {
                        let lines: Vec<&str> = text.lines().collect();
                        let classified = khaloni_poe2_core::mapmods::analyze(&lines, &map_rules);
                        let mut mod_lines: Vec<hover::PopupLine> = Vec::new();
                        for (l, k) in classified {
                            let prefix = match k {
                                khaloni_poe2_core::mapmods::ModKind::Danger => "!! ",
                                khaloni_poe2_core::mapmods::ModKind::Good => "+ ",
                            };
                            mod_lines.push(hover::PopupLine {
                                text: format!("{prefix}{l}"),
                                denom: khaloni_poe2::pricing::Denom::None,
                            });
                        }
                        if !mod_lines.is_empty() {
                            if let Some(p) = &mut hover.current {
                                p.lines.extend(mod_lines);
                            } else {
                                hover.current = Some(hover::Popup {
                                    title: "waystone mods".into(),
                                    lines: mod_lines,
                                    expires: std::time::Instant::now()
                                        + std::time::Duration::from_secs(8),
                                });
                            }
                        }
                    }
                    if let Some(item) = hover.pending_appraisal.take() {
                        // A fresh check replaces any open panel.
                        if apanel.take().is_some() {
                            overlay.set_interactive(None)?;
                        }
                        let _ = appraise_req_tx.send(AppraiseReq::Auto(item));
                    }
                    if let Some(name) = hover.pending_currency.take() {
                        let _ = appraise_req_tx.send(AppraiseReq::Currency { name });
                    }
                }
                Err(e) => eprintln!("price check: {e}"),
            }
            // A fresh popup anchors at the cursor that triggered it.
            popup_at = hover.current.as_ref().map(|p| {
                let size = khaloni_poe2::render::Renderer::popup_size(p);
                let (px, py) = khaloni_poe2::popup_pos::place(cursor_pos, size, game_rect);
                (cursor_pos, Rect { x: px, y: py, w: size.0 as u32, h: size.1 as u32 })
            });
        }
        // Currency-exchange results replace the "checking exchange..." popup
        // in place (the anchor from the F7 press still applies).
        while let Ok((name, rate)) = exch_rx.try_recv() {
            hover.show_exchange(&name, rate);
        }
        while let Ok(done) = appraise_rx.try_recv() {
            let listings_of = |outcome: &Result<Vec<khaloni_poe2_core::trade::Listing>, String>| match outcome {
                Ok(ls) if ls.is_empty() => (vec![], "no online matches".to_string()),
                Ok(ls) => (
                    ls.iter()
                        .take(8)
                        .map(|l| format!("{} {} ({})", l.price_amount, l.price_currency, l.account))
                        .collect(),
                    format!("{} shown", ls.len().min(8)),
                ),
                Err(e) => (vec![], e.clone()),
            };
            match (done.query, apanel.as_mut()) {
                // Auto response: seed the interactive panel where the
                // "searching trade..." popup was anchored.
                (Some(query), _) => {
                    let (listings, status) = listings_of(&done.outcome);
                    let mut mods: Vec<khaloni_poe2::appraise_ui::ModRow> = done
                        .labels
                        .iter()
                        .enumerate()
                        .map(|(i, l)| khaloni_poe2::appraise_ui::ModRow {
                            label: l.text.clone(),
                            tier: l.tier,
                            min: query.filters[i].value.min,
                            max: query.filters[i].value.max,
                            enabled: !query.filters[i].disabled,
                            filter_index: i,
                            tag: l.tag.to_string(),
                        })
                        .collect();
                    // Group implicits first, then explicits, then map (EE2 order).
                    mods.sort_by_key(|m| khaloni_poe2::appraise_ui::tag_rank(&m.tag));
                    // Gear carries a base-type toggle so the user can search
                    // mods-only; items priced by their base (waystones, whose
                    // category is None) get no toggle.
                    let base = query.category.as_deref().map(|c| {
                        khaloni_poe2::appraise_ui::BaseToggle {
                            label: format!("Base: {}", pretty_category(c)),
                            enabled: query.category_enabled,
                        }
                    });
                    let panel = khaloni_poe2::appraise_ui::Panel {
                        title: done.title,
                        base,
                        mods,
                        listings,
                        status,
                        search_id: done.search_id,
                    };
                    let origin = popup_at.map(|(o, _)| o).unwrap_or(cursor_pos);
                    let lay = khaloni_poe2::appraise_ui::layout(&panel, &|s| {
                        renderer.appraisal_label_width(s)
                    });
                    let pos = khaloni_poe2::popup_pos::place(origin, lay.size, game_rect);
                    hover.current = None;
                    popup_at = None;
                    let out_pos = overlay.output_pos();
                    overlay.set_interactive(Some((
                        pos.0 - out_pos.0,
                        pos.1 - out_pos.1,
                        lay.size.0 as u32,
                        lay.size.1 as u32,
                    )))?;
                    // Fresh check: forget any earlier drag so the panel opens
                    // at its placed position, never where it was last dragged.
                    panel_drag = None;
                    editing = None;
                    overlay.set_keyboard(false)?;
                    apanel = Some((panel, query, pos));
                }
                // Exact response: update the open panel in place.
                (None, Some((panel, _, _))) if panel.title == done.title => {
                    let (listings, status) = listings_of(&done.outcome);
                    panel.listings = listings;
                    panel.status = status;
                    if done.search_id.is_some() {
                        panel.search_id = done.search_id;
                    }
                }
                // Panel was closed while the search ran: drop the result.
                (None, _) => {}
            }
        }
        // Panel clicks: geometry from the same layout the renderer drew.
        // The appraisal panel gets first claim on each click (preserving its
        // drag-grab semantics); clicks outside it spill into
        // `leftover_clicks` for the reference/leveling panels below.
        let out_pos = overlay.output_pos();
        let (sw, sh) = overlay.size();
        let mut leftover_clicks: Vec<(i32, i32)> = Vec::new();
        if apanel.is_some() {
            for (cx, cy) in overlay.take_clicks() {
                let Some((panel, query, pos)) = apanel.as_mut() else {
                    leftover_clicks.push((cx, cy));
                    continue;
                };
                let lay = khaloni_poe2::appraise_ui::layout(panel, &|s| renderer.appraisal_label_width(s));
                let local = (cx - (pos.0 - out_pos.0), cy - (pos.1 - out_pos.1));
                let inside = local.0 >= 0
                    && local.0 < lay.size.0
                    && local.1 >= 0
                    && local.1 < lay.size.1;
                if !inside {
                    leftover_clicks.push((cx, cy));
                    continue;
                }
                match khaloni_poe2::appraise_ui::hit(panel, &lay, local.0, local.1) {
                    Some(khaloni_poe2::appraise_ui::Action::ToggleMod(fi)) => {
                        if let Some(f) = query.filters.get_mut(fi) {
                            f.disabled = !f.disabled;
                        }
                        if let Some(m) = panel.mods.iter_mut().find(|m| m.filter_index == fi) {
                            m.enabled = !m.enabled;
                        }
                    }
                    // Dropping the base searches the mods across every base.
                    Some(khaloni_poe2::appraise_ui::Action::ToggleBase) => {
                        query.category_enabled = !query.category_enabled;
                        if let Some(b) = panel.base.as_mut() {
                            b.enabled = query.category_enabled;
                        }
                    }
                    // Clicking a value box focuses it for keyboard entry.
                    Some(khaloni_poe2::appraise_ui::Action::Edit(fi, field)) => {
                        editing = Some((fi, field));
                        edit_buf.clear();
                        overlay.set_keyboard(true)?;
                    }
                    Some(khaloni_poe2::appraise_ui::Action::Search) => {
                        panel.status = "searching...".into();
                        let _ = appraise_req_tx.send(AppraiseReq::Exact {
                            title: panel.title.clone(),
                            query: query.clone(),
                        });
                    }
                    Some(khaloni_poe2::appraise_ui::Action::OpenSite) => {
                        // Feedback in the status line, since opening the browser
                        // gives no in-overlay cue on its own.
                        match &panel.search_id {
                            Some(id) => {
                                let url = format!(
                                    "https://www.pathofexile.com/trade2/search/poe2/{}/{}",
                                    cfg.league.replace(' ', "%20"),
                                    id
                                );
                                open_url(&url);
                                panel.status = "opened in browser".into();
                            }
                            None => panel.status = "run a search first".into(),
                        }
                    }
                    Some(khaloni_poe2::appraise_ui::Action::Close) => {
                        apanel = None;
                        editing = None;
                        panel_drag = None;
                        overlay.set_keyboard(ref_panel.is_some() || lvl_panel.is_some())?;
                        sync_input_region(&mut overlay, &renderer, &apanel, &ref_panel, &lvl_panel)?;
                    }
                    // A press on a non-interactive part of the panel (title
                    // bar, gaps between controls) grabs it for dragging. Widen
                    // the input region to the whole surface so motion keeps
                    // arriving even as the panel slides out from under the
                    // cursor; the region is settled back on release.
                    // Inside the panel but on no control: grab for dragging
                    // (bounds were checked before the hit test).
                    None => {
                        panel_drag = Some(((cx, cy), *pos));
                        overlay.set_interactive(Some((0, 0, sw, sh)))?;
                    }
                }
            }
            // Advance or finish an in-progress drag.
            if let Some((grab, orig)) = panel_drag {
                if overlay.button_down() {
                    let (px, py) = overlay.pointer_pos();
                    if let Some((_, _, pos)) = apanel.as_mut() {
                        *pos = (orig.0 + (px - grab.0), orig.1 + (py - grab.1));
                    }
                } else {
                    panel_drag = None;
                    sync_input_region(&mut overlay, &renderer, &apanel, &ref_panel, &lvl_panel)?;
                }
            }
        } else {
            leftover_clicks = overlay.take_clicks();
        }
        // Reference/leveling panel clicks: whatever the appraisal panel did
        // not claim, in priority order reference then leveling.
        for (cx, cy) in leftover_clicks {
            if let Some((p, pos)) = ref_panel.as_mut() {
                let lay = khaloni_poe2::reference_ui::layout(p, &|s| renderer.appraisal_label_width(s));
                let local = (cx - (pos.0 - out_pos.0), cy - (pos.1 - out_pos.1));
                if local.0 >= 0 && local.0 < lay.w && local.1 >= 0 && local.1 < lay.h {
                    match khaloni_poe2::reference_ui::hit(p, &lay, local.0, local.1) {
                        Some(khaloni_poe2::reference_ui::Action::Close) => {
                            ref_panel = None;
                            overlay.set_keyboard(editing.is_some() || lvl_panel.is_some())?;
                            sync_input_region(&mut overlay, &renderer, &apanel, &ref_panel, &lvl_panel)?;
                        }
                        Some(khaloni_poe2::reference_ui::Action::FocusSearch) => p.focused = true,
                        Some(khaloni_poe2::reference_ui::Action::SetCat(c)) => {
                            p.cat = c;
                            if let Some(r) = reference.get() {
                                khaloni_poe2::reference_ui::refresh(p, r);
                            }
                            // Result width can change with the category.
                            sync_input_region(&mut overlay, &renderer, &apanel, &ref_panel, &lvl_panel)?;
                        }
                        Some(khaloni_poe2::reference_ui::Action::ScrollUp) => {
                            p.scroll = p.scroll.saturating_sub(1);
                        }
                        Some(khaloni_poe2::reference_ui::Action::ScrollDown) => {
                            p.scroll = (p.scroll + 1).min(p.rows.len().saturating_sub(1));
                        }
                        None => {}
                    }
                    continue;
                }
            }
            if let Some((p, pos)) = lvl_panel.as_mut() {
                let lay = khaloni_poe2::leveling_ui::layout(p, &|s| renderer.appraisal_label_width(s));
                let local = (cx - (pos.0 - out_pos.0), cy - (pos.1 - out_pos.1));
                if local.0 >= 0 && local.0 < lay.w && local.1 >= 0 && local.1 < lay.h {
                    match khaloni_poe2::leveling_ui::hit(p, &lay, local.0, local.1) {
                        Some(khaloni_poe2::leveling_ui::Action::Close) => {
                            lvl_panel = None;
                            overlay.set_keyboard(editing.is_some() || ref_panel.is_some())?;
                            sync_input_region(&mut overlay, &renderer, &apanel, &ref_panel, &lvl_panel)?;
                        }
                        Some(khaloni_poe2::leveling_ui::Action::PrevAct) => {
                            p.act = p.act.saturating_sub(1);
                            p.scroll = 0;
                        }
                        Some(khaloni_poe2::leveling_ui::Action::NextAct) => {
                            if p.act + 1 < p.acts.len() {
                                p.act += 1;
                                p.scroll = 0;
                            }
                        }
                        Some(khaloni_poe2::leveling_ui::Action::ToggleStep(id)) => {
                            if !p.done.remove(&id) {
                                p.done.insert(id);
                            }
                            if let Some(dir) = Config::path().parent() {
                                if let Err(e) = khaloni_poe2::leveling_ui::save_done(dir, &p.done) {
                                    eprintln!("leveling: save failed: {e}");
                                }
                            }
                        }
                        Some(khaloni_poe2::leveling_ui::Action::ScrollUp) => {
                            p.scroll = p.scroll.saturating_sub(1);
                        }
                        Some(khaloni_poe2::leveling_ui::Action::ScrollDown) => {
                            p.scroll += 1; // layout clamps via the visible window
                        }
                        None => {}
                    }
                }
            }
        }
        // Typed digits into a focused value box (EE2-style numeric entry):
        // digits append, Backspace deletes, Enter commits the parsed number
        // to both the query filter and the panel row, Escape cancels.
        if editing.is_some() {
            for key in overlay.take_keys() {
                let Some((fi, field)) = editing else { break };
                let Some((panel, query, _)) = apanel.as_mut() else {
                    editing = None;
                    overlay.set_keyboard(false)?;
                    break;
                };
                match key {
                    khaloni_poe2::platform::Key::Digit(c) => {
                        if edit_buf.len() < 8 {
                            edit_buf.push(c);
                        }
                    }
                    // One decimal point, so values like 3.5 are typeable.
                    khaloni_poe2::platform::Key::Dot => {
                        if edit_buf.len() < 8 && !edit_buf.contains('.') {
                            if edit_buf.is_empty() {
                                edit_buf.push('0');
                            }
                            edit_buf.push('.');
                        }
                    }
                    khaloni_poe2::platform::Key::Backspace => {
                        edit_buf.pop();
                    }
                    khaloni_poe2::platform::Key::Enter => {
                        // Trailing "." (e.g. "3.") parses fine after trimming.
                        let cleaned = edit_buf.trim_end_matches('.');
                        let parsed: Option<f64> = if cleaned.is_empty() {
                            None
                        } else {
                            cleaned.parse().ok()
                        };
                        if let Some(f) = query.filters.get_mut(fi) {
                            match field {
                                khaloni_poe2::appraise_ui::Field::Min => {
                                    f.value.min = parsed.unwrap_or(0.0);
                                }
                                khaloni_poe2::appraise_ui::Field::Max => {
                                    f.value.max = parsed;
                                }
                            }
                        }
                        if let Some(m) = panel.mods.iter_mut().find(|m| m.filter_index == fi) {
                            match field {
                                khaloni_poe2::appraise_ui::Field::Min => m.min = parsed.unwrap_or(0.0),
                                khaloni_poe2::appraise_ui::Field::Max => m.max = parsed,
                            }
                        }
                        editing = None;
                        edit_buf.clear();
                        overlay.set_keyboard(false)?;
                    }
                    khaloni_poe2::platform::Key::Escape => {
                        editing = None;
                        edit_buf.clear();
                        overlay.set_keyboard(ref_panel.is_some() || lvl_panel.is_some())?;
                    }
                    // Text and arrow keys have no meaning in a numeric value
                    // box; they exist for the reference/leveling panels.
                    khaloni_poe2::platform::Key::Char(_)
                    | khaloni_poe2::platform::Key::Up
                    | khaloni_poe2::platform::Key::Down => {}
                }
            }
        } else if ref_panel.is_some() {
            // Search typing: every edit re-runs the category search so the
            // list live-filters; panel width can change with the results.
            let mut changed = false;
            for key in overlay.take_keys() {
                let Some((p, _)) = ref_panel.as_mut() else { break };
                match key {
                    khaloni_poe2::platform::Key::Char(c) => {
                        p.query.push(c);
                        changed = true;
                    }
                    khaloni_poe2::platform::Key::Digit(c) => {
                        p.query.push(c);
                        changed = true;
                    }
                    khaloni_poe2::platform::Key::Dot => {
                        p.query.push('.');
                        changed = true;
                    }
                    khaloni_poe2::platform::Key::Backspace => {
                        p.query.pop();
                        changed = true;
                    }
                    khaloni_poe2::platform::Key::Up => p.scroll = p.scroll.saturating_sub(1),
                    khaloni_poe2::platform::Key::Down => {
                        p.scroll = (p.scroll + 1).min(p.rows.len().saturating_sub(1));
                    }
                    khaloni_poe2::platform::Key::Escape => {
                        ref_panel = None;
                        overlay.set_keyboard(lvl_panel.is_some())?;
                        sync_input_region(&mut overlay, &renderer, &apanel, &ref_panel, &lvl_panel)?;
                    }
                    khaloni_poe2::platform::Key::Enter => {}
                }
            }
            if changed {
                if let (Some((p, _)), Some(r)) = (ref_panel.as_mut(), reference.get()) {
                    khaloni_poe2::reference_ui::refresh(p, r);
                }
                sync_input_region(&mut overlay, &renderer, &apanel, &ref_panel, &lvl_panel)?;
            }
        } else if lvl_panel.is_some() {
            for key in overlay.take_keys() {
                let Some((p, _)) = lvl_panel.as_mut() else { break };
                match key {
                    khaloni_poe2::platform::Key::Up => p.scroll = p.scroll.saturating_sub(1),
                    khaloni_poe2::platform::Key::Down => p.scroll += 1,
                    khaloni_poe2::platform::Key::Escape => {
                        lvl_panel = None;
                        overlay.set_keyboard(false)?;
                        sync_input_region(&mut overlay, &renderer, &apanel, &ref_panel, &lvl_panel)?;
                    }
                    _ => {}
                }
            }
        }
        // The panel is a deliberate, sticky action: it stays put until the
        // user closes it (X or a new price check) so they can alt-tab, click a
        // value box (which itself steals focus from the game to type), and edit
        // without it vanishing. Only a fully-gone game tears it down, since
        // then the overlay hides and its input region must not linger.
        if apanel.is_some() && !game_present {
            apanel = None;
            editing = None;
            panel_drag = None;
            overlay.set_keyboard(false)?;
            overlay.set_interactive(None)?;
        }

        hover.tick();
        match (&hover.current, popup_at) {
            (Some(_), Some((origin, rect))) => {
                if khaloni_poe2::popup_pos::should_dismiss(origin, cursor_pos, rect) {
                    hover.current = None;
                    popup_at = None;
                }
            }
            (None, Some(_)) => popup_at = None,
            _ => {}
        }

        let paused = !scanning || !game_present || (!game_visible && cfg.pause_when_hidden);
        pipeline_paused.store(paused, std::sync::atomic::Ordering::Relaxed);

        while let Ok(msg) = rows_rx.try_recv() {
            if dbg {
                match &msg {
                    khaloni_poe2::stabilize::ScanResult::GateEmpty => {
                        eprintln!("DBG rows_rx: gate-empty");
                    }
                    khaloni_poe2::stabilize::ScanResult::NoBands => {
                        eprintln!("DBG rows_rx: no-bands");
                    }
                    khaloni_poe2::stabilize::ScanResult::Rows(rows, stale) => {
                        eprintln!("DBG rows_rx: {} rows, stale={stale}", rows.len());
                    }
                    khaloni_poe2::stabilize::ScanResult::Scrolled(dy) => {
                        eprintln!("DBG rows_rx: scrolled {dy}");
                    }
                    khaloni_poe2::stabilize::ScanResult::TrackingLost => {
                        eprintln!("DBG rows_rx: tracking-lost");
                    }
                }
            }
            if scanning {
                let before = dbg.then(|| stabilizer.rows().iter().map(|r| format!("{}@y{}", r.item_key, r.y_top)).collect::<Vec<_>>());
                stabilizer.apply(msg);
                if let Some(before) = before {
                    let after: Vec<String> = stabilizer.rows().iter().map(|r| format!("{}@y{}", r.item_key, r.y_top)).collect();
                    if before != after {
                        eprintln!("TRACE stab: [{}] -> [{}]", before.join(", "), after.join(", "));
                    }
                }
            }
        }
        if dbg {
            static TICK: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let t = TICK.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if t.is_multiple_of(10) {
                eprintln!(
                    "DBG t={t} paused={paused} scanning={scanning} present={game_present} focused={game_focused} visible={game_visible} region={:?} rows={} surface={:?} game_pos={game_pos:?}",
                    scan_geom.lock().unwrap().1,
                    stabilizer.rows().len(),
                    overlay.size()
                );
            }
        }

        // Rows obey the F8 master switch; the popup only needs the game
        // on screen. An explicit F7 (or the F8 toggle note itself) must
        // stay visible while the overlay is toggled off, otherwise the
        // hotkeys read as dead keys (live finding, 2026-07-23).
        // VISIBILITY, not focus, decides hiding: an unfocused game that is
        // still on screen keeps its overlay; a minimized or covered game
        // does not (the always-on-top layer would draw over the coverer).
        let on_screen = game_present && (game_visible || !cfg.pause_when_hidden);
        let show_rows = scanning && on_screen;
        // The appraisal panel renders whenever it is open and the game is
        // present, even while unfocused: editing a value box steals keyboard
        // focus from the game, and the panel must not blink out mid-edit.
        let show = show_rows
            || (on_screen && hover.current.is_some())
            || (game_present && apanel.is_some());
        let size = overlay.size();
        if size.0 > 0 && size.1 > 0 {
            let mut resized = false;
            let pm = pixmap.get_or_insert_with(|| {
                resized = true;
                tiny_skia::Pixmap::new(size.0, size.1).expect("pixmap")
            });
            if (pm.width(), pm.height()) != size {
                *pm = tiny_skia::Pixmap::new(size.0, size.1).expect("pixmap");
                resized = true;
            }

            let frame_state = if show {
                let rows = if show_rows { stabilizer.rows() } else { Vec::new() };
                let out_pos = overlay.output_pos();
                // Placement geometry, rebuilt every paint from the live game
                // position plus the detector's (frame dims, region): labels
                // need the full map, rumour badges only the capture scale.
                let (frame_dims, region_now) = *scan_geom.lock().unwrap();
                let smap = match (frame_dims, region_now) {
                    (Some(f), Some(r)) => Some(CoordMap::new(
                        Rect { x: game_pos.0, y: game_pos.1, w: game.w, h: game.h },
                        f,
                        r,
                    )),
                    _ => None,
                };
                let cap_scale = frame_dims.map(|f| f.0 as f64 / game.w.max(1) as f64);
                // Best-pick: the single highest-value priced row (in
                // exalted terms) gets the gold marker; only meaningful
                // when at least two rows are priced (a pick-one panel).
                let best_key: Option<u32> = {
                    let priced: Vec<_> = rows
                        .iter()
                        .filter(|r| r.denom != pricing::Denom::None)
                        .collect();
                    if priced.len() >= 2 {
                        priced
                            .iter()
                            .max_by(|a, b| a.value_ex.total_cmp(&b.value_ex))
                            .map(|r| r.y_top)
                    } else {
                        None
                    }
                };
                let placed: Vec<_> = match &smap {
                    Some(m) => rows
                        .iter()
                        .map(|r| {
                            let (lx, ly) = m.label_pos_centered(r.y_top, r.height);
                            khaloni_poe2::render::Placed {
                                x: lx - out_pos.0,
                                y: ly - out_pos.1,
                                amount: r.amount.clone(),
                                denom: r.denom,
                                tier: r.tier,
                                best: Some(r.y_top) == best_key,
                            }
                        })
                        .collect(),
                    // Rows without geometry cannot happen (rows require the
                    // detector's region), but never panic in the paint path.
                    None => Vec::new(),
                };
                // Popup anchor: the rect placed at check time next to the
                // cursor (popup_pos::place), converted global -> surface
                // like the row labels. Global coords are already live, so
                // no dx/dy re-anchoring applies to the popup.
                let popup = hover.current.as_ref().and_then(|p| {
                    popup_at.map(|(_, rect)| {
                        (p.clone(), (rect.x - out_pos.0, rect.y - out_pos.1))
                    })
                });
                let panel = apanel
                    .as_ref()
                    .map(|(p, _, pos)| (p.clone(), (pos.0 - out_pos.0, pos.1 - out_pos.1)));
                // Divine=>exalted rate as the header pill above the first
                // row: answers "is this divine price worth it" at a glance
                // without a manual lookup.
                let rate = svc
                    .snapshot()
                    .table
                    .lookup("Divine Orb")
                    .map(|p| format!("1 div = {} ex", p.exalted.round() as i64))
                    .unwrap_or_default();
                // Rumour badges: capture-physical box -> global logical (game
                // origin + phys/scale) -> surface-local. Hung off the tooltip
                // panel's right edge at each rumour line's vertical center.
                let rumour_badges: Vec<khaloni_poe2::render::RumourBadge> = match cap_scale {
                    Some(scale) if show_rows => latest_rumours
                        .iter()
                        .map(|h| {
                            let phys_x = f64::from(h.panel.x1);
                            let phys_y = f64::from(h.line.y0 + h.line.y1) / 2.0;
                            khaloni_poe2::render::RumourBadge {
                                x: game_pos.0 + (phys_x / scale) as i32 - out_pos.0 + 12,
                                y: game_pos.1 + (phys_y / scale) as i32 - out_pos.1,
                                rating: h.entry.rating.clone(),
                            }
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                let edit_state = editing.map(|(fi, field)| (fi, field, edit_buf.clone()));
                // Reference/leveling panels: cloned into the frame state so
                // typing, scrolling, and checkbox toggles trigger repaints
                // through the same equality gate as everything else.
                let ref_state = ref_panel
                    .as_ref()
                    .map(|(p, pos)| (p.clone(), (pos.0 - out_pos.0, pos.1 - out_pos.1)));
                let lvl_state = lvl_panel
                    .as_ref()
                    .map(|(p, pos)| (p.clone(), (pos.0 - out_pos.0, pos.1 - out_pos.1)));
                Some((
                    placed,
                    rate,
                    stabilizer.stale(),
                    popup,
                    panel,
                    rumour_badges,
                    edit_state,
                    ref_state,
                    lvl_state,
                ))
            } else {
                None
            };

            // A fresh/resized buffer always needs a real draw regardless of
            // content equality; otherwise only repaint+present when the
            // stabilized row set (or its stale flag, or the popup, or
            // visibility) actually changed since the last tick. Including
            // the popup here is what makes its 6s expiry repaint the frame
            // to clear it, even though nothing else about the rows changed.
            if resized || frame_state != last_frame {
                match &frame_state {
                    Some((placed, rate, stale, popup, panel, rumours, edit_state, ref_state, lvl_state)) => {
                        renderer.draw_frame(pm, placed, rate, *stale);
                        // Rumour rating badges sit on the cleared frame with
                        // the rows; both are part of the on-panel overlay.
                        renderer.draw_rumours(pm, rumours);
                        // Popup drawn after the rows so it sits on top.
                        if let Some((p, anchor)) = popup {
                            renderer.draw_popup(pm, p, *anchor);
                        }
                        if let Some((p, anchor)) = panel {
                            let lay = khaloni_poe2::appraise_ui::layout(p, &|s| {
                                renderer.appraisal_label_width(s)
                            });
                            let ed = edit_state.as_ref().map(|(fi, f, _)| (*fi, *f));
                            let buf = edit_state.as_ref().map(|(_, _, b)| b.as_str()).unwrap_or("");
                            renderer.draw_appraisal(pm, p, &lay, *anchor, ed, buf);
                        }
                        if let Some((p, anchor)) = ref_state {
                            let lay = khaloni_poe2::reference_ui::layout(p, &|s| {
                                renderer.appraisal_label_width(s)
                            });
                            renderer.draw_reference(pm, p, &lay, *anchor);
                        }
                        if let Some((p, anchor)) = lvl_state {
                            let lay = khaloni_poe2::leveling_ui::layout(p, &|s| {
                                renderer.appraisal_label_width(s)
                            });
                            renderer.draw_leveling(pm, p, &lay, *anchor);
                        }
                    }
                    None => pm.fill(tiny_skia::Color::TRANSPARENT),
                }
                overlay.present(pm)?;
                if !first_present_logged {
                    first_present_logged = true;
                    phase("first frame presented");
                }
                last_frame = frame_state;
            }
        }
        // 16ms so label motion renders at the tracker's cadence during
        // scrolls; the frame_state change-detection above keeps an idle
        // tick to a channel drain plus one comparison, no repaint.
        std::thread::sleep(std::time::Duration::from_millis(16));
    }
}

fn mean_gray_brightness(img: &image::GrayImage) -> u64 {
    let raw = img.as_raw();
    if raw.is_empty() {
        return 0;
    }
    raw.iter().map(|&p| p as u64).sum::<u64>() / raw.len() as u64
}

#[cfg(test)]
mod main_tests {
    use super::urlencode;
    #[test]
    fn urlencode_handles_spaces_and_apostrophes() {
        assert_eq!(urlencode("Cold as ice"), "Cold%20as%20ice");
        assert_eq!(urlencode("Wanderlust"), "Wanderlust");
        assert_eq!(urlencode("Kaom's Heart"), "Kaom%27s%20Heart");
    }
}
