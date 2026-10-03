//! The market panel's model, geometry and wording, on the same fixture
//! lines the core tests use (cut from the cached overviews of 2026-09-19),
//! plus a render pass through the real renderer.

use std::sync::Arc;

use khaloni_poe2::market_ui::{self, Action, Freshness, Line, Panel, Tab, Tone, View};
use khaloni_poe2::prices::Market;
use khaloni_poe2::render::Renderer;
use khaloni_poe2_core::market::{Floors, Kind, MarketItem, Source};
use khaloni_poe2_core::ninja::{ExchangeOverview, ItemOverview};

const ESSENCES: &str = include_str!("../../core/tests/fixtures/market/Essences.json");
const BREACH: &str = include_str!("../../core/tests/fixtures/market/Breach.json");
const ARMOURS: &str = include_str!("../../core/tests/fixtures/market/UniqueArmours.json");

/// Fixed-advance stand-in for the glyph measurer.
fn m(s: &str) -> i32 {
    7 * s.chars().count() as i32
}

fn market() -> Market {
    let essences: ExchangeOverview = serde_json::from_str(ESSENCES).unwrap();
    let breach: ExchangeOverview = serde_json::from_str(BREACH).unwrap();
    let armours: ItemOverview = serde_json::from_str(ARMOURS).unwrap();
    Market {
        source: Source::from_overviews(
            "Forbidden Rites",
            &[("Essences", &essences), ("Breach", &breach)],
            &[("UniqueArmours", &armours)],
        ),
        fetched_at: Some(0),
        uniques_fetched_at: Some(0),
        ..Market::default()
    }
}

fn view(young: bool) -> Arc<View> {
    Arc::new(View::build(&market(), Floors::default(), young, 1.0))
}

/// Both parts the same age, in minutes.
fn aged(minutes: i64, stale: bool) -> Freshness {
    Freshness { stale, uniques_stale: stale, age_minutes: Some(minutes), uniques_age_minutes: Some(minutes) }
}

fn panel() -> Panel {
    let mut p = Panel::loading("Forbidden Rites");
    p.set_data("Forbidden Rites", Some(view(false)), aged(10, false));
    p
}

fn click(p: &mut Panel, x: i32, y: i32) -> Option<Action> {
    let lay = market_ui::layout(p, &m);
    let action = market_ui::hit(&lay, x, y);
    if let Some(a) = &action {
        market_ui::apply(p, a);
    }
    action
}

fn items(p: &Panel) -> Vec<market_ui::ItemRow> {
    market_ui::layout(p, &m)
        .lines
        .into_iter()
        .filter_map(|(_, l)| if let Line::Item(r) = l { Some(r) } else { None })
        .collect()
}

#[test]
fn the_loading_state_says_so_and_has_no_rows() {
    let p = Panel::loading("Forbidden Rites");
    let lay = market_ui::layout(&p, &m);
    assert_eq!(lay.lines.len(), 1);
    assert_eq!(lay.lines[0].1, Line::Message("loading Forbidden Rites prices".into()));
    assert!(lay.group_hits.is_empty() && lay.back.is_none() && lay.footer.is_none());
    // A snapshot with nothing in it is still loading, not an empty market.
    let mut empty = Panel::loading("Forbidden Rites");
    empty.set_data("Forbidden Rites", Some(Arc::new(View::default())), Freshness::loading());
    assert!(empty.view.is_none());
    // The tabs and the close box still answer.
    assert_eq!(market_ui::hit(&lay, lay.close.x + 1, lay.close.y + 1), Some(Action::Close));
}

