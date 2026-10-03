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

/// Set once the overlay has reached its main loop. An error after that is
/// the overlay stopping, not failing to start, and the dialog says which.
static OVERLAY_RUNNING: AtomicBool = AtomicBool::new(false);

/// Marks OMP_* variables as set by `limit_openmp_threads`, so the launch
/// wrapper knows to keep them away from the game.
#[cfg(target_os = "linux")]
const OMP_MARKER: &str = "KHALONI_OMP_LIMITED";

/// Restarts this process with tesseract's OpenMP pool limited to one
/// passive thread. libgomp reads OMP_THREAD_LIMIT and OMP_WAIT_POLICY once,
/// when it is loaded, so setting them from inside the process does
/// nothing; without them its idle workers spin and were measured at close
/// to half of the overlay's CPU. `exec` keeps the pid, so the process Steam
/// tracks for `--launch` is unchanged. No loop: the restarted process sees
/// the variable and returns. A failed exec only costs the CPU saving.
#[cfg(target_os = "linux")]
fn limit_openmp_threads() {
    use std::os::unix::process::CommandExt;
    if !needs_openmp_restart(std::env::var_os("OMP_THREAD_LIMIT").as_deref()) {
        return;
    }
    let Ok(exe) = std::env::current_exe() else { return };
    let err = std::process::Command::new(exe)
        .args(std::env::args_os().skip(1))
        .env("OMP_THREAD_LIMIT", "1")
        .env("OMP_WAIT_POLICY", "passive")
        .env(OMP_MARKER, "1")
        .exec();
    eprintln!("restart with OpenMP limited failed ({err}); OCR will use more CPU");
}

/// A value the user (or the first pass through here) already set stands.
#[cfg(target_os = "linux")]
fn needs_openmp_restart(thread_limit: Option<&std::ffi::OsStr>) -> bool {
    thread_limit.is_none()
}

fn main() {
    #[cfg(target_os = "linux")]
    limit_openmp_threads();
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
    // Steam wrapper mode exits with the GAME's code, not the overlay's,
    // so it resolves before the common error path below.
    if args.get(1).map(String::as_str) == Some("--launch") {
        match launch_mode(&args[2..]) {
            Ok(code) => std::process::exit(code),
            Err(e) => {
                eprintln!("fatal: {e:#}");
                fatal_dialog(fatal_heading(false, None), &format!("{e:#}"));
                std::process::exit(1);
            }
        }
    }
    let result = match args.get(1).map(String::as_str).unwrap_or("") {
        "--headless" => headless(),
        "--settings" => khaloni_poe2::settings_ui::run(),
        GAME_SESSION_ARG => game_session_mode(),
        _ => overlay_mode(None, None),
    };
    if let Err(e) = result {
        // A GUI app's fatal error must be visible like any normal
        // program's: native dialog first, log always.
        eprintln!("fatal: {e:#}");
        let heading = fatal_heading(
            OVERLAY_RUNNING.load(Ordering::Relaxed),
            e.downcast_ref::<khaloni_poe2::platform::OverlayError>(),
        );
        fatal_dialog(heading, &format!("{e:#}"));
        std::process::exit(1);
    }
}

/// The dialog's first line. An overlay that ran for an hour and then lost
/// the compositor did not fail to start, and saying it did sent the user
/// looking at their install instead of at what just happened.
fn fatal_heading(
    running: bool,
    overlay: Option<&khaloni_poe2::platform::OverlayError>,
) -> &'static str {
    use khaloni_poe2::platform::OverlayError;
    match overlay {
        Some(OverlayError::Connection(_)) => "khaloni-poe2 stopped: the display connection was lost",
        Some(OverlayError::Startup(_) | OverlayError::NoOutput) if !running => "khaloni-poe2 could not start",
        _ if running => "khaloni-poe2 stopped working",
        _ => "khaloni-poe2 could not start",
    }
}

/// Spawns a helper program and leaves it running, with a thread waiting on
/// it: a child nobody waits for stays a zombie for as long as the overlay
/// lives, one per opened link.
fn spawn_detached(mut cmd: std::process::Command, what: &str) -> std::io::Result<()> {
    let mut child = cmd.spawn()?;
    let waiter = std::thread::Builder::new().name(format!("reap-{what}")).spawn(move || {
        let _ = child.wait();
    });
    if let Err(e) = waiter {
        eprintln!("{what}: no waiter thread ({e}); the child will linger until exit");
    }
    Ok(())
}

