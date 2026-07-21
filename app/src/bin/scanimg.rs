use poe2_lens::{config::Config, ocr, pricing, prices};
use poe2_lens_core::ninja::NinjaClient;

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("usage: scanimg <image>");
    let cfg = Config::load()?;
    let cache = directories::ProjectDirs::from("", "", "poe2-lens")
        .unwrap()
        .cache_dir()
        .to_path_buf();
    let svc = prices::PriceService::start(NinjaClient::new(cache), cfg.league.clone())?;
    let img = image::open(&path)?.to_luma8();
    let bands = ocr::detect_bands(&img);
    eprintln!("{} band(s) detected", bands.len());
    let lines = ocr::ocr_scan(&cfg.tesseract_cmd, &img);
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
