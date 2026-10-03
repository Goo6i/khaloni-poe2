//! Keeps the listing cards price checks fetched, per league, for the observed craft model.
//!
//! Each league has its own file of one listing per line under the cache
//! directory. A price check's listings are appended as they arrive; the
//! file is rewritten whole, atomically, only when opening finds it untidy
//! (a line cut short by a crash, a repeat, a line past the cap) or when
//! appends have run a quarter past the cap. Rewriting a few megabytes on
//! every price check would cost more than the listings are worth, and the
//! slack bounds the file at a quarter over the cap.

use std::io::Write;
use std::path::{Path, PathBuf};

use khaloni_poe2_core::craft::data::CraftData;
use khaloni_poe2_core::craft::model::Model;
use khaloni_poe2_core::craft::observed::{listing_entries, Observed, Unjoined, MAX_LISTINGS, MIN_LISTINGS};
use serde_json::Value;

/// The line the panel shows beside every observed figure.
pub const BIAS_NOTE: &str = "biased toward what sellers list";

/// Where the league files live: `observed` under the app's cache directory.
pub fn default_dir() -> PathBuf {
    directories::ProjectDirs::from("", "", "khaloni-poe2")
        .map(|d| d.cache_dir().to_path_buf())
        .unwrap_or_else(std::env::temp_dir)
        .join("observed")
}

fn file_name(league: &str) -> String {
    let safe: String = league.chars().map(|c| if c == '/' || c == '\\' { '_' } else { c }).collect();
    format!("{safe}.jsonl")
}

/// What one price check's listings added.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FetchRecord {
    /// Listings new to the store.
    pub recorded: usize,
    /// Listings the store already held.
    pub repeated: usize,
    /// Entries with nothing to record: vanished listings, normal, unique
    /// and unidentified items, bases the data does not know.
    pub not_read: usize,
    /// Rolled modifiers of the recorded listings that joined no entry.
    pub unjoined: Vec<Unjoined>,
    /// Crafted and desecrated modifiers of the recorded listings, left out
    /// because they were chosen, not rolled.
    pub chosen: usize,
}

/// The observed store of the current league and its file.
#[derive(Debug)]
pub struct ObservedStore {
    dir: PathBuf,
    observed: Observed,
    /// Lines in the league's file, kept or since dropped by the cap.
    lines_in_file: usize,
    cap: usize,
}

impl ObservedStore {
    pub fn open(dir: &Path, league: &str) -> std::io::Result<ObservedStore> {
        ObservedStore::with_limits(dir, league, MIN_LISTINGS, MAX_LISTINGS)
    }

    pub fn with_limits(dir: &Path, league: &str, min_listings: u32, cap: usize) -> std::io::Result<ObservedStore> {
        let observed = Observed::with_limits(league, min_listings, cap);
        let mut store = ObservedStore { dir: dir.to_path_buf(), observed, lines_in_file: 0, cap: cap.max(1) };
        store.load()?;
        Ok(store)
    }

    pub fn path(&self) -> PathBuf {
        self.dir.join(file_name(self.observed.league()))
    }

    pub fn observed(&self) -> &Observed {
        &self.observed
    }

    /// The observed model of `class`, once the minimum sample is kept.
    pub fn model(&self, class: &str) -> Option<Model> {
        self.observed.model(class)
    }

    pub fn set_min_listings(&mut self, n: u32) {
        self.observed.set_min_listings(n);
    }

    /// Moves to `league`'s file. The previous league's file stays as it is.
    pub fn switch_league(&mut self, league: &str) -> std::io::Result<()> {
        if self.observed.switch_league(league) {
            self.lines_in_file = 0;
            self.load()?;
        }
        Ok(())
    }

    /// Reads the league's file into the emptied store, and rewrites the
    /// file when what it holds is not exactly the store's lines.
    fn load(&mut self) -> std::io::Result<()> {
        let text = match std::fs::read_to_string(self.path()) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        let report = self.observed.load_jsonl(&text);
        let lines = report.kept + report.other_league + report.unreadable + report.repeated;
        // A file not ending in a newline ends in a write cut short: the
        // next append would join that fragment and be lost with it.
        let untidy = (!text.is_empty() && !text.ends_with('\n')) || lines != self.observed.len();
        if untidy {
            self.rewrite()?;
        } else {
            self.lines_in_file = lines;
        }
        Ok(())
    }

    fn rewrite(&mut self) -> std::io::Result<()> {
        khaloni_poe2_core::ninja::write_cache_atomic(&self.path(), self.observed.jsonl().as_bytes())?;
        self.lines_in_file = self.observed.len();
        Ok(())
    }

    /// Records the listings one price check fetched (`None` where the API
    /// sent null), each joined to the mod database, and appends the new
    /// ones to the league's file.
    pub fn record_fetch(&mut self, raw: &[Option<Value>], data: &CraftData) -> std::io::Result<FetchRecord> {
        let mut got = FetchRecord::default();
        let mut lines = String::new();
        for entry in raw {
            let Some(Ok(read)) = entry.as_ref().map(|v| listing_entries(v, data)) else {
                got.not_read += 1;
                continue;
            };
            let line = Observed::line(self.observed.league(), &read.listing);
            if !self.observed.record(read.listing) {
                got.repeated += 1;
                continue;
            }
            got.recorded += 1;
            got.chosen += read.chosen;
            got.unjoined.extend(read.unjoined);
            lines.push_str(&line);
            lines.push('\n');
        }
        if got.recorded == 0 {
            return Ok(got);
        }
        std::fs::create_dir_all(&self.dir)?;
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(self.path())?;
        f.write_all(lines.as_bytes())?;
        f.sync_all()?;
        self.lines_in_file += got.recorded;
        if self.lines_in_file > self.cap + self.cap / 4 {
            self.rewrite()?;
        }
        Ok(got)
    }
}
