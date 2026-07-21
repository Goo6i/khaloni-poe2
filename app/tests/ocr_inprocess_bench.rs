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
