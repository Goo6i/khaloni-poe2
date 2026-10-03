//! Tails the game's `Client.txt` on a background thread for the run
//! tracker. The first open reads the end of the existing log into the
//! tracker (`myruns::cold_read`: the maps played before the overlay started
//! count too), and every line read afterwards goes to the hub. It polls
//! (~500ms: inotify/ReadDirectoryChanges would be per-OS machinery for a
//! file that only needs sub-second latency) and handles the two ways the
//! file goes sideways: truncation/rotation (size shrank → reopen from the
//! start) and the file not existing yet (the game may launch after us →
//! retry forever).

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;

const POLL: Duration = Duration::from_millis(500);
/// Missing-file retry is much slower than the read poll: stat-ing a path that
/// does not exist every half second buys nothing.
const RETRY_OPEN: Duration = Duration::from_secs(5);

/// The default Steam install's `Client.txt`, if it exists on this machine.
/// Only the stock per-OS Steam library is probed; a custom library location
/// needs explicit configuration by the user.
pub fn default_log_path() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    #[cfg(not(windows))]
    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(
            PathBuf::from(home)
                .join(".local/share/Steam/steamapps/common/Path of Exile 2/logs/Client.txt"),
        );
    }
    #[cfg(windows)]
    candidates.push(PathBuf::from(
        r"C:\Program Files (x86)\Steam\steamapps\common\Path of Exile 2\logs\Client.txt",
    ));
    candidates.into_iter().find(|p| p.exists())
}

/// The tail feeding the run tracker. It runs for the life of the process;
/// the handle is returned for tests.
pub fn spawn_with_runs(path: PathBuf, hub: std::sync::Arc<crate::myruns::Hub>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || tail_loop(&path, &hub))
}

fn epoch(t: std::time::SystemTime) -> Option<i64> {
    t.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs() as i64)
}

fn tail_loop(path: &Path, hub: &crate::myruns::Hub) {
    // The cold read applies only to the FIRST successful open; a reopen
    // after truncation/rotation must read the new content from the start or
    // the first lines of the fresh file would be lost.
    let mut first_open = true;
    loop {
        let mut file = match File::open(path) {
            Ok(f) => f,
            Err(_) => {
                hub.set_log_missing();
                std::thread::sleep(RETRY_OPEN);
                continue;
            }
        };
        // Bytes read but not yet terminated by '\n': the writer can flush
        // mid-line, so only complete lines are parsed and the partial tail
        // carries over to the next poll.
        let mut carry = String::new();
        let mut pos: u64 = match first_open {
            // The tail picks up exactly where the cold read stopped, its
            // unfinished last line included, so no line is lost or read
            // twice between the two.
            true => match crate::myruns::cold_read(path) {
                Ok((tracker, end, rest)) => {
                    let mtime = file.metadata().ok().and_then(|m| m.modified().ok()).and_then(epoch);
                    hub.set_cold(tracker, mtime);
                    carry = rest;
                    end
                }
                Err(_) => {
                    std::thread::sleep(RETRY_OPEN);
                    continue;
                }
            },
            false => 0,
        };
        first_open = false;

        #[allow(clippy::while_let_loop)] // retry structure reads clearer explicit
        loop {
            let len = match file.metadata() {
                Ok(m) => m.len(),
                Err(_) => break, // fd went bad (file replaced/deleted) → reopen
            };
            if len < pos {
                break; // truncated/rotated → reopen from the start
            }
            if len > pos {
                let mut chunk = Vec::with_capacity((len - pos) as usize);
                if file.seek(SeekFrom::Start(pos)).is_err() {
                    break;
                }
                match (&mut file).take(len - pos).read_to_end(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => pos += n as u64,
                }
                // Lossy decode: one bad byte in a chat line must not stall
                // the tail.
                carry.push_str(&String::from_utf8_lossy(&chunk));
                let mut lines = Vec::new();
                while let Some(nl) = carry.find('\n') {
                    lines.push(carry[..nl].trim_end_matches('\r').to_string());
                    carry.drain(..=nl);
                }
                let now = epoch(std::time::SystemTime::now()).unwrap_or(0);
                hub.feed_live(lines.iter().map(String::as_str), now);
            }
            std::thread::sleep(POLL);
        }
    }
}
