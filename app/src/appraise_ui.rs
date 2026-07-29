//! Interactive rare-appraisal panel: pure model, layout, and hit-testing.
//! The renderer draws from THIS geometry and the click handler resolves
//! actions from THIS geometry, so pixels and hitboxes cannot drift apart.
//! All coordinates are panel-local logical pixels; the caller offsets by
//! the panel's placed position.
//!
//! Modelled on EE2: mods are grouped by tag (implicits first, then explicits,
//! then map), each row shows a colored tag chip, the mod text, and a min and
//! a max value box. Clicking a box focuses it for keyboard entry (the caller
//! drives the edit); the panel sizes to its content.

use crate::config::Rect;

const PAD: i32 = 12;
const TITLE_H: i32 = 30;
const ROW_H: i32 = 26;
const CHECK: i32 = 16;
const TAG_W: i32 = 60;
const TAG_GAP: i32 = 6;
const LABEL_GAP: i32 = 8;
const BOX_W: i32 = 48;
const BOX_H: i32 = 20;
const BOX_GAP: i32 = 6;
const GROUP_GAP: i32 = 10;
const BTN_H: i32 = 28;
const BTN_GAP: i32 = 10;
const LISTING_H: i32 = 22;
const CLOSE: i32 = 20;
const LABEL_MIN_W: i32 = 150;
const CHAR_W: i32 = 7;
const WIDTH_MIN: i32 = 440;
const WIDTH_MAX: i32 = 1600;

#[derive(Debug, Clone, PartialEq)]
pub struct ModRow {
    pub label: String,
    pub tier: Option<u8>,
    pub min: f64,
    pub max: Option<f64>,
    pub enabled: bool,
    /// Index into the Query's filters vec this row controls.
    pub filter_index: usize,
    /// Mod group: "implicit", "explicit", "map".
    pub tag: String,
}

/// The item's base/category constraint, shown as a toggle above the mods so
/// the user can drop it and price the rolls across every base (EE2's base-type
/// chip). `None` for items where the base is intrinsic to the price (waystones).
#[derive(Debug, Clone, PartialEq)]
pub struct BaseToggle {
    pub label: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Panel {
    pub title: String,
    pub base: Option<BaseToggle>,
    pub mods: Vec<ModRow>,
    pub listings: Vec<String>,
    pub status: String,
    pub search_id: Option<String>,
}

/// Which value box of a row is being edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Min,
    Max,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    ToggleMod(usize),
    /// Toggle the base/category constraint (mods-only search when off).
    ToggleBase,
    /// Focus a row's min/max box for keyboard entry (filter_index, field).
    Edit(usize, Field),
    Search,
    OpenSite,
    Close,
}

