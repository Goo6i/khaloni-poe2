use std::time::{Duration, Instant};

use crate::pricing::{Denom, Priced, Tier};

/// Where the list stood when a scan was read: `offset` is the scroll
/// accumulated since tracking last (re)started, in preprocessed pixels,
/// and `epoch` counts those restarts (each lost track begins a new one),
/// so offsets from different epochs are never compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScrollMark {
    pub epoch: u64,
    pub offset: i64,
}

/// One pass's priced rows. `at` is the scroll position of the frame they
/// were read from (None: read at the current position). A `partial` scan
/// carries only some of the rows on screen (template hits that did not
/// wait for tesseract): it updates and adds slots, but says nothing about
/// the slots it does not mention.
pub struct Scan {
    pub rows: Vec<Priced>,
    /// The price service's staleness flag.
    pub stale: bool,
    pub at: Option<ScrollMark>,
    pub partial: bool,
}

/// What one pass of the reward pipeline produced: priced rows; a signal
/// that the bar gate is closed, so no panel is on screen; a signal that
/// the gate is open but this frame shows no reward bar; or the list's
/// motion since the previous frame.
pub enum ScanResult {
    Rows(Scan),
    NoBands,
    GateEmpty,
    /// The list content moved by `dy` preprocessed pixels (positive:
    /// down), leaving the scroll position at `to`. `span` is the list's
    /// visible extent (top, bottom) in preprocessed pixels: a row whose
    /// centre leaves it has scrolled out of view. Slots move instantly; no
    /// confirmation or miss bookkeeping is touched, so a scan landing
    /// after the scroll matches the moved slots in place.
    Scrolled { dy: i64, to: ScrollMark, span: (i64, i64) },
    /// Frame-to-frame tracking broke (a flick the tracker cannot follow,
    /// a panel switch); positions continue from `to`, a new epoch. If a
    /// scroll was in progress the held positions are untrustworthy and the
    /// display hides until the next scan re-anchors; outside a scroll it
    /// is ignored so tooltip occlusion keeps its ride-through tolerance.
    TrackingLost { to: ScrollMark },
}

impl ScanResult {
    /// A complete scan read at the current scroll position.
    pub fn rows(rows: Vec<Priced>, stale: bool) -> ScanResult {
        ScanResult::Rows(Scan { rows, stale, at: None, partial: false })
    }
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
    count: u32,
    value_ex: f64,
    value_chaos: f64,
}

