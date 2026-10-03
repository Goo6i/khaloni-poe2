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
/// Whole-request budget for the small API and checksum requests.
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
/// The binary download is bounded differently: by how long the connection
/// may take to open and how long it may then go without delivering a byte.
/// A whole-request timeout cannot fit both a fast link and a slow one - the
/// 60s above cut off a 40 MB download on anything under ~5 Mbit/s, every
/// time, while still letting a dead connection hang for a minute.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);
const IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// Backstop so an abandoned reader thread cannot outlive a trickling server
/// forever.
const DOWNLOAD_CEILING: std::time::Duration = std::time::Duration::from_secs(60 * 60);

const STAGED: &str = ".khaloni-poe2.new";
const BACKUP: &str = ".khaloni-poe2.old";
/// Left by `apply`, consumed by the first start after it: while it exists
/// the backup is the only way back from an update nobody has run yet.
const PENDING: &str = ".khaloni-poe2.pending";

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

/// GETs `url` into memory, refusing more than `max_bytes`, giving up when
/// the connection takes longer than `connect` to open or stalls for longer
/// than `idle` between chunks. The body is read on its own thread and
/// handed over in chunks, because the blocking client offers no per-read
/// timeout: the wait for each chunk is what `idle` bounds.
pub fn download(
    url: &str,
    max_bytes: u64,
    connect: std::time::Duration,
    idle: std::time::Duration,
) -> anyhow::Result<Vec<u8>> {
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(connect)
        .timeout(DOWNLOAD_CEILING)
        .user_agent(concat!("khaloni-poe2/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let (tx, rx) = std::sync::mpsc::channel::<anyhow::Result<Vec<u8>>>();
    let url = url.to_string();
    std::thread::spawn(move || {
        let run = || -> anyhow::Result<()> {
            let mut resp = client.get(&url).send()?.error_for_status()?;
            if let Some(len) = resp.content_length() {
                anyhow::ensure!(len <= max_bytes, "refusing an implausibly large download ({len} bytes)");
            }
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let n = resp.read(&mut buf)?;
                // A closed receiver means the caller gave up: stop reading.
                if n == 0 || tx.send(Ok(buf[..n].to_vec())).is_err() {
                    return Ok(());
                }
            }
        };
        if let Err(e) = run() {
            let _ = tx.send(Err(e));
        }
    });
    let mut bytes = Vec::new();
    loop {
        // The first wait also covers connecting and the response headers.
        let wait = if bytes.is_empty() { connect + idle } else { idle };
        match rx.recv_timeout(wait) {
            Ok(Ok(chunk)) => {
                bytes.extend_from_slice(&chunk);
                // Cap the read too: content-length is a claim, not a guarantee.
                anyhow::ensure!(bytes.len() as u64 <= max_bytes, "download exceeds {max_bytes} bytes");
            }
            Ok(Err(e)) => return Err(e),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Ok(bytes),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                anyhow::bail!("download stalled: no data for {}s", wait.as_secs())
            }
        }
    }
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

    let bytes = download(&update.asset_url, MAX_DOWNLOAD, CONNECT_TIMEOUT, IDLE_TIMEOUT)?;

    let got = hex(&Sha256::digest(&bytes));
    anyhow::ensure!(
        got == update.sha256,
        "checksum mismatch (expected {}, got {got}); nothing installed",
        update.sha256
    );

    // Staged in the destination directory so the final rename is atomic on
    // the same filesystem, and synced so that rename cannot install a file
    // whose contents never reached the disk.
    let staged = dir.join(STAGED);
    write_synced(&staged, &bytes)?;
    set_executable(&staged)?;

    if let Err(e) = install(&staged, &exe, &dir.join(BACKUP)) {
        let _ = std::fs::remove_file(&staged);
        return Err(anyhow::anyhow!("install failed, original left in place: {e}"));
    }
    // Best-effort: without the marker the backup is merely swept one start
    // earlier.
    let _ = std::fs::write(dir.join(PENDING), &update.version);
    sync_dir(dir);
    Ok(exe)
}

fn write_synced(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::File::create(path)?;
    f.write_all(bytes)?;
    f.sync_all()
}

/// Makes the renames in `dir` durable. Unix only: Windows cannot open a
/// directory this way and journals its renames itself.
fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}

/// Puts `staged` in place of `exe`, keeping the previous binary at
/// `backup`. On Unix the backup is a second link to the old file and the
/// new one is renamed straight over `exe`: one atomic step, with a runnable
/// binary at `exe` before it, after it, and if the machine dies during it.
/// The process already running keeps its old image either way. Moving the
/// old binary aside first and the new one in second - the earlier order -
/// left no executable at all between the two renames.
#[cfg(unix)]
pub fn install(staged: &Path, exe: &Path, backup: &Path) -> std::io::Result<()> {
    let _ = std::fs::remove_file(backup);
    // A link costs nothing and cannot be half-written; a filesystem without
    // hard links gets a copy instead.
    if std::fs::hard_link(exe, backup).is_err() {
        std::fs::copy(exe, backup)?;
    }
    std::fs::rename(staged, exe)?;
    if let Some(dir) = exe.parent() {
        sync_dir(dir);
    }
    Ok(())
}

/// Windows cannot rename over a running executable, only rename it away,
/// so the swap takes two steps; a failed second step puts the original
/// back rather than leaving nothing behind.
#[cfg(not(unix))]
pub fn install(staged: &Path, exe: &Path, backup: &Path) -> std::io::Result<()> {
    let _ = std::fs::remove_file(backup);
    std::fs::rename(exe, backup)?;
    if let Err(e) = std::fs::rename(staged, exe) {
        let _ = std::fs::rename(backup, exe);
        return Err(e);
    }
    Ok(())
}

/// Sweeps the previous binary left behind by `apply`, called at startup.
/// The first start after an update only consumes the pending marker: the
/// backup stays until the new version has been started once and is started
/// again, so a release that does not come up can still be undone by moving
/// `.khaloni-poe2.old` back over the executable. Best-effort: on Windows
/// the file may still be locked by an exiting process.
pub fn cleanup_backup() {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            cleanup_backup_in(dir);
        }
    }
}

/// [`cleanup_backup`] for the install directory `dir`.
pub fn cleanup_backup_in(dir: &Path) {
    let pending = dir.join(PENDING);
    if pending.exists() {
        let _ = std::fs::remove_file(pending);
    } else {
        let _ = std::fs::remove_file(dir.join(BACKUP));
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
