//! The player's own runs, from the game log to the words on the panel.
//!
//! [`Hub`] is what the threads share: the log tail feeds it lines, the
//! wealth worker asks it whether loot may just have been stashed, and a
//! small worker of its own turns runs and stash snapshots into a [`View`]
//! whenever either changed, so the main loop only ever picks up an `Arc`.
//!
//! [`View`] is the pure part: every string of the "My runs" tab, built
//! from `core::income::Summary`. A cell is empty because the summary had
//! nothing for it; no figure is made up here.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use khaloni_poe2_core::income::{
    self, date_text, hours_text, mechanics_text, missing_text, not_enough_text, rate_text, sample_text, seen_text,
    signed_div_text, time_text, MechanicIncome, StashAccess, Summary,
};
use khaloni_poe2_core::runs::{Place, Run, Totals, Tracker, COLD_READ_BYTES};

/// Blocks listed on the overlay tab, newest first.
pub const BLOCKS_SHOWN: usize = 8;

#[derive(Debug, Clone, PartialEq)]
pub struct MechanicRow {
    pub name: String,
    pub seen: String,
    /// The rate with its sample, or how far the sample is from one; empty
    /// without stash access.
    pub income: String,
    pub has_rate: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BlockRow {
    pub time: String,
    pub maps: String,
    pub hours: String,
    pub mechanics: String,
    /// "pending" while a vanished stack waits for the next snapshot.
    pub income: String,
    pub income_sign: i8,
    pub revaluation: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct View {
    /// (label, maps, map hours): today, then everything the log read holds.
    pub totals: Vec<[String; 3]>,
    /// Maps that are in no hour figure, when there are any.
    pub totals_note: Option<String>,
    /// The overall rate; `None` when there is none, and then `income_note`
    /// says why.
    pub overall: Option<(String, String)>,
    pub income_note: Option<String>,
    pub mechanics: Vec<MechanicRow>,
    pub blocks: Vec<BlockRow>,
    /// Market category -> "yours: ...", only where a rate exists.
    pub yours: BTreeMap<String, String>,
    /// What the income figure is; only with stash access.
    pub footer: Option<String>,
}

pub const NO_RUNS: &str = "no finished map in the part of the game log that was read";
pub const READING_LOG: &str = "reading the game log";
pub const LOG_MISSING: &str = "game log not found: set client_log_path in config.toml";
pub const SEEN_NOTE: &str = "the log names a mechanic only when the engine complains about its monsters: a floor";

fn sign(x: f64) -> i8 {
    if x > 0.0 {
        1
    } else if x < 0.0 {
        -1
    } else {
        0
    }
}

impl View {
    pub fn build(s: &Summary) -> View {
        let totals_row = |label: String, t: &Totals| [label, t.maps.to_string(), hours_text(t.map_seconds)];
        let mut totals = vec![totals_row("today".into(), &s.today)];
        if let Some(since) = s.since {
            totals.push(totals_row(format!("since {}", date_text(since)), &s.all));
        }
        let totals_note = (s.all.maps_unknown_duration > 0).then(|| {
            format!(
                "{} of {} maps have no known duration (client restart, or over an hour inside): in no hour figure",
                s.all.maps_unknown_duration, s.all.maps
            )
        });
        let ready = s.access == StashAccess::Ready;
        let income_note = match (&s.access, &s.overall) {
            (StashAccess::Missing(what), _) => Some(missing_text(what)),
            (StashAccess::Ready, Some(_)) => None,
            (StashAccess::Ready, None) if s.blocks.is_empty() => Some(income::NO_BLOCKS.to_string()),
            (StashAccess::Ready, None) => Some(income::NO_COMPLETE_BLOCKS.to_string()),
        };
        let mut yours = BTreeMap::new();
        let mechanics = s
            .mechanics
            .iter()
            .map(|m| {
                let (income, has_rate) = match m.income {
                    None => (String::new(), false),
                    Some(MechanicIncome::NotEnough { blocks, maps }) => (not_enough_text(blocks, maps), false),
                    Some(MechanicIncome::Rate(r)) => {
                        yours.insert(
                            m.mechanic.name().to_string(),
                            format!("yours: {} ({} maps)", rate_text(&r), r.sample.maps),
                        );
                        (format!("{}, {}", rate_text(&r), sample_text(&r.sample)), true)
                    }
                };
                MechanicRow {
                    name: m.mechanic.name().to_string(),
                    seen: seen_text(m.seen_in, s.all.maps),
                    income,
                    has_rate,
                }
            })
            .collect();
        let blocks = s
            .blocks
            .iter()
            .rev()
            .take(BLOCKS_SHOWN)
            .map(|b| BlockRow {
                time: time_text(b.to),
                maps: b.maps.to_string(),
                hours: match (b.maps, b.map_seconds) {
                    (0, _) => String::new(),
                    (_, Some(secs)) => hours_text(secs),
                    (_, None) => "unknown".to_string(),
                },
                mechanics: mechanics_text(&b.mechanics),
                income: b.figures.map(|f| signed_div_text(f.income_div)).unwrap_or_else(|| income::PENDING.into()),
                income_sign: b.figures.map(|f| sign(f.income_div)).unwrap_or(0),
                revaluation: b.figures.map(|f| signed_div_text(f.revaluation_div)).unwrap_or_default(),
            })
            .collect();
        View {
            totals,
            totals_note,
            overall: s.overall.map(|r| (rate_text(&r), sample_text(&r.sample))),
            income_note,
            mechanics,
            blocks,
            yours,
            footer: ready.then(|| income::INCOME_NOTE.to_string()),
        }
    }

    pub fn has_runs(&self) -> bool {
        self.totals.len() > 1
    }
}

/// What the panel gets: the view, or why there is none.
#[derive(Debug, Clone, PartialEq)]
pub enum Shown {
    Reading,
    LogMissing,
    Runs(Arc<View>),
}

#[derive(Default)]
struct State {
    tracker: Tracker,
    /// Seconds the log's clock runs ahead of UTC, to the quarter hour.
    offset: Option<i64>,
    hideout_since: Option<Instant>,
    /// `Tracker::intervals_closed` when the last stash walk began.
    snapshot_mark: u64,
    log_read: bool,
    log_missing: bool,
}

#[derive(Default)]
pub struct Hub {
    state: Mutex<State>,
    shown: Mutex<Option<Arc<View>>>,
}

/// The log's clock against UTC, from one moment known on both: rounded to
/// the quarter hour every zone sits on, and refused when no zone could be
/// that far out (the two readings were then not of one moment).
pub fn clock_offset(naive_log_secs: i64, epoch_secs: i64) -> Option<i64> {
    let raw = naive_log_secs - epoch_secs;
    let rounded = (raw as f64 / 900.0).round() as i64 * 900;
    ((raw - rounded).abs() <= 120 && rounded.abs() <= 16 * 3600).then_some(rounded)
}

impl Hub {
    pub fn new() -> Arc<Hub> {
        Arc::new(Hub::default())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Lines the game wrote just now (`epoch_now` is when they were read).
    pub fn feed_live<'a>(&self, lines: impl IntoIterator<Item = &'a str>, epoch_now: i64) {
        let mut st = self.lock();
        let mut stamped = None;
        for line in lines {
            stamped = khaloni_poe2_core::runs::timestamp(line).or(stamped);
            let before = st.tracker.place();
            let closed = st.tracker.intervals_closed();
            st.tracker.feed(line);
            // A hideout entered now; one left, or re-entered from a map,
            // restarts the clock.
            if st.tracker.place() != before || st.tracker.intervals_closed() != closed {
                st.hideout_since = (st.tracker.place() == Place::Hideout).then(Instant::now);
            }
        }
        // Only a line written just now says what the log's clock reads now.
        if let Some(offset) = stamped.and_then(|t| clock_offset(t, epoch_now)) {
            st.offset = Some(offset);
        }
    }

    /// Replaces the tracker with one that read the existing log.
    /// `file_mtime_epoch` stands in for "now" on the log's last line.
    pub fn set_cold(&self, tracker: Tracker, file_mtime_epoch: Option<i64>) {
        let mut st = self.lock();
        st.offset = tracker.last_seen().zip(file_mtime_epoch).and_then(|(t, m)| clock_offset(t, m));
        // Already in the hideout when the overlay started: the minute
        // counts from here.
        st.hideout_since = (tracker.place() == Place::Hideout).then(Instant::now);
        st.snapshot_mark = tracker.intervals_closed();
        st.tracker = tracker;
        st.log_read = true;
        st.log_missing = false;
    }

    pub fn set_log_missing(&self) {
        self.lock().log_missing = true;
    }

    pub fn offset(&self) -> Option<i64> {
        self.lock().offset
    }

    pub fn in_hideout_for(&self) -> Option<Duration> {
        self.lock().hideout_since.map(|t| t.elapsed())
    }

    pub fn intervals_closed(&self) -> u64 {
        self.lock().tracker.intervals_closed()
    }

    pub fn map_ended_since_snapshot(&self) -> bool {
        let st = self.lock();
        st.tracker.intervals_closed() > st.snapshot_mark
    }

    /// A stash walk that began at `intervals_closed_before` is on disk.
    pub fn mark_snapshot(&self, intervals_closed_before: u64) {
        self.lock().snapshot_mark = intervals_closed_before;
    }

    pub fn finished_runs(&self) -> Vec<Run> {
        self.lock().tracker.finished().into_iter().cloned().collect()
    }

    /// What the panel shows right now; cheap, for the main loop.
    pub fn shown(&self) -> Shown {
        if let Some(view) = self.shown.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            return Shown::Runs(view);
        }
        if self.lock().log_missing {
            Shown::LogMissing
        } else {
            Shown::Reading
        }
    }

    fn publish(&self, view: View) {
        let mut slot = self.shown.lock().unwrap_or_else(|e| e.into_inner());
        if slot.as_deref() != Some(&view) {
            *slot = Some(Arc::new(view));
        }
    }
}

/// Reads the end of an existing log into a fresh tracker: at most
/// [`COLD_READ_BYTES`], the cut-off first line dropped. Returns the tracker,
/// the offset the tail continues from, and the unterminated rest of the
/// last line (the writer can be mid-line).
pub fn cold_read(path: &Path) -> std::io::Result<(Tracker, u64, String)> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    let start = len.saturating_sub(COLD_READ_BYTES);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::with_capacity((len - start) as usize);
    file.take(len - start).read_to_end(&mut bytes)?;
    let end = start + bytes.len() as u64;
    // Lossy: one bad byte in a chat line must not cost the runs.
    let text = String::from_utf8_lossy(&bytes);
    let mut body: &str = &text;
    if start > 0 {
        body = body.split_once('\n').map(|(_, rest)| rest).unwrap_or("");
    }
    let (complete, carry) = match body.rfind('\n') {
        Some(nl) => (&body[..nl], &body[nl + 1..]),
        None => ("", body),
    };
    let mut tracker = Tracker::new();
    for line in complete.split('\n') {
        tracker.feed(line);
    }
    Ok((tracker, end, carry.to_string()))
}

