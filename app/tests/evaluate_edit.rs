//! Value-box editing on the Evaluate card: what a keypress does to the
//! text and what a commit does to the row and the query.

use khaloni_poe2::evaluate_ui::{
    commit_edit, edit_key, parse_edit, result_status, toggle_row, EditKey, EquipKey, Field,
    ItemHeader, Panel, RowGroup, StatRow, Strictness, Target,
};
use khaloni_poe2_core::trade::{Query, StatFilter};

fn stat_row(i: usize, min: Option<f64>) -> StatRow {
    StatRow {
        label: format!("mod {i}"),
        badge: None,
        score: None,
        min,
        max: None,
        enabled: true,
        target: Some(Target::Stat(i)),
        hidden: false,
        group: RowGroup::Explicit,
        note: None,
    }
}

fn panel(rows: Vec<StatRow>) -> Panel {
    Panel {
        header: ItemHeader { name: "Horror Bane".into(), rarity: "Rare".into(), item_level: None, requires_level: None, base: None },
        rows,
        ..Panel::default()
    }
}

fn query(mins: &[f64]) -> Query {
    Query {
        filters: mins.iter().enumerate().map(|(i, m)| StatFilter::at_least(format!("explicit.stat_{i}"), *m, false)).collect(),
        ..Default::default()
    }
}

#[test]
fn a_pending_edit_commits_the_same_way_enter_does() {
    // The user typed 40 into row 1's min and pressed Search without Enter:
    // the main loop commits with this same call before the search is sent,
    // so the query carries 40, not the 23 the box opened with.
    let mut p = panel(vec![stat_row(0, Some(10.0)), stat_row(1, Some(23.0))]);
    let mut q = query(&[10.0, 23.0]);
    let mut buf = String::new();
    for c in ['4', '0'] {
        edit_key(&mut buf, EditKey::Digit(c));
    }
    commit_edit(&mut p, &mut q, 1, Field::Min, &buf);
    assert_eq!(p.rows[1].min, Some(40.0));
    assert_eq!(q.filters[1].value.min, Some(40.0));
    assert_eq!(q.filters[0].value.min, Some(10.0), "other rows untouched");
    let body = q.to_body().to_string();
    assert!(body.contains("40"), "the search body carries the committed bound: {body}");

    // Max works the same, and an emptied box clears the bound.
    commit_edit(&mut p, &mut q, 1, Field::Max, "55.5");
    assert_eq!((p.rows[1].max, q.filters[1].value.max), (Some(55.5), Some(55.5)));
    commit_edit(&mut p, &mut q, 1, Field::Min, "");
    assert_eq!((p.rows[1].min, q.filters[1].value.min), (None, None));
}

#[test]
fn value_boxes_take_a_minus_and_a_decimal_point() {
    let mut buf = String::new();
    for k in [EditKey::Minus, EditKey::Digit('1'), EditKey::Digit('2'), EditKey::Dot, EditKey::Digit('5')] {
        edit_key(&mut buf, k);
    }
    assert_eq!(buf, "-12.5");
    assert_eq!(parse_edit(&buf), Some(-12.5));
    // A minus only leads, a point appears once, and a leading point or
    // "-." gets its zero.
    edit_key(&mut buf, EditKey::Minus);
    edit_key(&mut buf, EditKey::Dot);
    assert_eq!(buf, "-12.5");
    let mut b = String::new();
    edit_key(&mut b, EditKey::Dot);
    assert_eq!(b, "0.");
    assert_eq!(parse_edit(&b), Some(0.0));
    let mut b = String::from("-");
    edit_key(&mut b, EditKey::Dot);
    assert_eq!(b, "-0.");
    // Backspace always works, even at the length cap.
    let mut b = String::from("12345678");
    edit_key(&mut b, EditKey::Digit('9'));
    assert_eq!(b, "12345678");
    edit_key(&mut b, EditKey::Backspace);
    assert_eq!(b, "1234567");
    // Not a bound at all.
    assert_eq!(parse_edit(""), None);
    assert_eq!(parse_edit("-"), None);
}

#[test]
fn an_equipment_bound_follows_its_row() {
    let mut p = panel(vec![StatRow { target: Some(Target::Equipment(EquipKey::Dps)), ..stat_row(0, Some(300.0)) }]);
    let mut q = Query::default();
    commit_edit(&mut p, &mut q, 0, Field::Min, "350");
    assert_eq!(q.equipment.as_ref().and_then(|e| e.get(EquipKey::Dps)), Some(350.0));
    // Switched off, the bound leaves the query and the empty section with it.
    toggle_row(&mut p, &mut q, 0);
    assert!(!p.rows[0].enabled);
    assert!(q.equipment.is_none());
    // A switched-off row's edit is kept on the row and not searched.
    commit_edit(&mut p, &mut q, 0, Field::Min, "400");
    assert_eq!(p.rows[0].min, Some(400.0));
    assert!(q.equipment.is_none());
    toggle_row(&mut p, &mut q, 0);
    assert_eq!(q.equipment.as_ref().and_then(|e| e.get(EquipKey::Dps)), Some(400.0));
}

