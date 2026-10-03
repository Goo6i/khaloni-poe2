//! The "My runs" tab of the market panel and what feeds it: the panel's
//! layout, clicks, drawing and wording, the log read at startup, the log's
//! clock, the hideout snapshot rule, the itemised stash history, the export.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use khaloni_poe2::market_ui::{self, Action, Freshness, Ink, Line, Panel, Tab};
use khaloni_poe2::myruns::{self, clock_offset, cold_read, Hub, Shown, View};
use khaloni_poe2::render::Renderer;
use khaloni_poe2::wealth::{self, WealthSnapshot};
use khaloni_poe2_core::income::{self, Holding, Snapshot, StashAccess};
use khaloni_poe2_core::runs::{timestamp, Mechanic, Run};

fn m(s: &str) -> i32 {
    7 * s.chars().count() as i32
}

/// Four pure Ritual blocks of three maps, then a mixed block, then a block
/// whose last snapshot lost a stack (pending).
fn sample() -> (Vec<Run>, Vec<Snapshot>) {
    let day = 20_000 * 86_400;
    let snap = |at: i64, div: u64, chaos: u64| Snapshot {
        at,
        league: "L".into(),
        items: BTreeMap::from([
            ("Divine Orb".to_string(), Holding { qty: div, price_div: Some(1.0) }),
            ("Chaos Orb".to_string(), Holding { qty: chaos, price_div: Some(0.02) }),
        ]),
    };
    let mut runs = Vec::new();
    let mut snaps = vec![snap(day, 10, 100)];
    for b in 0..4i64 {
        for k in 0..3i64 {
            let start = day + b * 3000 + 100 + k * 800;
            runs.push(Run::synthetic("MapRavine", &format!("{b}{k}"), start, Some(700), &[Mechanic::Ritual]));
        }
        snaps.push(snap(day + (b + 1) * 3000, 10 + 2 * (b as u64 + 1), 100));
    }
    runs.push(Run::synthetic("MapSteppe", "90", day + 12_100, Some(700), &[Mechanic::Ritual, Mechanic::Breach]));
    runs.push(Run::synthetic("MapSteppe", "91", day + 13_000, Some(700), &[Mechanic::Breach]));
    snaps.push(snap(day + 15_000, 21, 100));
    runs.push(Run::synthetic("MapSteppe", "92", day + 15_100, None, &[]));
    snaps.push(snap(day + 18_000, 22, 0));
    (runs, snaps)
}

fn view(access: StashAccess) -> Arc<View> {
    let (runs, snaps) = sample();
    Arc::new(View::build(&income::summarize(&runs, &snaps, 20_000 * 86_400 + 20_000, access)))
}

fn missing() -> StashAccess {
    StashAccess::from_credentials("", "")
}

fn runs_panel(access: StashAccess) -> Panel {
    let mut p = Panel::loading("L");
    market_ui::apply(&mut p, &Action::SetTab(Tab::MyRuns));
    p.set_runs(Shown::Runs(view(access)));
    p
}

fn texts(p: &Panel) -> Vec<String> {
    market_ui::all_text(p, &m)
}

#[test]
fn the_tab_lays_out_its_tables_inside_the_panel() {
    let p = runs_panel(StashAccess::Ready);
    let lay = market_ui::layout(&p, &m);
    assert_eq!(lay.tabs.len(), 3);
    let mut y = 0;
    for (rect, line) in &lay.lines {
        assert!(rect.y >= y, "lines run downwards without overlap");
        y = rect.y + rect.h as i32;
        assert!(rect.x >= 0 && rect.x + rect.w as i32 <= lay.w);
        if let Line::Cells { table, cells } = line {
            let xs = &lay.tables[*table];
            assert!(xs.len() >= cells.len());
            for (i, (text, _)) in cells.iter().enumerate() {
                assert!(xs[i] + m(text) <= lay.w - 12, "{text:?} runs past the panel's edge");
                if i + 1 < cells.len() {
                    assert!(xs[i] + m(text) < xs[i + 1], "{text:?} runs into the next column");
                }
            }
        }
    }
    assert!(y <= lay.h);
    let sections: Vec<&str> =
        lay.lines.iter().filter_map(|(_, l)| if let Line::Section(s) = l { Some(s.as_str()) } else { None }).collect();
    assert_eq!(sections, ["Maps", "Income", "Mechanics seen", "Last blocks"]);
    let all = texts(&p);
    assert!(all.contains(&"today".to_string()) && all.iter().any(|t| t.starts_with("since ")));
    assert!(all.iter().any(|t| t.contains("div per map hour")));
    assert!(all.contains(&"measured over 5 blocks, 14 maps, 2.7 h".to_string()), "{all:?}");
    assert!(all.contains(&"pending".to_string()), "the block that lost a stack waits");
    assert!(all.iter().any(|t| t.starts_with("1 of 15 maps have no known duration")));
}

