//! Market panel: pure model, layout, and hit-testing, in the same shape as
//! the Evaluate panel. The renderer draws from THIS geometry
//! and the click handler resolves actions from THIS geometry. Coordinates
//! are panel-local logical pixels.
//!
//! The panel never types, so it never asks for the keyboard: tabs, group
//! rows, the back arrow and paging are all clicks.
//!
//! Every figure shown here is worded by `core::market`, which also decides
//! whether there is a figure at all. A cell this module leaves empty is
//! empty because the model carried nothing for it; nothing is filled in.
//!
//! The whole panel is cloned into the frame state on every tick, so the
//! prepared rows live behind an `Arc` ([`View`]) and are rebuilt only when
//! the price data or the floors change.
//!
//! The third tab, "My runs", is the player's own results (`myruns::View`):
//! maps, map hours and, where the stash can be read, income. It is the
//! only place those words appear. The market tabs take one thing from it, a
//! "yours: ..." suffix on a mechanic's row, and only when a rate exists;
//! the market figures themselves never mix with it.

use std::sync::Arc;

use khaloni_poe2_core::market::{
    self, age_text, band_text_ascii, breadth_text, category_label, coverage_text, days_note, direction_text,
    percent_text, share_text, verdict_text, volume_text, Direction, Floors, Grade, Graded, Model, Options, DAYS,
    GREY_AFTER_SECS, THIN_MARKET, TOO_YOUNG,
};
use khaloni_poe2_core::value::display_price;

use crate::config::Rect;
use crate::myruns::{self, Shown};
use crate::prices::Market;

const PAD: i32 = 12;
const TITLE_H: i32 = 30;
const CLOSE: i32 = 20;
const STATUS_H: i32 = 18;
const TAB_H: i32 = 22;
const TAB_PAD_X: i32 = 10;
const TAB_GAP: i32 = 6;
const HEAD_H: i32 = 18;
/// A group row is two lines: the figures, then what they rest on.
const GROUP_ROW_H: i32 = 40;
const ROW_H: i32 = 22;
const SECTION_H: i32 = 22;
const COL_GAP: i32 = 14;
pub const SPARK_W: i32 = 56;
const WIDTH_MIN: i32 = 520;
const WIDTH_MAX: i32 = 1100;
/// Item and mover rows per page. Paging, not scrolling: the panel has no
/// keyboard, and a page is a pure function of the model.
pub const PAGE_ROWS: usize = 14;
/// Movers listed per side.
pub const MOVERS_PER_SIDE: usize = 10;
const NAME_MAX_CHARS: usize = 60;
/// The least the name column shrinks to when a row is too wide.
const NAME_MIN_W: i32 = 180;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Mechanics,
    Movers,
    MyRuns,
}

pub const TABS: [(Tab, &str); 3] = [(Tab::Mechanics, "Mechanics"), (Tab::Movers, "Movers"), (Tab::MyRuns, "My runs")];

/// How a row is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Normal,
    /// Thin market: greyed, under the liquid rows.
    Thin,
    /// Untrusted: shown only on request, and with no trend cells at all.
    Untrusted,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GroupRow {
    pub category: String,
    pub label: String,
    pub volume: String,
    pub share: String,
    /// The index in percent, or empty when the group has none.
    pub index: String,
    /// Sign of the index, for its colour.
    pub index_sign: i8,
    /// Second line: the verdict, the coverage sentence and the breadth
    /// counts. Cut to the panel's width at layout, never allowed to run
    /// past the edge.
    pub basis: String,
    pub spark: [Option<f64>; DAYS],
}

#[derive(Debug, Clone, PartialEq)]
pub struct ItemRow {
    pub name: String,
    pub price: String,
    pub volume: String,
    /// Empty whenever the model has no trend for the item.
    pub change: String,
    pub change_sign: i8,
    pub band: String,
    /// Rising or falling, for the note's colour; `None` for everything
    /// else, an item without a trend included.
    pub direction: Option<Direction>,
    /// Direction, the days note, the grade's note.
    pub note: String,
    pub spark: [Option<f64>; DAYS],
    pub tone: Tone,
    /// A listed unique: its freshness is the uniques', not the table's.
    pub listed: bool,
}

/// A group's item rows: liquid, then thin. Groups are exchange categories,
/// and only listed items can be untrusted, so there is nothing to hide
/// here; untrusted uniques are listed in the settings tab, on request.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GroupItems {
    pub category: String,
    pub rows: Vec<ItemRow>,
}

/// Everything the panel can show for one state of the price data, as text.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct View {
    pub league: String,
    pub young: bool,
    pub mechanics: Vec<GroupRow>,
    pub other_markets: Vec<GroupRow>,
    pub groups: Vec<GroupItems>,
    pub risers: Vec<ItemRow>,
    pub fallers: Vec<ItemRow>,
    pub old_categories: Vec<String>,
    /// Whether any listed (unique) item is in the data at all.
    pub has_listed: bool,
}

