use std::sync::mpsc;

use poe2_lens::{
    capture,
    config::{Config, Rect},
    coord::CoordMap,
    ocr, pricing, prices,
};
use poe2_lens_core::ninja::NinjaClient;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str).unwrap_or("") {
        "--calibrate" => calibrate(),
        "--headless" => headless(),
        _ => overlay_mode(),
    }
}

/// slurp prints the dragged region in global logical coordinates.
fn calibrate() -> anyhow::Result<()> {
    let out = std::process::Command::new("slurp")
        .args(["-f", "%x %y %w %h"])
        .output()?;
    anyhow::ensure!(out.status.success(), "slurp cancelled or missing");
    let s = String::from_utf8_lossy(&out.stdout);
    let v: Vec<i32> = s.split_whitespace().filter_map(|t| t.parse().ok()).collect();
    anyhow::ensure!(v.len() == 4, "unexpected slurp output: {s}");
    let mut cfg = Config::load()?;
    cfg.calibration = Some(Rect {
        x: v[0],
        y: v[1],
        w: v[2] as u32,
        h: v[3] as u32,
    });
    cfg.save()?;
    println!(
        "calibration saved: {:?} -> {}",
        cfg.calibration.unwrap(),
        Config::path().display()
    );
    Ok(())
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

fn headless() -> anyhow::Result<()> {
    let mut cfg = Config::load()?;
    let cal = cfg
        .calibration
        .ok_or_else(|| anyhow::anyhow!("run --calibrate first"))?;

    eprintln!("fetching prices for {}...", cfg.league);
    let cache = directories::ProjectDirs::from("", "", "poe2-lens")
        .unwrap()
        .cache_dir()
        .to_path_buf();
    let svc = prices::PriceService::start(NinjaClient::new(cache), cfg.league.clone())?;
    eprintln!("price table ready ({} names)", svc.snapshot().table.len());

    let rt = tokio::runtime::Runtime::new()?;
    let start = rt.block_on(capture::portal_session(cfg.restore_token.as_deref()))?;
    if let Some(tok) = &start.new_token {
        cfg.restore_token = Some(tok.clone());
        cfg.save()?;
    }

    // Capture geometry is negotiated at 3840x2160 on the reference machine.
    let map = CoordMap::new(game_window_logical(), (3840, 2160), cal);
    let (ftx, frx) = mpsc::channel();
    let (_rtx, rrx) = mpsc::channel::<Rect>();
    let region = map.region_px();
    std::thread::spawn(move || {
        if let Err(e) = capture::consume(start, rrx, region, ftx) {
            eprintln!("capture thread died: {e}");
        }
    });

    eprintln!("headless pipeline running; open a Runeshape panel. Ctrl+C to quit.");
    for frame in frx {
        let pre = ocr::preprocess(&frame.gray);
        let lines = match ocr::run_tesseract(&cfg.tesseract_cmd, &pre) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("ocr error: {e}");
                continue;
            }
        };
        let snap = svc.snapshot();
        let (rows, total) = pricing::price_lines(&snap.table, &snap.vocab, &lines, &cfg);
        println!(
            "--- scan ({} lines, {} priced){}",
            lines.len(),
            rows.len(),
            if snap.stale { " [STALE PRICES]" } else { "" }
        );
        for r in &rows {
            let (lx, ly) = map.label_pos_logical(r.y_top);
            println!("  y={:>4} ({lx},{ly})  {:?}  {}", r.y_top, r.tier, r.label);
        }
        if !total.is_empty() {
            println!("  {total}");
        }
    }
    Ok(())
}

