//! The Evaluate panel: a three-column item card modelled on the game's own
//! item tooltip, replacing the flat appraisal list.
//!
//! Layout, left to right:
//!
//! ```text
//!   TIERING │        item card          │ SCORING │  filters
//!     P9    │  23 to Accuracy Rating    │   0.8   │ [23][max]
//!     S1    │  24% to Critical Damage   │   4.0   │ [24][max]
//! ```
//!
//! The card reads like the tooltip a player already knows (name header,
//! rarity/level block, then mods); the gutters add what the game does not
//! show — which affix family and tier each mod is, and how good the roll
//! is within that tier ladder — and the filter column on the right is what
//! actually goes to the trade search.
//!
//! Same discipline as the rest of the overlay: this module owns the pure
//! model, the geometry, and the hit-testing, and the renderer draws from
//! THIS geometry, so pixels and click targets cannot drift apart.
//!
//! On scoring, deliberately: it is roll quality — where this roll sits in
//! its own tier ladder — NOT an estimate of how much the mod contributes
//! to price. A price-contribution number would need a model we do not have
//! and could not justify, and the closed-source overlays' invented numbers
//! are exactly what their users learned to distrust.

use crate::config::Rect;
use crate::pricing::Denom;

const PAD: i32 = 12;
/// The name line is drawn in the large font; this is its whole band.
const TITLE_H: i32 = 30;
/// Rarity and the item-level lines: plain text, no hit targets.
const LINE_H: i32 = 18;
const ROW_H: i32 = 26;
const CHECK: i32 = 16;
/// Left gutter: wide enough for "P12" plus breathing room, fixed so every
/// badge lands on the same column and the card text starts flush.
const GUT_L: i32 = 34;
/// Right gutter: "5.0" plus room, likewise fixed.
const GUT_R: i32 = 40;
const GAP: i32 = 8;
const BOX_W: i32 = 48;
const BOX_H: i32 = 20;
const BOX_GAP: i32 = 6;
/// The one-off "TIERING"/"SCORING" heading line above the row band.
const HEAD_H: i32 = 20;
/// Separation between the card's blocks (header, rows, controls).
const BLOCK_GAP: i32 = 10;
const TOGGLE_H: i32 = 22;
const RADIO_H: i32 = 22;
const RADIO_GAP: i32 = 10;
const BTN_H: i32 = 28;
const BTN_GAP: i32 = 10;
const LISTING_H: i32 = 22;
const CLOSE: i32 = 20;
/// Value box: heading line, the headline number, then range/reliability.
const EST_H: i32 = 74;
const LABEL_MIN_W: i32 = 170;
const CHAR_W: i32 = 7;
const WIDTH_MIN: i32 = 480;
const WIDTH_MAX: i32 = 1600;

/// Which affix family a mod belongs to, for the tiering gutter's badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AffixKind {
    Prefix,
    Suffix,
    /// Implicit, corrupted, or otherwise not a rollable prefix/suffix.
    Other,
}

impl AffixKind {
    /// Badge prefix character: "P9", "S1", "—".
    pub fn letter(self) -> &'static str {
        match self {
            AffixKind::Prefix => "P",
            AffixKind::Suffix => "S",
            AffixKind::Other => "",
        }
    }
}

/// The left-gutter badge for a row, when the affix is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TierBadge {
    pub kind: AffixKind,
    /// 1 is the best tier, matching how players say "T1".
    pub tier: u8,
}

/// What a searchable row feeds when its checkbox is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// Index into the Query's stat filters (item mods, pseudo totals).
    Stat(usize),
    /// One of the trade site's computed weapon numbers. These are
    /// open-ended minimums ("this much DPS or more"), so a row carrying
    /// one gets a min box and no max box.
    Weapon(WeaponBound),
}

/// The trade site's equipment_filters keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeaponBound {
    Dps,
    Pdps,
    Edps,
    Crit,
    Aps,
}

