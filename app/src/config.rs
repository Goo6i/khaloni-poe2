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
fn default_panel_min_brightness() -> u8 {
    110
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
    /// Minimum mean grayscale brightness (0-255) of a captured frame for it
    /// to be treated as the price panel; below this, tesseract is skipped.
    /// The panel parchment measures ~168, the bare game world ~40.
    #[serde(default = "default_panel_min_brightness")]
    pub panel_min_brightness: u8,
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
