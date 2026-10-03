//! The run tracker against a log written for the purpose: the line forms
//! are the game's, the content is made up.

use khaloni_poe2_core::runs::{timestamp, Mechanic, Totals, Tracker};

fn at(clock: &str) -> String {
    format!("2026/09/01 {clock} 1234567 3ef23348")
}

fn area(clock: &str, code: &str, seed: u64) -> String {
    format!("{} [DEBUG Client 356] Generating level 79 area \"{code}\" with seed {seed}", at(clock))
}

fn system(clock: &str, text: &str) -> String {
    format!("{} [INFO Client 356] : {text}", at(clock))
}

fn chat(clock: &str, sender: &str, text: &str) -> String {
    format!("{} [INFO Client 356] #{sender}: {text}", at(clock))
}

fn warning(clock: &str, path: &str) -> String {
    format!("{} [CRIT Client 356] Tried to issue a MoveTo with NaN speed! Object: {path}", at(clock))
}

const CLIENT_START: &str = "2026/09/01 09:00:00 ***** LOG FILE OPENING *****";

fn track(lines: &[String]) -> Tracker {
    let mut t = Tracker::new();
    for l in lines {
        t.feed(l);
    }
    t
}

#[test]
fn the_same_seed_again_is_a_portal_not_a_new_run() {
    let t = track(&[
        area("10:00:00", "MapRavine", 111),
        area("10:05:00", "HideoutCanal", 1),
        area("10:06:00", "MapRavine", 111),
        area("10:10:00", "HideoutCanal", 1),
        // The same layout under another seed is another instance.
        area("10:11:00", "MapRavine", 222),
        area("10:12:00", "HideoutCanal", 1),
    ]);
    let runs = t.finished();
    assert_eq!(runs.len(), 2);
    assert_eq!((runs[0].seed.as_str(), runs[0].portals), ("111", 1));
    assert_eq!((runs[1].seed.as_str(), runs[1].portals), ("222", 0));
    assert_eq!(runs[0].started, timestamp("2026/09/01 10:00:00").unwrap());
    assert_eq!(runs[0].ended, timestamp("2026/09/01 10:10:00").unwrap());
}

#[test]
fn hideout_time_between_portals_is_not_map_time() {
    let t = track(&[
        area("10:00:00", "MapRavine", 111),
        area("10:05:00", "HideoutCanal", 1),
        // Twenty minutes of trading in the hideout.
        area("10:25:00", "MapRavine", 111),
        area("10:28:00", "HideoutCanal", 1),
    ]);
    assert_eq!(t.finished()[0].seconds(), Some(5 * 60 + 3 * 60));
}

#[test]
fn a_client_start_inside_a_map_makes_its_duration_unknown() {
    let t = track(&[
        area("08:40:00", "MapRavine", 111),
        system("08:45:00", "Connecting to instance server"),
        CLIENT_START.to_string(),
        area("09:01:00", "HideoutCanal", 1),
        area("09:02:00", "MapSteppe", 333),
        area("09:06:00", "HideoutCanal", 1),
    ]);
    let runs = t.finished();
    assert_eq!(runs[0].seconds(), None, "the crash took an unknown share of it");
    assert_eq!(runs[0].ended, timestamp("2026/09/01 08:45:00").unwrap(), "closed at the last line of the old client");
    assert_eq!(runs[1].seconds(), Some(240), "the next map is measured as usual");
    let totals = Totals::of(runs);
    assert_eq!((totals.maps, totals.maps_unknown_duration, totals.map_seconds), (2, 1, 240));
}

#[test]
fn an_interval_over_an_hour_is_unknown_not_an_hour() {
    let t = track(&[
        area("10:00:00", "MapRavine", 111),
        area("11:00:01", "HideoutCanal", 1),
        area("11:01:00", "MapSteppe", 333),
        area("12:01:00", "HideoutCanal", 1),
    ]);
    let runs = t.finished();
    assert_eq!(runs[0].seconds(), None);
    assert_eq!(runs[1].seconds(), Some(3600), "exactly an hour is still a length");
    // One unknown interval is enough, whatever the others measured.
    let t = track(&[
        area("10:00:00", "MapRavine", 111),
        area("10:05:00", "HideoutCanal", 1),
        area("10:06:00", "MapRavine", 111),
        area("12:00:00", "HideoutCanal", 1),
    ]);
    assert_eq!(t.finished()[0].seconds(), None);
}

