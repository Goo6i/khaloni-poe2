use std::time::{Duration, Instant};

use crate::pricing::{Denom, Priced, Tier};

/// What one OCR pass produced: priced rows (with the price service's
/// staleness flag); a signal that the brightness gate was closed, so
/// tesseract never even ran (too dark to be the panel; see the brightness
/// hysteresis in main.rs); or a signal that the gate was open (bright
/// enough to plausibly be the panel) but band detection found no reward
/// bars, so tesseract wasn't run against nothing (see ocr::detect_bands).
pub enum ScanResult {
    Rows(Vec<Priced>, bool),
    NoBands,
    GateEmpty,
    /// The panel content shifted vertically by this many PREPROCESSED
    /// pixels (optical scroll estimate between consecutive frames; the
    /// caller converts capture-space dy through UPSCALE). Slots move
    /// instantly; no confirmation/miss bookkeeping is touched, so a scan
    /// landing after the scroll matches the shifted slots in place.
    Scrolled(i64),
}

// --- Slot-model constants, ported from the reference overlay's MergeReads
// state machine. The reference operates on ~1x screen-space pixels; our OCR
// runs on preprocessed images that are scaled ~4.5x versus raw screen
// pixels (see ocr::UPSCALE and the capture-to-window scale folded into
// coord::CoordMap), so every pixel constant below is the reference value
// times 4.5, documented at each site.

/// Y-tolerance for matching an incoming read to an existing slot at all:
/// reference Tolerance = 20px * 4.5 = 90 preprocessed px.
const Y_MATCH_TOLERANCE_PX: u32 = 90;
/// Once a read is matched to a slot, a Y within this distance of the slot's
/// locked Y is jitter and ignored (position smoothing): reference 5px * 4.5
/// = 22 preprocessed px. Strictly tighter than Y_MATCH_TOLERANCE_PX, which
/// only decides whether a read belongs to a slot in the first place; a
/// drift bigger than this but still inside the match tolerance is treated
/// as a real (small) position change and the slot follows it.
const POSITION_SNAP_PX: u32 = 22;
/// A plain Fuzzy-tier read needs this many consecutive identical reads
/// (same item_key) before a slot displays it for the first time.
const CONFIRM_FUZZY: u8 = 2;
/// An already-displayed slot needs this many consecutive reads of the SAME
/// different item before it switches its display away from what it's
/// currently showing.
const PENDING_SWITCH: u8 = 2;
/// Consecutive scans with no matching read at all before a slot is evicted.
const EVICT_AFTER: u8 = 8;
/// Consecutive zero-row Rows scans before the stabilized display is hidden;
/// slots are kept alive underneath for fast recovery (see STALE_CLEAR_AFTER).
const STALE_HIDE_AFTER: u32 = 8;
/// Consecutive zero-row Rows scans before slots are dropped for good.
const STALE_CLEAR_AFTER: u32 = 12;
/// Consecutive NoBands scans (gate open, but band detection found nothing)
/// before the stabilized display hides. Shorter than STALE_HIDE_AFTER since
/// NoBands is a stronger signal that the panel itself is gone (band
/// detection ran and found no reward bars at all) rather than a transient
/// OCR/matching miss on a panel that's still there; ~1.2s at the 120ms
/// panel-open capture throttle.
const NOBANDS_HIDE_AFTER: u32 = 2;
/// Consecutive NoBands scans before slots are dropped for good.
const NOBANDS_CLEAR_AFTER: u32 = 4;
/// How long a slot remembers its last explicit "Nx" reading for stack-count
/// stickiness.
const STACK_STICKY: Duration = Duration::from_millis(1500);

/// A snapshot of everything about a row that isn't its position: what gets
/// displayed once a slot resolves what to show for a given read.
#[derive(Clone)]
struct Snapshot {
    label: String,
    amount: String,
    denom: Denom,
    tier: Tier,
}

impl Snapshot {
    fn from_row(row: &Priced) -> Snapshot {
        Snapshot {
            label: row.label.clone(),
            amount: row.amount.clone(),
            denom: row.denom,
            tier: row.tier,
        }
    }
}

