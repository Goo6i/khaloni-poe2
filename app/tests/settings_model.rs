use khaloni_poe2::config::{Config, Macro, ResourceShortcut};
use khaloni_poe2::settings_ui::{CaptureTarget, EditModel};

#[test]
fn key_capture_writes_the_right_binding() {
    let mut m = EditModel::from_config(Config::default());
    assert!(!m.dirty, "fresh model must start clean");
    m.apply_key(CaptureTarget::PriceCheck, "F5".into());
    assert_eq!(m.cfg.hotkey_price_check, "F5");
    assert!(m.dirty);
}

#[test]
fn key_capture_covers_every_fixed_hotkey() {
    // Each target must land in its own Config field, not a neighbor's.
    type Field = fn(&Config) -> &String;
    let cases: [(CaptureTarget, Field); 4] = [
        (CaptureTarget::PriceCheck, |c| &c.hotkey_price_check),
        (CaptureTarget::Settings, |c| &c.hotkey_settings),
        (CaptureTarget::Market, |c| &c.hotkey_market),
        (CaptureTarget::Upgrade, |c| &c.hotkey_upgrade),
    ];
    for (i, (target, field)) in cases.into_iter().enumerate() {
        let mut m = EditModel::from_config(Config::default());
        let key = format!("CTRL+{i}");
        m.apply_key(target, key.clone());
        assert_eq!(field(&m.cfg), &key, "target {target:?}");
        assert!(m.dirty, "target {target:?} must mark the model dirty");
    }
}

#[test]
fn key_capture_writes_macro_and_shortcut_rows() {
    let mut m = EditModel::from_config(Config::default());
    m.cfg.macros.push(Macro {
        key: String::new(),
        message: "wtb".into(),
    });
    m.cfg.resource_shortcuts.push(ResourceShortcut {
        key: String::new(),
        url: "https://poe2db.tw/us/search?q={name}".into(),
    });
    m.dirty = false;

    m.apply_key(CaptureTarget::Macro(0), "CTRL+1".into());
    assert_eq!(m.cfg.macros[0].key, "CTRL+1");
    assert_eq!(m.cfg.macros[0].message, "wtb", "message must survive rebinding");
    assert!(m.dirty);

    m.apply_key(CaptureTarget::Shortcut(0), "CTRL+2".into());
    assert_eq!(m.cfg.resource_shortcuts[0].key, "CTRL+2");
}

#[test]
fn key_capture_out_of_range_row_is_a_no_op() {
    // The UI can race a delete against a pending capture; stale indices
    // must not panic or dirty the model.
    let mut m = EditModel::from_config(Config::default());
    m.apply_key(CaptureTarget::Macro(3), "F5".into());
    m.apply_key(CaptureTarget::Shortcut(0), "F6".into());
    assert!(!m.dirty);
}

#[test]
fn tier_ladder_order_enforced() {
    let mut m = EditModel::from_config(Config::default());
    m.cfg.tier_decent_chaos = 50.0;
    m.cfg.tier_good_chaos = 10.0;
    assert!(!m.tier_valid());

    // Equal thresholds collapse the decent band to nothing, which is legal.
    m.cfg.tier_good_chaos = 50.0;
    assert!(m.tier_valid());

    m.cfg.tier_good_chaos = 50.1;
    assert!(m.tier_valid());
}

#[test]
fn stash_regex_assembles_from_a_selection() {
    // A selection over the checkbox options must compose exactly like the
    // core helper: escaped, '#' widened to \d+, OR-joined in option order.
    let user = vec!["monsters gain # to # added damage".to_string()];
    let options = khaloni_poe2::settings_ui::stash_needle_options(&user);
    let picked: Vec<String> = options
        .into_iter()
        .filter(|n| n == "pack size" || n.contains("added damage"))
        .collect();
    // Built-in "pack size" sorts before the appended user needle.
    assert_eq!(picked, ["pack size", "monsters gain # to # added damage"]);
    assert_eq!(
        khaloni_poe2_core::mapmods::regex_for_needles(&picked),
        r"pack size|monsters gain \d+ to \d+ added damage"
    );
}