#[test]
fn the_third_tab_is_clickable_and_does_not_wait_for_prices() {
    // Prices still loading: the first two tabs have one sentence, the
    // third has the runs.
    let mut p = Panel::loading("L");
    p.set_runs(Shown::Runs(view(StashAccess::Ready)));
    let lay = market_ui::layout(&p, &m);
    let (rect, tab) = lay.tabs[2];
    assert_eq!(tab, Tab::MyRuns);
    let action = market_ui::hit(&lay, rect.x + 3, rect.y + 3);
    assert_eq!(action, Some(Action::SetTab(Tab::MyRuns)));
    assert!(market_ui::apply(&mut p, &action.unwrap()));
    assert!(texts(&p).contains(&"Last blocks".to_string()));
    // No row of the tab opens anything.
    let lay = market_ui::layout(&p, &m);
    for (rect, _) in &lay.lines {
        assert_eq!(market_ui::hit(&lay, rect.x + 5, rect.y + 5), None);
    }
    assert_eq!(market_ui::hit(&lay, lay.close.x + 2, lay.close.y + 2), Some(Action::Close));
    // Before the log is read, and without a log, it says which.
    p.set_runs(Shown::Reading);
    assert!(texts(&p).contains(&myruns::READING_LOG.to_string()));
    p.set_runs(Shown::LogMissing);
    assert!(texts(&p).contains(&myruns::LOG_MISSING.to_string()));
}

#[test]
fn without_a_session_the_tab_names_what_is_missing_and_prints_no_income_figure() {
    let p = runs_panel(missing());
    let all = texts(&p);
    assert!(all.contains(
        &"income needs the account name and POESESSID in Settings, Account: maps and hours only".to_string()
    ));
    assert!(all.contains(&"seen in 13 of 15 maps".to_string()), "maps and mechanics seen still show: {all:?}");
    for t in &all {
        assert!(!t.contains(" div"), "{t:?} is an income figure");
        assert!(!t.contains("not enough runs"), "{t:?} speaks of a sample that cannot exist");
    }
    assert!(!all.contains(&"Last blocks".to_string()));
    assert!(!p.status().contains("stash"));
}

#[test]
fn income_wording_stays_on_the_my_runs_tab_and_says_map_hour() {
    for access in [StashAccess::Ready, missing()] {
        let mut p = runs_panel(access);
        for t in texts(&p) {
            let t = t.to_lowercase();
            assert!(!t.contains("profit"), "{t:?}");
            assert!(!t.contains("per hour") && !t.contains("/h"), "{t:?} must say per map hour");
        }
        // The same data behind the other tabs: not a word of it.
        for tab in [Tab::Mechanics, Tab::Movers] {
            market_ui::apply(&mut p, &Action::SetTab(tab));
            for t in texts(&p) {
                let t = t.to_lowercase();
                assert!(!t.contains("income") && !t.contains("profit"), "{t:?} on {tab:?}");
            }
        }
    }
}

