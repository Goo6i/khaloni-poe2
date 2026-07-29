use poe2_lens::config::{Config, Macro, ResourceShortcut};
use poe2_lens::settings_ui::{CaptureTarget, EditModel};

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
