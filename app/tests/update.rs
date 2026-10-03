use khaloni_poe2::update::{host_allowed, is_newer, parse_version, pick_asset, sha_for};

#[test]
fn version_comparison_only_moves_forward() {
    assert!(is_newer("0.2.0", "v0.2.1"));
    assert!(is_newer("v0.2.0", "1.0.0"));
    assert!(is_newer("0.2.0", "0.3.0"));
    // Same version, or an older one, must never offer an "update".
    assert!(!is_newer("0.2.0", "0.2.0"));
    assert!(!is_newer("0.2.0", "v0.1.9"));
    assert!(!is_newer("1.0.0", "0.9.9"));
    // Garbage tags are inert rather than a downgrade vector.
    assert!(!is_newer("0.2.0", "latest"));
    assert!(!is_newer("0.2.0", ""));
    assert!(!is_newer("not-a-version", "9.9.9"));
    // Pre-release metadata does not participate in the comparison.
    assert_eq!(parse_version("v1.2.3-rc1"), Some((1, 2, 3)));
    assert_eq!(parse_version("1.2"), None);
}

#[test]
fn asset_pick_ignores_archives() {
    let names = [
        "SHA256SUMS",
        "khaloni-poe2-v0.2.1-linux-x86_64.tar.gz",
        "khaloni-poe2-v0.2.1-windows-x86_64.zip",
        "khaloni-poe2-v0.2.1-linux-x86_64",
        "khaloni-poe2-v0.2.1-windows-x86_64.exe",
    ];
    let picked = pick_asset(names).expect("this target's binary is in the list");
    // Whichever platform the tests run on, the archive with the same suffix
    // must not win.
    assert!(!picked.ends_with(".zip") && !picked.ends_with(".tar.gz"));
    assert!(picked.starts_with("khaloni-poe2-v0.2.1-"));
    // A release without a raw binary yields nothing to install.
    assert_eq!(pick_asset(["SHA256SUMS", "khaloni-poe2-v0.2.1-linux-x86_64.tar.gz"]), None);
}

#[test]
fn checksum_lookup_matches_the_exact_asset() {
    let good = "a".repeat(64);
    let other = "b".repeat(64);
    let sums = format!(
        "{good}  khaloni-poe2-v0.2.1-linux-x86_64\n{other} *khaloni-poe2-v0.2.1-windows-x86_64.exe\n"
    );
    assert_eq!(sha_for(&sums, "khaloni-poe2-v0.2.1-linux-x86_64"), Some(good));
    // The '*' binary marker sha256sum writes must not defeat the match.
    assert_eq!(sha_for(&sums, "khaloni-poe2-v0.2.1-windows-x86_64.exe"), Some(other));
    // An asset with no line, or a malformed hash, has no checksum.
    assert_eq!(sha_for(&sums, "khaloni-poe2-v0.2.1-macos"), None);
    assert_eq!(sha_for("deadbeef  khaloni-poe2-v0.2.1-linux-x86_64", "khaloni-poe2-v0.2.1-linux-x86_64"), None);
}

#[test]
fn only_github_hosts_are_downloadable() {
    assert!(host_allowed("https://github.com/Goo6i/khaloni-poe2/releases/download/v1/x"));
    assert!(host_allowed("https://objects.githubusercontent.com/whatever"));
    // Look-alike hosts, path tricks, and plaintext are all refused.
    assert!(!host_allowed("https://github.com.evil.example/x"));
    assert!(!host_allowed("https://evil.example/github.com/x"));
    assert!(!host_allowed("http://github.com/x"));
    assert!(!host_allowed("ftp://github.com/x"));
    assert!(!host_allowed(""));
}

#[test]
fn understands_a_real_published_release() {
    // The actual GitHub payload for the v0.2.1 release (captured live), so
    // a workflow change that stops publishing bare binaries or SHA256SUMS
    // fails here instead of silently disabling everyone's updates.
    let body: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/release_latest.json")).unwrap();

    let plan = khaloni_poe2::update::plan_from_release(&body, "0.2.0")
        .expect("an older build must see this release as an update");
    assert_eq!(plan.version, "v0.2.1");
    // The bare binary, never the archive, and always over a GitHub host.
    assert!(!plan.asset_name.ends_with(".zip") && !plan.asset_name.ends_with(".tar.gz"));
    assert!(plan.asset_url.starts_with("https://github.com/Goo6i/khaloni-poe2/releases/download/"));
    assert!(plan.sums_url.ends_with("/SHA256SUMS"));

    // Same version, and a newer one, both offer nothing.
    assert_eq!(khaloni_poe2::update::plan_from_release(&body, "0.2.1"), None);
    assert_eq!(khaloni_poe2::update::plan_from_release(&body, "9.9.9"), None);
}

