// On non-Linux targets the overlay/headless pipelines are compiled out
// (they need the Linux OCR stack; see platform/windows/mod.rs), which
// leaves their helpers and imports dead there. Linux lints are unaffected.
#![cfg_attr(not(ocr), allow(dead_code, unused_imports))]
// Release Windows builds are GUI-subsystem: no console window appears when
// the exe is launched from Explorer. Debug builds keep the console so
// `cargo run` output stays visible during development.
#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]

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

fn main() {
    // Remove the binary a previous self-update replaced, if any.
    khaloni_poe2::update::cleanup_backup();
    // Without a console (GUI subsystem, or a menu launch), diagnostics
    // must survive somewhere findable: on Windows stderr/stdout are
    // rebound to last-run.log in the cache dir (Linux menu launches
    // already land in the journal).
    #[cfg(target_os = "windows")]
    redirect_output_to_log();
    migrate_legacy_dirs();
    let args: Vec<String> = std::env::args().collect();
    let result = match args.get(1).map(String::as_str).unwrap_or("") {
        "--headless" => headless(),
        "--settings" => khaloni_poe2::settings_ui::run(),
        _ => overlay_mode(),
    };
    if let Err(e) = result {
        // A GUI app's fatal error must be visible like any normal
        // program's: native dialog first, log always.
        eprintln!("fatal: {e:#}");
        fatal_dialog(&format!("{e:#}"));
        std::process::exit(1);
    }
}

/// Shows a native error dialog. Best-effort: a missing dialog helper
/// falls back to the (already-written) log line.
fn fatal_dialog(msg: &str) {
    let text = format!("khaloni-poe2 could not start:\n\n{msg}");
    #[cfg(target_os = "windows")]
    {
        use windows::core::HSTRING;
        use windows::Win32::UI::WindowsAndMessaging::{
            MessageBoxW, MB_ICONERROR, MB_OK,
        };
        unsafe {
            MessageBoxW(None, &HSTRING::from(text.as_str()), &HSTRING::from("khaloni-poe2"), MB_OK | MB_ICONERROR);
        }
    }
    #[cfg(target_os = "linux")]
    {
        // kdialog on KDE, zenity elsewhere; silent if neither exists (the
        // journal/terminal already carries the message).
        let tried = std::process::Command::new("kdialog")
            .args(["--error", &text, "--title", "khaloni-poe2"])
            .status();
        if tried.is_err() {
            let _ = std::process::Command::new("zenity")
                .args(["--error", "--text", &text, "--title", "khaloni-poe2"])
                .status();
        }
    }
}

