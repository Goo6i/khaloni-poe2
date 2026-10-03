//! Income blocks: what counts, what waits, and what is never printed.

use std::collections::BTreeMap;

use khaloni_poe2_core::income::{
    blocks, mechanic_income, missing_text, not_enough_text, rate_text, sample_text, summarize, Holding,
    MechanicIncome, Snapshot, StashAccess,
};
use khaloni_poe2_core::runs::{Mechanic, Run};

fn snap(at: i64, items: &[(&str, u64, Option<f64>)]) -> Snapshot {
    Snapshot {
        at,
        league: "L".into(),
        items: items
            .iter()
            .map(|(n, q, p)| (n.to_string(), Holding { qty: *q, price_div: *p }))
            .collect::<BTreeMap<_, _>>(),
    }
}

/// A finished ten-minute run ending at `end`.
fn run(end: i64, mechanics: &[Mechanic]) -> Run {
    Run::synthetic("MapRavine", &end.to_string(), end - 600, Some(600), mechanics)
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

#[test]
fn a_price_move_alone_is_revaluation_not_income() {
    let snaps = [snap(0, &[("Divine Orb", 10, Some(1.0)), ("Chaos Orb", 100, Some(0.01))]),
        snap(1000, &[("Divine Orb", 10, Some(1.0)), ("Chaos Orb", 100, Some(0.03))])];
    let b = &blocks(&snaps, &[])[0];
    let f = b.figures.unwrap();
    assert!(close(f.income_div, 0.0), "nothing entered the stash: {f:?}");
    assert!(close(f.revaluation_div, 2.0));
}

#[test]
fn income_prices_both_ends_with_one_price_set() {
    // 50 chaos gained while the chaos tripled. Valued at B's prices the
    // gain is 50 x 0.03; the old stack's rise is revaluation. Together they
    // are the whole change of the priced total, and nothing is in both.
    let snaps = [snap(0, &[("Chaos Orb", 100, Some(0.01)), ("Old Base", 3, None)]),
        snap(1000, &[("Chaos Orb", 150, Some(0.03)), ("Old Base", 5, None)])];
    let f = blocks(&snaps, &[])[0].figures.unwrap();
    assert!(close(f.income_div, 1.5), "{f:?}");
    assert!(close(f.revaluation_div, 2.0), "{f:?}");
    assert!(close(f.income_div + f.revaluation_div, 150.0 * 0.03 - 100.0 * 0.01));
    assert_eq!(f.unpriced_changes, 1, "an item no table prices is counted as left out, not valued at zero");
}

#[test]
fn a_vanished_stack_waits_for_the_next_snapshot() {
    let full = [("Divine Orb", 10, Some(1.0)), ("Exalted Orb", 500, Some(0.002))];
    // B is missing the exalted stack (listed at 0 with its price, as the
    // tracker writes a name it held before).
    let gap = [("Divine Orb", 11, Some(1.0)), ("Exalted Orb", 0, Some(0.002))];
    let two = [snap(0, &full), snap(1000, &gap)];
    let b = blocks(&two, &[]);
    assert!(b[0].pending() && b[0].figures.is_none(), "no figure until another snapshot has spoken");
    assert!(!b[0].complete());

    // The next snapshot has the stack again: a gap in the API, not a loss.
    let back = [snap(0, &full), snap(1000, &gap), snap(2000, &[("Divine Orb", 11, Some(1.0)), ("Exalted Orb", 520, Some(0.002))])];
    let b = blocks(&back, &[]);
    assert!(close(b[0].figures.unwrap().income_div, 1.0), "only the divine: {:?}", b[0].figures);
    assert!(close(b[1].figures.unwrap().income_div, 20.0 * 0.002), "the 20 gained since A land here");

    // The next snapshot agrees it is gone: then it was spent, in A to B.
    let gone = [snap(0, &full), snap(1000, &gap), snap(2000, &[("Divine Orb", 11, Some(1.0))])];
    let b = blocks(&gone, &[]);
    assert!(close(b[0].figures.unwrap().income_div, 1.0 - 500.0 * 0.002));
    assert!(close(b[1].figures.unwrap().income_div, 0.0));
}

fn ritual_blocks(pure: usize, maps_each: usize) -> (Vec<Snapshot>, Vec<Run>) {
    let mut snaps = vec![snap(0, &[("Divine Orb", 0, Some(1.0))])];
    let mut runs = Vec::new();
    for i in 0..pure {
        let start = (i as i64) * 10_000;
        for k in 0..maps_each {
            runs.push(run(start + 1000 + (k as i64) * 700, &[Mechanic::Ritual]));
        }
        snaps.push(snap(start + 10_000, &[("Divine Orb", (i as u64 + 1) * 2, Some(1.0))]));
    }
    (snaps, runs)
}

#[test]
fn a_mixed_block_never_feeds_a_mechanic() {
    let (mut snaps, mut runs) = ritual_blocks(3, 4);
    // A fourth block, rich, where one map of three did not show Ritual.
    let t = 30_000;
    runs.push(run(t + 1000, &[Mechanic::Ritual]));
    runs.push(run(t + 2000, &[Mechanic::Ritual, Mechanic::Breach]));
    runs.push(run(t + 3000, &[Mechanic::Breach]));
    snaps.push(snap(t + 10_000, &[("Divine Orb", 106, Some(1.0))]));
    let b = blocks(&snaps, &runs);
    assert!(b[3].pure_for.is_empty() && b[3].mechanics.len() == 2);
    let MechanicIncome::Rate(r) = mechanic_income(&b, Mechanic::Ritual) else { panic!("three pure blocks, twelve maps") };
    assert_eq!((r.sample.blocks, r.sample.maps, r.sample.map_seconds), (3, 12, 12 * 600));
    assert!(close(r.div_per_map_hour, 6.0 / 2.0), "the rich mixed block is not in it: {r:?}");
    // It does count overall.
    let s = summarize(&runs, &snaps, 0, StashAccess::Ready);
    let overall = s.overall.unwrap();
    assert_eq!((overall.sample.blocks, overall.sample.maps), (4, 15));
    assert!(close(overall.div_per_map_hour, 106.0 / 2.5));
    assert_eq!(mechanic_income(&b, Mechanic::Breach), MechanicIncome::NotEnough { blocks: 0, maps: 0 });
}

#[test]
fn a_sample_under_the_minimum_prints_no_rate() {
    // Blocks enough, maps too few; then maps enough, blocks too few.
    for (pure, each) in [(3, 3), (2, 6)] {
        let (snaps, runs) = ritual_blocks(pure, each);
        let got = mechanic_income(&blocks(&snaps, &runs), Mechanic::Ritual);
        assert_eq!(got, MechanicIncome::NotEnough { blocks: pure, maps: pure * each });
    }
    let text = not_enough_text(2, 12);
    assert_eq!(text, "not enough runs yet (2 of 3 pure blocks, 12 of 10 maps)");
    assert!(!text.contains("div"));
    // At both minimums the rate prints, and never without its sample.
    let (snaps, runs) = ritual_blocks(3, 4);
    let MechanicIncome::Rate(r) = mechanic_income(&blocks(&snaps, &runs), Mechanic::Ritual) else { panic!() };
    assert_eq!(rate_text(&r), "3.00 div per map hour");
    assert_eq!(sample_text(&r.sample), "measured over 3 blocks, 12 maps, 2.0 h");
}

#[test]
fn runs_of_unknown_duration_stay_out_of_hourly_figures() {
    let snaps = [snap(0, &[("Divine Orb", 0, Some(1.0))]),
        snap(10_000, &[("Divine Orb", 50, Some(1.0))]),
        snap(20_000, &[("Divine Orb", 52, Some(1.0))])];
    let runs = vec![
        run(2000, &[]),
        Run::synthetic("MapSteppe", "9", 3000, None, &[]),
        run(12_000, &[]),
        run(13_000, &[]),
    ];
    let s = summarize(&runs, &snaps, 0, StashAccess::Ready);
    assert_eq!(s.blocks[0].map_seconds, None);
    assert!(!s.blocks[0].complete(), "its 50 div cannot be set against a time nobody knows");
    let overall = s.overall.unwrap();
    assert_eq!((overall.sample.blocks, overall.sample.maps, overall.sample.map_seconds), (1, 2, 1200));
    assert!(close(overall.div_per_map_hour, 2.0 / (1200.0 / 3600.0)));
    // The map still counts as a map.
    assert_eq!((s.all.maps, s.all.maps_unknown_duration, s.all.map_seconds), (4, 1, 1800));
    // A block without maps has no map time to divide by: it feeds no rate.
    let idle = [snap(0, &[("Divine Orb", 0, Some(1.0))]), snap(10_000, &[("Divine Orb", 50, Some(1.0))])];
    assert_eq!(summarize(&[], &idle, 0, StashAccess::Ready).overall, None);
}

#[test]
fn without_a_session_the_view_shows_maps_and_hours_only() {
    let (snaps, runs) = ritual_blocks(3, 4);
    let access = StashAccess::from_credentials("", "  ");
    assert_eq!(access, StashAccess::Missing(vec!["account name".into(), "POESESSID".into()]));
    let day = 86_400 * 20_000;
    let today = Run::synthetic("MapRavine", "77", day + 3600, Some(900), &[Mechanic::Ritual]);
    let mut all = runs.clone();
    all.push(today);
    let s = summarize(&all, &snaps, day + 7200, access.clone());
    assert_eq!((s.all.maps, s.today.maps, s.today.map_seconds), (13, 1, 900));
    assert!(s.overall.is_none() && s.blocks.is_empty(), "old snapshots do not speak for a tracker that is off");
    assert_eq!(s.mechanics.len(), 1);
    assert_eq!((s.mechanics[0].seen_in, s.mechanics[0].income), (13, None));
    let StashAccess::Missing(missing) = &s.access else { panic!() };
    assert_eq!(missing_text(missing), "income needs the account name and POESESSID in Settings, Account: maps and hours only");
    assert_eq!(StashAccess::from_credentials("me", "cookie"), StashAccess::Ready);
    assert_eq!(StashAccess::from_credentials("me", ""), StashAccess::Missing(vec!["POESESSID".into()]));
}