#[test]
fn a_release_without_checksums_offers_nothing() {
    // Exactly the half-published state this project hit when the checksums
    // job lost its runner: binaries up, SHA256SUMS missing. An updater that
    // installed from it would be installing unverified bytes.
    let body: serde_json::Value = serde_json::json!({
        "tag_name": "v9.0.0",
        "html_url": "https://github.com/Goo6i/khaloni-poe2/releases/tag/v9.0.0",
        "assets": [
            {"name": "khaloni-poe2-v9.0.0-linux-x86_64",
             "browser_download_url": "https://github.com/Goo6i/khaloni-poe2/releases/download/v9.0.0/khaloni-poe2-v9.0.0-linux-x86_64"},
            {"name": "khaloni-poe2-v9.0.0-windows-x86_64.exe",
             "browser_download_url": "https://github.com/Goo6i/khaloni-poe2/releases/download/v9.0.0/khaloni-poe2-v9.0.0-windows-x86_64.exe"}
        ]
    });
    assert_eq!(khaloni_poe2::update::plan_from_release(&body, "0.2.1"), None);
}

#[test]
fn per_asset_checksum_files_are_read_in_both_shapes() {
    use khaloni_poe2::update::sha_from_file;
    let h = "c".repeat(64);
    // A bare digest (what many tools emit) …
    assert_eq!(sha_from_file(&format!("{h}\n"), "khaloni-poe2-v1-linux-x86_64"), Some(h.clone()));
    // … and a sha256sum line naming the asset.
    assert_eq!(
        sha_from_file(&format!("{h}  khaloni-poe2-v1-linux-x86_64\n"), "khaloni-poe2-v1-linux-x86_64"),
        Some(h.clone())
    );
    // Uppercase is normalized; junk and wrong names still yield nothing.
    assert_eq!(sha_from_file(&h.to_uppercase(), "x"), Some(h));
    assert_eq!(sha_from_file("not a hash", "x"), None);
    assert_eq!(sha_from_file(&format!("{}  other-asset\n", "d".repeat(64)), "x"), None);
}

#[test]
fn per_asset_checksum_wins_over_the_combined_file() {
    // Both present: the file published by the same job that built the
    // binary is the one to trust.
    let body: serde_json::Value = serde_json::json!({
        "tag_name": "v9.0.0",
        "assets": [
            {"name": "khaloni-poe2-v9.0.0-linux-x86_64",
             "browser_download_url": "https://github.com/Goo6i/khaloni-poe2/releases/download/v9.0.0/khaloni-poe2-v9.0.0-linux-x86_64"},
            {"name": "khaloni-poe2-v9.0.0-linux-x86_64.sha256",
             "browser_download_url": "https://github.com/Goo6i/khaloni-poe2/releases/download/v9.0.0/khaloni-poe2-v9.0.0-linux-x86_64.sha256"},
            {"name": "khaloni-poe2-v9.0.0-windows-x86_64.exe",
             "browser_download_url": "https://github.com/Goo6i/khaloni-poe2/releases/download/v9.0.0/khaloni-poe2-v9.0.0-windows-x86_64.exe"},
            {"name": "khaloni-poe2-v9.0.0-windows-x86_64.exe.sha256",
             "browser_download_url": "https://github.com/Goo6i/khaloni-poe2/releases/download/v9.0.0/khaloni-poe2-v9.0.0-windows-x86_64.exe.sha256"},
            {"name": "SHA256SUMS",
             "browser_download_url": "https://github.com/Goo6i/khaloni-poe2/releases/download/v9.0.0/SHA256SUMS"}
        ]
    });
    let plan = khaloni_poe2::update::plan_from_release(&body, "0.2.1").expect("newer release");
    assert_eq!(plan.sums_url, format!("https://github.com/Goo6i/khaloni-poe2/releases/download/v9.0.0/{}.sha256", plan.asset_name));
    // The .sha256 sidecar must never be mistaken for the binary itself.
    assert!(!plan.asset_name.ends_with(".sha256"));
}

