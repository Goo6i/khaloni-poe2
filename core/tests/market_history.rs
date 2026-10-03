//! The market history log: what it appends, what it refuses to invent when
//! read back, and how it stays bounded.

use std::path::PathBuf;

use khaloni_poe2_core::market::Source;
use khaloni_poe2_core::market_history::{series, window_change, Bucket, Field, HistoryLog, Rates, Record};
use khaloni_poe2_core::ninja::ExchangeOverview;
use khaloni_poe2_core::value::Unit;

const BREACH: &str = include_str!("fixtures/market/Breach.json");
const HOUR: i64 = 3600;
const DAY: i64 = 86_400;
const T0: i64 = 1_790_000_000 - 1_790_000_000 % DAY;

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("khalonipoe2-mkthist-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn source() -> Source {
    let breach: ExchangeOverview = serde_json::from_str(BREACH).unwrap();
    Source::from_overviews("Forbidden Rites", &[("Breach", &breach)], &[])
}

/// One Breachstone record: `price` divines at `exalted` to the divine.
fn record(t: i64, price: f64, exalted: f64) -> Record {
    Record {
        t,
        source_t: t,
        rates: Rates { exalted, chaos: 8.0 },
        items: vec![("Breach".into(), "breachstone".into(), price, Some(700.0))],
    }
}