/// Steam wrapper mode: `khaloni-poe2 --launch %command%` in the game's
/// launch options. Spawns the game command as a child, starts the overlay
/// beside it as a second child, and closes the overlay the moment the game
/// exits. The wrapper process stays alive as long as the game does, so
/// Steam keeps tracking the session it started, and the game's own exit
/// code is what Steam sees.
///
/// The overlay is its own process so that quitting it quits it. When it
/// ran inside the wrapper, Quit ended the overlay loop but the process had
/// to live on for Steam, tray icon and all: the overlay looked impossible
/// to close.
fn launch_mode(rest: &[String]) -> anyhow::Result<i32> {
    let cmd = game_command(rest).ok_or_else(|| {
        anyhow::anyhow!(
            "--launch needs the game command; in Steam's launch options use: khaloni-poe2 --launch %command%"
        )
    })?;
    let mut game_cmd = std::process::Command::new(&cmd[0]);
    game_cmd.args(&cmd[1..]);
    // The OpenMP limit is for this program's OCR. The game gets the
    // environment Steam gave it, minus what `limit_openmp_threads` added.
    #[cfg(target_os = "linux")]
    if std::env::var_os(OMP_MARKER).is_some() {
        game_cmd.env_remove("OMP_THREAD_LIMIT").env_remove("OMP_WAIT_POLICY").env_remove(OMP_MARKER);
    }
    let mut game = game_cmd.spawn().map_err(|e| anyhow::anyhow!("launching {}: {e}", cmd[0]))?;

    // The overlay reads its stdin only for end-of-file: the wrapper holds
    // the other end for as long as the game runs, so the pipe closing means
    // "game over" whether the wrapper dropped it or died.
    // The overlay is a program of the host, not of the Steam runtime this
    // wrapper was started in: it gets the host environment back, without
    // the Steam overlay preload (which has no business in a layer-shell
    // client and is inherited by everything the overlay starts in turn).
    let overlay = std::env::current_exe().and_then(|exe| {
        khaloni_poe2::platform::host_command(exe)
            .arg(GAME_SESSION_ARG)
            .stdin(std::process::Stdio::piped())
            .spawn()
    });
    let mut overlay = match overlay {
        Ok(child) => Some(child),
        Err(e) => {
            // The game plays on without the overlay.
            eprintln!("starting the overlay: {e}");
            None
        }
    };

    let code = game.wait()?.code().unwrap_or(0);
    if let Some(child) = overlay.as_mut() {
        drop(child.stdin.take());
        // It closes on its own within a loop tick; the kill is for an
        // overlay that is wedged, so Steam is never left waiting on it.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while matches!(child.try_wait(), Ok(None)) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        if matches!(child.try_wait(), Ok(None)) {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
    Ok(code)
}

/// The overlay as `launch_mode` starts it: tied to the game session.
const GAME_SESSION_ARG: &str = "--game-session";

/// Runs the overlay until the user quits it or the launch wrapper's pipe
/// closes (see `launch_mode`). An overlay that is already running is left
/// alone, without the error a second manual start gets: the game was the
/// point of this launch.
fn game_session_mode() -> anyhow::Result<()> {
    let Ok(lock) = khaloni_poe2::platform::single_instance() else {
        eprintln!("overlay already running; leaving it be");
        return Ok(());
    };
    let game_over = Arc::new(AtomicBool::new(false));
    let flag = game_over.clone();
    std::thread::Builder::new().name("game-session-pipe".into()).spawn(move || {
        use std::io::Read;
        let mut sink = [0u8; 64];
        let mut stdin = std::io::stdin();
        while matches!(stdin.read(&mut sink), Ok(n) if n > 0) {}
        flag.store(true, Ordering::Relaxed);
    })?;
    overlay_mode(Some(lock), Some(game_over))
}

/// The game command from everything after `--launch`, tolerating an
/// optional `--` separator. `None` when nothing is left to run.
fn game_command(rest: &[String]) -> Option<&[String]> {
    let rest = match rest.first().map(String::as_str) {
        Some("--") => &rest[1..],
        _ => rest,
    };
    (!rest.is_empty()).then_some(rest)
}

/// Shows a native error dialog. Best-effort: a missing dialog helper
/// falls back to the (already-written) log line.
fn fatal_dialog(heading: &str, msg: &str) {
    let text = format!("{heading}:\n\n{msg}");
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
        // kdialog on KDE, zenity elsewhere; silent if neither works (the
        // journal/terminal already carries the message). Both are host
        // programs: started from Steam they need the host's libraries back.
        // kdialog can be installed and still fail (no Qt platform plugin
        // under the Steam runtime), which its exit status shows.
        let shown = khaloni_poe2::platform::host_command("kdialog")
            .args(["--error", &text, "--title", "khaloni-poe2"])
            .status()
            .is_ok_and(|st| st.success());
        if !shown {
            let _ = khaloni_poe2::platform::host_command("zenity")
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

/// Prices one specific cut skill gem: resolve the OCR'd name to an exact gem
/// type, item-search it at the given level, and convert the cheapest listing
/// to exalted via the currency table. `Ok(None)` when the name does not
/// resolve, there are no listings, or none of them converts; an error is
/// returned as one, so the caller can retry it instead of caching "no price".
fn price_one_gem(
    client: &mut khaloni_poe2_core::trade::TradeClient,
    skill_lower: &str,
    level: u32,
    gem_types: &[String],
    cur_id_to_name: &std::collections::HashMap<String, String>,
    table: &khaloni_poe2_core::ninja::PriceTable,
) -> Result<Option<f64>, khaloni_poe2_core::trade::TradeError> {
    let Some(name) = khaloni_poe2_core::trade::match_gem_name(skill_lower, gem_types) else {
        return Ok(None);
    };
    let listings = client.price_gem(&name, i64::from(level))?;
    // Cheapest listing that converts to exalted (search is price-asc, so the
    // first convertible one is the floor).
    Ok(listings.iter().find_map(|l| khaloni_poe2::appraise::listing_exalted(l, cur_id_to_name, table)))
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

/// Percent-encodes a string for use in a URL query (RFC 3986 unreserved
/// set kept literal; everything else percent-encoded, space as %20).
/// The trade site's search page for a query, the query itself in the link
/// (see the "Open site" action for why not an id).
fn site_search_url(league: &str, query: &khaloni_poe2_core::trade::Query) -> String {
    format!(
        "https://www.pathofexile.com/trade2/search/poe2/{}?q={}",
        urlencode(league),
        urlencode(&query.to_body().to_string())
    )
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
/// no distinct name (magic/normal). An unparseable/empty item is an error
/// for the caller to show.
fn open_resource(url_template: &str, item_text: &str) -> Result<(), String> {
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
        return Err("hover an item first".into());
    }
    let url = url_template.replace("{name}", &urlencode(name.trim()));
    open_url(&url)
}

/// Opens a URL in the default browser, per-OS. Detached spawn: the overlay
/// must never block on a browser starting up. `Err` carries what to tell
/// the user: a link that silently did not open reads as a dead button.
fn open_url(url: &str) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        // The browser is a host program and gets the host's environment
        // (see `platform::host_command`): kde-open died on the Steam
        // runtime's pinned libcurl, so nothing opened (journal 2026-09-19).
        let mut cmd = khaloni_poe2::platform::host_command("xdg-open");
        cmd.arg(url);
        spawn_detached(cmd, "xdg-open").map_err(|e| format!("could not run xdg-open: {e}"))
    }
    #[cfg(target_os = "windows")]
    {
        // `start` is a cmd builtin; the empty "" is its window-title slot so
        // a URL containing spaces is not mistaken for the title.
        // CREATE_NO_WINDOW keeps the helper cmd from flashing a console in
        // the GUI-subsystem build.
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut cmd = std::process::Command::new("cmd");
        cmd.args(["/C", "start", "", url]).creation_flags(CREATE_NO_WINDOW);
        spawn_detached(cmd, "start").map_err(|e| format!("could not open the browser: {e}"))
    }
}

/// Sets the overlay's pointer input region to the union bounding box of
/// every open interactive panel (evaluate with its hover card, market,
/// craft), or clears it when none is open. One region because the layer
/// surface supports a single rect; the union is slightly generous when
/// panels are far apart, but clicks between them still fall through to
/// nothing (hit() misses).
fn sync_input_region(
    overlay: &mut khaloni_poe2::platform::overlay::Overlay,
    renderer: &khaloni_poe2::render::Renderer,
    apanel: &Option<EvalPanel>,
    mkt_panel: &Option<(khaloni_poe2::market_ui::Panel, (i32, i32))>,
    craft_panel: &Option<CraftPanel>,
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
        // The hover card sits beside the panel (left of it when the
        // output's edge is near, so its x can be negative) and is drawn
        // there; a compositor that clips to the region would cut it off.
        if let Some(c) = &lay.card {
            boxes.push((pos.0 - out.0 + c.rect.x, pos.1 - out.1 + c.rect.y, c.rect.w as i32, c.rect.h as i32));
        }
    }
    if let Some((p, pos)) = mkt_panel {
        let lay = khaloni_poe2::market_ui::layout(p, &measure);
        boxes.push((pos.0 - out.0, pos.1 - out.1, lay.w, lay.h));
    }
    if let Some((p, pos)) = craft_panel {
        let lay = khaloni_poe2::craft_ui::layout(p, &measure);
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
    for n in cfg.map_danger_needles.iter().filter(|n| !n.trim().is_empty()) {
        r.push(khaloni_poe2_core::mapmods::ModRule {
            needle: n.to_lowercase(),
            kind: khaloni_poe2_core::mapmods::ModKind::Danger,
        });
    }
    for n in cfg.map_good_needles.iter().filter(|n| !n.trim().is_empty()) {
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
/// One window at a time: the tray fires on every click, and each click used
/// to open another copy, all editing the same config file. `Err` is for the
/// caller to show.
fn open_settings() -> Result<(), String> {
    static OPEN: AtomicBool = AtomicBool::new(false);
    if OPEN.swap(true, Ordering::AcqRel) {
        // Still running: that window is the settings window.
        return Ok(());
    }
    let started = std::env::current_exe().map_err(|e| format!("cannot find own binary: {e}")).and_then(|exe| {
        // A host program like the overlay itself (see `launch_mode`): no
        // Steam overlay preload in a window that has nothing to do with
        // the game.
        let mut cmd = khaloni_poe2::platform::host_command(exe);
        cmd.arg("--settings");
        let mut child = cmd.spawn().map_err(|e| format!("settings window: {e}"))?;
        std::thread::Builder::new()
            .name("settings-window".into())
            .spawn(move || {
                let _ = child.wait();
                OPEN.store(false, Ordering::Release);
            })
            .map_err(|e| format!("settings window: {e}"))?;
        Ok(())
    });
    if started.is_err() {
        OPEN.store(false, Ordering::Release);
    }
    started
}

/// Headless one-shot needs the Linux capture + OCR stack; the Windows
/// backend lands in SP3 (see platform/windows/mod.rs).
#[cfg(not(ocr))]
fn headless() -> anyhow::Result<()> {
    anyhow::bail!("this build has no OCR (windows-gnu check target); the shipped Windows build is MSVC with vcpkg tesseract")
}

/// The capture thread shared by the overlay and headless mode: streams
/// until the capture cannot be re-opened any more, re-opening the portal
/// session with the saved restore token whenever the stream ends. Returns
/// the receiver of its lifecycle events.
#[cfg(ocr)]
#[allow(clippy::too_many_arguments)]
fn spawn_capture(
    rt: &tokio::runtime::Runtime,
    start: capture::CaptureStart,
    region_rx: mpsc::Receiver<Rect>,
    region: Rect,
    ftx: mpsc::SyncSender<khaloni_poe2::platform::RegionFrame>,
    panel_open: Arc<AtomicBool>,
    full_tx: mpsc::SyncSender<image::GrayImage>,
    paused: Arc<AtomicBool>,
) -> anyhow::Result<mpsc::Receiver<khaloni_poe2::platform::CaptureEvent>> {
    let (cap_ev_tx, cap_ev_rx) = mpsc::channel();
    let handle = rt.handle().clone();
    let control = khaloni_poe2::platform::CaptureControl { paused, events: Some(cap_ev_tx) };
    std::thread::Builder::new().name("capture".into()).spawn(move || {
        let reopen = move || {
            // Read at each attempt: a re-open may have stored a newer token.
            let token = khaloni_poe2::config::RestoreToken::new().load();
            handle.block_on(capture::portal_session(token.as_deref()))
        };
        if let Err(e) =
            capture::consume_supervised(start, region_rx, region, ftx, panel_open, Some(full_tx), control, reopen)
        {
            eprintln!("capture ended: {e}");
        }
    })?;
    Ok(cap_ev_rx)
}

#[cfg(ocr)]
fn headless() -> anyhow::Result<()> {
    let cfg = Config::load()?;

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
    let token_store = khaloni_poe2::config::RestoreToken::new();
    let start = rt.block_on(capture::portal_session(token_store.load().as_deref()))?;
    if let Some(tok) = &start.new_token {
        token_store.save(tok)?;
    }

    // Headless works from full frames only: detect the reward panel on
    // each frame (zero calibration), crop in-process, and scan the crop.
    // The region channel is unused; the region path's throttling doesn't
    // matter because the dummy region below is never OCR'd.
    let (ftx, frx) = mpsc::sync_channel(1);
    let (_rtx, rrx) = mpsc::channel::<Rect>();
    let (full_tx, full_rx) = mpsc::sync_channel::<image::GrayImage>(1);
    let panel_open = Arc::new(AtomicBool::new(false));
    let dummy = Rect { x: 0, y: 0, w: 64, h: 64 };
    let cap_ev_rx =
        spawn_capture(&rt, start, rrx, dummy, ftx, panel_open, full_tx, Arc::new(AtomicBool::new(false)))?;
    // Keep the region channel drained so capture's try_send never backs up.
    std::thread::Builder::new().name("region-drain".into()).spawn(move || for _ in frx {})?;
    std::thread::Builder::new().name("capture-events".into()).spawn(move || {
        for ev in cap_ev_rx {
            if let khaloni_poe2::platform::CaptureEvent::NewToken(tok) = ev {
                if let Err(e) = khaloni_poe2::config::RestoreToken::new().save(&tok) {
                    eprintln!("capture: restore token not saved: {e}");
                }
            }
        }
    })?;

    eprintln!("headless pipeline running; open a Runeshape panel. Ctrl+C to quit.");
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
        // No window feed here: positions print in capture pixels, as if the
        // game window were the captured frame.
        let map = CoordMap::new(
            Rect { x: 0, y: 0, w: frame.width(), h: frame.height() },
            (frame.width(), frame.height()),
            Rect {
                x: region.x0 as i32,
                y: region.y0 as i32,
                w: region.x1 - region.x0,
                h: region.y1 - region.y0,
            },
        );
        // The UI scales with the whole frame's height, not the crop's.
        let scale = ocr::UiScale::from_frame_height(frame.height());
        let bars = ocr::reward_bars_at(&crop, &ocr::row_profile(&crop), scale);
        let lines = ocr::ocr_scan_at(&mut engine, &crop, &bars, scale);
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
    Option<(khaloni_poe2::market_ui::Panel, (i32, i32))>,
    Option<CraftPanel>,
);

/// The craft planner panel and its placed top-left (global logical px).
type CraftPanel = (khaloni_poe2::craft_ui::Panel, (i32, i32));

/// Shared placement geometry from the full-frame worker: (capture frame
/// dims, detected reward region in capture px), both None until first seen.
type ScanGeom = std::sync::Arc<std::sync::Mutex<(Option<(u32, u32)>, Option<Rect>)>>;

/// What an in-flight copy-hovered request (other than a price check) should
/// do with the copied item text once it arrives.
enum PendingAction {
    /// Open the item in the browser with this URL template. The template is
    /// the one bound to the key that was pressed (see `bindings`), not an
    /// index into a list that may have been edited since.
    Shortcut(String),
    /// Run the gear-upgrade search on the copied item.
    UpgradeCheck,
    /// Open the craft planner on the copied item.
    Craft,
}

/// Trade worker requests. Item checks carry the number of the check they
/// belong to: a check the user has already replaced with another is dropped
/// unrun instead of spending two rate-limited requests on a closed panel.
enum AppraiseReq {
    /// A fresh item: build its query and search once, exactly as built.
    Auto { item: khaloni_poe2_core::item::Item, generation: u64 },
    /// The panel's Search: the user's checkbox state (relaxed by the main
    /// loop when Broad is selected), run verbatim.
    Exact {
        title: String,
        query: khaloni_poe2_core::trade::Query,
        strictness: khaloni_poe2::evaluate_ui::Strictness,
    },
    /// Price a stackable currency (e.g. an omen) by its display name via the
    /// trade exchange; the result comes back on the exchange channel.
    /// `hover_stack` is the hovered stack's size for a user check, None for
    /// a reward row.
    Currency { name: String, hover_stack: Option<u32> },
    /// Find strictly-better listings for an equipped item: same category,
    /// every matched mod meets-or-beats the current roll, cheapest first.
    Upgrade { item: khaloni_poe2_core::item::Item, generation: u64 },
    /// Price a specific cut skill gem (reward-panel "Skill Level N: <name>")
    /// by name + level via item search; the result is written to the shared
    /// gem cache the OCR pricer reads.
    Gem { skill: String, level: u32 },
    /// What each of the panel's strongest mods is worth: one search per
    /// mod with that mod's filter dropped, against the baseline's cheapest
    /// listing. Sent on the panel's button only, never on its own.
    Attribute {
        title: String,
        /// (label, the searched query without that mod) per mod, strongest
        /// first.
        mods: Vec<(String, khaloni_poe2_core::trade::Query)>,
        /// The cheapest table listing of the search on the card, in `unit`,
        /// and its seller.
        baseline: (f64, String),
        unit: String,
        unit_per_exalted: f64,
    },
}

/// An exchange lookup's answer. The error keeps its reason and how long to
/// leave the API alone, so the popup can say "trade cooldown 12s" and the
/// cache can ask again afterwards instead of holding "no price" for the run.
struct ExchangeDone {
    /// The league the price was asked in; an answer from a league the
    /// overlay has since left is dropped on arrival.
    league: String,
    name: String,
    hover_stack: Option<u32>,
    /// How many offers stood behind the rate, when the body was read.
    offers: Option<usize>,
    outcome: Result<Option<f64>, (String, Duration)>,
}

/// Exchange prices in exalted per unit (None = nobody offers it), shared by
/// the reward-row pricer and the hover check.
type CurrencyMap =
    Arc<std::sync::Mutex<khaloni_poe2::appraise::AsyncCache<String, Option<f64>>>>;
/// Specific-gem prices in exalted (None = not priceable), written by the
/// trade worker and read (with lazy request) by the reward-panel pricer.
type GemMap =
    Arc<std::sync::Mutex<khaloni_poe2::appraise::AsyncCache<(String, u32), Option<f64>>>>;

/// What a reward row's request must clear before it is sent: the budget
/// (shared with the user's own requests, which it yields to), the search
/// limiter's free slots, and the gem-row setting. Refused, the row stays
/// "..." and a later scan asks again; nothing is queued.
#[derive(Clone)]
struct RowGate {
    budget: Arc<std::sync::Mutex<khaloni_poe2::budget::Budget>>,
    limiters: khaloni_poe2_core::trade::Limiters,
    cfg: Arc<std::sync::RwLock<Config>>,
    /// When a refusal was last logged: a refused row asks again on every
    /// scan, and one line a scan would fill the log with the same news.
    last_refusal_logged: Arc<std::sync::Mutex<Option<std::time::Instant>>>,
}

/// How often a refused row's reason goes to the log.
const ROW_REFUSAL_LOG_GAP: Duration = Duration::from_secs(30);

impl RowGate {
    fn allows(&self, kind: khaloni_poe2::appraise::RowKind) -> bool {
        let gem_rows = self.cfg.read().unwrap_or_else(|e| e.into_inner()).price_gem_rows;
        // The budget is the minute-and-longer rules; a full burst rule means
        // the request would wait, and a background request that would wait
        // is dropped.
        let search = khaloni_poe2_core::trade::Endpoint::Search;
        let free = if self.limiters.burst_free(search) == 0 { 0 } else { self.limiters.budget_free(search) };
        let now = std::time::Instant::now();
        let mut budget = self.budget.lock().unwrap_or_else(|e| e.into_inner());
        let (allowed, line) = khaloni_poe2::appraise::row_decision(kind, gem_rows, &mut budget, now, free);
        let mut last = self.last_refusal_logged.lock().unwrap_or_else(|e| e.into_inner());
        if allowed || last.is_none_or(|at| now.duration_since(at) >= ROW_REFUSAL_LOG_GAP) {
            eprintln!("{line}");
            if !allowed {
                *last = Some(now);
            }
        }
        allowed
    }
}

/// Async exchange pricer for reward rows naming currencies the ninja table
/// lacks (niche runes etc.): cache-or-request, GemCache's sibling. Lookup
/// keys are canonical vocab names. A good answer is served for
/// `appraise::GOOD_TTL` and then asked again; a failed lookup is asked again
/// after its retry wait. These requests queue behind the user's own.
struct CurrencyCache {
    map: CurrencyMap,
    req_tx: khaloni_poe2::appraise::PrioritySender<AppraiseReq>,
    gate: RowGate,
}

impl khaloni_poe2::pricing::CurrencyPricer for CurrencyCache {
    fn lookup(&self, name: &str) -> Option<khaloni_poe2::pricing::CurrencyState> {
        use khaloni_poe2::pricing::CurrencyState;
        // A blank read is no currency; asking for it only draws a refusal.
        if name.trim().is_empty() {
            return None;
        }
        let key = name.to_string();
        let mut m = self.map.lock().unwrap_or_else(|e| e.into_inner());
        let found = m.lookup(&key, std::time::Instant::now());
        if found.request
            && (!self.gate.allows(khaloni_poe2::appraise::RowKind::Currency)
                || self.req_tx.send_background(AppraiseReq::Currency { name: key.clone(), hover_stack: None }).is_err())
        {
            m.unsent(&key);
        }
        Some(match found.value {
            Some(Some(ex)) => CurrencyState::Priced(ex),
            Some(None) => CurrencyState::Unpriced,
            None => CurrencyState::Pending,
        })
    }
}

struct GemCache {
    map: GemMap,
    req_tx: khaloni_poe2::appraise::PrioritySender<AppraiseReq>,
    gate: RowGate,
}

impl khaloni_poe2::pricing::GemPricer for GemCache {
    fn lookup(&self, skill_lower: &str, level: u32) -> khaloni_poe2::pricing::GemState {
        use khaloni_poe2::pricing::GemState;
        let key = (skill_lower.to_string(), level);
        let mut m = self.map.lock().unwrap_or_else(|e| e.into_inner());
        let found = m.lookup(&key, std::time::Instant::now());
        if found.request
            && (!self.gate.allows(khaloni_poe2::appraise::RowKind::Gem)
                || self.req_tx.send_background(AppraiseReq::Gem { skill: key.0.clone(), level }).is_err())
        {
            m.unsent(&key);
        }
        match found.value {
            Some(Some(ex)) => GemState::Priced(ex),
            Some(None) => GemState::Unpriced,
            None => GemState::Pending,
        }
    }
}

/// The reward pipeline's template half, on the tracking thread: every band
/// already learned resolves in ~0.7 ms (measured on the live corpus) with
/// no tesseract.
struct TemplateResolver {
    tstore: Arc<std::sync::Mutex<khaloni_poe2::template::TemplateStore>>,
    svc: prices::PriceService,
    cfg: Arc<std::sync::RwLock<Config>>,
}

impl khaloni_poe2::reward_pipeline::Resolve for TemplateResolver {
    fn resolve(
        &mut self,
        frame: &khaloni_poe2::reward_pipeline::Frame,
        bars: &[(u32, u32)],
    ) -> khaloni_poe2::reward_pipeline::TemplatePass {
        let snap = self.svc.snapshot();
        let cfg = self.cfg.read().unwrap_or_else(|e| e.into_inner()).clone();
        let store = self.tstore.lock().unwrap_or_else(|e| e.into_inner());
        let mut pass = khaloni_poe2::reward_pipeline::TemplatePass {
            league: Some(snap.league.clone()),
            stale: snap.stale,
            ..Default::default()
        };
        for &(y0, y1) in bars {
            let hit = ocr::band_crop_at(&frame.gray, y0, y1, frame.scale).and_then(|crop| store.lookup(&crop));
            // A template cannot tell "3x" from "8x" on its own: until OCR
            // has agreed once for this exact crop, the band is read too.
            if let Some(ticket) = hit.as_ref().and_then(|h| h.verify) {
                pass.tickets.push((y0, ticket));
            }
            let row = hit.and_then(|hit| {
                pricing::price_resolved(
                    &snap.table,
                    &hit.item_key,
                    hit.count,
                    hit.count_explicit,
                    y0 * ocr::UPSCALE,
                    (y1 - y0) * ocr::UPSCALE,
                    &cfg,
                )
            });
            match row {
                Some(r) => pass.rows.push(r),
                None => pass.unresolved = true,
            }
        }
        pass
    }
}

/// The reward pipeline's tesseract half, on its own thread: reads the bars
/// of the newest frame that needs it, prices the lines, settles the
/// template confirmations and teaches the store.
struct RewardReader {
    engine: ocr::OcrEngine,
    /// What tesseract already read, by pixel content: an unchanged panel
    /// costs no OCR at all (see ocr::ScanCache).
    scan_cache: ocr::ScanCache,
    tstore: Arc<std::sync::Mutex<khaloni_poe2::template::TemplateStore>>,
    tpl_path: Option<std::path::PathBuf>,
    tpl_saved_at: std::time::Instant,
    svc: prices::PriceService,
    cfg: Arc<std::sync::RwLock<Config>>,
    exch_names: Arc<std::sync::OnceLock<Vec<String>>>,
    /// Match vocab = price-table names + exchange catalog (async-published),
    /// rebuilt only when either side actually changes.
    vocab: Option<((usize, usize), pricing::Vocab)>,
    rumours: Option<khaloni_poe2_core::rumour::RumourIndex>,
    gem_cache: GemCache,
    currency_cache: CurrencyCache,
    dbg: bool,
    t0: std::time::Instant,
}

impl khaloni_poe2::reward_pipeline::ReadRows for RewardReader {
    fn read(&mut self, job: &khaloni_poe2::reward_pipeline::OcrJob) -> Option<khaloni_poe2::reward_pipeline::ReadOut> {
        let t = std::time::Instant::now();
        let (gray, scale) = (&job.frame.gray, job.frame.scale);
        let snap = self.svc.snapshot();
        let cfg = self.cfg.read().unwrap_or_else(|e| e.into_inner()).clone();
        let runs_before = self.scan_cache.ocr_runs;
        let lines = self.scan_cache.scan_at(&mut self.engine, gray, &job.bars, job.with_whole, scale);
        if self.dbg {
            let secs = self.t0.elapsed().as_secs_f32();
            eprintln!("TRACE {secs:>8.2}s ocr passes={} lines={}", self.scan_cache.ocr_runs - runs_before, lines.len());
            let d = std::env::temp_dir().join("khalonipoe2-frames");
            let _ = std::fs::create_dir_all(&d);
            let _ = gray.save(d.join(format!("t{secs:06.2}_bands{}_lines{}.png", job.bars.len(), lines.len())));
        }
        let extra = self.exch_names.get().map(|v| v.as_slice()).unwrap_or(&[]);
        let key = (snap.table.len(), extra.len());
        if self.vocab.as_ref().is_none_or(|(k, _)| *k != key) {
            self.vocab = Some((key, pricing::build_vocab_with(&snap.table, extra)));
        }
        let vocab = self.vocab.as_ref().map_or(&snap.vocab, |(_, v)| v);
        let out = pricing::price_lines_with_rumours(
            &snap.table,
            vocab,
            &lines,
            &cfg,
            self.rumours.as_ref(),
            Some(&self.gem_cache),
            Some(&self.currency_cache),
        );
        // Template rows priced under another league's table are not this
        // scan's to show.
        let mut resolved = if job.templates.league.as_deref() == Some(snap.league.as_str()) {
            job.templates.rows.clone()
        } else {
            Vec::new()
        };
        {
            let mut store = self.tstore.lock().unwrap_or_else(|e| e.into_inner());
            // Settle the owed confirmations against what OCR read off the
            // same bands. A template OCR disagrees with is removed by
            // `confirm`, and its row leaves `resolved` so the OCR row is
            // the one shown; the learn loop below re-teaches the band.
            for &(y0, ticket) in &job.templates.tickets {
                let y_top = y0 * ocr::UPSCALE;
                let read = out.0.iter().find(|r| r.y_top == y_top).map(|r| (r.item_key.as_str(), r.count));
                if !store.confirm(ticket, read) {
                    resolved.retain(|r| r.y_top != y_top);
                }
            }
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
                if let Some(&(y0, y1)) = job.bars.iter().find(|&&(y0, _)| y0 * ocr::UPSCALE == r.y_top) {
                    if let Some(crop) = ocr::band_crop_at(gray, y0, y1, scale) {
                        store.learn(&r.item_key, r.count, r.count_explicit, &crop);
                    }
                }
            }
            if store.dirty && self.tpl_saved_at.elapsed().as_secs() >= 30 {
                if let Some(p) = self.tpl_path.as_deref() {
                    let _ = store.save(p);
                }
                self.tpl_saved_at = std::time::Instant::now();
            }
        }
        // Merge template-resolved rows with the OCR pass: a resolved row
        // wins over any OCR row overlapping its y range.
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
        // Bands were present but nothing priced (tooltip occlusion,
        // mid-transition frame): plain empty rows, which the stabilizer
        // rides out with its occlusion tolerance.
        if self.dbg {
            eprintln!(
                "TRACE {:>8.2}s ocr_done in {:?}: {} lines -> {} rows [{}]",
                self.t0.elapsed().as_secs_f32(),
                t.elapsed(),
                lines.len(),
                merged.len(),
                merged.iter().map(|r| format!("{}@y{}", r.item_key, r.y_top)).collect::<Vec<_>>().join(", ")
            );
        }
        Some(khaloni_poe2::reward_pipeline::ReadOut { rows: merged, league: Some(snap.league.clone()), stale: snap.stale })
    }
}

/// The item-card facts the Evaluate header shows, read off the parsed item
/// in the trade worker. They travel with the response because the panel is
/// built on the main loop, which only ever sees the query and the labels —
/// and a header must state what the item says, not a plausible default.
#[derive(Clone)]
struct ItemFacts {
    /// "Rare", "Magic", … exactly as the item text words it.
    rarity: String,
    item_level: Option<u32>,
    requires_level: Option<u32>,
    /// The item's computed figures (DPS, defences, spirit, sockets) with
    /// their search floors and default state (see core::props).
    props: Vec<khaloni_poe2_core::props::PropFilter>,
    /// Chaos DPS, which the trade site cannot filter on: shown, never
    /// searched.
    chaos_dps: f64,
}

impl ItemFacts {
    /// `props` are the figures the search was built with (see
    /// `ee2::Built::props`). `searched` keeps each one's default state (a
    /// price check); without it every property is offered switched off (an
    /// upgrade search, whose bounds are the user's to raise).
    fn read(
        item: &khaloni_poe2_core::item::Item,
        mut props: Vec<khaloni_poe2_core::props::PropFilter>,
        searched: bool,
    ) -> ItemFacts {
        if !searched {
            for p in &mut props {
                p.enabled = false;
            }
        }
        ItemFacts {
            rarity: rarity_label(&item.rarity),
            item_level: item.item_level,
            requires_level: requires_level(item),
            props,
            chaos_dps: khaloni_poe2_core::derived::weapon_stats(item).map_or(0.0, |w| w.chaos_dps),
        }
    }
}

/// A trade worker response. A check that opens a panel sends two: the
/// `Seed` the moment the query is built, so the panel shows every row
/// while the search runs, then the `Result` with the listings. A Search
/// press sends only a `Result`. Every request is answered, failures
/// included: a press that gets nothing back looks like a dead key.
enum AppraiseDone {
    /// Opens the panel: rows, the query its checkboxes edit (boxed: it is
    /// the bulk of the message), header facts.
    Seed {
        title: String,
        query: Box<khaloni_poe2_core::trade::Query>,
        labels: Vec<khaloni_poe2_core::trade::FilterLabel>,
        facts: Option<ItemFacts>,
        /// Rows with no searchable stat that are no line of the item (a
        /// total without a trade id): listed on the card so the user sees
        /// they are not part of the search.
        unsearchable: Vec<String>,
        /// Every modifier line EE2 gives no row of its own, as a row the
        /// user can tick into the search.
        extra: Vec<khaloni_poe2_core::ee2::request::ExtraRow>,
    },
    /// Listings for the panel with this title. Boxed: a search's answer
    /// carries every block of the card and dwarfs the other variants.
    Result {
        title: String,
        outcome: Result<Box<SearchDone>, String>,
        /// What the search was run as, for the status line.
        strictness: khaloni_poe2::evaluate_ui::Strictness,
    },
    /// What each mod is worth, for the panel with this title.
    Attributed { title: String, outcome: Result<Vec<khaloni_poe2::evaluate_ui::AttributionRow>, String> },
    /// Something the user should read that belongs to no panel (a reward
    /// row's gem could not be priced, and why).
    Note(String),
}

/// A search that ran, with everything the panel shows for it.
#[derive(Clone)]
struct SearchDone {
    /// None for a stackable's exchange check, which has no search to open.
    search_id: Option<String>,
    /// The query that was sent, with Broad's relaxed bounds when that is
    /// what ran. "Open site" opens this one, so the browser shows the
    /// search the listings came from.
    searched: khaloni_poe2_core::trade::Query,
    /// Set when this is a recent identical search's answer being shown
    /// again instead of a new request.
    reused_age: Option<Duration>,
    /// The league the listings were found in and converted at.
    league: String,
    /// The table, the ladder, the price-fixed strip, the closest listings.
    blocks: khaloni_poe2::appraise::Blocks,
    /// poe.ninja's line for the item, when it tracks it.
    ninja: Option<khaloni_poe2::evaluate_ui::NinjaBlock>,
    /// The exchange offers and the stack's worth, for a stackable.
    bulk: Option<khaloni_poe2::evaluate_ui::BulkBlock>,
    stack_value: Option<String>,
    /// "searches 4/30 (5 min)" and whether it is near the cap.
    budget: (String, bool),
    attribute_enabled: bool,
}

/// What the panel's listings came from, kept beside the panel.
struct Searched {
    query: khaloni_poe2_core::trade::Query,
    /// "Open site" opens the search in this league, the one the listings
    /// on the card came from, whatever the overlay prices in by then.
    league: String,
    /// The cheapest table listing and its seller, in `unit`: the baseline
    /// the attribution searches compare against.
    cheapest: Option<(f64, String)>,
    unit: String,
    unit_per_exalted: f64,
}

/// Where a request's answer goes, worked out before the request runs so a
/// panic while running it can still be answered.
enum ReplyTo {
    Panel { title: String, seeds: bool, strictness: khaloni_poe2::evaluate_ui::Strictness },
    Exchange { name: String, hover_stack: Option<u32> },
    Gem { skill: String, level: u32 },
    Attribution { title: String },
}

fn item_title(item: &khaloni_poe2_core::item::Item) -> String {
    if item.name.is_empty() {
        item.base_type.clone().unwrap_or_default()
    } else {
        item.name.clone()
    }
}

impl AppraiseReq {
    fn reply_to(&self) -> ReplyTo {
        use khaloni_poe2::evaluate_ui::Strictness;
        match self {
            AppraiseReq::Auto { item, .. } => {
                ReplyTo::Panel { title: item_title(item), seeds: true, strictness: Strictness::Quick }
            }
            AppraiseReq::Upgrade { item, .. } => ReplyTo::Panel {
                title: khaloni_poe2_core::trade::upgrade_title(item),
                seeds: true,
                strictness: Strictness::Quick,
            },
            AppraiseReq::Exact { title, strictness, .. } => {
                ReplyTo::Panel { title: title.clone(), seeds: false, strictness: *strictness }
            }
            AppraiseReq::Currency { name, hover_stack } => {
                ReplyTo::Exchange { name: name.clone(), hover_stack: *hover_stack }
            }
            AppraiseReq::Gem { skill, level } => ReplyTo::Gem { skill: skill.clone(), level: *level },
            AppraiseReq::Attribute { title, .. } => ReplyTo::Attribution { title: title.clone() },
        }
    }
}

/// The last item check's own side, kept for the searches the panel runs
/// after it: the built item the closest listings are compared with, and
/// what poe.ninja is asked for.
struct Checked {
    title: String,
    built: khaloni_poe2_core::ee2::request::Built,
    /// (name, base when the item has a name of its own, corrupted).
    ninja: (String, Option<String>, bool),
}

/// The trade site's catalogs a price check needs. Each loads on first use
/// through `TradeClient::cached_data` (validated, written atomically, kept
/// for a day) and is retried with a growing wait when it fails.
struct Catalogs {
    /// Body of data/stats, and the index built from it.
    stats: khaloni_poe2::appraise::Retrying<(String, khaloni_poe2_core::trade::StatIndex)>,
    /// From data/static: display name -> exchange id, and the reverse.
    currencies: khaloni_poe2::appraise::Retrying<(
        std::collections::HashMap<String, String>,
        std::collections::HashMap<String, String>,
    )>,
    /// Body of data/items, and the gem type names in it.
    items: khaloni_poe2::appraise::Retrying<(String, Vec<String>)>,
}

/// How long a catalog on disk is used before it is downloaded again. They
/// change with a game patch, not by the hour.
const CATALOG_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// Everything the trade worker thread owns.
struct TradeWorker {
    done_tx: mpsc::Sender<AppraiseDone>,
    exch_tx: mpsc::Sender<ExchangeDone>,
    /// The league the client is pointed at and this worker's answers are
    /// in. It follows `current` between requests, never during one.
    league: String,
    current: khaloni_poe2::league::Current,
    cache_dir: std::path::PathBuf,
    gem_map: GemMap,
    svc: prices::PriceService,
    exch_names: Arc<std::sync::OnceLock<Vec<String>>>,
    reference: Arc<std::sync::OnceLock<khaloni_poe2::refcache::Reference>>,
    generation: Arc<std::sync::atomic::AtomicU64>,
    client: khaloni_poe2::appraise::Retrying<khaloni_poe2_core::trade::TradeClient>,
    catalogs: Catalogs,
    ee2: khaloni_poe2::appraise::Retrying<khaloni_poe2_core::ee2::Ee2Data>,
    /// The last search that ran, by request body.
    last_search: khaloni_poe2::appraise::ReuseSlot<String, SearchDone>,
    /// The last item check, for the panel's later searches.
    last_check: Option<Checked>,
    /// The config as last saved: the display threshold, the market floors
    /// and the account name the listings are read with.
    cfg: Arc<std::sync::RwLock<Config>>,
    /// Where every price check's fetched listings go, for the observed
    /// craft model. The send never waits: the craft worker records them.
    craft_tx: mpsc::Sender<CraftReq>,
}

impl TradeWorker {
    fn run(mut self, rx: khaloni_poe2::appraise::PriorityReceiver<AppraiseReq>) {
        // The catalogs load before anyone asks: the reward-row matcher
        // needs the exchange names to recognise a row at all, and a row it
        // cannot recognise never sends the request that would load them.
        self.load_catalogs();
        loop {
            let req = match rx.recv_timeout(Duration::from_secs(5)) {
                Ok(req) => req,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // A quiet moment: retry whatever failed to load (each
                    // catalog keeps its own backoff).
                    self.load_catalogs();
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            };
            let reply = req.reply_to();
            // One bad item must not end trade pricing for the session: a
            // panic is answered like any other failure and the loop goes on.
            let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.handle(req)));
            if let Err(panic) = ran {
                let why = panic
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "unknown panic".into());
                eprintln!("trade worker: request panicked: {why}");
                self.fail(reply, format!("internal error: {why}"), khaloni_poe2::appraise::ERROR_RETRY);
            }
        }
    }

    /// Answers a request with a failure, wherever its answer goes.
    fn fail(&mut self, reply: ReplyTo, why: String, retry: Duration) {
        match reply {
            ReplyTo::Panel { title, seeds, strictness } => {
                // The card opens to say why: a failed check that opened
                // nothing looked like the hotkey was dead.
                if seeds {
                    let _ = self.done_tx.send(AppraiseDone::Seed {
                        title: title.clone(),
                        query: Default::default(),
                        labels: Vec::new(),
                        facts: None,
                        unsearchable: Vec::new(),
                        extra: Vec::new(),
                    });
                }
                let _ = self.done_tx.send(AppraiseDone::Result { title, outcome: Err(why), strictness });
            }
            ReplyTo::Exchange { name, hover_stack } => {
                // A hovered stack's card is open by now: the reason goes on
                // it, and the cache learns the failure like a row's.
                if hover_stack.is_some() {
                    let _ = self.done_tx.send(AppraiseDone::Result {
                        title: name.clone(),
                        outcome: Err(why.clone()),
                        strictness: khaloni_poe2::evaluate_ui::Strictness::Quick,
                    });
                }
                let _ = self.exch_tx.send(ExchangeDone {
                    league: self.league.clone(),
                    name,
                    hover_stack,
                    offers: None,
                    outcome: Err((why, retry)),
                });
            }
            ReplyTo::Gem { skill, level } => {
                khaloni_poe2::league::store_if_still(
                    &self.current,
                    &self.league,
                    &self.gem_map,
                    (skill.clone(), level),
                    Err((why.clone(), retry)),
                    std::time::Instant::now(),
                );
                let _ = self.done_tx.send(AppraiseDone::Note(format!("{skill} {level} not priced: {why}")));
            }
            ReplyTo::Attribution { title } => {
                let _ = self.done_tx.send(AppraiseDone::Attributed { title, outcome: Err(why) });
            }
        }
    }

    /// Between requests only: a request runs in one league from its first
    /// byte to its answer. A request queued before a league change runs in
    /// the new league, which is what the user is looking at by then.
    fn follow_league(&mut self) {
        let now = self.current.name();
        if now != self.league {
            eprintln!("trade worker: league {} -> {now}", self.league);
            self.league = now;
            khaloni_poe2::league::retarget(&mut self.client, &mut self.last_search, &self.league);
        }
    }

    /// Why nothing may be converted right now, if so: the price table is
    /// another league's or has not arrived.
    fn prices_not_ready(&self) -> Option<String> {
        khaloni_poe2::league::not_ready(&self.svc.snapshot(), &self.league)
    }

    /// The trade client, with the POESESSID saved in settings.
    fn client(&mut self) -> Result<&mut khaloni_poe2_core::trade::TradeClient, String> {
        let league = self.league.clone();
        let session = self.cfg.read().unwrap_or_else(|e| e.into_inner()).poesessid.clone();
        khaloni_poe2::appraise::session_client(&mut self.client, &session, std::time::Instant::now(), || {
            khaloni_poe2_core::trade::TradeClient::new(khaloni_poe2_core::trade::TRADE_BASE, &league)
                .map_err(|e| format!("trade client unavailable: {e}"))
        })
    }

    /// Loads whichever catalogs are still missing and due for an attempt.
    /// Failures are logged here and surface where the catalog is needed.
    fn load_catalogs(&mut self) {
        use khaloni_poe2_core::trade::{self, TradeData};
        let now = std::time::Instant::now();
        let dir = self.cache_dir.clone();
        let Ok(client) = self.client.get_or_load(now, || {
            trade::TradeClient::new(trade::TRADE_BASE, &self.league).map_err(|e| format!("trade client unavailable: {e}"))
        }) else {
            return;
        };
        let client: &trade::TradeClient = client;
        let fetch = |kind: TradeData, file: &str| {
            client.cached_data(kind, &dir.join(file), CATALOG_MAX_AGE).map_err(|e| e.to_string())
        };
        if self.catalogs.stats.attempt_due(now) {
            let r = self.catalogs.stats.get_or_load(now, || {
                let body = fetch(TradeData::Stats, "trade_stats.json")?;
                let index = trade::StatIndex::from_json(&body).map_err(|e| e.to_string())?;
                Ok((body, index))
            });
            if let Err(e) = r {
                eprintln!("trade stats catalog: {e}");
            }
        }
        if self.catalogs.currencies.attempt_due(now) {
            let r = self.catalogs.currencies.get_or_load(now, || {
                let ids = trade::parse_static_currency_ids(&fetch(TradeData::Static, "trade_static.json")?);
                let names = ids.iter().map(|(name, id)| (id.clone(), name.clone())).collect();
                Ok((ids, names))
            });
            match r {
                // Published once: the OCR worker extends its match vocab
                // with the exchange catalog's names.
                Ok((ids, _)) => {
                    let _ = self.exch_names.set(ids.keys().cloned().collect());
                }
                Err(e) => eprintln!("trade currency catalog: {e}"),
            }
        }
        if self.catalogs.items.attempt_due(now) {
            let r = self.catalogs.items.get_or_load(now, || {
                let body = fetch(TradeData::Items, "trade_items.json")?;
                let gems = trade::parse_gem_types(&body);
                Ok((body, gems))
            });
            if let Err(e) = r {
                eprintln!("trade items catalog: {e}");
            }
        }
    }

    /// EE2's stat and item data, which the price-check search is built
    /// from. A failed load is tried again on a later check.
    fn load_ee2(&mut self) -> Result<(), String> {
        use khaloni_poe2_core::ee2::data;
        let now = std::time::Instant::now();
        if !self.ee2.is_loaded() {
            // The reference loader downloads the same files at startup;
            // give it a moment so both do not fetch them at once, but never
            // wait on it without a limit (it may have died).
            let deadline = now + Duration::from_secs(20);
            while self.reference.get().is_none() && std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        let dir = self.cache_dir.clone();
        let d = self.ee2.get_or_load(now, || {
            khaloni_poe2::refcache::try_ee2_data(&dir)
                .map_err(|e| format!("EE2 reference data unavailable ({e}); the search cannot be built"))
        })?;
        // The trade catalogs may arrive after the EE2 data did.
        if d.trade_stats.is_none() {
            d.trade_stats =
                self.catalogs.stats.get().and_then(|(body, _)| data::TradeStatTexts::from_json(body).ok());
        }
        if d.trade_items.is_none() {
            d.trade_items = self.catalogs.items.get().and_then(|(body, _)| data::trade_item_names(body).ok());
        }
        Ok(())
    }

    fn handle(&mut self, req: AppraiseReq) {
        use khaloni_poe2::appraise;
        let reply = req.reply_to();
        self.follow_league();
        self.load_catalogs();
        match req {
            AppraiseReq::Currency { name, hover_stack: None } => {
                let outcome = self.exchange_price(&name).map(|a| a.map(|(ex, _)| ex));
                let _ = self.exch_tx.send(ExchangeDone {
                    league: self.league.clone(),
                    name,
                    hover_stack: None,
                    offers: None,
                    outcome,
                });
            }
            // The user's own check of a stack: the card opens with the
            // exchange offers and the stack's worth; the cache learns the
            // rate like a row's answer, so a reward row naming the same
            // currency costs nothing more.
            AppraiseReq::Currency { name, hover_stack: Some(stack) } => {
                let _ = self.done_tx.send(AppraiseDone::Seed {
                    title: name.clone(),
                    query: Default::default(),
                    labels: Vec::new(),
                    facts: Some(ItemFacts {
                        rarity: "Currency".to_string(),
                        item_level: None,
                        requires_level: None,
                        props: Vec::new(),
                        chaos_dps: 0.0,
                    }),
                    unsearchable: Vec::new(),
                    extra: Vec::new(),
                });
                let answer = self.exchange_price(&name);
                let offers = answer.as_ref().ok().and_then(|a| a.as_ref()).map(|(_, view)| view.offers.len());
                let outcome = match &answer {
                    Ok(Some((ex, view))) => Ok(Box::new(self.stack_done(*ex, view, stack))),
                    Ok(None) => Err("no exchange offers right now".to_string()),
                    Err((why, _)) => Err(why.clone()),
                };
                let _ = self.done_tx.send(AppraiseDone::Result {
                    title: name.clone(),
                    outcome,
                    strictness: khaloni_poe2::evaluate_ui::Strictness::Quick,
                });
                let _ = self.exch_tx.send(ExchangeDone {
                    league: self.league.clone(),
                    name,
                    hover_stack: Some(stack),
                    offers,
                    outcome: answer.map(|a| a.map(|(ex, _)| ex)),
                });
            }
            AppraiseReq::Attribute { title, mods, baseline, unit, unit_per_exalted } => {
                let outcome = self.attribute(&mods, &baseline, &unit, unit_per_exalted);
                let _ = self.done_tx.send(AppraiseDone::Attributed { title, outcome });
            }
            AppraiseReq::Gem { skill, level } => {
                let outcome = self.gem_price(&skill, level);
                if let Err((why, _)) = &outcome {
                    let _ = self.done_tx.send(AppraiseDone::Note(format!("{skill} {level} not priced: {why}")));
                }
                khaloni_poe2::league::store_if_still(
                    &self.current,
                    &self.league,
                    &self.gem_map,
                    (skill, level),
                    outcome,
                    std::time::Instant::now(),
                );
            }
            AppraiseReq::Auto { item, generation } => {
                if generation < self.generation.load(Ordering::Acquire) {
                    return; // the user has checked another item since
                }
                let title = item_title(&item);
                let built = self.load_ee2().and_then(|()| {
                    let data = self.ee2.get().expect("loaded by load_ee2");
                    khaloni_poe2_core::ee2::build(&item.raw, data).map_err(|e| e.to_string())
                });
                // An item the EE2 data cannot place gets no search at all:
                // a loosened one would price something else. The card
                // opens to say why.
                let mut built = match built {
                    Ok(b) => b,
                    Err(e) => return self.fail(reply, e, appraise::ERROR_RETRY),
                };
                // The extra rows take their ids from the site's own catalog
                // when it is loaded; without it they keep the pinned data's.
                if let Some((_, catalog)) = self.catalogs.stats.get() {
                    built.resolve_extra(catalog);
                }
                // Header facts are read here, while the parsed item still
                // exists; the main loop never sees it.
                let unreadable = built.reads_no_modifier();
                let facts = ItemFacts::read(&item, built.props.clone(), true);
                // What poe.ninja is asked for: a unique by its name and
                // base, anything else by the name it goes by.
                let ninja = if item.name.is_empty() {
                    (item.base_type.clone().unwrap_or_default(), None, hover::is_corrupted(&item.raw))
                } else {
                    (item.name.clone(), item.base_type.clone(), hover::is_corrupted(&item.raw))
                };
                self.last_check = Some(Checked { title: title.clone(), built: built.clone(), ninja });
                let unsearchable = built.unsearchable_totals();
                // Nothing on this item could be read into the search: the
                // listings would be "any item of this category" and a price
                // from them would be a number about something else. The
                // card opens with every line on it and says why it is not
                // priced.
                if unreadable {
                    let _ = self.done_tx.send(AppraiseDone::Seed {
                        title: title.clone(),
                        query: Box::new(built.query),
                        labels: built.labels,
                        facts: Some(facts),
                        unsearchable,
                        extra: built.extra,
                    });
                    let _ = self.done_tx.send(AppraiseDone::Result {
                        title,
                        outcome: Err("not priced: the item text is the simple format, so no modifier \
                                      could be read. Chat-linked items copy this way; for your own \
                                      items turn on advanced mod descriptions"
                            .into()),
                        strictness: khaloni_poe2::evaluate_ui::Strictness::Quick,
                    });
                    return;
                }
                self.seed_and_search(title, built.query, built.labels, facts, unsearchable, built.extra);
            }
            AppraiseReq::Upgrade { item, generation } => {
                if generation < self.generation.load(Ordering::Acquire) {
                    return;
                }
                // An upgrade search is not this item's price: the listings
                // are compared with nothing and poe.ninja is not asked.
                self.last_check = None;
                let Some((_, stats)) = self.catalogs.stats.get() else {
                    let why = format!(
                        "upgrade search needs the trade stats catalog: {}",
                        self.catalogs.stats.last_error()
                    );
                    return self.fail(reply, why, appraise::ERROR_RETRY);
                };
                let (q, labels) = khaloni_poe2_core::trade::build_upgrade_query_with_labels(&item, stats);
                let title = khaloni_poe2_core::trade::upgrade_title(&item);
                // The figures are a nicety on this card; without the EE2
                // data the search still runs on the mods.
                let props = match self.load_ee2() {
                    Ok(()) => khaloni_poe2_core::ee2::build(&item.raw, self.ee2.get().expect("loaded"))
                        .map(|b| b.props)
                        .unwrap_or_default(),
                    Err(_) => Vec::new(),
                };
                let facts = ItemFacts::read(&item, props, false);
                self.seed_and_search(title, q, labels, facts, Vec::new(), Vec::new());
            }
            AppraiseReq::Exact { title, query, strictness } => {
                let outcome = self.search(&query, &title).map(Box::new);
                let _ = self.done_tx.send(AppraiseDone::Result { title, outcome, strictness });
            }
        }
    }

    /// The panel opens now, with every row, before the search: the user
    /// reads and adjusts the selection while the listings are fetched
    /// instead of waiting on the request to see the mods at all.
    fn seed_and_search(
        &mut self,
        title: String,
        query: khaloni_poe2_core::trade::Query,
        labels: Vec<khaloni_poe2_core::trade::FilterLabel>,
        facts: ItemFacts,
        unsearchable: Vec<String>,
        extra: Vec<khaloni_poe2_core::ee2::request::ExtraRow>,
    ) {
        let _ = self.done_tx.send(AppraiseDone::Seed {
            title: title.clone(),
            query: Box::new(query.clone()),
            labels,
            facts: Some(facts),
            unsearchable,
            extra,
        });
        let outcome = self.search(&query, &title).map(Box::new);
        let _ = self.done_tx.send(AppraiseDone::Result {
            title,
            outcome,
            strictness: khaloni_poe2::evaluate_ui::Strictness::Quick,
        });
    }

    /// One search, exactly as given, and its listings: the table's twenty
    /// in two fetch calls, up to forty when the closest listings are wanted
    /// and the search matched more than twenty (the pages past the table
    /// are best effort: a cooldown there costs the comparison, not the
    /// check). A query that finds nothing says so: dropping filters until
    /// something matched is how a price check came back with items nothing
    /// like the one checked (observed live 2026-09-18). The same request
    /// body within `appraise::REUSE_TTL` is answered from the previous
    /// result.
    fn search(&mut self, q: &khaloni_poe2_core::trade::Query, title: &str) -> Result<SearchDone, String> {
        use khaloni_poe2::appraise;
        // Before the request, not after: a search costs a slot of the
        // site's rate limit, and its listings could not be converted.
        if let Some(why) = self.prices_not_ready() {
            return Err(why);
        }
        let body = q.to_body().to_string();
        let now = std::time::Instant::now();
        if let Some((mut done, age)) = self.last_search.get(&body, now) {
            eprintln!("trade search: same request {}s ago, shown again", age.as_secs());
            done.reused_age = Some(age);
            done.budget = self.budget_text();
            done.attribute_enabled = self.attribute_enabled();
            return Ok(done);
        }
        let wants_closest = self.last_check.as_ref().is_some_and(|c| c.title == title && appraise::wants_closest(&c.built));
        let client = self.client()?;
        let fetched = client.search(q).and_then(|s| {
            let plan = appraise::fetch_plan(s.total, s.hashes.len(), wants_closest);
            let mut raw: Vec<Option<serde_json::Value>> = Vec::new();
            let mut dropped = 0;
            for (page, range) in plan.into_iter().enumerate() {
                // Fetching an empty id list 404s, so an empty search stops
                // here; the plan never yields an empty page.
                match client.fetch_counted(&s.id, &s.hashes[range]) {
                    Ok(outcome) => {
                        dropped += outcome.dropped;
                        raw.extend(outcome.raw);
                    }
                    Err(e) if page >= appraise::TABLE_LISTINGS / appraise::PAGE => {
                        eprintln!(
                            "trade fetch (page {} for the closest listings): {}",
                            page + 1,
                            appraise::error_text(&e)
                        );
                        break;
                    }
                    Err(e) => return Err(e),
                }
            }
            Ok((s.id, s.total, raw, dropped))
        });
        // The outcome joins the "price check:" line in the log: a report of
        // failed checks could not be told apart from a cooldown, a refused
        // body or a dead connection without it.
        let (search_id, total, raw, dropped) = match fetched {
            Ok(f) => f,
            Err(e) => {
                let why = appraise::error_text(&e);
                eprintln!("trade search: {why}");
                return Err(why);
            }
        };
        eprintln!(
            "trade search: {} listings fetched ({dropped} gone or unpriced, {:?} matches)",
            raw.len() - dropped,
            total
        );
        // The listings' mods and tiers feed the observed craft model; the
        // craft worker joins them to the mod data off this thread.
        let _ = self.craft_tx.send(CraftReq::Record { league: self.league.clone(), raw: raw.clone() });
        let snap = self.svc.snapshot();
        // The listings are `self.league`'s. Converting them at another
        // league's rates gives a number that looks like any other.
        if snap.league != self.league || !self.current.is(&self.league) {
            return Err(format!("the league changed to {} during the search: search again", self.current.name()));
        }
        let empty = std::collections::HashMap::new();
        let names = self.catalogs.currencies.get().map(|(_, names)| names).unwrap_or(&empty);
        let (divine_threshold, floors, my_account) = {
            let cfg = self.cfg.read().unwrap_or_else(|e| e.into_inner());
            (cfg.divine_threshold, cfg.market_floors(), cfg.account_name.clone())
        };
        let now_unix = prices::unix_now();
        let checked = self.last_check.as_ref().filter(|c| c.title == title);
        let blocks = appraise::blocks(&appraise::Fetched {
            raw: &raw,
            total,
            now_unix,
            my_account: &my_account,
            currency_names: names,
            table: &snap.table,
            divine_threshold,
            built: checked.map(|c| &c.built),
        });
        let ninja = checked.and_then(|c| {
            let model = appraise::market_model(&self.svc.market(), floors, now_unix);
            let (name, base, corrupted) = &c.ninja;
            khaloni_poe2_core::market::ninja_block(&model, name, base.as_deref(), *corrupted, None)
                .map(|b| appraise::ninja_view(&b, &snap.table, divine_threshold))
        });
        let done = SearchDone {
            search_id: Some(search_id),
            searched: q.clone(),
            reused_age: None,
            league: self.league.clone(),
            blocks,
            ninja,
            bulk: None,
            stack_value: None,
            budget: self.budget_text(),
            attribute_enabled: self.attribute_enabled(),
        };
        self.last_search.put(body, done.clone(), now);
        Ok(done)
    }

    /// The panel's budget line: the five-minute rule as the last search
    /// response reported it, else the limiter's tightest rule.
    fn budget_text(&self) -> (String, bool) {
        let fallback =
            self.client.get().map(|c| c.limiters().budget_text(khaloni_poe2_core::trade::Endpoint::Search));
        khaloni_poe2::appraise::budget_text(fallback.as_deref().unwrap_or(""))
    }

    fn free_slots(&self) -> u32 {
        self.client.get().map_or(0, |c| c.limiters().budget_free(khaloni_poe2_core::trade::Endpoint::Search))
    }

    fn attribute_enabled(&self) -> bool {
        khaloni_poe2::appraise::attribute_enabled(self.free_slots())
    }

    /// The card's answer for a hovered stack: the bulk offers and the
    /// stack's worth at the going rate.
    fn stack_done(&self, per_unit_exalted: f64, view: &khaloni_poe2_core::bulk::BulkView, stack: u32) -> SearchDone {
        let snap = self.svc.snapshot();
        let empty = std::collections::HashMap::new();
        let names = self.catalogs.currencies.get().map(|(_, names)| names).unwrap_or(&empty);
        let divine_threshold = self.cfg.read().unwrap_or_else(|e| e.into_inner()).divine_threshold;
        SearchDone {
            search_id: None,
            searched: Default::default(),
            reused_age: None,
            league: self.league.clone(),
            blocks: Default::default(),
            ninja: None,
            bulk: Some(khaloni_poe2::appraise::bulk_block(view, names)),
            stack_value: Some(khaloni_poe2::appraise::stack_value_text(
                stack,
                per_unit_exalted,
                &snap.table,
                divine_threshold,
            )),
            budget: self.budget_text(),
            attribute_enabled: false,
        }
    }

    /// One search per mod with its filter dropped, each fetched once (its
    /// cheapest page), against the baseline. Refused before the first
    /// request when the budget has fewer than `ATTRIBUTE_MIN_FREE` slots.
    fn attribute(
        &mut self,
        mods: &[(String, khaloni_poe2_core::trade::Query)],
        baseline: &(f64, String),
        unit: &str,
        unit_per_exalted: f64,
    ) -> Result<Vec<khaloni_poe2::evaluate_ui::AttributionRow>, String> {
        use khaloni_poe2::appraise;
        if let Some(why) = self.prices_not_ready() {
            return Err(why);
        }
        let free = self.free_slots();
        if !appraise::attribute_enabled(free) {
            return Err(format!(
                "what each mod is worth needs {} free search slots; {free} free",
                appraise::ATTRIBUTE_MIN_FREE
            ));
        }
        if mods.is_empty() {
            return Err("no ticked mod with a tier to price".to_string());
        }
        let snap = self.svc.snapshot();
        let empty = std::collections::HashMap::new();
        let names = self.catalogs.currencies.get().map(|(_, names)| names.clone()).unwrap_or(empty);
        let client = self.client()?;
        let mut dropped: Vec<(String, Option<(f64, String)>)> = Vec::new();
        for (label, q) in mods {
            let cheapest = client
                .search(q)
                .and_then(|s| {
                    if s.hashes.is_empty() {
                        return Ok(None);
                    }
                    let outcome = client.fetch_counted(&s.id, &s.hashes[..s.hashes.len().min(appraise::PAGE)])?;
                    // The search is price-ascending, so the first listing
                    // that converts is the cheapest.
                    Ok(outcome.listings.iter().find_map(|l| {
                        appraise::listing_exalted(l, &names, &snap.table)
                            .map(|ex| (ex * unit_per_exalted, l.account.clone()))
                    }))
                })
                .map_err(|e| appraise::error_text(&e))?;
            eprintln!("attribution: without {label}: {:?}", cheapest.as_ref().map(|(p, _)| p));
            dropped.push((label.clone(), cheapest));
        }
        Ok(appraise::attribution_rows((baseline.0, &baseline.1), &dropped, unit))
    }

    /// A currency's exalted price per unit, with the exchange offers it was
    /// read from. The poe.ninja table answers first, with no request and
    /// no offers; only a currency it lacks goes to the exchange.
    fn exchange_price(
        &mut self,
        name: &str,
    ) -> Result<Option<(f64, khaloni_poe2_core::bulk::BulkView)>, (String, Duration)> {
        use khaloni_poe2::appraise;
        if name.trim().is_empty() {
            return Ok(None);
        }
        let soon = |why: String| (why, appraise::ERROR_RETRY);
        if let Some(why) = self.prices_not_ready() {
            return Err(soon(why));
        }
        let snap = self.svc.snapshot();
        let table_price = snap.table.lookup(name).map(|p| p.exalted);
        if !appraise::needs_exchange(table_price) {
            return Ok(table_price.map(|ex| (ex, khaloni_poe2_core::bulk::BulkView::default())));
        }
        let id = match self.catalogs.currencies.get() {
            Some((ids, _)) => match ids.get(&name.to_lowercase()) {
                Some(id) => id.clone(),
                // Not an exchange item at all: that is an answer.
                None => return Ok(None),
            },
            None => {
                return Err(soon(format!("trade currency catalog: {}", self.catalogs.currencies.last_error())));
            }
        };
        let client = self.client().map_err(soon)?;
        // The body that answered is kept beside the rate: the bulk view
        // reads the offers, the rate is their median.
        let mut offers: Option<khaloni_poe2_core::bulk::BulkView> = None;
        let rate = appraise::exchange_with_fallback(
            &mut |have| {
                let view = khaloni_poe2_core::bulk::parse_exchange(&client.exchange_raw(&id, have)?);
                let rate = view.median_rate();
                if rate.is_some() {
                    offers = Some(view);
                }
                Ok(rate)
            },
            &|table_name| snap.table.lookup(table_name).map(|p| p.exalted),
        )
        .map_err(|e| (appraise::error_text(&e), appraise::retry_after(&e)))?;
        Ok(rate.map(|ex| (ex, offers.unwrap_or_default())))
    }

    fn gem_price(&mut self, skill: &str, level: u32) -> Result<Option<f64>, (String, Duration)> {
        use khaloni_poe2::appraise;
        let soon = |why: String| (why, appraise::ERROR_RETRY);
        if let Some(why) = self.prices_not_ready() {
            return Err(soon(why));
        }
        let Some((_, gem_types)) = self.catalogs.items.get() else {
            return Err(soon(format!("trade items catalog: {}", self.catalogs.items.last_error())));
        };
        let gem_types = gem_types.clone();
        let empty = std::collections::HashMap::new();
        let names = self.catalogs.currencies.get().map(|(_, names)| names.clone()).unwrap_or(empty);
        let snap = self.svc.snapshot();
        let client = self.client().map_err(soon)?;
        price_one_gem(client, skill, level, &gem_types, &names, &snap.table)
            .map_err(|e| (appraise::error_text(&e), appraise::retry_after(&e)))
    }
}

/// Craft worker requests. Item work carries the number of the craft panel
/// it belongs to: an answer for a panel the user has since replaced is
/// dropped on arrival.
enum CraftReq {
    /// Read a copied item into the planner.
    Open { text: String, generation: u64 },
    /// The price of buying one (a trade search), then every strategy
    /// costed. Runs of many seconds happen here, never on the main loop.
    Plan {
        generation: u64,
        state: khaloni_poe2_core::craft::types::ItemState,
        target: khaloni_poe2_core::craft::strategy::Target,
    },
    /// Gather listings of `class` for the observed model; its cost was
    /// stated and Run pressed.
    Calibrate { generation: u64, class: String },
    /// Work out what scanning the profile `name` costs, for the panel to
    /// state before anything is sent.
    StateScan { name: String },
    /// Run the scan of the profile `name`; its cost was stated and Run
    /// pressed.
    Scan { name: String },
    /// The listings a price check fetched in `league`, for the observed
    /// store.
    Record { league: String, raw: Vec<Option<serde_json::Value>> },
}

/// Craft worker answers. Every request is answered, failures included.
enum CraftDone {
    Opened {
        generation: u64,
        outcome: Result<(khaloni_poe2_core::craft::types::ItemState, khaloni_poe2::craft_ui::Picker, String), String>,
    },
    Planned {
        generation: u64,
        outcome: Result<khaloni_poe2::craft_ui::PlanView, String>,
        observed: String,
        note: Option<String>,
    },
    Calibrated { generation: u64, outcome: Result<String, String>, observed: String },
    ScanStated { name: String, outcome: Result<(String, Option<String>), String> },
    Scanned { outcome: Result<(khaloni_poe2::craft_ui::FlipList, Vec<String>), String> },
    /// What the worker is doing now, for the busy line of the panel of
    /// `generation` (any panel for a scan, which belongs to none).
    Busy { generation: Option<u64>, text: String },
}

/// The planner's data, loaded on first use and retried with a growing wait
/// when a download is missing.
struct CraftSources {
    craft: khaloni_poe2::appraise::Retrying<Arc<khaloni_poe2_core::craft::data::CraftData>>,
    ee2: khaloni_poe2::appraise::Retrying<khaloni_poe2_core::ee2::Ee2Data>,
    stats: khaloni_poe2::appraise::Retrying<khaloni_poe2_core::trade::StatIndex>,
    /// Trade currency id -> display name, for converting listing prices.
    currency_names: khaloni_poe2::appraise::Retrying<std::collections::HashMap<String, String>>,
}

/// How long the price of buying one is reused for the same target: a
/// calibration re-plans the shown target, and a second search for the same
/// item within minutes would say nothing new.
const BUY_REUSE: Duration = Duration::from_secs(10 * 60);

/// Everything the craft worker thread owns: the planner's data, its own
/// trade client (drawing on the process-wide limiter the price check uses),
/// and the observed store, which only this thread writes.
struct CraftWorker {
    done_tx: mpsc::Sender<CraftDone>,
    league: String,
    current: khaloni_poe2::league::Current,
    cache_dir: std::path::PathBuf,
    svc: prices::PriceService,
    cfg: Arc<std::sync::RwLock<Config>>,
    /// Every search a calibration, a scan or a buy line sends is the
    /// user's own and is noted here, so reward rows keep their distance.
    budget: Arc<std::sync::Mutex<khaloni_poe2::budget::Budget>>,
    /// The overlay's exchange answers and its background path for asking
    /// about a currency the table lacks.
    currency: CurrencyCache,
    /// Names plans could not price, looked up again before each plan.
    unpriced: std::collections::BTreeSet<String>,
    client: khaloni_poe2::appraise::Retrying<khaloni_poe2_core::trade::TradeClient>,
    sources: CraftSources,
    store: khaloni_poe2::appraise::Retrying<khaloni_poe2::observed_store::ObservedStore>,
    last_buy: Option<(String, Result<khaloni_poe2_core::craft::plan::BuyQuote, String>, std::time::Instant)>,
    /// The panel the request being handled belongs to.
    serving: Option<u64>,
}

impl CraftWorker {
    fn run(mut self, rx: mpsc::Receiver<CraftReq>) {
        for req in rx {
            let what = match &req {
                CraftReq::Open { .. } => "reading the item",
                CraftReq::Plan { .. } => "planning",
                CraftReq::Calibrate { .. } => "calibrating",
                CraftReq::StateScan { .. } | CraftReq::Scan { .. } => "scanning",
                CraftReq::Record { .. } => "recording listings",
            };
            // One bad item must not end the planner for the session.
            let answer = craft_failure(&req);
            let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.handle(req)));
            if let Err(panic) = ran {
                let why = panic
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "unknown panic".into());
                eprintln!("craft worker: {what} panicked: {why}");
                if let Some(done) = answer(format!("internal error while {what}: {why}")) {
                    let _ = self.done_tx.send(done);
                }
            }
        }
    }

    fn follow_league(&mut self) {
        let now = self.current.name();
        if now != self.league {
            eprintln!("craft worker: league {} -> {now}", self.league);
            self.league = now;
            if let Some(c) = self.client.get_mut() {
                c.set_league(&self.league);
            }
            self.last_buy = None;
        }
        let league = self.league.clone();
        if let Some(store) = self.store.get_mut() {
            if let Err(e) = store.switch_league(&league) {
                eprintln!("observed store: {league}: {e}");
            }
        }
    }

    fn busy(&self, text: String) {
        let _ = self.done_tx.send(CraftDone::Busy { generation: self.serving, text });
    }

    fn craft_data(&mut self) -> Result<Arc<khaloni_poe2_core::craft::data::CraftData>, String> {
        let dir = self.cache_dir.clone();
        self.sources
            .craft
            .get_or_load(std::time::Instant::now(), || {
                let files = khaloni_poe2::refcache::craft_files(&dir)?;
                khaloni_poe2_core::craft::data::CraftData::load(&files.mods, &files.bases, &files.essences).map(Arc::new)
            })
            .map(|d| d.clone())
            .map_err(|e| format!("the planner's mod data is not available: {e}"))
    }

    /// The trade client, with the POESESSID saved in settings.
    fn client(&mut self) -> Result<&mut khaloni_poe2_core::trade::TradeClient, String> {
        let league = self.league.clone();
        let session = self.cfg.read().unwrap_or_else(|e| e.into_inner()).poesessid.clone();
        khaloni_poe2::appraise::session_client(&mut self.client, &session, std::time::Instant::now(), || {
            khaloni_poe2_core::trade::TradeClient::new(khaloni_poe2_core::trade::TRADE_BASE, &league)
                .map_err(|e| format!("trade client unavailable: {e}"))
        })
    }

    /// The trade site's stat catalog and currency names, from the cache the
    /// price check fills (a day old at most), downloaded when missing.
    fn load_catalogs(&mut self) -> Result<(), String> {
        use khaloni_poe2_core::trade::{self, TradeData};
        let now = std::time::Instant::now();
        let dir = self.cache_dir.clone();
        let league = self.league.clone();
        let client: &trade::TradeClient = self.client.get_or_load(now, || {
            trade::TradeClient::new(trade::TRADE_BASE, &league).map_err(|e| format!("trade client unavailable: {e}"))
        })?;
        let fetch = |kind: TradeData, file: &str| {
            client.cached_data(kind, &dir.join(file), CATALOG_MAX_AGE).map_err(|e| e.to_string())
        };
        let stats = self.sources.stats.get_or_load(now, || {
            trade::StatIndex::from_json(&fetch(TradeData::Stats, "trade_stats.json")?).map_err(|e| e.to_string())
        });
        if let Err(e) = stats {
            return Err(format!("the trade stats catalog is not available: {e}"));
        }
        let names = self.sources.currency_names.get_or_load(now, || {
            let ids = trade::parse_static_currency_ids(&fetch(TradeData::Static, "trade_static.json")?);
            Ok(ids.into_iter().map(|(name, id)| (id, name)).collect())
        });
        if let Err(e) = names {
            return Err(format!("the trade currency catalog is not available: {e}"));
        }
        Ok(())
    }

    /// EE2's data, which reads a copied item, with the trade catalogs it
    /// matches stat lines against.
    fn load_ee2(&mut self) -> Result<(), String> {
        use khaloni_poe2_core::ee2::data;
        let dir = self.cache_dir.clone();
        let now = std::time::Instant::now();
        self.sources
            .ee2
            .get_or_load(now, || khaloni_poe2::refcache::try_ee2_data(&dir))
            .map_err(|e| format!("EE2 reference data unavailable ({e}); the item cannot be read"))?;
        let d = self.sources.ee2.get_mut().expect("loaded above");
        if d.trade_stats.is_none() {
            d.trade_stats = std::fs::read_to_string(dir.join("trade_stats.json"))
                .ok()
                .and_then(|body| data::TradeStatTexts::from_json(&body).ok());
        }
        if d.trade_items.is_none() {
            d.trade_items = std::fs::read_to_string(dir.join("trade_items.json"))
                .ok()
                .and_then(|body| data::trade_item_names(&body).ok());
        }
        Ok(())
    }

    fn store(&mut self) -> Result<&mut khaloni_poe2::observed_store::ObservedStore, String> {
        let league = self.league.clone();
        let min = self.cfg.read().unwrap_or_else(|e| e.into_inner()).craft_observed_min;
        let store = self
            .store
            .get_or_load(std::time::Instant::now(), || {
                khaloni_poe2::observed_store::ObservedStore::open(&khaloni_poe2::observed_store::default_dir(), &league)
                    .map_err(|e| format!("the observed store could not be opened: {e}"))
            })?;
        store.set_min_listings(min);
        Ok(store)
    }

    fn observed_line(&mut self, class: &str) -> String {
        match self.store() {
            Ok(store) => khaloni_poe2::craft_flow::observed_line(store, class),
            Err(why) => format!("observed model unavailable: {why}"),
        }
    }

    /// The panel's units at the current exchange rate.
    fn rates(&self) -> khaloni_poe2::craft_ui::Rates {
        let threshold = self.cfg.read().unwrap_or_else(|e| e.into_inner()).divine_threshold;
        khaloni_poe2::craft_flow::rates(&self.svc.snapshot().table, threshold)
    }

    /// Exchange answers the overlay holds for names the table lacks. Each
    /// lookup goes through the overlay's own background path, which asks
    /// the exchange (within the reward rows' budget) for a name it has no
    /// fresh answer to.
    fn exchange_prices(&self) -> std::collections::HashMap<String, f64> {
        use khaloni_poe2::pricing::{CurrencyPricer, CurrencyState};
        self.unpriced
            .iter()
            .filter_map(|name| match self.currency.lookup(name) {
                Some(CurrencyState::Priced(ex)) => Some((name.clone(), ex)),
                _ => None,
            })
            .collect()
    }

    /// Runs one search and fetches up to `listings` of its results through
    /// the process-wide limiter. The search is the user's own.
    fn fetch(&mut self, q: &khaloni_poe2_core::trade::Query, listings: usize) -> Result<Vec<Option<serde_json::Value>>, String> {
        use khaloni_poe2::appraise::error_text;
        self.budget.lock().unwrap_or_else(|e| e.into_inner()).note_user(std::time::Instant::now());
        let client = self.client()?;
        let s = client.search(q).map_err(|e| error_text(&e))?;
        let mut raw = Vec::new();
        for range in khaloni_poe2::craft_flow::fetch_pages(s.hashes.len(), listings) {
            raw.extend(client.fetch_counted(&s.id, &s.hashes[range]).map_err(|e| error_text(&e))?.raw);
        }
        eprintln!("craft search: {} listings fetched", raw.iter().flatten().count());
        Ok(raw)
    }

    fn handle(&mut self, req: CraftReq) {
        self.follow_league();
        self.serving = match &req {
            CraftReq::Open { generation, .. } | CraftReq::Plan { generation, .. } | CraftReq::Calibrate { generation, .. } => {
                Some(*generation)
            }
            _ => None,
        };
        match req {
            CraftReq::Record { league, raw } => {
                // A price check from a league the store has left says
                // nothing about this one's market.
                if league != self.league {
                    return;
                }
                let Ok(data) = self.craft_data() else { return };
                match self.store().and_then(|s| s.record_fetch(&raw, &data).map_err(|e| e.to_string())) {
                    Ok(r) if r.recorded > 0 => eprintln!(
                        "observed store: {} listings recorded ({} repeated, {} unjoined modifiers)",
                        r.recorded,
                        r.repeated,
                        r.unjoined.len()
                    ),
                    Ok(_) => {}
                    Err(e) => eprintln!("observed store: {e}"),
                }
            }
            CraftReq::Open { text, generation } => {
                let outcome = self.open(&text);
                let _ = self.done_tx.send(CraftDone::Opened { generation, outcome });
            }
            CraftReq::Plan { generation, state, target } => {
                let (outcome, note) = match self.plan(&state, &target) {
                    Ok((view, note)) => (Ok(view), note),
                    Err(why) => (Err(why), None),
                };
                let observed = self.observed_line(&state.class);
                let _ = self.done_tx.send(CraftDone::Planned { generation, outcome, observed, note });
            }
            CraftReq::Calibrate { generation, class } => {
                let outcome = self.calibrate(&class);
                let observed = self.observed_line(&class);
                let _ = self.done_tx.send(CraftDone::Calibrated { generation, outcome, observed });
            }
            CraftReq::StateScan { name } => {
                let outcome = self.state_scan(&name);
                let _ = self.done_tx.send(CraftDone::ScanStated { name, outcome });
            }
            CraftReq::Scan { name } => {
                let outcome = self.scan(&name);
                let _ = self.done_tx.send(CraftDone::Scanned { outcome });
            }
        }
    }

    fn open(
        &mut self,
        text: &str,
    ) -> Result<(khaloni_poe2_core::craft::types::ItemState, khaloni_poe2::craft_ui::Picker, String), String> {
        let data = self.craft_data()?;
        self.load_ee2()?;
        let ee2 = self.sources.ee2.get().expect("loaded by load_ee2");
        let opened = khaloni_poe2::craft_flow::read_item(text, ee2, &data)?;
        let observed = self.observed_line(&opened.state.class);
        Ok((opened.state, opened.picker, observed))
    }

    /// The price of buying one: a search for the finished item on the
    /// item's base, reused for the same target for a while.
    fn buy(
        &mut self,
        state: &khaloni_poe2_core::craft::types::ItemState,
        target: &khaloni_poe2_core::craft::strategy::Target,
        data: &khaloni_poe2_core::craft::data::CraftData,
    ) -> Result<khaloni_poe2_core::craft::plan::BuyQuote, String> {
        use khaloni_poe2::craft_flow;
        let key = format!("{}|{:?}", state.base, target.wants);
        if let Some((k, got, at)) = &self.last_buy {
            if *k == key && at.elapsed() < BUY_REUSE {
                return got.clone();
            }
        }
        self.load_catalogs()?;
        self.load_ee2()?;
        let stats = &self.sources.ee2.get().expect("loaded by load_ee2").stats;
        let catalog = self.sources.stats.get().expect("loaded by load_catalogs");
        let (resolved, query) = craft_flow::buy_search(state, target, data, stats, catalog)?;
        let cost = khaloni_poe2_core::flip::RequestCost::of(1);
        craft_flow::allow(cost, craft_flow::search_window(std::time::Instant::now()))
            .map_err(|why| format!("the finished item was not searched: {why}"))?;
        self.busy("planning: searching for the finished item to price buying one".to_string());
        let raw = self.fetch(&query, khaloni_poe2_core::flip::LISTINGS_PER_SEARCH as usize)?;
        let entries: Vec<serde_json::Value> = raw.into_iter().flatten().collect();
        let snap = self.svc.snapshot();
        let names = self.sources.currency_names.get().expect("loaded by load_catalogs");
        let convert = |amount: f64, currency: &str| craft_flow::listing_exalted(amount, currency, names, &snap.table);
        let market = khaloni_poe2_core::flip::Market { convert: &convert, unit: "ex" };
        let got = craft_flow::buy_quote(&entries, &resolved, data, &market);
        self.last_buy = Some((key, got.clone(), std::time::Instant::now()));
        got
    }

    fn plan(
        &mut self,
        state: &khaloni_poe2_core::craft::types::ItemState,
        target: &khaloni_poe2_core::craft::strategy::Target,
    ) -> Result<(khaloni_poe2::craft_ui::PlanView, Option<String>), String> {
        use khaloni_poe2::craft_flow;
        if let Some(why) = khaloni_poe2::league::not_ready(&self.svc.snapshot(), &self.league) {
            return Err(why);
        }
        let data = self.craft_data()?;
        let exchange = self.exchange_prices();
        let buy = self.buy(state, target, &data);
        let observed = self.store()?.model(&state.class);
        let config = self.cfg.read().unwrap_or_else(|e| e.into_inner()).craft_sim();
        let snap = self.svc.snapshot();
        let prices = |name: &str| craft_flow::price_of(&snap.table, &exchange, name);
        self.busy(format!("planning: costing every strategy on {} runs each", config.runs));
        let started = std::time::Instant::now();
        let plan = craft_flow::plan_item(craft_flow::PlanInputs {
            state,
            target,
            data: &data,
            observed: observed.as_ref(),
            prices: &prices,
            buy,
            config: &config,
        });
        eprintln!("craft plan: {} strategies in {:.1}s", plan.strategies.len(), started.elapsed().as_secs_f32());
        // What the table lacks is looked up in the overlay's exchange
        // answers before every later plan. A price check of a stack of it
        // puts its exchange price there; the background queue may too, when
        // its budget has room.
        let missing = craft_flow::missing_prices(&plan);
        let note = (!missing.is_empty()).then(|| {
            self.unpriced.extend(missing.iter().cloned());
            let _ = self.exchange_prices();
            format!(
                "no price in the price table for {}: a price check of a stack of it prices it from the exchange, then press Plan again",
                missing.join(", ")
            )
        });
        Ok((khaloni_poe2::craft_ui::PlanView::build(&plan, &self.rates()), note))
    }

    fn calibrate(&mut self, class: &str) -> Result<String, String> {
        use khaloni_poe2::craft_flow;
        let data = self.craft_data()?;
        let window = craft_flow::search_window(std::time::Instant::now());
        self.busy(format!("calibrating {class}: 5 searches, 20 fetches"));
        // The store and the client are both this worker's; the fetch
        // closure borrows the client while the store is recorded into, so
        // the store is taken out for the calibration and put back.
        self.store()?;
        // The price bands are set from the first search's cheapest price,
        // converted the way the price check converts its listings.
        self.load_catalogs()?;
        let snap = self.svc.snapshot();
        let names = self.sources.currency_names.get().cloned().unwrap_or_default();
        let convert = |amount: f64, currency: &str| craft_flow::listing_exalted(amount, currency, &names, &snap.table);
        let mut store = std::mem::take(&mut self.store);
        let got = {
            let store = store.get_mut().expect("opened above");
            let mut fetch = |q: &khaloni_poe2_core::trade::Query, n: usize| {
                match q.price_min {
                    Some(min) => eprintln!("calibration search: {class} from {min} ex up"),
                    None => eprintln!("calibration search: cheapest {class}"),
                }
                self.fetch(q, n)
            };
            craft_flow::calibrate(class, window, &mut fetch, store, &data, &convert)
        };
        self.store = store;
        got.map(|c| c.text(class))
    }

    /// The profile `name` from the saved profiles file.
    fn profile(&self, name: &str) -> Result<khaloni_poe2_core::flip::Profile, String> {
        let dir = Config::path().parent().map(std::path::Path::to_path_buf).ok_or("no config directory")?;
        let loaded = khaloni_poe2::craft_flow::load_profiles(&khaloni_poe2::craft_flow::profiles_path(&dir));
        loaded.profiles.into_iter().find(|p| p.name == name).ok_or_else(|| {
            let why: Vec<String> = loaded.errors.iter().map(|e| e.to_string()).collect();
            if why.is_empty() {
                format!("no profile named \"{name}\" in profiles.toml")
            } else {
                format!("no loadable profile named \"{name}\" in profiles.toml ({})", why.join("; "))
            }
        })
    }

    fn scan_plan(&mut self, name: &str) -> Result<khaloni_poe2::craft_flow::ScanPlan, String> {
        let profile = self.profile(name)?;
        let data = self.craft_data()?;
        self.load_catalogs()?;
        self.load_ee2()?;
        let stats = &self.sources.ee2.get().expect("loaded by load_ee2").stats;
        let catalog = self.sources.stats.get().expect("loaded by load_catalogs");
        khaloni_poe2::craft_flow::scan_plan(&profile, &data, stats, catalog)
    }

    fn state_scan(&mut self, name: &str) -> Result<(String, Option<String>), String> {
        use khaloni_poe2::craft_flow;
        let plan = self.scan_plan(name)?;
        let window = craft_flow::search_window(std::time::Instant::now());
        let mut statement = craft_flow::scan_statement(&plan, window);
        for s in &plan.relaxations.skipped {
            statement.push_str(&format!("; {s}"));
        }
        Ok((statement, craft_flow::allow(plan.cost, window).err()))
    }

    fn scan(&mut self, name: &str) -> Result<(khaloni_poe2::craft_ui::FlipList, Vec<String>), String> {
        use khaloni_poe2::craft_flow;
        if let Some(why) = khaloni_poe2::league::not_ready(&self.svc.snapshot(), &self.league) {
            return Err(why);
        }
        let plan = self.scan_plan(name)?;
        let data = self.craft_data()?;
        let window = craft_flow::search_window(std::time::Instant::now());
        let statement = craft_flow::scan_statement(&plan, window);
        let observed = self.store()?.model(&plan.resolved.profile.class);
        let mut config = self.cfg.read().unwrap_or_else(|e| e.into_inner()).craft_sim();
        config.runs = config.runs.min(craft_flow::SCAN_RUNS);
        let snap = self.svc.snapshot();
        let exchange = self.exchange_prices();
        let prices = |item: &str| craft_flow::price_of(&snap.table, &exchange, item);
        let names = self.sources.currency_names.get().cloned().unwrap_or_default();
        let convert = |amount: f64, currency: &str| craft_flow::listing_exalted(amount, currency, &names, &snap.table);
        let market = khaloni_poe2_core::flip::Market { convert: &convert, unit: "ex" };
        let planner = khaloni_poe2_core::flip::Planner {
            pool: data.as_ref(),
            observed: observed.as_ref(),
            prices: &prices,
            config: &config,
        };
        let rates = self.rates();
        let league = self.league.clone();
        let inputs = craft_flow::ScanInputs { data: &data, planner: &planner, market: &market, rates: &rates, league: &league, statement };
        self.busy(format!("scanning {name}: {} searches, then costing each candidate", plan.cost.searches));
        let mut fetch = |q: &khaloni_poe2_core::trade::Query, n: usize| self.fetch(q, n);
        craft_flow::run_scan(&plan, window, &mut fetch, &inputs).map(|s| (s.list, s.notes))
    }
}

