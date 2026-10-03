//! Native settings window (`khaloni-poe2 --settings`), eframe/egui.
//!
//! All edits go through [`EditModel`], a pure struct with no egui types so
//! the binding/validation rules are unit-testable without a display. The
//! window autosaves config.toml (atomically, via `Config::save`); the
//! overlay hot-reloads it by mtime, so no IPC is needed.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use eframe::egui;

use crate::config::{Config, Macro, ResourceShortcut};

/// Which key-capture button is armed. Fixed hotkeys get their own variant;
/// macro/shortcut rows are addressed by index because the user can add and
/// remove them freely.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CaptureTarget {
    PriceCheck,
    Settings,
    Market,
    Macro(usize),
    Shortcut(usize),
    Upgrade,
    Craft,
}

/// The pure edit state behind the settings window: the config being edited,
/// whether it has unsaved changes, and which binding (if any) is waiting for
/// a keypress.
pub struct EditModel {
    pub cfg: Config,
    pub dirty: bool,
    pub capture: Option<CaptureTarget>,
}

impl EditModel {
    pub fn from_config(cfg: Config) -> EditModel {
        EditModel {
            cfg,
            dirty: false,
            capture: None,
        }
    }

    /// Write a captured portal trigger string into the field `target` points
    /// at. Row indices can go stale (delete racing a pending capture), so an
    /// out-of-range Macro/Shortcut is a no-op rather than a panic.
    pub fn apply_key(&mut self, target: CaptureTarget, key: String) {
        let slot = match target {
            CaptureTarget::PriceCheck => Some(&mut self.cfg.hotkey_price_check),
            CaptureTarget::Settings => Some(&mut self.cfg.hotkey_settings),
            CaptureTarget::Market => Some(&mut self.cfg.hotkey_market),
            CaptureTarget::Upgrade => Some(&mut self.cfg.hotkey_upgrade),
            CaptureTarget::Craft => Some(&mut self.cfg.hotkey_craft),
            CaptureTarget::Macro(i) => self.cfg.macros.get_mut(i).map(|m| &mut m.key),
            CaptureTarget::Shortcut(i) => {
                self.cfg.resource_shortcuts.get_mut(i).map(|s| &mut s.key)
            }
        };
        if let Some(slot) = slot {
            *slot = key;
            self.dirty = true;
        }
    }

    /// Equal thresholds collapse the decent band to nothing, which is legal.
    pub fn tier_valid(&self) -> bool {
        self.cfg.tier_decent_chaos <= self.cfg.tier_good_chaos
    }

    pub fn save(&mut self) -> anyhow::Result<()> {
        self.cfg.save()?;
        self.dirty = false;
        Ok(())
    }
}

pub fn run() -> anyhow::Result<()> {
    // eframe's accessibility layer (accesskit) speaks AT-SPI over zbus, and
    // our dependency graph compiles zbus in tokio mode (ashpd's portal
    // stack), which panics without an ambient tokio reactor. Entering a
    // runtime context here gives every thread spawned under it a reactor.
    let rt = tokio::runtime::Runtime::new()?;
    let _guard = rt.enter();
    let cfg = Config::load()?;
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([760.0, 640.0])
            .with_min_inner_size([560.0, 480.0]),
        ..Default::default()
    };
    eframe::run_native(
        "khaloni-poe2 settings",
        options,
        Box::new(move |_cc| Ok(Box::new(SettingsApp::new(cfg)))),
    )
    .map_err(|e| anyhow::anyhow!("settings window: {e}"))
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Section {
    Hotkeys,
    Display,
    Pricing,
    Market,
    Crafting,
    CaptureOcr,
    MacrosShortcuts,
    RunWithGame,
    Waystones,
    Account,
    Updates,
}

const SECTIONS: [(Section, &str); 11] = [
    (Section::Hotkeys, "Hotkeys"),
    (Section::Display, "Display"),
    (Section::Pricing, "Pricing"),
    (Section::Market, "Market"),
    (Section::Crafting, "Crafting"),
    (Section::CaptureOcr, "Capture & OCR"),
    (Section::MacrosShortcuts, "Macros & Shortcuts"),
    (Section::RunWithGame, "Run with the Game"),
    (Section::Waystones, "Waystones"),
    (Section::Account, "Account"),
    (Section::Updates, "Updates"),
];