/// A mod row's interactive geometry.
#[derive(Debug, Clone, PartialEq)]
pub struct RowGeom {
    pub check: Rect,
    pub tag_pos: (i32, i32),
    pub label_pos: (i32, i32),
    pub min_box: Rect,
    pub max_box: Rect,
    /// True on the first row of each tag group (draw a separator above).
    pub group_start: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub size: (i32, i32),
    /// The base-type toggle checkbox and its label position, when the panel
    /// has a base row.
    pub base_check: Option<Rect>,
    pub base_label_pos: (i32, i32),
    pub rows: Vec<RowGeom>,
    pub buttons: Vec<(Rect, Action, &'static str)>,
    pub close: Rect,
    pub status_pos: (i32, i32),
    pub listing_pos: Vec<(i32, i32)>,
}

/// Formats a value-box number: whole numbers show without a decimal point
/// (`155`), fractional ones keep their digits (`3.5`). Matches how the search
/// body is serialized, so the box shows exactly what gets searched.
pub fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        // Trim to at most 2 decimals, then drop trailing zeros.
        let s = format!("{v:.2}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// Rank tags so implicits sort above explicits above map (EE2 order).
pub fn tag_rank(tag: &str) -> u8 {
    match tag {
        "implicit" => 0,
        "explicit" => 1,
        _ => 2,
    }
}

/// The string the renderer actually draws for a row: the tier prefix (if any)
/// followed by the mod text. Width must be measured against THIS, not the bare
/// label, or the tier prefix pushes text into the value boxes.
pub fn drawn_label(m: &ModRow) -> String {
    let tier = m.tier.map(|t| format!("T{t} ")).unwrap_or_default();
    format!("{tier}{}", m.label)
}

fn content_width(panel: &Panel, measure: &dyn Fn(&str) -> i32) -> i32 {
    // Widest actually-rendered label (tier prefix included), so the value-box
    // column always begins to the right of every mod's text. The base toggle's
    // label is measured too so it never runs under the box column.
    let label_w = panel
        .mods
        .iter()
        .map(|m| measure(&drawn_label(m)))
        .chain(panel.base.iter().map(|b| measure(&b.label)))
        .max()
        .unwrap_or(0)
        .max(LABEL_MIN_W);
    let other_chars = panel
        .listings
        .iter()
        .map(|l| l.chars().count())
        .chain([panel.title.chars().count(), panel.status.chars().count()])
        .max()
        .unwrap_or(10) as i32;
    let row_w =
        PAD + CHECK + TAG_GAP + TAG_W + LABEL_GAP + label_w + LABEL_GAP + BOX_W * 2 + BOX_GAP + PAD;
    let other_w = PAD + other_chars * CHAR_W + PAD;
    row_w.max(other_w).clamp(WIDTH_MIN, WIDTH_MAX)
}

pub fn layout(panel: &Panel, measure: &dyn Fn(&str) -> i32) -> Layout {
    let w = content_width(panel, measure);
    let mut y = PAD + TITLE_H;
    // Base-type toggle row (its own line just under the title).
    let (base_check, base_label_pos) = if panel.base.is_some() {
        let check = Rect { x: PAD, y: y + (ROW_H - CHECK) / 2, w: CHECK as u32, h: CHECK as u32 };
        let label_pos = (PAD + CHECK + TAG_GAP, y + ROW_H - 8);
        y += ROW_H + GROUP_GAP;
        (Some(check), label_pos)
    } else {
        (None, (0, 0))
    };
    let mut rows = Vec::new();
    let mut prev_tag: Option<&str> = None;
    for m in &panel.mods {
        let group_start = prev_tag.is_some_and(|t| t != m.tag);
        if group_start {
            y += GROUP_GAP;
        }
        prev_tag = Some(&m.tag);
        let check = Rect { x: PAD, y: y + (ROW_H - CHECK) / 2, w: CHECK as u32, h: CHECK as u32 };
        let tag_x = PAD + CHECK + TAG_GAP;
        let label_x = tag_x + TAG_W + LABEL_GAP;
        let baseline = y + ROW_H - 8;
        let max_box = Rect {
            x: w - PAD - BOX_W,
            y: y + (ROW_H - BOX_H) / 2,
            w: BOX_W as u32,
            h: BOX_H as u32,
        };
        let min_box = Rect { x: max_box.x - BOX_GAP - BOX_W, ..max_box };
        rows.push(RowGeom {
            check,
            tag_pos: (tag_x, baseline),
            label_pos: (label_x, baseline),
            min_box,
            max_box,
            group_start,
        });
        y += ROW_H;
    }
    y += 6;
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
    let close = Rect { x: w - PAD - CLOSE, y: PAD, w: CLOSE as u32, h: CLOSE as u32 };
    Layout {
        size: (w, y + PAD),
        base_check,
        base_label_pos,
        rows,
        buttons,
        close,
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
    // Base-toggle row: the whole line (checkbox + label) toggles it.
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
    for (i, g) in lay.rows.iter().enumerate() {
        let fi = panel.mods[i].filter_index;
        if inside(&g.min_box, x, y) {
            return Some(Action::Edit(fi, Field::Min));
        }
        if inside(&g.max_box, x, y) {
            return Some(Action::Edit(fi, Field::Max));
        }
        let toggle_w = (g.min_box.x - BOX_GAP - PAD).max(CHECK);
        let row = Rect {
            x: PAD,
            y: g.check.y - (ROW_H - CHECK) / 2,
            w: toggle_w as u32,
            h: ROW_H as u32,
        };
        if inside(&row, x, y) {
            return Some(Action::ToggleMod(fi));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in measurer: a fixed advance per char, enough to exercise the
    /// geometry without a font.
    fn m(s: &str) -> i32 {
        s.chars().count() as i32 * 8
    }

    fn row(i: usize, tag: &str) -> ModRow {
        ModRow {
            label: format!("mod {i}"),
            tier: Some(3),
            min: 40.0,
            max: None,
            enabled: true,
            filter_index: i,
            tag: tag.into(),
        }
    }

    fn panel(mods: Vec<ModRow>, nlist: usize) -> Panel {
        Panel {
            title: "Horror Bane".into(),
            base: None,
            mods,
            listings: (0..nlist).map(|i| format!("listing {i}")).collect(),
            status: String::new(),
            search_id: None,
        }
    }

    #[test]
    fn base_toggle_row_sits_above_the_mods_and_hit_tests() {
        let mut p = panel(vec![row(0, "explicit")], 0);
        p.base = Some(BaseToggle { label: "Bow".into(), enabled: true });
        let lay = layout(&p, &m);
        let check = lay.base_check.expect("base row present");
        // It is above the first mod row.
        assert!(check.y < lay.rows[0].check.y);
        assert_eq!(hit(&p, &lay, check.x + 1, check.y + 1), Some(Action::ToggleBase));
    }

    #[test]
    fn no_base_row_when_absent() {
        let p = panel(vec![row(0, "explicit")], 0);
        assert!(layout(&p, &m).base_check.is_none());
    }

    #[test]
    fn clicking_boxes_edits_min_and_max_and_row_toggles() {
        let p = panel(vec![row(0, "explicit"), row(1, "explicit")], 0);
        let lay = layout(&p, &m);
        let g = &lay.rows[1];
        assert_eq!(hit(&p, &lay, g.min_box.x + 2, g.min_box.y + 2), Some(Action::Edit(1, Field::Min)));
        assert_eq!(hit(&p, &lay, g.max_box.x + 2, g.max_box.y + 2), Some(Action::Edit(1, Field::Max)));
        assert_eq!(hit(&p, &lay, g.check.x + 1, g.check.y + 1), Some(Action::ToggleMod(1)));
    }

    #[test]
    fn a_group_change_starts_a_new_section_with_a_gap() {
        // implicit then explicit -> the explicit row is a group_start with extra gap.
        let p = panel(vec![row(0, "implicit"), row(1, "explicit")], 0);
        let lay = layout(&p, &m);
        assert!(!lay.rows[0].group_start);
        assert!(lay.rows[1].group_start, "explicit starts a new group");
        // The gap pushes the second row down by more than one ROW_H.
        assert!(lay.rows[1].check.y - lay.rows[0].check.y > ROW_H);
    }

    #[test]
    fn tag_rank_orders_implicit_before_explicit_before_map() {
        assert!(tag_rank("implicit") < tag_rank("explicit"));
        assert!(tag_rank("explicit") < tag_rank("map"));
    }

    #[test]
    fn width_grows_with_long_labels() {
        let narrow = layout(&panel(vec![row(0, "explicit")], 0), &m).size.0;
        let mut wide = panel(vec![row(0, "explicit")], 0);
        wide.mods[0].label = "a very long mod description that must widen the panel considerably".into();
        assert!(layout(&wide, &m).size.0 > narrow);
    }

    #[test]
    fn buttons_and_close_resolve() {
        let p = panel(vec![row(0, "explicit")], 0);
        let lay = layout(&p, &m);
        let (r, _, _) = &lay.buttons[0];
        assert_eq!(hit(&p, &lay, r.x + 5, r.y + 5), Some(Action::Search));
        assert_eq!(hit(&p, &lay, lay.close.x + 5, lay.close.y + 5), Some(Action::Close));
    }
}
