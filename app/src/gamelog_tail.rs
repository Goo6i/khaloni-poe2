//! Tails the game's `Client.txt` on a background thread and streams parsed
//! `LogEvent`s over an mpsc channel. The first open seeks to the END of the
//! file — the log holds hours of history and replaying it would re-fire every
//! old zone/whisper event on each app start. After that it polls (~500ms:
//! inotify/ReadDirectoryChanges would be per-OS machinery for a file that
//! only needs sub-second latency) and handles the two ways the file goes
//! sideways: truncation/rotation (size shrank → reopen from the start) and
//! the file not existing yet (the game may launch after us → retry forever).

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::Duration;

use khaloni_poe2_core::gamelog::{self, LogEvent};

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

/// Spawns the tail thread. Events flow out through `tx`; the thread exits
/// when the receiver is dropped (its next send fails). The handle is
/// returned for tests — the app can just detach it.
pub fn spawn(path: PathBuf, tx: Sender<LogEvent>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || tail_loop(&path, &tx))
}

fn tail_loop(path: &Path, tx: &Sender<LogEvent>) {
    // Seek-to-end applies only to the FIRST successful open (skip history);
    // a reopen after truncation/rotation must read the new content from the
    // start or the first lines of the fresh file would be lost.
    let mut first_open = true;
    loop {
        let mut file = match File::open(path) {
            Ok(f) => f,
            Err(_) => {
                std::thread::sleep(RETRY_OPEN);
                continue;
            }
        };
        let mut pos: u64 = if first_open {
            file.seek(SeekFrom::End(0)).unwrap_or(0)
        } else {
            0
        };
        first_open = false;
        // Bytes read but not yet terminated by '\n': the writer can flush
        // mid-line, so only complete lines are parsed and the partial tail
        // carries over to the next poll.
        let mut carry = String::new();

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
                while let Some(nl) = carry.find('\n') {
                    let line = carry[..nl].trim_end_matches('\r').to_string();
                    carry.drain(..=nl);
                    if let Some(ev) = gamelog::parse_line(&line) {
                        if tx.send(ev).is_err() {
                            return; // receiver gone: the app is done with us
                        }
                    }
                }
            }
            std::thread::sleep(POLL);
        }
    }
}