/// One line of the card. Covers both derived stats (DPS, total Attributes)
/// and real mods; `target` is what makes a row searchable.
#[derive(Debug, Clone, PartialEq)]
pub struct StatRow {
    /// Text as the game words it, e.g. "23 to Accuracy Rating".
    pub label: String,
    /// Left gutter badge; None for rows with no known affix (derived
    /// stats, unmatched mods).
    pub badge: Option<TierBadge>,
    /// Right gutter roll-quality score, 0.0..=5.0. None when the affix has
    /// no tier ladder to score against — never a fabricated value.
    pub score: Option<f32>,
    /// Search bounds. `min` is prefilled from the item's own roll.
    pub min: f64,
    pub max: Option<f64>,
    pub enabled: bool,
    /// What the row drives in the search; None for display-only rows.
    pub target: Option<Target>,
    /// Rows the player rarely filters on, collapsed behind "Show N more"
    /// so the card stays as short as the tooltip it imitates.
    pub hidden: bool,
}

/// How hard the search should be relaxed before it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strictness {
    /// Every kept mod must meet the item's own roll.
    Quick,
    /// All minimums relaxed by 10%, which finds the comparable items an
    /// exact-roll search misses.
    Broad,
}

impl Strictness {
    pub fn label(self) -> &'static str {
        match self {
            Strictness::Quick => "Quick Price",
            Strictness::Broad => "Broad (-10%)",
        }
    }
}

/// The headline answer, pre-formatted so layout and drawing share one
/// source of truth for the strings. See `core::estimate` for the maths.
#[derive(Debug, Clone, PartialEq)]
pub struct EstimateView {
    pub amount: String,
    pub denom: Denom,
    pub detail: String,
    pub reliability: String,
    /// Below Medium: the renderer paints the reliability word red.
    pub shaky: bool,
}

/// The item header block, worded exactly as the tooltip does.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemHeader {
    pub name: String,
    /// "Rare", "Magic", … drives the name's colour.
    pub rarity: String,
    pub item_level: Option<u32>,
    pub requires_level: Option<u32>,
    /// Base-type constraint toggle, when the item has one.
    pub base: Option<BaseToggle>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BaseToggle {
    pub label: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Panel {
    pub header: ItemHeader,
    pub rows: Vec<StatRow>,
    /// Whether hidden rows are currently expanded.
    pub show_hidden: bool,
    pub strictness: Strictness,
    pub estimate: Option<EstimateView>,
    pub listings: Vec<String>,
    pub status: String,
    pub search_id: Option<String>,
}

/// Everything clickable or drawable, in panel-local logical pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub size: (i32, i32),
    pub close: Rect,
    /// Name, rarity line, and the level lines.
    pub name_pos: (i32, i32),
    pub rarity_pos: (i32, i32),
    pub level_pos: Vec<(i32, String)>,
    pub base_check: Option<Rect>,
    pub base_label_pos: (i32, i32),
    /// Column headings drawn once above the rows.
    pub tiering_head_pos: (i32, i32),
    pub scoring_head_pos: (i32, i32),
    /// One entry per VISIBLE row, parallel to `visible_rows`.
    pub rows: Vec<RowGeom>,
    /// Indices into `Panel::rows` that `rows` describes.
    pub visible_rows: Vec<usize>,
    /// "Show N more" / "Hide" toggle, when the item has hidden rows.
    pub hidden_toggle: Option<(Rect, String)>,
    pub strictness: Vec<(Rect, Strictness)>,
    pub estimate_box: Option<Rect>,
    pub buttons: Vec<(Rect, Action, &'static str)>,
    pub status_pos: (i32, i32),
    pub listing_pos: Vec<(i32, i32)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RowGeom {
    pub check: Rect,
    /// Left gutter badge baseline; drawn only when the row has a badge.
    pub badge_pos: (i32, i32),
    pub label_pos: (i32, i32),
    /// Right gutter score baseline.
    pub score_pos: (i32, i32),
    pub min_box: Rect,
    pub max_box: Rect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Min,
    Max,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    ToggleRow(usize),
    ToggleBase,
    Edit(usize, Field),
    SetStrictness(Strictness),
    ToggleHidden,
    Search,
    OpenSite,
    Close,
}

/// Text drawn in the left gutter for a badge. "Other" affixes have no tier
/// ladder worth naming, so they get a dash rather than a made-up "O3".
pub fn badge_text(b: &TierBadge) -> String {
    match b.kind {
        AffixKind::Other => "—".to_string(),
        _ => format!("{}{}", b.kind.letter(), b.tier),
    }
}

/// Right-gutter score text; one decimal keeps the column narrow and stops
/// float noise from implying more precision than the ladder gives.
pub fn score_text(s: f32) -> String {
    format!("{s:.1}")
}

impl Panel {
    /// Rows currently drawn: everything, minus the collapsed ones.
    pub fn visible_rows(&self) -> Vec<usize> {
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, r)| !r.hidden || self.show_hidden)
            .map(|(i, _)| i)
            .collect()
    }

    pub fn hidden_count(&self) -> usize {
        self.rows.iter().filter(|r| r.hidden).count()
    }
}