/// A candidate item competing to replace what an already-displayed slot is
/// showing; needs PENDING_SWITCH consecutive reads of the same item before
/// it wins.
struct PendingSwitch {
    item_key: String,
    row: Priced,
    count: u8,
}

/// A single stabilized display position: a fixed row on the panel, keyed by
/// Y rather than by item name, since the item occupying a position can
/// change (a re-rolled reward) without the position itself moving.
struct Slot {
    y: u32,
    height: u32,
    /// The item currently tracked: either on display, or the candidate
    /// awaiting its CONFIRM_FUZZY-th matching read before first display.
    item_key: String,
    displayed: bool,
    snap: Snapshot,
    /// Consecutive matching reads seen so far while `!displayed`.
    confirm: u8,
    /// A different item competing to replace `item_key` once `displayed`.
    pending: Option<PendingSwitch>,
    /// Consecutive scans with no read matching this slot at all.
    misses: u8,
    /// The last read that had an explicit "Nx" count, and when: honored for
    /// STACK_STICKY after the marker itself drops out of a matching read.
    last_explicit: Option<(Snapshot, Instant)>,
}

impl Slot {
    fn new(row: Priced) -> Slot {
        let mut slot = Slot {
            y: row.y_top,
            height: row.height,
            item_key: String::new(),
            displayed: false,
            snap: Snapshot::from_row(&row),
            confirm: 0,
            pending: None,
            misses: 0,
            last_explicit: None,
        };
        slot.establish_pre_display(&row);
        slot
    }

    fn touch_position(&mut self, row: &Priced) {
        if row.y_top.abs_diff(self.y) > POSITION_SNAP_PX {
            self.y = row.y_top;
        }
        self.height = row.height;
    }

    /// Resolves what to show for a read of the item this slot is ALREADY
    /// displaying: explicit-read > locked (keep exactly what's shown) - no
    /// remembered/default fallback is needed here, since something is
    /// already on screen.
    fn resolve_established(&mut self, row: &Priced) -> Snapshot {
        if row.count_explicit {
            let snap = Snapshot::from_row(row);
            self.last_explicit = Some((snap.clone(), Instant::now()));
            return snap;
        }
        self.snap.clone()
    }

    /// Resolves what to show for a read establishing a NEW tracked identity
    /// (first display, or an item switch committing): explicit-read >
    /// remembered (<1500ms) > the read's own (implicit) amount. There is no
    /// "locked" fallback here, since nothing of this identity has been
    /// shown before.
    fn resolve_new(&mut self, row: &Priced) -> Snapshot {
        if row.count_explicit {
            let snap = Snapshot::from_row(row);
            self.last_explicit = Some((snap.clone(), Instant::now()));
            return snap;
        }
        if let Some((snap, when)) = &self.last_explicit {
            if when.elapsed() < STACK_STICKY {
                return snap.clone();
            }
        }
        Snapshot::from_row(row)
    }

    /// (Re)targets this slot at `row`'s item before it has been displayed
    /// yet: a confident read locks in on the spot; a Fuzzy one starts (or
    /// restarts) the CONFIRM_FUZZY-read confirmation count.
    fn establish_pre_display(&mut self, row: &Priced) {
        self.item_key = row.item_key.clone();
        self.pending = None;
        if row.locks_in_one {
            self.snap = self.resolve_new(row);
            self.displayed = true;
            self.confirm = 0;
        } else {
            self.displayed = false;
            self.confirm = 1;
            if row.count_explicit {
                self.last_explicit = Some((Snapshot::from_row(row), Instant::now()));
            }
        }
    }

    /// Commits a pending switch: the PENDING_SWITCH consecutive agreeing
    /// reads that triggered it are themselves the confirmation, so the new
    /// item displays immediately regardless of its own match tier.
    fn commit_switch(&mut self, row: &Priced) {
        self.item_key = row.item_key.clone();
        self.snap = self.resolve_new(row);
        self.displayed = true;
        self.pending = None;
        self.confirm = 0;
    }
}

