//! Map runs read off the game log, one line at a time.
//!
//! The log carries four things this is built on: every area entered
//! (`Generating level 79 area "MapRavine" with seed 123`, the seed naming
//! the instance), client starts (`***** LOG FILE OPENING *****`), the AFK
//! system messages, and engine warnings that happen to name a league
//! mechanic's monsters (`Metadata/Monsters/LeagueRitual/...`).
//!
//! A run is one map instance, keyed by (area code, seed). Its time is the
//! sum of the intervals spent inside it; hideout time between portals is
//! counted nowhere. An interval a client start falls into, or one longer
//! than an hour, has no known length, and then neither has the run: it is
//! still a map, and it stays out of everything measured per hour.
//!
//! Timestamps in the log are local wall-clock time without a zone. They are
//! read as naive seconds (the civil date and time taken as if it were UTC):
//! only differences and same-log comparisons are ever made with them.

use std::collections::{BTreeSet, HashMap};

/// An interval longer than this was not spent playing the map.
pub const MAX_INTERVAL_SECS: i64 = 3600;
/// How much of an existing log is read at startup, from its end.
pub const COLD_READ_BYTES: u64 = 40 * 1024 * 1024;

const CLIENT_START: &str = "***** LOG FILE OPENING *****";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Mechanic {
    Abyss,
    Breach,
    Delirium,
    Essences,
    Expedition,
    Incursion,
    Ritual,
}

impl Mechanic {
    pub const ALL: [Mechanic; 7] = [
        Mechanic::Abyss,
        Mechanic::Breach,
        Mechanic::Delirium,
        Mechanic::Essences,
        Mechanic::Expedition,
        Mechanic::Incursion,
        Mechanic::Ritual,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Mechanic::Abyss => "Abyss",
            Mechanic::Breach => "Breach",
            Mechanic::Delirium => "Delirium",
            Mechanic::Essences => "Essences",
            Mechanic::Expedition => "Expedition",
            Mechanic::Incursion => "Incursion",
            Mechanic::Ritual => "Ritual",
        }
    }

    pub fn from_name(name: &str) -> Option<Mechanic> {
        Mechanic::ALL.into_iter().find(|m| m.name() == name)
    }

    /// The mechanic a monster directory belongs to: the path segment right
    /// after `Metadata/Monsters/`.
    pub fn from_token(token: &str) -> Option<Mechanic> {
        let starts = |p: &str| token.starts_with(p);
        if starts("LeagueRitual") {
            Some(Mechanic::Ritual)
        } else if starts("Breach") {
            Some(Mechanic::Breach)
        } else if starts("LeagueExpedition") {
            Some(Mechanic::Expedition)
        } else if starts("LeagueDelirium") || starts("Delirium") {
            Some(Mechanic::Delirium)
        } else if starts("LeagueAbyss") || starts("Abyss") {
            Some(Mechanic::Abyss)
        } else if starts("LeagueIncursion") {
            Some(Mechanic::Incursion)
        } else if starts("LeagueEssence") || starts("Essence") {
            Some(Mechanic::Essences)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub area: String,
    /// Kept as text: it is an identity, never a quantity.
    pub seed: String,
    /// Naive seconds of the first entry, and of the end of the last
    /// interval closed so far.
    pub started: i64,
    pub ended: i64,
    known_secs: i64,
    unknown: bool,
    /// Entries after the first.
    pub portals: u32,
    pub mechanics: BTreeSet<Mechanic>,
}

impl Run {
    /// Time spent inside the map with AFK spans taken out; `None` when any
    /// of its intervals has no known length.
    pub fn seconds(&self) -> Option<i64> {
        (!self.unknown).then_some(self.known_secs)
    }

    pub fn saw(&self, m: Mechanic) -> bool {
        self.mechanics.contains(&m)
    }

    /// A finished run for tests and previews, without going through a log.
    pub fn synthetic(area: &str, seed: &str, started: i64, seconds: Option<i64>, mechanics: &[Mechanic]) -> Run {
        Run {
            area: area.to_string(),
            seed: seed.to_string(),
            started,
            ended: started + seconds.unwrap_or(0),
            known_secs: seconds.unwrap_or(0),
            unknown: seconds.is_none(),
            portals: 0,
            mechanics: mechanics.iter().copied().collect(),
        }
    }
}

/// Where the last area line put the player.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Place {
    /// Nothing read yet, or the client started and no area followed.
    #[default]
    Unknown,
    Map,
    Hideout,
    Elsewhere,
}

