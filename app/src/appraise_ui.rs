//! Interactive rare-appraisal panel: pure model, layout, and hit-testing.
//! The renderer draws from THIS geometry and the click handler resolves
//! actions from THIS geometry, so pixels and hitboxes cannot drift apart.
//! All coordinates are panel-local logical pixels; the caller offsets by
//! the panel's placed position.

use crate::config::Rect;

pub const PANEL_W: i32 = 460;
const PAD: i32 = 12;
const TITLE_H: i32 = 30;
const ROW_H: i32 = 26;
const CHECK: i32 = 16;
const BTN_H: i32 = 28;
const BTN_GAP: i32 = 10;
const LISTING_H: i32 = 22;
const CLOSE: i32 = 20;

#[derive(Debug, Clone, PartialEq)]
pub struct ModRow {
    /// Human mod text from the item.
    pub label: String,
    pub tier: Option<u8>,
    /// The search floor for this mod (from the item's tier-range
    /// annotation), shown so the user knows what "on" means.
    pub min: i64,
    pub enabled: bool,
    /// Index into the Query's filters vec this row controls.
    pub filter_index: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Panel {
    pub title: String,
    pub mods: Vec<ModRow>,
    /// Pre-formatted listing lines ("2.5 exalted | account | 3d").
    pub listings: Vec<String>,
    /// One-line state: "12 matches", "searching...", "trade cooldown 30s".
    pub status: String,
    pub search_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    ToggleMod(usize),
    Search,
    OpenSite,
    Close,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub size: (i32, i32),
    /// Checkbox rect + label baseline-left position per mod row.
    pub rows: Vec<(Rect, (i32, i32))>,
    /// Buttons as (rect, action, label).
    pub buttons: Vec<(Rect, Action, &'static str)>,
    pub close: Rect,
    /// Baseline-left of the status line.
    pub status_pos: (i32, i32),
    /// Baseline-left per listing line.
    pub listing_pos: Vec<(i32, i32)>,
}

pub fn layout(panel: &Panel) -> Layout {
    let mut y = PAD + TITLE_H;
    let mut rows = Vec::new();
    for _ in &panel.mods {
        let check = Rect {
            x: PAD,
            y: y + (ROW_H - CHECK) / 2,
            w: CHECK as u32,
            h: CHECK as u32,
        };
        rows.push((check, (PAD + CHECK + 8, y + ROW_H - 7)));
        y += ROW_H;
    }
    y += 6;
    let buttons = vec![
        (
            Rect { x: PAD, y, w: 110, h: BTN_H as u32 },
            Action::Search,
            "Search",
        ),
        (
            Rect { x: PAD + 110 + BTN_GAP, y, w: 130, h: BTN_H as u32 },
            Action::OpenSite,
            "Open site",
        ),
    ];
    let status_pos = (PAD + 110 + BTN_GAP + 130 + BTN_GAP, y + BTN_H - 9);
    y += BTN_H + 6;
    let mut listing_pos = Vec::new();
    for _ in &panel.listings {
        listing_pos.push((PAD, y + LISTING_H - 6));
        y += LISTING_H;
    }
    let close = Rect {
        x: PANEL_W - PAD - CLOSE,
        y: PAD,
        w: CLOSE as u32,
        h: CLOSE as u32,
    };
    Layout {
        size: (PANEL_W, y + PAD),
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

/// Resolves a click at panel-local (x, y). Checkbox rows accept the whole
/// row width, not just the small box, because a 16px box is a miserable
/// click target mid-game.
pub fn hit(panel: &Panel, lay: &Layout, x: i32, y: i32) -> Option<Action> {
    if inside(&lay.close, x, y) {
        return Some(Action::Close);
    }
    for (rect, action, _) in &lay.buttons {
        if inside(rect, x, y) {
            return Some(*action);
        }
    }
    for (i, (check, _)) in lay.rows.iter().enumerate() {
        let row = Rect {
            x: PAD,
            y: check.y - (ROW_H - CHECK) / 2,
            w: (PANEL_W - 2 * PAD) as u32,
            h: ROW_H as u32,
        };
        if inside(&row, x, y) {
            let _ = i;
            return Some(Action::ToggleMod(panel.mods[i].filter_index));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel(nmods: usize, nlist: usize) -> Panel {
        Panel {
            title: "Horror Bane".into(),
            mods: (0..nmods)
                .map(|i| ModRow {
                    label: format!("mod {i}"),
                    tier: Some(3),
                    min: 40,
                    enabled: true,
                    filter_index: i,
                })
                .collect(),
            listings: (0..nlist).map(|i| format!("listing {i}")).collect(),
            status: String::new(),
            search_id: None,
        }
    }

    #[test]
    fn clicking_a_mod_row_toggles_that_filter() {
        let p = panel(3, 0);
        let lay = layout(&p);
        let (check, _) = &lay.rows[1];
        let a = hit(&p, &lay, check.x + 2, check.y + 2);
        assert_eq!(a, Some(Action::ToggleMod(1)));
        // Anywhere along the row counts, not just the box.
        let a = hit(&p, &lay, PANEL_W - PAD - 40, check.y + 2);
        assert_eq!(a, Some(Action::ToggleMod(1)));
    }

    #[test]
    fn buttons_and_close_resolve() {
        let p = panel(2, 0);
        let lay = layout(&p);
        let (r, _, _) = &lay.buttons[0];
        assert_eq!(hit(&p, &lay, r.x + 5, r.y + 5), Some(Action::Search));
        let (r, _, _) = &lay.buttons[1];
        assert_eq!(hit(&p, &lay, r.x + 5, r.y + 5), Some(Action::OpenSite));
        assert_eq!(hit(&p, &lay, lay.close.x + 5, lay.close.y + 5), Some(Action::Close));
    }

    #[test]
    fn empty_space_is_no_action() {
        let p = panel(2, 3);
        let lay = layout(&p);
        assert_eq!(hit(&p, &lay, PANEL_W / 2, lay.size.1 - 4), None);
    }

    #[test]
    fn height_grows_with_mods_and_listings() {
        let short = layout(&panel(1, 0)).size.1;
        let tall = layout(&panel(4, 5)).size.1;
        assert_eq!(tall - short, 3 * ROW_H + 5 * LISTING_H);
    }

    #[test]
    fn filter_index_is_reported_not_row_index() {
        let mut p = panel(2, 0);
        p.mods[0].filter_index = 7;
        let lay = layout(&p);
        let (check, _) = &lay.rows[0];
        assert_eq!(hit(&p, &lay, check.x + 1, check.y + 1), Some(Action::ToggleMod(7)));
    }
}