// --- download and install ---

use std::io::{Read, Write};
use std::time::{Duration, Instant};

/// Serves one response: the headers claiming `total` bytes, then `chunks`
/// chunks `gap` apart, then (when fewer bytes than claimed were sent) an
/// open, silent connection.
fn serve(total: usize, chunks: usize, chunk: usize, gap: Duration) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let Ok((mut s, _)) = listener.accept() else { return };
        let mut buf = [0u8; 2048];
        let _ = s.read(&mut buf);
        let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {total}\r\nConnection: close\r\n\r\n");
        for _ in 0..chunks {
            if s.write_all(&vec![7u8; chunk]).and_then(|()| s.flush()).is_err() {
                return;
            }
            std::thread::sleep(gap);
        }
        if chunks * chunk < total {
            std::thread::sleep(Duration::from_secs(20));
        }
    });
    format!("http://{addr}/asset")
}

#[test]
fn a_slow_but_moving_download_outlives_the_idle_timeout() {
    // Ten chunks 150 ms apart take 1.5 s in all, three times the idle
    // allowance: only a gap between chunks may end the download, never its
    // total length.
    let url = serve(10_000, 10, 1_000, Duration::from_millis(150));
    let bytes = khaloni_poe2::update::download(&url, 1 << 20, Duration::from_secs(5), Duration::from_millis(500))
        .expect("a moving download completes");
    assert_eq!(bytes.len(), 10_000);
}

#[test]
fn a_stalled_download_fails_after_the_idle_timeout() {
    let url = serve(10_000, 2, 1_000, Duration::from_millis(10));
    let started = Instant::now();
    let err = khaloni_poe2::update::download(&url, 1 << 20, Duration::from_secs(5), Duration::from_millis(400))
        .expect_err("a connection that goes quiet is given up on");
    assert!(err.to_string().contains("stalled"), "{err}");
    assert!(started.elapsed() < Duration::from_secs(10), "gave up after {:?}", started.elapsed());
}

#[test]
fn an_oversized_download_is_refused() {
    let url = serve(5_000, 5, 1_000, Duration::from_millis(1));
    assert!(khaloni_poe2::update::download(&url, 1_000, Duration::from_secs(5), Duration::from_secs(2)).is_err());
}

fn install_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("khalonipoe2-install-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn install_replaces_the_binary_and_keeps_the_previous_one() {
    let dir = install_dir("swap");
    let (exe, staged, backup) = (dir.join("khaloni-poe2"), dir.join(".khaloni-poe2.new"), dir.join(".khaloni-poe2.old"));
    std::fs::write(&exe, "old binary").unwrap();
    std::fs::write(&staged, "new binary").unwrap();
    std::fs::write(&backup, "an even older backup").unwrap();
    khaloni_poe2::update::install(&staged, &exe, &backup).expect("installs");
    assert_eq!(std::fs::read_to_string(&exe).unwrap(), "new binary");
    assert_eq!(std::fs::read_to_string(&backup).unwrap(), "old binary");
    assert!(!staged.exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn a_failed_install_leaves_the_running_binary_where_it_was() {
    let dir = install_dir("fail");
    let (exe, backup) = (dir.join("khaloni-poe2"), dir.join(".khaloni-poe2.old"));
    std::fs::write(&exe, "old binary").unwrap();
    // No staged file: the rename over the executable fails, and the
    // executable must never have moved.
    assert!(khaloni_poe2::update::install(&dir.join("missing"), &exe, &backup).is_err());
    assert_eq!(std::fs::read_to_string(&exe).unwrap(), "old binary");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn the_backup_survives_the_first_start_after_an_update() {
    let dir = install_dir("sweep");
    let (backup, pending) = (dir.join(".khaloni-poe2.old"), dir.join(".khaloni-poe2.pending"));
    std::fs::write(&backup, "old binary").unwrap();
    std::fs::write(&pending, "v9.9.9").unwrap();
    // First start of the new version: the marker goes, the way back stays.
    khaloni_poe2::update::cleanup_backup_in(&dir);
    assert!(backup.exists() && !pending.exists());
    // It has started once; the next start sweeps the backup.
    khaloni_poe2::update::cleanup_backup_in(&dir);
    assert!(!backup.exists());
    std::fs::remove_dir_all(dir).unwrap();
}
