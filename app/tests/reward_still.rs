//! A still reward panel is never reported as scrolling. Live frames of an
//! untouched panel are not bit-identical (compositor dithering, animated
//! reward icons, scenery showing through the book's edges), and a false
//! one-pixel scroll would nudge every price off its row.

use image::{imageops, GrayImage, Luma};
use khaloni_poe2::ocr::{self, Motion, RowSignature, UiScale};

/// The reward book of `reward-live-1.png`, as the detector crops it.
const LIVE_REGION: (u32, u32, u32, u32) = (96, 144, 1088, 1244);

fn region() -> GrayImage {
    let full = image::open(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/reward-live-1.png"))
        .expect("reward-live-1.png")
        .to_luma8();
    let (x, y, w, h) = LIVE_REGION;
    imageops::crop_imm(&full, x, y, w, h).to_image()
}

/// A deterministic stream of small noise values in -amp..=amp.
fn noise(seed: &mut u64, amp: i32) -> i32 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    (*seed % (2 * amp as u64 + 1)) as i32 - amp
}

fn motion(a: &GrayImage, b: &GrayImage) -> Motion {
    let bars = ocr::reward_bars_at(a, &ocr::row_profile(a), UiScale::REFERENCE);
    assert_eq!(bars.len(), 2, "the live book has two reward bars");
    let span = (bars[0].0, bars[bars.len() - 1].1);
    ocr::track_motion(&RowSignature::of(a), &RowSignature::of(b), span, UiScale::REFERENCE)
}

#[test]
fn a_still_panel_with_live_frame_noise_is_not_a_scroll() {
    let base = region();
    let (w, h) = base.dimensions();

    // Dithering: every pixel off by up to 3 grey levels, frame to frame.
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut dithered = base.clone();
    for p in dithered.pixels_mut() {
        p.0[0] = (i32::from(p.0[0]) + noise(&mut seed, 3)).clamp(0, 255) as u8;
    }
    assert_eq!(motion(&base, &dithered), Motion::Still, "dithering is not a scroll");

    // An animated reward icon: the first icon cell of the first bar flips
    // to something else entirely.
    let bars = ocr::reward_bars_at(&base, &ocr::row_profile(&base), UiScale::REFERENCE);
    let (y0, y1) = bars[0];
    let mut animated = base.clone();
    for y in y0 + 8..y1 - 8 {
        for x in 60..130 {
            let v = base.get_pixel(x, y).0[0];
            animated.put_pixel(x, y, Luma([255 - v]));
        }
    }
    assert_eq!(motion(&base, &animated), Motion::Still, "an animated icon is not a scroll");

    // Scenery through the book's edges: the outer columns change freely.
    let mut scenery = base.clone();
    for y in 0..h {
        for x in (0..40).chain(w - 40..w) {
            scenery.put_pixel(x, y, Luma([noise(&mut seed, 120).unsigned_abs() as u8]));
        }
    }
    assert_eq!(motion(&base, &scenery), Motion::Still, "moving scenery beside the book is not a scroll");

    // All three at once, several frames running.
    let mut prev = base.clone();
    for _ in 0..10 {
        let mut next = scenery.clone();
        for p in next.pixels_mut() {
            p.0[0] = (i32::from(p.0[0]) + noise(&mut seed, 3)).clamp(0, 255) as u8;
        }
        assert_eq!(motion(&prev, &next), Motion::Still, "a still panel stays still frame after frame");
        prev = next;
    }
}