const AUTOSAVE_DEBOUNCE: Duration = Duration::from_millis(300);

struct SettingsApp {
    model: EditModel,
    section: Section,
    /// League names land here from a one-shot background fetch; empty means
    /// still loading (or the fetch failed) and the UI falls back to free text.
    leagues: Arc<Mutex<Vec<String>>>,
    /// Canonical mod texts (lowercased, `#` for the rolled number) for the
    /// waystone-needle autocomplete, from the same cached reference data the
    /// overlay's price card grades tiers with. Empty while loading.
    mods: Arc<Mutex<Vec<String>>>,
    /// The needle row currently showing suggestions: (list id, row index).
    /// Tracked explicitly instead of via widget focus so clicking a
    /// suggestion button (which steals focus) still lands.
    suggest: Option<(&'static str, usize)>,
    /// Which needles the stash-regex builder has checked. Ephemeral by
    /// design — the composed regex is a scratch artifact the user copies
    /// into the game, not a setting, so it lives here and not in Config.
    /// Keyed by needle text so it survives edits to the needle lists.
    stash_selected: BTreeSet<String>,
    /// Change detection: Config has no PartialEq, but it serializes, so one
    /// toml snapshot per frame catches every widget edit in one place.
    /// Update check/install state, driven by background threads.
    updates: crate::settings_update::UpdateUi,
    /// Wealth snapshots loaded once at window start (display only).
    wealth_history: Vec<crate::wealth::WealthSnapshot>,
    /// The Market tab's tables, read from the overlay's price cache.
    market: crate::settings_market::MarketUi,
    /// The Crafting tab's profiles and scan requests.
    craft: crate::settings_craft::CraftUi,
    last_serialized: String,
    last_edit: Instant,
    saved_at: Option<String>,
    save_err: Option<String>,
}

impl SettingsApp {
    fn new(cfg: Config) -> SettingsApp {
        let leagues: Arc<Mutex<Vec<String>>> = Arc::default();
        {
            // One fetch per window; NinjaClient is blocking, so keep it off
            // the frame thread. The Arc write flips the combo in on arrival.
            let leagues = leagues.clone();
            let spawned = std::thread::Builder::new().name("settings-leagues".into()).spawn(move || {
                let cache = directories::ProjectDirs::from("", "", "khaloni-poe2")
                    .map(|d| d.cache_dir().to_path_buf())
                    .unwrap_or_else(std::env::temp_dir);
                let client = khaloni_poe2_core::ninja::NinjaClient::new(cache);
                if let Ok(ls) = client.leagues() {
                    *leagues.lock().unwrap() = ls.into_iter().map(|l| l.name).collect();
                }
            });
            if let Err(e) = spawned {
                eprintln!("settings: league list not loaded: {e}");
            }
        }
        let mods: Arc<Mutex<Vec<String>>> = Arc::default();
        {
            // Reference data is disk-cached by the overlay; a cold cache
            // fetches once. Off the frame thread like the league fetch.
            let mods = mods.clone();
            let spawned = std::thread::Builder::new().name("settings-mods".into()).spawn(move || {
                let cache = directories::ProjectDirs::from("", "", "khaloni-poe2")
                    .map(|d| d.cache_dir().to_path_buf())
                    .unwrap_or_else(std::env::temp_dir);
                let r = crate::refcache::reference_data(&cache);
                *mods.lock().unwrap() =
                    r.affixes.iter().map(|a| a.text.to_lowercase()).collect();
            });
            if let Err(e) = spawned {
                eprintln!("settings: mod suggestions not loaded: {e}");
            }
        }
        let last_serialized = toml::to_string(&cfg).unwrap_or_default();
        let wealth_history = crate::wealth::load_history(&cfg.league, 10);
        SettingsApp {
            model: EditModel::from_config(cfg),
            section: Section::Hotkeys,
            leagues,
            mods,
            suggest: None,
            stash_selected: BTreeSet::new(),
            updates: crate::settings_update::UpdateUi::default(),
            wealth_history,
            market: crate::settings_market::MarketUi::default(),
            craft: crate::settings_craft::CraftUi::default(),
            last_serialized,
            last_edit: Instant::now(),
            saved_at: None,
            save_err: None,
        }
    }