/// How a request that failed outright is answered, worked out before it
/// runs so a panic can still be answered.
fn craft_failure(req: &CraftReq) -> Box<dyn Fn(String) -> Option<CraftDone>> {
    match req {
        CraftReq::Open { generation, .. } => {
            let generation = *generation;
            Box::new(move |why| Some(CraftDone::Opened { generation, outcome: Err(why) }))
        }
        CraftReq::Plan { generation, .. } => {
            let generation = *generation;
            Box::new(move |why| Some(CraftDone::Planned { generation, outcome: Err(why), observed: String::new(), note: None }))
        }
        CraftReq::Calibrate { generation, .. } => {
            let generation = *generation;
            Box::new(move |why| Some(CraftDone::Calibrated { generation, outcome: Err(why), observed: String::new() }))
        }
        CraftReq::StateScan { name } => {
            let name = name.clone();
            Box::new(move |why| Some(CraftDone::ScanStated { name: name.clone(), outcome: Err(why) }))
        }
        CraftReq::Scan { .. } => Box::new(|why| Some(CraftDone::Scanned { outcome: Err(why) })),
        CraftReq::Record { .. } => Box::new(|_| None),
    }
}

/// A reward row whose trade lookup failed retries by itself every half
/// minute; the reason is put on screen this often at most, every failure
/// goes to the log.
const ROW_ERROR_NOTE_GAP: Duration = Duration::from_secs(120);

