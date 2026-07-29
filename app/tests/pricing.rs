use poe2_lens::config::Config;
use poe2_lens::ocr::OcrLine;
use poe2_lens::pricing::{build_vocab, price_lines, Denom, Tier};
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
    // amount is the label's leading number, denom carries what the label's
    // suffix used to say ("div" or "ex"); the renderer draws an icon instead.
    assert_ne!(rows[0].amount, poe2_lens_core::value::UNKNOWN);
    let expected_denom = if rows[0].label.contains(" div") { Denom::Divine } else { Denom::Exalted };
    assert_eq!(rows[0].denom, expected_denom);
    // The panel is a pick-one choice: no summed total is ever rendered.
    assert!(total.is_empty());
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
    assert!(rows[0].amount.contains("20.6"), "got {}", rows[0].amount);
    assert_eq!(rows[0].denom, Denom::Exalted);
    // Support row has no level on the panel: must be "?" and Unknown tier.
    assert_eq!(rows[1].label, poe2_lens_core::value::UNKNOWN);
    assert_eq!(rows[1].amount, poe2_lens_core::value::UNKNOWN);
    assert_eq!(rows[1].denom, Denom::None);
    assert_eq!(rows[1].tier, Tier::Unknown);
}

#[test]
fn specific_gems_price_individually_via_the_gem_pricer() {
    use poe2_lens::pricing::{price_lines_with_rumours, GemPricer, GemState};
    // A pricer that gives each named gem its own price, proving rows no longer
    // collapse to one uncut value.
    struct Mock;
    impl GemPricer for Mock {
        fn lookup(&self, skill: &str, level: u32) -> GemState {
            assert_eq!(level, 20);
            match skill {
                "detonate living" => GemState::Priced(1.0),
                "conductive runes" => GemState::Pending,
                _ => GemState::Unpriced,
            }
        }
    }
    let t = table();
    let v = build_vocab(&t);
    let cfg = Config::default();
    let (rows, _) = price_lines_with_rumours(
        &t,
        &v,
        &[
            line("skill level 20: detonate living", 0),
            line("skill level 20: conductive runes", 0),
        ],
        &cfg,
        None,
        Some(&Mock),
    );
    assert_eq!(rows.len(), 2);
    // Priced gem: its own trade price, keyed by name so it never templates back.
    assert_eq!(rows[0].item_key, "gemx:detonate living:20");
    assert!((rows[0].value_ex - 1.0).abs() < 1e-9, "got {}", rows[0].value_ex);
    // Pending gem shows a placeholder, not a stale/wrong price.
    assert_eq!(rows[1].item_key, "gemx:conductive runes:20");
    assert_eq!(rows[1].value_ex, 0.0);
}

#[test]
fn unmatched_rows_are_dropped_not_guessed() {
    let t = table();
    let v = build_vocab(&t);
    let cfg = Config::default();
    let (rows, _) = price_lines(&t, &v, &[line("runeshape combinations", 0)], &cfg);
    assert!(rows.is_empty());
}

#[test]
fn unmatched_counted_stack_shows_question_mark_instead_of_dropping() {
    let t = table();
    let v = build_vocab(&t);
    let cfg = Config::default();
    // Leading count token but the item name itself doesn't match anything
    // in the vocab (mangled OCR): still worth a row, not a silent drop.
    let (rows, _) = price_lines(&t, &v, &[line("3x mystery orb", 10)], &cfg);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].label, poe2_lens_core::value::UNKNOWN);
    assert_eq!(rows[0].amount, poe2_lens_core::value::UNKNOWN);
    assert_eq!(rows[0].denom, Denom::None);
    assert_eq!(rows[0].tier, Tier::Unknown);
}

