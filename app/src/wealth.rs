//! Wealth tracker worker: every 30 minutes it walks the account's stash
//! tabs (via the core legacy-stash client, which owns the rate limiting),
//! prices every item through the live poe.ninja table, appends the total to
//! an on-disk jsonl history, and sends the fresh snapshot to the UI.

use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use khaloni_poe2_core::stash::{fetch_stash_value, StashClient};
use serde::{Deserialize, Serialize};

use crate::prices::PriceService;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WealthSnapshot {
    pub at_epoch_s: u64,
    pub total_ex: f64,
}

/// Stash contents move on the hours scale and each snapshot costs up to 20
/// account-endpoint requests; 30 minutes is frequent enough for a trend
/// line and polite to the endpoint.
const SNAPSHOT_INTERVAL: Duration = Duration::from_secs(30 * 60);

/// Snapshot history lives beside the other caches: one JSON object per
/// line, append-only, so a crash can lose at most the line being written.
pub fn history_path() -> PathBuf {
    directories::ProjectDirs::from("", "", "khaloni-poe2")
        .map(|d| d.cache_dir().to_path_buf())
        .unwrap_or_else(std::env::temp_dir)
        .join("wealth.jsonl")
}

/// Last `limit` snapshots, oldest first. Any unreadable line (torn write,
/// old format) is skipped: history is a nicety, never an error source.
pub fn load_history(limit: usize) -> Vec<WealthSnapshot> {
    let Ok(text) = std::fs::read_to_string(history_path()) else {
        return Vec::new();
    };
    let all: Vec<WealthSnapshot> = text
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let skip = all.len().saturating_sub(limit);
    all.into_iter().skip(skip).collect()
}

fn append_history(snap: &WealthSnapshot) -> std::io::Result<()> {
    use std::io::Write;
    let path = history_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(f, "{}", serde_json::json!(snap))
}

/// Spawns the snapshot worker. Missing account name or session cookie means
/// the feature is off: nothing is spawned, no request is ever made. The
/// thread exits when the snapshot receiver is dropped.
pub fn spawn(
    account: String,
    league: String,
    poesessid: String,
    svc: PriceService,
    tx: Sender<WealthSnapshot>,
) {
    if account.trim().is_empty() || poesessid.trim().is_empty() {
        return;
    }
    std::thread::spawn(move || run(account, league, poesessid, svc, tx));
}

fn run(account: String, league: String, poesessid: String, svc: PriceService, tx: Sender<WealthSnapshot>) {
    // One client for the thread's lifetime: its rate limiter must remember
    // the request history across snapshots.
    let mut client = StashClient::new();
    loop {
        // A fresh price snapshot per cycle: the table refreshes on its own
        // cadence and this picks up whatever is current. Unknown names
        // price at 0 by design - the total is a lower bound, and gear the
        // exchange table cannot price would otherwise poison the trend.
        let prices = svc.snapshot();
        let price = |type_line: &str, stack: u32| -> f64 {
            prices
                .table
                .lookup(type_line)
                .map(|p| p.exalted * f64::from(stack))
                .unwrap_or(0.0)
        };
        match fetch_stash_value(&mut client, &account, &league, &poesessid, &price) {
            Ok(total_ex) => {
                let at_epoch_s = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let snap = WealthSnapshot { at_epoch_s, total_ex };
                if let Err(e) = append_history(&snap) {
                    eprintln!("wealth: history append failed: {e}");
                }
                if tx.send(snap).is_err() {
                    return; // receiver gone: the app is shutting down
                }
            }
            // Log-and-continue: a bad cookie or a network blip must not
            // kill the tracker; the next cycle retries with current config.
            Err(e) => eprintln!("wealth snapshot failed: {e}"),
        }
        std::thread::sleep(SNAPSHOT_INTERVAL);
    }
}