fn content_width(panel: &Panel, measure: &dyn Fn(&str) -> i32) -> i32 {
    // Every row is measured, hidden ones included, so expanding "Show N more"
    // cannot reflow the card out from under the cursor.
    let label_w = panel
        .rows
        .iter()
        .map(|r| measure(&r.label))
        .chain(panel.header.base.iter().map(|b| measure(&b.label)))
        .max()
        .unwrap_or(0)
        .max(LABEL_MIN_W);
    // The name draws in the large font; approximate its advance so a long
    // rare name does not overrun the close box.
    let name_w = measure(&panel.header.name) * 3 / 2 + CLOSE + GAP;
    let other_chars = panel
        .listings
        .iter()
        .map(|l| l.chars().count())
        .chain(panel.estimate.iter().map(|e| e.detail.chars().count()))
        .chain([panel.status.chars().count()])
        .max()
        .unwrap_or(10) as i32;
    let row_w = PAD
        + GUT_L
        + CHECK
        + GAP
        + label_w
        + GAP
        + GUT_R
        + GAP
        + BOX_W * 2
        + BOX_GAP
        + PAD;
    let other_w = PAD + other_chars * CHAR_W + PAD;
    row_w.max(other_w).max(PAD + name_w + PAD).clamp(WIDTH_MIN, WIDTH_MAX)
}