#[test]
fn a_rate_has_its_sample_and_a_short_sample_has_no_rate() {
    let v = view(StashAccess::Ready);
    let ritual = v.mechanics.iter().find(|r| r.name == "Ritual").unwrap();
    assert!(ritual.has_rate);
    assert_eq!(ritual.income, "3.43 div per map hour, measured over 4 blocks, 12 maps, 2.3 h");
    let breach = v.mechanics.iter().find(|r| r.name == "Breach").unwrap();
    assert!(!breach.has_rate);
    assert_eq!(breach.income, "not enough runs yet (1 of 3 pure blocks, 2 of 10 maps)");
    assert_eq!(v.yours.keys().collect::<Vec<_>>(), ["Ritual"], "the suffix exists only where a rate does");
    assert_eq!(v.yours["Ritual"], "yours: 3.43 div per map hour (12 maps)");
    // The overall figure takes the mixed block too, never the pending one
    // nor the one whose map has no known length.
    // 8 div over the pure blocks and 3 over the mixed one, in 14 x 700 s.
    assert_eq!(v.overall.clone().unwrap(), ("4.04 div per map hour".to_string(), "measured over 5 blocks, 14 maps, 2.7 h".to_string()));
}

#[test]
fn every_my_runs_label_can_be_drawn_by_the_overlay_font() {
    let r = Renderer::new().unwrap();
    let mut states = vec![runs_panel(StashAccess::Ready), runs_panel(missing())];
    for shown in [Shown::Reading, Shown::LogMissing, Shown::Runs(Arc::new(View::default()))] {
        let mut p = runs_panel(missing());
        p.set_runs(shown);
        states.push(p);
    }
    for p in &states {
        for text in texts(p) {
            assert!(r.can_draw(&text), "{text:?} has a character the overlay font cannot draw");
        }
    }
    for text in [income::NO_BLOCKS, income::NO_COMPLETE_BLOCKS, myruns::NO_RUNS] {
        assert!(r.can_draw(text));
    }
}

fn painted(pm: &tiny_skia::Pixmap, x0: i32, y0: i32, x1: i32, y1: i32, test: impl Fn(u8, u8, u8) -> bool) -> usize {
    let mut n = 0;
    for y in y0.max(0)..y1.min(pm.height() as i32) {
        for x in x0.max(0)..x1.min(pm.width() as i32) {
            let px = pm.pixel(x as u32, y as u32).unwrap();
            if test(px.red(), px.green(), px.blue()) {
                n += 1;
            }
        }
    }
    n
}

#[test]
fn the_renderer_draws_the_tables_and_colours_only_settled_income() {
    let r = Renderer::new().unwrap();
    let measure = |s: &str| r.evaluate_label_width(s);
    let p = runs_panel(StashAccess::Ready);
    let lay = market_ui::layout(&p, &measure);
    let mut pm = tiny_skia::Pixmap::new(lay.w as u32, lay.h as u32).unwrap();
    r.draw_market(&mut pm, &p, &lay, (0, 0));
    let income_x = lay.tables[3][4];
    let revaluation_x = lay.tables[3][5];
    let green = |r: u8, g: u8, b: u8| g > 140 && g > r.saturating_add(25) && g > b.saturating_add(25);
    let any = |r: u8, g: u8, b: u8| u32::from(r) + u32::from(g) + u32::from(b) > 330;
    let mut seen = (false, false);
    for (rect, line) in &lay.lines {
        let Line::Cells { table: 3, cells } = line else { continue };
        let (y0, y1) = (rect.y, rect.y + rect.h as i32);
        match cells[4].1 {
            Ink::Up => {
                assert!(painted(&pm, income_x, y0, revaluation_x - 4, y1, green) > 10, "a gain is drawn green");
                seen.0 = true;
            }
            Ink::Dim if cells[4].0 == "pending" => {
                assert!(painted(&pm, income_x, y0, revaluation_x - 4, y1, any) > 10, "pending is written out");
                assert_eq!(painted(&pm, income_x, y0, revaluation_x - 4, y1, green), 0);
                assert_eq!(painted(&pm, revaluation_x, y0, lay.w - 12, y1, any), 0, "and has no figure beside it");
                seen.1 = true;
            }
            _ => {}
        }
    }
    assert_eq!(seen, (true, true));
    // Nothing is drawn outside the panel's own rectangle.
    assert!(lay.lines.iter().all(|(rect, _)| rect.y + rect.h as i32 <= lay.h));
}

