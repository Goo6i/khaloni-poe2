use std::{fs, path::PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

/// A chat macro: pressing `key` opens chat, types `message`, and sends it.
/// `key` is a portal GlobalShortcut trigger string (e.g. "CTRL+1").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Macro {
    pub key: String,
    pub message: String,
}

/// An external-resource shortcut: pressing `key` copies the hovered item and
/// opens `url` with `{name}` replaced by the item name (URL-encoded), e.g.
/// "https://poe2db.tw/us/search?q={name}" or a wiki/scout URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceShortcut {
    pub key: String,
    pub url: String,
}

fn default_true() -> bool {
    true
}
fn default_refresh_minutes() -> u64 {
    10
}

fn default_divine_threshold() -> f64 {
    1.0
}
fn default_font() -> String {
    "/usr/share/fonts/TTF/DejaVuSans.ttf".into()
}
fn default_tesseract() -> String {
    "tesseract".into()
}
fn default_tier_decent() -> f64 {
    1.0
}
fn default_tier_good() -> f64 {
    10.0
}
fn default_panel_open_brightness() -> u8 {
    100
}
fn default_panel_close_brightness() -> u8 {
    80
}
fn default_hotkey_price_check() -> String {
    "F7".into()
}
fn default_hotkey_overlay() -> String {
    "F8".into()
}
fn default_hotkey_settings() -> String {
    "F12".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub league: String,
    /// Screencast portal restore token; grants silent capture on later runs.
    #[serde(default)]
    pub restore_token: Option<String>,
    /// Calibrated list region in GLOBAL logical coordinates (slurp's space).
    #[serde(default)]
    pub calibration: Option<Rect>,
    #[serde(default = "default_divine_threshold")]
    pub divine_threshold: f64,
    /// Price table refresh interval; while data is stale a 60s retry
    /// takes over until a fetch succeeds.
    #[serde(default = "default_refresh_minutes")]
    pub refresh_minutes: u64,
    /// Portal GlobalShortcuts preferred triggers. KDE shows one approval
    /// dialog whenever the binding set changes.
    #[serde(default = "default_hotkey_price_check")]
    pub hotkey_price_check: String,
    #[serde(default = "default_hotkey_overlay")]
    pub hotkey_overlay: String,
    /// Hotkey to open the in-overlay settings panel. Changing it triggers one
    /// KDE re-approval on next launch.
    #[serde(default = "default_hotkey_settings")]
    pub hotkey_settings: String,
    #[serde(default = "default_true")]
    pub pause_when_unfocused: bool,
    #[serde(default = "default_font")]
    pub font_path: String,
    #[serde(default = "default_tesseract")]
    pub tesseract_cmd: String,
    /// Value tier thresholds in exalts: below decent = junk, above good = jackpot.
    #[serde(default = "default_tier_decent")]
    pub tier_decent_ex: f64,
    #[serde(default = "default_tier_good")]
    pub tier_good_ex: f64,
    /// Mean grayscale brightness (0-255) a captured frame must exceed, for
    /// 2 consecutive frames, before the brightness gate opens and tesseract
    /// starts running (see `brightness::BrightnessGate`). The panel
    /// parchment measures ~168, the bare game world ~40.
    #[serde(default = "default_panel_open_brightness")]
    pub panel_open_brightness: u8,
    /// Mean grayscale brightness a captured frame must fall below, for 3
    /// consecutive frames, before the brightness gate closes again and
    /// tesseract is skipped in favor of `stabilize::ScanResult::GateEmpty`.
    #[serde(default = "default_panel_close_brightness")]
    pub panel_close_brightness: u8,
    /// Chat macros, each bound to its own global shortcut. Empty by default
    /// (feature off). Changing this set triggers one KDE re-approval dialog.
    #[serde(default)]
    pub macros: Vec<Macro>,
    /// External-resource shortcuts (open hovered item on wiki/poedb/scout).
    /// Empty by default. Changing this set triggers one KDE re-approval.
    #[serde(default)]
    pub resource_shortcuts: Vec<ResourceShortcut>,
    /// Hotkey to analyze the hovered waystone: copies a stash-search regex of
    /// its reward mods and notifies which mods are dangerous. None = off.
    #[serde(default)]
    pub map_hotkey: Option<String>,
    /// Extra danger/reward mod needles (lowercase substrings) merged with the
    /// built-in map-mod rules, so the classifier is tunable without a rebuild.
    #[serde(default)]
    pub map_danger_needles: Vec<String>,
    #[serde(default)]
    pub map_good_needles: Vec<String>,
    /// Milliseconds to wait after opening chat (Enter) before a macro starts
    /// typing, so the chat box is ready. Raise if the first characters drop.
    #[serde(default = "default_macro_open_delay_ms")]
    pub macro_open_delay_ms: u64,
}

fn default_macro_open_delay_ms() -> u64 {
    400
}

impl Default for Config {
    fn default() -> Self {
        toml::from_str("league = \"Runes of Aldur\"").expect("defaults parse")
    }
}

impl Config {
    pub fn path() -> PathBuf {
        directories::ProjectDirs::from("", "", "poe2-lens")
            .expect("home dir resolvable")
            .config_dir()
            .join("config.toml")
    }

    pub fn load() -> anyhow::Result<Config> {
        let p = Self::path();
        match fs::read_to_string(&p) {
            Ok(text) => Ok(toml::from_str(&text)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let p = Self::path();
        if let Some(dir) = p.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(&p, toml::to_string_pretty(self)?)?;
        Ok(())
    }
}