#[derive(Debug, Clone)]
struct Open {
    run: usize,
    since: i64,
    unknown: bool,
    afk_since: Option<i64>,
    afk: i64,
}

#[derive(Debug, Clone, Default)]
pub struct Tracker {
    runs: Vec<Run>,
    index: HashMap<(String, String), usize>,
    open: Option<Open>,
    /// The last timestamp read on any line.
    last: Option<i64>,
    place: Place,
    intervals_closed: u64,
    revision: u64,
}

/// `YYYY/MM/DD HH:MM:SS` at the start of a line, as naive seconds.
pub fn timestamp(line: &str) -> Option<i64> {
    let b = line.as_bytes();
    if b.len() < 19 {
        return None;
    }
    let num = |from: usize, to: usize| -> Option<i64> {
        b[from..to].iter().try_fold(0i64, |acc, c| c.is_ascii_digit().then(|| acc * 10 + i64::from(c - b'0')))
    };
    let sep = |i: usize, c: u8| b[i] == c;
    if !(sep(4, b'/') && sep(7, b'/') && sep(10, b' ') && sep(13, b':') && sep(16, b':')) {
        return None;
    }
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi, s) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    Some(days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + s)
}

/// Days since 1970-01-01 of a proleptic Gregorian date.
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// (year, month, day) of a day count since 1970-01-01.
pub fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// What follows the `[LEVEL Client N] ` header, with the level.
fn header_and_message(line: &str) -> Option<(&str, &str)> {
    let open = line.find('[')?;
    let close = open + line[open..].find("] ")?;
    let level = line[open + 1..close].split(' ').next()?;
    Some((level, &line[close + 2..]))
}

/// A chat line has a sender before the colon; a system message has none, so
/// its text starts with the colon itself. Only INFO lines carry chat.
fn is_chat(level: &str, message: &str) -> bool {
    level == "INFO" && !message.starts_with(": ")
}

fn area_line(message: &str) -> Option<(&str, &str)> {
    let rest = message.strip_prefix("Generating level ")?;
    let rest = rest.trim_start_matches(|c: char| c.is_ascii_digit()).strip_prefix(" area \"")?;
    let (area, rest) = rest.split_once('"')?;
    let seed = rest.strip_prefix(" with seed ")?;
    let digits = seed.len() - seed.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    (digits > 0 && !area.is_empty()).then(|| (area, &seed[..digits]))
}

fn mechanics_on(line: &str) -> impl Iterator<Item = Mechanic> + '_ {
    const ROOT: &str = "Metadata/Monsters/";
    line.match_indices(ROOT).filter_map(|(at, _)| {
        let rest = &line[at + ROOT.len()..];
        let end = rest.find(|c: char| !c.is_ascii_alphabetic()).unwrap_or(rest.len());
        Mechanic::from_token(&rest[..end])
    })
}

impl Tracker {
    pub fn new() -> Tracker {
        Tracker::default()
    }

