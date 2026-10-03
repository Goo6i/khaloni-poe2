//! Settings > Market: the overlay panel's model as sortable tables with
//! larger charts, the editable liquidity floors, and what the volume column
//! is. The settings window is its own process, so it reads the overviews
//! the overlay's refresh cached on disk, through the same
//! `prices::market_from_cache` -> `core::market::build` path; it makes no
//! request of its own.
//!
//! "My runs" sits in the same tab, folded: the overlay tab's figures as
//! tables with every block, read from the game log and the stash history
//! on disk, and an export of the runs and blocks as CSV. It is the only
//! part of this tab that speaks of income.
//!
//! [`Tables`] is the pure part: rows and sorting, testable without a
//! display. A cell the model has no figure for sorts last in either
//! direction and shows as empty; it is never sorted as a zero.

use eframe::egui;

use khaloni_poe2_core::market::{
    self, age_text, band_text, breadth_text, category_label, coverage_text, days_note, direction_text,
    percent_text, share_text, verdict_text, volume_text, Floors, Grade, Graded, Model, Options, DAYS, THIN_MARKET,
    TOO_YOUNG,
};
use khaloni_poe2_core::market_history::{window_change, HistoryLog, Record};
use khaloni_poe2_core::value::display_price;

use crate::config::Config;
use crate::prices::Market;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    Name,
    Price,
    Depth,
    Change,
    Band,
    Day,
    ThreeDays,
}

/// One item as the tables show it: the figures to sort by beside the text.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub name: String,
    pub category: String,
    pub id: String,
    pub price_div: f64,
    pub price: String,
    /// Traded volume, or the listing count of a listed item.
    pub depth: Option<f64>,
    pub depth_text: String,
    pub grade: Grade,
    pub change: Option<f64>,
    pub band: Option<u32>,
    pub note: String,
    pub points: [Option<f64>; DAYS],
    /// Changes over windows the source does not publish, from the history
    /// log: (percent, records behind it).
    pub day: Option<(f64, usize)>,
    pub three_days: Option<(f64, usize)>,
}

fn row(model: &Model, g: &Graded, threshold: f64, records: &[Record], now: i64) -> Row {
    let mut name = g.item.name.clone();
    if let Some(base) = &g.item.base_type {
        name = format!("{name}, {base}");
    }
    if g.item.corrupted {
        name.push_str(" (corrupted)");
    }
    let untrusted = g.grade == Grade::Untrusted;
    let mut notes: Vec<String> = Vec::new();
    if untrusted {
        notes.push(if g.at_floor {
            "untrusted: listed at the 1 ex floor".into()
        } else {
            format!("untrusted: {} listings", g.item.listings.unwrap_or(0))
        });
    } else if !model.young {
        notes.push(direction_text(g.trend.as_ref()).into());
        match g.trend {
            Some(t) => notes.extend(days_note(t.points_used)),
            None => notes.push(format!("{} of {DAYS} days", g.item.points_used())),
        }
        if g.grade == Grade::Thin {
            let coarse = khaloni_poe2_core::market::is_coarsely_priced(&g.item, 0.0);
            notes.push(if coarse { khaloni_poe2_core::market::COARSE_PRICE } else { THIN_MARKET }.into());
        }
    }
    // The log's windows are trends too: none for an item that is not
    // trusted to have one, none in a league too young for them.
    let window = |secs: i64| {
        (!untrusted && !model.young)
            .then(|| window_change(records, &g.item.category, &g.item.id, now, secs))
            .flatten()
            .map(|w| (w.pct, w.records))
    };
    Row {
        name,
        category: g.item.category.clone(),
        id: g.item.id.clone(),
        price_div: g.item.price_div,
        price: display_price(&model.price(&g.item), 1, threshold),
        depth: g.item.volume_div.or(g.item.listings.map(f64::from)),
        depth_text: match (g.item.volume_div, g.item.listings) {
            (Some(v), _) => volume_text(v),
            (None, Some(l)) => format!("{l} listed"),
            (None, None) => String::new(),
        },
        grade: g.grade,
        change: g.trend.map(|t| t.change),
        band: g.trend.map(|t| t.band),
        note: notes.join(" · "),
        points: if g.trend.is_some() { g.item.points } else { [None; DAYS] },
        day: window(86_400),
        three_days: window(3 * 86_400),
    }
}

