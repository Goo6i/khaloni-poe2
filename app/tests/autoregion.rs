//! Auto-region detector gate: the reward panel must be found on synthetic
//! full-frame composites (a real panel crop pasted onto a dark canvas —
//! no real full-frame reward fixture exists yet, see the auto-region
//! design note), and must NOT fire on the 5 real rumour frames, whose
//! expedition tooltip is the same bright parchment the blob sweep keys on.
//!
//! Pure image math (no OCR, no capture), so unlike tests/rumours.rs this
//! file runs on every platform.

use std::path::PathBuf;

use image::{imageops, GrayImage};
use khaloni_poe2::autoregion::detect_reward_region;
use khaloni_poe2_core::rumour_scan::Rect;

/// Real "choice" Runeshape reward-panel region crop (996x1074): 4 bright
/// reward rows over the mid-gray parchment map (same fixture the band-OCR
/// evidence in app/src/ocr.rs was measured on).
fn panel_fixture() -> GrayImage {
    image::load_from_memory(include_bytes!("fixtures/panel_choice.png"))
        .expect("panel_choice.png must decode")
        .to_luma8()
}

/// 3840x2160 canvas at game-dark brightness (~40, well under every
/// threshold in play) with the panel fixture pasted at (x, y).
fn composite(panel: &GrayImage, x: u32, y: u32) -> GrayImage {
    let mut canvas = GrayImage::from_pixel(3840, 2160, image::Luma([40]));
    imageops::replace(&mut canvas, panel, i64::from(x), i64::from(y));
    canvas
}

fn iou(a: &Rect, b: &Rect) -> f64 {
    let ix0 = a.x0.max(b.x0);
    let iy0 = a.y0.max(b.y0);
    let ix1 = a.x1.min(b.x1);
    let iy1 = a.y1.min(b.y1);
    if ix1 <= ix0 || iy1 <= iy0 {
        return 0.0;
    }
    let inter = f64::from(ix1 - ix0) * f64::from(iy1 - iy0);
    let area = |r: &Rect| f64::from(r.width()) * f64::from(r.height());
    inter / (area(a) + area(b) - inter)
}

/// Paste the panel at (x, y) and require a detection that substantially
/// overlaps the pasted area (IoU > 0.5: the detector's blob box may snap
/// to the subsample grid and carry its own padding, but it must be *this*
/// panel it found, not something elsewhere on the canvas).
fn assert_found_at(x: u32, y: u32) {
    let panel = panel_fixture();
    let truth = Rect { x0: x, y0: y, x1: x + panel.width(), y1: y + panel.height() };
    let frame = composite(&panel, x, y);
    let got = detect_reward_region(&frame)
        .unwrap_or_else(|| panic!("no region detected with panel pasted at ({x}, {y})"));
    let overlap = iou(&got, &truth);
    assert!(overlap > 0.5, "detected {got:?} vs pasted {truth:?}: IoU {overlap:.3} <= 0.5");
}

#[test]
fn finds_the_panel_on_a_synthetic_full_frame() {
    assert_found_at(1400, 300);
}

#[test]
fn finds_the_panel_at_a_second_offset() {
    // Different quadrant of the frame: detection must not depend on where
    // the panel happens to sit.
    assert_found_at(2400, 900);
}

#[test]
fn a_dark_frame_detects_nothing() {
    let frame = GrayImage::from_pixel(3840, 2160, image::Luma([40]));
    assert!(detect_reward_region(&frame).is_none(), "nothing bright to find");
}

#[test]
fn rumour_frames_do_not_validate() {
    // The expedition rumour tooltip is bright parchment too and passes the
    // blob sweep; band validation must reject it on every real fixture.
    // Same skip-if-absent convention as tests/rumours.rs.
    let dir: PathBuf = [env!("CARGO_MANIFEST_DIR"), "tests", "fixtures", "rumours"]
        .iter()
        .collect();
    if !dir.exists() {
        eprintln!("SKIP: rumour fixtures absent at {}", dir.display());
        return;
    }
    let mut checked = 0;
    for n in 1..=5 {
        let path = dir.join(format!("rumour-{n}.png"));
        if !path.exists() {
            continue;
        }
        let gray = image::open(&path).expect("open fixture").to_luma8();
        let got = detect_reward_region(&gray);
        assert!(got.is_none(), "rumour-{n} must not validate as a reward panel, got {got:?}");
        checked += 1;
    }
    assert_eq!(checked, 5, "all 5 rumour fixtures present and checked");
}

#[test]
fn detects_the_real_live_reward_panel() {
    // A real 4K capture with the rune rewards panel open (live band means
    // 176/216/211 — dimmer than the synthetic fixture, the miss that made
    // detection take 30s of lucky frames before the threshold was retuned).
    let img = image::open("tests/fixtures/reward-live-1.png").unwrap().to_luma8();
    let r = khaloni_poe2::autoregion::detect_reward_region(&img).expect("live panel must detect");
    // The panel occupies the left half around (100,160)-(1176,1340).
    assert!(r.x0 < 300 && r.y0 < 400 && r.x1 > 1000 && r.y1 > 1100);
}
