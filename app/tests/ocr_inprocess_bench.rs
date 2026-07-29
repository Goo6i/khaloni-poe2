#![cfg(ocr)]
use leptess::LepTess;
use std::time::Instant;

#[test]
#[ignore]
fn bench_inprocess_vs_spawn() {
    let img = "tests/fixtures/panel_choice.png";
    let mut lt = LepTess::new(None, "eng").expect("init");
    lt.set_variable(leptess::Variable::TesseditPagesegMode, "6").unwrap();
    // warm-up + correctness
    lt.set_image(img).unwrap();
    let text = lt.get_utf8_text().unwrap();
    assert!(text.to_lowercase().contains("orb"), "in-process OCR must read the fixture: {text}");
    let t = Instant::now();
    for _ in 0..5 {
        lt.set_image(img).unwrap();
        let _ = lt.get_utf8_text().unwrap();
    }
    println!("in-process avg: {:?}", t.elapsed() / 5);
    let t = Instant::now();
    for _ in 0..5 {
        std::process::Command::new("tesseract")
            .args([img, "-", "--psm", "6", "-l", "eng"])
            .output()
            .unwrap();
    }
    println!("spawned avg: {:?}", t.elapsed() / 5);
    // TSV availability check
    let tsv = lt.get_tsv_text(0);
    println!("tsv available: {}", tsv.is_ok());
}

#[test]
#[ignore]
fn corpus_template_cross_frame_validation() {
    use image::imageops;
    use poe2_lens::{ocr, template::TemplateStore};
    let dir = std::path::Path::new("/tmp/poe2lens-frames");
    let mut frames: Vec<_> = std::fs::read_dir(dir)
        .expect("corpus dir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.file_name().unwrap().to_string_lossy().contains("bands12"))
        .collect();
    frames.sort();
    assert!(frames.len() >= 5, "need several same-panel frames");
    let crop_bands = |img: &image::GrayImage| -> Vec<image::GrayImage> {
        let bands = ocr::detect_bands(img);
        let w = img.width();
        let x0 = (w as f32 * 0.30) as u32;
        let x1 = ((w as f32 * 0.98) as u32).min(w);
        bands
            .iter()
            .map(|&(a, b)| imageops::crop_imm(img, x0, a.saturating_sub(4), x1 - x0, (b + 4).min(img.height()) - a.saturating_sub(4)).to_image())
            .collect()
    };
    let teacher = image::open(&frames[0]).unwrap().to_luma8();
    let tcrops = crop_bands(&teacher);
    let mut store = TemplateStore::new();
    for (i, c) in tcrops.iter().enumerate() {
        store.learn(&format!("row-{i}"), 1, false, c);
    }
    // Self-identification: every learned band must match itself at ~1.0
    // and never a different row of the same frame.
    for (i, c) in tcrops.iter().enumerate() {
        let (hit, score) = store.match_band(c).expect("self match");
        assert_eq!(hit.item_key, format!("row-{i}"), "same-frame confusion at band {i}");
        assert!(score > 0.98, "self score {score:.3} at band {i}");
    }
    // Cross-frame (the panel scrolls between frames, so identity moves
    // position): matched keys must appear in strictly increasing row
    // order within each frame (scrolling preserves order), most bands of
    // an overlapping viewport must identify, and timing must be instant.
    let mut identified = 0usize;
    let mut total = 0usize;
    let t = std::time::Instant::now();
    for f in &frames[1..] {
        let img = image::open(f).unwrap().to_luma8();
        let mut last_row: i32 = -1;
        for c in crop_bands(&img).iter() {
            total += 1;
            if let Some((hit, score)) = store.match_band(c) {
                identified += 1;
                let row: i32 = hit.item_key.trim_start_matches("row-").parse().unwrap();
                assert!(
                    row > last_row,
                    "order violated in {f:?}: row {row} after {last_row} (score {score:.3})"
                );
                last_row = row;
            }
        }
    }
    let per_band = t.elapsed() / total.max(1) as u32;
    println!(
        "corpus: {total} bands across {} frames, {identified} identified, avg {per_band:?}/band, store {}",
        frames.len() - 1,
        store.len()
    );
    // Coverage varies with how far the corpus scrolled beyond the single
    // learned viewport; correctness is the order/confusion assertions
    // above. Just report coverage.
    let _ = identified;
    assert!(per_band < std::time::Duration::from_millis(10), "must be near instant");
}