/// Rebinds stdout/stderr to `<cache>/last-run.log` (rotating the previous
/// run to prev-run.log) when no console is attached, so eprintln-based
/// diagnostics keep working in the GUI-subsystem build.
#[cfg(target_os = "windows")]
fn redirect_output_to_log() {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Console::{
        GetConsoleWindow, SetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
    };
    if !unsafe { GetConsoleWindow() }.is_invalid() {
        return; // launched from a terminal: leave output where it is
    }
    let Some(dirs) = directories::ProjectDirs::from("", "", "khaloni-poe2") else {
        return;
    };
    let dir = dirs.cache_dir();
    let _ = std::fs::create_dir_all(dir);
    let last = dir.join("last-run.log");
    let _ = std::fs::rename(&last, dir.join("prev-run.log"));
    let Ok(file) = std::fs::File::create(&last) else {
        return;
    };
    let h = HANDLE(file.as_raw_handle());
    unsafe {
        let _ = SetStdHandle(STD_ERROR_HANDLE, h);
        let _ = SetStdHandle(STD_OUTPUT_HANDLE, h);
    }
    // The handle must outlive the process's logging; leak it deliberately.
    std::mem::forget(file);
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
    {
        // `start` is a cmd builtin; the empty "" is its window-title slot so
        // a URL containing spaces is not mistaken for the title.
        // CREATE_NO_WINDOW keeps the helper cmd from flashing a console in
        // the GUI-subsystem build.
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
}

/// Sets the overlay's pointer input region to the union bounding box of
/// every open interactive panel (evaluate, reference, leveling), or clears
/// it when none is open. One region because the layer surface supports a
/// single rect; the union is slightly generous when panels are far apart,
/// but clicks between them still fall through to nothing (hit() misses).
fn sync_input_region(
    overlay: &mut khaloni_poe2::platform::overlay::Overlay,
    renderer: &khaloni_poe2::render::Renderer,
    apanel: &Option<(khaloni_poe2::evaluate_ui::Panel, khaloni_poe2_core::trade::Query, (i32, i32))>,
    ref_panel: &Option<(khaloni_poe2::reference_ui::Panel, (i32, i32))>,
    lvl_panel: &Option<(khaloni_poe2::leveling_ui::Panel, (i32, i32))>,
) -> anyhow::Result<()> {
    let out = overlay.output_pos();
    // One measurer for every panel: they all draw their text in the same
    // face and size, and the input region must match the drawn geometry
    // exactly or clicks land off the controls they look like they hit.
    let measure = |s: &str| renderer.evaluate_label_width(s);
    let mut boxes: Vec<(i32, i32, i32, i32)> = Vec::new();
    if let Some((p, _, pos)) = apanel {
        let lay = khaloni_poe2::evaluate_ui::layout(p, &measure);
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

/// Formats a computed estimate for the panel's value box. Divine display
/// kicks in on the same threshold the rest of the overlay uses, and the
/// listing count is always shown: this is arithmetic over listings we
/// actually fetched, not an opinion, and it should read that way.
fn estimate_view(
    est: &khaloni_poe2_core::estimate::Estimate,
    table: &khaloni_poe2_core::ninja::PriceTable,
    cfg: &Config,
) -> khaloni_poe2::evaluate_ui::EstimateView {
    use khaloni_poe2_core::estimate::Reliability;
    let div = table.lookup("Divine Orb").map(|p| p.exalted).filter(|v| *v > 0.0);
    let fmt = |ex: f64| -> (String, khaloni_poe2::pricing::Denom) {
        match div {
            Some(rate) if ex / rate >= cfg.divine_threshold => (
                khaloni_poe2_core::value::format_amount(ex / rate),
                khaloni_poe2::pricing::Denom::Divine,
            ),
            _ => (khaloni_poe2_core::value::format_amount(ex), khaloni_poe2::pricing::Denom::Exalted),
        }
    };
    let (amount, denom) = fmt(est.exalted);
    let (lo, lo_d) = fmt(est.low);
    let (hi, hi_d) = fmt(est.high);
    let unit = |d: khaloni_poe2::pricing::Denom| match d {
        khaloni_poe2::pricing::Denom::Divine => "div",
        _ => "ex",
    };
    let range = if lo_d == hi_d {
        format!("{lo}-{hi} {}", unit(hi_d))
    } else {
        format!("{lo} {} - {hi} {}", unit(lo_d), unit(hi_d))
    };
    khaloni_poe2::evaluate_ui::EstimateView {
        amount,
        denom,
        detail: format!("Range: {range}  -  from {} listing(s)", est.count),
        reliability: est.reliability.label().to_string(),
        shaky: est.reliability < Reliability::Medium,
    }
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
/// anchor), and the interactive Evaluate panel (with its anchor).
type FrameState = (
    Vec<khaloni_poe2::render::Placed>,
    String,
    bool,
    Option<(hover::Popup, (i32, i32))>,
    Option<(khaloni_poe2::evaluate_ui::Panel, (i32, i32))>,
    Vec<khaloni_poe2::render::RumourBadge>,
    // Focused value box (row index, field, live edit buffer), so typed
    // digits repaint even though the committed panel values are unchanged.
    Option<(usize, khaloni_poe2::evaluate_ui::Field, String)>,
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
    /// Run the gear-upgrade search on the copied item.
    UpgradeCheck,
}

/// Appraisal worker requests: Auto = fresh item, build the query and
/// relax until listings appear; Exact = the user's checkbox state, run
/// verbatim with no relaxation (their toggle IS the intent).
enum AppraiseReq {
    Auto(khaloni_poe2_core::item::Item),
    Exact { title: String, query: khaloni_poe2_core::trade::Query },
    /// Price a stackable currency (e.g. an omen) by its display name via the
    /// trade exchange; the result comes back on the exchange channel.
    Currency { name: String, for_row: bool },
    /// Find strictly-better listings for an equipped item: same category,
    /// every matched mod meets-or-beats the current roll, cheapest first.
    Upgrade(khaloni_poe2_core::item::Item),
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
/// Async exchange pricer for reward rows naming currencies the ninja table
/// lacks (niche runes etc.): cache-or-request, GemCache's sibling. Lookup
/// keys are canonical vocab names; misses insert Pending and queue one
/// exchange query, so each name is asked exactly once per run.
struct CurrencyCache {
    map: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, khaloni_poe2::pricing::CurrencyState>>>,
    req_tx: mpsc::Sender<AppraiseReq>,
}

impl khaloni_poe2::pricing::CurrencyPricer for CurrencyCache {
    fn lookup(&self, name: &str) -> Option<khaloni_poe2::pricing::CurrencyState> {
        let mut m = self.map.lock().unwrap();
        if let Some(state) = m.get(name) {
            return Some(*state);
        }
        m.insert(name.to_string(), khaloni_poe2::pricing::CurrencyState::Pending);
        let _ = self.req_tx.send(AppraiseReq::Currency { name: name.to_string(), for_row: true });
        Some(khaloni_poe2::pricing::CurrencyState::Pending)
    }
}

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

/// The item-card facts the Evaluate header shows, read off the parsed item
/// in the trade worker. They travel with the response because the panel is
/// built on the main loop, which only ever sees the query and the labels —
/// and a header must state what the item says, not a plausible default.
struct ItemFacts {
    /// "Rare", "Magic", … exactly as the item text words it.
    rarity: String,
    item_level: Option<u32>,
    requires_level: Option<u32>,
    /// Computed DPS figures, present only when the item states an attack
    /// rate (see core::derived).
    weapon: Option<khaloni_poe2_core::derived::WeaponStats>,
    /// Pseudo-total rows: label, the item's own total, and the index of
    /// the disabled pseudo filter appended to the query for it.
    pseudo_rows: Vec<(String, f64, usize)>,
}

struct AppraiseDone {
    title: String,
    outcome: Result<Vec<khaloni_poe2_core::trade::Listing>, String>,
    /// Query + labels only on Auto responses (they seed the panel); an
    /// Exact response updates listings on the panel the user already has.
    query: Option<khaloni_poe2_core::trade::Query>,
    labels: Vec<khaloni_poe2_core::trade::FilterLabel>,
    /// Header facts, likewise Auto-only: nothing else builds a panel.
    facts: Option<ItemFacts>,
    search_id: Option<String>,
    /// Computed from the listings this search actually returned (never a
    /// model's opinion), or None when nothing priceable came back.
    estimate: Option<khaloni_poe2_core::estimate::Estimate>,
}

/// Writes one weapon bound into the query, dropping the whole section when
/// the last bound clears so an empty block never serializes.
fn set_weapon_bound(
    query: &mut khaloni_poe2_core::trade::Query,
    bound: khaloni_poe2::evaluate_ui::WeaponBound,
    min: Option<f64>,
) {
    use khaloni_poe2::evaluate_ui::WeaponBound as B;
    let w = query.weapon.get_or_insert_with(Default::default);
    *match bound {
        B::Dps => &mut w.dps,
        B::Pdps => &mut w.pdps,
        B::Edps => &mut w.edps,
        B::Crit => &mut w.crit,
        B::Aps => &mut w.aps,
    } = min;
    if w.is_empty() {
        query.weapon = None;
    }
}

/// The rarity word the item text carried. `Rarity::Other` keeps whatever the
/// game wrote rather than being folded into a family we did not read.
fn rarity_label(r: &khaloni_poe2_core::item::Rarity) -> String {
    use khaloni_poe2_core::item::Rarity as R;
    match r {
        R::Normal => "Normal".to_string(),
        R::Magic => "Magic".to_string(),
        R::Rare => "Rare".to_string(),
        R::Unique => "Unique".to_string(),
        R::Currency => "Currency".to_string(),
        R::Gem => "Gem".to_string(),
        R::Quest => "Quest".to_string(),
        R::Other(s) => s.clone(),
    }
}

/// The level requirement off the item's own "Requires: Level 78, 163 Dex"
/// line (the parser keeps every section's raw lines). None when the item
/// text has no such line — several classes and chat links omit it, and an
/// absent line must show as absent, not as level 1.
fn requires_level(item: &khaloni_poe2_core::item::Item) -> Option<u32> {
    item.sections
        .iter()
        .flatten()
        .find_map(|l| l.strip_prefix("Requires:"))
        .and_then(|rest| {
            // "Level 78, 163 Dex" -> 78; attribute requirements are listed
            // after the level and are not what the header line states.
            let after = rest.split(',').find_map(|p| {
                let p = p.trim();
                p.strip_prefix("Level ").or_else(|| p.strip_prefix("Level: "))
            })?;
            after.trim().parse().ok()
        })
}

/// Core's affix family -> the Evaluate panel's badge family. Two enums on
/// purpose: core cannot depend on the app crate, and the UI type is free to
/// diverge (a badge is a drawing concern, a generation type is data).
fn ui_affix_kind(k: khaloni_poe2_core::refdata::AffixKind) -> khaloni_poe2::evaluate_ui::AffixKind {
    use khaloni_poe2::evaluate_ui::AffixKind as U;
    use khaloni_poe2_core::refdata::AffixKind as C;
    match k {
        C::Prefix => U::Prefix,
        C::Suffix => U::Suffix,
        C::Other => U::Other,
    }
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
        if !cfg.hotkey_upgrade.is_empty() {
            extra.push(("upgrade".to_string(), cfg.hotkey_upgrade.clone()));
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

    // Game-log tail: zone events drive the F10 leveling auto-advance. A
    // missing Client.txt just means the feature stays dormant (the tail
    // retries the open forever).
    let (log_tx, log_rx) = mpsc::channel();
    match cfg.client_log_path.as_ref().map(std::path::PathBuf::from).or_else(khaloni_poe2::gamelog_tail::default_log_path) {
        Some(p) => {
            khaloni_poe2::gamelog_tail::spawn(p, log_tx);
        }
        None => eprintln!("game log not found; leveling auto-advance off (set client_log_path)"),
    }
    // Update check: report-only, background, silent on failure.
    let (update_tx, update_rx) = mpsc::channel();
    // Dev builds check too (knowing is useful, and it is read-only);
    // only INSTALLING is refused there, in update::apply.
    if cfg.check_updates {
        khaloni_poe2::update::spawn_check(update_tx);
    }
    // Live-search alerts + wealth snapshots: both no-op without credentials.
    let (alert_tx, alert_rx) = mpsc::channel();
    khaloni_poe2::livesearch::spawn(cfg.live_searches.clone(), cfg.poesessid.clone(), alert_tx);
    {
        let (wealth_tx, _wealth_rx) = mpsc::channel();
        khaloni_poe2::wealth::spawn(
            cfg.account_name.clone(),
            cfg.league.clone(),
            cfg.poesessid.clone(),
            svc.clone(),
            wealth_tx,
        );
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
    let (exch_tx, exch_rx) = mpsc::channel::<(String, Option<f64>, bool)>();
    // Exchange-catalog display names, published by the trade worker once the
    // static list arrives; the OCR worker extends its match vocab with them.
    let exch_names: std::sync::Arc<std::sync::OnceLock<Vec<String>>> =
        std::sync::Arc::new(std::sync::OnceLock::new());
    // name -> async exchange price state for reward rows (GemCache's sibling).
    let currency_map: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, khaloni_poe2::pricing::CurrencyState>>> =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    // Specific-gem price cache, shared with the OCR pricer.
    let gem_map: GemMap = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    {
        let tx = appraise_tx.clone();
        let exch_tx = exch_tx.clone();
        let league = cfg.league.clone();
        let gem_map = gem_map.clone();
        let svc_gem = svc.clone();
        let exch_names_pub = exch_names.clone();
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
            let _ = exch_names_pub.set(currency_ids.keys().cloned().collect());
            // Reverse map (trade currency id -> display name), for converting a
            // gem listing's price currency to exalted via the poe.ninja table.
            let cur_id_to_name: std::collections::HashMap<String, String> =
                currency_ids.iter().map(|(name, id)| (id.clone(), name.clone())).collect();
            // Exact gem base-type names, for resolving OCR'd skill names.
            let gem_types = client.gem_types().unwrap_or_default();
            for req in appraise_req_rx {
                // Currency exchange is priced separately from item search.
                if let AppraiseReq::Currency { name, for_row } = &req {
                    let rate = currency_ids
                        .get(&name.to_lowercase())
                        .and_then(|id| client.exchange(id, "exalted").ok().flatten());
                    let _ = exch_tx.send((name.clone(), rate, *for_row));
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
                let (title, q, labels, facts, relaxed) = match req {
                    AppraiseReq::Auto(item) => {
                        let title = if item.name.is_empty() {
                            item.base_type.clone().unwrap_or_default()
                        } else {
                            item.name.clone()
                        };
                        let (mut q, labels) =
                            khaloni_poe2_core::trade::build_query_with_labels(&item, &stats);
                        // Pseudo totals ride along as DISABLED filters: a
                        // pseudo aggregates mods the query already filters
                        // individually, and sending both over-constrains.
                        // The user opts in from the panel.
                        let pseudo = khaloni_poe2_core::derived::pseudo_totals(&item);
                        let mut pseudo_rows = Vec::new();
                        for (id, label, total) in [
                            ("pseudo.pseudo_total_life", "Total Life", pseudo.total_life),
                            (
                                "pseudo.pseudo_total_energy_shield",
                                "Total Energy Shield",
                                pseudo.total_es,
                            ),
                            (
                                "pseudo.pseudo_total_elemental_resistance",
                                "Total Elemental Resistance",
                                pseudo.total_elemental_resistance,
                            ),
                            (
                                "pseudo.pseudo_total_attributes",
                                "Total Attributes",
                                pseudo.total_attributes,
                            ),
                        ] {
                            if total <= 0.0 {
                                continue;
                            }
                            if let Some(mut f) =
                                khaloni_poe2_core::trade::pseudo_filter(&stats, id, total)
                            {
                                f.disabled = true;
                                q.filters.push(f);
                                pseudo_rows.push((label.to_string(), total, q.filters.len() - 1));
                            }
                        }
                        // Header facts are read here, while the parsed item
                        // still exists; the main loop never sees it.
                        let facts = ItemFacts {
                            rarity: rarity_label(&item.rarity),
                            item_level: item.item_level,
                            requires_level: requires_level(&item),
                            weapon: khaloni_poe2_core::derived::weapon_stats(&item),
                            pseudo_rows,
                        };
                        (title, q, labels, Some(facts), true)
                    }
                    AppraiseReq::Exact { title, query } => (title, query, Vec::new(), None, false),
                    AppraiseReq::Upgrade(item) => {
                        let q = khaloni_poe2_core::trade::build_upgrade_query(&item, &stats);
                        let title = khaloni_poe2_core::trade::upgrade_title(&item);
                        (title, q, Vec::new(), None, false)
                    }
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
                // Normalize every listing to exalted before estimating;
                // listings priced in a currency the table does not carry
                // are dropped rather than guessed at.
                let estimate = outcome.as_ref().ok().and_then(|ls| {
                    let table = &svc_gem.snapshot().table;
                    let ex: Vec<f64> = ls
                        .iter()
                        .filter_map(|l| {
                            if l.price_currency == "exalted" {
                                Some(l.price_amount)
                            } else {
                                cur_id_to_name
                                    .get(&l.price_currency)
                                    .and_then(|n| table.lookup(n))
                                    .map(|p| l.price_amount * p.exalted)
                            }
                        })
                        .collect();
                    khaloni_poe2_core::estimate::estimate(&ex)
                });
                let _ = tx.send(AppraiseDone {
                    title,
                    outcome,
                    query: relaxed.then_some(q),
                    labels,
                    facts,
                    search_id,
                    estimate,
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
    // Full frames feed two consumers with very different costs: region
    // detection (pure math, ~40ms) and rumour OCR (seconds when any
    // parchment-like blob — including combat explosions — is on screen).
    // They MUST NOT share a thread: reward panels open right after combat,
    // exactly when a shared thread would still be chewing explosion frames,
    // which measured as 30s+ first-detection latency. The fan-out clones
    // each ~8MB frame once per 700ms — noise next to one OCR pass.
    let (det_tx, det_rx) = mpsc::sync_channel::<image::GrayImage>(1);
    let (rum_tx, rum_rx) = mpsc::sync_channel::<image::GrayImage>(1);
    std::thread::spawn(move || {
        for frame in full_rx {
            let _ = det_tx.try_send(frame.clone());
            let _ = rum_tx.try_send(frame);
        }
    });
    // Region-detection worker: always fast, never blocked by OCR.
    {
        let scan_geom = scan_geom.clone();
        let region_ready = region_ready.clone();
        let panel_open_det = panel_open.clone();
        std::thread::spawn(move || {
            let dbg = std::env::var("KHALONI_DEBUG").is_ok();
            let mut last_region: Option<Rect> = None;
            for frame in det_rx {
                // While the brightness gate is open the region is LOCKED:
                // the stabilizer's scroll origin must not move under it.
                // Redetect only when closed.
                // Live-debug: keep the latest full frame on disk so a
                // detection miss can be reproduced offline against the
                // exact pixels (overwritten each ~700ms frame).
                if std::env::var("KHALONI_REGION_DUMP").is_ok() {
                    let _ = frame.save(std::env::temp_dir().join("khaloni-frame.png"));
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
                    // A vanished panel keeps the last region: the gate is
                    // closed anyway, and reusing it makes reopening in the
                    // same spot (the common case) instant.
                }
            }
        });
    }
    // Rumour recognizer worker: OCR-heavy, allowed to lag; latest-only
    // channels mean it just skips to the newest frame when it falls behind.
    {
        let rumour_csv = Config::path().parent().map(|d| d.join("rumours.csv"));
        let paused_rumour = pipeline_paused.clone();
        std::thread::spawn(move || {
            let dbg = std::env::var("KHALONI_DEBUG").is_ok();
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
            for frame in rum_rx {
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
    let currency_cache = CurrencyCache { map: currency_map.clone(), req_tx: appraise_req_tx.clone() };
    let exch_names_ocr = exch_names.clone();
    let region_ready_ocr = region_ready.clone();
    std::thread::spawn(move || {
        let dbg = std::env::var("KHALONI_DEBUG").is_ok();
        let t0 = std::time::Instant::now();
        // Match vocab = price-table names + exchange catalog (async-published);
        // rebuilt only when either side actually changes.
        let mut vocab_ext: Option<pricing::Vocab> = None;
        let mut vocab_key: (usize, usize) = (0, 0);
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
            let extra = exch_names_ocr.get().map(|v| v.as_slice()).unwrap_or(&[]);
            let key = (snap.table.len(), extra.len());
            if vocab_ext.is_none() || vocab_key != key {
                vocab_ext = Some(pricing::build_vocab_with(&snap.table, extra));
                vocab_key = key;
            }
            let out = pricing::price_lines_with_rumours(
                &snap.table,
                vocab_ext.as_ref().unwrap_or(&snap.vocab),
                &lines,
                &ocr_cfg,
                rumours.as_ref(),
                Some(&gem_cache),
                Some(&currency_cache),
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
    // Interactive Evaluate panel: model + the query its checkboxes edit
    // + placed top-left (global logical). While Some, the overlay's input
    // region covers the panel and clicks resolve through evaluate_ui.
    let mut apanel: Option<(
        khaloni_poe2::evaluate_ui::Panel,
        khaloni_poe2_core::trade::Query,
        (i32, i32),
    )> = None;
    // Which value box is being typed into (index into `panel.rows`, which is
    // what evaluate_ui's actions carry), and the digits typed so far.
    let mut editing: Option<(usize, khaloni_poe2::evaluate_ui::Field)> = None;
    let mut edit_buf = String::new();
    // In-overlay reference search panel (F9) and leveling checklist (F10),
    // each with its placed top-left in global logical coordinates. While
    // open they join the overlay's input region and take keyboard focus
    // for search typing / scrolling.
    let mut ref_panel: Option<(khaloni_poe2::reference_ui::Panel, (i32, i32))> = None;
    let mut lvl_panel: Option<(khaloni_poe2::leveling_ui::Panel, (i32, i32))> = None;
    // Last zone seen in the game log, applied when the leveling panel opens
    // so it comes up already pointing at where the player is.
    let mut last_zone: Option<String> = None;
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
        // Zone changes advance the leveling panel (open now, or on next
        // open via last_zone). Whispers/joins go unused — the whisper
        // queue was deliberately cut.
        while let Ok(ev) = log_rx.try_recv() {
            if let khaloni_poe2_core::gamelog::LogEvent::ZoneEnter(zone) = ev {
                if let Some((p, _)) = lvl_panel.as_mut() {
                    if khaloni_poe2::leveling_ui::advance_to_zone(p, &zone) {
                        if let Some(dir) = Config::path().parent() {
                            let _ = khaloni_poe2::leveling_ui::save_done(dir, &p.done);
                        }
                    }
                }
                last_zone = Some(zone);
            }
        }
        while let Ok(u) = update_rx.try_recv() {
            // One passive note; installing lives in the settings window so
            // an update never interrupts play.
            hover.show_note(&format!("{} available — see Settings", u.version));
        }
        while let Ok(alert) = alert_rx.try_recv() {
            let khaloni_poe2::livesearch::Alert::NewListings { search, count } = alert;
            hover.show_note(&format!("{search}: {count} new listing(s)"));
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
                        let size = renderer.popup_size(p);
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
                            let mut p = khaloni_poe2::leveling_ui::Panel { acts, act: 0, done, scroll: 0 };
                            if let Some(z) = &last_zone {
                                let _ = khaloni_poe2::leveling_ui::advance_to_zone(&mut p, z);
                            }
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
                    } else if id == "upgrade" {
                        if let Some(inj) = &injector {
                            if pending_action.is_none() {
                                pending_action = Some(PendingAction::UpgradeCheck);
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
            match (action, result) {
                (Some(PendingAction::Shortcut(i)), Ok(text)) => {
                    if let Some(sc) = cfg.resource_shortcuts.get(i) {
                        open_resource(&sc.url, &text);
                    }
                }
                (Some(PendingAction::UpgradeCheck), Ok(text)) => {
                    match khaloni_poe2_core::item::parse_item(&text) {
                        Ok(item) => {
                            hover.show_note("searching upgrades...");
                            let _ = appraise_req_tx.send(AppraiseReq::Upgrade(item));
                        }
                        Err(_) => hover.show_note("hover an equipped item first"),
                    }
                }
                _ => {}
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
                        let _ = appraise_req_tx.send(AppraiseReq::Currency { name, for_row: false });
                    }
                }
                Err(e) => eprintln!("price check: {e}"),
            }
            // A fresh popup anchors at the cursor that triggered it.
            popup_at = hover.current.as_ref().map(|p| {
                let size = renderer.popup_size(p);
                let (px, py) = khaloni_poe2::popup_pos::place(cursor_pos, size, game_rect);
                (cursor_pos, Rect { x: px, y: py, w: size.0 as u32, h: size.1 as u32 })
            });
        }
        // Currency-exchange results replace the "checking exchange..." popup
        // in place (the anchor from the F7 press still applies).
        while let Ok((name, rate, for_row)) = exch_rx.try_recv() {
            let state = match rate {
                Some(ex) => khaloni_poe2::pricing::CurrencyState::Priced(ex),
                None => khaloni_poe2::pricing::CurrencyState::Unpriced,
            };
            currency_map.lock().unwrap().insert(name.clone(), state);
            if !for_row {
                hover.show_exchange(&name, rate);
            }
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
                    // Affix index once per panel, not once per row: it is a
                    // map over the whole affix export (tens of thousands of
                    // entries) and every row looks into the same one.
                    let affix_ix = reference
                        .get()
                        .map(|r| khaloni_poe2_core::refdata::affix_index(&r.affixes));
                    let mut rows: Vec<khaloni_poe2::evaluate_ui::StatRow> = done
                        .labels
                        .iter()
                        .enumerate()
                        .filter_map(|(i, l)| {
                            let f = query.filters.get(i)?;
                            // The filter's min IS the item's own roll at build
                            // time (build_query_with_labels seeds it from the
                            // rolled value), so it is what the tier ladder and
                            // the score are read against.
                            let rolled = f.value.min;
                            // A miss, or an affix with no ladder joined to it,
                            // gets no badge and no score. An unknown roll is
                            // shown as unknown; it is never approximated.
                            let affix = affix_ix
                                .as_ref()
                                .and_then(|ix| {
                                    ix.get(&khaloni_poe2_core::refdata::normalize_mod_text(&l.text))
                                })
                                .filter(|a| !a.tiers.is_empty());
                            let badge = affix.and_then(|a| {
                                khaloni_poe2_core::rollquality::tier_of(&a.tiers, rolled).map(
                                    |tier| khaloni_poe2::evaluate_ui::TierBadge {
                                        kind: ui_affix_kind(a.kind),
                                        tier,
                                    },
                                )
                            });
                            let score = affix
                                .and_then(|a| khaloni_poe2_core::rollquality::score(&a.tiers, rolled));
                            Some(khaloni_poe2::evaluate_ui::StatRow {
                                label: l.text.clone(),
                                badge,
                                score,
                                min: f.value.min,
                                max: f.value.max,
                                enabled: !f.disabled,
                                target: Some(khaloni_poe2::evaluate_ui::Target::Stat(i)),
                                hidden: false,
                            })
                        })
                        .collect();
                    // Gear carries a base-type toggle so the user can search
                    // mods-only; items priced by their base (waystones, whose
                    // category is None) get no toggle.
                    let facts = done.facts;
                    // Weapon figures lead the card the way the tooltip's own
                    // property block does; each is searchable as an
                    // equipment_filters minimum, off until the user opts in.
                    if let Some(w) = facts.as_ref().and_then(|f| f.weapon) {
                        use khaloni_poe2::evaluate_ui::{StatRow, Target, WeaponBound};
                        let head: Vec<StatRow> = [
                            ("Physical DPS", w.phys_dps, WeaponBound::Pdps),
                            ("Elemental DPS", w.ele_dps, WeaponBound::Edps),
                            ("Chaos DPS", w.chaos_dps, WeaponBound::Dps),
                            ("Total DPS", w.total_dps, WeaponBound::Dps),
                            ("Critical Hit Chance", w.crit_chance, WeaponBound::Crit),
                            ("Attacks per Second", w.aps, WeaponBound::Aps),
                        ]
                        .into_iter()
                        .filter(|(_, v, _)| *v > 0.0)
                        .map(|(label, value, bound)| StatRow {
                            label: label.to_string(),
                            badge: None,
                            score: None,
                            // One decimal: the box shows what would be
                            // searched, and 420.75 as a bound reads as noise.
                            min: (value * 10.0).round() / 10.0,
                            max: None,
                            enabled: false,
                            target: Some(Target::Weapon(bound)),
                            hidden: false,
                        })
                        .collect();
                        rows.splice(0..0, head);
                    }
                    // Pseudo totals collapse behind "Show N more": they
                    // duplicate mods already listed, so they earn a line
                    // only when the user asks for them.
                    for (label, total, fi) in
                        facts.as_ref().map(|f| f.pseudo_rows.as_slice()).unwrap_or_default()
                    {
                        rows.push(khaloni_poe2::evaluate_ui::StatRow {
                            label: label.clone(),
                            badge: None,
                            score: None,
                            min: *total,
                            max: None,
                            enabled: false,
                            target: Some(khaloni_poe2::evaluate_ui::Target::Stat(*fi)),
                            hidden: true,
                        });
                    }
                    let base = query.category.as_deref().map(|c| {
                        khaloni_poe2::evaluate_ui::BaseToggle {
                            label: format!("Base: {}", pretty_category(c)),
                            enabled: query.category_enabled,
                        }
                    });
                    let estimate = done
                        .estimate
                        .as_ref()
                        .map(|e| estimate_view(e, &svc.snapshot().table, &cfg));
                    let panel = khaloni_poe2::evaluate_ui::Panel {
                        header: khaloni_poe2::evaluate_ui::ItemHeader {
                            name: done.title,
                            // Rare is the fallback only when the response
                            // carried no facts at all (it always does for an
                            // Auto search); the rest stay absent when absent.
                            rarity: facts
                                .as_ref()
                                .map(|f| f.rarity.clone())
                                .unwrap_or_else(|| "Rare".to_string()),
                            item_level: facts.as_ref().and_then(|f| f.item_level),
                            requires_level: facts.as_ref().and_then(|f| f.requires_level),
                            base,
                        },
                        rows,
                        show_hidden: false,
                        strictness: khaloni_poe2::evaluate_ui::Strictness::Quick,
                        listings,
                        estimate,
                        status,
                        search_id: done.search_id,
                    };
                    let origin = popup_at.map(|(o, _)| o).unwrap_or(cursor_pos);
                    let lay = khaloni_poe2::evaluate_ui::layout(&panel, &|s| {
                        renderer.evaluate_label_width(s)
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
                (None, Some((panel, _, _))) if panel.header.name == done.title => {
                    let (listings, status) = listings_of(&done.outcome);
                    panel.listings = listings;
                    panel.estimate = done
                        .estimate
                        .as_ref()
                        .map(|e| estimate_view(e, &svc.snapshot().table, &cfg));
                    panel.status = status;
                    if done.search_id.is_some() {
                        panel.search_id = done.search_id;
                    }
                    // Listings and the value box grow the card downwards, so
                    // the buttons under them move: without this the region
                    // still describes the pre-search panel and the Search
                    // button stops answering after the first search.
                    sync_input_region(&mut overlay, &renderer, &apanel, &ref_panel, &lvl_panel)?;
                }
                // Panel was closed while the search ran: drop the result.
                (None, _) => {}
            }
        }
        // Panel clicks: geometry from the same layout the renderer drew.
        // The Evaluate panel gets first claim on each click (preserving its
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
                let lay = khaloni_poe2::evaluate_ui::layout(panel, &|s| renderer.evaluate_label_width(s));
                let local = (cx - (pos.0 - out_pos.0), cy - (pos.1 - out_pos.1));
                let inside = local.0 >= 0
                    && local.0 < lay.size.0
                    && local.1 >= 0
                    && local.1 < lay.size.1;
                if !inside {
                    leftover_clicks.push((cx, cy));
                    continue;
                }
                match khaloni_poe2::evaluate_ui::hit(panel, &lay, local.0, local.1) {
                    // Row indices, not filter indices: evaluate_ui's actions
                    // address `panel.rows`, and the filter behind a row is
                    // whatever that row's `filter_index` names.
                    Some(khaloni_poe2::evaluate_ui::Action::ToggleRow(i)) => {
                        if let Some(row) = panel.rows.get_mut(i) {
                            row.enabled = !row.enabled;
                            // The row's checkbox and the query are one state
                            // shown twice; they are written together so they
                            // cannot disagree about what gets searched.
                            match row.target {
                                Some(khaloni_poe2::evaluate_ui::Target::Stat(fi)) => {
                                    if let Some(f) = query.filters.get_mut(fi) {
                                        f.disabled = !row.enabled;
                                    }
                                }
                                Some(khaloni_poe2::evaluate_ui::Target::Weapon(b)) => {
                                    set_weapon_bound(query, b, row.enabled.then_some(row.min));
                                }
                                None => {}
                            }
                        }
                    }
                    // Dropping the base searches the mods across every base.
                    Some(khaloni_poe2::evaluate_ui::Action::ToggleBase) => {
                        query.category_enabled = !query.category_enabled;
                        if let Some(b) = panel.header.base.as_mut() {
                            b.enabled = query.category_enabled;
                        }
                    }
                    // Clicking a value box focuses it for keyboard entry.
                    Some(khaloni_poe2::evaluate_ui::Action::Edit(i, field)) => {
                        editing = Some((i, field));
                        edit_buf.clear();
                        overlay.set_keyboard(true)?;
                    }
                    Some(khaloni_poe2::evaluate_ui::Action::SetStrictness(s)) => {
                        panel.strictness = s;
                    }
                    // Expanding the collapsed rows changes the panel's height,
                    // so the input region has to follow it.
                    Some(khaloni_poe2::evaluate_ui::Action::ToggleHidden) => {
                        panel.show_hidden = !panel.show_hidden;
                        sync_input_region(&mut overlay, &renderer, &apanel, &ref_panel, &lvl_panel)?;
                    }
                    Some(khaloni_poe2::evaluate_ui::Action::Search) => {
                        panel.status = "searching...".into();
                        // Broad relaxes every kept minimum by 10% before the
                        // search runs; Quick sends the user's own numbers
                        // verbatim, since their toggles ARE the intent.
                        let q = match panel.strictness {
                            khaloni_poe2::evaluate_ui::Strictness::Broad => {
                                khaloni_poe2_core::trade::relax_query(query, 0.10)
                            }
                            khaloni_poe2::evaluate_ui::Strictness::Quick => query.clone(),
                        };
                        let _ = appraise_req_tx.send(AppraiseReq::Exact {
                            title: panel.header.name.clone(),
                            query: q,
                        });
                    }
                    Some(khaloni_poe2::evaluate_ui::Action::OpenSite) => {
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
                    Some(khaloni_poe2::evaluate_ui::Action::Close) => {
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
        // Reference/leveling panel clicks: whatever the Evaluate panel did
        // not claim, in priority order reference then leveling.
        for (cx, cy) in leftover_clicks {
            if let Some((p, pos)) = ref_panel.as_mut() {
                let lay = khaloni_poe2::reference_ui::layout(p, &|s| renderer.evaluate_label_width(s));
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
                let lay = khaloni_poe2::leveling_ui::layout(p, &|s| renderer.evaluate_label_width(s));
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
                let Some((row_i, field)) = editing else { break };
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
                        // The row is what was clicked; the filter behind it is
                        // what gets searched. Both are written from the one
                        // parse so the drawn box and the query cannot differ.
                        let target = panel.rows.get(row_i).and_then(|r| r.target);
                        if let Some(row) = panel.rows.get_mut(row_i) {
                            match field {
                                khaloni_poe2::evaluate_ui::Field::Min => {
                                    row.min = parsed.unwrap_or(0.0)
                                }
                                khaloni_poe2::evaluate_ui::Field::Max => row.max = parsed,
                            }
                        }
                        match target {
                            Some(khaloni_poe2::evaluate_ui::Target::Stat(fi)) => {
                                if let Some(f) = query.filters.get_mut(fi) {
                                    match field {
                                        khaloni_poe2::evaluate_ui::Field::Min => {
                                            f.value.min = parsed.unwrap_or(0.0);
                                        }
                                        khaloni_poe2::evaluate_ui::Field::Max => {
                                            f.value.max = parsed;
                                        }
                                    }
                                }
                            }
                            // Weapon bounds are minimums only (hit() never
                            // yields a Max edit for them), and only a row
                            // that is switched on has a live bound.
                            Some(khaloni_poe2::evaluate_ui::Target::Weapon(b)) => {
                                if let Some(row) = panel.rows.get(row_i) {
                                    if row.enabled {
                                        set_weapon_bound(query, b, Some(row.min));
                                    }
                                }
                            }
                            None => {}
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
        // The Evaluate panel renders whenever it is open and the game is
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
                            let lay = khaloni_poe2::evaluate_ui::layout(p, &|s| {
                                renderer.evaluate_label_width(s)
                            });
                            let ed = edit_state.as_ref().map(|(i, f, _)| (*i, *f));
                            let buf = edit_state.as_ref().map(|(_, _, b)| b.as_str()).unwrap_or("");
                            renderer.draw_evaluate(pm, p, &lay, *anchor, ed, buf);
                        }
                        if let Some((p, anchor)) = ref_state {
                            let lay = khaloni_poe2::reference_ui::layout(p, &|s| {
                                renderer.evaluate_label_width(s)
                            });
                            renderer.draw_reference(pm, p, &lay, *anchor);
                        }
                        if let Some((p, anchor)) = lvl_state {
                            let lay = khaloni_poe2::leveling_ui::layout(p, &|s| {
                                renderer.evaluate_label_width(s)
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
    use super::{rarity_label, requires_level, urlencode};

    fn item(text: &str) -> khaloni_poe2_core::item::Item {
        khaloni_poe2_core::item::parse_item(text).expect("fixture parses")
    }

    /// The Evaluate header states what the item text says: its rarity word,
    /// and the level line only when the item carries one.
    #[test]
    fn header_facts_come_from_the_item_text() {
        let bow = item(concat!(
            "Item Class: Bows\n",
            "Rarity: Rare\n",
            "Horror Bane\n",
            "Advanced Zealot Bow\n",
            "--------\n",
            "Requires: Level 78, 163 Dex\n",
            "--------\n",
            "Item Level: 81\n",
        ));
        assert_eq!(rarity_label(&bow.rarity), "Rare");
        assert_eq!(requires_level(&bow), Some(78));
        assert_eq!(bow.item_level, Some(81));

        // No "Requires:" line: absent, not defaulted to a level.
        let ring = item(concat!(
            "Item Class: Rings\n",
            "Rarity: Magic\n",
            "Kraken Grip Sapphire Ring\n",
            "--------\n",
            "Item Level: 74\n",
        ));
        assert_eq!(rarity_label(&ring.rarity), "Magic");
        assert_eq!(requires_level(&ring), None);
    }

    #[test]
    fn urlencode_handles_spaces_and_apostrophes() {
        assert_eq!(urlencode("Cold as ice"), "Cold%20as%20ice");
        assert_eq!(urlencode("Wanderlust"), "Wanderlust");
        assert_eq!(urlencode("Kaom's Heart"), "Kaom%27s%20Heart");
    }
}