#[test]
fn the_header_names_the_part_that_is_old() {
    let mut p = panel();
    assert_eq!(p.status(), "Forbidden Rites · prices 10 min old");
    assert!(!p.greyed() && !p.uniques_greyed());
    // The unique prices refresh apart and can be hours behind a fresh
    // table: the header says which, and only the uniques' rows grey.
    let uniques_behind =
        Freshness { stale: false, uniques_stale: true, age_minutes: Some(10), uniques_age_minutes: Some(11 * 60) };
    p.set_data("Forbidden Rites", Some(view(false)), uniques_behind);
    assert_eq!(p.status(), "Forbidden Rites · prices 10 min old · uniques 11 h 00 min old (stale)");
    assert!(!p.greyed() && p.uniques_greyed(), "older than three hours is greyed, part by part");
    // Both equally old: said once, for the whole.
    p.set_data("Forbidden Rites", Some(view(false)), aged(181, true));
    assert_eq!(p.status(), "Forbidden Rites · prices 3 h 01 min old (stale)");
    assert!(p.greyed() && p.uniques_greyed());
    let unknown = Freshness { stale: false, uniques_stale: false, age_minutes: None, uniques_age_minutes: None };
    p.set_data("Forbidden Rites", Some(view(false)), unknown);
    assert_eq!(p.status(), "Forbidden Rites · prices age unknown");
    // A category that kept its last good lines is named.
    let mut old = market();
    old.old_categories = vec!["SoulCores".into(), "Breach".into()];
    p.set_data("Forbidden Rites", Some(Arc::new(View::build(&old, Floors::default(), false, 1.0))), aged(1, true));
    assert!(p.status().ends_with("(stale) · old: Soul Cores, Breach"), "{}", p.status());
}

#[test]
fn groups_are_listed_by_traded_volume_and_everything_stays_inside_the_panel() {
    let p = panel();
    let lay = market_ui::layout(&p, &m);
    let groups: Vec<&market_ui::GroupRow> =
        lay.lines.iter().filter_map(|(_, l)| if let Line::Group(g) = l { Some(g) } else { None }).collect();
    assert_eq!(groups.iter().map(|g| g.label.as_str()).collect::<Vec<_>>(), ["Breach", "Essences"]);
    assert_eq!(groups[0].volume, "10003 div");
    assert_eq!(groups[0].index, "+85%");
    assert!(groups[0].basis.starts_with("rising · index from 5 of 5 items (100% of the group's volume) · 3 up"));
    assert_eq!(lay.group_hits.len(), 2);
    for (rect, _) in &lay.lines {
        assert!(rect.x >= 0 && rect.x + rect.w as i32 <= lay.w && rect.y + rect.h as i32 <= lay.h);
    }
    let c = lay.cols;
    assert!(c.name < c.price && c.price < c.volume && c.volume < c.change && c.spark + market_ui::SPARK_W <= lay.w);
    assert_eq!(lay.footer.as_ref().unwrap().1, "traded volume: period not published by poe.ninja");
}

#[test]
fn clicks_resolve_tabs_group_rows_and_back() {
    let mut p = panel();
    let lay = market_ui::layout(&p, &m);
    let (rect, category) = lay.group_hits[1].clone();
    assert_eq!(click(&mut p, rect.x + 5, rect.y + 5), Some(Action::OpenGroup(category.clone())));
    assert_eq!(p.open.as_deref(), Some("Essences"));

    let open = market_ui::layout(&p, &m);
    assert!(open.group_hits.is_empty(), "item rows are not clickable");
    assert_eq!(market_ui::heading_text(&p), "Essences");
    let back = open.back.expect("an open group has a way back");
    assert_eq!(click(&mut p, back.x + 1, back.y + 1), Some(Action::Back));
    assert_eq!(p.open, None);

    let (movers_tab, _) = lay.tabs[1];
    assert_eq!(click(&mut p, movers_tab.x + 1, movers_tab.y + 1), Some(Action::SetTab(Tab::Movers)));
    assert_eq!(p.tab, Tab::Movers);
    assert!(market_ui::layout(&p, &m).group_hits.is_empty());
    // Dead space between controls resolves to nothing.
    assert_eq!(market_ui::hit(&lay, lay.w - 2, lay.h - 2), None);
    // A league change drops what was open: it named the old league's rows.
    p.open = Some("Essences".into());
    p.set_data("Runes of Aldur", None, Freshness::loading());
    assert_eq!((p.open.clone(), p.view.is_none()), (None, true));
}