fn clip(s: &str) -> String {
    if s.chars().count() <= NAME_MAX_CHARS {
        return s.to_string();
    }
    let mut cut: String = s.chars().take(NAME_MAX_CHARS - 1).collect();
    cut.push('…');
    cut
}

/// `text` cut with an ellipsis so it measures at most `max_w`.
fn clip_to_width(text: &str, max_w: i32, measure: &dyn Fn(&str) -> i32) -> String {
    if measure(text) <= max_w {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut keep = chars.len();
    while keep > 0 {
        keep -= 1;
        let cut: String = chars[..keep].iter().collect::<String>() + "…";
        if measure(&cut) <= max_w {
            return cut;
        }
    }
    String::new()
}

fn sign(x: f64) -> i8 {
    if x > 0.0 {
        1
    } else if x < 0.0 {
        -1
    } else {
        0
    }
}

fn item_row(model: &Model, g: &Graded, divine_threshold: f64, with_category: bool) -> ItemRow {
    let tone = match g.grade {
        Grade::Liquid => Tone::Normal,
        Grade::Thin => Tone::Thin,
        Grade::Untrusted => Tone::Untrusted,
    };
    let mut name = g.item.name.clone();
    if let Some(base) = &g.item.base_type {
        name = format!("{name}, {base}");
    }
    if g.item.corrupted {
        name.push_str(" (corrupted)");
    }
    if with_category {
        name = format!("{name} · {}", category_label(&g.item.category));
    }
    let mut notes: Vec<String> = Vec::new();
    // An untrusted item gets no trend words either: "not enough history"
    // would be wrong about why there is none.
    if tone == Tone::Untrusted {
        notes.push(if g.at_floor {
            "untrusted: listed at the 1 ex floor".to_string()
        } else {
            format!("untrusted: {} listings", g.item.listings.unwrap_or(0))
        });
    } else if !model.young {
        notes.push(direction_text(g.trend.as_ref()).to_string());
        match g.trend {
            Some(t) => notes.extend(days_note(t.points_used)),
            // Says how little there was, never a zero.
            None => notes.push(format!("{} of {DAYS} days", g.item.points_used())),
        }
        if tone == Tone::Thin {
            // Held back by its price grid or by its depth: the note says which.
            let coarse = khaloni_poe2_core::market::is_coarsely_priced(&g.item, 0.0);
            notes.push(if coarse { khaloni_poe2_core::market::COARSE_PRICE } else { THIN_MARKET }.to_string());
        }
    }
    let trend = g.trend.filter(|_| tone != Tone::Untrusted);
    ItemRow {
        name: clip(&name),
        price: display_price(&model.price(&g.item), 1, divine_threshold),
        volume: match (g.item.volume_div, g.item.listings) {
            (Some(v), _) => volume_text(v),
            (None, Some(l)) => format!("{l} listed"),
            (None, None) => String::new(),
        },
        change: trend.map(|t| percent_text(t.change)).unwrap_or_default(),
        change_sign: trend.map(|t| sign(t.change)).unwrap_or(0),
        band: trend.map(|t| band_text_ascii(t.band)).unwrap_or_default(),
        direction: trend.map(|t| t.direction).filter(|d| *d != Direction::Unclear),
        note: notes.join(" · "),
        // The sparkline is a trend too: none for an item that has none.
        spark: if trend.is_some() { g.item.points } else { [None; DAYS] },
        tone,
        listed: g.item.listings.is_some(),
    }
}

impl View {
    pub fn build(market: &Market, floors: Floors, young: bool, divine_threshold: f64) -> View {
        let model = market::build(&market.source, &Options { floors, young });
        let group_row = |g: &market::Group| {
            let index = g.index_pct.map(percent_text).unwrap_or_default();
            let verdict = verdict_text(&g.verdict);
            GroupRow {
                category: g.category.clone(),
                label: category_label(&g.category),
                volume: if model.young { String::new() } else { volume_text(g.volume_div) },
                share: if model.young { String::new() } else { share_text(g.volume_share) },
                index,
                index_sign: g.index_pct.map(sign).unwrap_or(0),
                basis: if model.young {
                    format!("{} items", g.coverage.items_total)
                } else {
                    [verdict, coverage_text(g), breadth_text(&g.breadth)]
                        .into_iter()
                        .filter(|t| !t.is_empty())
                        .collect::<Vec<_>>()
                        .join(" · ")
                },
                spark: g.index_points,
            }
        };
        let groups = model
            .groups
            .iter()
            .map(|g| {
                let rows = g
                    .items
                    .iter()
                    .map(|&i| &model.items[i])
                    .filter(|it| it.grade != Grade::Untrusted)
                    .map(|it| item_row(&model, it, divine_threshold, false))
                    .collect();
                GroupItems { category: g.category.clone(), rows }
            })
            .collect();
        let movers = |ix: &[usize]| {
            ix.iter().take(MOVERS_PER_SIDE).map(|&i| item_row(&model, &model.items[i], divine_threshold, true)).collect()
        };
        View {
            league: model.league.clone(),
            young: model.young,
            mechanics: model.groups.iter().filter(|g| g.is_mechanic).map(group_row).collect(),
            other_markets: model.groups.iter().filter(|g| !g.is_mechanic).map(group_row).collect(),
            groups,
            risers: movers(&model.risers),
            fallers: movers(&model.fallers),
            old_categories: market.old_categories.iter().map(|c| category_label(c)).collect(),
            has_listed: model.items.iter().any(|g| g.item.listings.is_some()),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.groups.is_empty() && self.risers.is_empty() && self.fallers.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct Panel {
    pub league: String,
    /// `None` while the league's prices are loading: no rows at all.
    pub view: Option<Arc<View>>,
    pub tab: Tab,
    /// The group whose items are shown instead of the group list.
    pub open: Option<String>,
    pub page: usize,
    pub fresh: Freshness,
    /// The player's own runs, or why there are none to show.
    pub mine: Shown,
}

/// How old each part of the data is. The exchange table and the unique
/// prices refresh apart, so the header names the part that is old and only
/// that part's rows are greyed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Freshness {
    /// The price table's existing stale flag, and the uniques' own.
    pub stale: bool,
    pub uniques_stale: bool,
    /// Age of the oldest exchange overview in whole minutes; `None` when
    /// unknown. The same for the unique overviews.
    pub age_minutes: Option<i64>,
    pub uniques_age_minutes: Option<i64>,
}

impl Freshness {
    pub fn loading() -> Freshness {
        Freshness { stale: true, uniques_stale: true, age_minutes: None, uniques_age_minutes: None }
    }

    /// `now` and the fetch times in unix seconds.
    pub fn at(now: i64, market: &Market, stale: bool, uniques_stale: bool) -> Freshness {
        let minutes = |t: Option<i64>| t.map(|t| (now - t).max(0) / 60);
        Freshness {
            stale,
            uniques_stale,
            age_minutes: minutes(market.fetched_at),
            uniques_age_minutes: minutes(market.uniques_fetched_at),
        }
    }
}

fn old(minutes: Option<i64>) -> bool {
    minutes.is_some_and(|m| m * 60 > GREY_AFTER_SECS)
}

impl PartialEq for Panel {
    /// Compared every tick to decide on a repaint. Views are built once per
    /// price refresh, so two panels show the same rows exactly when they
    /// hold the same `Arc`.
    fn eq(&self, o: &Panel) -> bool {
        let same_view = match (&self.view, &o.view) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        let same_runs = match (&self.mine, &o.mine) {
            (Shown::Runs(a), Shown::Runs(b)) => Arc::ptr_eq(a, b),
            (a, b) => a == b,
        };
        same_view
            && same_runs
            && self.league == o.league
            && self.tab == o.tab
            && self.open == o.open
            && self.page == o.page
            && self.fresh == o.fresh
    }
}

impl Panel {
    pub fn loading(league: &str) -> Panel {
        Panel {
            league: league.to_string(),
            view: None,
            tab: Tab::default(),
            open: None,
            page: 0,
            fresh: Freshness::loading(),
            mine: Shown::Reading,
        }
    }

    /// Takes the latest own-runs view. True when it changed (the panel's
    /// size may have).
    pub fn set_runs(&mut self, mine: Shown) -> bool {
        let same = match (&self.mine, &mine) {
            (Shown::Runs(a), Shown::Runs(b)) => Arc::ptr_eq(a, b),
            (a, b) => a == b,
        };
        self.mine = mine;
        !same
    }

    /// Takes the latest price data. `view` is `None` while `league` is
    /// loading; a league other than the one shown resets what was open,
    /// since the group and page named the old league's rows.
    pub fn set_data(&mut self, league: &str, view: Option<Arc<View>>, fresh: Freshness) {
        if self.league != league {
            self.league = league.to_string();
            self.open = None;
            self.page = 0;
        }
        self.view = view.filter(|v| !v.is_empty());
        self.fresh = fresh;
        if let (Some(open), Some(v)) = (&self.open, &self.view) {
            if !v.groups.iter().any(|g| &g.category == open) {
                self.open = None;
                self.page = 0;
            }
        }
    }

    /// Exchange prices older than three hours: groups and exchange rows are
    /// drawn greyed.
    pub fn greyed(&self) -> bool {
        old(self.fresh.age_minutes)
    }

    /// The same for the rows of listed uniques, by the uniques' own age.
    pub fn uniques_greyed(&self) -> bool {
        old(self.fresh.uniques_age_minutes)
    }

    /// The line under the title: league, the age of each part that has
    /// one, which part is stale, and "too young" when that applies.
    pub fn status(&self) -> String {
        // The own results do not age with the price table: its freshness
        // words would only read as a warning about figures they are not
        // about.
        if self.tab == Tab::MyRuns {
            let stash = matches!(&self.mine, Shown::Runs(v) if v.footer.is_some());
            let source = if stash { "from your game log and stash" } else { "from your game log" };
            return format!("{} · {source}", self.league);
        }
        // While loading, the body carries the one sentence there is.
        let Some(view) = &self.view else { return self.league.clone() };
        let f = &self.fresh;
        let aged = |what: &str, minutes: Option<i64>, stale: bool| {
            let age = match minutes {
                Some(0) => format!("{what} just fetched"),
                Some(m) => format!("{what} {} old", age_text(m * 60)),
                None => format!("{what} age unknown"),
            };
            if stale {
                format!("{age} (stale)")
            } else {
                age
            }
        };
        let mut parts = vec![self.league.clone(), aged("prices", f.age_minutes, f.stale)];
        // The uniques get their own words only when they differ from the
        // table: another age bracket, or stale on their own.
        let apart = f.uniques_stale != f.stale
            || match (f.age_minutes, f.uniques_age_minutes) {
                (Some(a), Some(b)) => (a - b).abs() > 30,
                (a, b) => a.is_some() != b.is_some(),
            };
        if view.has_listed && apart {
            parts.push(aged("uniques", f.uniques_age_minutes, f.uniques_stale));
        }
        if !view.old_categories.is_empty() {
            parts.push(format!("old: {}", view.old_categories.join(", ")));
        }
        if view.young {
            parts.push(TOO_YOUNG.to_string());
        }
        parts.join(" · ")
    }

    fn open_group(&self) -> Option<&GroupItems> {
        let (view, open) = (self.view.as_ref()?, self.open.as_ref()?);
        view.groups.iter().find(|g| &g.category == open)
    }

    /// The item rows of the open group: liquid, then thin.
    pub fn open_rows(&self) -> Vec<&ItemRow> {
        self.open_group().map(|g| g.rows.iter().collect()).unwrap_or_default()
    }

    fn pages(&self) -> usize {
        self.open_rows().len().div_ceil(PAGE_ROWS).max(1)
    }
}

/// Keeps an open panel in step with the price service, building a [`View`]
/// only when what it is built from changed. The main loop calls
/// [`Feed::refresh`] once a tick and does nothing else about the data.
#[derive(Default)]
pub struct Feed {
    built_from: Option<(Arc<Market>, Floors, u64, bool)>,
    view: Option<Arc<View>>,
}

impl Feed {
    /// `snap_league`, `loading` and the two stale flags are the price
    /// snapshot's. The
    /// panel gets rows only when the market data is that same league's and
    /// its table has loaded; from a league change until then it shows the
    /// loading sentence, never the previous league's rows. True when the
    /// panel changed (its size may have, so the input region must follow).
    #[allow(clippy::too_many_arguments)]
    pub fn refresh(
        &mut self,
        panel: &mut Panel,
        snap_league: &str,
        loading: bool,
        (stale, uniques_stale): (bool, bool),
        market: &Arc<Market>,
        floors: Floors,
        divine_threshold: f64,
        now: i64,
    ) -> bool {
        let before = panel.clone();
        if loading || market.source.league != snap_league || market.source.is_empty() {
            self.built_from = None;
            self.view = None;
            panel.set_data(snap_league, None, Freshness::loading());
            return *panel != before;
        }
        let young = market::league_is_young(now, None, market.first_record, market.source.history_days());
        let key = (market.clone(), floors, divine_threshold.to_bits(), young);
        let same = self.built_from.as_ref().is_some_and(|(m, f, t, y)| {
            Arc::ptr_eq(m, &key.0) && *f == key.1 && *t == key.2 && *y == key.3
        });
        if !same {
            self.view = Some(Arc::new(View::build(market, floors, young, divine_threshold)));
            self.built_from = Some(key);
        }
        panel.set_data(snap_league, self.view.clone(), Freshness::at(now, market, stale, uniques_stale));
        *panel != before
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Close,
    SetTab(Tab),
    OpenGroup(String),
    Back,
    PrevPage,
    NextPage,
}

/// What a body line is, for the renderer.
#[derive(Debug, Clone, PartialEq)]
pub enum Line {
    /// "Mechanics" / "Other markets" / "Risers" / "Fallers".
    Section(String),
    /// Column captions.
    Caption,
    Group(GroupRow),
    Item(ItemRow),
    /// A sentence in place of rows: loading, too young, nothing to rank.
    Message(String),
    /// A row of the "My runs" tables: `table` picks the column set in
    /// [`Layout::tables`].
    Cells { table: usize, cells: Vec<(String, Ink)> },
    /// Small print under a table: what a figure leaves out.
    Note(String),
}

/// How a cell of a "My runs" table is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ink {
    Normal,
    Dim,
    Up,
    Down,
    /// A column caption: small capitals over a rule.
    Caption,
}

/// Left edges of the columns, shared by every row of the body so figures
/// line up. `end` is the right edge of the last column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Columns {
    pub name: i32,
    pub price: i32,
    pub volume: i32,
    pub change: i32,
    pub band: i32,
    pub spark: i32,
    pub note: i32,
    pub end: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub w: i32,
    pub h: i32,
    pub close: Rect,
    pub title_pos: (i32, i32),
    pub status_pos: (i32, i32),
    pub tabs: Vec<(Rect, Tab)>,
    /// Present while a group is open.
    pub back: Option<Rect>,
    pub heading_pos: Option<(i32, i32)>,
    pub lines: Vec<(Rect, Line)>,
    pub cols: Columns,
    /// Clickable group rows: the rect and the category it opens.
    pub group_hits: Vec<(Rect, String)>,
    pub prev: Option<Rect>,
    pub next: Option<Rect>,
    pub page_pos: Option<(i32, i32)>,
    /// The note under the body, when there is one.
    pub footer: Option<((i32, i32), String)>,
    /// Left edges of the columns of each "My runs" table.
    pub tables: Vec<Vec<i32>>,
}

pub const CAPTIONS: [&str; 5] = ["price", "traded volume", "change", "band", "7 days"];
/// The movers mix exchange items with listed uniques, whose depth is a
/// listing count, so the column names both.
pub const MOVER_CAPTIONS: [&str; 5] = ["price", "traded volume / listed", "change", "band", "7 days"];
pub const GROUP_CAPTIONS: [&str; 4] = ["traded volume", "share", "price index", "7 days"];

pub const RUNS_CAPTIONS: [&str; 3] = ["", "maps", "map hours"];
pub const BLOCK_CAPTIONS: [&str; 6] = ["block ended", "maps", "map hours", "mechanics seen", "income", "revaluation"];
const T_TOTALS: usize = 0;
const T_OVERALL: usize = 1;
const T_MECHANICS: usize = 2;
const T_BLOCKS: usize = 3;

fn cells(table: usize, ink: Ink, texts: &[&str]) -> Line {
    Line::Cells { table, cells: texts.iter().map(|t| (t.to_string(), ink)).collect() }
}

/// The "My runs" tab. It rests on the game log and the stash, not on the
/// price data, so it shows while prices load.
fn runs_lines(mine: &Shown) -> Vec<Line> {
    let v = match mine {
        Shown::Reading => return vec![Line::Message(myruns::READING_LOG.to_string())],
        Shown::LogMissing => return vec![Line::Message(myruns::LOG_MISSING.to_string())],
        Shown::Runs(v) => v,
    };
    let mut out = vec![Line::Section("Maps".to_string())];
    if v.has_runs() {
        out.push(cells(T_TOTALS, Ink::Caption, &RUNS_CAPTIONS));
        out.extend(v.totals.iter().map(|r| cells(T_TOTALS, Ink::Normal, &[&r[0], &r[1], &r[2]])));
        out.extend(v.totals_note.iter().cloned().map(Line::Note));
    } else {
        out.push(Line::Message(myruns::NO_RUNS.to_string()));
    }
    out.push(Line::Section("Income".to_string()));
    if let Some((rate, sample)) = &v.overall {
        out.push(Line::Cells {
            table: T_OVERALL,
            cells: vec![("overall".to_string(), Ink::Dim), (rate.clone(), Ink::Normal), (sample.clone(), Ink::Dim)],
        });
    }
    out.extend(v.income_note.iter().cloned().map(Line::Message));
    if !v.mechanics.is_empty() {
        out.push(Line::Section("Mechanics seen".to_string()));
        for m in &v.mechanics {
            let ink = if m.has_rate { Ink::Normal } else { Ink::Dim };
            out.push(Line::Cells {
                table: T_MECHANICS,
                cells: vec![(m.name.clone(), Ink::Normal), (m.seen.clone(), Ink::Normal), (m.income.clone(), ink)],
            });
        }
        out.push(Line::Note(myruns::SEEN_NOTE.to_string()));
    }
    if !v.blocks.is_empty() {
        out.push(Line::Section("Last blocks".to_string()));
        out.push(cells(T_BLOCKS, Ink::Caption, &BLOCK_CAPTIONS));
        for b in &v.blocks {
            let income = match b.income_sign {
                1 => Ink::Up,
                -1 => Ink::Down,
                _ => Ink::Dim,
            };
            out.push(Line::Cells {
                table: T_BLOCKS,
                cells: vec![
                    (b.time.clone(), Ink::Normal),
                    (b.maps.clone(), Ink::Normal),
                    (b.hours.clone(), Ink::Normal),
                    (b.mechanics.clone(), Ink::Normal),
                    (b.income.clone(), income),
                    (b.revaluation.clone(), Ink::Dim),
                ],
            });
        }
    }
    out
}

fn body_lines(p: &Panel) -> Vec<Line> {
    if p.tab == Tab::MyRuns {
        return runs_lines(&p.mine);
    }
    let Some(view) = &p.view else {
        return vec![Line::Message(crate::league::loading_text(&p.league))];
    };
    let mut out = Vec::new();
    match (p.tab, p.open_group()) {
        (Tab::Mechanics, Some(_)) => {
            let rows = p.open_rows();
            out.push(Line::Caption);
            let start = p.page.min(p.pages() - 1) * PAGE_ROWS;
            out.extend(rows.iter().skip(start).take(PAGE_ROWS).map(|r| Line::Item((*r).clone())));
            if rows.is_empty() {
                out.push(Line::Message("no items in this group".to_string()));
            }
        }
        (Tab::Mechanics, None) => {
            for (title, rows) in [("Mechanics", &view.mechanics), ("Other markets", &view.other_markets)] {
                if rows.is_empty() {
                    continue;
                }
                out.push(Line::Section(title.to_string()));
                out.push(Line::Caption);
                out.extend(rows.iter().cloned().map(|mut g| {
                    // Beside the market's figures, never inside them, and
                    // only when the player's own sample carries a rate.
                    if let Shown::Runs(mine) = &p.mine {
                        if let Some(yours) = mine.yours.get(&g.category) {
                            g.basis = format!("{} · {yours}", g.basis);
                        }
                    }
                    Line::Group(g)
                }));
            }
        }
        (Tab::Movers, _) if view.young => out.push(Line::Message(TOO_YOUNG.to_string())),
        (Tab::Movers, _) => {
            for (title, rows) in [("Risers", &view.risers), ("Fallers", &view.fallers)] {
                out.push(Line::Section(title.to_string()));
                if rows.is_empty() {
                    out.push(Line::Message("no liquid item with a clear direction".to_string()));
                    continue;
                }
                out.push(Line::Caption);
                out.extend(rows.iter().cloned().map(Line::Item));
            }
        }
        (Tab::MyRuns, _) => {}
    }
    out
}

/// Size of the band and note text relative to the row text `measure`
/// speaks for. The renderer draws them at exactly this ratio (see
/// `SMALL_PX`), so the columns are sized for the glyphs that land in them.
pub const SMALL_RATIO: f32 = 14.0 / 18.0;

pub fn layout(p: &Panel, measure: &dyn Fn(&str) -> i32) -> Layout {
    let small = |s: &str| (measure(s) as f32 * SMALL_RATIO).ceil() as i32;
    let small: &dyn Fn(&str) -> i32 = &small;
    let lines = body_lines(p);
    let showing_groups = p.tab == Tab::Mechanics && p.open_group().is_none();
    let captions = captions_for(p);

    // Column widths from what is on this page, captions included.
    let mut wid = [0i32; 6]; // name, price/volume, volume/share, change/index, band, note
    for line in &lines {
        match line {
            Line::Group(g) => {
                wid[0] = wid[0].max(measure(&g.label));
                wid[1] = wid[1].max(measure(&g.volume));
                wid[2] = wid[2].max(measure(&g.share));
                wid[3] = wid[3].max(measure(&g.index));
            }
            Line::Item(r) => {
                wid[0] = wid[0].max(measure(&r.name));
                wid[1] = wid[1].max(measure(&r.price));
                wid[2] = wid[2].max(measure(&r.volume));
                wid[3] = wid[3].max(measure(&r.change));
                wid[4] = wid[4].max(small(&r.band));
                wid[5] = wid[5].max(small(&r.note));
            }
            _ => {}
        }
    }
    // The "My runs" tables: each its own columns, as wide as its widest
    // cell. Captions are drawn smaller than they are measured here.
    let mut table_wid: Vec<Vec<i32>> = Vec::new();
    for line in &lines {
        if let Line::Cells { table, cells } = line {
            if table_wid.len() <= *table {
                table_wid.resize(*table + 1, Vec::new());
            }
            let wid = &mut table_wid[*table];
            if wid.len() < cells.len() {
                wid.resize(cells.len(), 0);
            }
            for (i, (text, _)) in cells.iter().enumerate() {
                wid[i] = wid[i].max(measure(text));
            }
        }
    }
    let mut tables: Vec<Vec<i32>> = Vec::new();
    let mut tables_w = 0;
    for wid in &table_wid {
        let mut x = PAD;
        let mut edges = Vec::new();
        for w in wid {
            edges.push(x);
            x += w + COL_GAP + 6;
        }
        tables_w = tables_w.max(x - COL_GAP - 6 - PAD);
        tables.push(edges);
    }
    let has_rows = lines.iter().any(|l| matches!(l, Line::Group(_) | Line::Item(_)));
    if has_rows {
        for (i, c) in captions.iter().take(captions.len().saturating_sub(1)).enumerate() {
            wid[i + 1] = wid[i + 1].max(measure(c));
        }
    }
    // Past the widest panel, the name column gives way first and the note
    // second: a figure is never the thing that gets cut.
    if !showing_groups {
        let fixed = wid[1] + wid[2] + wid[3] + wid[4] + SPARK_W + 6 * COL_GAP;
        let room = WIDTH_MAX - 2 * PAD - fixed;
        if wid[0] + wid[5] > room {
            wid[0] = wid[0].min((room - wid[5]).max(NAME_MIN_W));
            wid[5] = wid[5].min((room - wid[0]).max(0));
        }
    }
    let mut cols = Columns { name: PAD, ..Columns::default() };
    cols.price = cols.name + wid[0] + COL_GAP;
    cols.volume = cols.price + wid[1] + COL_GAP;
    cols.change = cols.volume + wid[2] + COL_GAP;
    if showing_groups {
        // Groups have no band column: volume, share, index, sparkline.
        cols.band = cols.change + wid[3];
        cols.spark = cols.band + COL_GAP;
        cols.note = cols.spark + SPARK_W;
        cols.end = cols.note;
    } else {
        cols.band = cols.change + wid[3] + COL_GAP;
        cols.spark = cols.band + wid[4] + COL_GAP;
        cols.note = cols.spark + SPARK_W + COL_GAP;
        cols.end = cols.note + wid[5];
    }

    let status = p.status();
    let basis_w =
        lines.iter().filter_map(|l| if let Line::Group(g) = l { Some(small(&g.basis)) } else { None }).max();
    let message_w = lines
        .iter()
        .filter_map(|l| match l {
            Line::Message(m) => Some(measure(m)),
            Line::Note(n) => Some(small(n)),
            _ => None,
        })
        .max();
    let footer = footer_text(p);
    let content_w = [
        if has_rows { cols.end - PAD } else { 0 },
        measure(&status),
        basis_w.unwrap_or(0),
        message_w.unwrap_or(0),
        tables_w,
        footer.as_deref().map(small).unwrap_or(0),
    ]
    .into_iter()
    .max()
    .unwrap_or(0);
    let w = (PAD + content_w + PAD).clamp(WIDTH_MIN, WIDTH_MAX);

    let close = Rect { x: w - PAD - CLOSE, y: PAD, w: CLOSE as u32, h: CLOSE as u32 };
    let title_pos = (PAD, PAD + 18);
    let mut y = PAD + TITLE_H;
    let status_pos = (PAD, y + 13);
    y += STATUS_H + 6;

    let mut tabs = Vec::new();
    let mut x = PAD;
    for (tab, label) in TABS {
        let tw = measure(label) + 2 * TAB_PAD_X;
        tabs.push((Rect { x, y, w: tw as u32, h: TAB_H as u32 }, tab));
        x += tw + TAB_GAP;
    }
    y += TAB_H + 8;

    let (mut back, mut heading_pos) = (None, None);
    if p.tab == Tab::Mechanics && p.open_group().is_some() {
        back = Some(Rect { x: PAD, y, w: 24, h: 20 });
        heading_pos = Some((PAD + 24 + 8, y + 16));
        y += 20 + 6;
    }

    let inner = w - 2 * PAD;
    let mut placed = Vec::new();
    let mut group_hits = Vec::new();
    for mut line in lines {
        match &mut line {
            Line::Group(g) => g.basis = clip_to_width(&g.basis, inner, small),
            Line::Note(n) => *n = clip_to_width(n, inner, small),
            Line::Message(m) => *m = clip_to_width(m, inner, measure),
            // Only the last cell can reach the edge: the others are sized
            // by their content and the panel by the widest table.
            Line::Cells { table, cells } => {
                if let (Some(last), Some(x)) = (cells.len().checked_sub(1), tables.get(*table)) {
                    let room = w - PAD - x.get(last).copied().unwrap_or(PAD);
                    cells[last].0 = clip_to_width(&cells[last].0, room, measure);
                }
            }
            Line::Item(r) => {
                r.name = clip_to_width(&r.name, wid[0], measure);
                r.note = clip_to_width(&r.note, wid[5], small);
            }
            _ => {}
        }
        let h = match &line {
            Line::Section(_) => SECTION_H,
            Line::Caption => HEAD_H,
            Line::Group(_) => GROUP_ROW_H,
            Line::Item(_) => ROW_H,
            Line::Message(_) => ROW_H + 6,
            Line::Cells { cells, .. } if cells.iter().any(|(_, ink)| *ink == Ink::Caption) => HEAD_H,
            Line::Cells { .. } => ROW_H,
            Line::Note(_) => STATUS_H,
        };
        let rect = Rect { x: PAD, y, w: (w - 2 * PAD) as u32, h: h as u32 };
        if let Line::Group(g) = &line {
            group_hits.push((rect, g.category.clone()));
        }
        placed.push((rect, line));
        y += h;
    }

    let (mut prev, mut next, mut page_pos) = (None, None, None);
    if p.open_group().is_some() && p.tab == Tab::Mechanics && p.pages() > 1 {
        y += 6;
        prev = Some(Rect { x: PAD, y, w: 24, h: 20 });
        next = Some(Rect { x: PAD + 30, y, w: 24, h: 20 });
        page_pos = Some((PAD + 64, y + 15));
        y += 20;
    }
    let footer = footer.map(|text| {
        y += 8;
        let pos = (PAD, y + 12);
        y += 16;
        (pos, text)
    });
    Layout {
        w,
        h: y + PAD,
        close,
        title_pos,
        status_pos,
        tabs,
        back,
        heading_pos,
        lines: placed,
        cols,
        group_hits,
        prev,
        next,
        page_pos,
        footer,
        tables,
    }
}

/// The column captions of what `p` is showing.
pub fn captions_for(p: &Panel) -> &'static [&'static str] {
    match p.tab {
        Tab::Movers => &MOVER_CAPTIONS,
        Tab::Mechanics if p.open_group().is_none() => &GROUP_CAPTIONS,
        Tab::Mechanics => &CAPTIONS,
        // Its tables carry their captions as rows.
        Tab::MyRuns => &[],
    }
}

/// "page 2 of 4", for the pager.
pub fn page_text(p: &Panel) -> String {
    format!("page {} of {}", p.page.min(p.pages() - 1) + 1, p.pages())
}

/// The open group's heading.
pub fn heading_text(p: &Panel) -> String {
    p.open.as_deref().map(category_label).unwrap_or_default()
}

fn footer_text(p: &Panel) -> Option<String> {
    if p.tab == Tab::MyRuns {
        return match &p.mine {
            Shown::Runs(mine) => mine.footer.clone(),
            _ => None,
        };
    }
    let view = p.view.as_ref()?;
    match p.tab {
        Tab::Mechanics if !view.young => Some("traded volume: period not published by poe.ninja".to_string()),
        _ => None,
    }
}

fn inside(r: &Rect, x: i32, y: i32) -> bool {
    x >= r.x && x < r.x + r.w as i32 && y >= r.y && y < r.y + r.h as i32
}

/// Click resolution; never mutates. `apply` carries the action out.
pub fn hit(lay: &Layout, x: i32, y: i32) -> Option<Action> {
    if inside(&lay.close, x, y) {
        return Some(Action::Close);
    }
    for (rect, tab) in &lay.tabs {
        if inside(rect, x, y) {
            return Some(Action::SetTab(*tab));
        }
    }
    if lay.back.as_ref().is_some_and(|r| inside(r, x, y)) {
        return Some(Action::Back);
    }
    if lay.prev.as_ref().is_some_and(|r| inside(r, x, y)) {
        return Some(Action::PrevPage);
    }
    if lay.next.as_ref().is_some_and(|r| inside(r, x, y)) {
        return Some(Action::NextPage);
    }
    lay.group_hits.iter().find(|(r, _)| inside(r, x, y)).map(|(_, c)| Action::OpenGroup(c.clone()))
}

/// Carries out everything but `Close`, which is the caller's (it owns the
/// panel). True when the panel changed.
pub fn apply(p: &mut Panel, action: &Action) -> bool {
    let before = (p.tab, p.open.clone(), p.page);
    match action {
        Action::Close => {}
        Action::SetTab(tab) => {
            p.tab = *tab;
            p.open = None;
            p.page = 0;
        }
        Action::OpenGroup(category) => {
            // A young league has no group detail to rank or grade.
            if p.view.as_ref().is_some_and(|v| v.groups.iter().any(|g| &g.category == category)) {
                p.open = Some(category.clone());
                p.page = 0;
            }
        }
        Action::Back => {
            p.open = None;
            p.page = 0;
        }
        Action::PrevPage => p.page = p.page.min(p.pages() - 1).saturating_sub(1),
        Action::NextPage => p.page = (p.page + 1).min(p.pages() - 1),
    }
    before != (p.tab, p.open.clone(), p.page)
}

/// Every string the panel can put on screen for `p`, for tests that hold
/// the wording to its rules.
pub fn all_text(p: &Panel, measure: &dyn Fn(&str) -> i32) -> Vec<String> {
    let lay = layout(p, measure);
    let mut out = vec!["Market".to_string(), "x".to_string(), p.status()];
    if lay.heading_pos.is_some() {
        out.push(heading_text(p));
    }
    out.extend(TABS.iter().map(|(_, l)| l.to_string()));
    out.extend([(&lay.back, "<"), (&lay.prev, "<"), (&lay.next, ">")].iter().filter(|(r, _)| r.is_some()).map(|(_, g)| g.to_string()));
    if lay.lines.iter().any(|(_, l)| *l == Line::Caption) {
        out.extend(captions_for(p).iter().map(|c| c.to_string()));
    }
    out.extend(lay.footer.iter().map(|(_, t)| t.clone()));
    if lay.page_pos.is_some() {
        out.push(page_text(p));
    }
    for (_, line) in &lay.lines {
        match line {
            Line::Section(s) | Line::Message(s) | Line::Note(s) => out.push(s.clone()),
            Line::Cells { cells, .. } => out.extend(cells.iter().map(|(t, _)| t.clone())),
            Line::Caption => {}
            Line::Group(g) => {
                out.extend([g.label.clone(), g.volume.clone(), g.share.clone(), g.index.clone(), g.basis.clone()])
            }
            Line::Item(r) => out.extend([
                r.name.clone(),
                r.price.clone(),
                r.volume.clone(),
                r.change.clone(),
                r.band.clone(),
                r.note.clone(),
            ]),
        }
    }
    out
}