/// Sorts by `column`. Rows without a figure in that column go last whether
/// ascending or descending: "no data" is not the smallest value.
pub fn sort_rows(rows: &mut [Row], column: Column, descending: bool) {
    let key = |r: &Row| -> Option<f64> {
        match column {
            Column::Name => None,
            Column::Price => Some(r.price_div),
            Column::Depth => r.depth,
            Column::Change => r.change,
            Column::Band => r.band.map(f64::from),
            Column::Day => r.day.map(|d| d.0),
            Column::ThreeDays => r.three_days.map(|d| d.0),
        }
    };
    rows.sort_by(|a, b| {
        if column == Column::Name {
            let ord = a.name.to_lowercase().cmp(&b.name.to_lowercase());
            return if descending { ord.reverse() } else { ord };
        }
        match (key(a), key(b)) {
            (Some(x), Some(y)) => {
                let ord = x.total_cmp(&y);
                (if descending { ord.reverse() } else { ord }).then_with(|| a.name.cmp(&b.name))
            }
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.name.cmp(&b.name),
        }
    });
}

/// Everything the tab shows for one read of the cache.
pub struct Tables {
    pub model: Model,
    pub fetched_at: Option<i64>,
    pub uniques_fetched_at: Option<i64>,
    pub records: usize,
    pub first_record: Option<i64>,
    pub risers: Vec<Row>,
    pub fallers: Vec<Row>,
    /// Listed items under the minimum listings or at the price floor:
    /// shown only when asked for, with a price and no trend.
    pub untrusted: Vec<Row>,
    /// Item rows per group, in `model.groups` order.
    pub group_rows: Vec<Vec<Row>>,
}