#[test]
fn thin_items_sit_greyed_at_the_bottom_and_say_so() {
    let mut p = panel();
    market_ui::apply(&mut p, &Action::OpenGroup("Essences".into()));
    let rows = items(&p);
    assert_eq!(rows.len(), 10, "every essence is listed: no exchange item is untrusted");
    let tones: Vec<Tone> = rows.iter().map(|r| r.tone).collect();
    let first_thin = tones.iter().position(|t| *t == Tone::Thin).unwrap();
    assert!(tones[..first_thin].iter().all(|t| *t == Tone::Normal));
    assert!(tones[first_thin..].iter().all(|t| *t == Tone::Thin));
    assert!(rows[first_thin..].iter().all(|r| r.note.contains("thin market")));

    // Lesser Essence of Battle: liquid, two days of history. It says how
    // many days it had and shows no change, no band and no sparkline.
    let battle = rows.iter().find(|r| r.name == "Lesser Essence of Battle").unwrap();
    assert_eq!((battle.change.as_str(), battle.band.as_str()), ("", ""));
    assert_eq!(battle.note, "not enough history · 2 of 7 days");
    assert_eq!(battle.spark, [None; 7]);
    // Under one exalted on the exchange is a real price with a real week.
    let cheap = rows.iter().find(|r| r.name == "Greater Essence of the Mind").unwrap();
    assert_eq!(cheap.tone, Tone::Thin);
    assert!(!cheap.change.is_empty() && cheap.note.ends_with("thin market"));
}

#[test]
fn an_untrusted_unique_is_nowhere_in_the_overlay() {
    let mut p = panel();
    market_ui::apply(&mut p, &Action::SetTab(Tab::Movers));
    let rows = items(&p);
    for hidden in ["Lightning Coil", "Visage of Ayah", "Runemastered Cleric Vestments"] {
        assert!(!rows.iter().any(|r| r.name.contains(hidden)), "{hidden} is untrusted and must not rank");
    }
    assert!(rows.iter().all(|r| r.tone == Tone::Normal), "liquid only");
    assert!(market_ui::layout(&p, &m).footer.is_none());
}

#[test]
fn a_five_day_unique_says_so_among_the_movers() {
    let mut p = panel();
    market_ui::apply(&mut p, &Action::SetTab(Tab::Movers));
    let rows = items(&p);
    let proto = rows.iter().find(|r| r.name.starts_with("Doryani's Prototype")).unwrap();
    assert_eq!(proto.note, "falling · 5 of 7 days");
    assert_eq!((proto.change.as_str(), proto.band.as_str()), ("-77%", "+/-65% over 7 days"));
    assert_eq!(proto.volume, "53 listed");
    assert!(proto.listed);
    assert_eq!(proto.spark[2], None, "the lost days stay gaps in the line");
    // Waveshaper moved further, at three exalted: priced in whole orbs, so
    // it is not among the movers at all.
    assert!(!rows.iter().any(|r| r.name.starts_with("Waveshaper")));
    // Inside its own band, four days of history, thin: none of them ranks.
    for absent in ["Forgotten Warden", "Synthetic Four Days", "Temporalis"] {
        assert!(!rows.iter().any(|r| r.name.contains(absent)), "{absent}");
    }
    let sections: Vec<String> = market_ui::layout(&p, &m)
        .lines
        .into_iter()
        .filter_map(|(_, l)| if let Line::Section(s) = l { Some(s) } else { None })
        .collect();
    assert_eq!(sections, ["Risers", "Fallers"]);
}

#[test]
fn a_group_too_thin_to_index_shows_the_sentence_not_a_number() {
    let item = |name: &str, volume: f64, points: [Option<f64>; 7]| MarketItem {
        category: "Ritual".into(),
        id: name.into(),
        name: name.into(),
        base_type: None,
        corrupted: false,
        kind: Kind::Exchange,
        price_div: 1.0,
        volume_div: Some(volume),
        listings: None,
        points,
        own_exalted_rate: 474.2,
        own_chaos_rate: 8.32,
    };
    let week: [Option<f64>; 7] = std::array::from_fn(|i| Some(i as f64));
    let source = Source {
        league: "L".into(),
        items: vec![
            item("A", 100.0, week),
            item("B", 100.0, week),
            item("C", 100.0, week),
            item("Heavy", 500.0, [None, None, None, None, None, Some(0.0), Some(1.0)]),
        ],
        exalted_rate: 474.2,
        chaos_rate: 8.32,
        ..Source::default()
    };
    let v = View::build(&Market { source, ..Market::default() }, Floors::default(), false, 1.0);
    let ritual = &v.mechanics[0];
    assert_eq!(ritual.index, "");
    assert_eq!(ritual.spark, [None; 7]);
    assert!(ritual.basis.starts_with("too thin to index: 3 of 4 items hold 38% of the group's volume"), "{}", ritual.basis);
}