pub fn layout(panel: &Panel, measure: &dyn Fn(&str) -> i32) -> Layout {
    let w = content_width(panel, measure);
    let h = &panel.header;

    // --- title bar -------------------------------------------------------
    let name_pos = (PAD, PAD + TITLE_H - 10);
    let close = Rect { x: w - PAD - CLOSE, y: PAD, w: CLOSE as u32, h: CLOSE as u32 };
    let mut y = PAD + TITLE_H;

    // --- header block: rarity, then only the level lines the item has ----
    let rarity_pos = (PAD, y + LINE_H - 5);
    y += LINE_H;
    let mut level_pos = Vec::new();
    if let Some(il) = h.item_level {
        level_pos.push((y + LINE_H - 5, format!("Item Level: {il}")));
        y += LINE_H;
    }
    if let Some(rl) = h.requires_level {
        level_pos.push((y + LINE_H - 5, format!("Requires Level: {rl}")));
        y += LINE_H;
    }

    // --- base-type toggle, on its own line under the header --------------
    let (base_check, base_label_pos) = if h.base.is_some() {
        y += BLOCK_GAP;
        let check = Rect { x: PAD, y: y + (ROW_H - CHECK) / 2, w: CHECK as u32, h: CHECK as u32 };
        let label_pos = (PAD + CHECK + GAP, y + ROW_H - 8);
        y += ROW_H;
        (Some(check), label_pos)
    } else {
        (None, (0, 0))
    };

    // --- column geometry, shared by the headings and every row -----------
    let check_x = PAD + GUT_L;
    let label_x = check_x + CHECK + GAP;
    let max_box_x = w - PAD - BOX_W;
    let min_box_x = max_box_x - BOX_GAP - BOX_W;
    let score_x = min_box_x - GAP - GUT_R;

    y += BLOCK_GAP;
    let head_baseline = y + HEAD_H - 6;
    let tiering_head_pos = (PAD, head_baseline);
    let scoring_head_pos = (score_x, head_baseline);
    y += HEAD_H;

    // --- row band --------------------------------------------------------
    let visible_rows = panel.visible_rows();
    let mut rows = Vec::with_capacity(visible_rows.len());
    for _ in &visible_rows {
        let baseline = y + ROW_H - 8;
        rows.push(RowGeom {
            check: Rect {
                x: check_x,
                y: y + (ROW_H - CHECK) / 2,
                w: CHECK as u32,
                h: CHECK as u32,
            },
            badge_pos: (PAD, baseline),
            label_pos: (label_x, baseline),
            score_pos: (score_x, baseline),
            min_box: Rect {
                x: min_box_x,
                y: y + (ROW_H - BOX_H) / 2,
                w: BOX_W as u32,
                h: BOX_H as u32,
            },
            max_box: Rect {
                x: max_box_x,
                y: y + (ROW_H - BOX_H) / 2,
                w: BOX_W as u32,
                h: BOX_H as u32,
            },
        });
        y += ROW_H;
    }

    // --- collapse toggle, only when there is something collapsed ---------
    let nhidden = panel.hidden_count();
    let hidden_toggle = (nhidden > 0).then(|| {
        let text = if panel.show_hidden {
            format!("Hide {nhidden}")
        } else {
            format!("Show {nhidden} more")
        };
        let r = Rect {
            x: label_x,
            y,
            w: (measure(&text) + GAP * 2).max(60) as u32,
            h: TOGGLE_H as u32,
        };
        y += TOGGLE_H;
        (r, text)
    });

    // --- strictness radios ----------------------------------------------
    y += BLOCK_GAP;
    let mut strictness = Vec::new();
    let mut rx = PAD;
    for s in [Strictness::Quick, Strictness::Broad] {
        let rw = CHECK + GAP + measure(s.label()) + GAP;
        strictness.push((Rect { x: rx, y, w: rw as u32, h: RADIO_H as u32 }, s));
        rx += rw + RADIO_GAP;
    }
    y += RADIO_H + 8;

    // --- the answer, then the actions ------------------------------------
    let estimate_box = panel.estimate.as_ref().map(|_| {
        let r = Rect { x: PAD, y, w: (w - PAD * 2) as u32, h: EST_H as u32 };
        y += EST_H + 8;
        r
    });
    let buttons = vec![
        (Rect { x: PAD, y, w: 110, h: BTN_H as u32 }, Action::Search, "Search"),
        (Rect { x: PAD + 110 + BTN_GAP, y, w: 130, h: BTN_H as u32 }, Action::OpenSite, "Open site"),
    ];
    let status_pos = (PAD + 110 + BTN_GAP + 130 + BTN_GAP, y + BTN_H - 9);
    y += BTN_H + 6;
    let mut listing_pos = Vec::new();
    for _ in &panel.listings {
        listing_pos.push((PAD, y + LISTING_H - 6));
        y += LISTING_H;
    }

    Layout {
        size: (w, y + PAD),
        close,
        name_pos,
        rarity_pos,
        level_pos,
        base_check,
        base_label_pos,
        tiering_head_pos,
        scoring_head_pos,
        rows,
        visible_rows,
        hidden_toggle,
        strictness,
        estimate_box,
        buttons,
        status_pos,
        listing_pos,
    }
}

fn inside(r: &Rect, x: i32, y: i32) -> bool {
    x >= r.x && x < r.x + r.w as i32 && y >= r.y && y < r.y + r.h as i32
}