/// How long a price check waits for the copy's reply before the key is
/// given back. The slowest honest copy is the modifier-release wait (1.5s)
/// plus the clipboard windows and a read that times out (about 2s more), so
/// anything past this is a reply that is not coming.
const COPY_REPLY_TIMEOUT: Duration = Duration::from_secs(4);

/// The Evaluate panel as the main loop holds it: the model, the query its
/// checkboxes edit, and its placed top-left (global logical px).
type EvalPanel = (khaloni_poe2::evaluate_ui::Panel, khaloni_poe2_core::trade::Query, (i32, i32));

/// An in-progress panel drag: (grab point in surface px, the panel's global
/// position when the grab began).
type PanelDrag = ((i32, i32), (i32, i32));

/// Where the craft panel opens: left of the game's centre, below the top
/// bar.
fn craft_pos(game: Rect) -> (i32, i32) {
    (game.x + game.w as i32 / 2 - 320, game.y + 90)
}

/// The craft panel's price line: whose prices, and whether some are old.
fn craft_prices_line(snap: &prices::Snapshot) -> String {
    let old = if snap.stale { " (some are old)" } else { "" };
    format!("prices: poe.ninja, {}{old}", snap.league)
}

/// Carries out a craft panel action on the panel of `item`: the planner and
/// every request run on the craft worker; a calibration first states its
/// cost and runs only on Run.
fn craft_run(
    action: &khaloni_poe2::craft_ui::Action,
    p: &mut khaloni_poe2::craft_ui::Panel,
    item: &khaloni_poe2_core::craft::types::ItemState,
    generation: u64,
    tx: &mpsc::Sender<CraftReq>,
) {
    use khaloni_poe2::craft_flow;
    use khaloni_poe2::craft_ui::{apply, Action, Ask, PlanState, Prompt};
    match action {
        Action::Plan => {
            // Only a press that moved the panel to its waiting state sends:
            // a second press while planning does nothing.
            if apply(p, action) {
                p.note = None;
                if tx.send(CraftReq::Plan { generation, state: item.clone(), target: p.target() }).is_err() {
                    p.plan = PlanState::None;
                    p.note = Some("the craft planner is not running: restart the overlay".to_string());
                }
            }
        }
        Action::Calibrate => {
            if p.can_calibrate() {
                let class = p.picker.class.clone();
                let window = craft_flow::search_window(std::time::Instant::now());
                p.ask(Prompt {
                    ask: Ask::Calibrate,
                    statement: craft_flow::calibration_statement(&class, window),
                    refused: craft_flow::allow(craft_flow::calibration_cost(), window).err(),
                });
            }
        }
        Action::Run => match p.prompt.take() {
            Some(Prompt { ask: Ask::Calibrate, refused: None, .. }) => {
                let class = p.picker.class.clone();
                p.busy = Some(format!("calibrating {class}"));
                if tx.send(CraftReq::Calibrate { generation, class }).is_err() {
                    p.busy = None;
                    p.note = Some("the craft planner is not running: restart the overlay".to_string());
                }
            }
            other => craft_run_scan(other, p, tx),
        },
        _ => craft_run_without_item(action, p, tx),
    }
}

/// The craft panel actions that need no item: running a stated scan,
/// opening a candidate's listing, and the panel's own navigation.
fn craft_run_without_item(action: &khaloni_poe2::craft_ui::Action, p: &mut khaloni_poe2::craft_ui::Panel, tx: &mpsc::Sender<CraftReq>) {
    use khaloni_poe2::craft_ui::{apply, Action};
    match action {
        Action::Run => {
            let prompt = p.prompt.take();
            craft_run_scan(prompt, p, tx);
        }
        Action::OpenSite(url) => {
            p.note = Some(match open_url(url) {
                Ok(()) => "opened in the browser".to_string(),
                Err(e) => e,
            });
        }
        // Planning and calibrating act on an item; this panel has none.
        Action::Plan | Action::Calibrate => {}
        other => {
            apply(p, other);
        }
    }
}

