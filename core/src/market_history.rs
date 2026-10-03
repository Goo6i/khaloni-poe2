//! The market history log: one line of JSON per price refresh, appended to
//! `market-history/<league>.jsonl`, so the overlay can answer for windows
//! poe.ninja does not publish (24 hours, 3 days) and so a few days of
//! records can show what period the traded-volume field covers.
//!
//! The write side only ever appends (and trims the oldest lines at the size
//! cap); everything else happens when reading. A bucket without a record is
//! `None`: a gap drawn as a gap, never bridged from its neighbours. Prices
//! are stored in divines with the day's exalted and chaos rates beside
//! them, so an old price converts at the rates of its own day.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::market::{Kind, Source};
use crate::value::Unit;

/// The file is cut back to this size, oldest records first.
pub const MAX_BYTES: u64 = 50 * 1024 * 1024;
/// A window's change is given only from this many records inside it.
pub const MIN_WINDOW_RECORDS: usize = 3;
/// How far from the window's start its first record may lie, as a share of
/// the window: a "24 h" change measured over 14 hours is not one.
const WINDOW_START_TOLERANCE: f64 = 0.25;
/// The newest record must be this recent for a window to end "now".
const WINDOW_END_SECS: i64 = 3 * 3600;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rates {
    pub exalted: f64,
    pub chaos: f64,
}

/// `[category, id, price in divines, traded volume or listing count]`.
pub type Entry = (String, String, f64, Option<f64>);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    /// When the record was written, unix seconds.
    pub t: i64,
    /// When the figures were fetched: older than `t` when the refresh was
    /// served from the disk cache.
    pub source_t: i64,
    pub rates: Rates,
    pub items: Vec<Entry>,
}

impl Record {
    pub fn from_source(source: &Source, t: i64, source_t: i64) -> Record {
        Record {
            t,
            source_t,
            rates: Rates { exalted: source.exalted_rate, chaos: source.chaos_rate },
            items: source
                .items
                .iter()
                .map(|it| {
                    let depth = match it.kind {
                        Kind::Exchange => it.volume_div,
                        Kind::Listed => it.listings.map(f64::from),
                    };
                    (it.category.clone(), it.id.clone(), it.price_div, depth)
                })
                .collect(),
        }
    }

    /// A hash of what was priced, not of when: two refreshes that brought
    /// the same figures hash alike. FNV-1a, because the std hasher may
    /// change between Rust releases and the hash is compared with one read
    /// back from a file an older build wrote.
    pub fn payload_hash(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut eat = |bytes: &[u8]| {
            for b in bytes.iter().chain(&[0u8]) {
                h ^= u64::from(*b);
                h = h.wrapping_mul(0x0000_0100_0000_01b3);
            }
        };
        eat(&self.rates.exalted.to_bits().to_le_bytes());
        eat(&self.rates.chaos.to_bits().to_le_bytes());
        for (category, id, price, depth) in &self.items {
            eat(category.as_bytes());
            eat(id.as_bytes());
            eat(&price.to_bits().to_le_bytes());
            eat(&depth.map(f64::to_bits).unwrap_or(u64::MAX).to_le_bytes());
        }
        h
    }

    fn entry(&self, category: &str, id: &str) -> Option<&Entry> {
        self.items.iter().find(|(c, i, ..)| c == category && i == id)
    }

    /// The item's price in `unit`, converted at THIS record's rates.
    /// `None` when the record lacks the item or the rate.
    pub fn price_in(&self, category: &str, id: &str, unit: Unit) -> Option<f64> {
        let price = self.entry(category, id)?.2;
        let rate = match unit {
            Unit::Divine => 1.0,
            Unit::Exalted => self.rates.exalted,
            Unit::Chaos => self.rates.chaos,
        };
        (rate > 0.0).then_some(price * rate)
    }

    /// Traded volume (exchange items) or listing count (listed items).
    pub fn depth(&self, category: &str, id: &str) -> Option<f64> {
        self.entry(category, id)?.3
    }
}

