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

/// A saved trade live-search: a pasted trade-site search URL, polled in the
/// background; new listings raise an overlay alert.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveSearch {
    pub name: String,
    pub url: String,
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
fn default_tier_decent() -> f64 {
    1.0
}
fn default_tier_good() -> f64 {
    10.0
}
fn default_hotkey_price_check() -> String {
    "F7".into()
}
fn default_hotkey_settings() -> String {
    "F12".into()
}
fn default_overlay_opacity() -> f64 {
    1.0
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Config {
    pub league: String,
    /// Where the screencast restore token used to live. It is read so an
    /// old file's token can move to its own file (see [`RestoreToken`]) and
    /// never written back: the overlay saving a token and the settings
    /// window saving everything else were two writers of one file, and each
    /// undid the other's change.
    #[serde(default, skip_serializing)]
    pub restore_token: Option<String>,
    #[serde(default = "default_divine_threshold")]
    pub divine_threshold: f64,
    /// Price table refresh interval in minutes, at least
    /// [`MIN_REFRESH_MINUTES`]; while data is stale a backed-off retry
    /// takes over until a fetch succeeds.
    #[serde(default = "default_refresh_minutes")]
    pub refresh_minutes: u64,
    /// Portal GlobalShortcuts preferred triggers. KDE shows one approval
    /// dialog whenever the binding set changes.
    #[serde(default = "default_hotkey_price_check")]
    pub hotkey_price_check: String,
    /// The overlay on/off key, from before the switch was removed: the
    /// overlay follows the game's focus and visibility on its own. Read so
    /// an old file loads without a word, bound to nothing, and never
    /// written back.
    #[serde(default, skip_serializing)]
    pub hotkey_overlay: String,
    /// Hotkey to open the in-overlay settings panel. Changing it triggers one
    /// KDE re-approval on next launch.
    #[serde(default = "default_hotkey_settings")]
    pub hotkey_settings: String,
    /// The reference panel's key, from before the panel was removed. Read
    /// so an old file loads without a word, bound to nothing, and never
    /// written back.
    #[serde(default, skip_serializing)]
    pub hotkey_reference: String,
    /// The leveling guide's key, from before the guide was removed. Read
    /// so an old file loads without a word, bound to nothing, and never
    /// written back.
    #[serde(default, skip_serializing)]
    pub hotkey_leveling: String,
    /// Hide the overlay and pause scanning while the game is not actually
    /// on screen (minimized or covered by other windows). Losing focus to
    /// another window always pauses scanning and hides the rows (see
    /// `scanpolicy`); this adds the covered/minimized case. The old key
    /// name is accepted so pre-rename configs load unchanged.
    #[serde(default = "default_true", alias = "pause_when_unfocused")]
    pub pause_when_hidden: bool,
    /// Overlay opacity, floored at 0.1 (nearly transparent) up to 1.0
    /// (opaque) — never fully invisible, because hotkeys keep working and an
    /// unseeable overlay reads as broken. In-game surface only, never the
    /// settings window.
    #[serde(default = "default_overlay_opacity")]
    pub overlay_opacity: f64,
    /// Value tier thresholds in chaos: below decent = junk, above good =
    /// jackpot. They were `tier_decent_ex` / `tier_good_ex` while the
    /// exalted orb was the stable unit; see [`Config::from_toml`] for what
    /// happens to a file that still carries those.
    #[serde(default = "default_tier_decent")]
    pub tier_decent_chaos: f64,
    #[serde(default = "default_tier_good")]
    pub tier_good_chaos: f64,
    /// Chat macros, each bound to its own global shortcut. Empty by default
    /// (feature off). Changing this set triggers one KDE re-approval dialog.
    #[serde(default)]
    pub macros: Vec<Macro>,
    /// External-resource shortcuts (open hovered item on wiki/poedb/scout).
    /// Empty by default. Changing this set triggers one KDE re-approval.
    #[serde(default)]
    pub resource_shortcuts: Vec<ResourceShortcut>,
    /// Extra danger/reward mod needles (lowercase substrings) merged with the
    /// built-in map-mod rules, so the classifier is tunable without a rebuild.
    #[serde(default)]
    pub map_danger_needles: Vec<String>,
    #[serde(default)]
    pub map_good_needles: Vec<String>,
    /// Check GitHub releases for a newer version at startup. The check is
    /// report-only; installing is always an explicit action.
    #[serde(default = "default_true")]
    pub check_updates: bool,
    /// Path of Exile 2's Client.txt; None resolves the default Steam
    /// location per OS. Feeds the run tracker behind the market panel's
    /// "My runs" tab.
    #[serde(default)]
    pub client_log_path: Option<String>,
    /// POESESSID session cookie, needed only for the account features
    /// (live-search alerts, wealth tracker). Stored locally, sent only to
    /// pathofexile.com. Empty = those features off.
    #[serde(default)]
    pub poesessid: String,
    /// Account name for the stash/wealth endpoints. Empty = wealth off.
    #[serde(default)]
    pub account_name: String,
    /// Saved trade searches to poll for new listings.
    #[serde(default)]
    pub live_searches: Vec<LiveSearch>,
    /// Hotkey for the gear-upgrade check on the hovered item; empty = off.
    #[serde(default)]
    pub hotkey_upgrade: String,
    /// Milliseconds to wait after opening chat (Enter) before a macro starts
    /// typing, so the chat box is ready. Raise if the first characters drop.
    #[serde(default = "default_macro_open_delay_ms")]
    pub macro_open_delay_ms: u64,
    /// Copy the hovered item with Ctrl+Alt+C, which yields the advanced mod
    /// descriptions whatever the game's own setting is. False sends plain
    /// Ctrl+C, which follows that setting: with it off the copy carries no
    /// readable modifiers and the item cannot be priced.
    #[serde(default = "default_true")]
    pub advanced_copy: bool,
    /// Hotkey toggling the in-overlay market panel. Unbound by default:
    /// the F-keys near the others are taken.
    #[serde(default)]
    pub hotkey_market: String,
    /// The market view's liquidity floors (see `core::market::Floors`):
    /// traded volume from which an exchange item ranks, in the volume
    /// field's own unit, and the listing counts from which a listed item
    /// ranks and under which it is not trusted at all.
    #[serde(default = "default_market_volume_floor")]
    pub market_volume_floor_div: f64,
    #[serde(default = "default_market_listings_rank")]
    pub market_listings_rank: u32,
    #[serde(default = "default_market_listings_min")]
    pub market_listings_min: u32,
    /// Price reward-panel gems through a trade search. Off by default: a
    /// panel of gem rewards is a dozen searches nobody asked for, counted
    /// against the same limit as the user's own checks. Currency rows are
    /// unaffected: they price through poe.ninja and ask the exchange only
    /// for what it lacks.
    #[serde(default)]
    pub price_gem_rows: bool,
    /// Hotkey opening the craft planner on the hovered item. Unbound by
    /// default, like the market panel's.
    #[serde(default)]
    pub hotkey_craft: String,
    /// Simulated runs per strategy when the planner costs a plan. More runs
    /// steady the figures and take longer: at the default a slow strategy
    /// takes several seconds.
    #[serde(default = "default_craft_runs")]
    pub craft_runs: usize,
    /// A simulated run gives up after this many times the median number of
    /// currency uses of the runs that reached the target so far.
    #[serde(default = "default_craft_give_up_times")]
    pub craft_give_up_times: f64,
    /// Listings of an item class needed before the observed model is used
    /// for it.
    #[serde(default = "default_craft_observed_min")]
    pub craft_observed_min: u32,
    /// What loading changed on the user's behalf (a migrated setting), for
    /// the overlay to say on screen once. Never stored.
    #[serde(skip)]
    pub notices: Vec<String>,
}

fn default_macro_open_delay_ms() -> u64 {
    400
}
fn default_craft_runs() -> usize {
    khaloni_poe2_core::craft::sim::DEFAULT_RUNS
}
fn default_craft_give_up_times() -> f64 {
    khaloni_poe2_core::craft::sim::DEFAULT_MEDIAN_TIMES
}
fn default_craft_observed_min() -> u32 {
    khaloni_poe2_core::craft::observed::MIN_LISTINGS
}

/// The planner's run count, give-up multiple and observed minimum stay
/// within these: fewer runs give figures that move from one plan to the
/// next, more make a plan take minutes, a give-up multiple under two cuts
/// off ordinary unlucky runs, and a sample under fifty listings says little
/// about a class's tiers.
pub const CRAFT_RUNS_RANGE: (usize, usize) = (500, 200_000);
pub const CRAFT_GIVE_UP_RANGE: (f64, f64) = (2.0, 100.0);
pub const CRAFT_OBSERVED_MIN_RANGE: (u32, u32) = (50, 20_000);
fn default_market_volume_floor() -> f64 {
    khaloni_poe2_core::market::Floors::default().volume_div
}
fn default_market_listings_rank() -> u32 {
    khaloni_poe2_core::market::Floors::default().listings_rank
}
fn default_market_listings_min() -> u32 {
    khaloni_poe2_core::market::Floors::default().listings_min
}

/// Shortest price refresh interval a config may ask for. A hand-edited 0
/// made the refresh loop sweep the price API without pause.
pub const MIN_REFRESH_MINUTES: u64 = 5;

/// What stands in for a secret in `Debug` output.
const REDACTED: &str = "<redacted>";

/// `Debug` with the secrets left out: the session cookie signs requests as
/// the account, and the restore token grants screen capture. A `{cfg:?}` in
/// a log line or a panic message must not carry either into a file the
/// user may paste into a bug report. Whether one is set still shows.
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Config {
            league,
            restore_token,
            divine_threshold,
            refresh_minutes,
            hotkey_price_check,
            hotkey_overlay: _,
            hotkey_settings,
            hotkey_reference: _,
            hotkey_leveling: _,
            pause_when_hidden,
            overlay_opacity,
            tier_decent_chaos,
            tier_good_chaos,
            macros,
            resource_shortcuts,
            map_danger_needles,
            map_good_needles,
            check_updates,
            client_log_path,
            poesessid,
            account_name,
            live_searches,
            hotkey_upgrade,
            macro_open_delay_ms,
            advanced_copy,
            hotkey_market,
            market_volume_floor_div,
            market_listings_rank,
            market_listings_min,
            price_gem_rows,
            hotkey_craft,
            craft_runs,
            craft_give_up_times,
            craft_observed_min,
            notices,
        } = self;
        f.debug_struct("Config")
            .field("league", league)
            .field("restore_token", &restore_token.as_ref().map(|_| REDACTED))
            .field("divine_threshold", divine_threshold)
            .field("refresh_minutes", refresh_minutes)
            .field("hotkey_price_check", hotkey_price_check)
            .field("hotkey_settings", hotkey_settings)
            .field("pause_when_hidden", pause_when_hidden)
            .field("overlay_opacity", overlay_opacity)
            .field("tier_decent_chaos", tier_decent_chaos)
            .field("tier_good_chaos", tier_good_chaos)
            .field("macros", macros)
            .field("resource_shortcuts", resource_shortcuts)
            .field("map_danger_needles", map_danger_needles)
            .field("map_good_needles", map_good_needles)
            .field("check_updates", check_updates)
            .field("client_log_path", client_log_path)
            .field("poesessid", &if poesessid.is_empty() { "" } else { REDACTED })
            .field("account_name", account_name)
            .field("live_searches", live_searches)
            .field("hotkey_upgrade", hotkey_upgrade)
            .field("macro_open_delay_ms", macro_open_delay_ms)
            .field("advanced_copy", advanced_copy)
            .field("hotkey_market", hotkey_market)
            .field("market_volume_floor_div", market_volume_floor_div)
            .field("market_listings_rank", market_listings_rank)
            .field("market_listings_min", market_listings_min)
            .field("price_gem_rows", price_gem_rows)
            .field("hotkey_craft", hotkey_craft)
            .field("craft_runs", craft_runs)
            .field("craft_give_up_times", craft_give_up_times)
            .field("craft_observed_min", craft_observed_min)
            .field("notices", notices)
            .finish()
    }
}

