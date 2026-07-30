use khaloni_poe2::config::Config;

#[test]
fn roundtrips_through_toml() {
    let mut c = Config::default();
    c.league = "Runes of Aldur".into();
    c.restore_token = Some("tok".into());
    let text = toml::to_string_pretty(&c).unwrap();
    let back: Config = toml::from_str(&text).unwrap();
    assert_eq!(back.league, "Runes of Aldur");
    assert_eq!(back.restore_token.as_deref(), Some("tok"));
    assert!(back.pause_when_hidden);
    assert!((back.divine_threshold - 1.0).abs() < f64::EPSILON);
}

#[test]
fn missing_fields_take_defaults() {
    let c: Config = toml::from_str("league = \"Standard\"").unwrap();
    assert_eq!(c.league, "Standard");
    assert_eq!(c.tier_good_ex, 10.0);
    assert_eq!(c.hotkey_price_check, "F7");
    assert_eq!(c.hotkey_overlay, "F8");
    assert_eq!(c.hotkey_reference, "F9");
    assert_eq!(c.hotkey_leveling, "F10");
}

#[test]
fn dead_fields_are_gone_and_unknown_keys_ignored() {
    // Old configs still carry the removed keys; loading must not error and
    // saving must not resurrect them.
    let c: Config = toml::from_str(
        "league = \"X\"\nfont_path = \"/x\"\ntesseract_cmd = \"t\"\nmap_hotkey = \"F1\"",
    )
    .unwrap();
    assert_eq!(c.league, "X");
    let out = toml::to_string_pretty(&c).unwrap();
    assert!(!out.contains("font_path"));
    assert!(!out.contains("tesseract_cmd"));
    assert!(!out.contains("map_hotkey"));
}

#[test]
fn save_is_atomic_no_partial_file() {
    let dir = std::env::temp_dir().join(format!("khalonipoe2-atomic-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    khaloni_poe2::config::write_atomic(&path, "league = \"Y\"").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "league = \"Y\"");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "no temp litter");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn hotkeys_are_remappable() {
    let c: Config =
        toml::from_str("league = \"Standard\"\nhotkey_price_check = \"F2\"\nhotkey_overlay = \"F3\"")
            .unwrap();
    assert_eq!(c.hotkey_price_check, "F2");
    assert_eq!(c.hotkey_overlay, "F3");
}

#[test]
fn old_pause_key_still_loads() {
    // Pre-rename configs say pause_when_unfocused; the serde alias maps it.
    let c: Config = toml::from_str("league = \"X\"\npause_when_unfocused = false").unwrap();
    assert!(!c.pause_when_hidden);
}
