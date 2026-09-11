#![cfg(ocr)]
use khaloni_poe2::config::Config;
use khaloni_poe2::ocr;
use khaloni_poe2::pricing::{build_vocab, price_lines, Tier};
use khaloni_poe2_core::ninja::{ExchangeOverview, PriceTable};

/// A real capture of the "choice" Runeshape panel that whole-panel OCR
/// used to return 0 lines for (see the evidence block in app/src/ocr.rs):
/// 4 bright reward rows over a large mid-gray parchment map. Top to
/// bottom: "Unique Jewellery" (no count), "1x Greater Jeweller's Orb",
/// "1x Cyclonic Alloy", "3x Exalted Orb".
fn fixture_image() -> image::GrayImage {
    image::load_from_memory(include_bytes!("fixtures/panel_choice.png"))
        .expect("panel_choice.png must decode")
        .to_luma8()
}

#[test]
fn detects_all_four_bands_on_the_real_choice_panel_fixture() {
    let img = fixture_image();
    let bands = ocr::detect_bands(&img);
    assert_eq!(bands.len(), 4, "expected exactly 4 bands: {bands:?}");
}

#[test]
fn per_strip_ocr_prices_the_choice_panel_fixture_rows() {
    let img = fixture_image();

    let cfg = Config::default();
    // ocr_scan unions band OCR (this panel style's only working pass -
    // whole-panel OCR returns 0 lines here, see the evidence block in
    // app/src/ocr.rs) with the whole-panel pass; on this fixture the
    // union degrades gracefully to "just the band lines" since there's
    // nothing from the other pass to merge in.
    let mut engine = ocr::OcrEngine::new().expect("tesseract init");
    let bars = ocr::reward_bars(&img, &ocr::row_profile(&img));
    let lines = ocr::ocr_scan(&mut engine, &img, &bars);
    assert_eq!(lines.len(), 4, "all 4 bands must survive per-strip OCR + MIN_WORD_RUN: {lines:?}");

    // A small vocab with just the two real currency names visible on this
    // panel. The apostrophe in "Jeweller's" is deliberate: normalize()
    // strips it into a separating space on both the vocab side and the OCR
    // side identically, so this matches the game's real spelling exactly
    // rather than the apostrophe-free "Jewellers" used elsewhere in tests.
    let overview: ExchangeOverview = serde_json::from_str(
        r#"{
        "core": {"items": [], "rates": {"exalted": 412.0, "chaos": 7.29}, "primary": "divine", "secondary": "chaos"},
        "lines": [
            {"id": "greater-jewellers-orb", "primaryValue": 0.02},
            {"id": "exalted-orb", "primaryValue": 0.0024}
        ],
        "items": [
            {"id": "greater-jewellers-orb", "name": "Greater Jeweller's Orb", "category": "Currency"},
            {"id": "exalted-orb", "name": "Exalted Orb", "category": "Currency"}
        ]
    }"#,
    )
    .unwrap();
    let table = PriceTable::build(&[overview]);
    let vocab = build_vocab(&table);

    let (rows, _) = price_lines(&table, &vocab, &lines, &cfg);
    assert_eq!(rows.len(), 4, "all 4 rows must show, either priced or as '?': {rows:?}");

    let priced: Vec<_> = rows.iter().filter(|r| r.tier != Tier::Unknown).collect();
    let unpriced: Vec<_> = rows.iter().filter(|r| r.tier == Tier::Unknown).collect();
    assert_eq!(
        priced.len(),
        2,
        "Greater Jeweller's Orb and Exalted Orb must price as real vocab hits: {rows:?}"
    );
    assert_eq!(
        unpriced.len(),
        2,
        "Unique Jewellery (contains 'unique') and 1x Cyclonic Alloy (has a count) must show as '?', not drop silently: {rows:?}"
    );

    assert!(
        priced.iter().any(|r| r.item_key == "greater jeweller s orb"),
        "missing the Greater Jeweller's Orb hit: {rows:?}"
    );
    assert!(
        priced.iter().any(|r| r.item_key == "exalted orb"),
        "missing the Exalted Orb hit: {rows:?}"
    );
}

#[test]
fn an_unchanged_panel_costs_no_ocr_and_a_changed_row_costs_one_band_pass() {
    let img = fixture_image();
    let bars = ocr::reward_bars(&img, &ocr::row_profile(&img));
    assert_eq!(bars.len(), 4);
    let mut engine = ocr::OcrEngine::new().expect("tesseract init");
    let mut cache = ocr::ScanCache::default();

    let first = cache.scan(&mut engine, &img, &bars, true);
    assert_eq!(first.len(), 4, "{first:?}");
    assert_eq!(cache.ocr_runs, 5, "four band passes plus the whole-panel pass");

    // Same pixels again: the memoised union comes back, no tesseract.
    let again = cache.scan(&mut engine, &img, &bars, true);
    assert_eq!(again, first);
    assert_eq!(cache.ocr_runs, 5);

    // Scribble over the third bar's text: that bar re-reads, the other
    // three come from the cache, and the whole-panel pass runs once.
    let mut changed = img.clone();
    let (y0, y1) = bars[2];
    for y in y0 + 8..y1 - 8 {
        for x in changed.width() / 2..changed.width() - 20 {
            changed.put_pixel(x, y, image::Luma([(x % 7 * 30) as u8]));
        }
    }
    let after = cache.scan(&mut engine, &changed, &bars, true);
    assert_eq!(cache.ocr_runs, 7, "one band pass and one whole pass");
    // The untouched rows still read the same and sit where they were.
    for (a, b) in first.iter().zip(&after).filter(|(a, _)| a.y_top != y0 * ocr::UPSCALE) {
        assert_eq!(a.unfiltered, b.unfiltered);
        assert_eq!(a.y_top, b.y_top);
    }

    // Bands-only (post-scroll) never memoises a scene, but reuses bars.
    let fast = cache.scan(&mut engine, &img, &bars, false);
    assert_eq!(fast.len(), 4);
    assert_eq!(cache.ocr_runs, 7, "every bar of the original is cached");
}