    /// While a capture is armed, the next recognizable keypress becomes the
    /// binding; Escape disarms without writing.
    fn handle_capture(&mut self, ctx: &egui::Context) {
        let Some(target) = self.model.capture else {
            return;
        };
        for ev in ctx.input(|i| i.events.clone()) {
            if let egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } = ev
            {
                if key == egui::Key::Escape {
                    self.model.capture = None;
                    break;
                }
                if let Some(s) = portal_key(key, modifiers) {
                    self.model.apply_key(target, s);
                    self.model.capture = None;
                    break;
                }
            }
        }
    }

    fn autosave(&mut self, ctx: &egui::Context) {
        let now = toml::to_string(&self.model.cfg).unwrap_or_default();
        if now != self.last_serialized {
            self.last_serialized = now;
            self.model.dirty = true;
            self.last_edit = Instant::now();
        }
        if !self.model.dirty {
            return;
        }
        if self.last_edit.elapsed() >= AUTOSAVE_DEBOUNCE {
            match self.model.save() {
                Ok(()) => {
                    self.saved_at = Some(hms_now());
                    self.save_err = None;
                }
                Err(e) => {
                    self.save_err = Some(e.to_string());
                    // Rearm the debounce so a persistent failure (read-only
                    // fs, full disk) retries at 300ms, not every frame.
                    self.last_edit = Instant::now();
                }
            }
        } else {
            // Nothing wakes egui once input stops; schedule the save tick.
            ctx.request_repaint_after(AUTOSAVE_DEBOUNCE);
        }
    }
}

impl eframe::App for SettingsApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_capture(&ctx);

        // Bottom before left so the status bar spans the full window width.
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                if let Some(err) = &self.save_err {
                    ui.colored_label(egui::Color32::RED, format!("save failed: {err}"));
                } else if let Some(at) = &self.saved_at {
                    ui.weak(format!("Saved {at}"));
                }
            });
        });

        egui::Panel::left("nav")
            .resizable(false)
            .default_size(170.0)
            .show(ui, |ui| {
                ui.add_space(8.0);
                for (section, label) in SECTIONS {
                    ui.selectable_value(&mut self.section, section, label);
                }
            });

        let Self {
            model,
            section,
            leagues,
            mods,
            suggest,
            stash_selected,
            wealth_history,
            updates,
            market,
            craft,
            ..
        } = self;
        updates.poll();
        let mod_list = mods.lock().unwrap().clone();
        let tier_ok = model.tier_valid();
        let EditModel { cfg, capture, .. } = model;

        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .show(ui, |ui| match section {
                    Section::Hotkeys => section_hotkeys(ui, cfg, capture),
                    Section::Display => section_display(ui, cfg, tier_ok),
                    Section::Pricing => section_pricing(ui, cfg, leagues),
                    Section::Market => crate::settings_market::section_market(ui, cfg, market),
                    Section::Crafting => crate::settings_craft::section_crafting(ui, cfg, craft),
                    Section::CaptureOcr => {
                        section_capture_ocr(ui)
                    }
                    Section::MacrosShortcuts => section_macros(ui, cfg, capture),
                    Section::RunWithGame => crate::settings_launch::section_launch(ui),
                    Section::Updates => {
                        crate::settings_update::section_updates(ui, cfg, updates)
                    }
                    Section::Account => {
                        crate::settings_account::section_account(ui, cfg, wealth_history)
                    }
                    Section::Waystones => {
                        section_waystones(ui, cfg, &mod_list, suggest, stash_selected)
                    }
                });
        });

        self.autosave(&ctx);
    }
}

