//! Settings > Crafting: the planner's run count, give-up multiple and the
//! observed model's minimum sample (in config.toml), and the flip profiles
//! (in `profiles.toml` beside it), with a Scan button per profile.
//!
//! A scan is never sent from this window. The settings window is its own
//! process, and the trade site counts requests per address: a second
//! client here would spend the searches the overlay's limiter believes it
//! still has. So Scan states the scan's searches and fetches from the
//! profile, and "Send to the overlay" leaves the request for the overlay,
//! whose craft panel states the budget and waits for Run.
//!
//! [`ProfilesDoc`] is the pure part: a profiles file read with its
//! malformed profiles kept verbatim, so saving the good ones never drops
//! the ones that need fixing.

use std::sync::{Arc, Mutex};

use eframe::egui;

use khaloni_poe2_core::craft::data::CraftData;
use khaloni_poe2_core::ee2::data::StatDb;
use khaloni_poe2_core::flip::{self, Profile, ProfileError, Wanted, DEFAULT_MARGIN};
use khaloni_poe2_core::trade::StatIndex;

use crate::config::{Config, CRAFT_GIVE_UP_RANGE, CRAFT_OBSERVED_MIN_RANGE, CRAFT_RUNS_RANGE};
use crate::craft_flow;

/// A profiles file as the tab edits it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProfilesDoc {
    pub profiles: Vec<Profile>,
    /// Profiles that did not load, with their reasons.
    pub errors: Vec<ProfileError>,
    /// The tables of the profiles that did not load, written back as they
    /// were.
    kept: Vec<toml::Value>,
    /// Keys of the file that are not profiles, written back as they were.
    extra: toml::Table,
    /// Why the file cannot be written over: it is not TOML, or its
    /// `profile` key is not a list.
    pub file_error: Option<String>,
}

impl ProfilesDoc {
    pub fn parse(text: &str) -> ProfilesDoc {
        let loaded = flip::profiles_from_toml(text);
        let mut doc = ProfilesDoc { profiles: loaded.profiles, errors: loaded.errors, ..Default::default() };
        let mut table: toml::Table = match text.parse() {
            Ok(t) => t,
            Err(e) => {
                doc.file_error = Some(format!("profiles.toml is not valid TOML ({}); fix or remove it first", e.message()));
                return doc;
            }
        };
        let listed = match table.remove("profile") {
            None => Vec::new(),
            Some(toml::Value::Array(a)) => a,
            Some(_) => {
                doc.file_error = Some("\"profile\" in profiles.toml is not a list of [[profile]] tables".to_string());
                return doc;
            }
        };
        doc.extra = table;
        doc.kept = doc.errors.iter().filter_map(|e| e.index.and_then(|i| listed.get(i).cloned())).collect();
        doc
    }

    pub fn load(path: &std::path::Path) -> ProfilesDoc {
        match std::fs::read_to_string(path) {
            Ok(text) => ProfilesDoc::parse(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => ProfilesDoc::default(),
            Err(e) => ProfilesDoc { file_error: Some(format!("profiles.toml cannot be read: {e}")), ..Default::default() },
        }
    }

    /// The file's text: the profiles, then the kept malformed ones and the
    /// other keys unchanged. Every profile is checked first.
    pub fn to_text(&self) -> Result<String, String> {
        if let Some(why) = &self.file_error {
            return Err(why.clone());
        }
        let mut list = Vec::new();
        for (i, p) in self.profiles.iter().enumerate() {
            p.validate().map_err(|reason| format!("profile {} (\"{}\"): {reason}", i + 1, p.name))?;
            if self.profiles[..i].iter().any(|q| q.name == p.name) {
                return Err(format!("profile {} (\"{}\"): an earlier profile has the same name", i + 1, p.name));
            }
            list.push(toml::Value::try_from(p).map_err(|e| format!("profile \"{}\" does not write as TOML: {e}", p.name))?);
        }
        list.extend(self.kept.iter().cloned());
        let mut table = self.extra.clone();
        table.insert("profile".to_string(), toml::Value::Array(list));
        toml::to_string(&table).map_err(|e| format!("the profiles do not write as TOML: {e}"))
    }

    pub fn save(&self, path: &std::path::Path) -> Result<(), String> {
        let text = self.to_text()?;
        crate::config::write_atomic(path, &text).map_err(|e| format!("{} not written: {e}", path.display()))
    }
}

/// What a scan statement is worked out from, read from the overlay's cache.
struct ScanData {
    craft: CraftData,
    stats: StatDb,
    catalog: StatIndex,
}

fn load_scan_data() -> Result<ScanData, String> {
    let cache = directories::ProjectDirs::from("", "", "khaloni-poe2")
        .map(|d| d.cache_dir().to_path_buf())
        .unwrap_or_else(std::env::temp_dir);
    let files = crate::refcache::craft_files(&cache)?;
    let craft = CraftData::load(&files.mods, &files.bases, &files.essences)?;
    let stats = crate::refcache::try_ee2_data(&cache)?.stats;
    let body = std::fs::read_to_string(cache.join("trade_stats.json"))
        .map_err(|_| "the trade stats catalog is not cached yet: run one price check in the overlay first".to_string())?;
    let catalog = StatIndex::from_json(&body).map_err(|e| format!("the cached trade stats catalog is unreadable: {e}"))?;
    Ok(ScanData { craft, stats, catalog })
}

/// The tab's state.
#[derive(Default)]
pub struct CraftUi {
    doc: Option<ProfilesDoc>,
    /// Edits not yet saved.
    dirty: bool,
    saved: Option<Result<(), String>>,
    data: Arc<Mutex<Option<Result<ScanData, String>>>>,
    data_started: bool,
    /// The profile whose scan statement is shown, and the statement or why
    /// the profile cannot be scanned.
    statement: Option<(String, Result<String, String>)>,
    sent: Option<Result<String, String>>,
}

fn config_dir() -> Option<std::path::PathBuf> {
    Config::path().parent().map(std::path::Path::to_path_buf)
}

impl CraftUi {
    fn ensure(&mut self) {
        if self.doc.is_none() {
            self.doc = Some(config_dir().map(|d| ProfilesDoc::load(&craft_flow::profiles_path(&d))).unwrap_or_default());
        }
    }

