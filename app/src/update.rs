//! Self-update against this project's GitHub releases.
//!
//! Deliberate constraints, because an updater downloads and then RUNS code:
//!
//! - Only ever talks to the hardcoded repo's API over HTTPS, and only
//!   accepts an asset download whose URL is on a github.com host.
//! - Verifies the downloaded bytes against the SHA256SUMS asset published
//!   by the release workflow before anything is installed. No checksum, no
//!   install — a release without one is treated as "nothing to update to".
//! - Never applies silently and never restarts the app: the check runs in
//!   the background and only reports; installing is an explicit click in
//!   the settings window, and the new binary takes effect on the next
//!   launch. Swapping a running overlay out from under a live game would
//!   be hostile no matter how convenient.
//! - Refuses to touch a binary inside a cargo target directory, so a dev
//!   checkout is never overwritten by a release build.
//!
//! Only the executable is swapped. Data files that ship in the archives
//! (eng.traineddata) change rarely and stay the archive's job, which keeps
//! this module free of zip/tar handling.

use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// The repo releases are published from.
pub const REPO: &str = "Goo6i/khaloni-poe2";
/// This build's version, from Cargo.toml.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// Refuse anything larger than this; a release binary is ~40MB.
const MAX_DOWNLOAD: u64 = 200 * 1024 * 1024;
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    /// Release tag, e.g. "v0.2.1".
    pub version: String,
    /// Human-facing release page.
    pub notes_url: String,
    /// Direct asset download (validated github.com host).
    pub asset_url: String,
    pub asset_name: String,
    /// Lowercase hex SHA-256 the download must match.
    pub sha256: String,
}

/// (major, minor, patch) from "v1.2.3", "1.2.3", "1.2.3-rc1"; None if the
/// three numeric components are not all present.
pub fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let s = s.trim().trim_start_matches(['v', 'V']);
    // Pre-release/build metadata does not participate in the comparison.
    let core = s.split(['-', '+']).next()?;
    let mut it = core.split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next()?.parse().ok()?;
    let patch = it.next()?.parse().ok()?;
    Some((major, minor, patch))
}

/// Whether `candidate` is a strictly newer release than `current`.
/// Unparseable versions never trigger an update: a garbled tag must not be
/// able to push a "downgrade" onto users.
pub fn is_newer(current: &str, candidate: &str) -> bool {
    match (parse_version(current), parse_version(candidate)) {
        (Some(cur), Some(new)) => new > cur,
        _ => false,
    }
}

/// Suffix identifying this target's raw-binary asset in a release.
pub fn asset_suffix() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "-windows-x86_64.exe"
    }
    #[cfg(not(target_os = "windows"))]
    {
        "-linux-x86_64"
    }
}

/// Picks this target's binary asset from a release's asset names. Archives
/// are skipped explicitly so a ".tar.gz" can never satisfy the Linux
/// suffix match.
pub fn pick_asset<'a>(names: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    names.into_iter().find(|n| {
        !n.ends_with(".zip") && !n.ends_with(".tar.gz") && n.ends_with(asset_suffix())
    })
}

/// The hash from a per-asset checksum file: either a bare 64-hex digest
/// or a `sha256sum` line naming the asset. Per-asset files are what the
/// release jobs publish now (each alongside the binary it built), which
/// needs no third CI job to coordinate.
pub fn sha_from_file(text: &str, asset: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.len() == 64 && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
        return Some(trimmed.to_lowercase());
    }
    sha_for(text, asset)
}

/// The hash for `asset` from a `sha256sum`-style file ("<hex>  <name>").
pub fn sha_for(sums: &str, asset: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let name = parts.next()?.trim_start_matches('*');
        (name == asset && hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()))
            .then(|| hash.to_lowercase())
    })
}

/// Downloads must come from GitHub itself; a release edited to point
/// somewhere else is not something to fetch a binary from.
pub fn host_allowed(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split('/').next().unwrap_or("");
    host == "github.com"
        || host == "objects.githubusercontent.com"
        || host == "release-assets.githubusercontent.com"
}

/// True when the running executable lives in a cargo build directory, in
/// which case self-update is refused (it would clobber a dev build).
pub fn is_dev_build() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return true; // unknown provenance: refuse
    };
    exe.components().any(|c| c.as_os_str() == "target")
}

fn http() -> anyhow::Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .timeout(TIMEOUT)
        // GitHub rejects API requests without one.
        .user_agent(concat!("khaloni-poe2/", env!("CARGO_PKG_VERSION")))
        .build()?)
}

/// What a release offers this build, before any bytes are fetched: the
/// parsing half of `check`, split out so it is unit-testable against a
/// real captured release payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub version: String,
    pub notes_url: String,
    pub asset_url: String,
    pub asset_name: String,
    pub sums_url: String,
}