/// Map an egui key event to the portal GlobalShortcuts trigger syntax the
/// rest of the app stores ("F7", "CTRL+1"). Only the keys the overlay can
/// actually bind are accepted; anything else keeps the capture armed.
fn portal_key(key: egui::Key, modifiers: egui::Modifiers) -> Option<String> {
    use egui::Key as K;
    let base = match key {
        K::F1 => "F1",
        K::F2 => "F2",
        K::F3 => "F3",
        K::F4 => "F4",
        K::F5 => "F5",
        K::F6 => "F6",
        K::F7 => "F7",
        K::F8 => "F8",
        K::F9 => "F9",
        K::F10 => "F10",
        K::F11 => "F11",
        K::F12 => "F12",
        K::Num0 => "0",
        K::Num1 => "1",
        K::Num2 => "2",
        K::Num3 => "3",
        K::Num4 => "4",
        K::Num5 => "5",
        K::Num6 => "6",
        K::Num7 => "7",
        K::Num8 => "8",
        K::Num9 => "9",
        K::A => "A",
        K::B => "B",
        K::C => "C",
        K::D => "D",
        K::E => "E",
        K::F => "F",
        K::G => "G",
        K::H => "H",
        K::I => "I",
        K::J => "J",
        K::K => "K",
        K::L => "L",
        K::M => "M",
        K::N => "N",
        K::O => "O",
        K::P => "P",
        K::Q => "Q",
        K::R => "R",
        K::S => "S",
        K::T => "T",
        K::U => "U",
        K::V => "V",
        K::W => "W",
        K::X => "X",
        K::Y => "Y",
        K::Z => "Z",
        _ => return None,
    };
    Some(if modifiers.ctrl {
        format!("CTRL+{base}")
    } else {
        base.to_string()
    })
}

/// A button that shows the current binding, or "press a key…" while armed.
fn capture_button(
    ui: &mut egui::Ui,
    current: &str,
    target: CaptureTarget,
    capture: &mut Option<CaptureTarget>,
) {
    let armed = *capture == Some(target);
    let label = if armed {
        "press a key…"
    } else if current.is_empty() {
        "unbound"
    } else {
        current
    };
    if ui.button(label).clicked() {
        *capture = Some(target);
    }
}

fn section_hotkeys(ui: &mut egui::Ui, cfg: &mut Config, capture: &mut Option<CaptureTarget>) {
    ui.heading("Hotkeys");
    ui.add_space(6.0);
    // The same resolution the overlay binds from (`triggers::dedupe` under
    // it), so what is marked here is exactly what will be left unbound.
    let conflicts = crate::bindings::resolve(cfg).conflicts;
    egui::Grid::new("hotkeys")
        .num_columns(2)
        .spacing([16.0, 8.0])
        .show(ui, |ui| {
            let rows: [(&str, &str, CaptureTarget); 5] = [
                ("Price check", &cfg.hotkey_price_check, CaptureTarget::PriceCheck),
                ("Settings panel", &cfg.hotkey_settings, CaptureTarget::Settings),
                ("Market panel", &cfg.hotkey_market, CaptureTarget::Market),
                ("Upgrade check", &cfg.hotkey_upgrade, CaptureTarget::Upgrade),
                ("Craft planner", &cfg.hotkey_craft, CaptureTarget::Craft),
            ];
            // Bindings are cloned so capture_button can take &mut capture
            // while cfg stays borrowed by the row labels.
            let rows: Vec<(String, String, CaptureTarget)> = rows
                .into_iter()
                .map(|(l, k, t)| (l.to_string(), k.to_string(), t))
                .collect();
            for (label, key, target) in rows {
                ui.label(label);
                ui.horizontal(|ui| {
                    capture_button(ui, &key, target, capture);
                    conflict_label(ui, &conflicts, slot_of(target));
                });
                ui.end_row();
            }
        });
    ui.add_space(6.0);
    ui.small("a running overlay picks hotkey changes up within a second (KDE shows one approval dialog)");
    ui.add_space(10.0);
    ui.checkbox(&mut cfg.advanced_copy, "Copy items with advanced descriptions (Ctrl+Alt+C)");
    ui.small(
        "Off sends plain Ctrl+C, which copies what the game's \"advanced mod descriptions\" option says: with that \
         option off, the copy has no readable modifiers and the item is not priced.",
    );
}

