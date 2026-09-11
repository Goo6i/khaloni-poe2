//! Whether a reward panel is on screen, judged frame by frame from the
//! presence of signature reward bars (see `ocr::reward_bars`) with
//! hysteresis, so one odd frame cannot flap the scanner on and off.
//!
//! This replaces a mean-brightness gate. Brightness told parchment from
//! the dark game world, and nothing else: on a bright map the scan
//! region read well above its threshold with no panel there, tesseract
//! ran on terrain every 120 ms, and the garbage it read became labels.
//! Bars cannot be faked by terrain (tests/bars.rs), so presence is the
//! signal, and the two thresholds the user could mis-set are gone.

/// Opens after `OPEN_AFTER` consecutive frames with a bar, closes after
/// `CLOSE_AFTER` consecutive frames without one. Reference behaviour of
/// the gate it replaces: 2 to open, 3 to close.
pub struct PanelGate {
    is_open: bool,
    with_streak: u8,
    without_streak: u8,
}

const OPEN_AFTER: u8 = 2;
const CLOSE_AFTER: u8 = 3;

impl Default for PanelGate {
    fn default() -> PanelGate {
        PanelGate::new()
    }
}

impl PanelGate {
    pub fn new() -> PanelGate {
        PanelGate { is_open: false, with_streak: 0, without_streak: 0 }
    }

    /// Feeds one still frame's verdict and returns the gate's state after
    /// it. Callers skip mid-scroll frames (blurred edges fail the bar
    /// signature) and read `is_open` instead.
    pub fn observe(&mut self, bars_present: bool) -> bool {
        if bars_present {
            self.with_streak = self.with_streak.saturating_add(1);
            self.without_streak = 0;
        } else {
            self.without_streak = self.without_streak.saturating_add(1);
            self.with_streak = 0;
        }
        if !self.is_open && self.with_streak >= OPEN_AFTER {
            self.is_open = true;
        }
        if self.is_open && self.without_streak >= CLOSE_AFTER {
            self.is_open = false;
        }
        self.is_open
    }

    pub fn is_open(&self) -> bool {
        self.is_open
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_closed_and_needs_two_frames_to_open() {
        let mut g = PanelGate::new();
        assert!(!g.is_open());
        assert!(!g.observe(true));
        assert!(g.observe(true));
    }

    #[test]
    fn one_barless_frame_does_not_close_it() {
        let mut g = PanelGate::new();
        g.observe(true);
        g.observe(true);
        assert!(g.observe(false));
        assert!(g.observe(false));
        assert!(!g.observe(false), "third consecutive miss closes");
    }

    #[test]
    fn a_bar_frame_resets_the_closing_streak() {
        let mut g = PanelGate::new();
        g.observe(true);
        g.observe(true);
        g.observe(false);
        g.observe(false);
        assert!(g.observe(true));
        assert!(g.observe(false));
        assert!(g.observe(false), "streak restarted after the bar frame");
    }

    #[test]
    fn a_lone_bar_frame_does_not_open_it() {
        let mut g = PanelGate::new();
        assert!(!g.observe(true));
        assert!(!g.observe(false));
        assert!(!g.observe(true), "streak broken, still one frame");
    }
}
