//! Behavior tests for the Client.txt tailer against a real temp file: the
//! tail must skip pre-existing history (seek-to-end), deliver appended lines
//! as parsed events, and recover from truncation (log rotation) by reopening
//! from the start.

use std::io::Write;
use std::sync::mpsc;
use std::time::Duration;

use khaloni_poe2::gamelog_tail;
use khaloni_poe2_core::gamelog::LogEvent;

/// The fixed line prefix Client.txt puts before every message.
const PREFIX: &str = "2026/07/24 12:18:52 313944271 3ef231e0 [INFO Client 356] : ";

fn zone_line(zone: &str) -> String {
    format!("{PREFIX}You have entered {zone}.\n")
}

fn append(path: &std::path::Path, s: &str) {
    let mut f = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    f.write_all(s.as_bytes()).unwrap();
    f.flush().unwrap();
}

/// One sequential test covers spawn/seek/append/truncate: the tailer polls at
/// 500ms, so each phase costs real wall time and separate tests would multiply
/// it (integration tests in one binary run serially anyway).
#[test]
fn tail_skips_history_delivers_appends_and_survives_truncation() {
    let dir = std::env::temp_dir().join(format!("khalonipoe2-tail-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Client.txt");

    // History written BEFORE spawn must never replay: if it did, every app
    // start would re-fire hours of old zone events.
    std::fs::write(&path, zone_line("Stale History Zone")).unwrap();

    let (tx, rx) = mpsc::channel::<LogEvent>();
    let _h = gamelog_tail::spawn(path.clone(), tx);

    // The tailer records the end-of-file position when it opens; give the
    // thread ample time to get there before appending, so the appended line
    // is unambiguously "new".
    std::thread::sleep(Duration::from_millis(1000));

    append(&path, &zone_line("Ziggurat Refuge"));
    let ev = rx.recv_timeout(Duration::from_secs(10)).expect("appended line should arrive");
    assert_eq!(ev, LogEvent::ZoneEnter("Ziggurat Refuge".to_string()));

    // Unparseable lines are dropped, not delivered.
    append(&path, &format!("{PREFIX}Connecting to instance server\n"));

    // Truncation (log rotated/cleared): the new content is SHORTER than the
    // old read position, which is exactly the case the tailer must detect
    // (size shrank) and answer by reopening from the start.
    std::fs::write(&path, zone_line("Reborn")).unwrap();
    let ev = rx.recv_timeout(Duration::from_secs(10)).expect("post-truncation line should arrive");
    assert_eq!(ev, LogEvent::ZoneEnter("Reborn".to_string()));

    let _ = std::fs::remove_dir_all(&dir);
}

/// Dropping the receiver closes the channel; the tail thread must notice on
/// its next send and exit rather than tail forever.
#[test]
fn tail_thread_exits_when_receiver_is_dropped() {
    let dir = std::env::temp_dir().join(format!("khalonipoe2-tailexit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Client.txt");
    std::fs::write(&path, "").unwrap();

    let (tx, rx) = mpsc::channel::<LogEvent>();
    let h = gamelog_tail::spawn(path.clone(), tx);
    std::thread::sleep(Duration::from_millis(1000));
    drop(rx);
    // The next parsed line's send fails, which is the exit signal.
    append(&path, &zone_line("Anywhere"));
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !h.is_finished() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(h.is_finished(), "tail thread should exit after the channel closes");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `default_log_path` only returns paths that exist; on a machine without the
/// game installed it must be None rather than a guess.
#[test]
fn default_log_path_is_existing_or_none() {
    if let Some(p) = gamelog_tail::default_log_path() {
        assert!(p.exists());
    }
}