/// The binding row a capture target edits, in `bindings` terms.
fn slot_of(target: CaptureTarget) -> crate::bindings::Slot {
    use crate::bindings::Slot;
    match target {
        CaptureTarget::PriceCheck => Slot::PriceCheck,
        CaptureTarget::Settings => Slot::Settings,
        CaptureTarget::Market => Slot::Market,
        CaptureTarget::Upgrade => Slot::Upgrade,
        CaptureTarget::Craft => Slot::Craft,
        CaptureTarget::Macro(i) => Slot::Macro(i),
        CaptureTarget::Shortcut(i) => Slot::Shortcut(i),
    }
}

/// Marks a row whose key another binding got first. The overlay leaves such
/// a row unbound, and a settings window that accepted the duplicate without
/// a word was where the "my F10 does nothing" report came from.
fn conflict_label(ui: &mut egui::Ui, conflicts: &[crate::bindings::Conflict], slot: crate::bindings::Slot) {
    if let Some(c) = crate::bindings::conflict_for(conflicts, slot) {
        ui.colored_label(egui::Color32::from_rgb(0xE0, 0x60, 0x50), format!("off: {}", c.message));
    }
}

fn section_display(ui: &mut egui::Ui, cfg: &mut Config, tier_ok: bool) {
    ui.heading("Display");
    ui.add_space(6.0);
    ui.checkbox(&mut cfg.pause_when_hidden, "Hide overlay when the game is minimized or covered");
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label("Overlay opacity");
        // In-game overlay only — this window is unaffected by design.
        // Floor at 10%: a fully invisible overlay reads as broken (the
        // hotkeys still work with nothing on screen to show for it).
        let mut pct = (cfg.overlay_opacity * 100.0).round().clamp(10.0, 100.0) as u32;
        if ui.add(egui::Slider::new(&mut pct, 10..=100).suffix("%")).changed() {
            cfg.overlay_opacity = f64::from(pct) / 100.0;
        }
    });
    ui.horizontal(|ui| {
        // The stored number has always been compared against the value
        // in divines (`value::pick_unit`); only this label called it ex.
        ui.label("Show a value in divines from");
        ui.add(
            egui::DragValue::new(&mut cfg.divine_threshold)
                .speed(0.1)
                .range(0.0..=10000.0)
                .suffix(" div"),
        );
    });

    ui.add_space(12.0);
    ui.label("Value tiers (in chaos: the exalted orb's worth moves too much to set a bar by)");
    ui.horizontal(|ui| {
        ui.label("decent from");
        ui.add(
            egui::DragValue::new(&mut cfg.tier_decent_chaos)
                .speed(0.1)
                .range(0.0..=10000.0)
                .suffix(" chaos"),
        );
        ui.label("jackpot from");
        ui.add(
            egui::DragValue::new(&mut cfg.tier_good_chaos)
                .speed(0.1)
                .range(0.0..=10000.0)
                .suffix(" chaos"),
        );
    });
    tier_bar(ui, cfg, tier_ok);
    if !tier_ok {
        ui.colored_label(egui::Color32::RED, "decent must be ≤ jackpot");
    }
}

/// Three zones on a log10 scale over 0.1..1000 chaos, so the junk/decent split
/// stays visible even though jackpot thresholds run two orders higher.
fn tier_bar(ui: &mut egui::Ui, cfg: &Config, tier_ok: bool) {
    let width = ui.available_width().min(420.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 24.0), egui::Sense::hover());
    let painter = ui.painter();
    if !tier_ok {
        painter.rect_filled(rect, 3, egui::Color32::from_rgb(0xB0, 0x30, 0x30));
        return;
    }
    let frac = |v: f64| ((v.clamp(0.1, 1000.0).log10() + 1.0) / 4.0) as f32;
    let x1 = rect.left() + rect.width() * frac(cfg.tier_decent_chaos);
    let x2 = rect.left() + rect.width() * frac(cfg.tier_good_chaos);
    let zone = |a: f32, b: f32| {
        egui::Rect::from_min_max(egui::pos2(a, rect.top()), egui::pos2(b, rect.bottom()))
    };
    painter.rect_filled(zone(rect.left(), x1), 0, egui::Color32::from_rgb(0x6E, 0x65, 0x5A));
    painter.rect_filled(zone(x1, x2), 0, egui::Color32::from_rgb(0x2E, 0x5A, 0x8A));
    painter.rect_filled(zone(x2, rect.right()), 0, egui::Color32::from_rgb(0xC9, 0xA2, 0x27));
}

