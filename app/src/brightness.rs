/// Asymmetric hysteresis over consecutive frame brightness means, standing
/// in for the single `panel_min_brightness` threshold the OCR worker used
/// to check directly. A single noisy frame near the boundary can no longer
/// flap the gate: it opens only after `open_after` consecutive means
/// strictly above `open_at`, and closes only after `close_after`
/// consecutive means strictly below `close_at`. Between the two thresholds
/// is a dead zone: a frame landing there holds whatever state the gate is
/// already in and resets both streak counters (so an almost-there run of
/// above-threshold frames doesn't survive a single dead-zone dip and get
/// silently credited toward opening later).
pub struct BrightnessGate {
    open_at: u8,
    close_at: u8,
    open_after: u8,
    close_after: u8,
    is_open: bool,
    above_streak: u8,
    below_streak: u8,
}

impl BrightnessGate {
    /// `open_at` is the panel-parchment threshold (mean brightness must
    /// exceed this to start opening); `close_at` is the game-world
    /// threshold (mean brightness must fall below this to start closing).
    /// Reference behavior: opens after 2 consecutive frames above
    /// `open_at`, closes after 3 consecutive frames below `close_at`.
    pub fn new(open_at: u8, close_at: u8) -> BrightnessGate {
        BrightnessGate {
            open_at,
            close_at,
            open_after: 2,
            close_after: 3,
            is_open: false,
            above_streak: 0,
            below_streak: 0,
        }
    }

    /// Feeds one frame's mean brightness (0-255 scale) and returns the
    /// gate's state after observing it.
    pub fn observe(&mut self, mean: u64) -> bool {
        if mean > u64::from(self.open_at) {
            self.above_streak += 1;
            self.below_streak = 0;
        } else if mean < u64::from(self.close_at) {
            self.below_streak += 1;
            self.above_streak = 0;
        } else {
            // Dead zone: doesn't move the gate, and doesn't let partial
            // streaks survive across it either.
            self.above_streak = 0;
            self.below_streak = 0;
        }

        if !self.is_open && self.above_streak >= self.open_after {
            self.is_open = true;
        }
        if self.is_open && self.below_streak >= self.close_after {
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
    fn starts_closed() {
        let gate = BrightnessGate::new(100, 80);
        assert!(!gate.is_open());
    }

    #[test]
    fn a_single_bright_frame_does_not_open_it() {
        let mut gate = BrightnessGate::new(100, 80);
        assert!(!gate.observe(200));
    }

    #[test]
    fn opens_after_two_consecutive_bright_frames() {
        let mut gate = BrightnessGate::new(100, 80);
        assert!(!gate.observe(200));
        assert!(gate.observe(200));
    }

    #[test]
    fn a_dead_zone_frame_resets_the_opening_streak() {
        let mut gate = BrightnessGate::new(100, 80);
        assert!(!gate.observe(200)); // 1 above
        assert!(!gate.observe(90)); // dead zone: resets
        assert!(!gate.observe(200)); // 1 above again, not 2
        assert!(gate.observe(200)); // now 2 consecutive: opens
    }

    #[test]
    fn stays_open_through_a_single_dim_frame() {
        let mut gate = BrightnessGate::new(100, 80);
        gate.observe(200);
        gate.observe(200);
        assert!(gate.is_open());
        assert!(gate.observe(50), "a single dim frame must not close it");
        assert!(gate.observe(50), "a second consecutive dim frame must still not close it");
    }

    #[test]
    fn closes_after_three_consecutive_dim_frames() {
        let mut gate = BrightnessGate::new(100, 80);
        gate.observe(200);
        gate.observe(200);
        assert!(gate.is_open());
        gate.observe(50);
        gate.observe(50);
        assert!(!gate.observe(50), "the 3rd consecutive dim frame must close the gate");
    }

    #[test]
    fn a_dead_zone_frame_resets_the_closing_streak_and_the_gate_stays_open() {
        let mut gate = BrightnessGate::new(100, 80);
        gate.observe(200);
        gate.observe(200);
        assert!(gate.is_open());
        gate.observe(50); // dim 1
        gate.observe(50); // dim 2
        assert!(gate.observe(90), "dead zone must hold state, not close");
        assert!(gate.observe(50), "dim 1 again after the reset");
        assert!(gate.observe(50), "dim 2 again");
        assert!(!gate.observe(50), "dim 3: now it actually closes");
    }

    #[test]
    fn dead_zone_holds_a_closed_gate_closed() {
        let mut gate = BrightnessGate::new(100, 80);
        assert!(!gate.observe(90));
        assert!(!gate.observe(90));
        assert!(!gate.observe(90));
    }
}
