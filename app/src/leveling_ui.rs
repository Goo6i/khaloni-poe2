//! Leveling-checklist panel: pure model, layout, and hit-testing. The
//! renderer draws from THIS geometry and the click handler resolves actions
//! from THIS geometry, so pixels and hitboxes cannot drift apart. All
//! coordinates are panel-local logical pixels; the caller offsets by the
//! panel's placed position.
//!
//! One act shows at a time: prev/next arrow boxes flank the centered act
//! title, and the act's steps render as checkbox rows in a fixed window
//! (keyboard-only scrolling via `visible` — no pixel scrolling). Completed
//! step ids persist as a plain newline-separated file so progress survives
//! restarts.

use std::collections::HashSet;
use std::path::Path;

use crate::config::Rect;
use khaloni_poe2_core::refdata::{LevelingAct, LevelingStep};

const PAD: i32 = 12;
const TITLE_H: i32 = 30;
const CLOSE: i32 = 20;
const ARROW: i32 = 20;
const HEADER_H: i32 = 24;
const ROW_H: i32 = 22;
const CHECK: i32 = 16;
const CHECK_GAP: i32 = 8;
const VISIBLE_ROWS: usize = 16;
const WIDTH_MIN: i32 = 420;
const WIDTH_MAX: i32 = 760;
/// One-line rows: long guide steps are cut so one verbose description cannot
/// pin the panel at max width.
const ROW_MAX_CHARS: usize = 90;

#[derive(Debug, Clone, PartialEq)]
pub struct Panel {
    pub acts: Vec<LevelingAct>,
    /// Index into `acts` of the act on screen.
    pub act: usize,
    /// Completed step ids (see `step_id`), persisted via load/save_done.
    pub done: HashSet<String>,
    pub scroll: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Close,
    PrevAct,
    NextAct,
    /// Toggle the step with this id in the done set.
    ToggleStep(String),
    ScrollUp,
    ScrollDown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub w: i32,
    pub h: i32,
    pub close: Rect,
    /// Prev/next act arrows flanking the centered act title.
    pub prev: Rect,
    pub next: Rect,
    /// One full-width rect per VISIBLE step row, parallel to `visible`.
    pub rows: Vec<Rect>,
    /// Checkbox hit-zone and the step id it toggles, parallel to `rows`.
    pub checks: Vec<(Rect, String)>,
    pub visible: std::ops::Range<usize>,
}

/// Stable identity for a step in the done set. The XileHUD data carries an
/// `id` per step; if one is ever blank we fall back to "act{n}:{index}" so
/// the checkbox still round-trips (at the cost of breaking if the guide
/// reorders steps — acceptable for a fallback).
pub fn step_id(act: &LevelingAct, index: usize, step: &LevelingStep) -> String {
    if step.id.is_empty() {
        format!("act{}:{index}", act.act)
    } else {
        step.id.clone()
    }
}

/// The one-line string the renderer draws for a step: zone prefix when
/// present, then the description. Width is measured against THIS, so the
/// text column and panel width cannot disagree.
pub fn drawn_step(step: &LevelingStep) -> String {
    let s = if step.zone.is_empty() {
        step.description.clone()
    } else {
        format!("{} — {}", step.zone, step.description)
    };
    let flat = s.split('\n').map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" · ");
    if flat.chars().count() <= ROW_MAX_CHARS {
        return flat;
    }
    let mut cut: String = flat.chars().take(ROW_MAX_CHARS - 1).collect();
    cut.push('…');
    cut
}

/// The visible window over the current act's steps: scroll clamped so it can
/// never leave the tail when the act (and its step count) changes.
fn visible_window(p: &Panel) -> std::ops::Range<usize> {
    let len = p.acts.get(p.act).map_or(0, |a| a.steps.len());
    let scroll = p.scroll.min(len.saturating_sub(1));
    scroll..(scroll + VISIBLE_ROWS).min(len)
}

pub fn layout(p: &Panel, measure: &dyn Fn(&str) -> i32) -> Layout {
    let visible = visible_window(p);
    let empty: &[LevelingStep] = &[];
    let steps = p.acts.get(p.act).map_or(empty, |a| a.steps.as_slice());
    // Sized to the longest step on screen (checkbox column included), not the
    // whole act: off-screen text never widens the panel.
    let row_w = steps[visible.clone()].iter().map(|s| measure(&drawn_step(s))).max().unwrap_or(0);
    let w = (PAD + CHECK + CHECK_GAP + row_w + PAD).clamp(WIDTH_MIN, WIDTH_MAX);

    let close = Rect { x: w - PAD - CLOSE, y: PAD, w: CLOSE as u32, h: CLOSE as u32 };
    let mut y = PAD + TITLE_H;
    // Act header: arrows at the panel edges, the act title centered between
    // them (the renderer centers the text; only the arrows are hit zones).
    let prev = Rect { x: PAD, y: y + (HEADER_H - ARROW) / 2, w: ARROW as u32, h: ARROW as u32 };
    let next = Rect { x: w - PAD - ARROW, ..prev };
    y += HEADER_H + 6;

    let mut rows = Vec::new();
    let mut checks = Vec::new();
    if let Some(act) = p.acts.get(p.act) {
        for i in visible.clone() {
            rows.push(Rect { x: PAD, y, w: (w - 2 * PAD) as u32, h: ROW_H as u32 });
            checks.push((
                Rect { x: PAD, y: y + (ROW_H - CHECK) / 2, w: CHECK as u32, h: CHECK as u32 },
                step_id(act, i, &act.steps[i]),
            ));
            y += ROW_H;
        }
    }
    Layout { w, h: y + PAD, close, prev, next, rows, checks, visible }
}