#[test]
fn stash_needle_options_union_built_ins_and_user_needles() {
    let user = vec![
        "pack size".to_string(),        // duplicates a built-in: dropped
        "Pack Size".to_string(),        // case-insensitive duplicate: dropped
        "  ".to_string(),               // blank row mid-typing: dropped
        "my custom needle".to_string(), // genuinely new: appended
    ];
    let options = khaloni_poe2::settings_ui::stash_needle_options(&user);
    // Every built-in Good needle appears exactly once.
    let good: Vec<String> = khaloni_poe2_core::mapmods::default_rules()
        .into_iter()
        .filter(|r| r.kind == khaloni_poe2_core::mapmods::ModKind::Good)
        .map(|r| r.needle)
        .collect();
    assert_eq!(&options[..good.len()], &good[..]);
    assert_eq!(&options[good.len()..], ["my custom needle".to_string()]);
}

#[test]
fn stash_regex_length_gate_is_exactly_50_chars() {
    use khaloni_poe2::settings_ui::{stash_regex_too_long, STASH_SEARCH_LIMIT};
    assert_eq!(STASH_SEARCH_LIMIT, 50);
    assert!(!stash_regex_too_long(""));
    assert!(!stash_regex_too_long(&"a".repeat(50)), "50 chars fits the game field");
    assert!(stash_regex_too_long(&"a".repeat(51)), "51 chars gets truncated in-game");
    // Chars, not bytes: 50 two-byte chars must still fit.
    assert!(!stash_regex_too_long(&"é".repeat(50)));
}

#[test]
fn mod_suggestions_rank_tightest_first_and_require_all_tokens() {
    let mods: Vec<String> = [
        "monsters deal #% of their damage as extra fire damage",
        "monsters deal #% of their damage as extra cold damage",
        "#% increased pack size",
        "monsters have #% increased attack speed",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let hits = khaloni_poe2::settings_ui::mod_suggestions(&mods, "extra fire", 8);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].contains("extra fire"));
    // All tokens required: "extra pack" matches nothing.
    assert!(khaloni_poe2::settings_ui::mod_suggestions(&mods, "extra pack", 8).is_empty());
    // Shorter (tighter) texts rank first.
    let hits = khaloni_poe2::settings_ui::mod_suggestions(&mods, "monsters", 8);
    assert_eq!(hits[0], "monsters have #% increased attack speed");
    // Empty query suggests nothing.
    assert!(khaloni_poe2::settings_ui::mod_suggestions(&mods, "  ", 8).is_empty());
}

