use crate::config::Rect;
use crate::ocr::UPSCALE;

/// Maps between the game window (global logical px), the capture frame
/// (capture px, whose size comes from the REAL frames — the Linux portal
/// negotiates its own size, Windows WGC delivers window-sized frames), and
/// the auto-detected reward-panel region (capture px, from
/// `autoregion::detect_reward_region`).
pub struct CoordMap {
    pub window_logical: Rect,
    pub capture: (u32, u32),
    /// Detected reward-panel region in capture pixels.
    pub region: Rect,
    /// Capture px per logical px.
    pub scale: f64,
}

impl CoordMap {
    pub fn new(window_logical: Rect, capture: (u32, u32), region: Rect) -> CoordMap {
        let scale = capture.0 as f64 / window_logical.w.max(1) as f64;
        CoordMap { window_logical, capture, region, scale }
    }

    /// Where a priced label for a row at `y_pre` (preprocessed px, i.e.
    /// region-relative capture y * UPSCALE) goes, in GLOBAL logical
    /// coordinates: right edge of the detected region + margin.
    pub fn label_pos_logical(&self, y_pre: u32) -> (i32, i32) {
        let capture_y = self.region.y + (y_pre / UPSCALE) as i32;
        (
            self.window_logical.x
                + ((self.region.x + self.region.w as i32) as f64 / self.scale) as i32
                + 12,
            self.window_logical.y + (capture_y as f64 / self.scale) as i32,
        )
    }

    /// Same as `label_pos_logical`, but for the vertical center of a row
    /// (`y_top + height / 2`) rather than its top edge, so the renderer can
    /// center the pill on the OCR row instead of hanging it off the top.
    pub fn label_pos_centered(&self, y_top: u32, height: u32) -> (i32, i32) {
        self.label_pos_logical(y_top + height / 2)
    }
}