fn section_pricing(ui: &mut egui::Ui, cfg: &mut Config, leagues: &Arc<Mutex<Vec<String>>>) {
    ui.heading("Pricing");
    ui.add_space(6.0);
    let list = leagues.lock().unwrap().clone();
    ui.horizontal(|ui| {
        ui.label("League");
        if list.is_empty() {
            // Fetch still in flight (or failed): free text keeps the field
            // editable instead of blocking on the network.
            ui.text_edit_singleline(&mut cfg.league);
        } else {
            egui::ComboBox::from_id_salt("league")
                .selected_text(cfg.league.clone())
                .show_ui(ui, |ui| {
                    for l in &list {
                        ui.selectable_value(&mut cfg.league, l.clone(), l);
                    }
                });
        }
    });
    // The running overlay follows a league change as a whole (see
    // `league`); the refresh interval is read once, at startup.
    ui.small("A league change takes effect in the running overlay. The refresh interval applies from the next start.");
    ui.horizontal(|ui| {
        ui.label("Refresh prices every");
        egui::ComboBox::from_id_salt("refresh")
            .selected_text(format!("{} min", cfg.refresh_minutes))
            .show_ui(ui, |ui| {
                for m in [5u64, 10, 15, 30, 60] {
                    ui.selectable_value(&mut cfg.refresh_minutes, m, format!("{m} min"));
                }
            });
    });
    ui.add_space(10.0);
    ui.checkbox(&mut cfg.price_gem_rows, "Price reward-panel gems through trade search (uses search budget)");
    ui.small(
        "Each gem row is one trade search, counted against the same limit as your own price checks. Currency \
         rows price through poe.ninja and ask the exchange only for what it lacks, whatever this says.",
    );
}

fn section_capture_ocr(ui: &mut egui::Ui) {
    ui.heading("Capture & OCR");
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(
            "The reward panel is detected automatically — no calibration. \
             Scanning runs only while reward rows are on screen.",
        )
        .weak(),
    );
}

fn section_macros(ui: &mut egui::Ui, cfg: &mut Config, capture: &mut Option<CaptureTarget>) {
    ui.heading("Macros & Shortcuts");
    ui.add_space(6.0);
    let conflicts = crate::bindings::resolve(cfg).conflicts;

    ui.label("Chat macros");
    let mut remove: Option<usize> = None;
    for i in 0..cfg.macros.len() {
        let key = cfg.macros[i].key.clone();
        ui.horizontal(|ui| {
            capture_button(ui, &key, CaptureTarget::Macro(i), capture);
            ui.add(
                egui::TextEdit::singleline(&mut cfg.macros[i].message)
                    .hint_text("message to send"),
            );
            if ui.button("✕").clicked() {
                remove = Some(i);
            }
        });
        conflict_label(ui, &conflicts, crate::bindings::Slot::Macro(i));
    }
    if let Some(i) = remove {
        cfg.macros.remove(i);
        // Any armed capture into this list may now point at the wrong row.
        if matches!(*capture, Some(CaptureTarget::Macro(_))) {
            *capture = None;
        }
    }
    if ui.button("+ Add macro").clicked() {
        cfg.macros.push(Macro {
            key: String::new(),
            message: String::new(),
        });
    }

    ui.add_space(12.0);
    ui.label("Resource shortcuts");
    let mut remove: Option<usize> = None;
    for i in 0..cfg.resource_shortcuts.len() {
        let key = cfg.resource_shortcuts[i].key.clone();
        ui.horizontal(|ui| {
            capture_button(ui, &key, CaptureTarget::Shortcut(i), capture);
            ui.add(
                egui::TextEdit::singleline(&mut cfg.resource_shortcuts[i].url)
                    .hint_text("https://poe2db.tw/us/search?q={name}"),
            );
            if ui.button("✕").clicked() {
                remove = Some(i);
            }
        });
        conflict_label(ui, &conflicts, crate::bindings::Slot::Shortcut(i));
    }
    if let Some(i) = remove {
        cfg.resource_shortcuts.remove(i);
        if matches!(*capture, Some(CaptureTarget::Shortcut(_))) {
            *capture = None;
        }
    }
    if ui.button("+ Add shortcut").clicked() {
        cfg.resource_shortcuts.push(ResourceShortcut {
            key: String::new(),
            url: String::new(),
        });
    }

    ui.add_space(12.0);
    ui.horizontal(|ui| {
        ui.label("Chat-open delay");
        ui.add(egui::Slider::new(&mut cfg.macro_open_delay_ms, 0..=2000).suffix(" ms"));
    });
}