fn file_name(league: &str) -> String {
    let safe: String = league.chars().map(|c| if c == '/' || c == '\\' { '_' } else { c }).collect();
    format!("{safe}.jsonl")
}

/// The log of one league at a time. `dir` is the `market-history` directory
/// itself; the app decides where that lives.
#[derive(Debug)]
pub struct HistoryLog {
    dir: PathBuf,
    league: String,
    max_bytes: u64,
    /// Payload hash of the newest record in the file; `None` until read.
    last_hash: Option<Option<u64>>,
}

impl HistoryLog {
    pub fn open(dir: &Path, league: &str) -> HistoryLog {
        HistoryLog::with_cap(dir, league, MAX_BYTES)
    }

    pub fn with_cap(dir: &Path, league: &str, max_bytes: u64) -> HistoryLog {
        HistoryLog { dir: dir.to_path_buf(), league: league.to_string(), max_bytes, last_hash: None }
    }

    pub fn league(&self) -> &str {
        &self.league
    }

    pub fn path(&self) -> PathBuf {
        self.dir.join(file_name(&self.league))
    }

    /// Moves the log to `league`'s file. The previous league's file stays
    /// as it is. When the new league's start is known, records from before
    /// it are dropped from its file: a league name can be reused, and a
    /// price from the earlier league of that name is not this one's
    /// history.
    pub fn switch_league(&mut self, league: &str, league_start: Option<i64>) -> std::io::Result<()> {
        if self.league != league {
            self.league = league.to_string();
            self.last_hash = None;
        }
        let Some(start) = league_start else { return Ok(()) };
        let lines = self.lines()?;
        let kept: Vec<&String> = lines
            .iter()
            .filter(|l| serde_json::from_str::<Record>(l).map(|r| r.t >= start).unwrap_or(false))
            .collect();
        if kept.len() != lines.len() {
            self.rewrite(&kept)?;
            self.last_hash = None;
        }
        Ok(())
    }