#[test]
fn the_yours_suffix_sits_beside_a_mechanic_only_when_a_rate_exists() {
    use khaloni_poe2::prices::Market;
    use khaloni_poe2_core::market::{Floors, Source};
    use khaloni_poe2_core::ninja::ExchangeOverview;
    let overview = |json: &str| serde_json::from_str::<ExchangeOverview>(json).unwrap();
    let ritual = overview(include_str!("../../core/tests/fixtures/market/Essences.json"));
    let breach = overview(include_str!("../../core/tests/fixtures/market/Breach.json"));
    // The Essences fixture stands in as the Ritual category: the suffix is
    // matched by the category's name and nothing else.
    let market = Market {
        source: Source::from_overviews("L", &[("Ritual", &ritual), ("Breach", &breach)], &[]),
        fetched_at: Some(0),
        uniques_fetched_at: Some(0),
        ..Market::default()
    };
    let mut p = Panel::loading("L");
    let fresh = Freshness { stale: false, uniques_stale: false, age_minutes: Some(5), uniques_age_minutes: Some(5) };
    p.set_data("L", Some(Arc::new(market_ui::View::build(&market, Floors::default(), false, 1.0))), fresh);
    let basis = |p: &Panel, category: &str| {
        market_ui::layout(p, &m)
            .lines
            .into_iter()
            .find_map(|(_, l)| match l {
                Line::Group(g) if g.category == category => Some(g.basis),
                _ => None,
            })
            .unwrap()
    };
    assert!(!basis(&p, "Ritual").contains("yours"), "no runs, no suffix");
    let before = basis(&p, "Ritual");
    p.set_runs(Shown::Runs(view(StashAccess::Ready)));
    assert_eq!(basis(&p, "Ritual"), format!("{before} · yours: 3.43 div per map hour (12 maps)"));
    assert!(!basis(&p, "Breach").contains("yours"), "Breach has no rate: nothing at all, not a zero");
    p.set_runs(Shown::Runs(view(missing())));
    assert_eq!(basis(&p, "Ritual"), before);
}

// What feeds the tab.

fn log_line(clock: &str, rest: &str) -> String {
    format!("2026/09/01 {clock} 1234567 3ef23348 {rest}\n")
}

