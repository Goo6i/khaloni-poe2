//! The game lays its interface out against the window height, so every
//! panel measurement taken at 4K shrinks to two thirds on a 1440p client
//! and to half at 1080p. These tests replay the 4K fixtures at those two
//! sizes (downscaled in-process, so no extra fixture bytes) and hold the
//! detectors to the same answers, scaled.

use image::{imageops, GrayImage};
use khaloni_poe2::autoregion::detect_reward_region;
use khaloni_poe2::ocr::{self, UiScale};
use khaloni_poe2::rumours::find_panel;

/// (frame height, label) of the client sizes replayed below 4K.
const SIZES: [(u32, &str); 2] = [(1440, "1440p"), (1080, "1080p")];

fn downscaled(img: &GrayImage, frame_height: u32) -> GrayImage {
    let w = (u64::from(img.width()) * u64::from(frame_height) / 2160) as u32;
    let h = (u64::from(img.height()) * u64::from(frame_height) / 2160) as u32;
    imageops::resize(img, w, h, imageops::FilterType::Triangle)
}

fn panel_choice() -> GrayImage {
    image::load_from_memory(include_bytes!("fixtures/panel_choice.png"))
        .expect("panel_choice.png must decode")
        .to_luma8()
}

fn reward_live() -> GrayImage {
    image::open(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/reward-live-1.png"))
        .expect("reward-live-1.png")
        .to_luma8()
}

fn close(a: u32, b: f32, tol: f32) -> bool {
    (a as f32 - b).abs() <= tol
}

#[test]
fn ui_scale_is_the_identity_at_the_reference_height() {
    let s = UiScale::from_frame_height(2160);
    assert_eq!(s, UiScale::REFERENCE);
    for n in [0, 1, 2, 3, 4, 12, 13, 24, 240] {
        assert_eq!(s.px(n), n);
    }
    assert_eq!(UiScale::from_frame_height(1080).px(13), 7);
    assert_eq!(UiScale::from_frame_height(1080).px(1), 1, "a length never scales away");
}

#[test]
fn the_choice_panel_keeps_four_separate_bars_at_every_client_size() {
    let full = panel_choice();
    let bars_4k = ocr::reward_bars(&full, &ocr::row_profile(&full));
    assert_eq!(bars_4k.len(), 4);
    assert_eq!(
        bars_4k,
        ocr::reward_bars_at(&full, &ocr::row_profile(&full), UiScale::from_frame_height(2160)),
        "the scaled path at 4K is the unscaled path"
    );

    for (height, label) in SIZES {
        let scale = UiScale::from_frame_height(height);
        let img = downscaled(&full, height);
        let profile = ocr::row_profile(&img);
        let bars = ocr::reward_bars_at(&img, &profile, scale);
        assert_eq!(bars.len(), 4, "{label}: rows merged or dropped: {bars:?}");
        for (&(y0, y1), &(r0, r1)) in bars.iter().zip(&bars_4k) {
            assert!(
                close(y0, r0 as f32 * scale.factor(), 2.0) && close(y1, r1 as f32 * scale.factor(), 2.0),
                "{label}: bar ({y0},{y1}) is not 4K bar ({r0},{r1}) scaled"
            );
        }
    }
}

#[test]
fn the_live_reward_panel_is_found_at_every_client_size() {
    let full = reward_live();
    let r4k = detect_reward_region(&full).expect("4K frame must detect");
    for (height, label) in SIZES {
        let scale = UiScale::from_frame_height(height);
        let frame = downscaled(&full, height);
        let r = detect_reward_region(&frame)
            .unwrap_or_else(|| panic!("{label}: no reward region on a frame that has one at 4K"));
        // Blob edges snap to the sweep grid (one step each side). The top
        // and bottom edges run through the faded map, where the blob ends
        // wherever the parchment happens to cross the sweep threshold, so
        // resampling moves them further than the hard left/right edges.
        for (got, want, tol) in [(r.x0, r4k.x0, 12.0), (r.x1, r4k.x1, 12.0), (r.y0, r4k.y0, 30.0), (r.y1, r4k.y1, 30.0)] {
            assert!(
                close(got, want as f32 * scale.factor(), tol),
                "{label}: region {r:?} is not the 4K region {r4k:?} scaled"
            );
        }
        // The region must be usable by the scanner that reads it.
        let crop = imageops::crop_imm(&frame, r.x0, r.y0, r.x1 - r.x0, r.y1 - r.y0).to_image();
        let bars_4k = {
            let c = imageops::crop_imm(&full, r4k.x0, r4k.y0, r4k.x1 - r4k.x0, r4k.y1 - r4k.y0).to_image();
            ocr::reward_bars(&c, &ocr::row_profile(&c)).len()
        };
        let bars = ocr::reward_bars_at(&crop, &ocr::row_profile(&crop), scale).len();
        assert_eq!(bars, bars_4k, "{label}: the scanner sees a different number of rows than at 4K");
    }
}

#[test]
fn the_rumour_tooltip_is_found_at_every_client_size() {
    let dir: std::path::PathBuf =
        [env!("CARGO_MANIFEST_DIR"), "tests", "fixtures", "rumours"].iter().collect();
    if !dir.exists() {
        eprintln!("SKIP: rumour fixtures absent at {}", dir.display());
        return;
    }
    for n in 1..=5 {
        let path = dir.join(format!("rumour-{n}.png"));
        if !path.exists() {
            continue;
        }
        let full = image::open(&path).expect("open fixture").to_luma8();
        let p4k = find_panel(&full).expect("4K tooltip");
        for (height, label) in SIZES {
            let scale = UiScale::from_frame_height(height);
            let p = find_panel(&downscaled(&full, height))
                .unwrap_or_else(|| panic!("rumour-{n} {label}: tooltip not found"));
            for (got, want) in [(p.x0, p4k.x0), (p.y0, p4k.y0), (p.x1, p4k.x1), (p.y1, p4k.y1)] {
                assert!(
                    close(got, want as f32 * scale.factor(), 12.0),
                    "rumour-{n} {label}: panel {p:?} is not the 4K panel {p4k:?} scaled"
                );
            }
        }
    }
}

/// The rows tesseract reads off the choice panel at 4K must still be read
/// when the client renders it smaller: the crop is enlarged further so
/// the glyphs reach tesseract at the size the accuracy was measured at.
#[cfg(ocr)]
#[test]
fn the_choice_panel_rows_read_the_same_at_every_client_size() {
    use khaloni_poe2_core::matcher::{match_rows, Vocab};
    let vocab = Vocab::new(
        ["Greater Jeweller's Orb", "Exalted Orb", "Cyclonic Alloy"].iter().map(|s| s.to_string()).collect(),
    );
    let full = panel_choice();
    let mut engine = ocr::OcrEngine::new().expect("tesseract init");
    for (height, label) in SIZES {
        let scale = UiScale::from_frame_height(height);
        let img = downscaled(&full, height);
        let bars = ocr::reward_bars_at(&img, &ocr::row_profile(&img), scale);
        let lines = ocr::ScanCache::default().scan_at(&mut engine, &img, &bars, true, scale);
        assert_eq!(lines.len(), 4, "{label}: {lines:?}");
        assert!(lines[0].unfiltered.contains("unique"), "{label}: {:?}", lines[0]);
        let read: Vec<(String, Option<u32>)> = lines[1..]
            .iter()
            .map(|l| {
                let hits = match_rows(&vocab, std::slice::from_ref(&l.filtered), std::slice::from_ref(&l.unfiltered));
                let hit = hits.first().unwrap_or_else(|| panic!("{label}: no match for {l:?}"));
                (vocab.entry(hit.entry_index).to_string(), hit.count)
            })
            .collect();
        assert_eq!(
            read,
            vec![
                ("Greater Jeweller's Orb".to_string(), Some(1)),
                ("Cyclonic Alloy".to_string(), Some(1)),
                ("Exalted Orb".to_string(), Some(3)),
            ],
            "{label}"
        );
        // Whichever pass supplied a row's text, the row sits on its bar in
        // the UPSCALE coordinate space the overlay places labels in.
        for (line, &(y0, y1)) in lines.iter().zip(&bars) {
            let mid = line.y_top + line.height / 2;
            assert!(
                (y0 * ocr::UPSCALE..y1 * ocr::UPSCALE).contains(&mid),
                "{label}: line at {}+{} is off its bar ({y0},{y1})",
                line.y_top,
                line.height
            );
        }
    }
}

/// Rumour names are still recognised off a smaller client, and nothing is
/// recognised that is not there. Ground truth as in tests/rumours.rs.
#[cfg(ocr)]
#[test]
fn rumours_are_recognised_at_every_client_size() {
    use khaloni_poe2_core::rumour::{parse_csv, RumourIndex};
    let dir: std::path::PathBuf =
        [env!("CARGO_MANIFEST_DIR"), "tests", "fixtures", "rumours"].iter().collect();
    if !dir.exists() {
        eprintln!("SKIP: rumour fixtures absent at {}", dir.display());
        return;
    }
    let truth: [&[&str]; 5] = [
        &["Endless Cliffs"],
        &["Bleak and Awful", "Warm but risky", "Sulphite!"],
        &["Wild,.Roaming Free", "Cold as ice"],
        &["Cold as ice", "Wild,.Roaming Free"],
        &["Cold as ice", "Wild,.Roaming Free", "Warm but risky"],
    ];
    let index = RumourIndex::new(parse_csv(include_str!("../../core/tests/fixtures/rumours.csv")));
    let mut engine = ocr::OcrEngine::new().expect("tesseract");
    for (height, label) in SIZES {
        let (mut recalled, mut wrong) = (0, 0);
        for (n, expected) in truth.iter().enumerate() {
            let path = dir.join(format!("rumour-{}.png", n + 1));
            if !path.exists() {
                return;
            }
            let frame = downscaled(&image::open(&path).expect("open fixture").to_luma8(), height);
            let hits = khaloni_poe2::rumours::recognize(&mut engine, &frame, &index);
            recalled += hits.iter().filter(|h| expected.contains(&h.entry.rumour.as_str())).count();
            wrong += hits.iter().filter(|h| !expected.contains(&h.entry.rumour.as_str())).count();
        }
        eprintln!("{label}: recalled {recalled}/11, wrong {wrong}");
        assert_eq!(wrong, 0, "{label}: a rumour that is not on the tooltip");
        assert!(recalled >= 10, "{label}: recalled only {recalled}/11");
    }
}
