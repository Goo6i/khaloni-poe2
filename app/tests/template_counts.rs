//! A learned template must never decide a stack count. Whole-strip
//! correlation cannot see one changed digit; these tests edit the count on
//! a real band and hold the template store to a miss.

use image::{GrayImage, Luma};
use khaloni_poe2::ocr;
use khaloni_poe2::template::{TemplateStore, BLOCK_NCC_MIN, NCC_THRESHOLD};

/// The four band crops of the choice-panel fixture, top to bottom:
/// "Unique Jewellery", "1x Greater Jeweller's Orb", "1x Cyclonic Alloy",
/// "3x Exalted Orb".
fn crops() -> Vec<GrayImage> {
    let img = image::load_from_memory(include_bytes!("fixtures/panel_choice.png")).unwrap().to_luma8();
    let bars = ocr::reward_bars(&img, &ocr::row_profile(&img));
    bars.iter().map(|&(y0, y1)| ocr::band_crop(&img, y0, y1).unwrap()).collect()
}

// Glyph columns measured on the fixture crops: the "3" of "3x Exalted Orb"
// and the "1" of "1x Cyclonic Alloy", with blank bar on either side.
const THREE: (u32, u32) = (449, 462);
const ONE: (u32, u32) = (410, 418);
/// A glyph-free column of the "3x" strip, used as bar texture to paint over ink.
const BLANK_COL: u32 = 440;

fn erase(img: &mut GrayImage, (x0, x1): (u32, u32)) {
    for x in x0..x1 {
        for y in 0..img.height() {
            let p = img.get_pixel(BLANK_COL - (x - x0) % 6, y)[0];
            img.put_pixel(x, y, Luma([p]));
        }
    }
}

fn paste(img: &mut GrayImage, from: &GrayImage, (sx0, sx1): (u32, u32), at: u32) {
    for x in sx0..sx1 {
        for y in 0..img.height().min(from.height()) {
            img.put_pixel(at + (x - sx0), y, *from.get_pixel(x, y));
        }
    }
}

fn three_exalted() -> GrayImage {
    crops().remove(3)
}

/// "3x" with its digit replaced by the fixture's own "1" glyph.
fn one_exalted() -> GrayImage {
    let all = crops();
    let mut img = all[3].clone();
    erase(&mut img, THREE);
    paste(&mut img, &all[2], ONE, THREE.0 + 3);
    img
}

/// The nearest look-alike: an "8" made by closing the "3" with its own
/// mirror image, every stroke of the 3 still in place.
fn eight_exalted() -> GrayImage {
    let src = three_exalted();
    let mut img = src.clone();
    for x in THREE.0..THREE.1 {
        let mirror = THREE.1 - 1 - (x - THREE.0);
        for y in 0..img.height() {
            let p = src.get_pixel(x, y)[0].min(src.get_pixel(mirror, y)[0]);
            img.put_pixel(x, y, Luma([p]));
        }
    }
    img
}

/// "3x" with the digit painted out.
fn no_digit() -> GrayImage {
    let mut img = three_exalted();
    erase(&mut img, THREE);
    img
}

/// "13x": the fixture's "1" glyph set in front of the "3".
fn thirteen_exalted() -> GrayImage {
    let all = crops();
    let mut img = all[3].clone();
    paste(&mut img, &all[2], ONE, THREE.0 - 12);
    img
}

fn relit(img: &GrayImage, gain: f32, offset: f32) -> GrayImage {
    GrayImage::from_fn(img.width(), img.height(), |x, y| {
        Luma([(f32::from(img.get_pixel(x, y)[0]) * gain + offset).clamp(0.0, 255.0) as u8])
    })
}

fn store_with_three() -> TemplateStore {
    let mut store = TemplateStore::new();
    store.learn("exalted orb", 3, true, &three_exalted());
    store
}

