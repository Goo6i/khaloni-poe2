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
        other => {
            eprintln!("overlay mode arrives in Stage B; got {other:?}. Use --headless or --calibrate.");
            Ok(())
        }
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