/// The summary of `runs` and the league's snapshots on disk.
pub fn summarize_from_disk(runs: &[Run], league: &str, offset: Option<i64>, access: StashAccess, now: i64) -> Summary {
    let offset = offset.unwrap_or(0);
    let snapshots = match access {
        StashAccess::Ready => crate::wealth::load_itemised(league, offset),
        StashAccess::Missing(_) => Vec::new(),
    };
    income::summarize(runs, &snapshots, now + offset, access)
}

/// What the view was last built from.
#[derive(PartialEq)]
struct BuiltFrom {
    revision: u64,
    league: String,
    access: StashAccess,
    history: Option<(u64, std::time::SystemTime)>,
    day: i64,
}

/// Rebuilds the view when the runs, the snapshots, the league, the
/// credentials or the date changed. `inputs` reads (league, access) off the
/// live config. Exits when the hub is no longer shared.
pub fn spawn_view_worker(hub: Arc<Hub>, inputs: impl Fn() -> (String, StashAccess) + Send + 'static) {
    std::thread::spawn(move || {
        let mut built: Option<BuiltFrom> = None;
        while Arc::strong_count(&hub) > 1 {
            std::thread::sleep(Duration::from_secs(2));
            let (ready, offset, revision) = {
                let st = hub.lock();
                (st.log_read, st.offset, st.tracker.revision())
            };
            if !ready {
                continue;
            }
            let (league, access) = inputs();
            let now = crate::prices::unix_now();
            let history = std::fs::metadata(crate::wealth::history_path())
                .ok()
                .and_then(|m| Some((m.len(), m.modified().ok()?)));
            let key = BuiltFrom {
                revision,
                league: league.clone(),
                access: access.clone(),
                history,
                day: (now + offset.unwrap_or(0)).div_euclid(86_400),
            };
            if built.as_ref() == Some(&key) {
                continue;
            }
            let runs = hub.finished_runs();
            hub.publish(View::build(&summarize_from_disk(&runs, &league, offset, access, now)));
            built = Some(key);
        }
    });
}

