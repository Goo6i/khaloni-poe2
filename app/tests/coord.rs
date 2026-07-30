use khaloni_poe2::config::Rect;
use khaloni_poe2::coord::CoordMap;

#[test]
fn maps_logical_calibration_to_capture_pixels() {
    // Reference machine: window at logical (2560, 0) size 2560x1440, capture 3840x2160.
    let m = CoordMap::new(
        Rect { x: 2560, y: 0, w: 2560, h: 1440 },
        (3840, 2160),
        Rect { x: 2600, y: 120, w: 900, h: 1100 },
    );
    assert!((m.scale - 1.5).abs() < 1e-9);
    let r = m.region_px();
    assert_eq!((r.x, r.y, r.w, r.h), (60, 180, 1350, 1650));
}

#[test]
fn maps_preprocessed_row_back_to_global_logical() {
    let m = CoordMap::new(
        Rect { x: 2560, y: 0, w: 2560, h: 1440 },
        (3840, 2160),
        Rect { x: 2600, y: 120, w: 900, h: 1100 },
    );
    // A row at y=300 in the 3x-upscaled OCR image is at 100 px in region space,
    // 280 px in capture space, 186.67 logical -> global y 306 (rounded down + cal y offset).
    let (x, y) = m.label_pos_logical(300);
    // Label goes just right of the calibrated region.
    assert_eq!(x, 2600 + 900 + 12);
    assert_eq!(y, 120 + 66);
}
