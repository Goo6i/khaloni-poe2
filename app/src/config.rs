use std::{fs, path::PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
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