impl Snapshot {
    fn from_row(row: &Priced) -> Snapshot {
        Snapshot {
            label: row.label.clone(),
            amount: row.amount.clone(),
            denom: row.denom,
            tier: row.tier,
            count: row.count,
            value_ex: row.value_ex,
            value_chaos: row.value_chaos,
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
    /// Top of the row in preprocessed pixels. Signed: a slot is moved by
    /// scrolls, and one that leaves the list is dropped, never clamped.
    y: i64,
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
    /// The last read that had an explicit "Nx" count, the item it was a
    /// count OF, and when: honored for STACK_STICKY after the marker itself
    /// drops out of a matching read of that same item. A remembered "3x"
    /// says nothing about whatever replaces the item in this position.
    last_explicit: Option<(String, Snapshot, Instant)>,
}

impl Slot {
    fn new(row: Priced, y: i64) -> Slot {
        let mut slot = Slot {
            y,
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

    fn touch_position(&mut self, row: &Priced, y: i64) {
        if y.abs_diff(self.y) > u64::from(POSITION_SNAP_PX) {
            self.y = y;
        }
        self.height = row.height;
    }

    fn remember_explicit(&mut self, row: &Priced) {
        self.last_explicit = Some((row.item_key.clone(), Snapshot::from_row(row), Instant::now()));
    }

    /// Resolves what to show for a read of the item this slot is ALREADY
    /// displaying. Only the stack count is sticky: a read that lost its
    /// "Nx" marker was priced as a single unit, so while the slot shows a
    /// counted stack that read's amounts are for the wrong quantity and
    /// what is on screen stays. Every other same-item read is the fresher
    /// truth and replaces the display - a gem row shown as "…" while its
    /// trade search ran must pick up the price the moment a read carries
    /// it, and a repriced table must reach rows that never had a count.
    fn resolve_established(&mut self, row: &Priced) -> Snapshot {
        if row.count_explicit {
            self.remember_explicit(row);
            return Snapshot::from_row(row);
        }
        if row.count == self.snap.count {
            return Snapshot::from_row(row);
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
            self.remember_explicit(row);
            return Snapshot::from_row(row);
        }
        if let Some((key, snap, when)) = &self.last_explicit {
            if *key == row.item_key && when.elapsed() < STACK_STICKY {
                return snap.clone();
            }
        }
        Snapshot::from_row(row)
    }

    /// (Re)targets this slot at `row`'s item before it has been displayed
    /// yet: a confident read locks in on the spot; a Fuzzy one starts (or
    /// restarts) the CONFIRM_FUZZY-read confirmation count.
    fn establish_pre_display(&mut self, row: &Priced) {
        self.forget_other_items(&row.item_key);
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
                self.remember_explicit(row);
            }
        }
    }

    /// Drops a remembered count that belongs to a different item than the
    /// one this slot is about to track.
    fn forget_other_items(&mut self, item_key: &str) {
        if self.last_explicit.as_ref().is_some_and(|(key, _, _)| key != item_key) {
            self.last_explicit = None;
        }
    }

    /// Commits a pending switch: the PENDING_SWITCH consecutive agreeing
    /// reads that triggered it are themselves the confirmation, so the new
    /// item displays immediately regardless of its own match tier.
    fn commit_switch(&mut self, row: &Priced) {
        self.forget_other_items(&row.item_key);
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
fn apply_read(slot: &mut Slot, row: Priced, y: i64, immediate_switch: bool) {
    slot.misses = 0;
    slot.touch_position(&row, y);

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
                slot.remember_explicit(&row);
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
    // reads of the SAME candidate item before switching away from it,
    // UNLESS a scroll just proved the content moved (the guard would only
    // prolong showing the previous row's price at a reused position).
    if immediate_switch {
        slot.pending = None;
        slot.commit_switch(&row);
        return;
    }
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
    /// Nonzero right after Scrolled events, decremented per Rows scan.
    /// While set: (a) a different item read at a slot's position replaces
    /// it IMMEDIATELY instead of waiting PENDING_SWITCH reads - the
    /// scroll already proved content moved, so the anti-flicker rule
    /// would only prolong showing the previous row's price at a reused
    /// position (live symptom: "scrolling made prices mangle"); (b) new
    /// "?" rows are not created (the first post-burst frame can carry
    /// motion blur that fakes count tokens).
    scroll_recent: u8,
    /// Set by TrackingLost during a scroll: positions are untrustworthy,
    /// so rows() hides everything until the next Rows result re-anchors.
    lost_hidden: bool,
    /// The list's scroll position as of the last motion message: a scan
    /// read at an earlier position is moved by the scroll since.
    mark: Option<ScrollMark>,
    /// The list's visible extent (preprocessed px) from the last scroll.
    span: Option<(i64, i64)>,
}

/// Whether a row at `y` of `height` is still in the list: its centre is
/// inside the visible span. A row half scrolled out keeps its price until
/// most of it is gone.
fn in_span(y: i64, height: u32, span: (i64, i64)) -> bool {
    let centre = y + i64::from(height / 2);
    centre >= span.0 && centre < span.1
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
                self.lost_hidden = false;
                self.mark = None;
                self.span = None;
            }
            ScanResult::TrackingLost { to } => {
                self.mark = Some(to);
                // Only a scroll makes held positions wrong; outside one,
                // ride it out like any other occlusion/transition frame.
                if self.scroll_recent > 0 {
                    self.lost_hidden = true;
                }
            }
            ScanResult::NoBands => {
                self.nobands_streak = self.nobands_streak.saturating_add(1);
                if self.nobands_streak >= NOBANDS_CLEAR_AFTER {
                    self.slots.clear();
                }
            }
            ScanResult::Scrolled { dy, to, span } => {
                self.scroll_recent = 2;
                self.mark = Some(to);
                self.span = Some(span);
                // Instant translation. A slot that leaves the list at
                // either edge is dropped; it re-enters from a scan if the
                // list comes back.
                self.slots.retain_mut(|slot| {
                    slot.y += dy;
                    in_span(slot.y, slot.height, span)
                });
            }
            ScanResult::Rows(scan) => {
                // Rows read at an earlier scroll position move by the
                // scroll since; a read from before tracking was lost has
                // no known place and is dropped whole.
                let Some(shift) = self.shift_since(scan.at) else {
                    return;
                };
                let span = self.span;
                let was_empty = scan.rows.is_empty();
                let rows: Vec<(i64, Priced)> = scan
                    .rows
                    .into_iter()
                    .map(|r| (i64::from(r.y_top) + shift, r))
                    .filter(|(y, r)| span.is_none_or(|sp| in_span(*y, r.height, sp)))
                    .collect();
                self.nobands_streak = 0;
                self.stale = scan.stale;
                let post_scroll = self.scroll_recent > 0;
                if scan.partial {
                    self.update(rows, post_scroll, false, false);
                    return;
                }
                // A scan after lost tracking is the new ground truth:
                // slot positions are arbitrary, so anything this scan
                // does not match is evicted instead of miss-counted
                // (otherwise a stale slot re-shows at a wrong position
                // the moment the lost-hide lifts).
                let resync = std::mem::take(&mut self.lost_hidden);
                if was_empty {
                    self.empty_streak = self.empty_streak.saturating_add(1);
                } else {
                    self.empty_streak = 0;
                }
                if self.empty_streak >= STALE_CLEAR_AFTER {
                    self.slots.clear();
                } else {
                    self.scroll_recent = self.scroll_recent.saturating_sub(1);
                    self.update(rows, post_scroll, resync, true);
                }
            }
        }
    }

    /// How far the list has scrolled since a scan read at `at`, or None
    /// when it was read before tracking was lost. A scan from a newer
    /// epoch than any motion seen (the first after the gate opened) is
    /// taken as where the list is now.
    fn shift_since(&mut self, at: Option<ScrollMark>) -> Option<i64> {
        let Some(at) = at else {
            return Some(0);
        };
        match self.mark {
            Some(now) if at.epoch < now.epoch => None,
            Some(now) if at.epoch == now.epoch => Some(now.offset - at.offset),
            _ => {
                self.mark = Some(at);
                Some(0)
            }
        }
    }

    /// Matches reads (with their current positions) to slots. A partial
    /// scan (`count_misses` false) says nothing about the slots it does
    /// not mention.
    fn update(&mut self, rows: Vec<(i64, Priced)>, post_scroll: bool, resync: bool, count_misses: bool) {
        let mut matched = vec![false; self.slots.len()];
        for (y, row) in rows {
            // Post-scroll frames can carry motion blur that fakes count
            // tokens; do not mint NEW "?" slots from them (existing slots
            // still update normally).
            let is_unknown = row.item_key.starts_with("unpriceable") || row.denom == crate::pricing::Denom::None && row.amount == khaloni_poe2_core::value::UNKNOWN;
            let existing = self
                .slots
                .iter()
                .enumerate()
                .filter(|(i, _)| !matched[*i])
                .filter(|(_, s)| s.y.abs_diff(y) <= u64::from(Y_MATCH_TOLERANCE_PX))
                .min_by_key(|(_, s)| s.y.abs_diff(y))
                .map(|(i, _)| i);
            match existing {
                Some(i) => {
                    matched[i] = true;
                    apply_read(&mut self.slots[i], row, y, post_scroll);
                }
                None if post_scroll && is_unknown => {}
                None => {
                    self.slots.push(Slot::new(row, y));
                    matched.push(true);
                }
            }
        }

        if !count_misses {
            return;
        }
        let mut i = 0;
        while i < self.slots.len() {
            if matched[i] {
                i += 1;
                continue;
            }
            self.slots[i].misses += 1;
            if resync || self.slots[i].misses >= EVICT_AFTER {
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
        if self.lost_hidden
            || self.empty_streak >= STALE_HIDE_AFTER
            || self.nobands_streak >= NOBANDS_HIDE_AFTER
        {
            return Vec::new();
        }
        let mut out: Vec<Priced> = self
            .slots
            .iter()
            .filter(|s| s.displayed && self.span.is_none_or(|sp| in_span(s.y, s.height, sp)))
            .map(|s| Priced {
                count: s.snap.count,
                value_ex: s.snap.value_ex,
                value_chaos: s.snap.value_chaos,
                // The span's top is a bar's top inside the region, so a row
                // still in the list never starts above the region.
                y_top: u32::try_from(s.y).unwrap_or(0),
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

    /// Drops everything immediately (pricing paused, game window gone).
    pub fn clear(&mut self) {
        self.slots.clear();
        self.stale = false;
        self.empty_streak = 0;
        self.nobands_streak = 0;
        self.lost_hidden = false;
        self.mark = None;
        self.span = None;
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
            count: 1,
            value_ex: 1.0,
            value_chaos: 1.0,
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

    /// A scroll inside a list taller than any of these tests' rows.
    fn scrolled(dy: i64) -> ScanResult {
        ScanResult::Scrolled { dy, to: ScrollMark { epoch: 0, offset: dy }, span: (0, 100_000) }
    }

    fn lost() -> ScanResult {
        ScanResult::TrackingLost { to: ScrollMark { epoch: 1, offset: 0 } }
    }

    #[test]
    fn lock_in_one_for_exact_tier_read() {
        let mut s = Stabilizer::new();
        s.apply(ScanResult::rows(vec![exact("a", "3 ex", 100)], false));
        assert_eq!(s.rows().len(), 1, "a locks_in_one read must display after a single scan");
        assert_eq!(s.rows()[0].item_key, "a");
    }

    #[test]
    fn fuzzy_read_needs_two_consecutive_identical_reads_to_display() {
        let mut s = Stabilizer::new();
        s.apply(ScanResult::rows(vec![fuzzy("a", "3 ex", 100)], false));
        assert!(s.rows().is_empty(), "a single Fuzzy read must not display yet");

        s.apply(ScanResult::rows(vec![fuzzy("a", "3 ex", 100)], false));
        assert_eq!(s.rows().len(), 1, "a second consecutive identical Fuzzy read must confirm and display");
    }

    #[test]
    fn switch_after_two_consecutive_different_reads() {
        let mut s = Stabilizer::new();
        s.apply(ScanResult::rows(vec![exact("a", "3 ex", 100)], false));
        assert_eq!(s.rows()[0].item_key, "a");

        // First read of a different item: not enough to switch yet.
        s.apply(ScanResult::rows(vec![exact("b", "1 ex", 100)], false));
        assert_eq!(s.rows()[0].item_key, "a", "a single different read must not switch the slot");

        // Second consecutive read of the SAME different item: switches.
        s.apply(ScanResult::rows(vec![exact("b", "1 ex", 100)], false));
        assert_eq!(s.rows()[0].item_key, "b", "two consecutive different reads must switch the slot");
    }

    #[test]
    fn miss_preserves_the_lock_and_does_not_reset_a_pending_switch() {
        let mut s = Stabilizer::new();
        s.apply(ScanResult::rows(vec![exact("a", "3 ex", 100)], false));
        // First of two switch-reads for "b".
        s.apply(ScanResult::rows(vec![exact("b", "1 ex", 100)], false));
        assert_eq!(s.rows()[0].item_key, "a");

        // A miss for the y=100 slot: some unrelated row elsewhere, so this
        // is a per-slot miss, not a global empty scan.
        s.apply(ScanResult::rows(vec![exact("unrelated", "9 ex", 500)], false));
        assert_eq!(
            s.rows().iter().find(|r| r.y_top == 100).unwrap().item_key,
            "a",
            "a miss must not drop the existing lock"
        );

        // Second consecutive "b" read: the miss in between must not have
        // reset the pending-switch streak, so this completes the switch.
        s.apply(ScanResult::rows(vec![exact("b", "1 ex", 100)], false));
        assert_eq!(
            s.rows().iter().find(|r| r.y_top == 100).unwrap().item_key,
            "b",
            "a miss must not reset the pending-switch counter"
        );
    }

    #[test]
    fn evict_after_eight_consecutive_misses() {
        let mut s = Stabilizer::new();
        s.apply(ScanResult::rows(vec![exact("a", "3 ex", 100)], false));
        assert!(s.rows().iter().any(|r| r.item_key == "a"));

        for _ in 0..7 {
            s.apply(ScanResult::rows(vec![exact("other", "1 ex", 500)], false));
        }
        assert!(
            s.rows().iter().any(|r| r.item_key == "a"),
            "a slot must survive fewer than EVICT_AFTER consecutive misses"
        );

        // 8th consecutive miss for the y=100 slot: evicted.
        s.apply(ScanResult::rows(vec![exact("other", "1 ex", 500)], false));
        assert!(
            !s.rows().iter().any(|r| r.item_key == "a"),
            "the 8th consecutive miss must evict the slot"
        );
    }

    #[test]
    fn two_stage_stale_hides_at_eight_and_clears_at_twelve() {
        let mut s = Stabilizer::new();
        s.apply(ScanResult::rows(vec![exact("a", "3 ex", 100)], false));
        assert_eq!(s.rows().len(), 1);

        for i in 1..8 {
            s.apply(ScanResult::rows(vec![], false));
            assert_eq!(s.rows().len(), 1, "empty scan {i} of 7 must not hide yet");
        }
        s.apply(ScanResult::rows(vec![], false));
        assert!(s.rows().is_empty(), "the 8th consecutive empty scan must hide the display");

        // Slot is kept alive underneath: a read of the SAME item displays
        // immediately (fast recovery), no re-confirmation needed.
        s.apply(ScanResult::rows(vec![exact("a", "3 ex", 100)], false));
        assert_eq!(s.rows().len(), 1, "the slot must have survived the hide, showing again immediately");

        // Drive it back into an empty streak and all the way to 12 to
        // force a real clear.
        for _ in 0..12 {
            s.apply(ScanResult::rows(vec![], false));
        }

        // A different item at the same position: if the old slot had
        // truly been cleared, this is a brand new slot and displays
        // immediately. If it had only been hidden, "a" would still be the
        // tracked item and this would enter the 2-read pending-switch path
        // instead of showing right away.
        s.apply(ScanResult::rows(vec![exact("b", "1 ex", 100)], false));
        assert_eq!(
            s.rows().first().map(|r| r.item_key.as_str()),
            Some("b"),
            "12 consecutive empty scans must have cleared the old slot"
        );
    }

    #[test]
    fn gate_empty_clears_immediately_even_after_a_single_hit() {
        let mut s = Stabilizer::new();
        s.apply(ScanResult::rows(vec![exact("a", "3 ex", 100), exact("b", "1 ex", 200)], false));
        assert_eq!(s.rows().len(), 2);

        s.apply(ScanResult::GateEmpty);
        assert!(s.rows().is_empty(), "gate-empty must clear all slots instantly, no hide delay");

        // Verify it's a real clear, not just a hide: a different item at
        // the same position displays immediately rather than needing a
        // 2-read pending switch.
        s.apply(ScanResult::rows(vec![exact("c", "5 ex", 100)], false));
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
        s.apply(ScanResult::rows(vec![exact("a", "3 ex", 100)], false));
        assert_eq!(s.rows().len(), 1);

        s.apply(ScanResult::NoBands);
        assert_eq!(s.rows().len(), 1, "1st consecutive NoBands must not hide yet");
        s.apply(ScanResult::NoBands);
        assert!(s.rows().is_empty(), "the 2nd consecutive NoBands must hide the display");

        // Slot kept alive underneath: a read of the SAME item displays
        // immediately (fast recovery), no re-confirmation needed.
        s.apply(ScanResult::rows(vec![exact("a", "3 ex", 100)], false));
        assert_eq!(s.rows().len(), 1, "the slot must have survived the NoBands hide, showing again immediately");

        // Drive it back into a NoBands streak all the way to 4 to force a
        // real clear.
        for _ in 0..4 {
            s.apply(ScanResult::NoBands);
        }

        // A different item at the same position: if the old slot had truly
        // been cleared, this is a brand new slot and displays immediately.
        s.apply(ScanResult::rows(vec![exact("b", "1 ex", 100)], false));
        assert_eq!(
            s.rows().first().map(|r| r.item_key.as_str()),
            Some("b"),
            "4 consecutive NoBands scans must have cleared the old slot"
        );
    }

    #[test]
    fn nobands_recovery_resets_the_streak() {
        let mut s = Stabilizer::new();
        s.apply(ScanResult::rows(vec![exact("a", "3 ex", 100)], false));
        s.apply(ScanResult::NoBands);
        assert_eq!(s.rows().len(), 1, "1 NoBands must not hide yet");

        // A real Rows pass in between must reset the NoBands streak.
        s.apply(ScanResult::rows(vec![exact("a", "3 ex", 100)], false));
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
        s.apply(ScanResult::rows(vec![exact("a", "3 ex", 100)], false));

        // 80px drift: within Y_MATCH_TOLERANCE_PX (90) but beyond
        // POSITION_SNAP_PX (22), so it's the same slot, moved for real.
        s.apply(ScanResult::rows(vec![exact("a", "3 ex", 180)], false));
        assert_eq!(s.rows().len(), 1, "an 80px drift must still match the same slot, not create a new one");
        assert_eq!(s.rows()[0].y_top, 180, "a real position change beyond the snap radius must move the slot");
    }

    #[test]
    fn position_snap_ignores_small_drift() {
        let mut s = Stabilizer::new();
        s.apply(ScanResult::rows(vec![exact("a", "3 ex", 100)], false));

        // 10px drift: inside POSITION_SNAP_PX (22), treated as jitter.
        s.apply(ScanResult::rows(vec![exact("a", "3 ex", 110)], false));
        assert_eq!(s.rows()[0].y_top, 100, "small jitter must not move the slot");
    }

    #[test]
    fn count_stickiness_covers_a_dropped_marker_while_confirming() {
        let mut s = Stabilizer::new();
        // First (of two) Fuzzy reads carries an explicit "3x".
        s.apply(ScanResult::rows(vec![row("a", "3 ex", 100, false, true)], false));
        assert!(s.rows().is_empty());

        // Second, confirming read has the same item but the count marker
        // dropped this frame (count_explicit=false); the remembered "3 ex"
        // must be what actually displays, not a default-1 amount.
        s.apply(ScanResult::rows(vec![row("a", "1 ex", 100, false, false)], false));
        assert_eq!(s.rows().len(), 1);
        assert_eq!(s.rows()[0].amount, "3 ex", "a fresh remembered explicit count must be used, not the default");
    }

    #[test]
    fn count_stickiness_expires_after_1500ms() {
        let mut s = Stabilizer::new();
        s.apply(ScanResult::rows(vec![row("a", "3 ex", 100, false, true)], false));
        assert!(s.rows().is_empty());

        std::thread::sleep(Duration::from_millis(1600));

        s.apply(ScanResult::rows(vec![row("a", "1 ex", 100, false, false)], false));
        assert_eq!(s.rows().len(), 1);
        assert_eq!(s.rows()[0].amount, "1 ex", "an expired remembered count must fall back to the read's own amount");
    }

    #[test]
    fn stale_flag_tracks_the_latest_rows_result() {
        let mut s = Stabilizer::new();
        s.apply(ScanResult::rows(vec![exact("a", "3 ex", 100)], true));
        assert!(s.stale());
        s.apply(ScanResult::rows(vec![exact("a", "3 ex", 100)], false));
        assert!(!s.stale());
    }

    #[test]
    fn scrolled_shifts_rows_instantly_and_preserves_state() {
        let mut s = Stabilizer::new();
        s.apply(ScanResult::rows(vec![exact("a", "3 ex", 300), exact("b", "1 ex", 900)], false));
        assert_eq!(s.rows().len(), 2);

        s.apply(scrolled(-120));
        let ys: Vec<u32> = s.rows().iter().map(|r| r.y_top).collect();
        assert_eq!(ys, vec![180, 780], "labels must move by the scroll delta immediately");

        // Scroll must not count as a miss: a following matching read keeps
        // both slots displayed with no re-confirmation.
        s.apply(ScanResult::rows(vec![exact("a", "3 ex", 180), exact("b", "1 ex", 780)], false));
        assert_eq!(s.rows().len(), 2);

        // Accumulation.
        s.apply(scrolled(50));
        s.apply(scrolled(50));
        let ys: Vec<u32> = s.rows().iter().map(|r| r.y_top).collect();
        assert_eq!(ys, vec![280, 880]);
    }

    #[test]
    fn scrolled_drops_rows_pushed_above_the_region() {
        let mut s = Stabilizer::new();
        s.apply(ScanResult::rows(vec![exact("a", "3 ex", 100), exact("b", "1 ex", 900)], false));
        s.apply(scrolled(-600));
        let rows = s.rows();
        assert_eq!(rows.len(), 1, "the slot scrolled far above the top is dropped");
        assert_eq!(rows[0].item_key, "b");
        assert_eq!(rows[0].y_top, 300);
    }

    fn tagged(rows: Vec<Priced>, epoch: u64, offset: i64, partial: bool) -> ScanResult {
        ScanResult::Rows(Scan { rows, stale: false, at: Some(ScrollMark { epoch, offset }), partial })
    }

    #[test]
    fn a_read_made_before_a_scroll_moves_by_the_scroll_since() {
        let mut s = Stabilizer::new();
        s.apply(scrolled(-120));
        s.apply(tagged(vec![exact("a", "3 ex", 600)], 0, 0, false));
        assert_eq!(s.rows()[0].y_top, 480, "read at offset 0, shown at offset -120");
    }

    #[test]
    fn a_read_made_before_tracking_was_lost_is_dropped() {
        let mut s = Stabilizer::new();
        s.apply(tagged(vec![exact("a", "3 ex", 300)], 0, 0, false));
        s.apply(lost());
        s.apply(tagged(vec![exact("b", "1 ex", 600)], 0, 0, false));
        let keys: Vec<String> = s.rows().iter().map(|r| r.item_key.clone()).collect();
        assert_eq!(keys, ["a"], "a read from the old epoch has no known place");
        s.apply(tagged(vec![exact("b", "1 ex", 600)], 1, 0, false));
        assert!(s.rows().iter().any(|r| r.item_key == "b"));
    }

    #[test]
    fn a_partial_scan_says_nothing_about_rows_it_does_not_mention() {
        let mut s = Stabilizer::new();
        s.apply(ScanResult::rows(vec![exact("a", "3 ex", 100)], false));
        for _ in 0..EVICT_AFTER + 2 {
            s.apply(tagged(vec![exact("b", "1 ex", 600)], 0, 0, true));
        }
        let keys: Vec<String> = s.rows().iter().map(|r| r.item_key.clone()).collect();
        assert_eq!(keys, ["a", "b"], "template hits alone never evict a row");
    }

    #[test]
    fn tracking_lost_after_a_scroll_hides_until_the_next_scan() {
        let mut s = Stabilizer::new();
        s.apply(ScanResult::rows(vec![exact("a", "3 ex", 100)], false));
        assert_eq!(s.rows().len(), 1);
        s.apply(scrolled(30));
        s.apply(lost());
        assert!(s.rows().is_empty(), "lost tracking during a scroll must hide");
        s.apply(ScanResult::rows(vec![exact("a", "3 ex", 400)], false));
        assert_eq!(s.rows().len(), 1, "a fresh scan re-shows re-anchored rows");
    }

    #[test]
    fn tracking_lost_without_a_recent_scroll_is_ignored() {
        let mut s = Stabilizer::new();
        s.apply(ScanResult::rows(vec![exact("a", "3 ex", 100)], false));
        s.apply(lost()); // tooltip popped, no scroll
        assert_eq!(s.rows().len(), 1, "occlusion tolerance must keep rows visible");
    }

    #[test]
    fn gate_empty_resets_the_lost_hidden_state() {
        let mut s = Stabilizer::new();
        s.apply(ScanResult::rows(vec![exact("a", "3 ex", 100)], false));
        s.apply(scrolled(30));
        s.apply(lost());
        assert!(s.rows().is_empty());
        s.apply(ScanResult::GateEmpty);
        s.apply(ScanResult::rows(vec![exact("a", "3 ex", 100)], false));
        assert_eq!(s.rows().len(), 1);
    }
    #[test]
    fn a_pending_price_is_replaced_when_the_price_arrives() {
        let mut s = Stabilizer::new();
        let key = "gemx:detonate living:20";
        s.apply(ScanResult::rows(vec![exact(key, "…", 300)], false));
        assert_eq!(s.rows()[0].amount, "…");
        let mut priced = exact(key, "45", 300);
        priced.value_ex = 45.0;
        s.apply(ScanResult::rows(vec![priced], false));
        assert_eq!(s.rows()[0].amount, "45", "an uncounted same-item read must refresh the display");
        assert_eq!(s.rows()[0].value_ex, 45.0);
    }

    #[test]
    fn a_read_that_lost_its_count_marker_does_not_shrink_a_displayed_stack() {
        let mut s = Stabilizer::new();
        let mut stack = row("exalted orb", "3 (1 each)", 300, true, true);
        stack.count = 3;
        s.apply(ScanResult::rows(vec![stack], false));
        s.apply(ScanResult::rows(vec![exact("exalted orb", "1", 300)], false));
        assert_eq!(s.rows()[0].amount, "3 (1 each)");
        assert_eq!(s.rows()[0].count, 3);
    }

    #[test]
    fn a_remembered_count_never_prices_the_item_that_replaces_it() {
        // Two agreeing reads switch the slot to a new, uncounted item well
        // inside the stickiness window.
        let mut s = Stabilizer::new();
        let mut stack = row("exalted orb", "3 (1 each)", 300, true, true);
        stack.count = 3;
        s.apply(ScanResult::rows(vec![stack.clone()], false));
        for _ in 0..2 {
            s.apply(ScanResult::rows(vec![exact("divine orb", "1 div", 300)], false));
        }
        assert_eq!(s.rows()[0].item_key, "divine orb");
        assert_eq!(s.rows()[0].amount, "1 div");

        // Same through the post-scroll immediate switch.
        let mut s = Stabilizer::new();
        s.apply(ScanResult::rows(vec![stack], false));
        s.apply(scrolled(0));
        s.apply(ScanResult::rows(vec![exact("chaos orb", "1 chaos", 300)], false));
        assert_eq!(s.rows()[0].item_key, "chaos orb");
        assert_eq!(s.rows()[0].amount, "1 chaos");
    }
}