impl Tables {
    pub fn build(market: &Market, floors: Floors, threshold: f64, records: &[Record], now: i64) -> Tables {
        let first_record = records.first().map(|r| r.t);
        let young = market::league_is_young(now, None, first_record, market.source.history_days());
        let model = market::build(&market.source, &Options { floors, young });
        let rows = |ix: &[usize]| ix.iter().map(|&i| row(&model, &model.items[i], threshold, records, now)).collect();
        Tables {
            risers: rows(&model.risers),
            fallers: rows(&model.fallers),
            untrusted: rows(&(0..model.items.len()).filter(|&i| model.items[i].grade == Grade::Untrusted).collect::<Vec<_>>()),
            group_rows: model.groups.iter().map(|g| rows(&g.items)).collect(),
            fetched_at: market.fetched_at,
            uniques_fetched_at: market.uniques_fetched_at,
            records: records.len(),
            first_record,
            model,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shown {
    Groups,
    Movers,
}

/// The tab's state. Loaded on first show and on "Reload"; rebuilt (no file
/// access) when a floor changes.
pub struct MarketUi {
    league: String,
    market: Option<Market>,
    records: Vec<Record>,
    tables: Option<Tables>,
    built_for: Option<(Floors, u64)>,
    shown: Shown,
    group: usize,
    sort: (Column, bool),
    show_untrusted: bool,
    selected: Option<(String, String)>,
    runs: RunsUi,
}

/// The "My runs" part: loaded on a thread when first unfolded (the log
/// read is tens of megabytes), and again on "Reload".
#[derive(Default)]
struct RunsUi {
    loading: Option<std::sync::mpsc::Receiver<Result<crate::myruns::Loaded, String>>>,
    loaded: Option<Result<crate::myruns::Loaded, String>>,
    loaded_for: Option<(String, khaloni_poe2_core::income::StashAccess)>,
    exported: Option<Result<String, String>>,
}

impl Default for MarketUi {
    fn default() -> MarketUi {
        MarketUi {
            league: String::new(),
            market: None,
            records: Vec::new(),
            tables: None,
            built_for: None,
            shown: Shown::Groups,
            group: 0,
            sort: (Column::Depth, true),
            show_untrusted: false,
            selected: None,
            runs: RunsUi::default(),
        }
    }
}

fn cache_dir() -> std::path::PathBuf {
    directories::ProjectDirs::from("", "", "khaloni-poe2")
        .map(|d| d.cache_dir().to_path_buf())
        .unwrap_or_else(std::env::temp_dir)
}

impl MarketUi {
    fn load(&mut self, league: &str) {
        let dir = cache_dir();
        self.league = league.to_string();
        self.market = Some(crate::prices::market_from_cache(&dir, league));
        self.records = HistoryLog::open(&dir.join("market-history"), league).read();
        self.tables = None;
        self.selected = None;
        self.group = 0;
    }

    fn ensure(&mut self, cfg: &Config) {
        if self.market.is_none() || self.league != cfg.league {
            self.load(&cfg.league);
        }
        let key = (cfg.market_floors().or_default(), cfg.divine_threshold.to_bits());
        if self.tables.is_none() || self.built_for != Some(key) {
            if let Some(m) = &self.market {
                self.tables =
                    Some(Tables::build(m, key.0, cfg.divine_threshold, &self.records, crate::prices::unix_now()));
                self.built_for = Some(key);
            }
        }
    }
}

const UP: egui::Color32 = egui::Color32::from_rgb(0x7F, 0xB8, 0x6A);
const DOWN: egui::Color32 = egui::Color32::from_rgb(0xD9, 0x7A, 0x5F);

/// A week of points as a chart: a line through the days that have a
/// figure, broken where one is missing, a dot for a lone day.
fn chart(ui: &mut egui::Ui, size: egui::Vec2, points: &[Option<f64>], color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let known: Vec<f64> = points.iter().flatten().copied().collect();
    if known.len() < 2 {
        return;
    }
    let (lo, hi) = known.iter().fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
    let span = (hi - lo).max(1e-9);
    let inner = rect.shrink(2.0);
    let step = inner.width() / (points.len() - 1) as f32;
    let at = |i: usize, v: f64| {
        egui::pos2(inner.left() + step * i as f32, inner.bottom() - ((v - lo) / span) as f32 * inner.height())
    };
    let painter = ui.painter();
    let mut run: Vec<egui::Pos2> = Vec::new();
    let flush = |run: &mut Vec<egui::Pos2>| {
        match run.len() {
            0 => {}
            1 => {
                painter.circle_filled(run[0], 2.0, color);
            }
            _ => {
                painter.add(egui::Shape::line(run.clone(), egui::Stroke::new(1.5, color)));
            }
        }
        run.clear();
    };
    for (i, p) in points.iter().enumerate() {
        match p {
            Some(v) => run.push(at(i, *v)),
            None => flush(&mut run),
        }
    }
    flush(&mut run);
}

fn header(ui: &mut egui::Ui, sort: &mut (Column, bool), label: &str, column: Column) {
    let mark = if sort.0 == column { if sort.1 { " ▼" } else { " ▲" } } else { "" };
    if ui.button(format!("{label}{mark}")).clicked() {
        *sort = if sort.0 == column { (column, !sort.1) } else { (column, column != Column::Name) };
    }
}

fn signed(ui: &mut egui::Ui, pct: Option<f64>, muted: bool) {
    match pct {
        Some(p) if muted => ui.weak(percent_text(p)),
        Some(p) if p > 0.0 => ui.colored_label(UP, percent_text(p)),
        Some(p) if p < 0.0 => ui.colored_label(DOWN, percent_text(p)),
        Some(p) => ui.label(percent_text(p)),
        None => ui.label(""),
    };
}

fn window_cell(ui: &mut egui::Ui, w: Option<(f64, usize)>, muted: bool) {
    match w {
        Some((pct, n)) => {
            ui.horizontal(|ui| {
                signed(ui, Some(pct), muted);
                ui.weak(format!("({n} records)"));
            });
        }
        None => {
            ui.label("");
        }
    }
}

fn item_table(
    ui: &mut egui::Ui,
    id: &str,
    rows: &[Row],
    sort: &mut (Column, bool),
    depth_caption: &str,
    selected: &mut Option<(String, String)>,
) {
    let mut rows: Vec<Row> = rows.to_vec();
    sort_rows(&mut rows, sort.0, sort.1);
    egui::Grid::new(id).num_columns(9).striped(true).spacing([14.0, 4.0]).show(ui, |ui| {
        header(ui, sort, "item", Column::Name);
        header(ui, sort, "price", Column::Price);
        header(ui, sort, depth_caption, Column::Depth);
        header(ui, sort, "change", Column::Change);
        header(ui, sort, "band", Column::Band);
        ui.label("7 days");
        header(ui, sort, "24 h", Column::Day);
        header(ui, sort, "3 d", Column::ThreeDays);
        ui.label("");
        ui.end_row();
        for r in &rows {
            let muted = r.grade != Grade::Liquid;
            let key = (r.category.clone(), r.id.clone());
            let text = if muted { egui::RichText::new(&r.name).weak() } else { egui::RichText::new(&r.name) };
            if ui.selectable_label(selected.as_ref() == Some(&key), text).clicked() {
                *selected = if selected.as_ref() == Some(&key) { None } else { Some(key) };
            }
            ui.label(&r.price);
            ui.label(&r.depth_text);
            signed(ui, r.change, muted);
            match r.band {
                Some(b) => ui.weak(band_text(b)),
                None => ui.label(""),
            };
            let color = match r.change {
                _ if muted => egui::Color32::GRAY,
                Some(c) if c > 0.0 => UP,
                Some(c) if c < 0.0 => DOWN,
                _ => egui::Color32::LIGHT_GRAY,
            };
            chart(ui, egui::vec2(90.0, 20.0), &r.points, color);
            window_cell(ui, r.day, muted);
            window_cell(ui, r.three_days, muted);
            ui.weak(&r.note);
            ui.end_row();
        }
    });
}

fn start_runs_load(state: &mut RunsUi, cfg: &Config) {
    use khaloni_poe2_core::income::StashAccess;
    let access = StashAccess::from_credentials(&cfg.account_name, &cfg.poesessid);
    let league = cfg.league.clone();
    let log = cfg.client_log_path.as_ref().map(std::path::PathBuf::from).or_else(crate::gamelog_tail::default_log_path);
    let (tx, rx) = std::sync::mpsc::channel();
    state.loaded_for = Some((league.clone(), access.clone()));
    state.loaded = None;
    state.exported = None;
    state.loading = Some(rx);
    std::thread::spawn(move || {
        let result = match log {
            Some(path) => crate::myruns::load(&path, &league, access, crate::prices::unix_now())
                .map_err(|e| format!("{}: {e}", path.display())),
            None => Err(crate::myruns::LOG_MISSING.to_string()),
        };
        let _ = tx.send(result);
    });
}

fn section_my_runs(ui: &mut egui::Ui, cfg: &Config, state: &mut RunsUi) {
    use khaloni_poe2_core::income::{self, StashAccess};
    let wanted = (cfg.league.clone(), StashAccess::from_credentials(&cfg.account_name, &cfg.poesessid));
    if state.loaded_for.as_ref() != Some(&wanted) {
        start_runs_load(state, cfg);
    }
    if let Some(rx) = &state.loading {
        match rx.try_recv() {
            Ok(result) => {
                state.loaded = Some(result);
                state.loading = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ui.label(crate::myruns::READING_LOG);
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
                return;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                state.loaded = Some(Err("the log read stopped unexpectedly".to_string()));
                state.loading = None;
            }
        }
    }
    let reload = ui.button("Reload the game log and the stash history").clicked();
    match &state.loaded {
        None => {}
        Some(Err(e)) => {
            ui.colored_label(egui::Color32::from_rgb(0xE0, 0x60, 0x50), e);
        }
        Some(Ok(loaded)) => {
            let view = crate::myruns::View::build(&loaded.summary);
            if !view.has_runs() {
                ui.label(crate::myruns::NO_RUNS);
            }
            egui::Grid::new("my-runs-totals").num_columns(3).striped(true).spacing([18.0, 4.0]).show(ui, |ui| {
                for h in crate::market_ui::RUNS_CAPTIONS {
                    ui.label(egui::RichText::new(h).strong());
                }
                ui.end_row();
                for row in &view.totals {
                    for cell in row {
                        ui.label(cell);
                    }
                    ui.end_row();
                }
            });
            if let Some(note) = &view.totals_note {
                ui.small(note);
            }
            ui.add_space(6.0);
            ui.strong("Income");
            if let Some((rate, sample)) = &view.overall {
                ui.horizontal(|ui| {
                    ui.label(format!("overall: {rate}"));
                    ui.weak(sample);
                });
            }
            if let Some(note) = &view.income_note {
                ui.label(note);
            }
            if !view.mechanics.is_empty() {
                ui.add_space(6.0);
                ui.strong("Mechanics seen");
                egui::Grid::new("my-runs-mechanics").num_columns(3).striped(true).spacing([18.0, 4.0]).show(ui, |ui| {
                    for m in &view.mechanics {
                        ui.label(&m.name);
                        ui.label(&m.seen);
                        if m.has_rate {
                            ui.label(&m.income);
                        } else {
                            ui.weak(&m.income);
                        }
                        ui.end_row();
                    }
                });
                ui.small(crate::myruns::SEEN_NOTE);
            }
            if !loaded.summary.blocks.is_empty() {
                ui.add_space(6.0);
                ui.strong(format!("Blocks ({})", loaded.summary.blocks.len()));
                egui::ScrollArea::vertical().id_salt("my-runs-blocks-scroll").max_height(260.0).show(ui, |ui| {
                    egui::Grid::new("my-runs-blocks").num_columns(7).striped(true).spacing([18.0, 4.0]).show(ui, |ui| {
                        for h in crate::market_ui::BLOCK_CAPTIONS.iter().chain(&["changes without a price"]) {
                            ui.label(egui::RichText::new(*h).strong());
                        }
                        ui.end_row();
                        for b in loaded.summary.blocks.iter().rev() {
                            ui.label(income::time_text(b.to));
                            ui.label(b.maps.to_string());
                            ui.label(match (b.maps, b.map_seconds) {
                                (0, _) => String::new(),
                                (_, Some(secs)) => income::hours_text(secs),
                                (_, None) => "unknown".to_string(),
                            });
                            ui.label(income::mechanics_text(&b.mechanics));
                            match b.figures {
                                Some(f) => {
                                    let text = income::signed_div_text(f.income_div);
                                    if f.income_div > 0.0 {
                                        ui.colored_label(UP, text);
                                    } else if f.income_div < 0.0 {
                                        ui.colored_label(DOWN, text);
                                    } else {
                                        ui.weak(text);
                                    }
                                    ui.weak(income::signed_div_text(f.revaluation_div));
                                    ui.weak(if f.unpriced_changes > 0 { f.unpriced_changes.to_string() } else { String::new() });
                                }
                                None => {
                                    ui.weak(income::PENDING);
                                    ui.label("");
                                    ui.label("");
                                }
                            }
                            ui.end_row();
                        }
                    });
                });
            }
            if let Some(footer) = &view.footer {
                ui.small(footer);
            }
            ui.add_space(6.0);
            if ui.button("Export runs and blocks as CSV").clicked() {
                let now = crate::prices::unix_now();
                let stamp = income::stamp_text(now).replace([':', '-'], "").replace(' ', "-");
                state.exported = Some(
                    crate::myruns::export_csv(&cache_dir().join("exports"), loaded, &format!("{stamp}utc"))
                        .map(|(runs, blocks)| format!("written: {} and {}", runs.display(), blocks.display()))
                        .map_err(|e| format!("export failed: {e}")),
                );
            }
            match &state.exported {
                Some(Ok(text)) => {
                    ui.small(text);
                }
                Some(Err(e)) => {
                    ui.colored_label(egui::Color32::from_rgb(0xE0, 0x60, 0x50), e);
                }
                None => {}
            }
        }
    }
    if reload {
        state.loaded_for = None;
    }
}

pub fn section_market(ui: &mut egui::Ui, cfg: &mut Config, state: &mut MarketUi) {
    ui.heading("Market");
    ui.add_space(4.0);
    // The player's own results, apart from the market's figures and above
    // them, so they show whatever state the price cache is in.
    egui::CollapsingHeader::new("My runs").id_salt("market-my-runs").show(ui, |ui| {
        section_my_runs(ui, cfg, &mut state.runs);
    });
    ui.add_space(8.0);
    state.ensure(cfg);

    // Floors first: they decide what every table below may say.
    ui.label("Liquidity floors");
    ui.horizontal(|ui| {
        ui.label("an exchange item ranks from");
        ui.add(egui::DragValue::new(&mut cfg.market_volume_floor_div).speed(0.5).range(0.0..=100_000.0).suffix(" div"));
        ui.label("traded volume");
    });
    ui.horizontal(|ui| {
        ui.label("a listed item ranks from");
        ui.add(egui::DragValue::new(&mut cfg.market_listings_rank).range(1..=10_000).suffix(" listings"));
        ui.label("and is not trusted under");
        ui.add(egui::DragValue::new(&mut cfg.market_listings_min).range(1..=10_000).suffix(" listings"));
    });
    if let Some(problem) = cfg.market_floors().problem() {
        ui.colored_label(egui::Color32::RED, format!("{problem}: the defaults are used until this is fixed"));
    }
    ui.small(
        "Traded volume is poe.ninja's volumePrimaryValue, in divines. poe.ninja does not publish the period it \
         covers, so no period is named here and the figure is used only to rank, to weigh and against this floor.",
    );
    ui.add_space(8.0);

    let Some(tables) = &state.tables else { return };
    let now = crate::prices::unix_now();
    ui.horizontal(|ui| {
        if tables.model.items.is_empty() {
            ui.label(crate::league::loading_text(&cfg.league));
            ui.weak("(nothing cached yet: the running overlay fetches the prices)");
        } else {
            let part = |what: &str, at: Option<i64>| match at {
                Some(t) => format!("{what} {} old", age_text(now - t)),
                None => format!("{what} age unknown"),
            };
            ui.label(format!(
                "{} · {} · {}, read from the overlay's cache",
                cfg.league,
                part("prices", tables.fetched_at),
                part("uniques", tables.uniques_fetched_at)
            ));
            let oldest = [tables.fetched_at, tables.uniques_fetched_at].into_iter().flatten().min();
            if oldest.is_some_and(|t| now - t > market::GREY_AFTER_SECS) {
                ui.colored_label(egui::Color32::from_rgb(0xE0, 0x60, 0x50), "older than 3 hours");
            }
        }
    });
    let reload = ui.button("Reload from cache").clicked();
    if tables.model.items.is_empty() {
        if reload {
            state.market = None;
        }
        return;
    }
    let d = tables.model.dropped;
    ui.small(format!(
        "{} items. Left out at parse time: {} lines without a usable price, {} volumes and {} history points that \
         were not usable numbers. {} listed uniques sit at the 1 ex floor of their own overview and are untrusted: \
         their move in divines is the exalted orb's.",
        tables.model.items.len(),
        d.prices,
        d.volumes,
        d.points,
        tables.model.floor_pinned
    ));
    ui.small(match tables.first_record {
        Some(first) => format!(
            "History log: {} records since {} ago. The 24 h and 3 d columns fill in once the log spans them.",
            tables.records,
            age_text(now - first)
        ),
        None => "History log: no records yet. The running overlay writes one per price refresh that brought new figures."
            .to_string(),
    });
    if tables.model.young {
        ui.colored_label(egui::Color32::from_rgb(0xE0, 0xB0, 0x50), TOO_YOUNG);
    }
    ui.add_space(8.0);

    ui.horizontal(|ui| {
        ui.selectable_value(&mut state.shown, Shown::Groups, "Mechanics & markets");
        ui.selectable_value(&mut state.shown, Shown::Movers, "Movers");
    });
    ui.add_space(4.0);

    let MarketUi { shown, group, sort, show_untrusted, selected, tables, .. } = state;
    let Some(tables) = tables.as_ref() else { return };
    match shown {
        Shown::Groups => {
            egui::Grid::new("market-groups").num_columns(7).striped(true).spacing([14.0, 4.0]).show(ui, |ui| {
                for h in ["group", "traded volume", "share", "price index", "7 days", "rests on", "breadth"] {
                    ui.label(egui::RichText::new(h).strong());
                }
                ui.end_row();
                for (i, g) in tables.model.groups.iter().enumerate() {
                    let kind = if g.is_mechanic { "" } else { "  (other market)" };
                    if ui.selectable_label(*group == i, format!("{}{kind}", category_label(&g.category))).clicked() {
                        *group = i;
                        *selected = None;
                    }
                    if tables.model.young {
                        for _ in 0..6 {
                            ui.label("");
                        }
                        ui.end_row();
                        continue;
                    }
                    ui.label(volume_text(g.volume_div));
                    ui.label(share_text(g.volume_share));
                    match g.index_pct {
                        Some(ix) => {
                            ui.horizontal(|ui| {
                                signed(ui, Some(ix), false);
                                ui.weak(verdict_text(&g.verdict));
                            });
                        }
                        None => {
                            ui.label("");
                        }
                    }
                    let color = if g.index_pct.unwrap_or(0.0) >= 0.0 { UP } else { DOWN };
                    chart(ui, egui::vec2(110.0, 22.0), &g.index_points, color);
                    ui.weak(coverage_text(g));
                    ui.weak(breadth_text(&g.breadth));
                    ui.end_row();
                }
            });
            ui.add_space(10.0);
            if let (Some(g), Some(rows)) = (tables.model.groups.get(*group), tables.group_rows.get(*group)) {
                ui.strong(category_label(&g.category));
                item_table(ui, "market-items", rows, sort, "traded volume", selected);
                detail(ui, rows, selected);
            }
        }
        Shown::Movers => {
            if tables.model.young {
                ui.label(TOO_YOUNG);
                return;
            }
            ui.checkbox(
                show_untrusted,
                format!("show the {} untrusted listed items (no trend is given for them)", tables.untrusted.len()),
            );
            if *show_untrusted {
                item_table(ui, "market-untrusted", &tables.untrusted, sort, "listed", selected);
                ui.add_space(10.0);
            }
            for (title, rows, id) in [("Risers", &tables.risers, "market-risers"), ("Fallers", &tables.fallers, "market-fallers")] {
                ui.strong(format!("{title} ({})", rows.len()));
                item_table(ui, id, rows, sort, "traded volume / listed", selected);
                detail(ui, rows, selected);
                ui.add_space(10.0);
            }
        }
    }
    if reload {
        state.market = None;
    }
}

/// The selected item's week, large.
fn detail(ui: &mut egui::Ui, rows: &[Row], selected: &Option<(String, String)>) {
    let Some(r) = selected.as_ref().and_then(|(c, i)| rows.iter().find(|r| &r.category == c && &r.id == i)) else {
        return;
    };
    ui.add_space(6.0);
    ui.group(|ui| {
        ui.strong(&r.name);
        let color = match r.change {
            Some(c) if c < 0.0 => DOWN,
            _ => UP,
        };
        chart(ui, egui::vec2(ui.available_width().min(520.0), 120.0), &r.points, color);
        let days: Vec<String> =
            r.points.iter().map(|p| p.map(|v| format!("{v:+.0}%")).unwrap_or_else(|| "no data".into())).collect();
        ui.small(format!("day by day against the first day: {}", days.join("  ·  ")));
        if !r.note.is_empty() {
            ui.small(&r.note);
        }
    });
}