fn inside(r: &Rect, x: i32, y: i32) -> bool {
    x >= r.x && x < r.x + r.w as i32 && y >= r.y && y < r.y + r.h as i32
}

/// Click resolution. The whole step row toggles, not just the 16px checkbox:
/// small targets are miserable to hit mid-combat. Scrolling is keyboard-only
/// (the caller maps Up/Down to ScrollUp/ScrollDown).
pub fn hit(_p: &Panel, lay: &Layout, x: i32, y: i32) -> Option<Action> {
    if inside(&lay.close, x, y) {
        return Some(Action::Close);
    }
    if inside(&lay.prev, x, y) {
        return Some(Action::PrevAct);
    }
    if inside(&lay.next, x, y) {
        return Some(Action::NextAct);
    }
    for (row, (_, id)) in lay.rows.iter().zip(&lay.checks) {
        if inside(row, x, y) {
            return Some(Action::ToggleStep(id.clone()));
        }
    }
    None
}

/// Auto-advance from a `ZoneEnter` log event. Every XileHUD step carries an
/// explicit `zone` field (the zone the player is in while doing it), so the
/// match is `zone` against `step.zone`, case-insensitive.
///
/// The target is the FIRST not-yet-done step in that zone, in guide order:
/// entering a zone proves everything before that point is behind the player,
/// so all earlier steps (whole earlier acts plus the matched act's prior
/// steps) get marked done and the view moves to the matched act with the
/// step scrolled into sight. The matched step itself stays pending —
/// arriving in a zone is not the same as doing what the guide says there.
///
/// Returns whether anything changed: entering the same zone twice is a
/// no-op the second time, as is a zone the guide never mentions or one
/// whose steps are all already checked (revisits must not yank the view).
pub fn advance_to_zone(p: &mut Panel, zone: &str) -> bool {
    let want = zone.trim().to_lowercase();
    if want.is_empty() {
        return false;
    }
    let mut target = None;
    'find: for (ai, act) in p.acts.iter().enumerate() {
        for (si, st) in act.steps.iter().enumerate() {
            if st.zone.to_lowercase() == want && !p.done.contains(&step_id(act, si, st)) {
                target = Some((ai, si));
                break 'find;
            }
        }
    }
    let Some((ai, si)) = target else { return false };

    let mut changed = false;
    for (a, act) in p.acts.iter().enumerate().take(ai + 1) {
        // Earlier acts complete entirely; the matched act only up to (not
        // including) the matched step.
        let upto = if a == ai { si } else { act.steps.len() };
        for j in 0..upto {
            if p.done.insert(step_id(act, j, &act.steps[j])) {
                changed = true;
            }
        }
    }
    if p.act != ai {
        p.act = ai;
        changed = true;
    }
    // Minimal scroll adjustment: only move the window when the matched step
    // is outside it, so a user-chosen scroll position survives revisits.
    let scroll = if si < p.scroll {
        si
    } else if si >= p.scroll + VISIBLE_ROWS {
        si + 1 - VISIBLE_ROWS
    } else {
        p.scroll
    };
    if p.scroll != scroll {
        p.scroll = scroll;
        changed = true;
    }
    changed
}

/// Reads the persisted done set: one step id per line. A missing or
/// unreadable file is simply an empty set (first run).
pub fn load_done(dir: &Path) -> HashSet<String> {
    std::fs::read_to_string(dir.join("leveling_done.txt"))
        .map(|s| s.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect())
        .unwrap_or_default()
}

/// Writes the done set as newline-separated ids. Sorted so the file is
/// stable across runs (diffs and syncs stay quiet). A whole-file rewrite is
/// atomic enough here: worst case a crash loses checkbox state, not game
/// data.
pub fn save_done(dir: &Path, done: &HashSet<String>) -> anyhow::Result<()> {
    let mut ids: Vec<&str> = done.iter().map(String::as_str).collect();
    ids.sort_unstable();
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join("leveling_done.txt"), ids.join("\n"))?;
    Ok(())
}