/// The log, read once for a window that is not the overlay: the runs, the
/// log's clock offset and the summary, by the same path the overlay takes.
pub struct Loaded {
    pub runs: Vec<Run>,
    pub summary: Summary,
}

pub fn load(log: &Path, league: &str, access: StashAccess, now: i64) -> std::io::Result<Loaded> {
    let (tracker, _, _) = cold_read(log)?;
    let mtime = std::fs::metadata(log)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64);
    let offset = tracker.last_seen().zip(mtime).and_then(|(t, m)| clock_offset(t, m));
    let runs: Vec<Run> = tracker.finished().into_iter().cloned().collect();
    let summary = summarize_from_disk(&runs, league, offset, access, now);
    Ok(Loaded { runs, summary })
}

/// Writes `runs-<stamp>.csv` and `blocks-<stamp>.csv` into `dir` and
/// returns the two paths. `stamp` keeps an export from overwriting the
/// one before it.
pub fn export_csv(dir: &Path, loaded: &Loaded, stamp: &str) -> std::io::Result<(std::path::PathBuf, std::path::PathBuf)> {
    std::fs::create_dir_all(dir)?;
    let runs = dir.join(format!("runs-{stamp}.csv"));
    let blocks = dir.join(format!("blocks-{stamp}.csv"));
    std::fs::write(&runs, income::runs_csv(&loaded.runs))?;
    std::fs::write(&blocks, income::blocks_csv(&loaded.summary.blocks))?;
    Ok((runs, blocks))
}
