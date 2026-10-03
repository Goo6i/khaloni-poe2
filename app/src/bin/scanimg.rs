/// OCR + pricing over a saved image; a dev aid. Needs the Linux OCR stack
/// (leptess) — OCR linking on Windows is an SP3 packaging task, see
/// app/src/platform/windows/mod.rs.
#[cfg(not(ocr))]
fn main() -> anyhow::Result<()> {
    anyhow::bail!("scanimg needs the Linux OCR stack; windows OCR linking lands in SP3")
}

#[cfg(ocr)]
fn main() -> anyhow::Result<()> {
    use khaloni_poe2::{config::Config, ocr, pricing, prices};
    use khaloni_poe2_core::ninja::NinjaClient;

    let path = std::env::args().nth(1).expect("usage: scanimg <region image> [full frame height]");
    // The image is a reward-region crop, which does not say how tall the
    // frame it was cut from is; without that the 4K reference is assumed.
    let scale = match std::env::args().nth(2).and_then(|h| h.parse::<u32>().ok()) {
        Some(h) => ocr::UiScale::from_frame_height(h),
        None => ocr::UiScale::REFERENCE,
    };
    let cfg = Config::load()?;
    let cache = directories::ProjectDirs::from("", "", "khaloni-poe2")
        .unwrap()
        .cache_dir()
        .to_path_buf();
    let svc = prices::PriceService::start(
        NinjaClient::new(cache.clone()),
        khaloni_poe2_core::scout::ScoutClient::new(cache),
        cfg.league.clone(),
    )?;
    // The service starts empty and fills from a worker thread (startup must
    // never block on the network); a one-shot scan wants the real table, so
    // give the first fetch a bounded head start before pricing.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while svc.snapshot().table.is_empty() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    eprintln!("price table: {} names", svc.snapshot().table.len());
    let img = image::open(&path)?.to_luma8();
    let bars = ocr::reward_bars_at(&img, &ocr::row_profile(&img), scale);
    eprintln!("{} reward bar(s) detected: {bars:?}", bars.len());
    let mut engine = ocr::OcrEngine::new()?;
    let lines = ocr::ocr_scan_at(&mut engine, &img, &bars, scale);
    for l in &lines {
        eprintln!("line y={:>4}: filtered={:?} unfiltered={:?}", l.y_top, l.filtered, l.unfiltered);
    }
    let snap = svc.snapshot();
    let (rows, total) = pricing::price_lines(&snap.table, &snap.vocab, &lines, &cfg);
    println!("{} lines -> {} priced rows", lines.len(), rows.len());
    for r in &rows {
        println!("  y={:>4} {:?} {}", r.y_top, r.tier, r.label);
    }
    println!("total: {total}");
    Ok(())
}