fn overlay_mode() -> anyhow::Result<()> {
    let mut cfg = Config::load()?;
    let cal = cfg
        .calibration
        .ok_or_else(|| anyhow::anyhow!("run --calibrate first"))?;

    let cache = directories::ProjectDirs::from("", "", "poe2-lens").unwrap().cache_dir().to_path_buf();
    let svc = prices::PriceService::start(NinjaClient::new(cache), cfg.league.clone())?;

    let kwin = poe2_lens::kwin::GeometryFeed::start()?;
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
            Ok(poe2_lens::kwin::KwinEvent::Geometry(g)) => {
                game = g;
                break;
            }
            Ok(_) => continue, // ignore Active/GameGone while waiting for the real geometry
            Err(_) => break,   // channel closed or timed out
        }
    }

    let rt = tokio::runtime::Runtime::new()?;
    let start = rt.block_on(capture::portal_session(cfg.restore_token.as_deref()))?;
    if let Some(tok) = &start.new_token {
        cfg.restore_token = Some(tok.clone());
        cfg.save()?;
    }
    let (hk_tx, hk_rx) = mpsc::channel();
    rt.spawn(async move {
        if let Err(e) = poe2_lens::hotkeys::listen(hk_tx).await {
            eprintln!("hotkeys unavailable: {e}");
        }
    });

    let map = CoordMap::new(game, (3840, 2160), cal);
    let (ftx, frx) = mpsc::channel();
    let (region_tx, region_rx) = mpsc::channel::<Rect>();
    let region = map.region_px();
    std::thread::spawn(move || {
        let _ = capture::consume(start, region_rx, region, ftx);
    });

    // OCR worker: frames in, priced rows out. `pipeline_paused` is toggled by the
    // main loop on focus loss / scan toggle, so we stop feeding tesseract without
    // touching the capture thread (its frame-hash short-circuit keeps it cheap).
    let pipeline_paused = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (rows_tx, rows_rx) = mpsc::channel();
    let svc_ocr = svc.clone();
    let ocr_cfg = cfg.clone();
    let paused_ocr = pipeline_paused.clone();
    std::thread::spawn(move || {
        let dbg = std::env::var("POE2LENS_DEBUG").is_ok();
        for frame in frx {
            let mean = mean_gray_brightness(&frame.gray);
            if dbg {
                eprintln!("DBG ocr-worker: frame {}x{} mean_brightness={mean}", frame.gray.width(), frame.gray.height());
            }
            if paused_ocr.load(std::sync::atomic::Ordering::Relaxed) {
                // Drop the frame cheaply; no OCR/pricing work while paused.
                continue;
            }
            if mean < u64::from(ocr_cfg.panel_min_brightness) {
                // Too dark to be the parchment panel (game world, not the
                // list): skip tesseract entirely and report gated-empty so
                // the overlay can drop stale rows instead of holding them.
                let _ = rows_tx.send(poe2_lens::stabilize::ScanResult::GateEmpty);
                continue;
            }
            let pre = ocr::preprocess(&frame.gray);
            let Ok(lines) = ocr::run_tesseract(&ocr_cfg.tesseract_cmd, &pre) else { continue };
            let snap = svc_ocr.snapshot();
            let out = pricing::price_lines(&snap.table, &snap.vocab, &lines, &ocr_cfg);
            let _ = rows_tx.send(poe2_lens::stabilize::ScanResult::Rows(out.0, snap.stale));
        }
    });

    let center = (game.x + game.w as i32 / 2, game.y + game.h as i32 / 2);
    let mut overlay = poe2_lens::overlay::Overlay::new(center)?;
    let font = std::fs::read(&cfg.font_path)?;
    let renderer = poe2_lens::render::Renderer::new(&font)?;

    let mut scanning = true;
    let mut hidden = false;
    let mut game_focused = true;
    let mut game_present = true;
    let mut stabilizer = poe2_lens::stabilize::Stabilizer::new();
    let mut game_pos = (game.x, game.y);
    let mut pixmap: Option<tiny_skia::Pixmap> = None;
    // Tracks the pause state from the previous tick so a resume (pause ->
    // running) can force a rescan even if the panel pixels never changed
    // while we were paused.
    let mut prev_paused = false;

    loop {
        overlay.pump()?;

        while let Ok(ev) = kwin.rx.try_recv() {
            match ev {
                poe2_lens::kwin::KwinEvent::Geometry(g) => {
                    game_pos = (g.x, g.y);
                    game_present = true;
                    let m = CoordMap::new(g, (3840, 2160), cal);
                    let _ = region_tx.send(m.region_px());
                }
                poe2_lens::kwin::KwinEvent::Active(is_game) => game_focused = is_game,
                poe2_lens::kwin::KwinEvent::GameGone => {
                    stabilizer.clear();
                    game_present = false;
                    overlay.hide()?;
                }
            }
        }
        while let Ok(hk) = hk_rx.try_recv() {
            match hk {
                poe2_lens::hotkeys::Hotkey::ScanToggle => {
                    scanning = !scanning;
                    if scanning {
                        // Scanning just turned back on: force a rescan even
                        // if the panel looks pixel-identical to before.
                        let _ = region_tx.send(map.region_px());
                    } else {
                        stabilizer.clear();
                    }
                }
                poe2_lens::hotkeys::Hotkey::Hide => hidden = !hidden,
            }
        }

        let paused = !scanning || !game_present || (!game_focused && cfg.pause_when_unfocused);
        if prev_paused && !paused {
            // Resuming from pause: the capture thread's hash gate may have
            // been sitting on a stale frame the whole time, so force one.
            let _ = region_tx.send(map.region_px());
        }
        prev_paused = paused;
        pipeline_paused.store(paused, std::sync::atomic::Ordering::Relaxed);

        while let Ok(msg) = rows_rx.try_recv() {
            match &msg {
                poe2_lens::stabilize::ScanResult::GateEmpty => {
                    eprintln!("DBG rows_rx: gate-empty");
                }
                poe2_lens::stabilize::ScanResult::Rows(rows, stale) => {
                    eprintln!("DBG rows_rx: {} rows, stale={stale}", rows.len());
                }
            }
            if scanning {
                stabilizer.apply(msg);
            }
        }
        if std::env::var("POE2LENS_DEBUG").is_ok() {
            static TICK: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let t = TICK.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if t.is_multiple_of(10) {
                eprintln!(
                    "DBG t={t} paused={paused} scanning={scanning} present={game_present} focused={game_focused} hidden={hidden} rows={} surface={:?} game_pos={game_pos:?}",
                    stabilizer.rows().len(),
                    overlay.size()
                );
            }
        }

        let show =
            !hidden && scanning && game_present && (game_focused || !cfg.pause_when_unfocused);
        let size = overlay.size();
        if size.0 > 0 && size.1 > 0 {
            let pm = pixmap.get_or_insert_with(|| {
                tiny_skia::Pixmap::new(size.0, size.1).expect("pixmap")
            });
            if (pm.width(), pm.height()) != size {
                *pm = tiny_skia::Pixmap::new(size.0, size.1).expect("pixmap");
            }
            if show {
                let rows = stabilizer.rows();
                let out_pos = overlay.output_pos();
                let placed: Vec<_> = rows
                    .iter()
                    .map(|r| {
                        let (lx, ly) = map.label_pos_logical(r.y_top);
                        // Global logical -> surface-local (output-relative);
                        // the game may have moved, so re-anchor on its live pos.
                        let dx = game_pos.0 - map.window_logical.x;
                        let dy = game_pos.1 - map.window_logical.y;
                        poe2_lens::render::Placed {
                            x: lx + dx - out_pos.0,
                            y: ly + dy - out_pos.1,
                            label: r.label.clone(),
                            tier: r.tier,
                        }
                    })
                    .collect();
                renderer.draw_frame(pm, &placed, "", stabilizer.stale());
            } else {
                pm.fill(tiny_skia::Color::TRANSPARENT);
            }
            overlay.present(pm)?;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

fn mean_gray_brightness(img: &image::GrayImage) -> u64 {
    let raw = img.as_raw();
    if raw.is_empty() {
        return 0;
    }
    raw.iter().map(|&p| p as u64).sum::<u64>() / raw.len() as u64
}