/// Reads a GitHub "latest release" payload. `None` when `current` is
/// already up to date, when no raw binary exists for this platform, or
/// when the release publishes no checksums to verify against.
pub fn plan_from_release(body: &serde_json::Value, current: &str) -> Option<Plan> {
    let tag = body.get("tag_name")?.as_str()?;
    if !is_newer(current, tag) {
        return None;
    }
    let assets = body.get("assets")?.as_array()?;
    let names: Vec<&str> = assets.iter().filter_map(|a| a.get("name")?.as_str()).collect();
    let asset_name = pick_asset(names)?.to_string();
    let url_of = |want: &str| -> Option<String> {
        assets.iter().find_map(|a| {
            (a.get("name")?.as_str()? == want)
                .then(|| a.get("browser_download_url")?.as_str().map(str::to_string))
                .flatten()
        })
    };
    let asset_url = url_of(&asset_name).filter(|u| host_allowed(u))?;
    // Unverifiable download = no update offered, by design. Prefer the
    // per-asset checksum its own build job publishes; fall back to a
    // combined SHA256SUMS so releases made before that change still work.
    let sums_url = url_of(&format!("{asset_name}.sha256"))
        .or_else(|| url_of("SHA256SUMS"))
        .filter(|u| host_allowed(u))?;
    Some(Plan {
        version: tag.to_string(),
        notes_url: body
            .get("html_url")
            .and_then(|v| v.as_str())
            .unwrap_or("https://github.com/Goo6i/khaloni-poe2/releases/latest")
            .to_string(),
        asset_url,
        asset_name,
        sums_url,
    })
}

/// Asks GitHub for the latest release; `Ok(None)` when this build is
/// current, when the release lacks the pieces needed to install safely, or
/// when the tag is not parseable.
pub fn check() -> anyhow::Result<Option<Update>> {
    let client = http()?;
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let body: serde_json::Value = client.get(&url).send()?.error_for_status()?.json()?;
    let Some(plan) = plan_from_release(&body, CURRENT) else {
        return Ok(None);
    };
    let sums = client.get(&plan.sums_url).send()?.error_for_status()?.text()?;
    let Some(sha256) = sha_from_file(&sums, &plan.asset_name) else {
        return Ok(None);
    };
    Ok(Some(Update {
        version: plan.version,
        notes_url: plan.notes_url,
        asset_url: plan.asset_url,
        asset_name: plan.asset_name,
        sha256,
    }))
}

/// Runs `check` on a background thread and reports a found update. Silent
/// on any failure: an offline session must not nag.
pub fn spawn_check(tx: std::sync::mpsc::Sender<Update>) {
    std::thread::spawn(move || match check() {
        Ok(Some(u)) => {
            eprintln!("update available: {} (running {CURRENT})", u.version);
            let _ = tx.send(u);
        }
        Ok(None) => {}
        Err(e) => eprintln!("update check failed: {e}"),
    });
}

/// Downloads, verifies, and swaps in the new executable. Returns the path
/// that was replaced. The running process keeps executing the old image;
/// the update takes effect on the next launch, which the caller must say.
pub fn apply(update: &Update) -> anyhow::Result<PathBuf> {
    anyhow::ensure!(
        !is_dev_build(),
        "this is a cargo build, not an installed release; update skipped"
    );
    anyhow::ensure!(host_allowed(&update.asset_url), "refusing a non-GitHub download URL");
    let exe = std::env::current_exe()?;
    let dir = exe.parent().ok_or_else(|| anyhow::anyhow!("executable has no parent dir"))?;

    let client = http()?;
    let mut resp = client.get(&update.asset_url).send()?.error_for_status()?;
    if let Some(len) = resp.content_length() {
        anyhow::ensure!(len <= MAX_DOWNLOAD, "refusing an implausibly large download ({len} bytes)");
    }
    let mut bytes = Vec::new();
    // Cap the read too: content-length is a claim, not a guarantee.
    resp.by_ref().take(MAX_DOWNLOAD).read_to_end(&mut bytes)?;

    let got = hex(&Sha256::digest(&bytes));
    anyhow::ensure!(
        got == update.sha256,
        "checksum mismatch (expected {}, got {got}); nothing installed",
        update.sha256
    );

    // Staged in the destination directory so the final rename is atomic on
    // the same filesystem.
    let staged = dir.join(".khaloni-poe2.new");
    std::fs::write(&staged, &bytes)?;
    set_executable(&staged)?;

    // A running executable cannot be overwritten on either platform, but it
    // CAN be renamed out of the way; the .old file is swept on next start.
    let backup = dir.join(".khaloni-poe2.old");
    let _ = std::fs::remove_file(&backup);
    std::fs::rename(&exe, &backup)?;
    if let Err(e) = std::fs::rename(&staged, &exe) {
        // Put the working binary back rather than leaving nothing behind.
        let _ = std::fs::rename(&backup, &exe);
        let _ = std::fs::remove_file(&staged);
        return Err(anyhow::anyhow!("install failed, original restored: {e}"));
    }
    Ok(exe)
}

/// Removes the previous binary left behind by `apply`. Best-effort: on
/// Windows the file may still be locked by an exiting process.
pub fn cleanup_backup() {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let _ = std::fs::remove_file(dir.join(".khaloni-poe2.old"));
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(unix)]
fn set_executable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> std::io::Result<()> {
    Ok(()) // Windows has no executable bit
}
