//! Reference-browser panel: pure model, layout, and hit-testing. The renderer
//! draws from THIS geometry and the click handler resolves actions from THIS
//! geometry, so pixels and hitboxes cannot drift apart. All coordinates are
//! panel-local logical pixels; the caller offsets by the panel's placed
//! position.
//!
//! A search box sits under the title, category pills under that (wrapping to
//! more rows when they overflow the panel width), then a fixed window of
//! result rows. The rows are informational: clicks on them resolve nothing,
//! and scrolling is keyboard-only (Up/Down) via the `visible` window — no
//! pixel scrolling, so the layout stays a pure function of the model.

use crate::config::Rect;
use crate::refcache::{self, Reference};
use poe2_lens_core::refdata;

const PAD: i32 = 12;
const TITLE_H: i32 = 30;
const CLOSE: i32 = 20;
const SEARCH_H: i32 = 24;
const PILL_H: i32 = 20;
const PILL_PAD_X: i32 = 8;
const PILL_GAP: i32 = 6;
const ROW_H: i32 = 22;
const VISIBLE_ROWS: usize = 14;
const WIDTH_MIN: i32 = 420;
const WIDTH_MAX: i32 = 900;
/// One-line rows: anything longer is cut here so a stray long mod cannot
/// force the panel to its max width for every search.
const ROW_MAX_CHARS: usize = 90;
/// Cap results so a blank query over the full affix index stays cheap to
/// clone into FrameState every frame.
const MAX_ROWS: usize = 100;

/// Browsable category. `Xile(i)` indexes `refcache::XILE_CATEGORIES`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cat {
    Affixes,
    Bases,
    Uniques,
    Gems,
    Keystones,
    Xile(usize),
}

/// The pill label the renderer draws; xile pills show their API slug.
pub fn cat_label(cat: Cat) -> &'static str {
    match cat {
        Cat::Affixes => "Affixes",
        Cat::Bases => "Bases",
        Cat::Uniques => "Uniques",
        Cat::Gems => "Gems",
        Cat::Keystones => "Keystones",
        Cat::Xile(i) => refcache::XILE_CATEGORIES.get(i).map(|(slug, _)| *slug).unwrap_or("?"),
    }
}

/// Every pill in display order: the five fixed categories then the xile ones.
fn all_cats() -> impl Iterator<Item = Cat> {
    [Cat::Affixes, Cat::Bases, Cat::Uniques, Cat::Gems, Cat::Keystones]
        .into_iter()
        .chain((0..refcache::XILE_CATEGORIES.len()).map(Cat::Xile))
}

#[derive(Debug, Clone, PartialEq)]
pub struct Panel {
    pub query: String,
    pub cat: Cat,
    pub rows: Vec<String>,
    pub scroll: usize,
    pub focused: bool,
}