    fn start_data(&mut self) {
        if self.data_started {
            return;
        }
        self.data_started = true;
        let slot = self.data.clone();
        let spawned = std::thread::Builder::new().name("settings-craft-data".into()).spawn(move || {
            let loaded = load_scan_data();
            *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(loaded);
        });
        if let Err(e) = spawned {
            *self.data.lock().unwrap_or_else(|e| e.into_inner()) = Some(Err(format!("could not start loading: {e}")));
        }
    }

    fn scan_statement(&self, profile: &Profile) -> Result<String, String> {
        let data = self.data.lock().unwrap_or_else(|e| e.into_inner());
        match data.as_ref() {
            None => Err("the mod and trade data are still loading".to_string()),
            Some(Err(why)) => Err(why.clone()),
            Some(Ok(d)) => {
                let plan = craft_flow::scan_plan(profile, &d.craft, &d.stats, &d.catalog)?;
                let mut text = format!(
                    "this scan: {} searches, {} fetches; the overlay states its budget and waits for Run before sending",
                    plan.cost.searches, plan.cost.fetches
                );
                for s in &plan.relaxations.skipped {
                    text.push_str(&format!("\n{s}"));
                }
                Ok(text)
            }
        }
    }
}

fn planner_settings(ui: &mut egui::Ui, cfg: &mut Config) {
    ui.label(egui::RichText::new("Planner").strong());
    egui::Grid::new("craft-planner").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
        ui.label("Runs per strategy");
        ui.add(egui::DragValue::new(&mut cfg.craft_runs).range(CRAFT_RUNS_RANGE.0..=CRAFT_RUNS_RANGE.1).speed(100.0));
        ui.end_row();
        ui.label("Give up after (x the median uses)");
        ui.add(
            egui::DragValue::new(&mut cfg.craft_give_up_times)
                .range(CRAFT_GIVE_UP_RANGE.0..=CRAFT_GIVE_UP_RANGE.1)
                .speed(0.5),
        );
        ui.end_row();
        ui.label("Observed model: minimum listings");
        ui.add(
            egui::DragValue::new(&mut cfg.craft_observed_min)
                .range(CRAFT_OBSERVED_MIN_RANGE.0..=CRAFT_OBSERVED_MIN_RANGE.1)
                .speed(10.0),
        );
        ui.end_row();
    });
    ui.small(
        "More runs give steadier figures and slower plans: at 20,000 a slow strategy takes several seconds. A run \
         gives up once it has used this many times the median of the runs that finished. A class's observed \
         figures are used once this many of its listings are recorded.",
    );
}

fn profile_editor(ui: &mut egui::Ui, i: usize, p: &mut Profile, dirty: &mut bool) -> (bool, bool) {
    let (mut remove, mut scan) = (false, false);
    egui::Grid::new(("craft-profile", i)).num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
        ui.label("Name");
        *dirty |= ui.text_edit_singleline(&mut p.name).changed();
        ui.end_row();
        ui.label("Item class");
        *dirty |= ui.text_edit_singleline(&mut p.class).changed();
        ui.end_row();
        ui.label("Base (empty: the whole class)");
        let mut base = p.base.clone().unwrap_or_default();
        if ui.text_edit_singleline(&mut base).changed() {
            p.base = (!base.trim().is_empty()).then(|| base.trim().to_string());
            *dirty = true;
        }
        ui.end_row();
        ui.label("Item level at least");
        *dirty |= ui.add(egui::DragValue::new(&mut p.min_ilvl).range(0..=100)).changed();
        ui.end_row();
        ui.label("Margin wanted (%)");
        *dirty |= ui.add(egui::DragValue::new(&mut p.margin).range(0.0..=1000.0).speed(1.0)).changed();
        ui.end_row();
    });
    ui.label("Wanted modifiers (mod family, worst acceptable tier; tier 1 is the best)");
    let mut drop = None;
    for (j, w) in p.wants.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            *dirty |= ui.add(egui::TextEdit::singleline(&mut w.family).desired_width(260.0)).changed();
            ui.label("T");
            *dirty |= ui.add(egui::DragValue::new(&mut w.min_tier).range(1..=30)).changed();
            if ui.small_button("remove").clicked() {
                drop = Some(j);
            }
        });
    }
    if let Some(j) = drop {
        p.wants.remove(j);
        *dirty = true;
    }
    ui.horizontal(|ui| {
        if ui.small_button("add a modifier").clicked() {
            p.wants.push(Wanted { family: String::new(), min_tier: 1 });
            *dirty = true;
        }
        if ui.small_button("remove this profile").clicked() {
            remove = true;
        }
        scan = ui.button("Scan").clicked();
    });
    (remove, scan)
}