fn section_waystones(
    ui: &mut egui::Ui,
    cfg: &mut Config,
    mods: &[String],
    suggest: &mut Option<(&'static str, usize)>,
    stash_selected: &mut BTreeSet<String>,
) {
    ui.heading("Waystones");
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(
            "Type to search the mod database — # stands for the rolled \
             number, so a picked mod matches every roll. Free text works \
             too and matches as a substring.",
        )
        .weak(),
    );
    ui.add_space(6.0);
    string_list(ui, "Danger mod needles", "danger", &mut cfg.map_danger_needles, mods, suggest);
    ui.add_space(12.0);
    string_list(ui, "Reward mod needles", "good", &mut cfg.map_good_needles, mods, suggest);
    ui.add_space(16.0);
    stash_regex_builder(ui, &cfg.map_good_needles, stash_selected);
}

/// Interactive builder for an in-game stash search regex: check reward
/// needles, preview the composed regex, copy it. Copying to the clipboard is
/// fine here because this runs in the settings process — the overlay process
/// must never write the clipboard (Ctrl+C item text is its input channel).
fn stash_regex_builder(
    ui: &mut egui::Ui,
    user_good: &[String],
    selected: &mut BTreeSet<String>,
) {
    ui.separator();
    ui.label("Stash search regex");
    ui.label(
        egui::RichText::new(
            "Pick reward mods to build a search string for the in-game \
             stash search, then paste it there to find matching waystones.",
        )
        .weak(),
    );
    ui.add_space(4.0);
    let options = stash_needle_options(user_good);
    for needle in &options {
        let mut on = selected.contains(needle);
        if ui.checkbox(&mut on, needle).changed() {
            if on {
                selected.insert(needle.clone());
            } else {
                selected.remove(needle);
            }
        }
    }
    // Compose from the option list, not the set: options carry a stable
    // display order, and needles whose config row was deleted (their
    // selection lingers in the set) silently drop out of the regex.
    let picked: Vec<String> =
        options.into_iter().filter(|n| selected.contains(n)).collect();
    let regex = khaloni_poe2_core::mapmods::regex_for_needles(&picked);
    ui.add_space(4.0);
    // Read-only preview: TextBuffer for &str rejects edits but still
    // allows select-all, so the text stays inspectable.
    ui.add(
        egui::TextEdit::singleline(&mut regex.as_str())
            .desired_width(f32::INFINITY)
            .hint_text("select mods above"),
    );
    ui.horizontal(|ui| {
        if ui
            .add_enabled(!regex.is_empty(), egui::Button::new("Copy"))
            .clicked()
        {
            ui.ctx().copy_text(regex.clone());
        }
        let count = regex.chars().count();
        if stash_regex_too_long(&regex) {
            ui.colored_label(
                egui::Color32::RED,
                format!("{count}/{STASH_SEARCH_LIMIT} — too long for the in-game search"),
            );
        } else if !regex.is_empty() {
            ui.weak(format!("{count}/{STASH_SEARCH_LIMIT}"));
        }
    });
}