fn area(clock: &str, code: &str, seed: u64) -> String {
    log_line(clock, &format!("[DEBUG Client 356] Generating level 79 area \"{code}\" with seed {seed}"))
}

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("khalonipoe2-myruns-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn the_startup_read_takes_the_end_of_the_log_and_hands_over_mid_line() {
    let dir = temp("cold");
    let path = dir.join("Client.txt");
    let body = [
        area("10:00:00", "MapRavine", 1),
        area("10:05:00", "HideoutCanal", 1),
        area("10:06:00", "MapSteppe", 2),
    ]
    .concat();
    // The writer is in the middle of a line.
    std::fs::write(&path, format!("{body}2026/09/01 10:09:00 12345")).unwrap();
    let (tracker, end, rest) = cold_read(&path).unwrap();
    assert_eq!(end, std::fs::metadata(&path).unwrap().len());
    assert_eq!(rest, "2026/09/01 10:09:00 12345");
    let finished = tracker.finished();
    assert_eq!(finished.len(), 1, "the second map is still being played");
    assert_eq!(finished[0].seconds(), Some(300));

    // Past the bound only the end is read, and the line the cut fell into
    // is dropped rather than half-parsed.
    let filler = log_line("09:00:00", "[INFO Client 356] : Connecting to instance server").repeat(8);
    let mut big = area("08:00:00", "MapOld", 9);
    big.push_str(&area("08:05:00", "HideoutCanal", 1));
    while (big.len() as u64) < khaloni_poe2_core::runs::COLD_READ_BYTES + 4096 {
        big.push_str(&filler);
    }
    big.push_str(&body);
    big.push_str(&area("10:20:00", "HideoutCanal", 1));
    std::fs::write(&path, &big).unwrap();
    let (tracker, _, rest) = cold_read(&path).unwrap();
    assert!(rest.is_empty());
    let areas: Vec<&str> = tracker.finished().iter().map(|r| r.area.as_str()).collect();
    assert_eq!(areas, ["MapRavine", "MapSteppe"], "the map before the bound is not read");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_logs_clock_is_placed_against_utc_to_the_quarter_hour_or_not_at_all() {
    let t = timestamp("2026/09/01 12:00:03").unwrap();
    assert_eq!(clock_offset(t, t - 7200 - 3), Some(7200), "two hours ahead, three seconds of lag");
    assert_eq!(clock_offset(t, t + 5 * 3600 - 1800), Some(-5 * 3600 + 1800), "half-hour zones exist");
    assert_eq!(clock_offset(t, t), Some(0));
    assert_eq!(clock_offset(t, t - 7200 - 400), None, "seven minutes out is two different moments");
    assert_eq!(clock_offset(t, t - 40 * 3600), None, "no zone is forty hours ahead");
}

#[test]
fn the_hideout_snapshot_needs_a_map_a_minute_and_ten_minutes() {
    use wealth::extra_snapshot_due as due;
    let (min, s) = (Duration::from_secs(60), Duration::from_secs(1));
    assert!(due(Some(min), 11 * min, true));
    assert!(!due(Some(59 * s), 11 * min, true), "still walking to the stash");
    assert!(!due(None, 11 * min, true), "not in a hideout");
    assert!(!due(Some(min), 10 * min, true), "the last snapshot is too recent");
    assert!(!due(Some(30 * min), 25 * min, false), "idling in the hideout asks for nothing");

    let hub = Hub::new();
    let now = 1_800_000_000;
    hub.feed_live([area("10:00:00", "MapRavine", 1).trim_end()], now);
    assert_eq!(hub.in_hideout_for(), None);
    assert!(!hub.map_ended_since_snapshot());
    hub.feed_live([area("10:05:00", "HideoutCanal", 1).trim_end()], now);
    assert!(hub.in_hideout_for().is_some());
    assert!(hub.map_ended_since_snapshot());
    // A walk that began after that map ended covers it.
    hub.mark_snapshot(hub.intervals_closed());
    assert!(!hub.map_ended_since_snapshot());
    hub.feed_live([area("10:06:00", "MapRavine", 1).trim_end()], now);
    assert_eq!(hub.in_hideout_for(), None, "back in the map");
    assert_eq!(hub.finished_runs().len(), 0, "and that map is in progress again");
}

#[test]
fn old_total_only_lines_still_load_and_form_no_block() {
    let jsonl = concat!(
        r#"{"at_epoch_s":100,"total_ex":900.0,"league":"L"}"#,
        "\n",
        r#"{"at_epoch_s":200,"total_ex":950.0,"league":"L"}"#,
        "\n",
        r#"{"at_epoch_s":300,"total_ex":10.0,"league":"L","local_offset_s":-7200,"items":[["Divine Orb",2,1.0],["Old Base",1,null]]}"#,
        "\n",
        r#"{"at_epoch_s":400,"total_ex":10.0,"league":"Other","items":[["Divine Orb",9,1.0]]}"#,
        "\n",
        "a torn line\n",
        r#"{"at_epoch_s":500,"total_ex":15.0,"league":"L","items":[["Divine Orb",3,1.0],["Old Base",1,null]]}"#,
        "\n",
    );
    assert_eq!(wealth::series(jsonl, "L", 10).len(), 4, "the trend line keeps every total");
    let itemised = wealth::itemised_series(jsonl, "L", 3600);
    // Kept in the order they were taken, whatever the clocks said.
    assert_eq!(itemised.len(), 2, "only lines that say what they counted");
    assert_eq!(itemised[0].at, 300 - 7200, "placed by the offset stored with it");
    assert_eq!(itemised[1].at, 500 + 3600, "or by the current one when it has none");
    assert_eq!(itemised[0].items["Old Base"], Holding { qty: 1, price_div: None });
    let blocks = income::blocks(&itemised, &[]);
    assert_eq!(blocks.len(), 1);
    assert!((blocks[0].figures.unwrap().income_div - 1.0).abs() < 1e-9);
    assert_eq!(income::blocks(&wealth::itemised_series(jsonl, "Other", 0), &[]).len(), 0);
}

#[test]
fn a_snapshot_lists_what_it_counted_and_the_history_stays_bounded() {
    use khaloni_poe2_core::ninja::{ExchangeOverview, PriceTable, UniquePrices};
    use khaloni_poe2_core::stash::StashItem;
    let ov: ExchangeOverview =
        serde_json::from_str(include_str!("../../core/tests/fixtures/ninja_currency.json")).unwrap();
    let table = PriceTable::build(&[ov]);
    let prices = khaloni_poe2::prices::Snapshot {
        league: "L".into(),
        loading: false,
        error: None,
        vocab: khaloni_poe2::pricing::build_vocab(&table),
        table,
        uniques: UniquePrices::default(),
        stale: false,
        uniques_stale: false,
    };
    let item = |name: &str, n: u32| StashItem { type_line: name.into(), stack_size: n };
    let items = [item("Orb of Annulment", 4), item("Stellar Amulet", 1), item("Orb of Annulment", 6)];
    let lines = wealth::item_lines(&items, &prices, &["Chaos Orb".to_string(), "Stellar Amulet".to_string()]);
    let get = |name: &str| lines.iter().find(|(n, _, _)| n == name).cloned().unwrap();
    assert_eq!(get("Orb of Annulment").1, 10, "stacks in two tabs are one quantity");
    assert!((get("Orb of Annulment").2.unwrap() - 0.0325).abs() < 1e-9, "priced in divines");
    assert_eq!((get("Stellar Amulet").1, get("Stellar Amulet").2), (1, None), "counted, with no price made up");
    assert_eq!(get("Chaos Orb").1, 0, "held last time, gone now: listed at zero");
    assert!(get("Chaos Orb").2.is_some(), "with the price its loss is valued at");

    let dir = temp("history");
    let path = dir.join("wealth.jsonl");
    let snap = |at: u64| WealthSnapshot {
        at_epoch_s: at,
        total_ex: 1.0,
        league: "L".into(),
        local_offset_s: Some(0),
        items: Some(lines.clone()),
    };
    for at in 0..40 {
        wealth::append_to(&path, &snap(at), 2000).unwrap();
    }
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.len() < 4500, "cut back whenever it passes the cap: {}", text.len());
    let kept = wealth::itemised_series(&text, "L", 0);
    assert!(kept.len() >= 2 && kept.len() < 40);
    assert_eq!(kept.last().unwrap().at, 39, "the newest survive");
    assert!(text.lines().all(|l| serde_json::from_str::<WealthSnapshot>(l).is_ok()), "no line is cut in half");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_export_writes_runs_and_blocks_with_empty_cells_for_the_unknown() {
    let (runs, snaps) = sample();
    let summary = income::summarize(&runs, &snaps, 0, StashAccess::Ready);
    let loaded = myruns::Loaded { runs, summary };
    let dir = temp("export");
    let (runs_path, blocks_path) = myruns::export_csv(&dir.join("exports"), &loaded, "stamp").unwrap();
    let runs_csv = std::fs::read_to_string(&runs_path).unwrap();
    let blocks_csv = std::fs::read_to_string(&blocks_path).unwrap();
    assert_eq!(runs_csv.lines().count(), 1 + 15);
    assert!(runs_csv.starts_with("area,seed,started,ended,map_seconds,portals,mechanics_seen\n"));
    assert!(runs_csv.contains("MapSteppe,90,2024-10-04 03:21:40,2024-10-04 03:33:20,700,0,Breach Ritual\n"), "{runs_csv}");
    assert!(runs_csv.contains("MapSteppe,92,2024-10-04 04:11:40,2024-10-04 04:11:40,,0,\n"), "unknown is empty, not 0");
    assert_eq!(blocks_csv.lines().count(), 1 + 6);
    let last = blocks_csv.lines().last().unwrap();
    assert!(last.contains(",pending,") && last.ends_with(",,,"), "{last}");
    let _ = std::fs::remove_dir_all(&dir);
}
