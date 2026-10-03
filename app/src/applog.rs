//! A log file that outlives the session. The overlay reports everything
//! through `eprintln!`, and a trade ban is only explainable from the request
//! lines that led up to it, so stderr is teed into `overlay.log` in the
//! cache dir. The file is rotated by size and the oldest copies dropped;
//! nothing here can fill a disk.
//!
//! Only the file descriptor is redirected, not the macros: whatever a
//! dependency prints to stderr is caught the same way.

use std::io;
use std::path::Path;

/// A log file is rotated once it would grow past this.
pub const MAX_BYTES: u64 = 5 * 1024 * 1024;
/// Files kept: `overlay.log` and the `KEEP - 1` rotated ones before it.
pub const KEEP: usize = 4;

/// Tees stderr into `<cache_dir>/overlay.log`, rotating at [`MAX_BYTES`]
/// and keeping [`KEEP`] files.
pub fn install(cache_dir: &Path) -> io::Result<()> {
    install_to(cache_dir, MAX_BYTES, KEEP)
}

/// [`install`] with the limits spelled out.
///
/// Writes to stderr keep going to the original stderr as well. Should the
/// file stop being writable the tee carries on to stderr alone and tries
/// the file again on the next line; the writer never waits on the file.
#[cfg(unix)]
pub fn install_to(cache_dir: &Path, max_bytes: u64, keep: usize) -> io::Result<()> {
    use std::os::fd::{AsRawFd, BorrowedFd, OwnedFd};

    std::fs::create_dir_all(cache_dir)?;
    let log = RotatingFile::open(cache_dir, max_bytes, keep)?;
    // The original stderr, held open past the redirection so the tee can
    // still reach the terminal (or the service manager's journal).
    // Safe: descriptor 2 is open for the life of the process.
    let original = unsafe { BorrowedFd::borrow_raw(2) }.try_clone_to_owned()?;
    let original = std::fs::File::from(original);
    let (reader, writer) = io::pipe()?;
    // Everything that reaches descriptor 2 from here on goes down the pipe.
    // Safe: both descriptors are open, and the writer is closed right after
    // so the pipe's only writing end is descriptor 2 itself.
    if unsafe { dup2(writer.as_raw_fd(), 2) } < 0 {
        return Err(io::Error::last_os_error());
    }
    drop(writer);
    let reader = std::fs::File::from(OwnedFd::from(reader));
    std::thread::Builder::new()
        .name("stderr-tee".into())
        .spawn(move || tee(reader, original, log))?;
    Ok(())
}

/// Windows has no `dup2` in std and the overlay does not ship there yet,
/// so the log file is not written; stderr is left as it is.
#[cfg(not(unix))]
pub fn install_to(_cache_dir: &Path, _max_bytes: u64, _keep: usize) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "the stderr log file is not written on this platform"))
}

#[cfg(unix)]
extern "C" {
    fn dup2(src: std::os::raw::c_int, dst: std::os::raw::c_int) -> std::os::raw::c_int;
}

/// Copies the pipe to the original stderr and the log file, line by line,
/// until the last writer is gone. Nothing here may print: this thread's
/// output would go down its own pipe.
#[cfg(unix)]
fn tee(reader: std::fs::File, mut original: std::fs::File, mut log: RotatingFile) {
    use std::io::{BufRead, Write};

    let mut reader = io::BufReader::new(reader);
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let _ = original.write_all(&line);
        log.write_line(&line);
    }
}

/// `overlay.log`, reopened as `overlay.1.log`, `overlay.2.log`, ... as it
/// fills; the copy past `keep` is deleted.
#[cfg(unix)]
struct RotatingFile {
    dir: std::path::PathBuf,
    max_bytes: u64,
    keep: usize,
    file: Option<std::fs::File>,
    len: u64,
}

#[cfg(unix)]
impl RotatingFile {
    fn open(dir: &Path, max_bytes: u64, keep: usize) -> io::Result<Self> {
        let mut this = RotatingFile { dir: dir.to_path_buf(), max_bytes, keep, file: None, len: 0 };
        this.reopen()?;
        Ok(this)
    }

    fn path(&self, index: usize) -> std::path::PathBuf {
        match index {
            0 => self.dir.join("overlay.log"),
            n => self.dir.join(format!("overlay.{n}.log")),
        }
    }

    /// Opens (or appends to) `overlay.log`, picking up where a previous run
    /// left it so a restart does not start a fresh file each time.
    fn reopen(&mut self) -> io::Result<()> {
        let file = std::fs::OpenOptions::new().create(true).append(true).open(self.path(0))?;
        self.len = file.metadata().map(|m| m.len()).unwrap_or(0);
        self.file = Some(file);
        Ok(())
    }

    /// Shifts every kept file one index up, dropping the oldest, and starts
    /// a new `overlay.log`.
    fn rotate(&mut self) -> io::Result<()> {
        self.file = None;
        let _ = std::fs::remove_file(self.path(self.keep.max(1) - 1));
        for i in (1..self.keep).rev() {
            let _ = std::fs::rename(self.path(i - 1), self.path(i));
        }
        // With a single file kept there is nothing to shift into; the file
        // is simply started over.
        if self.keep <= 1 {
            let _ = std::fs::remove_file(self.path(0));
        }
        self.reopen()
    }

    /// Appends one line, rotating first when it would not fit. A line
    /// longer than the whole limit still goes in, on its own.
    fn write_line(&mut self, line: &[u8]) {
        use std::io::Write;

        if self.file.is_none() && self.reopen().is_err() {
            return;
        }
        if self.len > 0 && self.len + line.len() as u64 > self.max_bytes && self.rotate().is_err() {
            return;
        }
        let Some(file) = self.file.as_mut() else { return };
        match file.write_all(line) {
            Ok(()) => self.len += line.len() as u64,
            // The next line tries to open the file afresh.
            Err(_) => self.file = None,
        }
    }
}