impl Default for Config {
    fn default() -> Self {
        toml::from_str("league = \"Forbidden Rites\"").expect("defaults parse")
    }
}

impl Config {
    pub fn path() -> PathBuf {
        directories::ProjectDirs::from("", "", "khaloni-poe2")
            .expect("home dir resolvable")
            .config_dir()
            .join("config.toml")
    }

    pub fn load() -> anyhow::Result<Config> {
        Self::load_from(&Self::path())
    }

    /// Loads the config at `p`; a missing file is the defaults. A file left
    /// readable by other users (every version before the permissions were
    /// set at creation wrote 0644) is tightened on the way in.
    ///
    /// A file from before a setting moved is rewritten in today's shape
    /// right here, from the text just read, so the migration happens once
    /// and the overlay never has to save the whole config later on.
    pub fn load_from(p: &std::path::Path) -> anyhow::Result<Config> {
        match fs::read_to_string(p) {
            Ok(text) => {
                if let Err(e) = restrict_to_owner(p) {
                    eprintln!("config: could not restrict {} to its owner: {e}", p.display());
                }
                let cfg = Self::from_toml(&text)?;
                if let Some(token) = cfg.restore_token.as_deref() {
                    // The token leaves this file on the rewrite below, so it
                    // has to be safe in its own file first.
                    let store = RestoreToken::beside(p);
                    if store.load().is_none() {
                        store.save(token)?;
                    }
                }
                if cfg.restore_token.is_some() || !cfg.notices.is_empty() {
                    if let Err(e) = cfg.save_to(p) {
                        eprintln!("config: migrated settings not written back: {e}");
                    }
                }
                Ok(cfg)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Parses config text, holding the values to their allowed ranges.
    ///
    /// The value tiers used to be `tier_decent_ex` / `tier_good_ex`, in
    /// exalted. Tiers are now judged in chaos (see `pricing::tier_for_chaos`)
    /// and an exalted number cannot be carried over: at ~55 exalted to the
    /// chaos, "5 ex" read as 5 chaos would paint every reward Junk, and
    /// converted (0.09 chaos) every reward Jackpot. The conversion rate is
    /// not known when a config loads and moves by the day anyway, so a file
    /// with only the old keys gets the chaos defaults and a notice saying
    /// so. No serde alias on purpose: an alias is exactly the silent
    /// reinterpretation this avoids.
    pub fn from_toml(text: &str) -> Result<Config, toml::de::Error> {
        let mut table: toml::Table = toml::from_str(text)?;
        // Both keys go, whichever of them is present.
        let mut had_legacy = false;
        for key in LEGACY_TIER_KEYS {
            had_legacy |= table.remove(key).is_some();
        }
        let has_current = table.contains_key("tier_decent_chaos") || table.contains_key("tier_good_chaos");
        let mut cfg: Config = table.try_into()?;
        cfg.refresh_minutes = cfg.refresh_minutes.max(MIN_REFRESH_MINUTES);
        cfg.clamp_craft();
        // Floors that cannot be used (a negative volume, a minimum above
        // the ranking count) would grade every item wrongly without a
        // word; they go back to the defaults and the overlay says so.
        if let Some(problem) = cfg.market_floors().problem() {
            let d = khaloni_poe2_core::market::Floors::default();
            cfg.market_volume_floor_div = d.volume_div;
            cfg.market_listings_rank = d.listings_rank;
            cfg.market_listings_min = d.listings_min;
            cfg.notices.push(format!("market floors reset to the defaults: {problem} (Settings > Market)"));
        }
        if had_legacy && !has_current {
            cfg.notices.push(format!(
                "value tiers are now in chaos: reset to {} / {} (Settings > Display)",
                cfg.tier_decent_chaos, cfg.tier_good_chaos
            ));
        }
        Ok(cfg)
    }

    pub fn save(&self) -> anyhow::Result<()> {
        self.save_to(&Self::path())
    }

    /// Holds the planner's settings to their ranges (a hand-edited 0 runs
    /// would cost nothing and show nothing). A give-up multiple that is not
    /// a number goes back to the default.
    pub fn clamp_craft(&mut self) {
        self.craft_runs = self.craft_runs.clamp(CRAFT_RUNS_RANGE.0, CRAFT_RUNS_RANGE.1);
        if !self.craft_give_up_times.is_finite() {
            self.craft_give_up_times = default_craft_give_up_times();
        }
        self.craft_give_up_times = self.craft_give_up_times.clamp(CRAFT_GIVE_UP_RANGE.0, CRAFT_GIVE_UP_RANGE.1);
        self.craft_observed_min = self.craft_observed_min.clamp(CRAFT_OBSERVED_MIN_RANGE.0, CRAFT_OBSERVED_MIN_RANGE.1);
    }

    /// The simulation settings a plan runs with.
    pub fn craft_sim(&self) -> khaloni_poe2_core::craft::sim::SimConfig {
        khaloni_poe2_core::craft::sim::SimConfig {
            runs: self.craft_runs,
            cap: khaloni_poe2_core::craft::sim::Cap::MedianTimes(self.craft_give_up_times),
            ..Default::default()
        }
    }

    /// The market view's floors as stored. `from_toml` has already put
    /// unusable ones back to the defaults.
    pub fn market_floors(&self) -> khaloni_poe2_core::market::Floors {
        khaloni_poe2_core::market::Floors {
            volume_div: self.market_volume_floor_div,
            listings_rank: self.market_listings_rank,
            listings_min: self.market_listings_min,
        }
    }

    pub fn save_to(&self, path: &std::path::Path) -> anyhow::Result<()> {
        // Blank needle rows are editing scaffolding (the settings list adds
        // an empty row to type into); persisted, an empty needle would match
        // every line. They stay in the UI and never reach the disk.
        let mut clean = self.clone();
        clean.map_danger_needles.retain(|n| !n.trim().is_empty());
        clean.map_good_needles.retain(|n| !n.trim().is_empty());
        write_atomic(path, &toml::to_string_pretty(&clean)?)
    }
}

const LEGACY_TIER_KEYS: [&str; 2] = ["tier_decent_ex", "tier_good_ex"];

/// The screencast portal's restore token (it grants silent capture on later
/// runs), in a file of its own beside the config. Only the overlay writes
/// it, so no other writer's save can revert it.
pub struct RestoreToken {
    path: PathBuf,
}

impl RestoreToken {
    pub fn new() -> RestoreToken {
        RestoreToken::beside(&Config::path())
    }

    /// The token file that goes with the config at `config_path`.
    pub fn beside(config_path: &std::path::Path) -> RestoreToken {
        RestoreToken { path: config_path.with_file_name("capture-token") }
    }

    pub fn load(&self) -> Option<String> {
        let text = fs::read_to_string(&self.path).ok()?;
        let token = text.trim();
        (!token.is_empty()).then(|| token.to_string())
    }

    pub fn save(&self, token: &str) -> anyhow::Result<()> {
        write_atomic(&self.path, token)
    }
}

impl Default for RestoreToken {
    fn default() -> RestoreToken {
        RestoreToken::new()
    }
}

/// Write `contents` to `path` atomically: temp file in the same directory,
/// fsync, then rename over the target. The overlay polls this file's mtime
/// every second, so it must never observe a torn write.
///
/// The file holds the session cookie and the capture restore token, so the
/// temp file is created owner-only (0600) before a byte is written, and the
/// rename carries that mode onto the target. Its name is unique per write:
/// the settings window and the overlay are separate processes saving the
/// same file, and a shared temp name let one truncate the other's
/// half-written copy.
pub fn write_atomic(path: &std::path::Path, contents: &str) -> anyhow::Result<()> {
    use std::io::Write;
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("config path has no parent dir"))?;
    fs::create_dir_all(dir)?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("config.toml");
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = dir.join(format!(".{name}.{}.{seq}.tmp", std::process::id()));
    let written = (|| -> std::io::Result<()> {
        let mut f = owner_only(fs::OpenOptions::new().write(true).create_new(true)).open(&tmp)?;
        f.write_all(contents.as_bytes())?;
        f.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    Ok(written?)
}

#[cfg(unix)]
fn owner_only(opts: &mut fs::OpenOptions) -> &mut fs::OpenOptions {
    use std::os::unix::fs::OpenOptionsExt;
    opts.mode(0o600)
}

/// Windows has no mode bits; the profile directory's ACL already keeps
/// other users out.
#[cfg(not(unix))]
fn owner_only(opts: &mut fs::OpenOptions) -> &mut fs::OpenOptions {
    opts
}

#[cfg(unix)]
fn restrict_to_owner(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(path)?.permissions().mode();
    if mode & 0o077 != 0 {
        fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o700))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn restrict_to_owner(_path: &std::path::Path) -> std::io::Result<()> {
    Ok(())
}
