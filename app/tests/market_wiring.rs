//! How the market view is wired into the app: the config keys, the hotkey
//! claim, the offline cache reader, the panel's data feed across a league
//! change, and the settings tab's sorting.

use std::sync::Arc;

use khaloni_poe2::bindings::{resolve, Action, Slot};
use khaloni_poe2::config::Config;
use khaloni_poe2::market_ui::{Feed, Panel};
use khaloni_poe2::prices::{market_from_cache, Market};
use khaloni_poe2::settings_market::{sort_rows, Column, Tables};
use khaloni_poe2_core::market::{Floors, Grade};
use khaloni_poe2_core::market_history::{Rates, Record};

const ESSENCES: &str = include_str!("../../core/tests/fixtures/market/Essences.json");
const BREACH: &str = include_str!("../../core/tests/fixtures/market/Breach.json");
const ARMOURS: &str = include_str!("../../core/tests/fixtures/market/UniqueArmours.json");

fn cache(name: &str, league: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("khalonipoe2-mktwire-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (typ, body) in [("Essences", ESSENCES), ("Breach", BREACH), ("UniqueArmours", ARMOURS)] {
        std::fs::write(dir.join(format!("{league}-{typ}.json")), body).unwrap();
    }
    dir
}

#[test]
fn the_market_hotkey_is_unbound_by_default_and_claimed_with_the_panels() {
    let cfg = Config::default();
    assert_eq!(cfg.hotkey_market, "");
    assert!(!resolve(&cfg).actions.values().any(|a| *a == Action::Market), "unbound binds nothing");

    // Bound, it outranks a macro on the same key, like the other panels.
    let cfg = Config {
        hotkey_market: "F11".into(),
        macros: vec![khaloni_poe2::config::Macro { key: "F11".into(), message: "/hideout".into() }],
        ..Config::default()
    };
    let r = resolve(&cfg);
    assert_eq!(r.actions.get("market"), Some(&Action::Market));
    assert!(r.bindings.extra.contains(&("market".to_string(), "F11".to_string())));
    assert_eq!(r.conflicts.len(), 1);
    assert_eq!((r.conflicts[0].winner, r.conflicts[0].loser), (Slot::Market, Slot::Macro(0)));
    assert!(r.conflicts[0].message.contains("Market panel"));
}

#[test]
fn the_floors_persist_and_unusable_ones_go_back_to_the_defaults_out_loud() {
    assert_eq!(Config::default().market_floors(), Floors::default());
    let kept = Config::from_toml(
        "league = \"L\"\nmarket_volume_floor_div = 1.5\nmarket_listings_rank = 30\nmarket_listings_min = 30\nhotkey_market = \"F11\"",
    )
    .unwrap();
    assert_eq!(kept.market_floors(), Floors { volume_div: 1.5, listings_rank: 30, listings_min: 30 });
    assert!(kept.notices.is_empty());
    // Round trip through the file's own format.
    let again = Config::from_toml(&toml::to_string_pretty(&kept).unwrap()).unwrap();
    assert_eq!((again.market_floors(), again.hotkey_market.as_str()), (kept.market_floors(), "F11"));

    for bad in [
        "market_volume_floor_div = -1.0",
        "market_listings_min = 0",
        "market_listings_rank = 0",
        "market_listings_min = 25",
    ] {
        let cfg = Config::from_toml(&format!("league = \"L\"\n{bad}")).unwrap();
        assert_eq!(cfg.market_floors(), Floors::default(), "{bad}");
        assert!(cfg.notices.iter().any(|n| n.contains("market floors reset")), "{bad}: {:?}", cfg.notices);
    }
    let shown = format!("{:?}", Config::default());
    assert!(shown.contains("market_volume_floor_div") && shown.contains("hotkey_market"));
}