    fn lines(&self) -> std::io::Result<Vec<String>> {
        match std::fs::File::open(self.path()) {
            Ok(f) => BufReader::new(f).lines().filter(|l| l.as_ref().map_or(true, |l| !l.trim().is_empty())).collect(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    fn rewrite(&self, lines: &[&String]) -> std::io::Result<()> {
        let mut body = String::with_capacity(lines.iter().map(|l| l.len() + 1).sum());
        for l in lines {
            body.push_str(l);
            body.push('\n');
        }
        crate::ninja::write_cache_atomic(&self.path(), body.as_bytes())
    }

    fn newest_hash(&mut self) -> std::io::Result<Option<u64>> {
        if let Some(known) = self.last_hash {
            return Ok(known);
        }
        let newest = self
            .lines()?
            .last()
            .and_then(|l| serde_json::from_str::<Record>(l).ok())
            .map(|r| r.payload_hash());
        self.last_hash = Some(newest);
        Ok(newest)
    }

    /// Appends `record` unless it prices exactly what the newest record
    /// does: poe.ninja recomputes about hourly and the overlay refreshes
    /// every few minutes, so most refreshes bring nothing new. True when a
    /// line was written.
    pub fn append(&mut self, record: &Record) -> std::io::Result<bool> {
        let hash = record.payload_hash();
        if self.newest_hash()? == Some(hash) {
            return Ok(false);
        }
        std::fs::create_dir_all(&self.dir)?;
        let mut line = serde_json::to_string(record).map_err(std::io::Error::other)?;
        line.push('\n');
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(self.path())?;
        f.write_all(line.as_bytes())?;
        f.sync_all()?;
        self.last_hash = Some(Some(hash));
        self.enforce_cap()?;
        Ok(true)
    }

    /// Drops the oldest records until the file fits the cap. The newest
    /// record always stays, whatever its size.
    fn enforce_cap(&self) -> std::io::Result<()> {
        if std::fs::metadata(self.path())?.len() <= self.max_bytes {
            return Ok(());
        }
        let lines = self.lines()?;
        let mut size: u64 = 0;
        let mut keep_from = lines.len();
        for (i, l) in lines.iter().enumerate().rev() {
            size += l.len() as u64 + 1;
            if size > self.max_bytes && keep_from < lines.len() {
                break;
            }
            keep_from = i;
        }
        self.rewrite(&lines[keep_from..].iter().collect::<Vec<_>>())
    }

    /// Every readable record, oldest first. A line that does not parse (a
    /// write cut short by a crash) is skipped, not an error.
    pub fn read(&self) -> Vec<Record> {
        let mut records: Vec<Record> = self
            .lines()
            .unwrap_or_default()
            .iter()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        records.sort_by_key(|r: &Record| r.t);
        records
    }

    /// When this install first recorded the league.
    pub fn first_record_time(&self) -> Option<i64> {
        let f = std::fs::File::open(self.path()).ok()?;
        let first = BufReader::new(f).lines().map_while(Result::ok).find(|l| !l.trim().is_empty())?;
        serde_json::from_str::<Record>(&first).ok().map(|r| r.t)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bucket {
    Hour,
    Day,
}

impl Bucket {
    pub fn secs(self) -> i64 {
        match self {
            Bucket::Hour => 3600,
            Bucket::Day => 86_400,
        }
    }
}

/// What a series reads from each record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    /// The price, in this currency, at the record's own rates.
    Price(Unit),
    /// Traded volume or listing count.
    Depth,
}

/// One value per bucket from `from` (inclusive) to `to` (exclusive): the
/// newest record inside the bucket. A bucket no record fell into is `None`.
/// Nothing is carried forward and nothing is interpolated, so a day the
/// overlay was not running shows as a day without data.
pub fn series(
    records: &[Record],
    category: &str,
    id: &str,
    field: Field,
    bucket: Bucket,
    from: i64,
    to: i64,
) -> Vec<Option<f64>> {
    let len = bucket.secs();
    let count = ((to - from).max(0) + len - 1) / len;
    let mut out = vec![None; count as usize];
    let mut newest = vec![i64::MIN; count as usize];
    for r in records.iter().filter(|r| r.t >= from && r.t < to) {
        let slot = ((r.t - from) / len) as usize;
        let value = match field {
            Field::Price(unit) => r.price_in(category, id, unit),
            Field::Depth => r.depth(category, id),
        };
        if let Some(v) = value.filter(|v| v.is_finite()) {
            if r.t >= newest[slot] {
                newest[slot] = r.t;
                out[slot] = Some(v);
            }
        }
    }
    out
}

/// A price change over a window the source does not publish.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowChange {
    /// Percent, in divines.
    pub pct: f64,
    /// Records the item appears in from the window's first to its last:
    /// shown beside the figure, so a change resting on three records reads
    /// differently from one resting on seventy.
    pub records: usize,
}

/// The item's change over the last `window_secs`, or `None` when the log
/// cannot support it: no record near the window's start, no recent record
/// at its end, or fewer than [`MIN_WINDOW_RECORDS`] in between.
pub fn window_change(
    records: &[Record],
    category: &str,
    id: &str,
    now: i64,
    window_secs: i64,
) -> Option<WindowChange> {
    let with_item: Vec<(i64, f64)> = records
        .iter()
        .filter_map(|r| Some((r.t, r.price_in(category, id, Unit::Divine)?)))
        .filter(|(t, p)| *t <= now && p.is_finite() && *p > 0.0)
        .collect();
    let &(end_t, end_price) = with_item.iter().max_by_key(|(t, _)| *t)?;
    if now - end_t > WINDOW_END_SECS {
        return None;
    }
    let want = now - window_secs;
    let &(start_t, start_price) = with_item.iter().min_by_key(|(t, _)| (t - want).abs())?;
    if (start_t - want).abs() as f64 > window_secs as f64 * WINDOW_START_TOLERANCE {
        return None;
    }
    let inside = with_item.iter().filter(|(t, _)| *t >= start_t && *t <= end_t).count();
    if inside < MIN_WINDOW_RECORDS {
        return None;
    }
    Some(WindowChange { pct: (end_price / start_price - 1.0) * 100.0, records: inside })
}