pub fn section_crafting(ui: &mut egui::Ui, cfg: &mut Config, state: &mut CraftUi) {
    ui.heading("Crafting");
    ui.add_space(6.0);
    planner_settings(ui, cfg);
    ui.add_space(14.0);
    ui.label(egui::RichText::new("Flip profiles").strong());
    ui.small(
        "A profile names a finished item. A scan runs one search for it (its listings are the resale) and one \
         relaxed search per wanted modifier and tier; each listing found is costed by the planner. Scans run only \
         when you ask, one profile at a time.",
    );
    state.ensure();
    state.start_data();
    let Some(dir) = config_dir() else {
        ui.colored_label(egui::Color32::RED, "no config directory: profiles cannot be kept");
        return;
    };
    let path = craft_flow::profiles_path(&dir);
    ui.small(format!("kept in {}", path.display()));
    ui.add_space(6.0);

    let mut doc = state.doc.take().unwrap_or_default();
    if let Some(why) = &doc.file_error {
        ui.colored_label(egui::Color32::from_rgb(0xE0, 0x60, 0x50), why);
    }
    for e in &doc.errors {
        ui.colored_label(egui::Color32::from_rgb(0xE0, 0x60, 0x50), format!("not loaded: {e} (kept in the file as it is)"));
    }

    let mut remove = None;
    let mut scan = None;
    for (i, p) in doc.profiles.iter_mut().enumerate() {
        let title = if p.name.trim().is_empty() { format!("profile {}", i + 1) } else { p.name.clone() };
        egui::CollapsingHeader::new(title).id_salt(("craft-profile-head", i)).default_open(false).show(ui, |ui| {
            let (r, s) = profile_editor(ui, i, p, &mut state.dirty);
            if r {
                remove = Some(i);
            }
            if s {
                scan = Some(i);
            }
            if let Some((name, statement)) = &state.statement {
                if *name == p.name {
                    match statement {
                        Ok(text) => {
                            ui.label(text);
                            let can_send = !state.dirty;
                            if ui.add_enabled(can_send, egui::Button::new("Send to the overlay")).clicked() {
                                state.sent = Some(
                                    craft_flow::request_scan(&dir, &p.name)
                                        .map(|()| "sent: the overlay's craft panel shows the budget and a Run scan button".to_string()),
                                );
                            }
                            if !can_send {
                                ui.small("save the profiles first: the overlay reads the saved file");
                            }
                        }
                        Err(why) => {
                            ui.colored_label(egui::Color32::from_rgb(0xE0, 0x60, 0x50), why);
                        }
                    }
                }
            }
        });
    }
    if let Some(i) = remove {
        doc.profiles.remove(i);
        state.dirty = true;
        state.statement = None;
    }
    if let Some(i) = scan {
        let p = &doc.profiles[i];
        state.statement = Some((p.name.clone(), state.scan_statement(p)));
        state.sent = None;
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button("Add a profile").clicked() {
            doc.profiles.push(Profile {
                name: format!("profile {}", doc.profiles.len() + 1),
                class: "Body Armour".to_string(),
                base: None,
                min_ilvl: 0,
                wants: vec![Wanted { family: String::new(), min_tier: 1 }],
                margin: DEFAULT_MARGIN,
            });
            state.dirty = true;
        }
        if ui.add_enabled(state.dirty, egui::Button::new("Save profiles")).clicked() {
            let saved = doc.save(&path);
            if saved.is_ok() {
                state.dirty = false;
            }
            state.saved = Some(saved);
        }
        if ui.button("Reload from the file").clicked() {
            doc = ProfilesDoc::load(&path);
            state.dirty = false;
            state.saved = None;
            state.statement = None;
        }
    });
    match &state.saved {
        Some(Ok(())) if !state.dirty => {
            ui.weak("profiles saved");
        }
        Some(Err(why)) => {
            ui.colored_label(egui::Color32::RED, format!("not saved: {why}"));
        }
        _ => {}
    }
    if let Some(sent) = &state.sent {
        match sent {
            Ok(text) => ui.weak(text),
            Err(why) => ui.colored_label(egui::Color32::RED, why),
        };
    }
    state.doc = Some(doc);
}
