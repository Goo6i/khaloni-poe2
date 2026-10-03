//! The reward-bar signature, pinned against every real panel style and
//! against the two things that fooled brightness alone: bright terrain
//! inside a stale scan region, and the parchment page-edge strip at the
//! top of every panel. Pure image math, runs on every platform.

use std::path::PathBuf;

use image::{imageops, GrayImage};
use khaloni_poe2::ocr::{detect_bands_from_profile, is_reward_bar, reward_bars, row_profile, RowSignature};

/// The region the detector reports on the live 4K fixture (see
/// tests/autoregion.rs): the open Runeshape book, left of the screen.
const LIVE_REGION: (u32, u32, u32, u32) = (96, 144, 1088, 1244);

fn live_frame() -> GrayImage {
    image::open(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/reward-live-1.png"))
        .expect("reward-live-1.png")
        .to_luma8()
}

fn crop(img: &GrayImage, (x, y, w, h): (u32, u32, u32, u32)) -> GrayImage {
    imageops::crop_imm(img, x, y, w, h).to_image()
}

/// The live frame with the book covered by terrain copied from the same
/// map: the exact "panel closed, region kept" situation the overlay lives
/// in most of the time on a bright map.
fn live_frame_without_panel() -> GrayImage {
    let mut frame = live_frame();
    let terrain = crop(&frame, (1900, 120, 1200, 1300));
    imageops::replace(&mut frame, &terrain, 60, 120);
    frame
}

fn sample(name: &str) -> Option<GrayImage> {
    let path: PathBuf =
        [env!("CARGO_MANIFEST_DIR"), "..", "spikes", "ocr", "samples", name].iter().collect();
    path.exists().then(|| image::open(&path).expect("sample decodes").to_luma8())
}

fn bars_of(region: &GrayImage) -> Vec<(u32, u32)> {
    reward_bars(region, &row_profile(region))
}

#[test]
fn the_live_book_has_exactly_its_two_reward_bars() {
    let region = crop(&live_frame(), LIVE_REGION);
    let profile = row_profile(&region);
    // Brightness alone also passes the page-edge strip at the top.
    assert_eq!(detect_bands_from_profile(&profile).len(), 3);
    let bars = bars_of(&region);
    assert_eq!(bars.len(), 2, "bars: {bars:?}");
    for (y0, y1) in &bars {
        assert!((70..=90).contains(&(y1 - y0)), "one reward row tall: {y0}-{y1}");
    }
}

#[test]
fn the_motion_signature_carries_the_band_profile() {
    // The reward pipeline finds bars on the profile its motion signature
    // computed, so the two must agree to the row.
    let region = crop(&live_frame(), LIVE_REGION);
    assert_eq!(RowSignature::of(&region).profile(), row_profile(&region).as_slice());
}

#[test]
fn terrain_in_the_stale_region_has_no_bars() {
    let region = crop(&live_frame_without_panel(), LIVE_REGION);
    let profile = row_profile(&region);
    assert!(!detect_bands_from_profile(&profile).is_empty(), "brightness alone finds a band");
    assert!(bars_of(&region).is_empty(), "the signature rejects it");
}

#[test]
fn the_choice_panel_keeps_all_four_bars() {
    let region = image::load_from_memory(include_bytes!("fixtures/panel_choice.png"))
        .expect("panel_choice.png")
        .to_luma8();
    assert_eq!(bars_of(&region).len(), 4);
}

#[test]
fn the_tall_gem_panels_keep_every_row_including_the_shaded_bottom_ones() {
    // Row counts read off the samples by eye: (rows, of which double
    // height). The last rows of s1/s2/s5 are split by a light shading gap
    // that the length-only merge rule used to cut in two.
    for (name, rows, tall_rows) in [
        ("s1.png", 10, 2),
        ("s2.png", 10, 2),
        ("s3.png", 3, 0),
        ("s4.png", 10, 0),
        ("s5.png", 7, 6),
    ] {
        let Some(region) = sample(name) else {
            eprintln!("SKIP: {name} absent");
            continue;
        };
        let bars = bars_of(&region);
        assert_eq!(bars.len(), rows, "{name}: {bars:?}");
        let tall = bars.iter().filter(|(y0, y1)| y1 - y0 > 120).count();
        assert_eq!(tall, tall_rows, "{name}: double-height rows in {bars:?}");
        for (y0, y1) in &bars {
            let h = y1 - y0;
            assert!((70..=90).contains(&h) || (135..=155).contains(&h), "{name}: odd row height {h}");
        }
    }
}

#[test]
fn a_band_touching_both_crop_edges_is_not_a_bar() {
    // A fully white crop (a loading screen inside the region) is one band
    // with no measurable edge on either side.
    let region = GrayImage::from_pixel(600, 300, image::Luma([240]));
    let profile = row_profile(&region);
    assert!(!is_reward_bar(&region, &profile, 0, 300));
}

#[test]
fn a_soft_edged_bright_run_is_not_a_bar() {
    // A bright plateau that fades in and out over many rows (a lit patch
    // of ground) has the brightness and width of a bar but no edge.
    let mut region = GrayImage::from_pixel(600, 200, image::Luma([120]));
    for y in 0..200u32 {
        let d = (y as i32 - 100).unsigned_abs();
        let v = (230i32 - i32::try_from(d * 3).unwrap()).clamp(120, 230) as u8;
        for x in 0..600 {
            region.put_pixel(x, y, image::Luma([v]));
        }
    }
    let profile = row_profile(&region);
    let bands = detect_bands_from_profile(&profile);
    assert!(!bands.is_empty(), "brightness alone accepts the plateau");
    assert!(reward_bars(&region, &profile).is_empty());
}

#[test]
fn a_narrow_bright_strip_is_not_a_bar() {
    // Hard edges and bar brightness, but only a third of the width: a
    // tooltip or HUD element, not a reward row spanning the panel.
    let mut region = GrayImage::from_pixel(600, 200, image::Luma([120]));
    for y in 80..140u32 {
        for x in 200..400 {
            region.put_pixel(x, y, image::Luma([230]));
        }
    }
    // The profile window (icon cut .. right trim) sees the strip as bright
    // enough to band, so only the width rule can reject it.
    let profile = row_profile(&region);
    assert!(reward_bars(&region, &profile).is_empty());
}