    /// Takes one complete log line (without its line ending).
    pub fn feed(&mut self, line: &str) {
        let line = line.trim_end_matches('\r');
        // The client-start line is the marker alone behind an optional
        // date; a chat line quoting it has a header in between.
        let bare = match timestamp(line) {
            Some(_) => line.get(20..).unwrap_or(""),
            None => line,
        };
        if bare.trim() == CLIENT_START {
            if let Some(open) = self.open.as_mut() {
                // Whatever ran until the client went away is not known: the
                // interval ends at the last thing the old client wrote.
                open.unknown = true;
                let at = self.last.unwrap_or(open.since);
                self.close(at);
            }
            self.place = Place::Unknown;
            self.revision += 1;
            return;
        }
        let Some(t) = timestamp(line) else { return };
        self.last = Some(t);
        let Some((level, message)) = header_and_message(line) else { return };
        if let Some((area, seed)) = area_line(message) {
            self.revision += 1;
            self.close(t);
            if area.starts_with("Map") {
                let key = (area.to_string(), seed.to_string());
                let run = match self.index.get(&key) {
                    Some(&i) => {
                        self.runs[i].portals += 1;
                        i
                    }
                    None => {
                        self.runs.push(Run {
                            area: key.0.clone(),
                            seed: key.1.clone(),
                            started: t,
                            ended: t,
                            known_secs: 0,
                            unknown: false,
                            portals: 0,
                            mechanics: BTreeSet::new(),
                        });
                        self.index.insert(key, self.runs.len() - 1);
                        self.runs.len() - 1
                    }
                };
                self.open = Some(Open { run, since: t, unknown: false, afk_since: None, afk: 0 });
                self.place = Place::Map;
            } else if area.starts_with("Hideout") {
                self.place = Place::Hideout;
            } else {
                self.place = Place::Elsewhere;
            }
            return;
        }
        let Some(open) = self.open.as_mut() else { return };
        if level == "INFO" && message.starts_with(": AFK mode is now ON.") {
            open.afk_since.get_or_insert(t);
        } else if level == "INFO" && message.starts_with(": AFK mode is now OFF.") {
            if let Some(since) = open.afk_since.take() {
                open.afk += t - since;
            }
        } else if !is_chat(level, message) {
            let run = &mut self.runs[open.run];
            run.mechanics.extend(mechanics_on(line));
        }
    }

    fn close(&mut self, t: i64) {
        let Some(mut open) = self.open.take() else { return };
        if let Some(since) = open.afk_since.take() {
            open.afk += (t - since).max(0);
        }
        let span = t - open.since;
        let run = &mut self.runs[open.run];
        if open.unknown || span > MAX_INTERVAL_SECS {
            run.unknown = true;
        } else {
            run.known_secs += (span - open.afk).max(0);
        }
        run.ended = t;
        self.intervals_closed += 1;
    }

    /// Every run but the one the player is inside right now, in order of
    /// first entry. The run in progress has no end yet, so it is not
    /// reported.
    pub fn finished(&self) -> Vec<&Run> {
        let inside = self.open.as_ref().map(|o| o.run);
        self.runs.iter().enumerate().filter(|(i, _)| Some(*i) != inside).map(|(_, r)| r).collect()
    }

    pub fn place(&self) -> Place {
        self.place
    }

    /// How many map intervals have ended.
    pub fn intervals_closed(&self) -> u64 {
        self.intervals_closed
    }

    /// Moves whenever what `finished` returns may have changed.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The last timestamp read, naive seconds.
    pub fn last_seen(&self) -> Option<i64> {
        self.last
    }
}

/// Maps, maps of unknown duration and the known map hours of `runs`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Totals {
    pub maps: usize,
    pub maps_unknown_duration: usize,
    pub map_seconds: i64,
}

impl Totals {
    pub fn of<'a>(runs: impl IntoIterator<Item = &'a Run>) -> Totals {
        let mut t = Totals::default();
        for r in runs {
            t.maps += 1;
            match r.seconds() {
                Some(s) => t.map_seconds += s,
                None => t.maps_unknown_duration += 1,
            }
        }
        t
    }

    pub fn map_hours(&self) -> f64 {
        self.map_seconds as f64 / 3600.0
    }
}