#[test]
fn ambiguous_variant_match_shows_question_mark_not_a_guess() {
    // Three near-identical currency variants, priced differently, shaped
    // like the real Lesser/Greater/Perfect Jeweller's Orb family that
    // motivated the matcher's ambiguity rule.
    let variants: ExchangeOverview = serde_json::from_str(
        r#"{
        "core": {"items": [], "rates": {"exalted": 412.0, "chaos": 7.29}, "primary": "divine", "secondary": "chaos"},
        "lines": [
            {"id": "lesser-jewellers-orb", "primaryValue": 0.001},
            {"id": "greater-jewellers-orb", "primaryValue": 0.02},
            {"id": "perfect-jewellers-orb", "primaryValue": 0.5}
        ],
        "items": [
            {"id": "lesser-jewellers-orb", "name": "Lesser Jewellers Orb", "category": "Currency"},
            {"id": "greater-jewellers-orb", "name": "Greater Jewellers Orb", "category": "Currency"},
            {"id": "perfect-jewellers-orb", "name": "Perfect Jewellers Orb", "category": "Currency"}
        ]
    }"#,
    )
    .unwrap();
    let t = PriceTable::build(&[variants]);
    let v = build_vocab(&t);
    let cfg = Config::default();

    // A clean line still resolves to its exact variant, priced.
    let (clean_rows, _) = price_lines(&t, &v, &[line("1x greater jewellers orb", 10)], &cfg);
    assert_eq!(clean_rows.len(), 1);
    assert_ne!(clean_rows[0].label, poe2_lens_core::value::UNKNOWN);
    assert_ne!(clean_rows[0].amount, poe2_lens_core::value::UNKNOWN);
    assert_ne!(clean_rows[0].denom, Denom::None);

    // "gleaser" fuzzy-scores Lesser and Greater within a hair of each
    // other; showing either price would risk being wrong, so this must
    // render as "?" rather than the row silently pricing as either.
    let (rows, _) = price_lines(&t, &v, &[line("1x gleaser jewellers orb", 10)], &cfg);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].label, poe2_lens_core::value::UNKNOWN);
    assert_eq!(rows[0].amount, poe2_lens_core::value::UNKNOWN);
    assert_eq!(rows[0].denom, Denom::None);
    assert_eq!(rows[0].tier, Tier::Unknown);
}

#[test]
fn unmatched_unique_line_shows_question_mark_instead_of_dropping() {
    let t = table();
    let v = build_vocab(&t);
    let cfg = Config::default();
    let (rows, _) = price_lines(&t, &v, &[line("saqawals unique rune", 10)], &cfg);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].label, poe2_lens_core::value::UNKNOWN);
    assert_eq!(rows[0].amount, poe2_lens_core::value::UNKNOWN);
    assert_eq!(rows[0].denom, Denom::None);
    assert_eq!(rows[0].tier, Tier::Unknown);
}

#[test]
fn unmatched_line_resolves_as_a_rumour_annotation() {
    let t = table();
    let v = build_vocab(&t);
    let cfg = Config::default();
    let idx = poe2_lens_core::rumour::RumourIndex::new(poe2_lens_core::rumour::parse_csv(
        include_str!("../../core/tests/fixtures/rumours.csv"),
    ));
    let (rows, _) = poe2_lens::pricing::price_lines_with_rumours(
        &t,
        &v,
        &[line("fallen stars", 10)],
        &cfg,
        Some(&idx),
        None,
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].item_key, "rumour:Fallen Stars|Moor|S+");
    assert_eq!(rows[0].amount, "Moor S+");
    assert_eq!(rows[0].denom, poe2_lens::pricing::Denom::None);
    assert!(rows[0].locks_in_one, "an exact rumour resolve locks in one read");
}

#[test]
fn rumour_template_key_reprices_without_ocr() {
    let t = table();
    let cfg = Config::default();
    let row = poe2_lens::pricing::price_resolved(
        &t,
        "rumour:Fallen Stars|Moor|S+",
        1,
        false,
        30,
        90,
        &cfg,
    )
    .expect("rumour keys must resolve without a table hit");
    assert_eq!(row.amount, "Moor S+");
    assert_eq!(row.denom, poe2_lens::pricing::Denom::None);
}
