use khaloni_poe2::config::Rect;
use khaloni_poe2::coord::CoordMap;

#[test]
fn scale_comes_from_real_frame_width() {
    // Reference: window at logical (2560, 0) size 2560x1440, frame 3840x2160
    // (gamescope 4K capture of a 1440p-logical window -> scale 1.5). The
    // region arrives in capture pixels straight from the detector.
    let m = CoordMap::new(
        Rect { x: 2560, y: 0, w: 2560, h: 1440 },
        (3840, 2160),
        Rect { x: 60, y: 180, w: 1350, h: 1650 },
    );
    assert!((m.scale - 1.5).abs() < 1e-9);
    // Windows WGC delivers window-sized frames: scale 1.0.
    let m = CoordMap::new(
        Rect { x: 0, y: 0, w: 2560, h: 1440 },
        (2560, 1440),
        Rect { x: 40, y: 120, w: 900, h: 1100 },
    );
    assert!((m.scale - 1.0).abs() < 1e-9);
}

#[test]
fn maps_preprocessed_row_back_to_global_logical() {
    let m = CoordMap::new(
        Rect { x: 2560, y: 0, w: 2560, h: 1440 },
        (3840, 2160),
        Rect { x: 60, y: 180, w: 1350, h: 1650 },
    );
    // A row at y=300 in the 3x-upscaled OCR image is 100 px into the region,
    // capture y 280, logical y 186 -> label just right of the region.
    let (x, y) = m.label_pos_logical(300);
    assert_eq!(x, 2560 + ((60 + 1350) as f64 / 1.5) as i32 + 12);
    assert_eq!(y, 186);
}
