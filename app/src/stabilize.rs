use crate::pricing::Priced;

/// What one OCR pass produced: either priced rows, with the price service's
/// staleness flag, or a signal that the frame was gated before tesseract
/// ever ran (too dark to be the panel; see `Config::panel_min_brightness`).
pub enum ScanResult {
    Rows(Vec<Priced>, bool),
    GateEmpty,
}

/// Rows survive one miss so a hover tooltip briefly occluding the panel
/// doesn't flicker the overlay away; a second consecutive miss drops the row.
const MAX_MISSES: u8 = 1;
/// Grouping bucket for row identity: (label, y_top / BUCKET_PX).
const BUCKET_PX: u32 = 40;
/// A new y_top within this many preprocessed pixels of the tracked one is
/// treated as OCR jitter and ignored; the row keeps its old position.
const POSITION_HYSTERESIS_PX: u32 = 12;

struct Entry {
    label: String,
    bucket: u32,
    y_top: u32,
    height: u32,
    tier: crate::pricing::Tier,
    missed: u8,
}

/// Smooths a jittery stream of per-frame OCR scans into a stable set of rows.
/// Rows are identified by (label, y_top bucket); a row absent from a scan
/// survives one miss before being dropped, and small position wobble is
/// ignored so labels don't visibly twitch between frames.
#[derive(Default)]
pub struct Stabilizer {
    entries: Vec<Entry>,
    stale: bool,
}

impl Stabilizer {
    pub fn new() -> Stabilizer {
        Stabilizer::default()
    }

    /// Applies one scan result. A gate-empty result clears everything
    /// immediately, since it means the panel is physically not on screen;
    /// a rows result updates matching entries and ages out unmatched ones.
    pub fn apply(&mut self, result: ScanResult) {
        match result {
            ScanResult::GateEmpty => {
                self.entries.clear();
                self.stale = false;
            }
            ScanResult::Rows(rows, stale) => {
                self.stale = stale;
                self.update(rows);
            }
        }
    }

    fn update(&mut self, rows: Vec<Priced>) {
        let mut matched = vec![false; self.entries.len()];
        for row in rows {
            let bucket = row.y_top / BUCKET_PX;
            let existing = self
                .entries
                .iter()
                .position(|e| e.label == row.label && e.bucket == bucket)
                .filter(|&i| !matched[i]);
            if let Some(i) = existing {
                let e = &mut self.entries[i];
                if row.y_top.abs_diff(e.y_top) > POSITION_HYSTERESIS_PX {
                    e.y_top = row.y_top;
                    e.bucket = bucket;
                }
                e.height = row.height;
                e.tier = row.tier;
                e.missed = 0;
                matched[i] = true;
            } else {
                self.entries.push(Entry {
                    label: row.label,
                    bucket,
                    y_top: row.y_top,
                    height: row.height,
                    tier: row.tier,
                    missed: 0,
                });
                matched.push(true);
            }
        }

        let mut i = 0;
        while i < self.entries.len() {
            if matched[i] {
                i += 1;
                continue;
            }
            self.entries[i].missed += 1;
            if self.entries[i].missed > MAX_MISSES {
                self.entries.remove(i);
                matched.remove(i);
            } else {
                i += 1;
            }
        }
    }

    /// Current stabilized rows, top to bottom.
    pub fn rows(&self) -> Vec<Priced> {
        let mut out: Vec<Priced> = self
            .entries
            .iter()
            .map(|e| Priced {
                y_top: e.y_top,
                height: e.height,
                label: e.label.clone(),
                tier: e.tier,
            })
            .collect();
        out.sort_by_key(|r| r.y_top);
        out
    }

    pub fn stale(&self) -> bool {
        self.stale
    }

    /// Drops everything immediately (scan toggled off, game window gone).
    pub fn clear(&mut self) {
        self.entries.clear();
        self.stale = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pricing::Tier;

    fn row(label: &str, y_top: u32) -> Priced {
        Priced {
            y_top,
            height: 30,
            label: label.to_string(),
            tier: Tier::Decent,
        }
    }

    #[test]
    fn survives_one_miss() {
        let mut s = Stabilizer::new();
        s.apply(ScanResult::Rows(vec![row("3 ex", 100)], false));
        assert_eq!(s.rows().len(), 1);

        s.apply(ScanResult::Rows(vec![], false));
        assert_eq!(s.rows().len(), 1, "a single missed scan must not drop the row");
        assert_eq!(s.rows()[0].label, "3 ex");
    }

    #[test]
    fn dies_on_second_consecutive_miss() {
        let mut s = Stabilizer::new();
        s.apply(ScanResult::Rows(vec![row("3 ex", 100)], false));
        s.apply(ScanResult::Rows(vec![], false));
        assert_eq!(s.rows().len(), 1);

        s.apply(ScanResult::Rows(vec![], false));
        assert!(s.rows().is_empty(), "two consecutive misses must drop the row");
    }

    #[test]
    fn a_hit_between_misses_resets_the_miss_counter() {
        let mut s = Stabilizer::new();
        s.apply(ScanResult::Rows(vec![row("3 ex", 100)], false));
        s.apply(ScanResult::Rows(vec![], false)); // miss 1
        s.apply(ScanResult::Rows(vec![row("3 ex", 100)], false)); // seen again
        s.apply(ScanResult::Rows(vec![], false)); // miss 1 again, not a 2nd consecutive
        assert_eq!(s.rows().len(), 1, "a hit must reset the consecutive-miss count");
    }

    #[test]
    fn gate_empty_clears_immediately_even_after_a_single_hit() {
        let mut s = Stabilizer::new();
        s.apply(ScanResult::Rows(
            vec![row("3 ex", 100), row("1 div", 200)],
            false,
        ));
        assert_eq!(s.rows().len(), 2);

        s.apply(ScanResult::GateEmpty);
        assert!(s.rows().is_empty(), "gate-empty must clear all rows instantly, no miss grace period");
    }

    #[test]
    fn small_position_drift_is_ignored() {
        let mut s = Stabilizer::new();
        s.apply(ScanResult::Rows(vec![row("3 ex", 100)], false));
        s.apply(ScanResult::Rows(vec![row("3 ex", 108)], false)); // diff 8, same bucket
        assert_eq!(s.rows()[0].y_top, 100, "small jitter should not move the row");
    }

    #[test]
    fn large_position_drift_updates_the_row() {
        let mut s = Stabilizer::new();
        s.apply(ScanResult::Rows(vec![row("3 ex", 100)], false));
        s.apply(ScanResult::Rows(vec![row("3 ex", 119)], false)); // diff 19, same bucket
        assert_eq!(s.rows()[0].y_top, 119, "a real position change should move the row");
    }

    #[test]
    fn stale_flag_tracks_the_latest_rows_result() {
        let mut s = Stabilizer::new();
        s.apply(ScanResult::Rows(vec![row("3 ex", 100)], true));
        assert!(s.stale());
        s.apply(ScanResult::Rows(vec![row("3 ex", 100)], false));
        assert!(!s.stale());
    }
}