/// Sends the scan the prompt stated, when the budget allowed it; any other
/// prompt goes back on the panel unchanged.
fn craft_run_scan(
    prompt: Option<khaloni_poe2::craft_ui::Prompt>,
    p: &mut khaloni_poe2::craft_ui::Panel,
    tx: &mpsc::Sender<CraftReq>,
) {
    use khaloni_poe2::craft_ui::{Ask, Prompt};
    match prompt {
        Some(Prompt { ask: Ask::Scan(name), refused: None, .. }) => {
            p.busy = Some(format!("scanning {name}"));
            if tx.send(CraftReq::Scan { name }).is_err() {
                p.busy = None;
                p.note = Some("the craft planner is not running: restart the overlay".to_string());
            }
        }
        other => p.prompt = other,
    }
}

/// Closes the Evaluate panel together with everything that only means
/// something while it is open: the box being typed into, its text, a drag,
/// and the record of what its listings came from. Every path that takes the
/// panel goes through here; an `editing` left behind by one that did not
/// kept the keyboard grabbed with no box to type into. True when a panel
/// was open. The caller settles keyboard and input region afterwards.
fn close_eval(
    apanel: &mut Option<EvalPanel>,
    editing: &mut Option<(usize, khaloni_poe2::evaluate_ui::Field)>,
    edit_buf: &mut String,
    panel_drag: &mut Option<PanelDrag>,
    searched: &mut Option<Searched>,
) -> bool {
    *editing = None;
    edit_buf.clear();
    *panel_drag = None;
    *searched = None;
    apanel.take().is_some()
}

/// True when `point` (global logical px) lies outside the output a surface
/// of `size` at `output_pos` covers: the game has moved to another monitor
/// and the overlay has to follow it. A surface that has no size yet has not
/// been configured, which says nothing about where the game is.
fn off_output(point: (i32, i32), output_pos: (i32, i32), size: (u32, u32)) -> bool {
    if size.0 == 0 || size.1 == 0 {
        return false;
    }
    let (x, y) = (point.0 - output_pos.0, point.1 - output_pos.1);
    x < 0 || y < 0 || x >= size.0 as i32 || y >= size.1 as i32
}

/// The Evaluate card for a freshly built query: one row per filter, led by
/// the item's computed figures, closed by the lines that cannot be searched.
fn build_panel(
    title: String,
    query: &khaloni_poe2_core::trade::Query,
    labels: &[khaloni_poe2_core::trade::FilterLabel],
    facts: Option<ItemFacts>,
    unsearchable: Vec<String>,
    extra: &[khaloni_poe2_core::ee2::request::ExtraRow],
    reference: Option<&khaloni_poe2::refcache::Reference>,
) -> khaloni_poe2::evaluate_ui::Panel {
    // Affix index once per panel, not once per row: it is a map over the
    // whole affix export (tens of thousands of entries) and every row looks
    // into the same one.
    let affix_ix = reference.map(|r| khaloni_poe2_core::refdata::affix_index(&r.affixes));
    // A miss, or an affix with no ladder joined to it, gets no badge and no
    // score. An unknown roll is shown as unknown; it is never approximated.
    let grade = |text: &str, rolled: Option<f64>| {
        let affix = affix_ix
            .as_ref()
            .and_then(|ix| ix.get(&khaloni_poe2_core::refdata::normalize_mod_text(text)))
            .filter(|a| !a.tiers.is_empty());
        let badge = affix.zip(rolled).and_then(|(a, rolled)| {
            khaloni_poe2_core::rollquality::tier_of(&a.tiers, rolled)
                .map(|tier| khaloni_poe2::evaluate_ui::TierBadge { kind: ui_affix_kind(a.kind), tier })
        });
        let score =
            affix.zip(rolled).and_then(|(a, rolled)| khaloni_poe2_core::rollquality::score(&a.tiers, rolled));
        (badge, score)
    };
    let mut rows: Vec<khaloni_poe2::evaluate_ui::StatRow> = labels
        .iter()
        .enumerate()
        .filter_map(|(i, l)| {
            let f = query.filters.get(i)?;
            // The filter's min is the SEARCH floor (the tier's low end on
            // advanced-format text); the roll the item actually has travels
            // in the label, and that is what the tier ladder and the score
            // are read against. The floor is only a fallback for a line
            // that carried no number at all.
            let rolled = l.rolled.or(f.value.min).or(f.value.max);
            let (badge, score) = grade(&l.text, rolled);
            Some(khaloni_poe2::evaluate_ui::StatRow {
                label: l.text.clone(),
                badge,
                score,
                // No lower bound in the filter is an empty box on the card.
                min: f.value.min,
                max: f.value.max,
                enabled: !f.disabled,
                target: Some(khaloni_poe2::evaluate_ui::Target::Stat(i)),
                // A total that is not searched repeats mods already listed,
                // so it collapses behind "Show N more" until asked for.
                hidden: l.hidden,
                group: khaloni_poe2::evaluate_ui::group_of(l.tag),
                note: None,
            })
        })
        .collect();
    // Every line EE2 gives no row of its own (one a figure or a total
    // counted, one it cannot search): a row with its tier, unticked, so the
    // search stays EE2's until the user adds it.
    let mut extras = Vec::new();
    rows.extend(khaloni_poe2::evaluate_ui::extra_rows(extra, &mut extras).into_iter().zip(extra).map(|(row, x)| {
        let (badge, score) = grade(&x.text, x.rolled);
        khaloni_poe2::evaluate_ui::StatRow { badge, score, ..row }
    }));
    // The item's computed figures lead the card the way the tooltip's own
    // property block does; each is searchable as an equipment_filters
    // minimum.
    if let Some(f) = facts.as_ref() {
        rows.splice(0..0, khaloni_poe2::evaluate_ui::property_rows(&f.props, f.chaos_dps));
    }
    rows.extend(unsearchable.into_iter().map(|text| khaloni_poe2::evaluate_ui::StatRow {
        label: format!("{text} (not searchable)"),
        badge: None,
        score: None,
        min: None,
        max: None,
        enabled: false,
        target: None,
        hidden: false,
        group: khaloni_poe2::evaluate_ui::RowGroup::Explicit,
        note: None,
    }));
    rows.sort_by_key(|r| r.group);
    // Gear carries a category toggle. Switched off, a price check searches
    // the item's exact base instead, the way EE2 does (see
    // `Query::category_replaces_type`); items with no category to search by
    // get no toggle.
    let base = query.category.as_deref().map(|c| khaloni_poe2::evaluate_ui::BaseToggle {
        label: format!("Category: {}", pretty_category(c)),
        enabled: query.category_enabled,
    });
    khaloni_poe2::evaluate_ui::Panel {
        header: khaloni_poe2::evaluate_ui::ItemHeader {
            name: title,
            // Rare is the fallback only when the response carried no facts
            // at all (a check that failed before the item was read); the
            // rest stay absent when absent.
            rarity: facts.as_ref().map(|f| f.rarity.clone()).unwrap_or_else(|| "Rare".to_string()),
            item_level: facts.as_ref().and_then(|f| f.item_level),
            requires_level: facts.as_ref().and_then(|f| f.requires_level),
            base,
        },
        rows,
        extras,
        // The worker follows every seed with a result, so a search is
        // running from the moment the card opens.
        status: "searching...".to_string(),
        searching: true,
        ..khaloni_poe2::evaluate_ui::Panel::default()
    }
}

/// Puts a search's answer on the panel: every block under the card, the
/// budget line, and the status. What the search did not find is cleared,
/// so nothing of an earlier search is read as this one's.
fn show_search(panel: &mut khaloni_poe2::evaluate_ui::Panel, done: &SearchDone) {
    let b = &done.blocks;
    panel.listings = b.listings.clone();
    panel.hover = None;
    panel.ladder = b.ladder.clone();
    panel.price_fixed = b.price_fixed.clone();
    panel.closest = b.closest.clone();
    panel.attribution.clear();
    // The poe.ninja line belongs to the item, not to one search: a Search
    // press without a checked item behind it keeps the line it had.
    if done.ninja.is_some() {
        panel.ninja = done.ninja.clone();
    }
    panel.bulk = done.bulk.clone();
    panel.stack_value = done.stack_value.clone();
    panel.budget_text = done.budget.0.clone();
    panel.budget_low = done.budget.1;
    panel.attribute_enabled = done.attribute_enabled;
}

/// Takes a search's blocks off the panel after a failed one: the reason
/// stays on the status line, and the listings of an earlier search do
/// not, since they no longer answer what the boxes now say.
fn clear_search(panel: &mut khaloni_poe2::evaluate_ui::Panel) {
    panel.listings.clear();
    panel.hover = None;
    panel.ladder.clear();
    panel.price_fixed = None;
    panel.closest = None;
    panel.attribution.clear();
    panel.bulk = None;
    panel.stack_value = None;
}

