use poe2_lens::config::Config;
use poe2_lens::ocr::OcrLine;
use poe2_lens::pricing::{build_vocab, price_lines, Tier};
use poe2_lens_core::ninja::{ExchangeOverview, PriceTable};

fn table() -> PriceTable {
    // Reuse the phase-2 currency fixture and a synthetic UncutGems overview
    // shaped exactly like the live response verified 2026-07-21.
    let currency: ExchangeOverview =
        serde_json::from_str(include_str!("../../core/tests/fixtures/ninja_currency.json"))
            .unwrap();
    let gems: ExchangeOverview = serde_json::from_str(
        r#"{
        "core": {"items": [], "rates": {"exalted": 412.0, "chaos": 7.29}, "primary": "divine", "secondary": "chaos"},
        "lines": [{"id": "uncut-skill-gem-20", "primaryValue": 0.05}],
        "items": [{"id": "uncut-skill-gem-20", "name": "Uncut Skill Gem (Level 20)", "category": "UncutGems"}]
    }"#,
    )
    .unwrap();
    PriceTable::build(&[currency, gems])
}

fn line(unf: &str, y: u32) -> OcrLine {
    OcrLine {
        filtered: unf.to_string(),
        unfiltered: unf.to_string(),
        y_top: y,
        height: 30,
    }
}

#[test]
fn prices_currency_rows_with_counts() {
    let t = table();
    let v = build_vocab(&t);
    let cfg = Config::default();
    let (rows, total) = price_lines(
        &t,
        &v,
        &[line("2x chaos orb", 10), line("1x exalted orb", 50)],
        &cfg,
    );
    assert_eq!(rows.len(), 2);
    assert!(rows[0].label.contains("each") || rows[0].label.contains("ex"));
    assert_ne!(rows[0].label, poe2_lens_core::value::UNKNOWN);
    assert!(!total.is_empty());
}

#[test]
fn maps_skill_gem_rows_by_level_and_never_guesses_support() {
    let t = table();
    let v = build_vocab(&t);
    let cfg = Config::default();
    let (rows, _) = price_lines(
        &t,
        &v,
        &[
            line("skill level 20 skyfall", 10),
            line("support concussive runes", 60),
        ],
        &cfg,
    );
    assert_eq!(rows.len(), 2);
    // Level 20 skill gem is priced from the UncutGems overview: 0.05 div * 412 = 20.6 ex.
    assert!(rows[0].label.contains("20.6"), "got {}", rows[0].label);
    // Support row has no level on the panel: must be "?" and Unknown tier.
    assert_eq!(rows[1].label, poe2_lens_core::value::UNKNOWN);
    assert_eq!(rows[1].tier, Tier::Unknown);
}

#[test]
fn unmatched_rows_are_dropped_not_guessed() {
    let t = table();
    let v = build_vocab(&t);
    let cfg = Config::default();
    let (rows, _) = price_lines(&t, &v, &[line("runeshape combinations", 0)], &cfg);
    assert!(rows.is_empty());
}