#[test]
fn afk_time_is_subtracted_from_map_time() {
    let t = track(&[
        area("10:00:00", "MapRavine", 111),
        system("10:02:00", "AFK mode is now ON. Autoreply \"back soon\""),
        system("10:07:00", "AFK mode is now OFF."),
        area("10:10:00", "HideoutCanal", 1),
        // Still away when the map is left: the span ends with the interval.
        area("10:20:00", "MapSteppe", 333),
        system("10:21:00", "AFK mode is now ON. Autoreply \"back soon\""),
        area("10:30:00", "HideoutCanal", 1),
        system("10:31:00", "AFK mode is now OFF."),
    ]);
    let runs = t.finished();
    assert_eq!(runs[0].seconds(), Some(10 * 60 - 5 * 60));
    assert_eq!(runs[1].seconds(), Some(60));
}

#[test]
fn a_chat_line_imitating_the_afk_message_changes_nothing() {
    let t = track(&[
        area("10:00:00", "MapRavine", 111),
        chat("10:01:00", "Someone", "AFK mode is now ON."),
        chat("10:01:30", "Someone", "] : AFK mode is now ON."),
        chat("10:02:00", "Someone", "Generating level 79 area \"MapSteppe\" with seed 5"),
        chat("10:02:30", "Someone", "***** LOG FILE OPENING *****"),
        chat("10:03:00", "Someone", "look at Metadata/Monsters/LeagueRitual/Thing"),
        area("10:10:00", "HideoutCanal", 1),
    ]);
    let runs = t.finished();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].seconds(), Some(600));
    assert!(runs[0].mechanics.is_empty());
}

#[test]
fn a_mechanic_is_counted_once_per_run() {
    let t = track(&[
        // Seen in the hideout: belongs to no run.
        warning("09:59:00", "Metadata/Monsters/Breach/BreachFodder"),
        area("10:00:00", "MapRavine", 111),
        warning("10:01:00", "Metadata/Monsters/LeagueRitual/RitualDemon"),
        warning("10:01:05", "Metadata/Monsters/LeagueRitual/RitualDemon"),
        warning("10:02:00", "Metadata/Monsters/LeagueExpeditionNew/Runner"),
        warning("10:03:00", "Metadata/Monsters/Skeletons/SkeletonBasic"),
        area("10:05:00", "HideoutCanal", 1),
        area("10:06:00", "MapRavine", 111),
        warning("10:07:00", "Metadata/Monsters/LeagueRitual/RitualDemon"),
        area("10:08:00", "HideoutCanal", 1),
    ]);
    let runs = t.finished();
    let seen: Vec<&str> = runs[0].mechanics.iter().map(|m| m.name()).collect();
    assert_eq!(seen, ["Expedition", "Ritual"]);
    for (token, name) in [
        ("LeagueRitual", "Ritual"),
        ("Breach", "Breach"),
        ("LeagueExpeditionNew", "Expedition"),
        ("LeagueDeliriumX", "Delirium"),
        ("Delirium", "Delirium"),
        ("LeagueAbyss", "Abyss"),
        ("Abyss", "Abyss"),
        ("LeagueIncursionNew", "Incursion"),
        ("LeagueEssence", "Essences"),
        ("Essence", "Essences"),
    ] {
        assert_eq!(Mechanic::from_token(token).map(|m| m.name()), Some(name), "{token}");
    }
    assert_eq!(Mechanic::from_token("LeagueHellscape"), None);
}

#[test]
fn the_run_in_progress_is_not_reported() {
    let mut t = track(&[
        area("10:00:00", "MapRavine", 111),
        area("10:05:00", "HideoutCanal", 1),
        area("10:06:00", "MapSteppe", 333),
    ]);
    assert_eq!(t.finished().len(), 1, "the map being played has no end yet");
    // Back inside the first map: that one is now the run in progress.
    t.feed(&area("10:20:00", "MapRavine", 111));
    let areas: Vec<&str> = t.finished().iter().map(|r| r.area.as_str()).collect();
    assert_eq!(areas, ["MapSteppe"]);
    // Fed line by line, as the app tails the log, it ends like any other.
    t.feed(&area("10:25:00", "HideoutCanal", 1));
    assert_eq!(t.finished().len(), 2);
    assert_eq!(t.finished()[0].seconds(), Some(300 + 300));
}
