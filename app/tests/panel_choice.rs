use poe2_lens::config::Config;
use poe2_lens::ocr;
use poe2_lens::pricing::{build_vocab, price_lines, Tier};
use poe2_lens_core::ninja::{ExchangeOverview, PriceTable};

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
    let bands = ocr::detect_bands(&img);
    assert_eq!(bands.len(), 4);

    let cfg = Config::default();
    let lines = ocr::ocr_bands(&cfg.tesseract_cmd, &img, &bands);
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