#[test]
fn toggling_a_stat_row_writes_the_filter_too() {
    let mut p = panel(vec![stat_row(0, Some(10.0))]);
    let mut q = query(&[10.0]);
    toggle_row(&mut p, &mut q, 0);
    assert!(!p.rows[0].enabled && q.filters[0].disabled);
    toggle_row(&mut p, &mut q, 0);
    assert!(p.rows[0].enabled && !q.filters[0].disabled);
}

#[test]
fn the_status_names_a_broad_search_and_the_size_of_the_result() {
    assert_eq!(result_status(8, Some(243), Strictness::Quick), "8 of 243 shown");
    assert_eq!(result_status(3, Some(3), Strictness::Quick), "3 shown");
    assert_eq!(result_status(3, None, Strictness::Quick), "3 shown");
    let broad = result_status(8, Some(243), Strictness::Broad);
    assert!(broad.contains("Broad") && broad.contains("-10%") && broad.contains("8 of 243"), "{broad}");
}

#[test]
fn a_long_status_widens_the_card_instead_of_running_off_it() {
    let measure = |s: &str| 8 * s.len() as i32;
    let mut p = panel(vec![stat_row(0, Some(10.0))]);
    let narrow = khaloni_poe2::evaluate_ui::layout(&p, &measure).size.0;
    p.status = "Broad search, bounds -10%: 8 of 243 shown (same search 12s ago) - league set to Standard: restart to apply".into();
    let lay = khaloni_poe2::evaluate_ui::layout(&p, &measure);
    assert!(lay.size.0 > narrow);
    assert!(lay.status_pos.0 + measure(&p.status) <= lay.size.0, "the status ends inside the card");
}

/// A real item from the EE2 corpus through the card as the app builds it:
/// every line EE2 folded into a total is a row, unticked, and the search
/// stays EE2's golden body until one is ticked; ticked, it adds exactly
/// that line's filter, Broad relaxes it with the rest, and unticked it is
/// gone again.
#[test]
fn a_folded_line_ticked_on_the_card_joins_the_search_and_nothing_else_moves() {
    use khaloni_poe2::evaluate_ui::{extra_rows, search_query};
    use khaloni_poe2_core::ee2::{self, data, Ee2Data};
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tools/ee2-parity");
    let read = |p: std::path::PathBuf| std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    let mut db = Ee2Data::from_ndjson(&read(dir.join("data/stats.ndjson")), &read(dir.join("data/items.ndjson"))).unwrap();
    db.trade_stats = Some(data::TradeStatTexts::from_json(&read(dir.join("data/trade-stats.json"))).unwrap());
    db.trade_items = Some(data::trade_item_names(&read(dir.join("data/trade-items.json"))).unwrap());
    let catalog = khaloni_poe2_core::trade::StatIndex::from_json(&read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../core/tests/fixtures/allmods/trade_stats.json"),
    ))
    .unwrap();
    let mut built = ee2::build(&read(dir.join("items/new-belt-life-resists-attributes.txt")), &db).unwrap();
    built.resolve_extra(&catalog);
    let golden: serde_json::Value = serde_json::from_str(&read(dir.join("golden/new-belt-life-resists-attributes.json"))).unwrap();

    let mut extras = Vec::new();
    let rows = extra_rows(&built.extra, &mut extras);
    let mut p = Panel { extras, ..panel(rows) };
    let mut q = built.query.clone();
    assert_eq!(search_query(&p, &q).to_body(), built.query.to_body());
    let canonical = |b: &serde_json::Value| {
        let mut b = b.clone();
        for g in b["query"]["stats"].as_array_mut().unwrap() {
            g["filters"].as_array_mut().unwrap().sort_by_key(|f| f.to_string());
        }
        b["query"]["stats"].as_array_mut().unwrap().sort_by_key(|g| g.to_string());
        b
    };
    assert_eq!(canonical(&search_query(&p, &q).to_body()), canonical(&golden["body"]), "unticked, the search is EE2's");

    let life = p.rows.iter().position(|r| r.label == "+142 to maximum Life").expect("the life line is a row");
    assert_eq!(p.rows[life].note.as_deref(), Some("counted in Total Life"));
    assert!(!p.rows[life].enabled && matches!(p.rows[life].target, Some(Target::Extra(_))));
    toggle_row(&mut p, &mut q, life);
    assert_eq!(q, built.query, "ticking an extra row writes nothing into EE2's query");
    let searched = search_query(&p, &q);
    assert_eq!(searched.filters.len(), built.query.filters.len() + 1);
    let added = searched.filters.last().unwrap();
    assert_eq!((added.id.as_str(), added.disabled, added.value.min), ("explicit.stat_3299347043", false, p.rows[life].min));
    let broad = khaloni_poe2_core::trade::relax_query(&searched, 0.10);
    assert!(broad.filters.last().unwrap().value.min < added.value.min, "Broad relaxes the added row too");
    toggle_row(&mut p, &mut q, life);
    assert_eq!(canonical(&search_query(&p, &q).to_body()), canonical(&golden["body"]));
}