/// Applies one matched read to a slot. A miss (no read at all this scan) is
/// handled by the caller and never reaches this function, so "no
/// information" naturally never touches confirm/pending/displayed/snap.
fn apply_read(slot: &mut Slot, row: Priced) {
    slot.misses = 0;
    slot.touch_position(&row);

    if row.item_key == slot.item_key {
        // Same item as tracked: a same-item read cancels any switch
        // attempt in progress, and either bumps confirmation or refreshes
        // the locked display.
        slot.pending = None;
        if slot.displayed {
            slot.snap = slot.resolve_established(&row);
        } else if row.locks_in_one {
            slot.snap = slot.resolve_new(&row);
            slot.displayed = true;
            slot.confirm = 0;
        } else {
            slot.confirm = slot.confirm.saturating_add(1);
            if row.count_explicit {
                slot.last_explicit = Some((Snapshot::from_row(&row), Instant::now()));
            }
            if slot.confirm >= CONFIRM_FUZZY {
                slot.snap = slot.resolve_new(&row);
                slot.displayed = true;
            }
        }
        return;
    }

    // A different item than currently tracked.
    if !slot.displayed {
        // Nothing displayed yet: a different candidate simply restarts
        // confirmation on itself rather than contributing to the old one.
        slot.establish_pre_display(&row);
        return;
    }

    // Already displaying something real: needs PENDING_SWITCH consecutive
    // reads of the SAME candidate item before switching away from it.
    match &mut slot.pending {
        Some(p) if p.item_key == row.item_key => {
            p.count += 1;
            p.row = row;
        }
        _ => {
            slot.pending = Some(PendingSwitch { item_key: row.item_key.clone(), row, count: 1 });
        }
    }
    if slot.pending.as_ref().is_some_and(|p| p.count >= PENDING_SWITCH) {
        let new_row = slot.pending.take().expect("just checked Some").row;
        slot.commit_switch(&new_row);
    }
}

/// Smooths a jittery stream of per-frame OCR scans into a stable set of
/// display rows, one per fixed panel position. Ported from the reference
/// overlay's MergeReads slot model (see the module-level constants for the
/// exact behaviors and their reference-to-preprocessed-pixel conversions).
#[derive(Default)]
pub struct Stabilizer {
    slots: Vec<Slot>,
    stale: bool,
    /// Consecutive Rows(_, _) scans in a row whose row list was empty (the
    /// two-stage stale mechanism; see STALE_HIDE_AFTER / STALE_CLEAR_AFTER).
    empty_streak: u32,
    /// Consecutive NoBands scans in a row (see NOBANDS_HIDE_AFTER /
    /// NOBANDS_CLEAR_AFTER), tracked separately from empty_streak: NoBands
    /// means band detection itself found nothing, a stronger "the panel is
    /// probably gone" signal than a Rows scan that ran OCR and matched
    /// nothing.
    nobands_streak: u32,
}

impl Stabilizer {
    pub fn new() -> Stabilizer {
        Stabilizer::default()
    }

    /// Applies one scan result. GateEmpty (the brightness gate closed, so
    /// tesseract never even ran) hides and clears immediately, since it
    /// means the panel is physically not on screen. NoBands (gate open, but
    /// no reward bars found) hides after NOBANDS_HIDE_AFTER consecutive
    /// occurrences and clears after NOBANDS_CLEAR_AFTER. A Rows result with
    /// 0 rows (bands existed, but OCR or matching yielded nothing) only
    /// hides after STALE_HIDE_AFTER consecutive empties (slots are kept
    /// alive for fast recovery) and only clears after STALE_CLEAR_AFTER.
    pub fn apply(&mut self, result: ScanResult) {
        match result {
            ScanResult::GateEmpty => {
                self.slots.clear();
                self.stale = false;
                self.empty_streak = 0;
                self.nobands_streak = 0;
            }
            ScanResult::NoBands => {
                self.nobands_streak = self.nobands_streak.saturating_add(1);
                if self.nobands_streak >= NOBANDS_CLEAR_AFTER {
                    self.slots.clear();
                }
            }
            ScanResult::Scrolled(dy) => {
                // Instant translation; slots scrolled above the region top
                // are dropped (they re-enter via OCR if scrolled back).
                self.slots.retain_mut(|slot| {
                    let ny = i64::from(slot.y) + dy;
                    if ny < -i64::from(slot.height) {
                        return false;
                    }
                    slot.y = ny.max(0) as u32;
                    true
                });
            }
            ScanResult::Rows(rows, stale) => {
                self.nobands_streak = 0;
                self.stale = stale;
                if rows.is_empty() {
                    self.empty_streak = self.empty_streak.saturating_add(1);
                } else {
                    self.empty_streak = 0;
                }
                if self.empty_streak >= STALE_CLEAR_AFTER {
                    self.slots.clear();
                } else {
                    self.update(rows);
                }
            }
        }
    }