#[test]
fn a_young_league_lists_prices_and_says_why_nothing_is_ranked() {
    let mut p = Panel::loading("Forbidden Rites");
    p.set_data("Forbidden Rites", Some(view(true)), aged(1, false));
    assert!(p.status().ends_with("league too young for trends"));
    let lay = market_ui::layout(&p, &m);
    for (_, line) in &lay.lines {
        if let Line::Group(g) = line {
            assert_eq!((g.volume.as_str(), g.share.as_str(), g.index.as_str()), ("", "", ""));
        }
    }
    market_ui::apply(&mut p, &Action::OpenGroup("Breach".into()));
    let rows = items(&p);
    assert_eq!(rows.len(), 5);
    assert!(rows.iter().all(|r| !r.price.is_empty() && r.change.is_empty() && r.band.is_empty() && r.note.is_empty()));
    market_ui::apply(&mut p, &Action::SetTab(Tab::Movers));
    let lay = market_ui::layout(&p, &m);
    assert_eq!(lay.lines.len(), 1);
    assert_eq!(lay.lines[0].1, Line::Message("league too young for trends".into()));
}

fn every_state() -> Vec<Panel> {
    let mut states = vec![Panel::loading("Forbidden Rites")];
    for young in [false, true] {
        let mut p = Panel::loading("Forbidden Rites");
        p.set_data("Forbidden Rites", Some(view(young)), aged(240, true));
        states.push(p.clone());
        for category in ["Breach", "Essences"] {
            let mut open = p.clone();
            market_ui::apply(&mut open, &Action::OpenGroup(category.into()));
            states.push(open);
        }
        market_ui::apply(&mut p, &Action::SetTab(Tab::Movers));
        states.push(p);
    }
    states
}

#[test]
fn no_label_promises_earnings_or_names_a_period_for_the_volume() {
    for p in every_state() {
        for text in market_ui::all_text(&p, &m) {
            let t = text.to_lowercase();
            for banned in ["income", "profit", "per day", "per hour", "/hr"] {
                assert!(!t.contains(banned), "{text:?} contains {banned:?}");
            }
        }
    }
    assert!(market_ui::CAPTIONS.contains(&"traded volume") && market_ui::GROUP_CAPTIONS.contains(&"traded volume"));
}

#[test]
fn every_label_can_be_drawn_by_the_overlay_font() {
    // Fontin maps the plus-minus sign to a glyph with no outline: "±58%"
    // drew as " 58%" and read as a gain. Nothing the panel says may
    // contain a character that draws as nothing.
    let r = Renderer::new().unwrap();
    assert!(!r.can_draw("±"), "if the font gains the sign, the ASCII band can go");
    for p in every_state() {
        for text in market_ui::all_text(&p, &m) {
            assert!(r.can_draw(&text), "{text:?} has a character the overlay font cannot draw");
        }
    }
}

fn painted(pm: &tiny_skia::Pixmap, x0: i32, y0: i32, x1: i32, y1: i32, bright: bool) -> usize {
    let mut n = 0;
    for y in y0.max(0)..y1.min(pm.height() as i32) {
        for x in x0.max(0)..x1.min(pm.width() as i32) {
            let px = pm.pixel(x as u32, y as u32).unwrap();
            let lum = u32::from(px.red()) + u32::from(px.green()) + u32::from(px.blue());
            if (bright && lum > 600) || (!bright && lum > 330 && lum <= 600) {
                n += 1;
            }
        }
    }
    n
}