#[test]
fn whole_strip_correlation_cannot_see_a_count_but_the_glyph_windows_can() {
    let store = store_with_three();
    for (what, img) in [("1x", one_exalted()), ("8x", eight_exalted()), ("no digit", no_digit()), ("13x", thirteen_exalted())] {
        let (whole, worst) = store.scores(&img).expect("same height");
        eprintln!("{what}: whole {whole:.4} worst window {worst:.4}");
        assert!(whole >= NCC_THRESHOLD, "{what}: the premise - whole-strip NCC {whole:.3} passes");
        assert!(worst < BLOCK_NCC_MIN - 0.12, "{what}: worst window {worst:.3} is not clear of the bar");
    }
    for (what, img) in [
        ("darker", relit(&three_exalted(), 1.0, -18.0)),
        ("brighter", relit(&three_exalted(), 1.0, 12.0)),
        ("dimmed", relit(&three_exalted(), 0.85, 0.0)),
    ] {
        let (whole, worst) = store.scores(&img).expect("same height");
        eprintln!("{what}: whole {whole:.4} worst window {worst:.4}");
        assert!(worst > BLOCK_NCC_MIN + 0.04, "{what}: worst window {worst:.3} is too near the bar");
    }
}

#[test]
fn a_template_never_answers_for_a_different_count() {
    let store = store_with_three();
    let hit = store.lookup(&three_exalted()).expect("the learned strip itself");
    assert_eq!((hit.item_key.as_str(), hit.count), ("exalted orb", 3));
    assert!(store.lookup(&relit(&three_exalted(), 1.0, -18.0)).is_some(), "relit strip still matches");
    for (what, img) in [("1x", one_exalted()), ("8x", eight_exalted()), ("no digit", no_digit()), ("13x", thirteen_exalted())] {
        assert!(store.lookup(&img).is_none(), "{what} matched the 3x template");
        assert!(store.match_band(&img).is_none(), "{what} matched the 3x template");
    }
}

#[test]
fn the_other_rows_of_the_panel_still_tell_each_other_apart() {
    let all = crops();
    let mut store = TemplateStore::new();
    for (i, (key, count)) in [("unique", 1), ("greater jeweller s orb", 1), ("cyclonic alloy", 1), ("exalted orb", 3)]
        .iter()
        .enumerate()
    {
        store.learn(key, *count, i > 0, &all[i]);
    }
    for (i, key) in ["unique", "greater jeweller s orb", "cyclonic alloy", "exalted orb"].iter().enumerate() {
        assert_eq!(store.lookup(&all[i]).expect("own strip").item_key, *key);
    }
}

#[test]
fn a_hit_owes_an_ocr_check_until_that_exact_crop_is_confirmed() {
    let dir = std::env::temp_dir().join(format!("khalonipoe2-tpl-verify-{}", std::process::id()));
    let path = dir.join("templates.bin");
    let mut store = store_with_three();
    // OCR taught this strip, so this content is already confirmed.
    assert!(store.lookup(&three_exalted()).unwrap().verify.is_none());
    // The same reward under other lighting is new content: shown, but owed.
    let dark = relit(&three_exalted(), 1.0, -18.0);
    let ticket = store.lookup(&dark).unwrap().verify.expect("unconfirmed content owes a check");
    assert!(store.confirm(ticket, None), "no OCR row: nothing learned either way");
    assert!(store.lookup(&dark).unwrap().verify.is_some(), "still owed");
    assert!(store.confirm(ticket, Some(("exalted orb", 3))));
    assert!(store.lookup(&dark).unwrap().verify.is_none(), "confirmed content is trusted");

    // A store from disk has confirmed nothing in this session.
    store.save(&path).unwrap();
    let mut loaded = TemplateStore::load(&path);
    let ticket = loaded.lookup(&three_exalted()).unwrap().verify.expect("loaded templates start unconfirmed");
    // OCR disagrees about the count: the template goes.
    assert!(!loaded.confirm(ticket, Some(("exalted orb", 8))));
    assert!(loaded.lookup(&three_exalted()).is_none(), "a contradicted template is removed");
    assert!(loaded.dirty, "and the removal reaches the file");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_store_written_by_the_previous_format_is_discarded() {
    let dir = std::env::temp_dir().join(format!("khalonipoe2-tpl-old-{}", std::process::id()));
    let path = dir.join("templates.bin");
    let mut store = store_with_three();
    store.save(&path).unwrap();
    let mut bytes = std::fs::read(&path).unwrap();
    assert_eq!(&bytes[..8], b"P2LTPL02");
    bytes[..8].copy_from_slice(b"P2LTPL01");
    std::fs::write(&path, bytes).unwrap();
    assert!(TemplateStore::load(&path).is_empty(), "a P2LTPL01 store may hold strips matched at the wrong count");
    let _ = std::fs::remove_dir_all(dir);
}
