//! Reference-browser panel: pure model, layout, and hit-testing. The renderer
//! draws from THIS geometry and the click handler resolves actions from THIS
//! geometry, so pixels and hitboxes cannot drift apart. All coordinates are
//! panel-local logical pixels; the caller offsets by the panel's placed
//! position.
//!
//! A search box sits under the title, category pills under that (wrapping to
//! more rows when they overflow the panel width), a prefix/suffix filter row
//! when the category is Affixes, then a fixed window of result rows. An affix
//! row carries a P/S family badge in its left gutter and a dim meta column
//! (tier count, top-tier ilvl, spawn weight) on its right; clicking a row
//! toggles an inline tier ladder under it. Scrolling is keyboard-only
//! (Up/Down) via the `visible` window — no pixel scrolling, so the layout
//! stays a pure function of the model (expansion state included).
//!
//! Expansion is click-only by necessity: the platform `Key` enum has no
//! Right/Left, and the caller's key loop swallows Enter and closes the panel
//! on Escape, so no key can reach the model to toggle a row. The scroll
//! cursor (`Panel::scroll`, always the first visible row) is drawn as a
//! selection mark so Up/Down visibly walks the list.

use crate::config::Rect;
use crate::refcache::{self, Reference};
use khaloni_poe2_core::refdata;

const PAD: i32 = 12;
const TITLE_H: i32 = 30;
const CLOSE: i32 = 20;
const SEARCH_H: i32 = 24;
const PILL_H: i32 = 20;
const PILL_PAD_X: i32 = 8;
const PILL_GAP: i32 = 6;
const ROW_H: i32 = 22;
/// Ladder lines are denser than rows: they are detail under one row, not
/// peers of the rows around them.
const LADDER_H: i32 = 18;
/// Ladder lines start where row text does plus a step, so the ladder reads
/// as belonging to the row above it.
const LADDER_INDENT: i32 = GUT_BADGE + 10;
/// Left gutter: one badge glyph ("P"/"S"/"—") plus breathing room, fixed so
/// every badge lands on the same column and row text starts flush (the same
/// discipline as the Evaluate panel's gutter).
const GUT_BADGE: i32 = 18;
/// Minimum air between a row's text and its right-aligned meta column.
const META_GAP: i32 = 16;
const VISIBLE_ROWS: usize = 14;
const WIDTH_MIN: i32 = 420;
const WIDTH_MAX: i32 = 900;
/// One-line rows: anything longer is cut here so a stray long mod cannot
/// force the panel to its max width for every search.
const ROW_MAX_CHARS: usize = 90;
/// Cap results so a blank query over the full affix index stays cheap to
/// clone into FrameState every frame (each row is three Strings).
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

/// Prefix/suffix filter over affix rows. Composes with the text search:
/// `refresh` applies it before the row cap, so "Prefix" over "life" is every
/// prefix matching "life", not the prefixes of the first 100 matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Family {
    #[default]
    All,
    Prefix,
    Suffix,
}

/// Filter pills in display order.
pub const FAMILIES: [Family; 3] = [Family::All, Family::Prefix, Family::Suffix];

pub fn family_label(f: Family) -> &'static str {
    match f {
        Family::All => "All",
        Family::Prefix => "Prefix",
        Family::Suffix => "Suffix",
    }
}

/// Whether an affix passes the family filter. `Other` (unknown family) only
/// shows under All: a filter must never present a guess as a prefix/suffix.
fn family_matches(f: Family, kind: refdata::AffixKind) -> bool {
    match f {
        Family::All => true,
        Family::Prefix => kind == refdata::AffixKind::Prefix,
        Family::Suffix => kind == refdata::AffixKind::Suffix,
    }
}

/// One result row. Non-affix categories carry bare text (`kind` None, the
/// rest empty); affix rows add the family badge, the meta column, and the
/// tier ladder shown when the row is expanded.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Row {
    pub text: String,
    /// P/S gutter badge; `None` outside the Affixes category (no gutter
    /// meaning there), `Some(Other)` for an affix whose family is unknown
    /// (drawn as a dim em dash, never a guessed letter).
    pub kind: Option<refdata::AffixKind>,
    /// Right-aligned dim column, e.g. "T×5  i75  w1000": tier count,
    /// top-tier ilvl, spawn weight. Empty when nothing is known — a missing
    /// ladder must not draw as "T×0".
    pub meta: String,
    /// Tier ladder for the expanded view, one line per tier best-first
    /// ("T1  i82  (35-39)"), '\n'-joined. One String, not a Vec, because the
    /// whole Panel is cloned into FrameState every tick.
    pub ladder: String,
}