#[test]
fn craft_settings_round_trip() {
    use khaloni_poe2::settings_craft::ProfilesDoc;
    use khaloni_poe2_core::flip::{Profile, Wanted};

    // The planner's settings and the craft hotkey survive the file.
    let mut m = EditModel::from_config(Config::default());
    m.apply_key(CaptureTarget::Craft, "CTRL+F7".into());
    assert_eq!(m.cfg.hotkey_craft, "CTRL+F7");
    assert!(m.dirty);
    m.cfg.craft_runs = 5_000;
    m.cfg.craft_give_up_times = 6.5;
    m.cfg.craft_observed_min = 400;
    let back = Config::from_toml(&toml::to_string_pretty(&m.cfg).unwrap()).unwrap();
    assert_eq!(back.hotkey_craft, "CTRL+F7");
    assert_eq!((back.craft_runs, back.craft_give_up_times, back.craft_observed_min), (5_000, 6.5, 400));
    assert!(back.notices.is_empty());
    let sim = back.craft_sim();
    assert_eq!(sim.runs, 5_000);
    assert_eq!(sim.cap, khaloni_poe2_core::craft::sim::Cap::MedianTimes(6.5));

    // The profiles file: a good profile, a malformed one and a stray key.
    let text = r#"
note = "kept as written"

[[profile]]
name = "Life boots"
class = "Boots"
min_ilvl = 75
wants = [{ family = "IncreasedLife", min_tier = 3 }, { family = "MovementVelocity", min_tier = 2 }]
margin = 25.0

[[profile]]
name = "Broken"
class = "Body Armour"
wants = [{ family = "IncreasedLife", min_tier = 0 }]
"#;
    let mut doc = ProfilesDoc::parse(text);
    assert_eq!(doc.profiles.len(), 1);
    assert_eq!(doc.profiles[0].name, "Life boots");
    assert_eq!(doc.profiles[0].wants[1], Wanted { family: "MovementVelocity".into(), min_tier: 2 });
    // The malformed profile is reported with its reason, not dropped.
    assert_eq!(doc.errors.len(), 2, "{:?}", doc.errors);
    let broken = doc.errors.iter().find(|e| e.index == Some(1)).expect("the broken profile is named");
    assert!(broken.to_string().contains("min_tier is 0"), "{broken}");

    // Edited and written back, the good profile changes and the broken one
    // and the stray key come back exactly as they were.
    doc.profiles[0].margin = 40.0;
    doc.profiles.push(Profile {
        name: "Armour cuirass".into(),
        class: "Body Armour".into(),
        base: Some("Soldier Cuirass".into()),
        min_ilvl: 80,
        wants: vec![Wanted { family: "LocalPhysicalDamageReductionRating".into(), min_tier: 2 }],
        margin: 30.0,
    });
    let written = doc.to_text().expect("the good profiles write");
    let again = ProfilesDoc::parse(&written);
    assert_eq!(again.profiles.len(), 2);
    assert_eq!(again.profiles[0].margin, 40.0);
    assert_eq!(again.profiles[1].base.as_deref(), Some("Soldier Cuirass"));
    assert!(again.errors.iter().any(|e| e.name.as_deref() == Some("Broken") && e.reason.contains("min_tier is 0")));
    assert!(written.contains("note = \"kept as written\""), "{written}");
    // Through the file on disk, with the overlay's own reader.
    let dir = std::env::temp_dir().join(format!("khalonipoe2-profiles-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = khaloni_poe2::craft_flow::profiles_path(&dir);
    again.save(&path).unwrap();
    let loaded = khaloni_poe2::craft_flow::load_profiles(&path);
    assert_eq!(loaded.profiles, again.profiles);
    assert_eq!(ProfilesDoc::load(&path).profiles, again.profiles);

    // An edit that breaks a profile is refused before anything is written.
    let mut bad = again.clone();
    bad.profiles[1].wants.clear();
    let err = bad.to_text().unwrap_err();
    assert!(err.contains("Armour cuirass") && err.contains("wants no modifier"), "{err}");
    let mut twice = again.clone();
    twice.profiles[1].name = "Life boots".into();
    assert!(twice.to_text().unwrap_err().contains("same name"));
    // A file that is not TOML is never written over.
    let garbled = ProfilesDoc::parse("[[profile]\nname = ");
    assert!(garbled.file_error.is_some());
    assert!(garbled.to_text().is_err());

    // A missing file is no profiles and no error.
    let none = khaloni_poe2::craft_flow::load_profiles(&dir.join("absent.toml"));
    assert!(none.profiles.is_empty() && none.errors.is_empty());

    // A scan asked for in Settings reaches the overlay once.
    khaloni_poe2::craft_flow::request_scan(&dir, "Life boots").unwrap();
    assert_eq!(khaloni_poe2::craft_flow::take_scan_request(&dir).as_deref(), Some("Life boots"));
    assert_eq!(khaloni_poe2::craft_flow::take_scan_request(&dir), None);
    let _ = std::fs::remove_dir_all(dir);
}
