use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};

use poe2_lens::{
    capture,
    config::{Config, Rect},
    coord::CoordMap,
    hover, inject, ocr, pricing, prices,
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
    // Capacity 1: only the latest frame is ever wanted; see capture::consume.
    let (ftx, frx) = mpsc::sync_channel(1);
    let (_rtx, rrx) = mpsc::channel::<Rect>();
    let region = map.region_px();
    // headless has no brightness gate of its own (it OCRs every frame
    // unconditionally below), so this never flips true: capture always
    // throttles at the closed (300ms) rate here.
    let panel_open = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    std::thread::spawn(move || {
        if let Err(e) = capture::consume(start, rrx, region, ftx, panel_open) {
            eprintln!("capture thread died: {e}");
        }
    });

    eprintln!("headless pipeline running; open a Runeshape panel. Ctrl+C to quit.");
    for frame in frx {
        let lines = ocr::ocr_scan(&cfg.tesseract_cmd, &frame.gray);
        let snap = svc.snapshot();
        let (rows, total) = pricing::price_lines(&snap.table, &snap.vocab, &lines, &cfg);
        println!(
            "--- scan ({} lines, {} priced){}",
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

/// What one tick draws+presents: the row labels, whether their prices are
/// stale, and the hover popup (with its anchor) if one is currently up.
type FrameState = (Vec<poe2_lens::render::Placed>, bool, Option<(hover::Popup, (i32, i32))>);

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

    // Hover price check: the Injector owns a persistent uinput virtual
    // keyboard, created once here since registering it with the compositor
    // is slow. A missing /dev/uinput permission (see inject.rs's doc
    // comment) is not fatal to the rest of the overlay: F7 just does
    // nothing and this is logged once at startup.
    let injector: Option<Arc<Mutex<inject::Injector>>> = match inject::Injector::new() {
        Ok(i) => Some(Arc::new(Mutex::new(i))),
        Err(e) => {
            eprintln!("price check unavailable: {e}");
            None
        }
    };
    // Guards against F7 firing again while an injection (Ctrl+C + the ~300ms
    // settle sleep in copy_hovered_item) is still in flight on its own
    // thread; also never touched while the game isn't focused, so a stray
    // F7 press over some other window is silently dropped.
    let price_check_in_flight = Arc::new(AtomicBool::new(false));
    let (clip_tx, clip_rx) = mpsc::channel::<anyhow::Result<String>>();

    let map = CoordMap::new(game, (3840, 2160), cal);
    // Capacity 1: only the latest frame is ever wanted; see capture::consume.
    let (ftx, frx) = mpsc::sync_channel(1);
    let (region_tx, region_rx) = mpsc::channel::<Rect>();
    let region = map.region_px();
    // Shared with the OCR worker below: it owns the BrightnessGate and
    // stores whether it's currently open here every pass; the capture
    // thread only reads it, to pick its 120ms/300ms throttle. An atomic is
    // the simplest correct way to move this one bit across the thread
    // boundary without a second channel (see capture::consume's doc comment).
    let panel_open = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let panel_open_capture = panel_open.clone();
    std::thread::spawn(move || {
        let _ = capture::consume(start, region_rx, region, ftx, panel_open_capture);
    });

    // OCR worker: frames in, priced rows out. `pipeline_paused` is toggled by the
    // main loop on focus loss / scan toggle, so we stop feeding tesseract without
    // touching the capture thread (which keeps running regardless, now that it
    // emits every throttle tick rather than only on pixel change).
    let pipeline_paused = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (rows_tx, rows_rx) = mpsc::channel();
    let svc_ocr = svc.clone();
    let ocr_cfg = cfg.clone();
    let paused_ocr = pipeline_paused.clone();
    std::thread::spawn(move || {
        let dbg = std::env::var("POE2LENS_DEBUG").is_ok();
        let t0 = std::time::Instant::now();
        let mut gate = poe2_lens::brightness::BrightnessGate::new(
            ocr_cfg.panel_open_brightness,
            ocr_cfg.panel_close_brightness,
        );
        // The frame channel is capacity-1 with try_send-and-drop on the
        // capture side (see capture::consume), so a backlog here is
        // structurally impossible: this is always the latest frame, and a
        // plain blocking recv (via the Receiver iterator) is enough.
        for frame in frx {
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
                let _ = rows_tx.send(poe2_lens::stabilize::ScanResult::GateEmpty);
                continue;
            }
            let bands = ocr::detect_bands(&frame.gray);
            if dbg {
                eprintln!("TRACE {:>8.2}s bands={}", t0.elapsed().as_secs_f32(), bands.len());
            }
            let lines = ocr::ocr_scan(&ocr_cfg.tesseract_cmd, &frame.gray);
            if dbg {
                let d = std::path::Path::new("/tmp/poe2lens-frames");
                let _ = std::fs::create_dir_all(d);
                let _ = frame.gray.save(d.join(format!(
                    "t{:06.2}_bands{}_lines{}.png",
                    t0.elapsed().as_secs_f32(),
                    bands.len(),
                    lines.len()
                )));
            }
            let snap = svc_ocr.snapshot();
            let out = pricing::price_lines(&snap.table, &snap.vocab, &lines, &ocr_cfg);
            // Fast-close invariant (regressed once, never again; see
            // stabilize tests): the panel's structural signal is the bright
            // reward bars. No bars AND no priced rows means the panel is
            // gone even when the region stays bright (terrain) and the
            // whole-panel pass reads scenery junk. Priced rows without
            // bars (band detector glitch) still count as a panel.
            if bands.is_empty() && out.0.is_empty() {
                let _ = rows_tx.send(poe2_lens::stabilize::ScanResult::NoBands);
                continue;
            }
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
            let _ = rows_tx.send(poe2_lens::stabilize::ScanResult::Rows(out.0, snap.stale));
        }
    });

    let center = (game.x + game.w as i32 / 2, game.y + game.h as i32 / 2);
    let mut overlay = poe2_lens::overlay::Overlay::new(center)?;
    let renderer = poe2_lens::render::Renderer::new()?;

    let mut scanning = true;
    let mut hidden = false;
    let mut game_focused = true;
    let mut game_present = true;
    let mut stabilizer = poe2_lens::stabilize::Stabilizer::new();
    let mut hover = hover::HoverState::default();
    let mut game_pos = (game.x, game.y);
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
    let dbg = std::env::var("POE2LENS_DEBUG").is_ok();

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
                    if !scanning {
                        stabilizer.clear();
                    }
                    // No forced rescan needed either way: capture emits a
                    // frame on every throttle tick regardless of pause
                    // state, so scanning turning back on picks up the next
                    // one within one tick on its own.
                }
                poe2_lens::hotkeys::Hotkey::Hide => hidden = !hidden,
                poe2_lens::hotkeys::Hotkey::PriceCheck => {
                    if let Some(inj) = &injector {
                        // game_focused-gated so a press over some other
                        // window never sends Ctrl+C into it; the swap is
                        // the concurrency guard (see price_check_in_flight's
                        // doc comment above), only reached at all when the
                        // game has focus.
                        if game_focused && !price_check_in_flight.swap(true, Ordering::AcqRel) {
                            let inj = inj.clone();
                            let tx = clip_tx.clone();
                            let in_flight = price_check_in_flight.clone();
                            std::thread::spawn(move || {
                                // The ~300ms Ctrl+C + settle sleep lives in
                                // copy_hovered_item; running it here keeps
                                // it off the tick loop entirely.
                                let result = inj
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                                    .copy_hovered_item();
                                let _ = tx.send(result);
                                in_flight.store(false, Ordering::Release);
                            });
                        }
                    }
                }
            }
        }

        // Drain injected clipboard text: reprice against whatever the price
        // table looks like right now (not at the moment F7 was pressed).
        while let Ok(result) = clip_rx.try_recv() {
            match result {
                Ok(text) => {
                    let snap = svc.snapshot();
                    hover.trigger(&text, &snap.table, cfg.divine_threshold);
                }
                Err(e) => eprintln!("price check: {e}"),
            }
        }
        hover.tick();

        let paused = !scanning || !game_present || (!game_focused && cfg.pause_when_unfocused);
        pipeline_paused.store(paused, std::sync::atomic::Ordering::Relaxed);

        while let Ok(msg) = rows_rx.try_recv() {
            if dbg {
                match &msg {
                    poe2_lens::stabilize::ScanResult::GateEmpty => {
                        eprintln!("DBG rows_rx: gate-empty");
                    }
                    poe2_lens::stabilize::ScanResult::NoBands => {
                        eprintln!("DBG rows_rx: no-bands");
                    }
                    poe2_lens::stabilize::ScanResult::Rows(rows, stale) => {
                        eprintln!("DBG rows_rx: {} rows, stale={stale}", rows.len());
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
                let rows = stabilizer.rows();
                let out_pos = overlay.output_pos();
                // Global logical -> surface-local (output-relative); the
                // game may have moved, so re-anchor on its live pos. Shared
                // by the row labels and the popup anchor below.
                let dx = game_pos.0 - map.window_logical.x;
                let dy = game_pos.1 - map.window_logical.y;
                let placed: Vec<_> = rows
                    .iter()
                    .map(|r| {
                        let (lx, ly) = map.label_pos_centered(r.y_top, r.height);
                        poe2_lens::render::Placed {
                            x: lx + dx - out_pos.0,
                            y: ly + dy - out_pos.1,
                            amount: r.amount.clone(),
                            denom: r.denom,
                            tier: r.tier,
                        }
                    })
                    .collect();
                // Popup anchor (Stage A fallback: no cursor coordinates are
                // available from the app side on Wayland): fixed margin off
                // the calibration rect's top-right, same coordinate route
                // as the row labels above rather than the live cursor.
                let popup = hover.current.as_ref().map(|p| {
                    let anchor_x = map.calibration.x + map.calibration.w as i32 + 24;
                    let anchor_y = map.calibration.y;
                    let ax = anchor_x + dx - out_pos.0;
                    let ay = anchor_y + dy - out_pos.1;
                    (p.clone(), (ax, ay))
                });
                Some((placed, stabilizer.stale(), popup))
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
                    Some((placed, stale, popup)) => {
                        renderer.draw_frame(pm, placed, "", *stale);
                        // Popup drawn after the rows so it sits on top.
                        if let Some((p, anchor)) = popup {
                            renderer.draw_popup(pm, p, *anchor);
                        }
                    }
                    None => pm.fill(tiny_skia::Color::TRANSPARENT),
                }
                overlay.present(pm)?;
                last_frame = frame_state;
            }
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