impl Row {
    fn plain(text: String) -> Self {
        Row { text, ..Row::default() }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Panel {
    pub query: String,
    pub cat: Cat,
    pub family: Family,
    pub rows: Vec<Row>,
    pub scroll: usize,
    /// Index into `rows` whose tier ladder is unfolded; cleared by `refresh`
    /// because a new result set invalidates the old index.
    pub expanded: Option<usize>,
    pub focused: bool,
}

impl Default for Panel {
    fn default() -> Self {
        // Opens ready to type: search focused, broadest category selected.
        Panel {
            query: String::new(),
            cat: Cat::Affixes,
            family: Family::All,
            rows: Vec::new(),
            scroll: 0,
            expanded: None,
            focused: true,
        }
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

/// Per-row geometry: the clickable rect plus the x columns the renderer
/// draws into (badge gutter, text start, right-aligned meta start).
#[derive(Debug, Clone, PartialEq)]
pub struct RowGeom {
    pub rect: Rect,
    pub badge_x: i32,
    pub text_x: i32,
    pub meta_x: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub w: i32,
    pub h: i32,
    pub close: Rect,
    pub search: Rect,
    pub pills: Vec<(Rect, Cat)>,
    /// Family filter pills; present only for the Affixes category (the only
    /// rows a prefix/suffix filter means anything for).
    pub fam_pills: Vec<(Rect, Family)>,
    /// One geom per VISIBLE row, parallel to `visible`.
    pub rows: Vec<RowGeom>,
    /// The visible slot (index into `rows`) whose ladder is unfolded; None
    /// when the expanded row is scrolled out of view or nothing is expanded.
    pub expanded_slot: Option<usize>,
    /// Rects for the unfolded ladder's lines, in tier order, sitting
    /// directly under `rows[expanded_slot]`.
    pub ladder: Vec<Rect>,
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
    // The ladder only exists on screen while its row does; everything else
    // keys off this one decision.
    let expanded = p.expanded.filter(|i| visible.contains(i));
    // Sized to the longest row actually on screen — badge gutter, text, and
    // meta column all take width, as do the unfolded ladder lines. Scrolling
    // may change the width, but off-screen text never does.
    let row_w = p.rows[visible.clone()]
        .iter()
        .map(|r| {
            let meta = if r.meta.is_empty() { 0 } else { META_GAP + measure(&r.meta) };
            GUT_BADGE + measure(&r.text) + meta
        })
        .max()
        .unwrap_or(0);
    let ladder_w = expanded
        .and_then(|i| p.rows.get(i))
        .map(|r| r.ladder.lines().map(|l| LADDER_INDENT + measure(l)).max().unwrap_or(0))
        .unwrap_or(0);
    let w = (PAD + row_w.max(ladder_w) + PAD).clamp(WIDTH_MIN, WIDTH_MAX);

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

    // Family filter pills, Affixes only. Three short labels always fit one
    // row at WIDTH_MIN, so no wrapping here.
    let mut fam_pills = Vec::new();
    if p.cat == Cat::Affixes {
        let mut x = PAD;
        for f in FAMILIES {
            let pw = measure(family_label(f)) + 2 * PILL_PAD_X;
            fam_pills.push((Rect { x, y, w: pw as u32, h: PILL_H as u32 }, f));
            x += pw + PILL_GAP;
        }
        y += PILL_H + PILL_GAP;
    }

    let mut rows = Vec::new();
    let mut ladder = Vec::new();
    let mut expanded_slot = None;
    for i in visible.clone() {
        let meta_w = p.rows.get(i).map(|r| measure(&r.meta)).unwrap_or(0);
        rows.push(RowGeom {
            rect: Rect { x: PAD, y, w: (w - 2 * PAD) as u32, h: ROW_H as u32 },
            badge_x: PAD,
            text_x: PAD + GUT_BADGE,
            meta_x: w - PAD - meta_w,
        });
        y += ROW_H;
        if expanded == Some(i) {
            expanded_slot = Some(rows.len() - 1);
            for _ in p.rows[i].ladder.lines() {
                ladder.push(Rect {
                    x: LADDER_INDENT,
                    y,
                    w: (w - LADDER_INDENT - PAD) as u32,
                    h: LADDER_H as u32,
                });
                y += LADDER_H;
            }
        }
    }
    Layout { w, h: y + PAD, close, search, pills, fam_pills, rows, expanded_slot, ladder, visible }
}

fn inside(r: &Rect, x: i32, y: i32) -> bool {
    x >= r.x && x < r.x + r.w as i32 && y >= r.y && y < r.y + r.h as i32
}

/// Click resolution. Scrolling is keyboard-only (the caller maps Up/Down keys
/// to the scroll), so clicks resolve on the close box, the search box, the
/// pills — and on result rows, which toggle that row's tier ladder.
///
/// Two clicks mutate the model here instead of returning a new Action: the
/// caller's `match` over Action is closed and out of bounds for this module,
/// so new behaviors must ride the existing variants or none.
/// - A family pill stores the filter on the panel and resolves as
///   `SetCat(current cat)`: the caller's SetCat arm is exactly what a filter
///   change needs — re-run the search (where the filter is applied) and
///   re-sync the input region for the new panel size.
/// - A row click toggles `expanded` in place and resolves as no action; the
///   toggle needs no re-search, and the ladder it unfolds is informational.
pub fn hit(p: &mut Panel, lay: &Layout, x: i32, y: i32) -> Option<Action> {
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
    for (rect, f) in &lay.fam_pills {
        if inside(rect, x, y) {
            p.family = *f;
            return Some(Action::SetCat(p.cat));
        }
    }
    for (slot, g) in lay.rows.iter().enumerate() {
        if inside(&g.rect, x, y) {
            let i = lay.visible.start + slot;
            // A row without a ladder has nothing to unfold; swallowing the
            // click keeps "expanded" meaning "there is a ladder below".
            if p.rows.get(i).is_some_and(|r| !r.ladder.is_empty()) {
                p.expanded = if p.expanded == Some(i) { None } else { Some(i) };
            }
            return None;
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

/// An affix as a row: its text, its family badge, the meta column, and the
/// full ladder for expansion. Meta only states what the join established —
/// tier count and top ilvl need a ladder, "w" needs a recorded weight; an
/// affix with none of it gets an empty meta, never a guessed one.
/// `tiers` is ilvl-ascending, so the top tier is the last.
fn affix_row(a: &refdata::Affix) -> Row {
    let mut meta = Vec::new();
    if let Some(top) = a.tiers.last() {
        meta.push(format!("T×{}", a.tiers.len()));
        meta.push(format!("i{}", top.ilvl));
    }
    if let Some(w) = a.weight {
        meta.push(format!("w{w}"));
    }
    // Best tier first: T1 is the highest-ilvl rung, matching how players
    // read tier ladders (see core's rollquality).
    let ladder = a
        .tiers
        .iter()
        .rev()
        .enumerate()
        .map(|(n, t)| format!("T{}  i{}  ({})", n + 1, t.ilvl, t.range))
        .collect::<Vec<_>>()
        .join("\n");
    Row { text: one_line(&a.text), kind: Some(a.kind), meta: meta.join("  "), ladder }
}

/// "Name — Category" when the catalog knows the class, bare name otherwise.
fn item_row(i: &refdata::RefItem) -> Row {
    Row::plain(match &i.category {
        Some(c) if !c.is_empty() => format!("{} — {}", i.name, c),
        _ => i.name.clone(),
    })
}

/// Re-runs the search for the panel's (cat, query, family) against the loaded
/// reference data. Rows are capped, and scroll/expansion reset: results
/// changed, so the old window position and expanded index are meaningless.
pub fn refresh(p: &mut Panel, r: &Reference) {
    let q = p.query.as_str();
    let rows: Vec<Row> = match p.cat {
        Cat::Affixes => refdata::search_affixes(&r.affixes, q)
            .into_iter()
            .filter(|a| family_matches(p.family, a.kind))
            .map(affix_row)
            .collect(),
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
                Row::plain(match u.mods.first() {
                    Some(m) if head.chars().count() + m.chars().count() + 3 <= ROW_MAX_CHARS => {
                        one_line(&format!("{head} · {m}"))
                    }
                    _ => one_line(&head),
                })
            })
            .collect(),
        Cat::Keystones => refdata::search_keystones(&r.keystones, q)
            .into_iter()
            .map(|k| Row::plain(one_line(&format!("{}: {}", k.name, k.description))))
            .collect(),
        Cat::Xile(i) => {
            let entries = refcache::XILE_CATEGORIES
                .get(i)
                .and_then(|(slug, _)| r.categories.get(*slug))
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            refdata::search_ref_entries(entries, q)
                .into_iter()
                .map(|e| {
                    Row::plain(match e.lines.first() {
                        Some(l) => one_line(&format!("{} — {}", e.name, l)),
                        None => one_line(&e.name),
                    })
                })
                .collect()
        }
    };
    p.rows = rows.into_iter().take(MAX_ROWS).collect();
    p.scroll = 0;
    p.expanded = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use khaloni_poe2_core::refdata::{Affix, AffixKind, AffixTier};

    fn measure(s: &str) -> i32 {
        s.chars().count() as i32 * 7
    }

    fn tier(ilvl: u32, range: &str, kind: AffixKind) -> AffixTier {
        AffixTier { ilvl, range: range.to_string(), kind }
    }

    fn affix(text: &str, kind: AffixKind, tiers: Vec<AffixTier>, weight: Option<u32>) -> Affix {
        Affix {
            text: text.to_string(),
            trade_ids: Vec::new(),
            required_level: tiers.first().map(|t| t.ilvl),
            kind,
            weight,
            tiers,
        }
    }

    fn reference() -> Reference {
        Reference {
            affixes: vec![
                affix(
                    "# to maximum Life",
                    AffixKind::Prefix,
                    vec![
                        tier(1, "10-19", AffixKind::Prefix),
                        tier(30, "20-29", AffixKind::Prefix),
                        tier(75, "30-39", AffixKind::Prefix),
                    ],
                    Some(1000),
                ),
                affix(
                    "# to Strength",
                    AffixKind::Suffix,
                    vec![tier(1, "5-8", AffixKind::Suffix)],
                    Some(500),
                ),
                affix("Veiled mystery", AffixKind::Other, Vec::new(), None),
            ],
            items: Vec::new(),
            uniques: Vec::new(),
            keystones: Vec::new(),
            categories: std::collections::HashMap::new(),
            leveling: Vec::new(),
        }
    }

    fn panel() -> Panel {
        let mut p = Panel::default();
        refresh(&mut p, &reference());
        p
    }

    #[test]
    fn affix_rows_carry_badge_meta_and_ladder() {
        let p = panel();
        assert_eq!(p.rows.len(), 3);
        let life = p.rows.iter().find(|r| r.text.contains("Life")).unwrap();
        assert_eq!(life.kind, Some(AffixKind::Prefix));
        assert_eq!(life.meta, "T×3  i75  w1000");
        assert_eq!(
            life.ladder,
            "T1  i75  (30-39)\nT2  i30  (20-29)\nT3  i1  (10-19)",
            "best tier first, players' numbering"
        );
        // No ladder, no weight -> nothing claimed.
        let veiled = p.rows.iter().find(|r| r.text.contains("Veiled")).unwrap();
        assert_eq!(veiled.kind, Some(AffixKind::Other));
        assert_eq!(veiled.meta, "");
        assert_eq!(veiled.ladder, "");
    }

    #[test]
    fn family_filter_reduces_rows_and_composes_with_search() {
        let mut p = panel();
        p.family = Family::Prefix;
        refresh(&mut p, &reference());
        assert_eq!(p.rows.len(), 1, "Other never passes a Prefix/Suffix filter");
        assert!(p.rows[0].text.contains("Life"));
        p.family = Family::Suffix;
        p.query = "strength".into();
        refresh(&mut p, &reference());
        assert_eq!(p.rows.len(), 1);
        p.query = "life".into();
        refresh(&mut p, &reference());
        assert_eq!(p.rows.len(), 0, "filter and text search must both pass");
    }

    #[test]
    fn family_pills_exist_only_for_affixes_and_click_sets_the_filter() {
        let mut p = panel();
        let lay = layout(&p, &measure);
        let labels: Vec<&str> = lay.fam_pills.iter().map(|(_, f)| family_label(*f)).collect();
        assert_eq!(labels, ["All", "Prefix", "Suffix"]);
        // The pill click stores the filter and asks the caller for the
        // re-search it cannot run itself (it has no Reference).
        let (rect, _) = lay.fam_pills[1];
        assert_eq!(hit(&mut p, &lay, rect.x + 1, rect.y + 1), Some(Action::SetCat(Cat::Affixes)));
        assert_eq!(p.family, Family::Prefix);

        p.cat = Cat::Bases;
        let lay = layout(&p, &measure);
        assert!(lay.fam_pills.is_empty(), "a family filter over bases means nothing");
    }

    #[test]
    fn row_click_toggles_expansion_and_ladder_enters_the_layout() {
        let mut p = panel();
        let collapsed = layout(&p, &measure);
        assert!(collapsed.ladder.is_empty());
        let rect = collapsed.rows[0].rect;
        assert_eq!(hit(&mut p, &collapsed, rect.x + 1, rect.y + 1), None);
        assert_eq!(p.expanded, Some(0));

        let expanded = layout(&p, &measure);
        assert_eq!(expanded.expanded_slot, Some(0));
        assert_eq!(expanded.ladder.len(), 3, "one line per tier");
        assert_eq!(
            expanded.h,
            collapsed.h + 3 * LADDER_H,
            "ladder lines are the only height change"
        );
        // Rows below the unfolded one shift down by the ladder's height.
        assert_eq!(expanded.rows[1].rect.y, collapsed.rows[1].rect.y + 3 * LADDER_H);
        assert_eq!(expanded.ladder[0].y, expanded.rows[0].rect.y + ROW_H);
        assert_eq!(expanded.ladder[0].x, LADDER_INDENT);

        // Second click on the same row folds it back up.
        assert_eq!(hit(&mut p, &expanded, rect.x + 1, rect.y + 1), None);
        assert_eq!(p.expanded, None);
    }

    #[test]
    fn ladderless_rows_do_not_expand() {
        let mut p = panel();
        let veiled = p.rows.iter().position(|r| r.ladder.is_empty()).unwrap();
        let lay = layout(&p, &measure);
        let rect = lay.rows[veiled].rect;
        assert_eq!(hit(&mut p, &lay, rect.x + 1, rect.y + 1), None);
        assert_eq!(p.expanded, None, "nothing to unfold, so the click claims nothing");
    }

    #[test]
    fn expansion_leaves_the_layout_when_scrolled_out_of_view() {
        let mut p = panel();
        // Enough rows that scrolling can push row 0 out of the window.
        p.rows = (0..20)
            .map(|i| Row {
                text: format!("row {i}"),
                kind: Some(AffixKind::Prefix),
                meta: String::new(),
                ladder: "T1  i1  (1-2)".to_string(),
            })
            .collect();
        p.expanded = Some(0);
        p.scroll = 5;
        let lay = layout(&p, &measure);
        assert_eq!(lay.expanded_slot, None);
        assert!(lay.ladder.is_empty());
    }

    #[test]
    fn badge_and_meta_columns_are_measured_into_the_width() {
        // A text long enough that the panel width is row-driven, not
        // WIDTH_MIN-driven, so column contributions are observable.
        let text = "x".repeat(70);
        let bare = Panel {
            rows: vec![Row::plain(text.clone())],
            ..Panel::default()
        };
        let with_meta = Panel {
            rows: vec![Row {
                text,
                kind: Some(AffixKind::Prefix),
                meta: "T×5  i75  w1000".to_string(),
                ladder: String::new(),
            }],
            ..Panel::default()
        };
        let bare_lay = layout(&bare, &measure);
        let meta_lay = layout(&with_meta, &measure);
        assert_eq!(
            meta_lay.w,
            bare_lay.w + META_GAP + measure("T×5  i75  w1000"),
            "meta widens the panel by exactly its measured column"
        );
        assert_eq!(bare_lay.rows[0].text_x, PAD + GUT_BADGE, "gutter reserved for every row");
        let g = &meta_lay.rows[0];
        assert_eq!(g.meta_x, meta_lay.w - PAD - measure("T×5  i75  w1000"));
        assert!(g.badge_x < g.text_x && g.text_x < g.meta_x);
    }

    #[test]
    fn refresh_resets_scroll_and_expansion() {
        let mut p = panel();
        p.scroll = 2;
        p.expanded = Some(1);
        refresh(&mut p, &reference());
        assert_eq!((p.scroll, p.expanded), (0, None));
    }
}