impl Default for Panel {
    fn default() -> Self {
        // Opens ready to type: search focused, broadest category selected.
        Panel { query: String::new(), cat: Cat::Affixes, rows: Vec::new(), scroll: 0, focused: true }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Close,
    FocusSearch,
    SetCat(Cat),
    ScrollUp,
    ScrollDown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub w: i32,
    pub h: i32,
    pub close: Rect,
    pub search: Rect,
    pub pills: Vec<(Rect, Cat)>,
    /// One rect per VISIBLE row, parallel to `visible`.
    pub rows: Vec<Rect>,
    pub visible: std::ops::Range<usize>,
}

/// The visible window: scroll clamped so it can never leave the tail (a
/// shrinking result set after a refresh must not strand the view past the
/// end).
fn visible_window(p: &Panel) -> std::ops::Range<usize> {
    let scroll = p.scroll.min(p.rows.len().saturating_sub(1));
    scroll..(scroll + VISIBLE_ROWS).min(p.rows.len())
}

pub fn layout(p: &Panel, measure: &dyn Fn(&str) -> i32) -> Layout {
    let visible = visible_window(p);
    // Sized to the longest row actually on screen, not the whole result set:
    // scrolling may change the width, but off-screen text never does.
    let row_w = p.rows[visible.clone()].iter().map(|r| measure(r)).max().unwrap_or(0);
    let w = (PAD + row_w + PAD).clamp(WIDTH_MIN, WIDTH_MAX);

    let close = Rect { x: w - PAD - CLOSE, y: PAD, w: CLOSE as u32, h: CLOSE as u32 };
    let mut y = PAD + TITLE_H;
    let search = Rect { x: PAD, y, w: (w - 2 * PAD) as u32, h: SEARCH_H as u32 };
    y += SEARCH_H + PILL_GAP;

    // Pills flow left-to-right and wrap when the next one would cross the
    // right padding; 13 pills rarely fit one row at the minimum width.
    let mut pills = Vec::new();
    let mut x = PAD;
    for cat in all_cats() {
        let pw = measure(cat_label(cat)) + 2 * PILL_PAD_X;
        if x > PAD && x + pw > w - PAD {
            x = PAD;
            y += PILL_H + PILL_GAP;
        }
        pills.push((Rect { x, y, w: pw as u32, h: PILL_H as u32 }, cat));
        x += pw + PILL_GAP;
    }
    y += PILL_H + PILL_GAP;

    let mut rows = Vec::new();
    for _ in visible.clone() {
        rows.push(Rect { x: PAD, y, w: (w - 2 * PAD) as u32, h: ROW_H as u32 });
        y += ROW_H;
    }
    Layout { w, h: y + PAD, close, search, pills, rows, visible }
}

fn inside(r: &Rect, x: i32, y: i32) -> bool {
    x >= r.x && x < r.x + r.w as i32 && y >= r.y && y < r.y + r.h as i32
}

/// Click resolution. Result rows are informational and scrolling is
/// keyboard-only (the caller maps Up/Down keys to ScrollUp/ScrollDown), so
/// clicks resolve only on the close box, the search box, and the pills.
pub fn hit(_p: &Panel, lay: &Layout, x: i32, y: i32) -> Option<Action> {
    if inside(&lay.close, x, y) {
        return Some(Action::Close);
    }
    if inside(&lay.search, x, y) {
        return Some(Action::FocusSearch);
    }
    for (rect, cat) in &lay.pills {
        if inside(rect, x, y) {
            return Some(Action::SetCat(*cat));
        }
    }
    None
}

/// Cuts a row to one drawable line (results can carry multi-line effect text).
fn one_line(s: &str) -> String {
    let flat = s.split('\n').map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" · ");
    if flat.chars().count() <= ROW_MAX_CHARS {
        return flat;
    }
    let mut cut: String = flat.chars().take(ROW_MAX_CHARS - 1).collect();
    cut.push('…');
    cut
}

/// "Name — Category" when the catalog knows the class, bare name otherwise.
fn item_row(i: &refdata::RefItem) -> String {
    match &i.category {
        Some(c) if !c.is_empty() => format!("{} — {}", i.name, c),
        _ => i.name.clone(),
    }
}

/// Re-runs the search for the panel's (cat, query) against the loaded
/// reference data. Rows are capped and scroll reset: results changed, so the
/// old window position is meaningless.
pub fn refresh(p: &mut Panel, r: &Reference) {
    let q = p.query.as_str();
    let rows: Vec<String> = match p.cat {
        Cat::Affixes => {
            refdata::search_affixes(&r.affixes, q).into_iter().map(|a| one_line(&a.text)).collect()
        }
        // The catalog mixes bases and gems under namespace ITEM/GEM; "Bases"
        // means everything craftable that is not a gem (uniques have their
        // own richer category).
        Cat::Bases => refdata::search_ref_items(&r.items, q, None)
            .into_iter()
            .filter(|i| i.namespace != "GEM" && i.namespace != "UNIQUE")
            .map(item_row)
            .collect(),
        Cat::Gems => {
            refdata::search_ref_items(&r.items, q, Some("GEM")).into_iter().map(item_row).collect()
        }
        Cat::Uniques => refdata::search_uniques(&r.uniques, q)
            .into_iter()
            .map(|u| {
                // First mod tagged on when it still fits one line, so a scan
                // down the list shows the signature effect without a detail
                // view.
                let head = format!("{} — {}", u.name, u.base);
                match u.mods.first() {
                    Some(m) if head.chars().count() + m.chars().count() + 3 <= ROW_MAX_CHARS => {
                        one_line(&format!("{head} · {m}"))
                    }
                    _ => one_line(&head),
                }
            })
            .collect(),
        Cat::Keystones => refdata::search_keystones(&r.keystones, q)
            .into_iter()
            .map(|k| one_line(&format!("{}: {}", k.name, k.description)))
            .collect(),
        Cat::Xile(i) => {
            let entries = refcache::XILE_CATEGORIES
                .get(i)
                .and_then(|(slug, _)| r.categories.get(*slug))
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            refdata::search_ref_entries(entries, q)
                .into_iter()
                .map(|e| match e.lines.first() {
                    Some(l) => one_line(&format!("{} — {}", e.name, l)),
                    None => one_line(&e.name),
                })
                .collect()
        }
    };
    p.rows = rows.into_iter().take(MAX_ROWS).collect();
    p.scroll = 0;
}
