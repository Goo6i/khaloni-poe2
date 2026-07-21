use poe2_lens_core::matcher::{match_rows, normalize, MatchTier, Vocab};

fn lines(s: &str) -> Vec<String> {
    s.lines().map(|l| l.to_string()).collect()
}

/// Expected rows carry a leading "Nx " count for currency; vocabulary entries
/// are the names without counts.
fn split_count(row: &str) -> (Option<u32>, String) {
    if let Some((n, rest)) = row.split_once("x ") {
        if let Ok(c) = n.parse::<u32>() {
            return (Some(c), rest.to_string());
        }
    }
    (None, row.to_string())
}

#[test]
fn normalize_strips_punctuation_and_case() {
    assert_eq!(
        normalize("Skill Level 20: Conductive Runes!"),
        "skill level 20 conductive runes"
    );
    assert_eq!(normalize("  a   b  "), "a b");
}

#[test]
fn substring_tier_beats_fuzzy_tier() {
    let vocab = Vocab::new(vec!["Verisium Pile".to_string()]);
    let hits = match_rows(
        &vocab,
        &[" pile ".to_string()],
        &[" verisium pile f ".to_string()],
    );
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].entry_index, 0);
    assert_eq!(hits[0].tier, MatchTier::Substring);
}

#[test]
fn count_comes_from_the_matched_line_only() {
    let vocab = Vocab::new(vec!["Exalted Orb".to_string()]);
    let hits = match_rows(
        &vocab,
        &[" 2x exalted orb".to_string()],
        &[" 2x exalted orb et".to_string()],
    );
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].count, Some(2));
}

#[test]
fn junk_lines_do_not_match() {
    let vocab = Vocab::new(vec![
        "Skill Level 20: Leylines".to_string(),
        "Exalted Orb".to_string(),
    ]);
    // "skill level 20" alone must not fuzzy-claim the Leylines entry
    let hits = match_rows(
        &vocab,
        &["skill level 20".to_string(), " 7 1".to_string()],
        &["skill level 20".to_string(), " 7 1 ".to_string()],
    );
    assert!(hits.is_empty());
}

/// A near-identical variant family: normalized Levenshtein between the three
/// entries sits around 0.81-0.95 of each other, well above FUZZY_THRESHOLD,
/// so a single garbled OCR line can plausibly fuzzy-match more than one of
/// them.
fn jeweller_variants() -> Vocab {
    Vocab::new(vec![
        "Lesser Jewellers Orb".to_string(),
        "Greater Jewellers Orb".to_string(),
        "Perfect Jewellers Orb".to_string(),
    ])
}

#[test]
fn clean_variant_line_matches_the_named_variant_exactly() {
    let vocab = jeweller_variants();
    let hits = match_rows(
        &vocab,
        &["1x greater jewellers orb".to_string()],
        &["1x greater jewellers orb".to_string()],
    );
    assert_eq!(hits.len(), 1);
    assert_eq!(vocab.entry(hits[0].entry_index), "Greater Jewellers Orb");
    assert_ne!(hits[0].tier, MatchTier::Ambiguous);
    assert_eq!(hits[0].count, Some(1));
}

#[test]
fn corrupted_variant_line_is_ambiguous_not_a_wrong_guess() {
    let vocab = jeweller_variants();
    // "gleaser" fuzzy-scores Greater and Lesser within a hair of each other
    // (0.86 vs 0.86), both clearing FUZZY_THRESHOLD; picking either would be
    // a coin flip, so this must come back Ambiguous rather than Lesser or
    // Greater.
    let hits = match_rows(
        &vocab,
        &["1x gleaser jewellers orb".to_string()],
        &["1x gleaser jewellers orb".to_string()],
    );
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].tier, MatchTier::Ambiguous);
}

#[test]
fn substring_tier_picks_the_longest_most_specific_entry() {
    let vocab = Vocab::new(vec![
        "Jewellers Orb".to_string(),
        "Perfect Jewellers Orb".to_string(),
    ]);
    let hits = match_rows(
        &vocab,
        &["1x perfect jewellers orb".to_string()],
        &["1x perfect jewellers orb".to_string()],
    );
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].tier, MatchTier::Substring);
    assert_eq!(vocab.entry(hits[0].entry_index), "Perfect Jewellers Orb");
}

#[test]
fn replays_milestone0_shootout_39_of_40_with_no_wrong_counts() {
    let fixtures = [
        (
            include_str!("fixtures/rows/s1_filtered.txt"),
            include_str!("fixtures/rows/s1_unfiltered.txt"),
            include_str!("fixtures/rows/s1_expected.txt"),
        ),
        (
            include_str!("fixtures/rows/s2_filtered.txt"),
            include_str!("fixtures/rows/s2_unfiltered.txt"),
            include_str!("fixtures/rows/s2_expected.txt"),
        ),
        (
            include_str!("fixtures/rows/s3_filtered.txt"),
            include_str!("fixtures/rows/s3_unfiltered.txt"),
            include_str!("fixtures/rows/s3_expected.txt"),
        ),
        (
            include_str!("fixtures/rows/s4_filtered.txt"),
            include_str!("fixtures/rows/s4_unfiltered.txt"),
            include_str!("fixtures/rows/s4_expected.txt"),
        ),
        (
            include_str!("fixtures/rows/s5_filtered.txt"),
            include_str!("fixtures/rows/s5_unfiltered.txt"),
            include_str!("fixtures/rows/s5_expected.txt"),
        ),
    ];

    let mut total_expected = 0usize;
    let mut total_hit = 0usize;
    let mut missed = Vec::new();

    for (i, (filtered, unfiltered, expected)) in fixtures.iter().enumerate() {
        let expected_rows: Vec<(Option<u32>, String)> = expected
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(split_count)
            .collect();
        let vocab_names: Vec<String> =
            expected_rows.iter().map(|(_, n)| n.clone()).collect();
        let vocab = Vocab::new(vocab_names.clone());

        let hits = match_rows(&vocab, &lines(filtered), &lines(unfiltered));

        // wrong-count rule: a hit with Some(count) must exist in the expected
        // set with exactly that count; a hit with None must at least name a
        // real expected row (count unknown is allowed, wrong is not)
        for h in &hits {
            let name = normalize(&vocab_names[h.entry_index]);
            match h.count {
                Some(c) => {
                    let ok = expected_rows
                        .iter()
                        .any(|(ec, en)| *ec == Some(c) && normalize(en) == name);
                    assert!(ok, "sample s{}: wrong-count hit {:?} {}", i + 1, h.count, name);
                }
                None => {
                    let ok = expected_rows.iter().any(|(_, en)| normalize(en) == name);
                    assert!(ok, "sample s{}: false name hit {}", i + 1, name);
                }
            }
        }

        for (c, n) in &expected_rows {
            total_expected += 1;
            let found = hits.iter().any(|h| {
                normalize(&vocab_names[h.entry_index]) == normalize(n)
                    && (h.count == *c || h.count.is_none())
            });
            if found {
                total_hit += 1;
            } else {
                missed.push(format!("s{}: {:?} {}", i + 1, c, n));
            }
        }
    }

    assert_eq!(total_expected, 40);
    // Fixtures are static; the deterministic baseline is exactly 39 hits.
    // A 40th hit would indicate a matcher false positive.
    assert_eq!(
        total_hit, 39,
        "hit {}/{}, missed: {:?}",
        total_hit,
        total_expected,
        missed
    );
    // the single known detection miss from the shootout
    assert!(missed.iter().all(|m| m.contains("Exalted Orb")), "{:?}", missed);
}
