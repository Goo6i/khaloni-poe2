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

/// Bright scenery of tooltip size reaches OCR on every polled frame; it
/// must cost one tesseract pass, not the full recipe, and an unchanged
/// frame none at all.
#[test]
fn a_tooltip_sized_blob_without_tooltip_text_costs_one_pass_then_none() {
    use khaloni_poe2::rumours::{RumourScanner, OCR_PASSES};
    let mut frame = image::GrayImage::new(3840, 2160);
    for y in 400..792 {
        for x in 2000..2620 {
            frame.put_pixel(x, y, image::Luma([215]));
        }
    }
    assert!(khaloni_poe2::rumours::find_panel(&frame).is_some(), "the blob must reach OCR for this to mean anything");
    let index = RumourIndex::new(parse_csv(SHEET));
    let mut engine = OcrEngine::new().expect("tesseract");
    let mut scanner = RumourScanner::default();
    assert!(scanner.recognize(&mut engine, &frame, &index).is_empty());
    assert_eq!(scanner.ocr_passes, 1, "no rumour and no tooltip chrome: the other {} passes are skipped", OCR_PASSES.len() - 1);
    assert!(scanner.recognize(&mut engine, &frame, &index).is_empty());
    assert_eq!(scanner.ocr_passes, 1, "an unchanged crop is not read again");
}

#[test]
fn an_unchanged_tooltip_is_read_once() {
    use khaloni_poe2::rumours::RumourScanner;
    let path: PathBuf =
        [env!("CARGO_MANIFEST_DIR"), "tests", "fixtures", "rumours", "rumour-2.png"].iter().collect();
    if !path.exists() {
        eprintln!("SKIP: rumour fixture absent at {}", path.display());
        return;
    }
    let gray = image::open(&path).expect("open fixture").to_luma8();
    let index = RumourIndex::new(parse_csv(SHEET));
    let mut engine = OcrEngine::new().expect("tesseract");
    let mut scanner = RumourScanner::default();
    let first = scanner.recognize(&mut engine, &gray, &index);
    let passes = scanner.ocr_passes;
    assert_eq!(first.len(), 3, "all three rumours, the third only a late pass reads");
    let again = scanner.recognize(&mut engine, &gray, &index);
    assert_eq!(scanner.ocr_passes, passes, "the second scan of identical pixels ran tesseract");
    let names = |v: &[khaloni_poe2::rumours::RumourHit]| v.iter().map(|h| h.entry.rumour.clone()).collect::<Vec<_>>();
    assert_eq!(names(&first), names(&again));

    // A tooltip that went away is forgotten, so reopening reads it afresh.
    scanner.recognize(&mut engine, &image::GrayImage::new(3840, 2160), &index);
    scanner.recognize(&mut engine, &gray, &index);
    assert!(scanner.ocr_passes > passes);
}

