use khaloni_poe2::config::{Config, RestoreToken};

#[test]
fn defaults_to_the_current_league() {
    assert_eq!(Config::default().league, "Forbidden Rites");
}

#[test]
fn roundtrips_through_toml() {
    let c = Config { league: "Runes of Aldur".into(), tier_decent_chaos: 2.5, ..Config::default() };
    let text = toml::to_string_pretty(&c).unwrap();
    let back = Config::from_toml(&text).unwrap();
    assert_eq!(back.league, "Runes of Aldur");
    assert_eq!(back.tier_decent_chaos, 2.5);
    assert!(back.notices.is_empty(), "a current file loads without a word");
    assert!(back.pause_when_hidden);
    assert!((back.divine_threshold - 1.0).abs() < f64::EPSILON);
}

#[test]
fn missing_fields_take_defaults() {
    let c: Config = toml::from_str("league = \"Standard\"").unwrap();
    assert_eq!(c.league, "Standard");
    assert_eq!(c.tier_good_chaos, 10.0);
    assert_eq!(c.hotkey_price_check, "F7");
}

#[test]
fn an_old_config_with_the_reference_hotkey_loads_without_a_notice() {
    // Every file written before the reference panel was removed carries
    // its key. It loads, says nothing, binds nothing, and is not written
    // back.
    let c = Config::from_toml("league = \"X\"\nhotkey_reference = \"F9\"\n").unwrap();
    assert!(c.notices.is_empty(), "{:?}", c.notices);
    assert_eq!(c.hotkey_reference, "F9", "the key is accepted");
    let r = khaloni_poe2::bindings::resolve(&c);
    assert!(!r.bindings.extra.iter().any(|(id, key)| id == "reference" || key == "F9"), "{:?}", r.bindings);
    assert!(r.conflicts.is_empty());
    let saved = toml::to_string_pretty(&c).unwrap();
    assert!(!saved.contains("hotkey_reference"), "{saved}");
    assert!(!format!("{c:?}").contains("hotkey_reference"));
    // A file without the key is the same config.
    assert_eq!(Config::from_toml("league = \"X\"").unwrap().hotkey_reference, "");
}