#[test]
fn the_renderer_greys_thin_rows_and_draws_gaps_as_gaps() {
    let r = Renderer::new().unwrap();
    let measure = |s: &str| r.evaluate_label_width(s);
    let mut p = panel();
    market_ui::apply(&mut p, &Action::OpenGroup("Essences".into()));
    let lay = market_ui::layout(&p, &measure);
    let mut pm = tiny_skia::Pixmap::new(lay.w as u32, lay.h as u32).unwrap();
    r.draw_market(&mut pm, &p, &lay, (0, 0));

    let name_cell = |want: Tone| {
        let (rect, _) = lay
            .lines
            .iter()
            .find(|(_, l)| matches!(l, Line::Item(row) if row.tone == want))
            .expect("a row of that tone");
        (lay.cols.name, rect.y, lay.cols.price - 4, rect.y + rect.h as i32)
    };
    let (x0, y0, x1, y1) = name_cell(Tone::Normal);
    assert!(painted(&pm, x0, y0, x1, y1, true) > 40, "a liquid row is drawn in the primary ink");
    let (x0, y0, x1, y1) = name_cell(Tone::Thin);
    assert_eq!(painted(&pm, x0, y0, x1, y1, true), 0, "a thin row has no primary ink in it");
    assert!(painted(&pm, x0, y0, x1, y1, false) > 40, "it is drawn, in the muted ink");

    // Lesser Essence of Battle has no trend: its sparkline cell is empty.
    let (rect, _) = lay
        .lines
        .iter()
        .find(|(_, l)| matches!(l, Line::Item(row) if row.name == "Lesser Essence of Battle"))
        .unwrap();
    let (sx, sw) = (lay.cols.spark, market_ui::SPARK_W);
    let cell = |pm: &tiny_skia::Pixmap, rect: &khaloni_poe2::config::Rect| {
        painted(pm, sx, rect.y + 2, sx + sw, rect.y + rect.h as i32 - 2, true)
            + painted(pm, sx, rect.y + 2, sx + sw, rect.y + rect.h as i32 - 2, false)
    };
    assert_eq!(cell(&pm, rect), 0, "no history, no line");
    let (full, _) = lay.lines.iter().find(|(_, l)| matches!(l, Line::Item(row) if row.spark[0].is_some())).unwrap();
    assert!(cell(&pm, full) > 20);

    // A five-day unique: nothing is drawn across the two lost days.
    market_ui::apply(&mut p, &Action::SetTab(Tab::Movers));
    let lay = market_ui::layout(&p, &measure);
    let mut pm = tiny_skia::Pixmap::new(lay.w as u32, lay.h as u32).unwrap();
    r.draw_market(&mut pm, &p, &lay, (0, 0));
    let (rect, _) = lay
        .lines
        .iter()
        .find(|(_, l)| matches!(l, Line::Item(row) if row.name.starts_with("Doryani's Prototype")))
        .unwrap();
    let step = market_ui::SPARK_W as f32 / 6.0;
    // Days 1..3 and 4..6 bracket the lost days 2 and 5; strictly between a
    // lost day's neighbours there must be no ink.
    for lost in [2.0f32, 5.0] {
        let (gx0, gx1) = (lay.cols.spark as f32 + step * (lost - 0.6), lay.cols.spark as f32 + step * (lost + 0.6));
        let ink = painted(&pm, gx0 as i32, rect.y, gx1 as i32, rect.y + rect.h as i32, true)
            + painted(&pm, gx0 as i32, rect.y, gx1 as i32, rect.y + rect.h as i32, false);
        assert_eq!(ink, 0, "the line was bridged across day {lost}");
    }

    // Loading and too-thin states draw without rows and without panicking.
    let loading = Panel::loading("Forbidden Rites");
    let lay = market_ui::layout(&loading, &measure);
    let mut pm = tiny_skia::Pixmap::new(lay.w as u32, lay.h as u32).unwrap();
    r.draw_market(&mut pm, &loading, &lay, (0, 0));
    let (rect, _) = &lay.lines[0];
    assert!(painted(&pm, rect.x, rect.y, rect.x + 300, rect.y + rect.h as i32, false) > 40, "the sentence is drawn");
}