/// Keeps a checked item's clipboard text, named by a hash of the text so
/// the same item checked twice is one file, and prunes the directory to the
/// newest `KEEP`. Best-effort: a failed write must never get in the way of
/// a price check.
fn dump_item_text(text: &str) {
    use std::hash::{Hash, Hasher};
    const KEEP: usize = 200;
    let dir = match std::env::var_os("KHALONI_ITEM_DUMP") {
        Some(d) => std::path::PathBuf::from(d),
        None => match directories::ProjectDirs::from("", "", "khaloni-poe2") {
            Some(d) => d.cache_dir().join("checked-items"),
            None => return,
        },
    };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    let _ = std::fs::write(dir.join(format!("item-{:016x}.txt", h.finish())), text);
    let mut files: Vec<(std::time::SystemTime, std::path::PathBuf)> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    files.sort();
    for (_, old) in files.iter().rev().skip(KEEP) {
        let _ = std::fs::remove_file(old);
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
fn overlay_mode(
    _lock: Option<khaloni_poe2::platform::InstanceLock>,
    _game_over: Option<Arc<AtomicBool>>,
) -> anyhow::Result<()> {
    anyhow::bail!("this build has no OCR (windows-gnu check target); the shipped Windows build is MSVC with vcpkg tesseract")
}

#[cfg(ocr)]
fn overlay_mode(
    lock: Option<khaloni_poe2::platform::InstanceLock>,
    game_over: Option<Arc<AtomicBool>>,
) -> anyhow::Result<()> {
    // One overlay owns the hotkeys, the tray, and the KWin script; a
    // second instance fights the first for all three (seen live as stale
    // KWin scripts piling up). --launch hands its lock in, having used it
    // to choose wrapper-vs-passive; plain launches acquire it here.
    let _lock = match lock {
        Some(l) => l,
        None => khaloni_poe2::platform::single_instance()?,
    };
    // Startup phase timing, permanently logged: cold-start stalls are only
    // diagnosable from user reports, and eight lines of log are cheap.
    let boot = std::time::Instant::now();
    let phase = move |name: &str| eprintln!("t+{:>5}ms {}", boot.elapsed().as_millis(), name);
    let mut cfg = Config::load()?;
    // Things the user has to be told once the overlay can draw: a migrated
    // setting, a hotkey that lost its key, a capture that stopped.
    let mut notices: std::collections::VecDeque<String> = cfg.notices.drain(..).collect();
    // The league everything is priced in. It follows the config as a
    // whole or not at all; see `league`.
    let current_league = khaloni_poe2::league::Current::new(&cfg.league);
    let mut league_announcer = khaloni_poe2::league::Announcer::default();

    let cache = directories::ProjectDirs::from("", "", "khaloni-poe2").unwrap().cache_dir().to_path_buf();
    // Everything stderr gets from here on is kept in overlay.log in the
    // cache dir: a trade ban is only explainable from the request lines
    // that led up to it, and the journal is not where the owner looks.
    if let Err(e) = khaloni_poe2::applog::install(&cache) {
        eprintln!("log file: {e}");
    }
    // Every trade request and the server's counters go through stderr
    // (and so into the log); the panel reads its budget line off them.
    khaloni_poe2_core::trade::set_request_logger(Box::new(|line: &str| {
        khaloni_poe2::appraise::note_request_line(line);
        khaloni_poe2::craft_flow::note_request_line(line);
        eprintln!("{line}");
    }));
    // Before any thread reads the cache: a release that bumped the pinned
    // reference data must not serve the previous patch's files.
    khaloni_poe2::refcache::sync_pin(&cache);
    // Files only a removed feature read (the leveling guide's route and its
    // ticked steps) go, once; nothing else in either directory is touched.
    if let Some(config_dir) = Config::path().parent() {
        for gone in khaloni_poe2::refcache::remove_obsolete_files(&cache, config_dir) {
            eprintln!("removed a file only a removed feature used: {}", gone.display());
        }
    }
    let svc = prices::PriceService::start_with_interval(
        NinjaClient::new(cache.clone()),
        khaloni_poe2_core::scout::ScoutClient::new(cache.clone()),
        cfg.league.clone(),
        std::time::Duration::from_secs(cfg.refresh_minutes * 60),
    )?;

    phase("price service up");
    let kwin = khaloni_poe2::platform::gamewin::start()?;
    phase("window tracker up");
    let rt = tokio::runtime::Runtime::new()?;
    // A killed process never runs Drop, and KWin keeps a script loaded until
    // told otherwise: SIGTERM/SIGINT/SIGHUP take the same exit as the tray's
    // Quit, and stop the script right away in case the loop is stuck.
    let quit = Arc::new(AtomicBool::new(false));
    #[cfg(unix)]
    {
        let quit = quit.clone();
        let stop = kwin.shutdown_handle();
        rt.spawn(async move {
            use tokio::signal::unix::{signal, SignalKind};
            let (Ok(mut term), Ok(mut int), Ok(mut hup)) = (
                signal(SignalKind::terminate()),
                signal(SignalKind::interrupt()),
                signal(SignalKind::hangup()),
            ) else {
                eprintln!("signal handlers unavailable; a kill will leave the KWin script loaded");
                return;
            };
            tokio::select! {
                _ = term.recv() => {}
                _ = int.recv() => {}
                _ = hup.recv() => {}
            }
            eprintln!("signal received; shutting down");
            quit.store(true, Ordering::Relaxed);
            let _ = tokio::task::spawn_blocking(move || stop.shutdown()).await;
            // The main loop leaves within a tick. If it is wedged, leave
            // anyway: the script is stopped, nothing else needs unwinding.
            tokio::time::sleep(Duration::from_secs(3)).await;
            std::process::exit(0);
        });
    }
    let leaving = {
        let (quit, game_over) = (quit.clone(), game_over.clone());
        move || quit.load(Ordering::Relaxed) || game_over.as_ref().is_some_and(|f| f.load(Ordering::Relaxed))
    };
    // The feed's first burst, folded in whole: it reports focus and
    // visibility only on change, so an event dropped here would stay wrong
    // until the user alt-tabs. There is no waiting for a game window: under
    // Proton it appears well after the overlay starts, and the overlay
    // surface is created when the first real geometry arrives.
    let mut win = khaloni_poe2::platform::GameWindowState::wait_for_geometry(
        &kwin.rx,
        std::time::Duration::from_millis(500),
    );
    phase("window feed read");
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
    // The portal may sit on a permission dialog for as long as the user
    // leaves it there; the game exiting or a signal ends the wait.
    let token_store = khaloni_poe2::config::RestoreToken::new();
    let saved_token = token_store.load();
    let start = rt.block_on(async {
        let gone = async {
            while !leaving() {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        };
        tokio::select! {
            r = capture::portal_session(saved_token.as_deref()) => Some(r),
            _ = gone => None,
        }
    });
    let Some(start) = start else {
        eprintln!("asked to leave during startup");
        return Ok(());
    };
    let start = start?;
    phase("capture session ready");
    if let Some(tok) = &start.new_token {
        if let Err(e) = token_store.save(tok) {
            eprintln!("capture: restore token not saved: {e}");
            notices.push_back("capture permission could not be saved: it will be asked again next start".into());
        }
    }
    // Hotkeys. `bound` is the binding set in force with the action behind
    // each id; a fired id is looked up there, never in the live config.
    let (hk_tx, hk_rx) = mpsc::channel();
    let (hk_status_tx, hk_status_rx) = mpsc::channel::<String>();
    let mut bound = khaloni_poe2::bindings::resolve(&cfg);
    for c in &bound.conflicts {
        eprintln!("hotkey conflict: {}", c.message);
        notices.push_back(c.message.clone());
    }
    let rebind = khaloni_poe2::platform::HotkeyRebind::new();
    // What a restarted listener binds: always the newest set.
    let wanted_bindings = Arc::new(std::sync::Mutex::new(bound.bindings.clone()));
    {
        let (hk_tx, rebind, wanted) = (hk_tx.clone(), rebind.clone(), wanted_bindings.clone());
        rt.spawn(async move {
            // `listen_with` returns only when the hotkeys are dead. Say so,
            // and bind again: a portal hiccup must not cost every hotkey
            // for the rest of the session.
            let mut wait = Duration::from_secs(5);
            loop {
                let bindings = wanted.lock().unwrap_or_else(|e| e.into_inner()).clone();
                let began = std::time::Instant::now();
                let end = khaloni_poe2::platform::hotkeys::listen_with(hk_tx.clone(), bindings, rebind.clone()).await;
                let why = match end {
                    Ok(()) => "the listener ended".to_string(),
                    Err(e) => e.to_string(),
                };
                eprintln!("hotkeys stopped: {why}");
                if began.elapsed() > Duration::from_secs(60) {
                    wait = Duration::from_secs(5);
                }
                let _ = hk_status_tx.send(format!("hotkeys stopped ({why}); binding again in {}s", wait.as_secs()));
                tokio::time::sleep(wait).await;
                wait = (wait * 2).min(Duration::from_secs(120));
            }
        });
    }

    // System tray: quick actions without a hotkey. A missing tray host
    // (no StatusNotifier) is not fatal — everything works without it.
    let (tray_tx, tray_rx) = mpsc::channel();
    if let Err(e) = khaloni_poe2::tray::spawn(tray_tx) {
        eprintln!("tray unavailable: {e}");
    }

    // Game-log tail: it feeds the run tracker behind the market panel's
    // "My runs" tab, which also reads the maps played before this start. A
    // missing Client.txt just means the tab stays empty (the tail retries
    // the open forever).
    let runs_hub = khaloni_poe2::myruns::Hub::new();
    match cfg.client_log_path.as_ref().map(std::path::PathBuf::from).or_else(khaloni_poe2::gamelog_tail::default_log_path) {
        Some(p) => {
            khaloni_poe2::gamelog_tail::spawn_with_runs(p, runs_hub.clone());
        }
        None => {
            runs_hub.set_log_missing();
            eprintln!("game log not found; run tracking off (set client_log_path)");
        }
    }
    {
        // The stash tracker starts with these credentials, so they are
        // what decides whether income can be shown.
        let access = khaloni_poe2_core::income::StashAccess::from_credentials(&cfg.account_name, &cfg.poesessid);
        let league = current_league.clone();
        khaloni_poe2::myruns::spawn_view_worker(runs_hub.clone(), move || (league.name(), access.clone()));
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
            cfg.poesessid.clone(),
            svc.clone(),
            wealth_tx,
            runs_hub.clone(),
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
            notices.push_back(format!("price check unavailable: {e}"));
            None
        }
    };
    // Set true while a price check is running on the injector thread so a
    // second F7 does not queue another; reset when its result is drained,
    // or by `COPY_REPLY_TIMEOUT` when the reply never comes.
    let price_check_in_flight = Arc::new(AtomicBool::new(false));
    let mut price_check_started: Option<std::time::Instant> = None;
    let (clip_tx, clip_rx) = mpsc::channel::<anyhow::Result<String>>();
    // Copy-hovered actions that are not price checks (resource shortcuts,
    // map analysis) share one reply channel; `pending_action` says what the
    // in-flight copy was for.
    let (action_tx, action_rx) = mpsc::channel::<anyhow::Result<String>>();
    let mut pending_action: Option<PendingAction> = None;
    // Map-mod rules: built-in seed plus any config-added needles. Rebuilt
    // on config hot-reload so settings edits apply without a relaunch.
    let mut map_rules = build_map_rules(&cfg);
    // Reference data for the price card's tier badges loads (cached,
    // fetched once) on a background thread so a cold fetch never blocks
    // startup.
    let reference: std::sync::Arc<std::sync::OnceLock<khaloni_poe2::refcache::Reference>> =
        std::sync::Arc::new(std::sync::OnceLock::new());
    {
        let reference = reference.clone();
        let cache = cache.clone();
        std::thread::Builder::new().name("reference-data".into()).spawn(move || {
            let r = khaloni_poe2::refcache::reference_data(&cache);
            eprintln!("reference data ready: {} affixes, {} items", r.affixes.len(), r.items.len());
            let _ = reference.set(r);
        })?;
    }
    // The config as the settings window last saved it, shared with the
    // trade worker and the OCR thread: a clone taken at startup priced the
    // rows by the old tier and divine thresholds while the popup already
    // used the new ones.
    let live_cfg = Arc::new(std::sync::RwLock::new(cfg.clone()));
    // The request budget: what the reward rows may spend on the trade
    // site behind the user's back. Every request the user makes is noted
    // here, and the rows keep a minute's distance from it.
    let budget: Arc<std::sync::Mutex<khaloni_poe2::budget::Budget>> = Arc::default();
    // The craft planner's worker (started once the exchange path it prices
    // through exists, below); the trade worker hands it every price
    // check's listings from the start.
    let (craft_req_tx, craft_req_rx) = mpsc::channel::<CraftReq>();
    let (craft_done_tx, craft_done_rx) = mpsc::channel::<CraftDone>();
    // Trade appraisal worker: rare items parsed from the clipboard get a
    // background search+fetch against the official trade API (strictly
    // rate limited inside TradeClient); results return on this channel.
    let (appraise_tx, appraise_rx) = mpsc::channel::<AppraiseDone>();
    // Two queues into the worker: the user's own checks are served before
    // anything a reward-row scan asked for.
    let (appraise_req_tx, appraise_req_rx) = khaloni_poe2::appraise::priority_channel::<AppraiseReq>();
    let (exch_tx, exch_rx) = mpsc::channel::<ExchangeDone>();
    // Exchange-catalog display names, published by the trade worker once the
    // static list arrives; the OCR worker extends its match vocab with them.
    let exch_names: std::sync::Arc<std::sync::OnceLock<Vec<String>>> =
        std::sync::Arc::new(std::sync::OnceLock::new());
    let currency_map: CurrencyMap = Arc::default();
    // Specific-gem price cache, shared with the OCR pricer.
    let gem_map: GemMap = Arc::default();
    // Number of the newest item check; see `AppraiseReq`.
    let check_generation = Arc::new(std::sync::atomic::AtomicU64::new(0));
    {
        let worker = TradeWorker {
            done_tx: appraise_tx.clone(),
            exch_tx: exch_tx.clone(),
            league: cfg.league.clone(),
            current: current_league.clone(),
            cache_dir: cache.clone(),
            gem_map: gem_map.clone(),
            svc: svc.clone(),
            exch_names: exch_names.clone(),
            reference: reference.clone(),
            generation: check_generation.clone(),
            client: Default::default(),
            catalogs: Catalogs {
                stats: Default::default(),
                currencies: Default::default(),
                items: Default::default(),
            },
            ee2: Default::default(),
            last_search: khaloni_poe2::appraise::ReuseSlot::new(khaloni_poe2::appraise::REUSE_TTL),
            last_check: None,
            cfg: live_cfg.clone(),
            craft_tx: craft_req_tx.clone(),
        };
        std::thread::Builder::new().name("trade-worker".into()).spawn(move || worker.run(appraise_req_rx))?;
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
    // Shared with the OCR worker below: it owns the PanelGate and
    // stores whether it's currently open here every pass; the capture
    // thread only reads it, to pick its 120ms/300ms throttle. An atomic is
    // the simplest correct way to move this one bit across the thread
    // boundary without a second channel (see capture::consume's doc comment).
    let panel_open = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    // Set by the main loop while nothing is being priced (game hidden or
    // unattended, user pause): the capture then drops frames
    // before any pixel work instead of converting them for nobody.
    let capture_paused = Arc::new(AtomicBool::new(false));
    let cap_ev_rx = spawn_capture(
        &rt,
        start,
        region_rx,
        region,
        ftx,
        panel_open.clone(),
        full_tx,
        capture_paused.clone(),
    )?;

    // OCR worker: frames in, priced rows out. `pipeline_paused` follows the
    // scan policy every tick (focus loss, occlusion, the tray pause), so
    // tesseract is not fed; `capture_paused` follows it too, so the capture
    // thread stops converting frames while still dequeuing them.
    let pipeline_paused = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    // Priced rows travel with the league they were priced in: a scan that
    // was under way during a league change arrives after the stabilizer was
    // cleared, and is told apart by that name.
    let (rows_tx, rows_rx) = mpsc::channel::<(Option<String>, khaloni_poe2::stabilize::ScanResult)>();
    let svc_ocr = svc.clone();
    let ocr_cfg = live_cfg.clone();
    // Full frames feed two consumers with very different costs: region
    // detection (pure math, ~40ms) and rumour OCR (seconds when any
    // parchment-like blob — including combat explosions — is on screen).
    // They MUST NOT share a thread: reward panels open right after combat,
    // exactly when a shared thread would still be chewing explosion frames,
    // which measured as 30s+ first-detection latency. The fan-out clones
    // each ~8MB frame once per 700ms — noise next to one OCR pass.
    let (det_tx, det_rx) = mpsc::sync_channel::<image::GrayImage>(1);
    let (rum_tx, rum_rx) = mpsc::sync_channel::<image::GrayImage>(1);
    std::thread::Builder::new().name("frame-fanout".into()).spawn(move || {
        for frame in full_rx {
            let _ = det_tx.try_send(frame.clone());
            let _ = rum_tx.try_send(frame);
        }
    })?;
    // Region-detection worker: always fast, never blocked by OCR.
    {
        let scan_geom = scan_geom.clone();
        let region_ready = region_ready.clone();
        let panel_open_det = panel_open.clone();
        std::thread::Builder::new().name("region-detect".into()).spawn(move || {
            let dbg = std::env::var("KHALONI_DEBUG").is_ok();
            let mut last_region: Option<Rect> = None;
            for frame in det_rx {
                // While the panel gate is open the region is LOCKED:
                // the stabilizer's scroll origin must not move under it.
                // Redetect only when closed.
                // Live-debug: keep the latest full frame on disk so a
                // detection miss can be reproduced offline against the
                // exact pixels (overwritten each ~700ms frame).
                if std::env::var("KHALONI_REGION_DUMP").is_ok() {
                    let _ = frame.save(std::env::temp_dir().join("khaloni-frame.png"));
                }
                // Detection runs with the lock free: it takes ~40ms, and the
                // paint path reads this geometry every 16ms tick.
                let detect = !panel_open_det.load(Ordering::Relaxed);
                let found = if detect {
                    khaloni_poe2::autoregion::detect_reward_region(&frame).map(|r| Rect {
                        x: r.x0 as i32,
                        y: r.y0 as i32,
                        w: r.x1 - r.x0,
                        h: r.y1 - r.y0,
                    })
                } else {
                    None
                };
                let mut geom = scan_geom.lock().unwrap_or_else(|e| e.into_inner());
                geom.0 = Some((frame.width(), frame.height()));
                if detect {
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
        })?;
    }
    // Rumour recognizer worker: OCR-heavy, allowed to lag; latest-only
    // channels mean it just skips to the newest frame when it falls behind.
    {
        let rumour_csv = Config::path().parent().map(|d| d.join("rumours.csv"));
        let paused_rumour = pipeline_paused.clone();
        std::thread::Builder::new().name("rumour-ocr".into()).spawn(move || {
            let dbg = std::env::var("KHALONI_DEBUG").is_ok();
            // Remembers the last panel it read: an unchanged tooltip costs
            // no tesseract passes (see rumours::RumourScanner).
            let mut scanner = khaloni_poe2::rumours::RumourScanner::default();
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
                    let _ = frame.save(std::env::temp_dir().join("poe2-live-frame.png"));
                }
                let passes_before = scanner.ocr_passes;
                let hits = scanner.recognize(engine, &frame, idx);
                let passes = scanner.ocr_passes - passes_before;
                // A remembered panel is not news; log what was read anew.
                if !hits.is_empty() && passes > 0 {
                    eprintln!(
                        "RUMOURS {} in {}ms ({passes} ocr passes): {}",
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
                        "rumour scan: {}x{} {} hits, {passes} ocr passes in {}ms",
                        frame.width(),
                        frame.height(),
                        hits.len(),
                        t.elapsed().as_millis()
                    );
                }
                // Always forward (even empty) so the render loop clears
                // badges the instant the tooltip leaves the screen.
                if rumour_tx.send(hits).is_err() {
                    break; // main loop gone
                }
            }
        })?;
    }

    let paused_ocr = pipeline_paused.clone();
    // The reward-panel pricer's handle to the specific-gem cache + trade
    // worker, through the budget: a row's request goes only while the
    // user's own checks keep their room.
    let row_gate = RowGate {
        budget: budget.clone(),
        limiters: khaloni_poe2_core::trade::Limiters::global(),
        cfg: live_cfg.clone(),
        last_refusal_logged: Arc::default(),
    };
    let gem_cache = GemCache { map: gem_map.clone(), req_tx: appraise_req_tx.clone(), gate: row_gate.clone() };
    {
        let worker = CraftWorker {
            done_tx: craft_done_tx,
            league: cfg.league.clone(),
            current: current_league.clone(),
            cache_dir: cache.clone(),
            svc: svc.clone(),
            cfg: live_cfg.clone(),
            budget: budget.clone(),
            currency: CurrencyCache { map: currency_map.clone(), req_tx: appraise_req_tx.clone(), gate: row_gate.clone() },
            unpriced: Default::default(),
            client: Default::default(),
            sources: CraftSources {
                craft: Default::default(),
                ee2: Default::default(),
                stats: Default::default(),
                currency_names: Default::default(),
            },
            store: Default::default(),
            last_buy: None,
            serving: None,
        };
        std::thread::Builder::new().name("craft-worker".into()).spawn(move || worker.run(craft_req_rx))?;
    }
    let currency_cache = CurrencyCache { map: currency_map.clone(), req_tx: appraise_req_tx.clone(), gate: row_gate };
    let region_ready_ocr = region_ready.clone();
    let scan_geom_ocr = scan_geom.clone();
    // Reward rows: motion, the bar gate, band detection and template hits
    // run on every captured frame on one thread; tesseract runs on its
    // own, on the newest frame that needs it (see reward_pipeline).
    match ocr::OcrEngine::new() {
        Err(_) => eprintln!("tesseract init failed; OCR disabled"),
        Ok(engine) => {
            // Learned-template store: identifies previously seen reward
            // bands in well under a millisecond, bypassing tesseract; OCR
            // remains the teacher for first encounters. Persisted across
            // sessions. Looked up by the tracking thread, taught by the
            // reading one.
            let tpl_path = directories::ProjectDirs::from("", "", "khaloni-poe2")
                .map(|d| d.cache_dir().join("templates.bin"));
            let tstore = Arc::new(std::sync::Mutex::new(
                tpl_path.as_deref().map(khaloni_poe2::template::TemplateStore::load).unwrap_or_default(),
            ));
            // Rumour annotations: optional dataset at config_dir/rumours.csv
            // (community sheet snapshot). Absent file = feature off; rumour
            // lines then render nothing, exactly as before the wiring.
            let rumours = Config::path()
                .parent()
                .map(|d| d.join("rumours.csv"))
                .and_then(|p| std::fs::read_to_string(p).ok())
                .map(|csv| {
                    let idx = khaloni_poe2_core::rumour::RumourIndex::new(khaloni_poe2_core::rumour::parse_csv(&csv));
                    eprintln!("rumour dataset loaded: {} entries", idx.len());
                    idx
                });
            if rumours.is_none() {
                eprintln!("no rumours.csv in config dir; rumour annotations off");
            }
            let resolver = TemplateResolver { tstore: tstore.clone(), svc: svc_ocr.clone(), cfg: ocr_cfg.clone() };
            let reader = RewardReader {
                engine,
                scan_cache: ocr::ScanCache::default(),
                tstore,
                tpl_path,
                tpl_saved_at: std::time::Instant::now(),
                svc: svc_ocr,
                cfg: ocr_cfg,
                exch_names: exch_names.clone(),
                vocab: None,
                rumours,
                gem_cache,
                currency_cache,
                dbg: std::env::var("KHALONI_DEBUG").is_ok(),
                t0: std::time::Instant::now(),
            };
            let frames = frx.into_iter().filter_map(move |frame: khaloni_poe2::platform::RegionFrame| {
                // Until the detector has found a reward region, capture is
                // cropping the startup dummy rect: never scan that. While
                // paused, frames are dropped before any pixel work.
                if !region_ready_ocr.load(Ordering::Relaxed) || paused_ocr.load(Ordering::Relaxed) {
                    return None;
                }
                // The game UI scales with the height of the whole frame;
                // the region crop in hand says nothing about it.
                let full_h = scan_geom_ocr.lock().unwrap_or_else(|e| e.into_inner()).0.map_or(0, |(_, h)| h);
                Some(khaloni_poe2::reward_pipeline::Frame { gray: frame.gray, scale: ocr::UiScale::from_frame_height(full_h) })
            });
            khaloni_poe2::reward_pipeline::spawn(frames, resolver, reader, rows_tx, panel_open.clone())?;
        }
    }

    // The overlay surface lives on the output the game is on, so it is
    // created when the feed first reports a game window, and again whenever
    // the compositor closes it or the game moves to another output. Until
    // there is one, nothing is drawn and no hotkey types anything.
    let mut overlay_slot: Option<khaloni_poe2::platform::overlay::Overlay> = None;
    let mut overlay_retry_at = std::time::Instant::now();
    let mut first_present_logged = false;
    // Overlay opacity live-applies from config; a change must force a
    // repaint because an idle overlay keeps its last presented buffer.
    let mut last_opacity = f64::NAN;
    let renderer = khaloni_poe2::render::Renderer::new()?;

    // The tray's "Pause Pricing"; an input to `scanpolicy`.
    let mut user_paused = false;
    // Last known game rect. Only read once an overlay exists, which takes a
    // reported geometry; the feed state `win` says whether it is current.
    let mut game = win.rect.unwrap_or(Rect { x: 0, y: 0, w: 0, h: 0 });
    let mut stabilizer = khaloni_poe2::stabilize::Stabilizer::new();
    let mut hover = hover::HoverState::default();
    // A price check pressed while another window has focus: the game is
    // focused first and the copy waits for the feed to confirm it.
    let mut focus_gate = khaloni_poe2::pricecheck::FocusGate::default();
    // Where the cursor was when the current popup fired, and the placed
    // popup rect: move-away dismissal measures against these. None while
    // no popup is up.
    let mut popup_at: Option<((i32, i32), Rect)> = None;
    // Interactive Evaluate panel: model + the query its checkboxes edit
    // + placed top-left (global logical). While Some, the overlay's input
    // region covers the panel and clicks resolve through evaluate_ui.
    let mut apanel: Option<EvalPanel> = None;
    // What the open panel's listings came from; "Open site" opens this.
    let mut searched: Option<Searched> = None;
    // The item text of the check the open panel belongs to, so the same
    // item checked again while its search runs is not queued twice.
    let mut check_in_flight: Option<String> = None;
    // A hover currency check waiting on an exchange request a reward row
    // had already queued for the same name: (name, hovered stack).
    let mut awaiting_exchange: Option<(String, u32)> = None;
    // Which value box is being typed into (index into `panel.rows`, which is
    // what evaluate_ui's actions carry), and the digits typed so far.
    let mut editing: Option<(usize, khaloni_poe2::evaluate_ui::Field)> = None;
    let mut edit_buf = String::new();
    // The market panel, with its placed top-left in global logical
    // coordinates, joins the input region like the Evaluate panel but never
    // asks for the keyboard: everything in it is a click.
    let mut mkt_panel: Option<(khaloni_poe2::market_ui::Panel, (i32, i32))> = None;
    let mut mkt_feed = khaloni_poe2::market_ui::Feed::default();
    // The craft planner joins the input region the same way and is all
    // clicks too. `craft_item` is the item it was opened on; answers for a
    // panel older than `craft_generation` are dropped.
    let mut craft_panel: Option<CraftPanel> = None;
    let mut craft_item: Option<khaloni_poe2_core::craft::types::ItemState> = None;
    let mut craft_generation: u64 = 0;
    // An in-progress panel drag: (grab point in surface px, panel's global
    // position when the grab began). Deliberately NOT persisted anywhere, so
    // each new price check reopens the panel at its freshly-placed spot.
    let mut panel_drag: Option<PanelDrag> = None;
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
    // Live config reload: the settings window writes config.toml; polling
    // its mtime (once a second) applies the change to this loop, to the OCR
    // thread (through `live_cfg`) and to the hotkeys (through `rebind`).
    // A changed league moves everything priced at once; see `league`.
    let mut cfg_mtime = std::fs::metadata(Config::path()).and_then(|m| m.modified()).ok();
    let mut last_cfg_poll = std::time::Instant::now();
    // Row-request failures are worth one note, not one per scan.
    let mut last_row_error_note = std::time::Instant::now() - Duration::from_secs(3600);
    let mut capture_lost = false;
    // The price snapshot the row caches last saw; a new one (a refresh, a
    // league change) lets requests the site refused be asked again.
    let mut last_inputs: Option<Arc<prices::Snapshot>> = None;

    loop {
        if leaving() {
            eprintln!("closing: the game exited or a signal arrived");
            return Ok(());
        }
        let snap = svc.snapshot();
        if !last_inputs.as_ref().is_some_and(|s| Arc::ptr_eq(s, &snap)) {
            currency_map.lock().unwrap_or_else(|e| e.into_inner()).inputs_changed();
            gem_map.lock().unwrap_or_else(|e| e.into_inner()).inputs_changed();
            last_inputs = Some(snap);
        }

        // Window feed first: everything below acts on the state it leaves.
        let mut game_went = false;
        let mut game_took_focus = false;
        while let Ok(ev) = kwin.rx.try_recv() {
            match ev {
                khaloni_poe2::platform::GameWindowEvent::GameGone => game_went = true,
                khaloni_poe2::platform::GameWindowEvent::Active(khaloni_poe2::platform::Focus::Game) => {
                    game_took_focus = true;
                }
                _ => {}
            }
            win.apply(ev);
        }
        // The scan region is capture-space and auto-detected, so a window
        // move needs no region update; label placement reads the live game
        // position every paint.
        if let Some(r) = win.rect {
            game = r;
        }
        let game_present = win.present();
        let game_visible = win.visible;
        let focus = win.focus;
        // The strict form injection needs; scanning and drawing go through
        // `scanpolicy`, which also accepts focus on our own overlay.
        let game_focused = win.game_focused();
        let game_pos = (game.x, game.y);
        let game_center = (game.x + game.w as i32 / 2, game.y + game.h as i32 / 2);
        // Live pointer position (global logical), fed by the KWin script's
        // cursor timer; the game center until the first move.
        let cursor_pos = win.cursor.unwrap_or(game_center);

        // (Re)create the overlay surface: none yet, the compositor closed
        // it (output switched off or replugged), or the game now sits on
        // another output than the surface.
        let stale_surface = overlay_slot.as_ref().is_some_and(|o| {
            o.is_closed() || (game_present && off_output(game_center, o.output_pos(), o.size()))
        });
        if (overlay_slot.is_none() || stale_surface)
            && game_present
            && std::time::Instant::now() >= overlay_retry_at
        {
            overlay_retry_at = std::time::Instant::now() + Duration::from_secs(1);
            let first = !OVERLAY_RUNNING.load(Ordering::Relaxed);
            let opened = if first {
                // Startup flavour: a compositor without layer-shell is a
                // reason not to start at all.
                khaloni_poe2::platform::overlay::Overlay::new(game_center).map_err(|e| {
                    match e.downcast::<khaloni_poe2::platform::OverlayError>() {
                        Ok(oe) => oe,
                        Err(other) => khaloni_poe2::platform::OverlayError::Startup(other.to_string()),
                    }
                })
            } else {
                khaloni_poe2::platform::overlay::Overlay::open(game_center)
            };
            match opened {
                Ok(mut o) => {
                    // The window feed hands focus back to the game once the
                    // overlay stops wanting the keyboard (KWin never does
                    // that on its own).
                    o.bind_keyboard_flag(kwin.keyboard_wanted.clone());
                    o.set_opacity(cfg.overlay_opacity.max(0.1));
                    last_opacity = cfg.overlay_opacity;
                    // A new surface starts from defaults: no keyboard, no
                    // input region, no buffer.
                    o.set_keyboard(editing.is_some())?;
                    sync_input_region(&mut o, &renderer, &apanel, &mkt_panel, &craft_panel)?;
                    pixmap = None;
                    last_frame = None;
                    panel_drag = None;
                    overlay_slot = Some(o);
                    if first {
                        OVERLAY_RUNNING.store(true, Ordering::Relaxed);
                        phase("overlay surface up");
                    } else {
                        eprintln!("overlay surface rebuilt on the game's output");
                    }
                }
                // All monitors off: keep asking until one is back.
                Err(khaloni_poe2::platform::OverlayError::NoOutput) => {
                    if stale_surface {
                        overlay_slot = None;
                    }
                }
                Err(e) => return Err(e.into()),
            }
        }
        let Some(overlay) = overlay_slot.as_mut() else {
            // No surface: nothing can be shown, so nothing is priced and no
            // hotkey acts. A press now is dropped, not queued to fire later
            // into whatever has focus by then. The settings window and the
            // tray's Quit need no surface and keep working.
            pipeline_paused.store(true, Ordering::Relaxed);
            capture_paused.store(true, Ordering::Relaxed);
            while rows_rx.try_recv().is_ok() {}
            while rumour_rx.try_recv().is_ok() {}
            let mut wants_settings = false;
            while let Ok(hk) = hk_rx.try_recv() {
                if let khaloni_poe2::platform::Hotkey::Extra(id) = hk {
                    wants_settings |= bound.actions.get(&id) == Some(&khaloni_poe2::bindings::Action::Settings);
                }
            }
            while let Ok(ev) = tray_rx.try_recv() {
                match ev {
                    khaloni_poe2::tray::TrayEvent::Quit => return Ok(()),
                    khaloni_poe2::tray::TrayEvent::OpenSettings => wants_settings = true,
                    _ => {}
                }
            }
            if wants_settings {
                if let Err(e) = open_settings() {
                    eprintln!("{e}");
                    notices.push_back(e);
                }
            }
            std::thread::sleep(Duration::from_millis(100));
            continue;
        };
        // A broken Wayland connection is the one overlay error nothing
        // here can recover (see `OverlayError`).
        overlay.pump()?;

        let game_rect = Rect { x: game_pos.0, y: game_pos.1, w: game.w, h: game.h };
        // Where a popup shown this tick goes: beside the cursor.
        let anchor = |hover: &hover::HoverState| {
            hover.current.as_ref().map(|p| {
                let size = renderer.popup_size(p);
                let (px, py) = khaloni_poe2::popup_pos::place(cursor_pos, size, game_rect);
                (cursor_pos, Rect { x: px, y: py, w: size.0 as u32, h: size.1 as u32 })
            })
        };

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
            // "league: X" once X's prices are in, or why they are not.
            if let Some(n) = league_announcer.poll(&svc.snapshot()) {
                notices.push_back(n);
            }
            if let Ok(m) = std::fs::metadata(Config::path()).and_then(|md| md.modified()) {
                if cfg_mtime != Some(m) {
                    cfg_mtime = Some(m);
                    match Config::load() {
                        Ok(mut new_cfg) => {
                            notices.extend(new_cfg.notices.drain(..));
                            let priced = khaloni_poe2::league::Priced {
                                currency: &currency_map,
                                gems: &gem_map,
                                stabilizer: &mut stabilizer,
                                hover: &mut hover,
                                awaiting_exchange: &mut awaiting_exchange,
                            };
                            if let Some(sw) = khaloni_poe2::league::switch(
                                &current_league,
                                priced,
                                &svc,
                                &mut league_announcer,
                                &new_cfg.league,
                            ) {
                                eprintln!("league: {} -> {}", sw.from, sw.to);
                                // Rows priced in the old league may still be
                                // on their way from the scan thread; they
                                // carry its name and are refused below.
                                popup_at = None;
                                last_frame = None;
                                // The market rows are the old league's; the
                                // feed brings the new league's when they land.
                                mkt_feed = khaloni_poe2::market_ui::Feed::default();
                                if let Some((p, _)) = mkt_panel.as_mut() {
                                    *p = khaloni_poe2::market_ui::Panel::loading(&sw.to);
                                }
                                // A plan's prices and a scan's listings are
                                // the old league's; the next press uses the
                                // new one.
                                if let Some((p, _)) = craft_panel.as_mut() {
                                    p.note = Some(format!(
                                        "the league changed to {}: plans and scans shown are the old league's; press Plan or Run again",
                                        sw.to
                                    ));
                                }
                                notices.push_back(khaloni_poe2::league::loading_text(&sw.to));
                                // A card that is open keeps its listings and
                                // says whose they are.
                                if let (Some(p), Some(done)) = (apanel.as_mut(), searched.as_ref()) {
                                    p.0.status = khaloni_poe2::league::old_league_status(&done.league, &p.0.status);
                                }
                            }
                            let new_bound = khaloni_poe2::bindings::resolve(&new_cfg);
                            if new_bound.bindings != bound.bindings {
                                *wanted_bindings.lock().unwrap_or_else(|e| e.into_inner()) =
                                    new_bound.bindings.clone();
                                rebind.rebind(new_bound.bindings.clone());
                                eprintln!("hotkeys: binding the changed set");
                            }
                            for c in new_bound.conflicts.iter().filter(|c| !bound.conflicts.contains(c)) {
                                notices.push_back(c.message.clone());
                            }
                            bound = new_bound;
                            map_rules = build_map_rules(&new_cfg);
                            *live_cfg.write().unwrap_or_else(|e| e.into_inner()) = new_cfg.clone();
                            cfg = new_cfg;
                        }
                        // The old settings stay in force, and the user is
                        // told their edit did not take.
                        Err(e) => {
                            eprintln!("config reload failed: {e:#}");
                            notices.push_back(format!("settings not applied: config.toml does not parse ({e})"));
                        }
                    }
                }
            }
            // A scan the settings window asked for: the panel states its
            // cost and waits for Run; nothing is sent from here.
            if let Some(name) = Config::path().parent().and_then(khaloni_poe2::craft_flow::take_scan_request) {
                let snap = svc.snapshot();
                let panel = craft_panel.get_or_insert_with(|| {
                    let rates = khaloni_poe2::craft_flow::rates(&snap.table, cfg.divine_threshold);
                    (khaloni_poe2::craft_flow::empty_panel(rates, craft_prices_line(&snap)), craft_pos(game_rect))
                });
                let p = &mut panel.0;
                p.view = khaloni_poe2::craft_ui::View::Flips;
                p.busy = Some(format!("working out what scanning \"{name}\" costs"));
                if craft_req_tx.send(CraftReq::StateScan { name }).is_err() {
                    p.busy = None;
                    p.note = Some("the craft planner is not running: restart the overlay".to_string());
                }
                sync_input_region(overlay, &renderer, &apanel, &mkt_panel, &craft_panel)?;
            }
        }

        // Latest rumour scan wins; empty vec clears badges when the panel closes.
        while let Ok(r) = rumour_rx.try_recv() {
            latest_rumours = r;
        }

        // The focus a price check asked for has landed: copy now.
        if game_took_focus && game_focused && focus_gate.focused(std::time::Instant::now()) {
            if let Some(inj) = &injector {
                if !price_check_in_flight.swap(true, Ordering::AcqRel) {
                    price_check_started = Some(std::time::Instant::now());
                    inj.submit(clip_tx.clone(), 0, cfg.advanced_copy);
                }
            }
        }
        if game_went {
            stabilizer.clear();
            let had_panel = close_eval(&mut apanel, &mut editing, &mut edit_buf, &mut panel_drag, &mut searched)
                | mkt_panel.take().is_some()
                | craft_panel.take().is_some();
            if had_panel {
                overlay.set_keyboard(false)?;
                overlay.set_interactive(None)?;
            }
            overlay.hide()?;
            last_frame = None;
        }
        while let Ok(why) = hk_status_rx.try_recv() {
            notices.push_back(why);
        }
        while let Ok(ev) = cap_ev_rx.try_recv() {
            use khaloni_poe2::platform::CaptureEvent;
            match ev {
                CaptureEvent::Streaming => {
                    if std::mem::take(&mut capture_lost) {
                        notices.push_back("screen capture is back".into());
                    }
                }
                CaptureEvent::Lost(why) => {
                    capture_lost = true;
                    notices.push_back(format!("screen capture lost ({why}): reconnecting, reward prices are off"));
                }
                CaptureEvent::NewToken(tok) => {
                    if let Err(e) = token_store.save(&tok) {
                        eprintln!("capture: restore token not saved: {e}");
                    }
                }
                CaptureEvent::GaveUp(why) => {
                    notices.push_back(format!("screen capture stopped ({why}): restart the overlay for reward prices"));
                }
            }
        }
        // Tray menu actions reuse the hotkey paths where one exists, so the
        // two entry points cannot drift apart.
        while let Ok(ev) = tray_rx.try_recv() {
            match ev {
                khaloni_poe2::tray::TrayEvent::OpenSettings => {
                    if let Err(e) = open_settings() {
                        notices.push_back(e);
                    }
                }
                khaloni_poe2::tray::TrayEvent::TogglePause => {
                    // Kept as the user's own state: the pause flag the
                    // workers read is recomputed from it every tick (see
                    // `scanpolicy`), so writing that flag here did nothing.
                    user_paused = !user_paused;
                    if user_paused {
                        stabilizer.clear();
                    }
                    hover.show_note(if user_paused { "pricing paused" } else { "pricing resumed" });
                    popup_at = anchor(&hover);
                }
                khaloni_poe2::tray::TrayEvent::Quit => return Ok(()),
            }
        }
        while let Ok(u) = update_rx.try_recv() {
            // One passive note; installing lives in the settings window so
            // an update never interrupts play.
            notices.push_back(format!("{} available — see Settings", u.version));
        }
        while let Ok(alert) = alert_rx.try_recv() {
            let khaloni_poe2::livesearch::Alert::NewListings { search, count } = alert;
            hover.show_note(&format!("{search}: {count} new listing(s)"));
            popup_at = anchor(&hover);
        }
        while let Ok(hk) = hk_rx.try_recv() {
            use khaloni_poe2::bindings::Action as Bound;
            // Price check arrives as its own variant; everything else is
            // looked up in the set that was bound, so an id from a list the
            // user has since edited fires what it was bound to or nothing at
            // all.
            let action = match hk {
                khaloni_poe2::platform::Hotkey::PriceCheck => Bound::PriceCheck,
                khaloni_poe2::platform::Hotkey::Extra(id) => match bound.actions.get(&id) {
                    Some(a) => a.clone(),
                    None => {
                        eprintln!("hotkey {id} is not in the current binding set; ignored");
                        continue;
                    }
                },
            };
            match action {
                Bound::PriceCheck => {
                    // The gate decides: copy now (game focused), focus the
                    // game first (pointer over it, another window focused;
                    // the copy follows on the feed's Active event), or a
                    // note. A press never sends Ctrl+C into some other
                    // window. The swap keeps a second press from queueing
                    // another copy while one runs on the injector thread.
                    let Some(inj) = &injector else {
                        hover.show_note("price check unavailable: no access to /dev/uinput");
                        popup_at = anchor(&hover);
                        continue;
                    };
                    let press = if price_check_in_flight.load(Ordering::Acquire) {
                        khaloni_poe2::pricecheck::Press::Ignore
                    } else {
                        focus_gate.press(
                            std::time::Instant::now(),
                            game_focused,
                            game_present,
                            cursor_pos,
                            game_rect,
                        )
                    };
                    match press {
                        khaloni_poe2::pricecheck::Press::Copy => {
                            if !price_check_in_flight.swap(true, Ordering::AcqRel) {
                                price_check_started = Some(std::time::Instant::now());
                                inj.submit(clip_tx.clone(), 0, cfg.advanced_copy);
                            }
                        }
                        khaloni_poe2::pricecheck::Press::FocusGame => {
                            eprintln!("price check: focusing the game first");
                            kwin.focus_game();
                        }
                        khaloni_poe2::pricecheck::Press::Note(text) => {
                            hover.show_note(text);
                            popup_at = anchor(&hover);
                        }
                        khaloni_poe2::pricecheck::Press::Ignore => {}
                    }
                }
                // The settings hotkey opens the native settings window in
                // its own process. No focus gate: it's an out-of-game
                // window; config changes flow back via the mtime watcher.
                Bound::Settings => {
                    if let Err(e) = open_settings() {
                        notices.push_back(e);
                    }
                }
                Bound::Market => {
                    if mkt_panel.take().is_none() {
                        let pos = (game_pos.0 + (game.w as i32) / 2 - 520, game_pos.1 + 120);
                        mkt_panel = Some((khaloni_poe2::market_ui::Panel::loading(&current_league.name()), pos));
                    }
                    sync_input_region(overlay, &renderer, &apanel, &mkt_panel, &craft_panel)?;
                }
                // The rest type or copy, so they only act while the game is
                // focused, never into another window.
                Bound::Macro(_) | Bound::Shortcut(_) | Bound::Upgrade | Bound::Craft if !game_focused => {}
                Bound::Macro(message) => {
                    if let Some(inj) = &injector {
                        inj.type_text(message, cfg.macro_open_delay_ms);
                    }
                }
                Bound::Shortcut(url) => {
                    if let Some(inj) = &injector {
                        if pending_action.is_none() {
                            pending_action = Some(PendingAction::Shortcut(url));
                            inj.submit(action_tx.clone(), 300, cfg.advanced_copy);
                        }
                    }
                }
                Bound::Upgrade => {
                    if let Some(inj) = &injector {
                        if pending_action.is_none() {
                            pending_action = Some(PendingAction::UpgradeCheck);
                            inj.submit(action_tx.clone(), 300, cfg.advanced_copy);
                        }
                    }
                }
                // The planner reads the item from its copy, like a price
                // check; the reading happens on the craft worker.
                Bound::Craft => match &injector {
                    Some(inj) => {
                        if pending_action.is_none() {
                            pending_action = Some(PendingAction::Craft);
                            inj.submit(action_tx.clone(), 300, cfg.advanced_copy);
                        }
                    }
                    None => {
                        hover.show_note("craft planner unavailable: no access to /dev/uinput");
                        popup_at = anchor(&hover);
                    }
                },
            }
        }

        // Drain copy-hovered action results (resource shortcuts, upgrades).
        while let Ok(result) = action_rx.try_recv() {
            let action = pending_action.take();
            match (action, result) {
                (Some(PendingAction::Shortcut(url)), Ok(text)) => {
                    if let Err(e) = open_resource(&url, &text) {
                        hover.show_note(&e);
                        popup_at = anchor(&hover);
                    }
                }
                (Some(PendingAction::UpgradeCheck), Ok(text)) => {
                    match khaloni_poe2_core::item::parse_item(&text) {
                        Ok(item) => {
                            let generation = check_generation.fetch_add(1, Ordering::AcqRel) + 1;
                            budget.lock().unwrap_or_else(|e| e.into_inner()).note_user(std::time::Instant::now());
                            if appraise_req_tx.send_user(AppraiseReq::Upgrade { item, generation }).is_ok() {
                                hover.show_note("searching upgrades...");
                            } else {
                                hover.show_notice("trade search is not running: restart the overlay");
                            }
                        }
                        Err(_) => hover.show_note("hover an equipped item first"),
                    }
                    popup_at = anchor(&hover);
                }
                (Some(PendingAction::Craft), Ok(text)) => {
                    craft_generation += 1;
                    if craft_req_tx.send(CraftReq::Open { text, generation: craft_generation }).is_ok() {
                        hover.show_note("reading the item for the craft planner...");
                    } else {
                        hover.show_notice("the craft planner is not running: restart the overlay");
                    }
                    popup_at = anchor(&hover);
                }
                // The copy failed: say why (no wl-paste, a clipboard that
                // does not answer, modifier keys still held).
                (Some(_), Err(e)) => {
                    eprintln!("copy for a hotkey action: {e}");
                    hover.show_note(&e.to_string());
                    popup_at = anchor(&hover);
                }
                (None, _) => {}
            }
        }

        // A focus request the compositor never answered: say so, rather
        // than leaving the press looking dead.
        if focus_gate.timed_out(std::time::Instant::now()) {
            eprintln!("price check: the game did not take focus");
            hover.show_note("could not focus the game");
            popup_at = anchor(&hover);
        }
        // A copy whose reply never came (the injector thread wedged or
        // died mid-request) must not leave F7 ignored for the session.
        if price_check_started.is_some_and(|t| t.elapsed() >= COPY_REPLY_TIMEOUT)
            && price_check_in_flight.swap(false, Ordering::AcqRel)
        {
            price_check_started = None;
            eprintln!("price check: no reply from the copy within {COPY_REPLY_TIMEOUT:?}");
            hover.show_note("price check got no answer from the clipboard; try again");
            popup_at = anchor(&hover);
        }
        // Drain injected clipboard text: reprice against whatever the price
        // table looks like right now (not at the moment F7 was pressed).
        while let Ok(result) = clip_rx.try_recv() {
            price_check_in_flight.store(false, Ordering::Release);
            price_check_started = None;
            match result {
                Ok(text) if text.trim().is_empty() => {
                    hover.show_no_item();
                }
                Ok(text) => {
                    let snap = svc.snapshot();
                    let fresh = hover::Freshness { table_stale: snap.stale, uniques_stale: snap.uniques_stale };
                    // Every checked item's text is kept (newest 200, in the
                    // cache dir, or in KHALONI_ITEM_DUMP when set): a price
                    // that looks wrong can then be replayed through
                    // `tools/ee2-parity` against Exiled Exchange 2 instead
                    // of being argued from memory.
                    dump_item_text(&text);
                    match khaloni_poe2::league::not_ready(&snap, &current_league.name()) {
                        // Nothing is looked up in a table that is not this
                        // league's yet; an empty one would call every
                        // currency unknown and send it to the exchange.
                        Some(why) => {
                            hover.last_error = Some(why.clone());
                            hover.current = None;
                        }
                        None => hover.trigger_priced(&text, &snap.table, &snap.uniques, cfg.divine_threshold, fresh),
                    }
                    // One line per check: what was read and where it went.
                    // Cheap, and the only evidence a "nothing showed" report
                    // can be diagnosed from after the fact.
                    if let Some(p) = &hover.current {
                        let route = if hover.pending_appraisal.is_some() {
                            "trade search"
                        } else if hover.pending_currency.is_some() {
                            "exchange"
                        } else {
                            "local"
                        };
                        eprintln!(
                            "price check: {:?} -> {route}: {}",
                            p.title,
                            p.lines.first().map(|l| l.text.as_str()).unwrap_or("")
                        );
                    } else if let Some(e) = &hover.last_error {
                        eprintln!("price check: {e}");
                        let e = e.clone();
                        hover.show_note(&e);
                    }
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
                        let same_running =
                            check_in_flight.as_deref() == Some(item.raw.as_str()) && apanel.is_some();
                        if same_running {
                            // This very item's search is still running and
                            // its card is open: a second copy of it would
                            // only queue behind the first.
                            hover.show_note("already searching this item");
                        } else {
                            // A fresh check replaces any open panel.
                            if close_eval(&mut apanel, &mut editing, &mut edit_buf, &mut panel_drag, &mut searched) {
                                overlay.set_keyboard(false)?;
                                sync_input_region(overlay, &renderer, &apanel, &mkt_panel, &craft_panel)?;
                            }
                            let generation = check_generation.fetch_add(1, Ordering::AcqRel) + 1;
                            check_in_flight = Some(item.raw.clone());
                            budget.lock().unwrap_or_else(|e| e.into_inner()).note_user(std::time::Instant::now());
                            if appraise_req_tx.send_user(AppraiseReq::Auto { item, generation }).is_err() {
                                check_in_flight = None;
                                hover.show_notice("trade search is not running: restart the overlay");
                            }
                        }
                    }
                    if let Some((name, stack)) = hover.pending_currency.take() {
                        // A recent answer for this name is the answer; the
                        // exchange is only asked when there is none, or it
                        // has aged out.
                        let found = currency_map
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .lookup(&name, std::time::Instant::now());
                        if let (Some(price), false) = (found.value, found.request) {
                            hover.show_exchange(&name, &Ok(price), None, stack, &snap.table, cfg.divine_threshold, fresh);
                        } else if let Some(why) = found.error {
                            hover.show_exchange(&name, &Err(why), None, stack, &snap.table, cfg.divine_threshold, fresh);
                        } else if found.request {
                            budget.lock().unwrap_or_else(|e| e.into_inner()).note_user(std::time::Instant::now());
                            if appraise_req_tx
                                .send_user(AppraiseReq::Currency { name: name.clone(), hover_stack: Some(stack) })
                                .is_err()
                            {
                                currency_map.lock().unwrap_or_else(|e| e.into_inner()).unsent(&name);
                                hover.show_notice("trade search is not running: restart the overlay");
                            }
                        } else {
                            // A row's request for the same name is already
                            // queued; its answer is shown when it lands.
                            awaiting_exchange = Some((name, stack));
                        }
                    }
                }
                // The copy failed, and the reason is the user's to fix:
                // wl-paste missing, a clipboard that does not answer,
                // modifier keys still held.
                Err(e) => {
                    eprintln!("price check: {e}");
                    hover.show_note(&e.to_string());
                }
            }
            // A fresh popup anchors at the cursor that triggered it.
            popup_at = anchor(&hover);
        }
        // Currency-exchange results. A hover check's answer replaces the
        // "checking exchange..." popup; by now that popup may have expired
        // or been walked away from, so the answer is anchored afresh at the
        // cursor instead of being set on a popup nobody draws.
        while let Ok(done) = exch_rx.try_recv() {
            let now = std::time::Instant::now();
            if !current_league.is(&done.league) {
                // Asked in a league the overlay has left. The name is freed
                // so the next look asks again, in the league it is in now.
                currency_map.lock().unwrap_or_else(|e| e.into_inner()).unsent(&done.name);
                continue;
            }
            currency_map.lock().unwrap_or_else(|e| e.into_inner()).store(done.name.clone(), done.outcome.clone(), now);
            let waited_for = awaiting_exchange.as_ref().is_some_and(|(n, _)| *n == done.name);
            match (done.hover_stack, waited_for) {
                // The user's own check of a stack: its card carries the
                // answer (the offers, the stack's worth, or the reason);
                // the cache learned it above, like a row's.
                (Some(_), _) => {}
                // A hover check was waiting on a row's request for the
                // same name: the answer goes on the popup, with how many
                // offers stood behind it when the body was read.
                (None, true) => {
                    let stack = awaiting_exchange.take().map(|(_, stack)| stack).unwrap_or(1);
                    let snap = svc.snapshot();
                    let fresh = hover::Freshness { table_stale: snap.stale, uniques_stale: snap.uniques_stale };
                    let outcome = done.outcome.map_err(|(why, _)| why);
                    hover.show_exchange(&done.name, &outcome, done.offers, stack, &snap.table, cfg.divine_threshold, fresh);
                    popup_at = anchor(&hover);
                }
                // A reward row's lookup failed: the row shows "..." and
                // retries by itself, and the reason is said once.
                (None, false) => {
                    if let Err((why, _)) = &done.outcome {
                        eprintln!("exchange price for {}: {why}", done.name);
                        if last_row_error_note.elapsed() >= ROW_ERROR_NOTE_GAP {
                            last_row_error_note = now;
                            notices.push_back(format!("{} not priced yet: {why}", done.name));
                        }
                    }
                }
            }
        }
        while let Ok(done) = appraise_rx.try_recv() {
            match done {
                AppraiseDone::Note(text) => {
                    eprintln!("{text}");
                    if last_row_error_note.elapsed() >= ROW_ERROR_NOTE_GAP {
                        last_row_error_note = std::time::Instant::now();
                        notices.push_back(text);
                    }
                }
                AppraiseDone::Attributed { title, outcome } => {
                    let Some((panel, _, _)) = apanel.as_mut() else { continue };
                    if panel.header.name != title {
                        continue;
                    }
                    panel.searching = false;
                    match outcome {
                        Ok(rows) => {
                            panel.status = format!("{} mods priced", rows.len());
                            panel.attribution = rows;
                        }
                        Err(why) => panel.status = why,
                    }
                    sync_input_region(overlay, &renderer, &apanel, &mkt_panel, &craft_panel)?;
                }
                // Seed: open the interactive panel where the "searching
                // trade..." popup was anchored, with every row and no
                // listings yet.
                AppraiseDone::Seed { title, query, labels, facts, unsearchable, extra } => {
                    let panel =
                        build_panel(title, &query, &labels, facts, unsearchable, &extra, reference.get());
                    let origin = popup_at.map(|(o, _)| o).unwrap_or(cursor_pos);
                    let lay = khaloni_poe2::evaluate_ui::layout(&panel, &|s| {
                        renderer.evaluate_label_width(s)
                    });
                    let pos = khaloni_poe2::popup_pos::place(origin, lay.size, game_rect);
                    hover.current = None;
                    popup_at = None;
                    // Fresh check: forget any earlier drag so the panel opens
                    // at its placed position, never where it was last dragged.
                    close_eval(&mut apanel, &mut editing, &mut edit_buf, &mut panel_drag, &mut searched);
                    let mut panel = panel;
                    panel.screen_right = Some(overlay.output_pos().0 + overlay.size().0 as i32 - pos.0);
                    apanel = Some((panel, *query, pos));
                    overlay.set_keyboard(false)?;
                    sync_input_region(overlay, &renderer, &apanel, &mkt_panel, &craft_panel)?;
                }
                // Result: listings for the open panel, from the auto search
                // or a Search press. A panel closed (or replaced) while the
                // search ran drops the result.
                AppraiseDone::Result { title, outcome, strictness } => {
                    let Some((panel, _, _)) = apanel.as_mut() else { continue };
                    if panel.header.name != title {
                        continue;
                    }
                    check_in_flight = None;
                    panel.searching = false;
                    let snap = svc.snapshot();
                    // An answer from the league the overlay has left is
                    // not shown: its exalted figures would be put into
                    // divines at the new league's rate right here.
                    let outcome = outcome.and_then(|done| {
                        if current_league.is(&done.league) {
                            Ok(done)
                        } else {
                            Err(format!("the league changed to {}: search again", current_league.name()))
                        }
                    });
                    match outcome {
                        Ok(done) => {
                            show_search(panel, &done);
                            let mut status = if let Some(b) = &done.bulk {
                                format!("{} exchange offers", b.offers.len())
                            } else if done.blocks.shown == 0 {
                                match strictness {
                                    khaloni_poe2::evaluate_ui::Strictness::Quick => "no online matches".to_string(),
                                    khaloni_poe2::evaluate_ui::Strictness::Broad => {
                                        "Broad search, bounds -10%: no online matches".to_string()
                                    }
                                }
                            } else {
                                khaloni_poe2::evaluate_ui::result_status(done.blocks.shown, done.blocks.total, strictness)
                            };
                            if snap.stale {
                                status.push_str(" - exchange rates are old");
                            }
                            if let Some(age) = done.reused_age {
                                status.push_str(&format!(" (same search {}s ago)", age.as_secs()));
                            }
                            let previous = searched.as_ref().map(|s| s.league.as_str());
                            panel.status = khaloni_poe2::league::searched_status(&status, previous, &done.league);
                            panel.search_id = done.search_id;
                            searched = Some(Searched {
                                query: done.searched,
                                league: done.league,
                                cheapest: done.blocks.cheapest,
                                unit: done.blocks.unit,
                                unit_per_exalted: done.blocks.unit_per_exalted,
                            });
                        }
                        Err(why) => {
                            clear_search(panel);
                            panel.status = why;
                        }
                    }
                    // The blocks grow the card downwards, so the buttons
                    // under them move: without this the region still
                    // describes the pre-search panel and the Search button
                    // stops answering after the first search.
                    sync_input_region(overlay, &renderer, &apanel, &mkt_panel, &craft_panel)?;
                }
            }
        }
        while let Ok(done) = craft_done_rx.try_recv() {
            let snap = svc.snapshot();
            match done {
                CraftDone::Busy { generation, text } => {
                    if let Some((p, _)) = craft_panel.as_mut().filter(|_| generation.is_none_or(|g| g == craft_generation)) {
                        p.busy = Some(text);
                    }
                }
                CraftDone::Opened { generation, outcome } => {
                    if generation != craft_generation {
                        continue;
                    }
                    match outcome {
                        Ok((state, picker, observed)) => {
                            let rates = khaloni_poe2::craft_flow::rates(&snap.table, cfg.divine_threshold);
                            let opened = khaloni_poe2::craft_flow::Opened { state: state.clone(), picker };
                            let mut panel = khaloni_poe2::craft_flow::open_panel(&opened, rates, craft_prices_line(&snap), observed);
                            // A new item keeps the last scan's list and the
                            // panel where the user left it.
                            let (flips, pos) = match craft_panel.take() {
                                Some((old, pos)) => (old.flips, pos),
                                None => (None, craft_pos(game_rect)),
                            };
                            panel.flips = flips;
                            craft_panel = Some((panel, pos));
                            craft_item = Some(state);
                            hover.current = None;
                            popup_at = None;
                        }
                        Err(why) => {
                            eprintln!("craft planner: {why}");
                            hover.show_note(&why);
                            popup_at = anchor(&hover);
                        }
                    }
                }
                CraftDone::Planned { generation, outcome, observed, note } => {
                    let Some((p, _)) = craft_panel.as_mut().filter(|_| generation == craft_generation) else { continue };
                    p.busy = None;
                    if !observed.is_empty() {
                        p.observed = observed;
                    }
                    match outcome {
                        Ok(view) => {
                            p.model = view.model.clone();
                            p.set_plan(view);
                            p.note = note;
                        }
                        Err(why) => {
                            p.plan = khaloni_poe2::craft_ui::PlanState::None;
                            p.note = Some(format!("not planned: {why}"));
                        }
                    }
                }
                CraftDone::Calibrated { generation, outcome, observed } => {
                    let Some((p, _)) = craft_panel.as_mut().filter(|_| generation == craft_generation) else { continue };
                    p.busy = None;
                    if !observed.is_empty() {
                        p.observed = observed;
                    }
                    match outcome {
                        Ok(text) => {
                            p.note = Some(text);
                            // The plan shown is costed again on the new
                            // sample; its buy line is reused, not searched.
                            if matches!(p.plan, khaloni_poe2::craft_ui::PlanState::Ready(_)) {
                                if let Some(state) = &craft_item {
                                    craft_run(&khaloni_poe2::craft_ui::Action::Plan, p, state, craft_generation, &craft_req_tx);
                                }
                            }
                        }
                        Err(why) => p.note = Some(format!("calibration not run: {why}")),
                    }
                }
                CraftDone::ScanStated { name, outcome } => {
                    let Some((p, _)) = craft_panel.as_mut() else { continue };
                    p.busy = None;
                    match outcome {
                        Ok((statement, refused)) => p.ask(khaloni_poe2::craft_ui::Prompt {
                            ask: khaloni_poe2::craft_ui::Ask::Scan(name),
                            statement,
                            refused,
                        }),
                        Err(why) => {
                            p.view = khaloni_poe2::craft_ui::View::Flips;
                            p.note = Some(why);
                        }
                    }
                }
                CraftDone::Scanned { outcome } => {
                    let Some((p, _)) = craft_panel.as_mut() else { continue };
                    p.busy = None;
                    match outcome {
                        Ok((list, notes)) => {
                            p.view = khaloni_poe2::craft_ui::View::Flips;
                            p.set_flips(list);
                            p.note = (!notes.is_empty()).then(|| notes.join("; "));
                        }
                        Err(why) => p.note = Some(format!("scan not run: {why}")),
                    }
                }
            }
            sync_input_region(overlay, &renderer, &apanel, &mkt_panel, &craft_panel)?;
        }
        // Panel clicks: geometry from the same layout the renderer drew.
        // The Evaluate panel gets first claim on each click (preserving its
        // drag-grab semantics); clicks outside it spill into
        // `leftover_clicks` for the craft and market panels below.
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
                let action = khaloni_poe2::evaluate_ui::hit(panel, &lay, local.0, local.1);
                // Any click takes focus away from a box being typed into,
                // and what was typed counts: it is committed exactly as
                // Enter would, before the click acts. A Search pressed with
                // a box still open used to search the old number while the
                // box showed the new one.
                if let Some((row_i, field)) = editing.take() {
                    khaloni_poe2::evaluate_ui::commit_edit(panel, query, row_i, field, &edit_buf);
                    edit_buf.clear();
                }
                match action {
                    // Row indices, not filter indices: evaluate_ui's actions
                    // address `panel.rows`, and the filter behind a row is
                    // whatever that row's target names.
                    Some(khaloni_poe2::evaluate_ui::Action::ToggleRow(i)) => {
                        khaloni_poe2::evaluate_ui::toggle_row(panel, query, i);
                    }
                    // Off, the search runs on the exact base instead of
                    // the category (an upgrade search has no base and runs
                    // across every category).
                    Some(khaloni_poe2::evaluate_ui::Action::ToggleBase) => {
                        query.category_enabled = !query.category_enabled;
                        if let Some(b) = panel.header.base.as_mut() {
                            b.enabled = query.category_enabled;
                        }
                    }
                    // Clicking a value box focuses it for keyboard entry.
                    Some(khaloni_poe2::evaluate_ui::Action::Edit(i, field)) => {
                        editing = Some((i, field));
                    }
                    Some(khaloni_poe2::evaluate_ui::Action::SetStrictness(s)) => {
                        panel.strictness = s;
                    }
                    // Expanding the collapsed rows changes the panel's height,
                    // so the input region has to follow it.
                    Some(khaloni_poe2::evaluate_ui::Action::ToggleHidden) => {
                        panel.show_hidden = !panel.show_hidden;
                    }
                    // A click on a listing row opens its card, as pointing
                    // at it does.
                    Some(khaloni_poe2::evaluate_ui::Action::HoverRow(i)) => {
                        panel.hover = Some(i);
                    }
                    // One search at a time: a press while one runs is not
                    // sent (the button is drawn switched off meanwhile).
                    Some(
                        khaloni_poe2::evaluate_ui::Action::Search
                        | khaloni_poe2::evaluate_ui::Action::PriceFixedFilter
                        | khaloni_poe2::evaluate_ui::Action::Attribute,
                    ) if panel.searching => {}
                    Some(
                        action @ (khaloni_poe2::evaluate_ui::Action::Search
                        | khaloni_poe2::evaluate_ui::Action::PriceFixedFilter),
                    ) => {
                        // The price-fixed button keeps the search but
                        // admits only listings priced in exalted or divine;
                        // the option stays on the query, so the next
                        // Search and "Open site" carry it too.
                        if action == khaloni_poe2::evaluate_ui::Action::PriceFixedFilter {
                            *query = khaloni_poe2::appraise::price_fixed_query(query);
                        }
                        // The ticked extra rows join EE2's query here, and
                        // Broad relaxes every kept bound, theirs included, by
                        // 10% before the search runs; Quick sends the user's
                        // own numbers verbatim, since their toggles ARE the
                        // intent.
                        let full = khaloni_poe2::evaluate_ui::search_query(panel, query);
                        let q = match panel.strictness {
                            khaloni_poe2::evaluate_ui::Strictness::Broad => {
                                khaloni_poe2_core::trade::relax_query(&full, 0.10)
                            }
                            khaloni_poe2::evaluate_ui::Strictness::Quick => full,
                        };
                        budget.lock().unwrap_or_else(|e| e.into_inner()).note_user(std::time::Instant::now());
                        let sent = appraise_req_tx.send_user(AppraiseReq::Exact {
                            title: panel.header.name.clone(),
                            query: q,
                            strictness: panel.strictness,
                        });
                        match sent {
                            Ok(()) => {
                                panel.status = "searching...".into();
                                panel.searching = true;
                            }
                            Err(_) => panel.status = "trade search is not running: restart the overlay".into(),
                        }
                    }
                    // What each mod is worth: on request, and only while
                    // the budget has the room the button was drawn with.
                    Some(khaloni_poe2::evaluate_ui::Action::Attribute) => {
                        let baseline = searched.as_ref().and_then(|s| s.cheapest.clone().map(|c| (c, s)));
                        match baseline {
                            _ if !panel.attribute_enabled => {
                                panel.status = format!(
                                    "what each mod is worth needs {} free search slots",
                                    khaloni_poe2::appraise::ATTRIBUTE_MIN_FREE
                                );
                            }
                            None => panel.status = "run a search with listings first".into(),
                            Some((cheapest, s)) => {
                                // Every "without" search is the one on the
                                // card, ticked lines included, less one mod.
                                let mods =
                                    khaloni_poe2::appraise::strongest_mods(panel, &s.query, query.filters.len());
                                budget.lock().unwrap_or_else(|e| e.into_inner()).note_user(std::time::Instant::now());
                                let sent = appraise_req_tx.send_user(AppraiseReq::Attribute {
                                    title: panel.header.name.clone(),
                                    mods,
                                    baseline: cheapest,
                                    unit: s.unit.clone(),
                                    unit_per_exalted: s.unit_per_exalted,
                                });
                                match sent {
                                    Ok(()) => {
                                        panel.status = "pricing each mod...".into();
                                        panel.searching = true;
                                    }
                                    Err(_) => panel.status = "trade search is not running: restart the overlay".into(),
                                }
                            }
                        }
                    }
                    Some(khaloni_poe2::evaluate_ui::Action::OpenSite) => {
                        // Feedback in the status line, since opening the browser
                        // gives no in-overlay cue on its own.
                        match &searched {
                            // A stack's card came from the exchange: there
                            // is no search behind it to open.
                            Some(_) if panel.search_id.is_none() => panel.status = "no search to open".into(),
                            Some(done) => {
                                // The link carries the query itself, the
                                // way Exiled Exchange 2's does. A search
                                // made without a login now returns an id
                                // that is only the compressed query, and
                                // the site's /search/<league>/<id> address
                                // answers "Resource not found" for it
                                // (checked live 2026-09-19). It is the
                                // query the listings came from - Broad's
                                // relaxed bounds included - in the league
                                // they were searched in.
                                let url = site_search_url(&done.league, &done.query);
                                panel.status = match open_url(&url) {
                                    Ok(()) => "opened in browser".into(),
                                    Err(e) => e,
                                };
                            }
                            None => panel.status = "run a search first".into(),
                        }
                    }
                    Some(khaloni_poe2::evaluate_ui::Action::Close) => {
                        close_eval(&mut apanel, &mut editing, &mut edit_buf, &mut panel_drag, &mut searched);
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
                    }
                }
                // Every click can change who wants the keyboard (a box
                // opened or committed, the panel closed) and how tall the
                // card is; both follow from the state left behind.
                overlay.set_keyboard(editing.is_some())?;
                if panel_drag.is_some() {
                    overlay.set_interactive(Some((0, 0, sw, sh)))?;
                } else {
                    sync_input_region(overlay, &renderer, &apanel, &mkt_panel, &craft_panel)?;
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
                    sync_input_region(overlay, &renderer, &apanel, &mkt_panel, &craft_panel)?;
                }
            }
            // The row under the pointer gets its card beside the panel.
            // The cursor comes from the window feed (global), which keeps
            // reporting outside the input region, so leaving the table
            // takes the card away; pointing at the card itself keeps it,
            // so it can be read. A changed row changes the region: the
            // card is part of it.
            if let Some((p, _, pos)) = apanel.as_mut() {
                p.screen_right = Some(out_pos.0 + sw as i32 - pos.0);
                let lay = khaloni_poe2::evaluate_ui::layout(p, &|s| renderer.evaluate_label_width(s));
                let local = (cursor_pos.0 - pos.0, cursor_pos.1 - pos.1);
                let over_card = lay.card.as_ref().is_some_and(|c| {
                    local.0 >= c.rect.x
                        && local.0 < c.rect.x + c.rect.w as i32
                        && local.1 >= c.rect.y
                        && local.1 < c.rect.y + c.rect.h as i32
                });
                let hover = match khaloni_poe2::evaluate_ui::hover_hit(&lay, local.0, local.1) {
                    Some(i) => Some(i),
                    None if over_card => p.hover,
                    None => None,
                };
                if hover != p.hover {
                    p.hover = hover;
                    sync_input_region(overlay, &renderer, &apanel, &mkt_panel, &craft_panel)?;
                }
            }
        } else {
            leftover_clicks = overlay.take_clicks();
        }
        // Craft and market panel clicks: whatever the Evaluate panel did
        // not claim, in priority order craft then market.
        for (cx, cy) in leftover_clicks {
            if let Some((p, pos)) = craft_panel.as_mut() {
                let lay = khaloni_poe2::craft_ui::layout(p, &|s| renderer.evaluate_label_width(s));
                let local = (cx - (pos.0 - out_pos.0), cy - (pos.1 - out_pos.1));
                if local.0 >= 0 && local.0 < lay.w && local.1 >= 0 && local.1 < lay.h {
                    match khaloni_poe2::craft_ui::hit(&lay, local.0, local.1) {
                        Some(khaloni_poe2::craft_ui::Action::Close) => craft_panel = None,
                        Some(action) => match &craft_item {
                            Some(state) => craft_run(&action, p, state, craft_generation, &craft_req_tx),
                            // A panel opened for a scan has no item: the
                            // picker's actions have nothing to act on.
                            None => craft_run_without_item(&action, p, &craft_req_tx),
                        },
                        None => {}
                    }
                    sync_input_region(overlay, &renderer, &apanel, &mkt_panel, &craft_panel)?;
                    continue;
                }
            }
            if let Some((p, pos)) = mkt_panel.as_mut() {
                let lay = khaloni_poe2::market_ui::layout(p, &|s| renderer.evaluate_label_width(s));
                let local = (cx - (pos.0 - out_pos.0), cy - (pos.1 - out_pos.1));
                if local.0 >= 0 && local.0 < lay.w && local.1 >= 0 && local.1 < lay.h {
                    match khaloni_poe2::market_ui::hit(&lay, local.0, local.1) {
                        Some(khaloni_poe2::market_ui::Action::Close) => mkt_panel = None,
                        Some(action) => {
                            khaloni_poe2::market_ui::apply(p, &action);
                        }
                        None => {}
                    }
                    // Every action changes the panel's size.
                    sync_input_region(overlay, &renderer, &apanel, &mkt_panel, &craft_panel)?;
                    continue;
                }
            }
        }
        // Typing into a focused value box (EE2-style numeric entry): digits,
        // one '.', a leading '-', Backspace; Enter commits the parsed
        // number to both the query filter and the panel row, Escape cancels.
        if editing.is_some() {
            for key in overlay.take_keys() {
                use khaloni_poe2::evaluate_ui::EditKey;
                let Some((row_i, field)) = editing else { break };
                let Some((panel, query, _)) = apanel.as_mut() else {
                    editing = None;
                    edit_buf.clear();
                    break;
                };
                match key {
                    khaloni_poe2::platform::Key::Digit(c) => {
                        khaloni_poe2::evaluate_ui::edit_key(&mut edit_buf, EditKey::Digit(c));
                    }
                    khaloni_poe2::platform::Key::Dot => {
                        khaloni_poe2::evaluate_ui::edit_key(&mut edit_buf, EditKey::Dot);
                    }
                    khaloni_poe2::platform::Key::Char('-') => {
                        khaloni_poe2::evaluate_ui::edit_key(&mut edit_buf, EditKey::Minus);
                    }
                    khaloni_poe2::platform::Key::Backspace => {
                        khaloni_poe2::evaluate_ui::edit_key(&mut edit_buf, EditKey::Backspace);
                    }
                    khaloni_poe2::platform::Key::Enter => {
                        khaloni_poe2::evaluate_ui::commit_edit(panel, query, row_i, field, &edit_buf);
                        editing = None;
                        edit_buf.clear();
                    }
                    khaloni_poe2::platform::Key::Escape => {
                        editing = None;
                        edit_buf.clear();
                    }
                    // Other text has no meaning in a numeric value box.
                    khaloni_poe2::platform::Key::Char(_) => {}
                }
            }
            if editing.is_none() {
                overlay.set_keyboard(false)?;
            }
        }
        // The panel is a deliberate, sticky action: it stays put until the
        // user closes it (X or a new price check) so they can alt-tab, click a
        // value box (which itself steals focus from the game to type), and edit
        // without it vanishing. Only a fully-gone game tears it down, since
        // then the overlay hides and its input region must not linger.
        if apanel.is_some() && !game_present {
            close_eval(&mut apanel, &mut editing, &mut edit_buf, &mut panel_drag, &mut searched);
            overlay.set_keyboard(false)?;
            sync_input_region(overlay, &renderer, &apanel, &mkt_panel, &craft_panel)?;
        }

        hover.tick();
        // The next thing the user has to be told, once nothing else is up
        // and there is a game on screen to show it over.
        if hover.current.is_none() && game_present {
            if let Some(text) = notices.pop_front() {
                hover.show_notice(&text);
                popup_at = anchor(&hover);
            }
        }
        match (&hover.current, popup_at) {
            (Some(_), Some((origin, rect))) => {
                if !hover.sticky && khaloni_poe2::popup_pos::should_dismiss(origin, cursor_pos, rect) {
                    hover.current = None;
                    popup_at = None;
                }
            }
            (None, Some(_)) => popup_at = None,
            _ => {}
        }

        let policy = khaloni_poe2::scanpolicy::decide(khaloni_poe2::scanpolicy::Inputs {
            game_present,
            game_visible,
            focus,
            pause_when_hidden: cfg.pause_when_hidden,
            user_paused,
        });
        let paused = policy.paused;
        pipeline_paused.store(paused, std::sync::atomic::Ordering::Relaxed);
        // The capture stops converting frames for as long as nobody reads
        // them (it keeps dequeuing, so the stream stays alive).
        capture_paused.store(paused, std::sync::atomic::Ordering::Relaxed);
        // A paused rumour worker sends nothing, so badges from before the
        // pause would come back stale on resume; drop them now.
        if paused {
            latest_rumours.clear();
        }

        while let Ok((priced_in, msg)) = rows_rx.try_recv() {
            if priced_in.is_some_and(|l| !current_league.is(&l)) {
                continue;
            }
            if dbg {
                match &msg {
                    khaloni_poe2::stabilize::ScanResult::GateEmpty => {
                        eprintln!("DBG rows_rx: gate-empty");
                    }
                    khaloni_poe2::stabilize::ScanResult::NoBands => {
                        eprintln!("DBG rows_rx: no-bands");
                    }
                    khaloni_poe2::stabilize::ScanResult::Rows(scan) => {
                        eprintln!(
                            "DBG rows_rx: {} rows, stale={}, partial={}, at={:?}",
                            scan.rows.len(),
                            scan.stale,
                            scan.partial,
                            scan.at
                        );
                    }
                    khaloni_poe2::stabilize::ScanResult::Scrolled { dy, to, span } => {
                        eprintln!("DBG rows_rx: scrolled {dy} to {to:?} in {span:?}");
                    }
                    khaloni_poe2::stabilize::ScanResult::TrackingLost { .. } => {
                        eprintln!("DBG rows_rx: tracking-lost");
                    }
                }
            }
            let before = dbg.then(|| stabilizer.rows().iter().map(|r| format!("{}@y{}", r.item_key, r.y_top)).collect::<Vec<_>>());
            stabilizer.apply(msg);
            if let Some(before) = before {
                let after: Vec<String> = stabilizer.rows().iter().map(|r| format!("{}@y{}", r.item_key, r.y_top)).collect();
                if before != after {
                    eprintln!("TRACE stab: [{}] -> [{}]", before.join(", "), after.join(", "));
                }
            }
        }
        if dbg {
            static TICK: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let t = TICK.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if t.is_multiple_of(10) {
                eprintln!(
                    "DBG t={t} paused={paused} present={game_present} focused={game_focused} visible={game_visible} region={:?} rows={} surface={:?} game_pos={game_pos:?}",
                    scan_geom.lock().unwrap_or_else(|e| e.into_inner()).1,
                    stabilizer.rows().len(),
                    overlay.size()
                );
            }
        }

        // Rows obey focus and visibility (see scanpolicy); the popup only
        // needs the game on screen. An explicit F7 must stay visible while
        // pricing is paused, otherwise the hotkey reads as a dead key (live
        // finding, 2026-07-23).
        let on_screen = policy.on_screen;
        let show_rows = policy.show_rows;
        // The Evaluate panel renders whenever it is open and the game is
        // present, even while unfocused: editing a value box steals keyboard
        // focus from the game, and the panel must not blink out mid-edit.
        let show = show_rows
            || (on_screen && hover.current.is_some())
            || (game_present && apanel.is_some());
        // The pixmap is in device pixels and the renderer scales into it,
        // so text is rasterized at the size it has on screen (on a 150%
        // output a logical-size buffer was stretched by the compositor and
        // every glyph came out soft). Layout, placement and hit-testing
        // stay in logical pixels throughout.
        let size = overlay.device_size();
        if size.0 > 0 && size.1 > 0 {
            let mut resized = renderer.set_scale(overlay.scale() as f32);
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
                let (frame_dims, region_now) = *scan_geom.lock().unwrap_or_else(|e| e.into_inner());
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
                if let Some((p, _)) = mkt_panel.as_mut() {
                    let snap = svc.snapshot();
                    let changed = mkt_feed.refresh(
                        p,
                        &snap.league,
                        snap.loading,
                        (snap.stale, snap.uniques_stale),
                        &svc.market(),
                        cfg.market_floors(),
                        cfg.divine_threshold,
                        khaloni_poe2::prices::unix_now(),
                    ) | p.set_runs(runs_hub.shown());
                    // New data resizes the panel; its input region follows.
                    if changed {
                        sync_input_region(overlay, &renderer, &apanel, &mkt_panel, &craft_panel)?;
                    }
                }
                let mkt_state = mkt_panel
                    .as_ref()
                    .map(|(p, pos)| (p.clone(), (pos.0 - out_pos.0, pos.1 - out_pos.1)));
                let craft_state = craft_panel
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
                    mkt_state,
                    craft_state,
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
                    Some((placed, rate, stale, popup, panel, rumours, edit_state, mkt_state, craft_state)) => {
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
                        if let Some((p, anchor)) = mkt_state {
                            let lay = khaloni_poe2::market_ui::layout(p, &|s| {
                                renderer.evaluate_label_width(s)
                            });
                            renderer.draw_market(pm, p, &lay, *anchor);
                        }
                        if let Some((p, anchor)) = craft_state {
                            let lay = khaloni_poe2::craft_ui::layout(p, &|s| renderer.evaluate_label_width(s));
                            renderer.draw_craft(pm, p, &lay, *anchor);
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

#[cfg(test)]
mod main_tests {
    #[test]
    fn the_wrapper_finds_the_game_command() {
        let v = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<String>>();
        // Steam's %command% expansion, with and without a -- separator.
        let args = v(&["--", "/path/game", "-somearg"]);
        assert_eq!(super::game_command(&args).unwrap(), &args[1..]);
        let args = v(&["/path/game", "-somearg"]);
        assert_eq!(super::game_command(&args).unwrap(), &args[..]);
        // Nothing to run is an error, not a silent no-op.
        assert_eq!(super::game_command(&[]), None);
        assert_eq!(super::game_command(&v(&["--"])), None);
    }

    use super::{rarity_label, requires_level, urlencode};

    #[test]
    fn a_runtime_failure_is_not_reported_as_a_failed_start() {
        use khaloni_poe2::platform::OverlayError;
        let heading = super::fatal_heading;
        assert!(heading(false, None).contains("could not start"));
        assert!(heading(false, Some(&OverlayError::Startup("no layer-shell".into()))).contains("could not start"));
        assert!(heading(true, None).contains("stopped working"));
        // A lost compositor connection is a runtime event whenever it is seen.
        let lost = OverlayError::Connection("broken pipe".into());
        assert!(heading(true, Some(&lost)).contains("display connection was lost"));
        assert!(!heading(true, Some(&lost)).contains("could not start"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_openmp_restart_happens_once_and_respects_the_user() {
        use std::ffi::OsStr;
        assert!(super::needs_openmp_restart(None), "unset: restart with the limit");
        // Set by the restart itself (no loop) or by the user (their call).
        assert!(!super::needs_openmp_restart(Some(OsStr::new("1"))));
        assert!(!super::needs_openmp_restart(Some(OsStr::new("8"))));
    }

    #[test]
    fn the_overlay_follows_the_game_to_another_output() {
        let off = super::off_output;
        // A 2560x1440 output at x=2560; the game's centre on it, then on
        // the output to its left.
        assert!(!off((3840, 720), (2560, 0), (2560, 1440)));
        assert!(off((1280, 720), (2560, 0), (2560, 1440)));
        assert!(off((3840, 1500), (2560, 0), (2560, 1440)));
        // A surface not configured yet says nothing.
        assert!(!off((1280, 720), (2560, 0), (0, 0)));
    }

    #[test]
    fn closing_the_panel_clears_everything_that_belongs_to_it() {
        use khaloni_poe2::evaluate_ui::Field;
        let panel = super::build_panel("Horror Bane".into(), &Default::default(), &[], None, Vec::new(), &[], None);
        let mut apanel = Some((panel, khaloni_poe2_core::trade::Query::default(), (10, 10)));
        let mut editing = Some((2usize, Field::Min));
        let mut buf = String::from("41");
        let mut drag = Some(((1, 1), (2, 2)));
        let mut searched = Some(super::Searched {
            query: Default::default(),
            league: "Standard".into(),
            cheapest: None,
            unit: "ex".into(),
            unit_per_exalted: 1.0,
        });
        assert!(super::close_eval(&mut apanel, &mut editing, &mut buf, &mut drag, &mut searched));
        assert!(apanel.is_none() && editing.is_none() && buf.is_empty() && drag.is_none() && searched.is_none());
        // Nothing open: still leaves no edit state behind.
        editing = Some((0, Field::Max));
        assert!(!super::close_eval(&mut apanel, &mut editing, &mut buf, &mut drag, &mut searched));
        assert!(editing.is_none());
    }

    #[test]
    fn a_filter_without_a_minimum_gets_an_empty_box() {
        use khaloni_poe2_core::trade::{FilterLabel, Query, StatFilter};
        let mut open_ended = StatFilter::at_least("explicit.stat_1", 0.0, false);
        open_ended.value.min = None;
        open_ended.value.max = Some(12.0);
        let query = Query { filters: vec![StatFilter::at_least("explicit.stat_0", 23.0, false), open_ended], ..Default::default() };
        let label = |text: &str| FilterLabel {
            text: text.into(),
            tier: None,
            min: 0,
            rolled: None,
            tag: "explicit",
            hidden: false,
            lines: Vec::new(),
        };
        let labels = [label("23 to Accuracy Rating"), label("12% reduced Attribute Requirements")];
        let panel =
            super::build_panel("X".into(), &query, &labels, None, vec!["Some unsearchable line".into()], &[], None);
        assert_eq!(panel.rows[0].min, Some(23.0));
        assert_eq!((panel.rows[1].min, panel.rows[1].max), (None, Some(12.0)), "no bound is not a bound of 0");
        assert_eq!(panel.rows[2].min, None);
        assert!(panel.searching, "a seeded card has its search running");
    }

    /// "Open site" opens the query the search sent, built by the same
    /// call Search makes: a ticked extra row is in the link with the bound
    /// in its box, an unticked one is not, and EE2's rows are as they were.
    #[test]
    fn open_site_carries_exactly_the_searched_filters() {
        use khaloni_poe2::evaluate_ui::{commit_edit, search_query, toggle_row, Field, Target};
        use khaloni_poe2_core::ee2::request::ExtraRow;
        use khaloni_poe2_core::trade::{FilterLabel, FilterValue, Query, StatFilter};
        let query = Query { filters: vec![StatFilter::at_least("explicit.stat_0", 23.0, false)], ..Default::default() };
        let labels = [FilterLabel {
            text: "23 to Accuracy Rating".into(),
            tier: None,
            min: 23,
            rolled: Some(25.0),
            tag: "explicit",
            hidden: false,
            lines: Vec::new(),
        }];
        let line = |text: &str, id: &str| ExtraRow {
            text: text.into(),
            tag: "explicit",
            rolled: Some(142.0),
            lines: vec![text.into()],
            into: vec!["Total Life".into()],
            group: "explicit",
            ids: vec![id.into()],
            option: None,
            value: FilterValue { min: Some(127.0), max: None },
            lookup: Vec::new(),
            stat_keys: Vec::new(),
        };
        let extra =
            [line("+142 to maximum Life", "explicit.stat_3299347043"), line("+30 to Strength", "explicit.stat_4080418644")];
        let mut panel = super::build_panel("X".into(), &query, &labels, None, Vec::new(), &extra, None);
        let rows: Vec<usize> =
            (0..panel.rows.len()).filter(|i| matches!(panel.rows[*i].target, Some(Target::Extra(_)))).collect();
        assert_eq!(rows.len(), 2, "both lines are rows");
        let mut q = query.clone();
        assert_eq!(search_query(&panel, &q), query, "nothing ticked: the search is EE2's");
        toggle_row(&mut panel, &mut q, rows[0]);
        commit_edit(&mut panel, &mut q, rows[0], Field::Min, "130");
        let sent = search_query(&panel, &q);
        let url = super::site_search_url("Standard", &sent);
        let body = sent.to_body().to_string();
        assert!(url.ends_with(&super::urlencode(&body)), "the link carries the body that was searched");
        assert!(body.contains(r#"{"disabled":false,"id":"explicit.stat_3299347043","value":{"min":130}}"#), "{body}");
        assert!(!body.contains("explicit.stat_4080418644"), "an unticked line is not searched");
        assert!(body.contains("explicit.stat_0"), "EE2's row is still there");
    }

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
