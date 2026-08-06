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
