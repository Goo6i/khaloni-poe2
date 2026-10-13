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
            .with_decorations(false)
            .with_inner_size([900.0, 680.0])
            .with_min_inner_size([720.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native(
        "khaloni-poe2 settings",
        options,
        Box::new(move |cc| {
            // Fonts take effect at the next pass, so they go in before the first frame.
            crate::settings_style::install(&cc.egui_ctx);
            Ok(Box::new(SettingsApp::new(cfg)))
        }),
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

const SECTION_DESCRIPTIONS: [&str; 11] = [
    "The keys that open the price check, the panels and the planner",
    "What the overlay shows and how it looks over the game",
    "Where prices come from and how values are tiered",
    "The market panel's tables and your own runs",
    "Crafting profiles and the planner's rules",
    "How the screen is read",
    "Keys that send a chat command or a stash search",
    "Starting the overlay with the game",
    "Waystone modifiers to mark",
    "Your pathofexile.com account",
    "Updates and the version in use",
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
        crate::settings_style::install(&ctx);
        self.handle_capture(&ctx);

        let bar = crate::settings_style::title_bar(ui, "Settings");
        if bar.close_clicked {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        let status = if let Some(err) = &self.save_err {
            Some(format!("save failed: {err}"))
        } else {
            self.saved_at.as_ref().map(|at| format!("Saved {at}"))
        };
        egui::Panel::left("nav")
            .resizable(false)
            .default_size(crate::settings_style::SIDEBAR_W)
            .frame(
                egui::Frame::NONE
                    .fill(crate::settings_style::color32(crate::theme::BG_SIDEBAR))
                    .inner_margin(egui::Margin { left: 12, right: 8, top: 16, bottom: 16 }),
            )
            .show(ui, |ui| {
                for (section, label) in SECTIONS {
                    if crate::settings_style::sidebar_item(ui, label, self.section == section).clicked() {
                        self.section = section;
                    }
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    ui.vertical(|ui| {
                        crate::settings_style::sidebar_footer(ui, "khaloni-poe2", env!("CARGO_PKG_VERSION"), status.as_deref());
                    });
                });
            });
        let (section_i, section_label) = SECTIONS
            .iter()
            .position(|(s, _)| *s == self.section)
            .map(|i| (i, SECTIONS[i].1))
            .unwrap_or((0, SECTIONS[0].1));

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

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(crate::settings_style::color32(crate::theme::BG_PANEL))
                    .inner_margin(egui::Margin { left: 48, right: 48, top: 8, bottom: 24 }),
            )
            .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    crate::settings_style::section_heading(ui, section_label, SECTION_DESCRIPTIONS[section_i]);
                    match section {
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
                    }
                });
        });

        crate::settings_style::frame(&ctx);

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

