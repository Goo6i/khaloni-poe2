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
    let pre = ocr::preprocess(&img);
    let lines = ocr::run_tesseract(&cfg.tesseract_cmd, &pre)?;
    for l in &lines {
        eprintln!("line y={:>4}: filtered={:?}", l.y_top, l.filtered);
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
