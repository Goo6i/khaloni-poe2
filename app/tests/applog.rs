//! Everything the overlay writes to stderr also lands in a log file that
//! survives the session, rotated so it cannot grow without bound.

#![cfg(unix)]

// Pulled in by path until the module is registered in the crate; the
// parts a test does not call are not dead.
#[allow(dead_code)]
#[path = "../src/applog.rs"]
mod applog;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use applog::{install_to, KEEP, MAX_BYTES};

/// Each test replaces the process's stderr; one at a time keeps the tee
/// chain simple to reason about.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("khalonipoe2-applog-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The test harness captures what `eprintln!` prints inside a test thread
/// and never lets it reach file descriptor 2, so the tests write to the
/// stderr handle directly; outside the harness both take the same path.
fn say(line: &str) {
    let mut err = std::io::stderr();
    err.write_all(line.as_bytes()).unwrap();
    err.write_all(b"\n").unwrap();
    err.flush().unwrap();
}

fn read_or_empty(p: &Path) -> String {
    std::fs::read(p).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default()
}

fn wait_until(what: &str, mut ok: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ok() {
        assert!(Instant::now() < deadline, "gave up waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_line_written_to_stderr_lands_in_the_log_file() {
    let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let dir = temp_dir("line");
    install_to(&dir, MAX_BYTES, KEEP).unwrap();
    let marker = format!("marker-{}-{:?}", std::process::id(), Instant::now());
    say(&marker);
    let log = dir.join("overlay.log");
    wait_until("the marker in overlay.log", || read_or_empty(&log).contains(&marker));
    // Whole lines, newline included: the file reads like the terminal did.
    assert!(read_or_empty(&log).contains(&format!("{marker}\n")));
}

#[test]
fn the_log_rotates_at_five_megabytes_keeping_four_files() {
    let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(MAX_BYTES, 5 * 1024 * 1024);
    assert_eq!(KEEP, 4);
    let dir = temp_dir("rotate");
    let max = 600;
    install_to(&dir, max, KEEP).unwrap();
    // Ten files' worth of lines: enough to rotate past the four that are
    // kept several times over. (They echo to the real stderr as well; the
    // limit is kept small so that stays a few kilobytes.)
    let line = format!("{:<29}", "rotate");
    for _ in 0..(max as usize * 10 / (line.len() + 1)) {
        say(&line);
    }
    let marker = format!("last-{}", std::process::id());
    say(&marker);
    let names: Vec<PathBuf> = (0..=KEEP)
        .map(|i| dir.join(if i == 0 { "overlay.log".to_string() } else { format!("overlay.{i}.log") }))
        .collect();
    wait_until("the last line in a log file", || {
        names.iter().any(|p| read_or_empty(p).contains(&marker))
    });
    // overlay.log plus overlay.1.log .. overlay.3.log, and nothing older.
    for p in &names[..KEEP] {
        assert!(p.exists(), "{} missing", p.display());
    }
    assert!(!names[KEEP].exists(), "{} should have been dropped", names[KEEP].display());
    // No file grew past the limit, and the rotated ones are full: rotation
    // happened at the size, not at some line count.
    for p in &names[..KEEP] {
        let len = std::fs::metadata(p).unwrap().len();
        assert!(len <= max, "{} is {len} bytes, over {max}", p.display());
    }
    for p in &names[1..KEEP] {
        let len = std::fs::metadata(p).unwrap().len();
        assert!(len > max - 2 * (line.len() as u64 + 1), "{} is only {len} bytes", p.display());
    }
}