/// The tooltip sits over live scenery (water, particles, the day cycle),
/// so the pixels around it change on every frame while its own do not.
/// Only what the panel shows decides whether it needs reading again.
#[test]
fn an_unchanged_tooltip_is_not_read_again_while_its_surroundings_move() {
    use khaloni_poe2::rumours::{find_panel, RumourHit, RumourScanner};
    let path: PathBuf =
        [env!("CARGO_MANIFEST_DIR"), "tests", "fixtures", "rumours", "rumour-3.png"].iter().collect();
    if !path.exists() {
        eprintln!("SKIP: rumour fixture absent at {}", path.display());
        return;
    }
    let gray = image::open(&path).expect("open fixture").to_luma8();
    let panel = find_panel(&gray).expect("the fixture's tooltip");
    let index = RumourIndex::new(parse_csv(SHEET));
    let mut engine = OcrEngine::new().expect("tesseract");
    let mut scanner = RumourScanner::default();
    let first = scanner.recognize(&mut engine, &gray, &index);
    assert_eq!(first.len(), 2, "the fixture shows two rumours");
    let passes = scanner.ocr_passes;

    // Repaint everything outside the panel, the crop padding included,
    // dim enough that it cannot join the parchment blob.
    let mut moved = gray.clone();
    for (x, y, p) in moved.enumerate_pixels_mut() {
        let inside = (panel.x0..panel.x1).contains(&x) && (panel.y0..panel.y1).contains(&y);
        if !inside {
            p.0[0] = p.0[0] / 2 + ((x * 7 + y * 13) % 40) as u8;
        }
    }
    assert_eq!(find_panel(&moved), Some(panel), "the altered frame must still show the same panel");
    let again = scanner.recognize(&mut engine, &moved, &index);
    assert_eq!(scanner.ocr_passes, passes, "an unchanged tooltip ran tesseract because its surroundings moved");
    let seen = |v: &[RumourHit]| v.iter().map(|h| (h.entry.rumour.clone(), h.line)).collect::<Vec<_>>();
    assert_eq!(seen(&first), seen(&again));

    // A tooltip that moved is read again, and its lines land where it now is.
    let shift = 24;
    let mut shifted = image::GrayImage::new(gray.width(), gray.height());
    image::imageops::replace(&mut shifted, &gray, i64::from(shift), 0);
    let at_new_place = scanner.recognize(&mut engine, &shifted, &index);
    assert!(scanner.ocr_passes > passes, "a moved tooltip was not read");
    assert_eq!(at_new_place.len(), 2);
    assert!(at_new_place.iter().all(|h| first.iter().any(|f| f.entry.rumour == h.entry.rumour && h.line.x0 > f.line.x0)));

    // A tooltip whose rumours changed in place is read again: blank the
    // second rumour's row with the parchment around it.
    let passes = scanner.ocr_passes;
    scanner.recognize(&mut engine, &gray, &index);
    let passes_back = scanner.ocr_passes;
    assert!(passes_back > passes, "returning to the first place is a move too");
    let second = first.iter().max_by_key(|h| h.line.y0).expect("two rumours");
    let mut edited = gray.clone();
    let fill = gray.get_pixel(second.line.x0.saturating_sub(12), second.line.y0).0[0];
    for y in second.line.y0.saturating_sub(4)..second.line.y1 + 4 {
        for x in second.line.x0.saturating_sub(8)..second.line.x1 + 8 {
            edited.put_pixel(x, y, image::Luma([fill]));
        }
    }
    let after_edit = scanner.recognize(&mut engine, &edited, &index);
    assert!(scanner.ocr_passes > passes_back, "a tooltip whose rumours changed was not read");
    assert_eq!(after_edit.len(), 1, "only the rumour still shown is found: {:?}", after_edit.iter().map(|h| &h.entry.rumour).collect::<Vec<_>>());
}

/// Empty row slots are plain parchment, so a tooltip naming one or two
/// rumours stops reading once those resolve rather than running every
/// pass for rows that are not there.
#[test]
fn a_tooltip_with_fewer_than_three_rumours_stops_once_they_resolve() {
    use khaloni_poe2::rumours::{RumourScanner, OCR_PASSES};
    let dir: PathBuf = [env!("CARGO_MANIFEST_DIR"), "tests", "fixtures", "rumours"].iter().collect();
    if !dir.exists() {
        eprintln!("SKIP: rumour fixtures absent at {}", dir.display());
        return;
    }
    let index = RumourIndex::new(parse_csv(SHEET));
    let mut engine = OcrEngine::new().expect("tesseract");
    for name in ["rumour-1", "rumour-3", "rumour-4"] {
        let gray = image::open(dir.join(format!("{name}.png"))).expect("open fixture").to_luma8();
        let mut scanner = RumourScanner::default();
        let hits = scanner.recognize(&mut engine, &gray, &index);
        eprintln!("{name}: {} rumours in {} passes", hits.len(), scanner.ocr_passes);
        assert_eq!(hits.len(), expected(name).len(), "{name}");
        assert!(scanner.ocr_passes < OCR_PASSES.len(), "{name} ran every pass");
    }
}