#[test]
fn an_old_config_with_the_leveling_and_overlay_hotkeys_loads_without_a_notice() {
    // Files written before the leveling guide and the overlay on/off key
    // were removed carry both keys. They load, say nothing, bind nothing,
    // and are not written back.
    let c = Config::from_toml("league = \"X\"\nhotkey_overlay = \"F8\"\nhotkey_leveling = \"F10\"\n").unwrap();
    assert!(c.notices.is_empty(), "{:?}", c.notices);
    assert_eq!((c.hotkey_overlay.as_str(), c.hotkey_leveling.as_str()), ("F8", "F10"), "the keys are accepted");
    let r = khaloni_poe2::bindings::resolve(&c);
    assert!(
        !r.bindings.extra.iter().any(|(id, key)| id == "overlay-toggle" || id == "leveling" || key == "F8" || key == "F10"),
        "{:?}",
        r.bindings
    );
    assert!(r.conflicts.is_empty());
    let saved = toml::to_string_pretty(&c).unwrap();
    assert!(!saved.contains("hotkey_overlay") && !saved.contains("hotkey_leveling"), "{saved}");
    let shown = format!("{c:?}");
    assert!(!shown.contains("hotkey_overlay") && !shown.contains("hotkey_leveling"), "{shown}");
    // Loading such a file from disk leaves it as it was: nothing migrated,
    // so nothing is rewritten.
    let dir = std::env::temp_dir().join(format!("khalonipoe2-oldkeys-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    let text = "league = \"X\"\nhotkey_overlay = \"F8\"\nhotkey_leveling = \"F10\"\n";
    std::fs::write(&path, text).unwrap();
    assert!(Config::load_from(&path).unwrap().notices.is_empty());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    std::fs::remove_dir_all(&dir).unwrap();
    // A file without the keys is the same config.
    let fresh = Config::from_toml("league = \"X\"").unwrap();
    assert_eq!((fresh.hotkey_overlay.as_str(), fresh.hotkey_leveling.as_str()), ("", ""));
}

#[test]
fn gem_row_pricing_defaults_off() {
    assert!(!Config::default().price_gem_rows);
    assert!(!Config::from_toml("league = \"X\"").unwrap().price_gem_rows);
    let on = Config::from_toml("league = \"X\"\nprice_gem_rows = true").unwrap();
    assert!(on.price_gem_rows);
    let saved = toml::to_string_pretty(&on).unwrap();
    assert!(saved.contains("price_gem_rows = true"), "{saved}");
    assert!(format!("{:?}", Config::default()).contains("price_gem_rows: false"));
}

#[test]
fn no_reference_panel_module_or_hotkey_row_remains() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    for gone in ["app/src/reference_ui.rs", "app/tests/reference_ui.rs", "core/src/estimate.rs"] {
        assert!(!root.join(gone).exists(), "{gone} still exists");
    }
    // The settings window's hotkey rows and the binding claims name no
    // reference panel.
    let settings = std::fs::read_to_string(root.join("app/src/settings_ui.rs")).unwrap();
    assert!(!settings.contains("Reference search") && !settings.contains("CaptureTarget::Reference"));
    let bindings = std::fs::read_to_string(root.join("app/src/bindings.rs")).unwrap();
    assert!(!bindings.contains("Slot::Reference") && !bindings.contains("Action::Reference"));
    let lib = std::fs::read_to_string(root.join("app/src/lib.rs")).unwrap();
    assert!(!lib.contains("reference_ui"));
}

#[test]
fn no_leveling_module_or_hotkey_row_and_no_overlay_toggle_remain() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    for gone in ["app/src/leveling_ui.rs", "app/tests/leveling_ui.rs"] {
        assert!(!root.join(gone).exists(), "{gone} still exists");
    }
    let read = |rel: &str| std::fs::read_to_string(root.join(rel)).unwrap();
    // No hotkey row, binding claim, hotkey id or tray item for either.
    let settings = read("app/src/settings_ui.rs");
    for word in ["Leveling guide", "CaptureTarget::Leveling", "Overlay toggle", "CaptureTarget::Overlay"] {
        assert!(!settings.contains(word), "settings_ui.rs still has {word}");
    }
    let bindings = read("app/src/bindings.rs");
    for word in ["Slot::Leveling", "Action::Leveling", "Slot::Overlay", "Action::OverlayToggle", "overlay-toggle"] {
        assert!(!bindings.contains(word), "bindings.rs still has {word}");
    }
    for rel in ["app/src/platform/mod.rs", "app/src/platform/linux/hotkeys.rs", "app/src/platform/windows/hotkeys.rs"] {
        let text = read(rel);
        assert!(!text.contains("OverlayToggle") && !text.contains("overlay-toggle"), "{rel} still binds the overlay toggle");
    }
    let tray = read("app/src/tray.rs");
    assert!(!tray.contains("Toggle Overlay") && !tray.contains("ToggleOverlay"));
    let lib = read("app/src/lib.rs");
    assert!(!lib.contains("leveling_ui"));
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
        toml::from_str("league = \"Standard\"\nhotkey_price_check = \"F2\"\nhotkey_settings = \"F3\"")
            .unwrap();
    assert_eq!(c.hotkey_price_check, "F2");
    assert_eq!(c.hotkey_settings, "F3");
}

#[test]
fn old_pause_key_still_loads() {
    // Pre-rename configs say pause_when_unfocused; the serde alias maps it.
    let c: Config = toml::from_str("league = \"X\"\npause_when_unfocused = false").unwrap();
    assert!(!c.pause_when_hidden);
}

#[test]
fn overlay_opacity_defaults_opaque() {
    let c: Config = toml::from_str("league = \"X\"").unwrap();
    assert!((c.overlay_opacity - 1.0).abs() < f64::EPSILON);
}

#[cfg(unix)]
#[test]
fn the_config_file_is_created_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("khalonipoe2-mode-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    khaloni_poe2::config::write_atomic(&path, "league = \"Y\"\npoesessid = \"secret\"").unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    // Saving over a file that is already there keeps it owner-only.
    khaloni_poe2::config::write_atomic(&path, "league = \"Z\"").unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[cfg(unix)]