    fn update(&mut self, rows: Vec<Priced>) {
        let mut matched = vec![false; self.slots.len()];
        for row in rows {
            let existing = self
                .slots
                .iter()
                .enumerate()
                .filter(|(i, _)| !matched[*i])
                .filter(|(_, s)| s.y.abs_diff(row.y_top) <= Y_MATCH_TOLERANCE_PX)
                .min_by_key(|(_, s)| s.y.abs_diff(row.y_top))
                .map(|(i, _)| i);
            match existing {
                Some(i) => {
                    matched[i] = true;
                    apply_read(&mut self.slots[i], row);
                }
                None => {
                    self.slots.push(Slot::new(row));
                    matched.push(true);
                }
            }
        }

        let mut i = 0;
        while i < self.slots.len() {
            if matched[i] {
                i += 1;
                continue;
            }
            self.slots[i].misses += 1;
            if self.slots[i].misses >= EVICT_AFTER {
                self.slots.remove(i);
                matched.remove(i);
            } else {
                i += 1;
            }
        }
    }

    /// Current stabilized rows, top to bottom. Hidden (empty) while the
    /// two-stage stale hide is active, even though slots survive
    /// underneath; `count_explicit`/`locks_in_one` are meaningless on a
    /// rendered snapshot and are stubbed since nothing downstream reads
    /// them.
    pub fn rows(&self) -> Vec<Priced> {
        if self.empty_streak >= STALE_HIDE_AFTER || self.nobands_streak >= NOBANDS_HIDE_AFTER {
            return Vec::new();
        }
        let mut out: Vec<Priced> = self
            .slots
            .iter()
            .filter(|s| s.displayed)
            .map(|s| Priced {
                y_top: s.y,
                height: s.height,
                label: s.snap.label.clone(),
                amount: s.snap.amount.clone(),
                denom: s.snap.denom,
                tier: s.snap.tier,
                item_key: s.item_key.clone(),
                count_explicit: false,
                locks_in_one: true,
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
        self.slots.clear();
        self.stale = false;
        self.empty_streak = 0;
        self.nobands_streak = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(item_key: &str, amount: &str, y_top: u32, locks_in_one: bool, count_explicit: bool) -> Priced {
        Priced {
            y_top,
            height: 30,
            label: amount.to_string(),
            amount: amount.to_string(),
            denom: Denom::Exalted,
            tier: Tier::Decent,
            item_key: item_key.to_string(),
            count_explicit,
            locks_in_one,
        }
    }

    fn exact(item_key: &str, amount: &str, y_top: u32) -> Priced {
        row(item_key, amount, y_top, true, false)
    }

    fn fuzzy(item_key: &str, amount: &str, y_top: u32) -> Priced {
        row(item_key, amount, y_top, false, false)
    }

    #[test]
    fn lock_in_one_for_exact_tier_read() {
        let mut s = Stabilizer::new();
        s.apply(ScanResult::Rows(vec![exact("a", "3 ex", 100)], false));
        assert_eq!(s.rows().len(), 1, "a locks_in_one read must display after a single scan");
        assert_eq!(s.rows()[0].item_key, "a");
    }

    #[test]
    fn fuzzy_read_needs_two_consecutive_identical_reads_to_display() {
        let mut s = Stabilizer::new();
        s.apply(ScanResult::Rows(vec![fuzzy("a", "3 ex", 100)], false));
        assert!(s.rows().is_empty(), "a single Fuzzy read must not display yet");

        s.apply(ScanResult::Rows(vec![fuzzy("a", "3 ex", 100)], false));
        assert_eq!(s.rows().len(), 1, "a second consecutive identical Fuzzy read must confirm and display");
    }

    #[test]
    fn switch_after_two_consecutive_different_reads() {
        let mut s = Stabilizer::new();
        s.apply(ScanResult::Rows(vec![exact("a", "3 ex", 100)], false));
        assert_eq!(s.rows()[0].item_key, "a");

        // First read of a different item: not enough to switch yet.
        s.apply(ScanResult::Rows(vec![exact("b", "1 ex", 100)], false));
        assert_eq!(s.rows()[0].item_key, "a", "a single different read must not switch the slot");

        // Second consecutive read of the SAME different item: switches.
        s.apply(ScanResult::Rows(vec![exact("b", "1 ex", 100)], false));
        assert_eq!(s.rows()[0].item_key, "b", "two consecutive different reads must switch the slot");
    }

    #[test]
    fn miss_preserves_the_lock_and_does_not_reset_a_pending_switch() {
        let mut s = Stabilizer::new();
        s.apply(ScanResult::Rows(vec![exact("a", "3 ex", 100)], false));
        // First of two switch-reads for "b".
        s.apply(ScanResult::Rows(vec![exact("b", "1 ex", 100)], false));
        assert_eq!(s.rows()[0].item_key, "a");

        // A miss for the y=100 slot: some unrelated row elsewhere, so this
        // is a per-slot miss, not a global empty scan.
        s.apply(ScanResult::Rows(vec![exact("unrelated", "9 ex", 500)], false));
        assert_eq!(
            s.rows().iter().find(|r| r.y_top == 100).unwrap().item_key,
            "a",
            "a miss must not drop the existing lock"
        );

        // Second consecutive "b" read: the miss in between must not have
        // reset the pending-switch streak, so this completes the switch.
        s.apply(ScanResult::Rows(vec![exact("b", "1 ex", 100)], false));
        assert_eq!(
            s.rows().iter().find(|r| r.y_top == 100).unwrap().item_key,
            "b",
            "a miss must not reset the pending-switch counter"
        );
    }

    #[test]
    fn evict_after_eight_consecutive_misses() {
        let mut s = Stabilizer::new();
        s.apply(ScanResult::Rows(vec![exact("a", "3 ex", 100)], false));
        assert!(s.rows().iter().any(|r| r.item_key == "a"));

        for _ in 0..7 {
            s.apply(ScanResult::Rows(vec![exact("other", "1 ex", 500)], false));
        }
        assert!(
            s.rows().iter().any(|r| r.item_key == "a"),
            "a slot must survive fewer than EVICT_AFTER consecutive misses"
        );

        // 8th consecutive miss for the y=100 slot: evicted.
        s.apply(ScanResult::Rows(vec![exact("other", "1 ex", 500)], false));
        assert!(
            !s.rows().iter().any(|r| r.item_key == "a"),
            "the 8th consecutive miss must evict the slot"
        );
    }

    #[test]
    fn two_stage_stale_hides_at_eight_and_clears_at_twelve() {
        let mut s = Stabilizer::new();
        s.apply(ScanResult::Rows(vec![exact("a", "3 ex", 100)], false));
        assert_eq!(s.rows().len(), 1);

        for i in 1..8 {
            s.apply(ScanResult::Rows(vec![], false));
            assert_eq!(s.rows().len(), 1, "empty scan {i} of 7 must not hide yet");
        }
        s.apply(ScanResult::Rows(vec![], false));
        assert!(s.rows().is_empty(), "the 8th consecutive empty scan must hide the display");

        // Slot is kept alive underneath: a read of the SAME item displays
        // immediately (fast recovery), no re-confirmation needed.
        s.apply(ScanResult::Rows(vec![exact("a", "3 ex", 100)], false));
        assert_eq!(s.rows().len(), 1, "the slot must have survived the hide, showing again immediately");

        // Drive it back into an empty streak and all the way to 12 to
        // force a real clear.
        for _ in 0..12 {
            s.apply(ScanResult::Rows(vec![], false));
        }

        // A different item at the same position: if the old slot had
        // truly been cleared, this is a brand new slot and displays
        // immediately. If it had only been hidden, "a" would still be the
        // tracked item and this would enter the 2-read pending-switch path
        // instead of showing right away.
        s.apply(ScanResult::Rows(vec![exact("b", "1 ex", 100)], false));
        assert_eq!(
            s.rows().first().map(|r| r.item_key.as_str()),
            Some("b"),
            "12 consecutive empty scans must have cleared the old slot"
        );
    }

    #[test]
    fn gate_empty_clears_immediately_even_after_a_single_hit() {
        let mut s = Stabilizer::new();
        s.apply(ScanResult::Rows(vec![exact("a", "3 ex", 100), exact("b", "1 ex", 200)], false));
        assert_eq!(s.rows().len(), 2);

        s.apply(ScanResult::GateEmpty);
        assert!(s.rows().is_empty(), "gate-empty must clear all slots instantly, no hide delay");

        // Verify it's a real clear, not just a hide: a different item at
        // the same position displays immediately rather than needing a
        // 2-read pending switch.
        s.apply(ScanResult::Rows(vec![exact("c", "5 ex", 100)], false));
        assert_eq!(s.rows().first().map(|r| r.item_key.as_str()), Some("c"));
    }

    #[test]
    fn nobands_hides_after_two_and_clears_after_four() {
        // FAST-CLOSE INVARIANT (user requirement, regressed once, guarded
        // forever): closing the panel must hide labels within
        // NOBANDS_HIDE_AFTER = 2 scans (~0.8-1.0 s at live cadence), in
        // ANY scene brightness. Do not raise this constant without an
        // explicit user decision.
        let mut s = Stabilizer::new();
        s.apply(ScanResult::Rows(vec![exact("a", "3 ex", 100)], false));
        assert_eq!(s.rows().len(), 1);

        s.apply(ScanResult::NoBands);
        assert_eq!(s.rows().len(), 1, "1st consecutive NoBands must not hide yet");
        s.apply(ScanResult::NoBands);
        assert!(s.rows().is_empty(), "the 2nd consecutive NoBands must hide the display");

        // Slot kept alive underneath: a read of the SAME item displays
        // immediately (fast recovery), no re-confirmation needed.
        s.apply(ScanResult::Rows(vec![exact("a", "3 ex", 100)], false));
        assert_eq!(s.rows().len(), 1, "the slot must have survived the NoBands hide, showing again immediately");

        // Drive it back into a NoBands streak all the way to 4 to force a
        // real clear.
        for _ in 0..4 {
            s.apply(ScanResult::NoBands);
        }

        // A different item at the same position: if the old slot had truly
        // been cleared, this is a brand new slot and displays immediately.
        s.apply(ScanResult::Rows(vec![exact("b", "1 ex", 100)], false));
        assert_eq!(
            s.rows().first().map(|r| r.item_key.as_str()),
            Some("b"),
            "4 consecutive NoBands scans must have cleared the old slot"
        );
    }

    #[test]
    fn nobands_recovery_resets_the_streak() {
        let mut s = Stabilizer::new();
        s.apply(ScanResult::Rows(vec![exact("a", "3 ex", 100)], false));
        s.apply(ScanResult::NoBands);
        assert_eq!(s.rows().len(), 1, "1 NoBands must not hide yet");

        // A real Rows pass in between must reset the NoBands streak.
        s.apply(ScanResult::Rows(vec![exact("a", "3 ex", 100)], false));
        s.apply(ScanResult::NoBands);
        assert_eq!(
            s.rows().len(),
            1,
            "the streak must have reset: 2 more NoBands after a recovery still isn't 3 consecutive"
        );
    }

    #[test]
    fn y_within_match_tolerance_still_matches_and_moves_the_slot() {
        let mut s = Stabilizer::new();
        s.apply(ScanResult::Rows(vec![exact("a", "3 ex", 100)], false));

        // 80px drift: within Y_MATCH_TOLERANCE_PX (90) but beyond
        // POSITION_SNAP_PX (22), so it's the same slot, moved for real.
        s.apply(ScanResult::Rows(vec![exact("a", "3 ex", 180)], false));
        assert_eq!(s.rows().len(), 1, "an 80px drift must still match the same slot, not create a new one");
        assert_eq!(s.rows()[0].y_top, 180, "a real position change beyond the snap radius must move the slot");
    }

    #[test]
    fn position_snap_ignores_small_drift() {
        let mut s = Stabilizer::new();
        s.apply(ScanResult::Rows(vec![exact("a", "3 ex", 100)], false));

        // 10px drift: inside POSITION_SNAP_PX (22), treated as jitter.
        s.apply(ScanResult::Rows(vec![exact("a", "3 ex", 110)], false));
        assert_eq!(s.rows()[0].y_top, 100, "small jitter must not move the slot");
    }

    #[test]
    fn count_stickiness_covers_a_dropped_marker_while_confirming() {
        let mut s = Stabilizer::new();
        // First (of two) Fuzzy reads carries an explicit "3x".
        s.apply(ScanResult::Rows(vec![row("a", "3 ex", 100, false, true)], false));
        assert!(s.rows().is_empty());

        // Second, confirming read has the same item but the count marker
        // dropped this frame (count_explicit=false); the remembered "3 ex"
        // must be what actually displays, not a default-1 amount.
        s.apply(ScanResult::Rows(vec![row("a", "1 ex", 100, false, false)], false));
        assert_eq!(s.rows().len(), 1);
        assert_eq!(s.rows()[0].amount, "3 ex", "a fresh remembered explicit count must be used, not the default");
    }

    #[test]
    fn count_stickiness_expires_after_1500ms() {
        let mut s = Stabilizer::new();
        s.apply(ScanResult::Rows(vec![row("a", "3 ex", 100, false, true)], false));
        assert!(s.rows().is_empty());

        std::thread::sleep(Duration::from_millis(1600));

        s.apply(ScanResult::Rows(vec![row("a", "1 ex", 100, false, false)], false));
        assert_eq!(s.rows().len(), 1);
        assert_eq!(s.rows()[0].amount, "1 ex", "an expired remembered count must fall back to the read's own amount");
    }

    #[test]
    fn stale_flag_tracks_the_latest_rows_result() {
        let mut s = Stabilizer::new();
        s.apply(ScanResult::Rows(vec![exact("a", "3 ex", 100)], true));
        assert!(s.stale());
        s.apply(ScanResult::Rows(vec![exact("a", "3 ex", 100)], false));
        assert!(!s.stale());
    }

    #[test]
    fn scrolled_shifts_rows_instantly_and_preserves_state() {
        let mut s = Stabilizer::new();
        s.apply(ScanResult::Rows(vec![exact("a", "3 ex", 300), exact("b", "1 ex", 900)], false));
        assert_eq!(s.rows().len(), 2);

        s.apply(ScanResult::Scrolled(-120));
        let ys: Vec<u32> = s.rows().iter().map(|r| r.y_top).collect();
        assert_eq!(ys, vec![180, 780], "labels must move by the scroll delta immediately");

        // Scroll must not count as a miss: a following matching read keeps
        // both slots displayed with no re-confirmation.
        s.apply(ScanResult::Rows(vec![exact("a", "3 ex", 180), exact("b", "1 ex", 780)], false));
        assert_eq!(s.rows().len(), 2);

        // Accumulation.
        s.apply(ScanResult::Scrolled(50));
        s.apply(ScanResult::Scrolled(50));
        let ys: Vec<u32> = s.rows().iter().map(|r| r.y_top).collect();
        assert_eq!(ys, vec![280, 880]);
    }

    #[test]
    fn scrolled_drops_rows_pushed_above_the_region() {
        let mut s = Stabilizer::new();
        s.apply(ScanResult::Rows(vec![exact("a", "3 ex", 100), exact("b", "1 ex", 900)], false));
        s.apply(ScanResult::Scrolled(-600));
        let rows = s.rows();
        assert_eq!(rows.len(), 1, "the slot scrolled far above the top is dropped");
        assert_eq!(rows[0].item_key, "b");
        assert_eq!(rows[0].y_top, 300);
    }
}
