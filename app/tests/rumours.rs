#![cfg(target_os = "linux")]
//! Rumour recognizer gate against the 5 real 4K fixtures (ground truth from
//! pyoverlay/test_rumours.py). Full recall 10/10, 0 false positives: better
//! than the Python spike's 8/10, which lost rumour-4 (anchor-first failed to
//! locate that frame's tooltip) and both "Warm but risky" instances (that
//! rumour was missing from the community sheet until the rename this port
//! added). This port is panel-first and includes the renamed entry.
//!
//! The fixtures are large (11MB each) local-only screenshots under
//! app/tests/fixtures/rumours/. If they are absent the test skips rather
//! than fails, so the suite stays green on machines without them.

use std::collections::HashSet;
use std::path::PathBuf;

use khaloni_poe2::ocr::OcrEngine;
use khaloni_poe2::rumours::recognize;
use khaloni_poe2_core::rumour::{parse_csv, RumourIndex};

const SHEET: &str = include_str!("../../core/tests/fixtures/rumours.csv");

fn expected(fixture: &str) -> HashSet<&'static str> {
    let names: &[&str] = match fixture {
        "rumour-1" => &["Endless Cliffs"],
        // Sulphite! is genuinely the 3rd rumour on this panel (visible in
        // the frame); the original manual labels missed it because PSM 6
        // never read it cleanly. PSM 11 recovers it.
        "rumour-2" => &["Bleak and Awful", "Warm but risky", "Sulphite!"],
        "rumour-3" => &["Wild,.Roaming Free", "Cold as ice"],
        "rumour-4" => &["Cold as ice", "Wild,.Roaming Free"],
        "rumour-5" => &["Cold as ice", "Wild,.Roaming Free", "Warm but risky"],
        _ => &[],
    };
    names.iter().copied().collect()
}

#[test]
fn recognizes_rumours_on_real_fixtures_at_spike_parity() {
    let dir: PathBuf = [env!("CARGO_MANIFEST_DIR"), "tests", "fixtures", "rumours"]
        .iter()
        .collect();
    if !dir.exists() {
        eprintln!("SKIP: rumour fixtures absent at {}", dir.display());
        return;
    }

    let index = RumourIndex::new(parse_csv(SHEET));
    // Only names actually present in the dataset can ever be recalled.
    let dataset: HashSet<String> =
        parse_csv(SHEET).into_iter().map(|e| e.rumour).collect();

    let mut engine = OcrEngine::new().expect("tesseract");
    let mut matchable = 0usize; // expected names that exist in the dataset
    let mut recalled = 0usize; // of those, how many we found
    let mut false_positives = 0usize;

    for n in 1..=5 {
        let name = format!("rumour-{n}");
        let path = dir.join(format!("{name}.png"));
        if !path.exists() {
            continue;
        }
        let gray = image::open(&path).expect("open fixture").to_luma8();
        let hits = recognize(&mut engine, &gray, &index);
        let found: HashSet<String> = hits.iter().map(|h| h.entry.rumour.clone()).collect();
        let exp = expected(&name);
        let exp_matchable: HashSet<&str> =
            exp.iter().copied().filter(|e| dataset.contains(*e)).collect();

        matchable += exp_matchable.len();
        recalled += found.iter().filter(|f| exp_matchable.contains(f.as_str())).count();
        false_positives += found.iter().filter(|f| !exp.contains(f.as_str())).count();

        eprintln!("{name}: found {found:?}");
    }

    eprintln!("RECALL {recalled}/{matchable}  false-positives {false_positives}");
    assert_eq!(matchable, 11, "all 11 ground-truth rumours now in the dataset");
    assert_eq!(false_positives, 0, "no false positives");
    assert_eq!(recalled, 11, "full recall on the fixtures");
}