#[test]
fn loading_tightens_a_config_written_world_readable() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("khalonipoe2-chmod-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    std::fs::write(&path, "league = \"Y\"\npoesessid = \"secret\"").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let cfg = Config::load_from(&path).expect("loads");
    assert_eq!(cfg.poesessid, "secret");
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn concurrent_saves_do_not_share_a_temp_file() {
    let dir = std::env::temp_dir().join(format!("khalonipoe2-race-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    let body = |i: usize| format!("league = \"{}\"", "x".repeat(2000 + i));
    let writers: Vec<_> = (0..8)
        .map(|i| {
            let path = path.clone();
            std::thread::spawn(move || {
                for _ in 0..20 {
                    khaloni_poe2::config::write_atomic(&path, &body(i)).unwrap();
                }
            })
        })
        .collect();
    for w in writers {
        w.join().unwrap();
    }
    // Whichever save landed last, it landed whole.
    let text = std::fs::read_to_string(&path).unwrap();
    assert!((0..8).any(|i| text == body(i)), "torn config of {} bytes", text.len());
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "no temp litter");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn debug_output_never_carries_the_secrets() {
    let c = Config {
        poesessid: "0123456789abcdef0123456789abcdef".into(),
        restore_token: Some("portal-token-xyz".into()),
        ..Config::default()
    };
    let shown = format!("{c:?}");
    assert!(!shown.contains("0123456789abcdef"), "{shown}");
    assert!(!shown.contains("portal-token-xyz"), "{shown}");
    assert!(shown.contains("Forbidden Rites"), "the rest still prints");
}

#[test]
fn a_refresh_interval_below_the_floor_is_raised_on_load() {
    let c = Config::from_toml("league = \"X\"\nrefresh_minutes = 0").unwrap();
    assert_eq!(c.refresh_minutes, khaloni_poe2::config::MIN_REFRESH_MINUTES);
    let c = Config::from_toml("league = \"X\"\nrefresh_minutes = 30").unwrap();
    assert_eq!(c.refresh_minutes, 30);
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("khalonipoe2-cfg-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn exalted_era_tiers_reset_to_the_chaos_defaults_and_say_so() {
    // The owner's live values. Read as chaos they would make every reward
    // Junk; converted at ~55 ex per chaos, every reward Jackpot.
    let old = "league = \"Forbidden Rites\"\ntier_decent_ex = 5.0\ntier_good_ex = 25.0\n";
    let c = Config::from_toml(old).unwrap();
    assert_eq!((c.tier_decent_chaos, c.tier_good_chaos), (1.0, 10.0));
    assert_eq!(c.notices.len(), 1);
    assert!(c.notices[0].contains("chaos") && c.notices[0].contains("1 / 10"), "{}", c.notices[0]);
    // What is saved carries the new keys only, so the next load is quiet
    // and keeps whatever the user sets from here on.
    let saved = toml::to_string_pretty(&c).unwrap();
    assert!(saved.contains("tier_decent_chaos") && !saved.contains("tier_decent_ex"), "{saved}");
    let mut edited = Config::from_toml(&saved).unwrap();
    assert!(edited.notices.is_empty());
    edited.tier_good_chaos = 40.0;
    let again = Config::from_toml(&toml::to_string_pretty(&edited).unwrap()).unwrap();
    assert_eq!(again.tier_good_chaos, 40.0);
    // A hand-merged file with both generations keeps the chaos values.
    let both = "league = \"X\"\ntier_decent_ex = 5.0\ntier_decent_chaos = 3.0\ntier_good_chaos = 30.0\n";
    let c = Config::from_toml(both).unwrap();
    assert_eq!((c.tier_decent_chaos, c.tier_good_chaos), (3.0, 30.0));
    assert!(c.notices.is_empty());
}

#[test]
fn loading_an_old_file_migrates_it_on_disk_once() {
    let dir = temp_dir("migrate");
    let path = dir.join("config.toml");
    std::fs::write(
        &path,
        "league = \"Forbidden Rites\"\nrestore_token = \"portal-tok\"\ntier_decent_ex = 5.0\ntier_good_ex = 25.0\nhotkey_upgrade = \"F11\"\n",
    )
    .unwrap();
    let first = Config::load_from(&path).unwrap();
    assert_eq!(first.notices.len(), 1);
    assert_eq!(first.hotkey_upgrade, "F11", "everything else survives the rewrite");
    // The token moved to its own file before it left the config.
    assert_eq!(RestoreToken::beside(&path).load().as_deref(), Some("portal-tok"));
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("restore_token") && !text.contains("portal-tok"), "{text}");
    assert!(!text.contains("tier_decent_ex") && text.contains("tier_decent_chaos"), "{text}");
    // Second load: nothing left to migrate, nothing to announce.
    let second = Config::load_from(&path).unwrap();
    assert!(second.notices.is_empty() && second.restore_token.is_none());
    assert_eq!(second.hotkey_upgrade, "F11");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_settings_save_cannot_revert_the_capture_token() {
    // The settings window holds a config loaded a while ago and saves all
    // of it; the overlay stores a new token meanwhile. They are different
    // files now, so neither save undoes the other.
    let dir = temp_dir("token");
    let path = dir.join("config.toml");
    std::fs::write(&path, "league = \"Forbidden Rites\"\n").unwrap();
    let mut settings_copy = Config::load_from(&path).unwrap();
    let store = RestoreToken::beside(&path);
    store.save("token-from-this-session").unwrap();
    settings_copy.overlay_opacity = 0.5;
    settings_copy.save_to(&path).unwrap();
    assert_eq!(store.load().as_deref(), Some("token-from-this-session"));
    assert_eq!(Config::load_from(&path).unwrap().overlay_opacity, 0.5);
    // An existing token file wins over a stale one still in an old config.
    std::fs::write(&path, "league = \"X\"\nrestore_token = \"stale\"\n").unwrap();
    Config::load_from(&path).unwrap();
    assert_eq!(store.load().as_deref(), Some("token-from-this-session"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.join("capture-token")).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "the token grants screen capture: owner-only");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn hotkey_craft_defaults_unbound() {
    use khaloni_poe2::bindings::{resolve, Action, Slot};
    let cfg = Config::default();
    assert_eq!(cfg.hotkey_craft, "");
    assert!(!resolve(&cfg).actions.values().any(|a| *a == Action::Craft), "unbound binds nothing");
    // The planner's settings start at the planner's own defaults.
    assert_eq!(cfg.craft_runs, khaloni_poe2_core::craft::sim::DEFAULT_RUNS);
    assert_eq!(cfg.craft_give_up_times, khaloni_poe2_core::craft::sim::DEFAULT_MEDIAN_TIMES);
    assert_eq!(cfg.craft_observed_min, khaloni_poe2_core::craft::observed::MIN_LISTINGS);
    assert_eq!(cfg.craft_sim().runs, 20_000);

    // A file from before the planner loads quietly with those defaults.
    let old = Config::from_toml("league = \"Forbidden Rites\"\nhotkey_market = \"F11\"\nhotkey_reference = \"F9\"").unwrap();
    assert!(old.notices.is_empty(), "{:?}", old.notices);
    assert_eq!((old.hotkey_craft.as_str(), old.craft_runs), ("", 20_000));

    // Bound, it takes its key like the other panels, ahead of a macro.
    let cfg = Config {
        hotkey_craft: "CTRL+F7".into(),
        macros: vec![khaloni_poe2::config::Macro { key: "CTRL+F7".into(), message: "/hideout".into() }],
        ..Config::default()
    };
    let r = resolve(&cfg);
    assert_eq!(r.actions.get("craft"), Some(&Action::Craft));
    assert!(r.bindings.extra.contains(&("craft".to_string(), "CTRL+F7".to_string())));
    assert_eq!(r.conflicts.len(), 1);
    assert_eq!((r.conflicts[0].winner, r.conflicts[0].loser), (Slot::Craft, Slot::Macro(0)));
    assert!(r.conflicts[0].message.contains("Craft planner"), "{}", r.conflicts[0].message);
    // Two panels asking for one key: the earlier claim keeps it.
    let clash = Config { hotkey_market: "F10".into(), hotkey_craft: "F10".into(), ..Config::default() };
    let r = resolve(&clash);
    assert!(r.conflicts.iter().any(|c| c.winner == Slot::Market && c.loser == Slot::Craft));

    // Values out of range are held to it, not taken as written.
    let wild = Config::from_toml(
        "league = \"L\"\ncraft_runs = 0\ncraft_give_up_times = 0.5\ncraft_observed_min = 3",
    )
    .unwrap();
    assert_eq!(wild.craft_runs, khaloni_poe2::config::CRAFT_RUNS_RANGE.0);
    assert_eq!(wild.craft_give_up_times, khaloni_poe2::config::CRAFT_GIVE_UP_RANGE.0);
    assert_eq!(wild.craft_observed_min, khaloni_poe2::config::CRAFT_OBSERVED_MIN_RANGE.0);
    let shown = format!("{:?}", Config::default());
    assert!(shown.contains("hotkey_craft") && shown.contains("craft_runs"));
}