#[test]
fn the_cached_overviews_are_read_without_a_request() {
    let dir = cache("offline", "Forbidden Rites");
    let market = market_from_cache(&dir, "Forbidden Rites");
    assert_eq!(market.source.items.len(), 26);
    assert_eq!(market.source.exalted_rate, 474.2);
    assert!(market.fetched_at.is_some(), "the files' own time is the prices' age");
    // Another league's files are not this league's market.
    assert!(market_from_cache(&dir, "Runes of Aldur").source.is_empty());
    // A file that does not parse is left out, not an error and not a guess.
    std::fs::write(dir.join("Forbidden Rites-Breach.json"), "<html>challenge</html>").unwrap();
    let partial = market_from_cache(&dir, "Forbidden Rites");
    assert_eq!(partial.source.items.len(), 21);
    assert!(!partial.source.items.iter().any(|i| i.category == "Breach"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_league_change_drops_the_rows_until_the_new_league_has_loaded() {
    let dir = cache("league", "Forbidden Rites");
    let rites = Arc::new(market_from_cache(&dir, "Forbidden Rites"));
    let now = rites.fetched_at.unwrap() + 120;
    let mut feed = Feed::default();
    let mut p = Panel::loading("Forbidden Rites");

    assert!(feed.refresh(&mut p, "Forbidden Rites", false, (false, false), &rites, Floors::default(), 1.0, now));
    let first = p.view.clone().expect("rows once the table has loaded");
    assert_eq!((p.fresh.age_minutes, p.fresh.uniques_age_minutes), (Some(2), Some(2)));
    // The same data again builds nothing and changes nothing.
    assert!(!feed.refresh(&mut p, "Forbidden Rites", false, (false, false), &rites, Floors::default(), 1.0, now));
    assert!(Arc::ptr_eq(&first, p.view.as_ref().unwrap()));
    // A changed floor rebuilds.
    let loose = Floors { volume_div: 1.0, ..Floors::default() };
    assert!(feed.refresh(&mut p, "Forbidden Rites", false, (false, false), &rites, loose, 1.0, now));
    assert!(!Arc::ptr_eq(&first, p.view.as_ref().unwrap()));

    // The switch: the snapshot already names the new league and is loading,
    // while the last market data published is still the old league's.
    p.open = Some("Breach".into());
    assert!(feed.refresh(&mut p, "Runes of Aldur", true, (true, true), &rites, Floors::default(), 1.0, now));
    assert_eq!((p.league.as_str(), p.view.is_none(), p.open.clone()), ("Runes of Aldur", true, None));
    // Even if the snapshot says loaded, another league's rows are refused.
    feed.refresh(&mut p, "Runes of Aldur", false, (false, false), &rites, Floors::default(), 1.0, now);
    assert!(p.view.is_none(), "Forbidden Rites rows under the Runes of Aldur name");
    let empty = Arc::new(Market::default());
    feed.refresh(&mut p, "Runes of Aldur", false, (false, false), &empty, Floors::default(), 1.0, now);
    assert!(p.view.is_none());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_settings_tables_sort_no_data_last_in_both_directions() {
    let dir = cache("tables", "Forbidden Rites");
    let market = market_from_cache(&dir, "Forbidden Rites");
    let now = market.fetched_at.unwrap();
    let tables = Tables::build(&market, Floors::default(), 1.0, &[], now);
    assert!(!tables.model.young, "seven days of figures prove the league is older than three");
    let essences = tables.model.groups.iter().position(|g| g.category == "Essences").unwrap();
    let mut rows = tables.group_rows[essences].clone();
    assert_eq!(rows.len(), 10);

    let without: Vec<String> = rows.iter().filter(|r| r.change.is_none()).map(|r| r.name.clone()).collect();
    assert!(without.contains(&"Lesser Essence of Battle".to_string()), "two days of history: no change");
    assert!(!without.contains(&"Greater Essence of the Mind".to_string()), "under 1 ex on the exchange is a real price");
    for descending in [true, false] {
        sort_rows(&mut rows, Column::Change, descending);
        let first_blank = rows.iter().position(|r| r.change.is_none()).unwrap();
        assert!(rows[first_blank..].iter().all(|r| r.change.is_none()), "blanks are not sorted as zeros");
        let figures: Vec<f64> = rows[..first_blank].iter().map(|r| r.change.unwrap()).collect();
        assert!(figures.windows(2).all(|w| if descending { w[0] >= w[1] } else { w[0] <= w[1] }));
    }
    // Untrusted uniques are listed apart, on request, with no trend.
    assert!(rows.iter().all(|r| r.grade != Grade::Untrusted));
    let names: Vec<&str> = tables.untrusted.iter().map(|r| r.name.as_str()).collect();
    assert!(names.contains(&"Lightning Coil, Ancestral Mail"));
    assert!(names.contains(&"The Mutable Star, Runemastered Cleric Vestments"));
    for u in &tables.untrusted {
        assert_eq!((u.change, u.band, u.points, u.day), (None, None, [None; 7], None), "{}", u.name);
    }
    assert_eq!(tables.model.floor_pinned, 2);

    // The log's windows appear only when records stand behind them, and say
    // how many.
    let at = |t: i64, price: f64| Record {
        t,
        source_t: t,
        rates: Rates { exalted: 474.2, chaos: 8.32 },
        items: vec![("Breach".into(), "breachstone".into(), price, Some(700.0))],
    };
    let records: Vec<Record> = (0..=24).map(|h| at(now - 86_400 + h * 3600, 2.0 + h as f64 * 0.02)).collect();
    let tables = Tables::build(&market, Floors::default(), 1.0, &records, now);
    let breach = tables.model.groups.iter().position(|g| g.category == "Breach").unwrap();
    let stone = tables.group_rows[breach].iter().find(|r| r.id == "breachstone").unwrap();
    let (pct, n) = stone.day.expect("a day of hourly records");
    assert!((pct - 24.0).abs() < 1e-9 && n == 25, "{pct} from {n}");
    assert_eq!(stone.three_days, None, "the log does not reach three days back");
    let _ = std::fs::remove_dir_all(dir);
}
