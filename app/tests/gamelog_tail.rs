//! Behavior tests for the Client.txt tailer against a real temp file: the
//! run tracker gets the history once and every appended line after it, and
//! the tail recovers from truncation (log rotation) by reopening from the
//! start of the new file.

use std::io::Write;
use std::time::Duration;

use khaloni_poe2::gamelog_tail;

/// The fixed line prefix Client.txt puts before every message.
const PREFIX: &str = "2026/07/24 12:18:52 313944271 3ef231e0 [INFO Client 356] : ";

fn zone_line(zone: &str) -> String {
    format!("{PREFIX}You have entered {zone}.\n")
}

fn area(clock: &str, code: &str, seed: u32) -> String {
    format!("2026/07/24 {clock} 313944271 3ef231e0 [DEBUG Client 356] Generating level 79 area \"{code}\" with seed {seed}\n")
}

fn append(path: &std::path::Path, s: &str) {
    let mut f = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    f.write_all(s.as_bytes()).unwrap();
    f.flush().unwrap();
}

/// Waits for the hub to hold `n` finished runs.
fn wait_for(hub: &khaloni_poe2::myruns::Hub, n: usize) {
    for _ in 0..60 {
        if hub.finished_runs().len() == n {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("expected {n} finished runs, have {}", hub.finished_runs().len());
}

/// A rotated or cleared log is shorter than the read position: the tail
/// must notice the size shrank and read the new file from its first line,
/// or the maps played right after the rotation would be lost.
#[test]
fn the_runs_tail_survives_truncation_and_reads_the_new_file_from_its_start() {
    let dir = std::env::temp_dir().join(format!("khalonipoe2-tailtrunc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Client.txt");
    // A long history, so the rotated file is shorter than where the tail
    // stands.
    let history: String = (0..40).map(|i| zone_line(&format!("Old Zone {i}"))).collect::<String>()
        + &area("12:00:00", "MapRavine", 7)
        + &area("12:04:00", "HideoutCanal", 1);
    std::fs::write(&path, history).unwrap();

    let hub = khaloni_poe2::myruns::Hub::new();
    let _h = gamelog_tail::spawn_with_runs(path.clone(), hub.clone());
    wait_for(&hub, 1);

    // Rotation: the new file holds one whole map from its first line.
    std::fs::write(&path, [area("13:00:00", "MapSteppe", 8), area("13:03:00", "HideoutCanal", 1)].concat()).unwrap();
    wait_for(&hub, 2);
    let runs = hub.finished_runs();
    assert_eq!((runs[1].area.as_str(), runs[1].seconds()), ("MapSteppe", Some(180)));
    // And the tail goes on from there.
    append(&path, &[area("13:10:00", "MapCanyon", 9), area("13:12:00", "HideoutCanal", 1)].concat());
    wait_for(&hub, 3);
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

/// With a runs hub, the maps already in the log count from the start and
/// the lines that follow reach the tracker exactly once.
#[test]
fn the_tail_hands_the_run_tracker_the_history_and_then_every_new_line() {
    let dir = std::env::temp_dir().join(format!("khalonipoe2-tailruns-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Client.txt");
    let history = [
        zone_line("Stale History Zone"),
        area("12:00:00", "MapRavine", 7),
        area("12:04:00", "HideoutCanal", 1),
        area("12:05:00", "MapSteppe", 8),
    ]
    .concat();
    // The writer stopped in the middle of the line that ends the map.
    let (head, tail) = {
        let line = area("12:09:00", "HideoutCanal", 1);
        (line[..30].to_string(), line[30..].to_string())
    };
    std::fs::write(&path, format!("{history}{head}")).unwrap();

    let hub = khaloni_poe2::myruns::Hub::new();
    let _h = gamelog_tail::spawn_with_runs(path.clone(), hub.clone());
    let wait_for = |n: usize| {
        for _ in 0..40 {
            if hub.finished_runs().len() == n {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("expected {n} finished runs, have {}", hub.finished_runs().len());
    };
    wait_for(1);
    assert_eq!(hub.finished_runs()[0].seconds(), Some(240));

    append(&path, &format!("{tail}{}", zone_line("Fresh Zone")));
    wait_for(2);
    let runs = hub.finished_runs();
    assert_eq!((runs[1].area.as_str(), runs[1].seconds(), runs[1].portals), ("MapSteppe", Some(240), 0));
    assert!(hub.in_hideout_for().is_some());
    let _ = std::fs::remove_dir_all(&dir);
}