#[test]
fn a_record_carries_every_item_with_its_rates() {
    let r = Record::from_source(&source(), T0 + 5, T0);
    assert_eq!((r.t, r.source_t), (T0 + 5, T0));
    assert_eq!(r.rates, Rates { exalted: 474.2, chaos: 8.32 });
    assert_eq!(r.items.len(), 5);
    assert!(r.items.contains(&("Breach".into(), "breachstone".into(), 2.48, Some(751.5))));
    // On disk: one line, the entries as plain arrays.
    let line = serde_json::to_string(&r).unwrap();
    assert!(line.contains(r#"["Breach","breachstone",2.48,751.5]"#), "{line}");
    assert!(!line.contains('\n'));
}

#[test]
fn an_unchanged_payload_appends_nothing() {
    let d = dir("unchanged");
    let mut log = HistoryLog::open(&d, "Forbidden Rites");
    let first = Record::from_source(&source(), T0, T0);
    assert!(log.append(&first).unwrap());
    // Ten minutes on, the source has not recomputed: same figures, new
    // clock. Nothing is written.
    let again = Record::from_source(&source(), T0 + 600, T0 + 600);
    assert!(!log.append(&again).unwrap());
    assert_eq!(log.read().len(), 1);
    // Nor after a restart, when the last hash has to come from the file.
    let mut reopened = HistoryLog::open(&d, "Forbidden Rites");
    assert!(!reopened.append(&again).unwrap());
    assert_eq!(std::fs::read_to_string(log.path()).unwrap().lines().count(), 1);

    // One price moves: that is a new record, appended after the old one.
    let mut moved = source();
    moved.items[0].price_div *= 1.01;
    assert!(reopened.append(&Record::from_source(&moved, T0 + 1200, T0 + 1200)).unwrap());
    // So is a changed rate with every price the same.
    let mut rate = moved.clone();
    rate.exalted_rate = 480.0;
    assert!(reopened.append(&Record::from_source(&rate, T0 + 1800, T0 + 1800)).unwrap());
    let times: Vec<i64> = reopened.read().iter().map(|r| r.t).collect();
    assert_eq!(times, [T0, T0 + 1200, T0 + 1800]);
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn an_empty_bucket_is_null_not_interpolated() {
    // Records in hours 0, 1 and 4; the overlay was off for hours 2 and 3.
    let records = vec![
        record(T0 + 60, 2.0, 400.0),
        record(T0 + HOUR + 60, 2.2, 400.0),
        record(T0 + HOUR + 1800, 2.3, 400.0),
        record(T0 + 4 * HOUR + 60, 3.0, 400.0),
    ];
    let hourly = series(&records, "Breach", "breachstone", Field::Price(Unit::Divine), Bucket::Hour, T0, T0 + 5 * HOUR);
    // Hour 1 shows its newest record; hours 2 and 3 show nothing, not 2.3
    // carried forward and not a line drawn from 2.3 to 3.0.
    assert_eq!(hourly, [Some(2.0), Some(2.3), None, None, Some(3.0)]);
    let daily = series(&records, "Breach", "breachstone", Field::Depth, Bucket::Day, T0 - DAY, T0 + 2 * DAY);
    assert_eq!(daily, [None, Some(700.0), None]);
    // An item the log never saw is a series of gaps, not of zeros.
    let unknown = series(&records, "Breach", "no-such-item", Field::Price(Unit::Divine), Bucket::Hour, T0, T0 + 2 * HOUR);
    assert_eq!(unknown, [None, None]);

    // A window is answered only when records stand near both its ends.
    let now = T0 + 4 * HOUR + 120;
    let four_hours = window_change(&records, "Breach", "breachstone", now, 4 * HOUR).unwrap();
    assert!((four_hours.pct - 50.0).abs() < 1e-9);
    assert_eq!(four_hours.records, 4, "the figure says how many records it rests on");
    assert_eq!(window_change(&records, "Breach", "breachstone", now, DAY), None, "no record a day back");
    assert_eq!(window_change(&records, "Breach", "breachstone", now + DAY, 4 * HOUR), None, "nothing recent");
    assert_eq!(window_change(&records[2..], "Breach", "breachstone", now, 3 * HOUR), None, "two records are not enough");
}

#[test]
fn old_values_convert_at_the_rates_recorded_with_them() {
    // The same 2 div price on two days; the exalted orb halved in between.
    let records = vec![record(T0 + 60, 2.0, 200.0), record(T0 + DAY + 60, 2.0, 400.0)];
    let in_exalted =
        series(&records, "Breach", "breachstone", Field::Price(Unit::Exalted), Bucket::Day, T0, T0 + 2 * DAY);
    assert_eq!(in_exalted, [Some(400.0), Some(800.0)], "day one at day one's 200, not at today's 400");
    let in_divine =
        series(&records, "Breach", "breachstone", Field::Price(Unit::Divine), Bucket::Day, T0, T0 + 2 * DAY);
    assert_eq!(in_divine, [Some(2.0), Some(2.0)]);
    let in_chaos = series(&records, "Breach", "breachstone", Field::Price(Unit::Chaos), Bucket::Day, T0, T0 + 2 * DAY);
    assert_eq!(in_chaos, [Some(16.0), Some(16.0)]);
    // A record without a rate cannot be converted, and is a gap.
    let rateless = vec![record(T0 + 60, 2.0, 0.0)];
    let gap = series(&rateless, "Breach", "breachstone", Field::Price(Unit::Exalted), Bucket::Day, T0, T0 + DAY);
    assert_eq!(gap, [None]);
    // It survives the trip through the file.
    let d = dir("rates");
    let mut log = HistoryLog::open(&d, "L");
    for r in &records {
        assert!(log.append(r).unwrap());
    }
    assert_eq!(log.read(), records);
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn the_history_file_is_capped_oldest_first() {
    let d = dir("cap");
    let line_len = serde_json::to_string(&record(T0, 2.0, 400.0)).unwrap().len() as u64 + 1;
    // Room for three records and a bit.
    let mut log = HistoryLog::with_cap(&d, "L", 3 * line_len + line_len / 2);
    for i in 0..6 {
        assert!(log.append(&record(T0 + i * HOUR, 2.0 + i as f64, 400.0)).unwrap());
    }
    let kept: Vec<i64> = log.read().iter().map(|r| (r.t - T0) / HOUR).collect();
    assert_eq!(kept, [3, 4, 5], "the oldest went, the newest stayed, in order");
    assert!(std::fs::metadata(log.path()).unwrap().len() <= 3 * line_len + line_len / 2);
    assert_eq!(log.first_record_time(), Some(T0 + 3 * HOUR));
    // The newest record is kept even when it alone is over the cap.
    let mut tiny = HistoryLog::with_cap(&d, "Tiny", 10);
    assert!(tiny.append(&record(T0, 2.0, 400.0)).unwrap());
    assert!(tiny.append(&record(T0 + HOUR, 3.0, 400.0)).unwrap());
    assert_eq!(tiny.read().iter().map(|r| r.t).collect::<Vec<_>>(), [T0 + HOUR]);
    // Skipping an unchanged payload still works after a trim.
    assert!(!log.append(&record(T0 + 9 * HOUR, 7.0, 400.0)).unwrap());
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn a_league_change_switches_the_history_file() {
    let d = dir("league");
    let mut log = HistoryLog::open(&d, "Forbidden Rites");
    assert!(log.append(&record(T0, 2.0, 400.0)).unwrap());
    let old_path = log.path();
    assert_eq!(old_path, d.join("Forbidden Rites.jsonl"));

    log.switch_league("Runes of Aldur", None).unwrap();
    assert_eq!((log.league(), log.path()), ("Runes of Aldur", d.join("Runes of Aldur.jsonl")));
    assert!(log.read().is_empty() && log.first_record_time().is_none());
    // The same figures are new to THIS league's file: the skip compares
    // with the file's own newest record, not with the last thing written.
    assert!(log.append(&record(T0 + HOUR, 2.0, 400.0)).unwrap());
    assert_eq!(log.read().len(), 1);
    // The league left behind keeps its file, untouched.
    assert_eq!(HistoryLog::open(&d, "Forbidden Rites").read(), [record(T0, 2.0, 400.0)]);

    // Back again with the league's start known: what was recorded before
    // it (an earlier league under the same name) goes.
    let mut back = HistoryLog::open(&d, "Runes of Aldur");
    back.switch_league("Forbidden Rites", None).unwrap();
    assert!(back.append(&record(T0 + 10 * DAY, 3.0, 450.0)).unwrap());
    back.switch_league("Forbidden Rites", Some(T0 + 5 * DAY)).unwrap();
    assert_eq!(back.read(), [record(T0 + 10 * DAY, 3.0, 450.0)]);
    assert!(!back.append(&record(T0 + 11 * DAY, 3.0, 450.0)).unwrap(), "the skip survives the prune");

    // A league name cannot climb out of the directory.
    assert_eq!(HistoryLog::open(&d, "../evil").path(), d.join(".._evil.jsonl"));
    let _ = std::fs::remove_dir_all(d);
}