fn section_hotkeys(ui: &mut egui::Ui, cfg: &mut Config, capture: &mut Option<CaptureTarget>) {
    use crate::settings_style as st;
    // The same resolution the overlay binds from (`triggers::dedupe` under
    // it), so what is marked here is exactly what will be left unbound.
    let conflicts = crate::bindings::resolve(cfg).conflicts;
    let rows: [(&str, &str, String, CaptureTarget); 5] = [
        ("Price check", "Prices the item under the cursor", cfg.hotkey_price_check.clone(), CaptureTarget::PriceCheck),
        ("Settings panel", "Opens this window", cfg.hotkey_settings.clone(), CaptureTarget::Settings),
        ("Market panel", "The market tables over the game", cfg.hotkey_market.clone(), CaptureTarget::Market),
        ("Upgrade check", "Searches for a better version of the item", cfg.hotkey_upgrade.clone(), CaptureTarget::Upgrade),
        ("Craft planner", "The planner for the item under the cursor", cfg.hotkey_craft.clone(), CaptureTarget::Craft),
    ];
    st::card(ui, "hotkeys", |ui| {
        let n = rows.len();
        for (i, (label, desc, key, target)) in rows.iter().enumerate() {
            let body = |ui: &mut egui::Ui| {
                if st::recorder(ui, key, *capture == Some(*target)).clicked() {
                    *capture = Some(*target);
                }
                if let Some(c) = crate::bindings::conflict_for(&conflicts, slot_of(*target)) {
                    st::warning(ui, &format!("off: {}", c.message));
                }
            };
            if i + 1 == n {
                st::row_last(ui, label, Some(desc), body)
            } else {
                st::row(ui, label, Some(desc), body)
            }
        }
    });
    st::note(ui, "a running overlay picks hotkey changes up within a second (KDE shows one approval dialog)");
    ui.add_space(8.0);
    st::card(ui, "copy", |ui| {
        st::row_last(
            ui,
            "Copy items with advanced descriptions (Ctrl+Alt+C)",
            Some(
                "Off sends plain Ctrl+C, which copies what the game's \"advanced mod descriptions\" option says: with \
                 that option off, the copy has no readable modifiers and the item is not priced.",
            ),
            |ui| {
                st::switch(ui, &mut cfg.advanced_copy);
            },
        );
    });
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

fn section_display(ui: &mut egui::Ui, cfg: &mut Config, tier_ok: bool) {
    use crate::settings_style as st;
    st::card(ui, "overlay", |ui| {
        st::row(ui, "Hide overlay when the game is minimized or covered", None, |ui| {
            st::switch(ui, &mut cfg.pause_when_hidden);
        });
        // In-game overlay only — this window is unaffected by design.
        // Floor at 10%: a fully invisible overlay reads as broken (the
        // hotkeys still work with nothing on screen to show for it).
        st::row(ui, "Overlay opacity", Some("In-game overlay only; this window is unaffected"), |ui| {
            let mut pct = (cfg.overlay_opacity * 100.0).round().clamp(10.0, 100.0);
            if st::stepper(ui, &mut pct, 5.0, 10.0..=100.0, "%") {
                cfg.overlay_opacity = pct / 100.0;
            }
        });
        // The stored number has always been compared against the value
        // in divines (`value::pick_unit`); only this label called it ex.
        st::row_last(ui, "Show a value in divines from", Some("Below this, prices read in exalted"), |ui| {
            st::stepper(ui, &mut cfg.divine_threshold, 0.5, 0.0..=10000.0, " div");
        });
    });
    st::group_heading(ui, "Value tiers");
    st::note(ui, "In chaos: the exalted orb's worth moves too much to set a bar by");
    st::card(ui, "tiers", |ui| {
        st::row(ui, "Decent from", None, |ui| {
            st::stepper(ui, &mut cfg.tier_decent_chaos, 1.0, 0.0..=10000.0, " chaos");
        });
        st::row_last(ui, "Jackpot from", None, |ui| {
            st::stepper(ui, &mut cfg.tier_good_chaos, 1.0, 0.0..=10000.0, " chaos");
        });
    });
    tier_bar(ui, cfg, tier_ok);
    if !tier_ok {
        st::warning(ui, "decent must be ≤ jackpot");
    }
}

/// Three zones on a log10 scale over 0.1..1000 chaos, so the junk/decent split
/// stays visible even though jackpot thresholds run two orders higher.
fn tier_bar(ui: &mut egui::Ui, cfg: &Config, tier_ok: bool) {
    use crate::settings_style::color32;
    let width = ui.available_width().min(420.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 24.0), egui::Sense::hover());
    let painter = ui.painter();
    if !tier_ok {
        painter.rect_filled(rect, 3, color32(crate::theme::TEXT_WARNING));
        return;
    }
    let frac = |v: f64| ((v.clamp(0.1, 1000.0).log10() + 1.0) / 4.0) as f32;
    let x1 = rect.left() + rect.width() * frac(cfg.tier_decent_chaos);
    let x2 = rect.left() + rect.width() * frac(cfg.tier_good_chaos);
    let zone = |a: f32, b: f32| {
        egui::Rect::from_min_max(egui::pos2(a, rect.top()), egui::pos2(b, rect.bottom()))
    };
    painter.rect_filled(zone(rect.left(), x1), 0, color32(crate::theme::TEXT_MUTED));
    painter.rect_filled(zone(x1, x2), 0, color32(crate::theme::RARITY_MAGIC));
    painter.rect_filled(zone(x2, rect.right()), 0, color32(crate::theme::GOLD));
}

