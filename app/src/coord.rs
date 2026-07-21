use crate::config::Rect;
use crate::ocr::UPSCALE;

pub struct CoordMap {
    pub window_logical: Rect,
    pub capture: (u32, u32),
    pub calibration: Rect,
    pub scale: f64,
}

impl CoordMap {
    pub fn new(window_logical: Rect, capture: (u32, u32), calibration: Rect) -> CoordMap {
        let scale = capture.0 as f64 / window_logical.w as f64;
        CoordMap {
            window_logical,
            capture,
            calibration,
            scale,
        }
    }

    /// Calibrated region in capture (physical) pixels.
    pub fn region_px(&self) -> Rect {
        let lx = (self.calibration.x - self.window_logical.x) as f64;
        let ly = (self.calibration.y - self.window_logical.y) as f64;
        Rect {
            x: (lx * self.scale) as i32,
            y: (ly * self.scale) as i32,
            w: (self.calibration.w as f64 * self.scale) as u32,
            h: (self.calibration.h as f64 * self.scale) as u32,
        }
    }

    /// Where a priced label for a row at `y_pre` (preprocessed px) goes,
    /// in GLOBAL logical coordinates: right edge of the calibration rect + margin.
    pub fn label_pos_logical(&self, y_pre: u32) -> (i32, i32) {
        let region_y = y_pre / UPSCALE;
        let logical_y = (region_y as f64 / self.scale) as i32;
        (
            self.calibration.x + self.calibration.w as i32 + 12,
            self.calibration.y + logical_y,
        )
    }
}
