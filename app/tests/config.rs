use poe2_lens::config::{Config, Rect};

#[test]
fn roundtrips_through_toml() {
    let mut c = Config::default();
    c.league = "Runes of Aldur".into();
    c.calibration = Some(Rect { x: 2600, y: 120, w: 900, h: 1100 });
    c.restore_token = Some("tok".into());
    let text = toml::to_string_pretty(&c).unwrap();
    let back: Config = toml::from_str(&text).unwrap();
    assert_eq!(back.league, "Runes of Aldur");
    assert_eq!(back.calibration.unwrap().w, 900);
    assert_eq!(back.restore_token.as_deref(), Some("tok"));
    assert!(back.pause_when_unfocused);
    assert!((back.divine_threshold - 1.0).abs() < f64::EPSILON);
}

#[test]
fn missing_fields_take_defaults() {
    let c: Config = toml::from_str("league = \"Standard\"").unwrap();
    assert_eq!(c.league, "Standard");
    assert!(c.calibration.is_none());
    assert_eq!(c.font_path, "/usr/share/fonts/TTF/DejaVuSans.ttf");
    assert_eq!(c.tier_good_ex, 10.0);
    assert_eq!(c.hotkey_price_check, "F7");
    assert_eq!(c.hotkey_overlay, "F8");
}

#[test]
fn hotkeys_are_remappable() {
    let c: Config =
        toml::from_str("league = \"Standard\"\nhotkey_price_check = \"F2\"\nhotkey_overlay = \"F3\"")
            .unwrap();
    assert_eq!(c.hotkey_price_check, "F2");
    assert_eq!(c.hotkey_overlay, "F3");
}