fn section_pricing(ui: &mut egui::Ui, cfg: &mut Config, leagues: &Arc<Mutex<Vec<String>>>) {
    use crate::settings_style as st;
    let list = leagues.lock().unwrap().clone();
    st::card(ui, "pricing", |ui| {
        st::row(ui, "League", None, |ui| {
            if list.is_empty() {
                // Fetch still in flight (or failed): free text keeps the field
                // editable instead of blocking on the network.
                st::text_field(ui, &mut cfg.league);
            } else if list.len() <= 4 {
                // The index is the value; no league picked lights nothing.
                let mut picked = list.iter().position(|l| *l == cfg.league).unwrap_or(usize::MAX);
                let options: Vec<(usize, &str)> = list.iter().map(String::as_str).enumerate().collect();
                if st::segmented(ui, "league", &options, &mut picked) {
                    cfg.league = list[picked].clone();
                }
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
        st::row_last(ui, "Refresh prices every", None, |ui| {
            egui::ComboBox::from_id_salt("refresh")
                .selected_text(format!("{} min", cfg.refresh_minutes))
                .show_ui(ui, |ui| {
                    for m in [5u64, 10, 15, 30, 60] {
                        ui.selectable_value(&mut cfg.refresh_minutes, m, format!("{m} min"));
                    }
                });
        });
    });
    // The running overlay follows a league change as a whole (see
    // `league`); the refresh interval is read once, at startup.
    st::note(ui, "A league change takes effect in the running overlay. The refresh interval applies from the next start.");
    ui.add_space(8.0);
    st::card(ui, "gems", |ui| {
        st::row_last(
            ui,
            "Price reward-panel gems through trade search (uses search budget)",
            Some(
                "Each gem row is one trade search, counted against the same limit as your own price checks. Currency \
                 rows price through poe.ninja and ask the exchange only for what it lacks, whatever this says.",
            ),
            |ui| {
                st::switch(ui, &mut cfg.price_gem_rows);
            },
        );
    });
}

fn section_capture_ocr(ui: &mut egui::Ui) {
    use crate::settings_style as st;
    st::card(ui, "capture", |ui| {
        st::row_last(
            ui,
            "The reward panel is detected automatically — no calibration. \
             Scanning runs only while reward rows are on screen.",
            None,
            |_| {},
        );
    });
}

fn section_macros(ui: &mut egui::Ui, cfg: &mut Config, capture: &mut Option<CaptureTarget>) {
    use crate::settings_style as st;
    let conflicts = crate::bindings::resolve(cfg).conflicts;

    st::group_heading(ui, "Chat macros");
    let mut remove: Option<usize> = None;
    for i in 0..cfg.macros.len() {
        let key = cfg.macros[i].key.clone();
        st::card(ui, &format!("macro{i}"), |ui| {
            st::row(ui, "Key", None, |ui| {
                if st::recorder(ui, &key, *capture == Some(CaptureTarget::Macro(i))).clicked() {
                    *capture = Some(CaptureTarget::Macro(i));
                }
                if let Some(c) = crate::bindings::conflict_for(&conflicts, crate::bindings::Slot::Macro(i)) {
                    st::warning(ui, &format!("off: {}", c.message));
                }
            });
            st::row_last(ui, "Message", None, |ui| {
                if st::button(ui, "Remove").clicked() {
                    remove = Some(i);
                }
                ui.add(
                    egui::TextEdit::singleline(&mut cfg.macros[i].message)
                        .desired_width(220.0)
                        .hint_text("message to send"),
                );
            });
        });
    }
    if let Some(i) = remove {
        cfg.macros.remove(i);
        // Any armed capture into this list may now point at the wrong row.
        if matches!(*capture, Some(CaptureTarget::Macro(_))) {
            *capture = None;
        }
    }
    if st::plate_button(ui, "Add macro").clicked() {
        cfg.macros.push(Macro {
            key: String::new(),
            message: String::new(),
        });
    }

    ui.add_space(12.0);
    st::group_heading(ui, "Resource shortcuts");
    let mut remove: Option<usize> = None;
    for i in 0..cfg.resource_shortcuts.len() {
        let key = cfg.resource_shortcuts[i].key.clone();
        st::card(ui, &format!("shortcut{i}"), |ui| {
            st::row(ui, "Key", None, |ui| {
                if st::recorder(ui, &key, *capture == Some(CaptureTarget::Shortcut(i))).clicked() {
                    *capture = Some(CaptureTarget::Shortcut(i));
                }
                if let Some(c) = crate::bindings::conflict_for(&conflicts, crate::bindings::Slot::Shortcut(i)) {
                    st::warning(ui, &format!("off: {}", c.message));
                }
            });
            st::row_last(ui, "Address", None, |ui| {
                if st::button(ui, "Remove").clicked() {
                    remove = Some(i);
                }
                ui.add(
                    egui::TextEdit::singleline(&mut cfg.resource_shortcuts[i].url)
                        .desired_width(220.0)
                        .hint_text("https://poe2db.tw/us/search?q={name}"),
                );
            });
        });
    }
    if let Some(i) = remove {
        cfg.resource_shortcuts.remove(i);
        if matches!(*capture, Some(CaptureTarget::Shortcut(_))) {
            *capture = None;
        }
    }
    if st::plate_button(ui, "Add shortcut").clicked() {
        cfg.resource_shortcuts.push(ResourceShortcut {
            key: String::new(),
            url: String::new(),
        });
    }

    ui.add_space(12.0);
    st::card(ui, "delay", |ui| {
        st::row_last(ui, "Chat-open delay", None, |ui| {
            let mut ms = cfg.macro_open_delay_ms as f64;
            if st::stepper(ui, &mut ms, 50.0, 0.0..=2000.0, " ms") {
                cfg.macro_open_delay_ms = ms as u64;
            }
        });
    });
}

fn section_waystones(
    ui: &mut egui::Ui,
    cfg: &mut Config,
    mods: &[String],
    suggest: &mut Option<(&'static str, usize)>,
    stash_selected: &mut BTreeSet<String>,
) {
    use crate::settings_style as st;
    st::note(
        ui,
        "Type to search the mod database — # stands for the rolled \
         number, so a picked mod matches every roll. Free text works \
         too and matches as a substring.",
    );
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
    use crate::settings_style as st;
    st::group_heading(ui, "Stash search regex");
    st::note(
        ui,
        "Pick reward mods to build a search string for the in-game \
         stash search, then paste it there to find matching waystones.",
    );
    let options = stash_needle_options(user_good);
    st::card(ui, "stash", |ui| {
        let n = options.len();
        for (i, needle) in options.iter().enumerate() {
            let mut on = selected.contains(needle);
            let body = |ui: &mut egui::Ui| {
                if st::switch(ui, &mut on).changed() {
                    if on {
                        selected.insert(needle.clone());
                    } else {
                        selected.remove(needle);
                    }
                }
            };
            ui.push_id(i, |ui| {
                if i + 1 == n {
                    st::row_last(ui, needle, None, body)
                } else {
                    st::row(ui, needle, None, body)
                }
            });
        }
    });
    // Compose from the option list, not the set: options carry a stable
    // display order, and needles whose config row was deleted (their
    // selection lingers in the set) silently drop out of the regex.
    let picked: Vec<String> =
        options.into_iter().filter(|n| selected.contains(n)).collect();
    let regex = khaloni_poe2_core::mapmods::regex_for_needles(&picked);
    // Read-only preview: TextBuffer for &str rejects edits but still
    // allows select-all, so the text stays inspectable.
    ui.add(
        egui::TextEdit::singleline(&mut regex.as_str())
            .desired_width(f32::INFINITY)
            .hint_text("select mods above"),
    );
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_enabled_ui(!regex.is_empty(), |ui| {
            if st::button(ui, "Copy").clicked() {
                ui.ctx().copy_text(regex.clone());
            }
        });
        let count = regex.chars().count();
        if stash_regex_too_long(&regex) {
            st::warning(ui, &format!("{count}/{STASH_SEARCH_LIMIT} — too long for the in-game search"));
        } else if !regex.is_empty() {
            st::note(ui, &format!("{count}/{STASH_SEARCH_LIMIT}"));
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
        // Skip empties: Add trims its input and rejects a blank one, but a
        // config edited by hand can still hold one.
        if !t.is_empty() && !out.iter().any(|o| o.eq_ignore_ascii_case(t)) {
            out.push(t.to_string());
        }
    }
    out
}

/// Needle list with autocomplete from the mod database: the marked mods in
/// one card, each an editable field with Remove, and a field to add another. Suggestion visibility is keyed
/// off `suggest`, not widget focus: a click on a suggestion button takes
/// focus from the text field, and focus-gated suggestions would vanish one
/// frame before the click could land.
fn string_list(
    ui: &mut egui::Ui,
    label: &str,
    id: &'static str,
    items: &mut Vec<String>,
    mods: &[String],
    suggest: &mut Option<(&'static str, usize)>,
) {
    use crate::settings_style as st;
    st::group_heading(ui, label);
    if !items.is_empty() {
        let mut remove: Option<usize> = None;
        st::card(ui, id, |ui| {
            let n = items.len();
            for (i, s) in items.iter_mut().enumerate() {
                // The field is the row: it carries the needle text, a blank
                // needle shows as an empty field, and Remove sits at the edge.
                let body = |ui: &mut egui::Ui| {
                    if st::button(ui, "Remove").clicked() {
                        remove = Some(i);
                    }
                    let w = (ui.available_width() - 8.0).max(120.0);
                    ui.add(egui::TextEdit::singleline(s).desired_width(w));
                };
                ui.push_id(i, |ui| {
                    if i + 1 == n {
                        st::row_last(ui, "", None, body)
                    } else {
                        st::row(ui, "", None, body)
                    }
                });
            }
        });
        if let Some(i) = remove {
            items.remove(i);
            *suggest = None;
        }
    }
    // The add field's suggestions live under a slot past every item.
    let slot = (id, usize::MAX);
    let buf_id = ui.id().with((id, "add"));
    let mut text: String = ui.data_mut(|d| d.get_temp(buf_id)).unwrap_or_default();
    let mut add = false;
    st::card(ui, &format!("{id}-add"), |ui| {
        st::row_last(ui, &format!("Add {}", label.to_lowercase()), None, |ui| {
            add = st::plate_button(ui, "Add").clicked();
            let resp = st::text_field(ui, &mut text);
            if resp.gained_focus() || resp.changed() {
                *suggest = Some(slot);
            }
            add |= resp.lost_focus() && ui.input(|inp| inp.key_pressed(egui::Key::Enter));
        });
    });
    if *suggest == Some(slot) && text.trim().len() >= 3 {
        if mods.is_empty() {
            st::note(ui, "loading mod database…");
        }
        let mut picked: Option<String> = None;
        for m in mod_suggestions(mods, &text, 8) {
            if st::button(ui, m).clicked() {
                picked = Some(m.clone());
            }
        }
        if let Some(p) = picked {
            text = p;
            *suggest = None;
        }
        if ui.input(|inp| inp.key_pressed(egui::Key::Escape)) {
            *suggest = None;
        }
    }
    if add && !text.trim().is_empty() {
        items.push(text.trim().to_string());
        text.clear();
        *suggest = None;
    }
    ui.data_mut(|d| d.insert_temp(buf_id, text));
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