pub fn hit(panel: &Panel, lay: &Layout, x: i32, y: i32) -> Option<Action> {
    if inside(&lay.close, x, y) {
        return Some(Action::Close);
    }
    for (rect, action, _) in &lay.buttons {
        if inside(rect, x, y) {
            return Some(*action);
        }
    }
    // Base-type row: the whole line, checkbox and label alike, toggles it.
    if let Some(check) = &lay.base_check {
        let row = Rect {
            x: PAD,
            y: check.y - (ROW_H - CHECK) / 2,
            w: (lay.size.0 - 2 * PAD) as u32,
            h: ROW_H as u32,
        };
        if inside(&row, x, y) {
            return Some(Action::ToggleBase);
        }
    }
    if let Some((rect, _)) = &lay.hidden_toggle {
        if inside(rect, x, y) {
            return Some(Action::ToggleHidden);
        }
    }
    for (rect, s) in &lay.strictness {
        if inside(rect, x, y) {
            return Some(Action::SetStrictness(*s));
        }
    }
    for (i, g) in lay.rows.iter().enumerate() {
        // Actions carry the index into `panel.rows`, never the visible
        // position: collapsing rows must not renumber what a click means.
        let idx = lay.visible_rows[i];
        let Some(target) = panel.rows[idx].target else {
            // Display-only line (unmatched mod, unsearchable stat): it has
            // no filter to drive, so it swallows nothing and offers nothing.
            continue;
        };
        if inside(&g.min_box, x, y) {
            return Some(Action::Edit(idx, Field::Min));
        }
        // Weapon bounds are minimums only; their max box is not drawn and
        // must not be editable.
        if matches!(target, Target::Stat(_)) && inside(&g.max_box, x, y) {
            return Some(Action::Edit(idx, Field::Max));
        }
        let band = Rect {
            x: PAD,
            y: g.check.y - (ROW_H - CHECK) / 2,
            w: (g.min_box.x - BOX_GAP - PAD).max(CHECK) as u32,
            h: ROW_H as u32,
        };
        if inside(&band, x, y) {
            return Some(Action::ToggleRow(idx));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stand-in measurer: a fixed advance per char, enough to exercise the
    /// geometry without a font.
    fn m(s: &str) -> i32 {
        7 * s.len() as i32
    }

    fn row(i: usize) -> StatRow {
        StatRow {
            label: format!("{i} to Accuracy Rating"),
            badge: Some(TierBadge { kind: AffixKind::Suffix, tier: 3 }),
            score: Some(4.0),
            min: i as f64,
            max: None,
            enabled: true,
            target: Some(Target::Stat(i)),
            hidden: false,
        }
    }

    fn hidden(i: usize) -> StatRow {
        StatRow { hidden: true, ..row(i) }
    }

    /// A derived line (DPS, total attributes): drawn, never searched.
    fn display_only(i: usize) -> StatRow {
        StatRow { target: None, badge: None, score: None, ..row(i) }
    }

    fn panel(rows: Vec<StatRow>) -> Panel {
        Panel {
            header: ItemHeader {
                name: "Horror Bane".into(),
                rarity: "Rare".into(),
                item_level: Some(82),
                requires_level: Some(65),
                base: None,
            },
            rows,
            show_hidden: false,
            strictness: Strictness::Quick,
            estimate: None,
            listings: Vec::new(),
            status: String::new(),
            search_id: None,
        }
    }

    fn estimate() -> EstimateView {
        EstimateView {
            amount: "5.5".into(),
            denom: crate::pricing::Denom::Divine,
            detail: "Range: 0.62-49 div  -  from 23 listing(s)".into(),
            reliability: "Very Low".into(),
            shaky: true,
        }
    }

    #[test]
    fn hidden_rows_stay_collapsed_until_the_toggle_is_flipped() {
        let mut p = panel(vec![row(0), hidden(1), hidden(2), row(3)]);
        let lay = layout(&p, &m);
        assert_eq!(lay.visible_rows, vec![0, 3]);
        assert_eq!(lay.rows.len(), lay.visible_rows.len());
        let (_, label) = lay.hidden_toggle.clone().expect("two rows are hidden");
        assert_eq!(label, "Show 2 more");

        p.show_hidden = true;
        let lay = layout(&p, &m);
        assert_eq!(lay.visible_rows, vec![0, 1, 2, 3]);
        assert_eq!(lay.rows.len(), 4);
        assert_eq!(lay.hidden_toggle.expect("still collapsible").1, "Hide 2");
    }

    #[test]
    fn no_toggle_when_nothing_is_hidden() {
        assert!(layout(&panel(vec![row(0), row(1)]), &m).hidden_toggle.is_none());
    }

    #[test]
    fn clicks_resolve_to_the_original_row_index_not_the_visible_one() {
        // Hidden rows interleaved: visible position 1 is panel row 3.
        let p = panel(vec![row(0), hidden(1), hidden(2), row(3)]);
        let lay = layout(&p, &m);
        let g = &lay.rows[1];
        assert_eq!(hit(&p, &lay, g.check.x + 2, g.check.y + 2), Some(Action::ToggleRow(3)));
        assert_eq!(
            hit(&p, &lay, g.min_box.x + 2, g.min_box.y + 2),
            Some(Action::Edit(3, Field::Min))
        );
        assert_eq!(
            hit(&p, &lay, g.max_box.x + 2, g.max_box.y + 2),
            Some(Action::Edit(3, Field::Max))
        );
    }

    #[test]
    fn display_only_rows_are_inert() {
        let p = panel(vec![display_only(0), row(1)]);
        let lay = layout(&p, &m);
        let g = &lay.rows[0];
        assert_eq!(hit(&p, &lay, g.check.x + 2, g.check.y + 2), None);
        assert_eq!(hit(&p, &lay, g.min_box.x + 2, g.min_box.y + 2), None);
        assert_eq!(hit(&p, &lay, g.max_box.x + 2, g.max_box.y + 2), None);
        // The searchable row below it still works.
        let g = &lay.rows[1];
        assert_eq!(hit(&p, &lay, g.check.x + 2, g.check.y + 2), Some(Action::ToggleRow(1)));
    }

    #[test]
    fn hidden_toggle_hit_tests() {
        let p = panel(vec![row(0), hidden(1)]);
        let lay = layout(&p, &m);
        let (r, _) = lay.hidden_toggle.clone().unwrap();
        assert_eq!(hit(&p, &lay, r.x + 3, r.y + 3), Some(Action::ToggleHidden));
    }

    #[test]
    fn strictness_rects_resolve_to_their_own_variant() {
        let p = panel(vec![row(0)]);
        let lay = layout(&p, &m);
        assert_eq!(lay.strictness.len(), 2);
        for (r, s) in &lay.strictness {
            assert_eq!(hit(&p, &lay, r.x + 2, r.y + 2), Some(Action::SetStrictness(*s)));
        }
        assert_eq!(lay.strictness[0].1, Strictness::Quick);
        assert_eq!(lay.strictness[1].1, Strictness::Broad);
        // Distinct targets, not stacked on one another.
        assert!(lay.strictness[1].0.x >= lay.strictness[0].0.x + lay.strictness[0].0.w as i32);
    }

    #[test]
    fn the_value_box_only_takes_space_when_there_is_a_value() {
        let p = panel(vec![row(0)]);
        let bare = layout(&p, &m);
        assert!(bare.estimate_box.is_none());

        let mut with = p.clone();
        with.estimate = Some(estimate());
        let lay = layout(&with, &m);
        let b = lay.estimate_box.expect("a value means a box");
        assert!(lay.size.1 > bare.size.1, "the box must claim vertical space");
        assert!(b.y > lay.rows[0].check.y, "the box follows the rows");
        assert!(b.x + b.w as i32 <= lay.size.0, "the box fits the panel width");
        // Everything below shifts down by exactly the box and its gap.
        let d = lay.buttons[0].0.y - bare.buttons[0].0.y;
        assert_eq!(d, EST_H + 8);
        assert!(lay.buttons.iter().all(|(r, _, _)| r.y > b.y), "buttons follow the box");
    }

    #[test]
    fn headings_and_gutters_frame_the_card() {
        let p = panel(vec![row(0)]);
        let lay = layout(&p, &m);
        let g = &lay.rows[0];
        // TIERING sits over the badge column, SCORING over the score column.
        assert_eq!(lay.tiering_head_pos.0, g.badge_pos.0);
        assert_eq!(lay.scoring_head_pos.0, g.score_pos.0);
        assert!(lay.tiering_head_pos.1 < g.label_pos.1, "headings sit above the rows");
        // Left gutter, checkbox, label, right gutter, then the boxes.
        assert!(g.badge_pos.0 < g.check.x);
        assert!(g.check.x + CHECK <= g.label_pos.0);
        assert!(g.label_pos.0 < g.score_pos.0);
        assert!(g.score_pos.0 + GUT_R <= g.min_box.x);
        assert!(g.min_box.x + BOX_W <= g.max_box.x);
        assert!(g.max_box.x + BOX_W as i32 + PAD <= lay.size.0);
    }

    #[test]
    fn header_lines_appear_only_when_the_item_has_them() {
        let mut p = panel(vec![row(0)]);
        assert_eq!(layout(&p, &m).level_pos.len(), 2);
        p.header.requires_level = None;
        let lay = layout(&p, &m);
        assert_eq!(lay.level_pos.len(), 1);
        assert!(lay.level_pos[0].1.contains("82"));
        p.header.item_level = None;
        assert!(layout(&p, &m).level_pos.is_empty());
    }

    #[test]
    fn base_toggle_row_sits_above_the_rows_and_hit_tests() {
        let mut p = panel(vec![row(0)]);
        assert!(layout(&p, &m).base_check.is_none());
        p.header.base = Some(BaseToggle { label: "Advanced Zealot Bow".into(), enabled: true });
        let lay = layout(&p, &m);
        let c = lay.base_check.expect("base row present");
        assert!(c.y < lay.rows[0].check.y);
        assert_eq!(hit(&p, &lay, c.x + 1, c.y + 1), Some(Action::ToggleBase));
    }

    #[test]
    fn width_grows_with_the_longest_label() {
        let narrow = layout(&panel(vec![row(0)]), &m).size.0;
        let mut wide = panel(vec![row(0)]);
        wide.rows[0].label =
            "a very long mod description that must widen the panel considerably".into();
        assert!(layout(&wide, &m).size.0 > narrow);
        // Collapsed rows are measured too, so expanding never reflows.
        let mut collapsed = panel(vec![row(0), hidden(1)]);
        collapsed.rows[1].label = wide.rows[0].label.clone();
        let a = layout(&collapsed, &m).size.0;
        collapsed.show_hidden = true;
        assert_eq!(a, layout(&collapsed, &m).size.0);
    }

    #[test]
    fn buttons_and_close_resolve() {
        let p = panel(vec![row(0)]);
        let lay = layout(&p, &m);
        assert_eq!(hit(&p, &lay, lay.buttons[0].0.x + 5, lay.buttons[0].0.y + 5), Some(Action::Search));
        assert_eq!(hit(&p, &lay, lay.buttons[1].0.x + 5, lay.buttons[1].0.y + 5), Some(Action::OpenSite));
        assert_eq!(hit(&p, &lay, lay.close.x + 5, lay.close.y + 5), Some(Action::Close));
    }

    #[test]
    fn dead_space_is_dead() {
        let p = panel(vec![row(0)]);
        let lay = layout(&p, &m);
        // The rarity/level text block carries no controls.
        assert_eq!(hit(&p, &lay, lay.rarity_pos.0 + 2, lay.rarity_pos.1 - 4), None);
        // Nor does the strip to the right of a row's boxes.
        assert_eq!(hit(&p, &lay, lay.size.0 - 2, lay.rows[0].check.y + 2), None);
    }

    #[test]
    fn badge_and_score_text_stay_honest() {
        assert_eq!(badge_text(&TierBadge { kind: AffixKind::Prefix, tier: 9 }), "P9");
        assert_eq!(badge_text(&TierBadge { kind: AffixKind::Suffix, tier: 1 }), "S1");
        assert_eq!(badge_text(&TierBadge { kind: AffixKind::Other, tier: 1 }), "—");
        assert_eq!(score_text(0.75), "0.8");
    }
}
