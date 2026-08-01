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
    let cases: [(CaptureTarget, fn(&Config) -> &String); 5] = [
        (CaptureTarget::PriceCheck, |c| &c.hotkey_price_check),
        (CaptureTarget::Overlay, |c| &c.hotkey_overlay),
        (CaptureTarget::Settings, |c| &c.hotkey_settings),
        (CaptureTarget::Reference, |c| &c.hotkey_reference),
        (CaptureTarget::Leveling, |c| &c.hotkey_leveling),
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
fn brightness_close_must_be_below_open() {
    let mut m = EditModel::from_config(Config::default());
    m.cfg.panel_close_brightness = 200;
    m.cfg.panel_open_brightness = 100;
    assert!(!m.brightness_valid());

    // Equal thresholds would make the gate oscillate: also invalid.
    m.cfg.panel_close_brightness = 100;
    assert!(!m.brightness_valid());

    m.cfg.panel_close_brightness = 99;
    assert!(m.brightness_valid());
}

#[test]
fn tier_ladder_order_enforced() {
    let mut m = EditModel::from_config(Config::default());
    m.cfg.tier_decent_ex = 50.0;
    m.cfg.tier_good_ex = 10.0;
    assert!(!m.tier_valid());

    // Equal thresholds collapse the decent band to nothing, which is legal.
    m.cfg.tier_good_ex = 50.0;
    assert!(m.tier_valid());

    m.cfg.tier_good_ex = 50.1;
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