/// The game's stash search field caps input at 50 characters; a longer
/// regex gets truncated and silently matches the wrong things.
pub const STASH_SEARCH_LIMIT: usize = 50;

/// Whether `regex` exceeds the in-game limit. Chars, not bytes: the game
/// counts what the user sees typed. Pure so it is unit-testable.
pub fn stash_regex_too_long(regex: &str) -> bool {
    regex.chars().count() > STASH_SEARCH_LIMIT
}

/// Checkbox options for the stash-regex builder: built-in reward needles
/// first (rule order), then the user's reward needles that aren't already
/// covered. Dedup is case-insensitive because built-ins are lowercase while
/// free-typed user needles may not be. Pure so it is unit-testable.
pub fn stash_needle_options(user_good: &[String]) -> Vec<String> {
    use khaloni_poe2_core::mapmods::{default_rules, ModKind};
    let mut out: Vec<String> = default_rules()
        .into_iter()
        .filter(|r| r.kind == ModKind::Good)
        .map(|r| r.needle)
        .collect();
    for n in user_good {
        let t = n.trim();
        // Skip empties: the "+ Add" button creates blank rows while typing.
        if !t.is_empty() && !out.iter().any(|o| o.eq_ignore_ascii_case(t)) {
            out.push(t.to_string());
        }
    }
    out
}

/// Editable needle list with autocomplete from the mod database. Suggestion
/// visibility is keyed off `suggest`, not widget focus: a click on a
/// suggestion button takes focus from the text field, and focus-gated
/// suggestions would vanish one frame before the click could land.
fn string_list(
    ui: &mut egui::Ui,
    label: &str,
    id: &'static str,
    items: &mut Vec<String>,
    mods: &[String],
    suggest: &mut Option<(&'static str, usize)>,
) {
    ui.label(label);
    let mut remove: Option<usize> = None;
    for (i, s) in items.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            let resp = ui.add(egui::TextEdit::singleline(s).id_salt((id, i)));
            if resp.gained_focus() || resp.changed() {
                *suggest = Some((id, i));
            }
            if ui.button("✕").clicked() {
                remove = Some(i);
                *suggest = None;
            }
        });
        if *suggest == Some((id, i)) && s.trim().len() >= 3 {
            if mods.is_empty() {
                ui.label(egui::RichText::new("  loading mod database…").weak().small());
            }
            let mut picked: Option<String> = None;
            ui.indent((id, i, "sugg"), |ui| {
                for m in mod_suggestions(mods, s, 8) {
                    if ui.small_button(m).clicked() {
                        picked = Some(m.clone());
                    }
                }
            });
            if let Some(p) = picked {
                *s = p;
                *suggest = None;
            }
            if ui.input(|inp| inp.key_pressed(egui::Key::Escape)) {
                *suggest = None;
            }
        }
    }
    if let Some(i) = remove {
        items.remove(i);
    }
    if ui.button(format!("+ Add {}", label.to_lowercase())).clicked() {
        items.push(String::new());
    }
}

/// Top `limit` mod texts matching `query`: every whitespace-separated query
/// token must occur (case-insensitive), shorter texts rank first so the
/// tightest mod surfaces on top. Pure, so it is unit-testable.
pub fn mod_suggestions<'a>(mods: &'a [String], query: &str, limit: usize) -> Vec<&'a String> {
    let q = query.to_lowercase();
    let tokens: Vec<&str> = q.split_whitespace().collect();
    if tokens.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<&String> = mods
        .iter()
        .filter(|m| tokens.iter().all(|t| m.contains(t)))
        .collect();
    hits.sort_by_key(|m| m.len());
    hits.truncate(limit);
    hits
}

/// Wall-clock HH:MM:SS for the "Saved …" heartbeat. chrono is deliberately
/// not a dependency, and std::time has no timezone, so this is UTC.
fn hms_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let s = secs % 86400;
    format!("{:02}:{:02}:{:02} UTC", s / 3600, (s % 3600) / 60, s % 60)
}
