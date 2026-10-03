//! Who gets a contested hotkey, and what a bound id fires.

use khaloni_poe2::bindings::{conflict_for, resolve, Action, Slot};
use khaloni_poe2::config::{Config, Macro, ResourceShortcut};

fn cfg_with(macros: &[(&str, &str)], shortcuts: &[(&str, &str)]) -> Config {
    Config {
        macros: macros.iter().map(|(k, m)| Macro { key: k.to_string(), message: m.to_string() }).collect(),
        resource_shortcuts: shortcuts
            .iter()
            .map(|(k, u)| ResourceShortcut { key: k.to_string(), url: u.to_string() })
            .collect(),
        ..Config::default()
    }
}

#[test]
fn panels_claim_their_key_before_macros_and_shortcuts() {
    // A wiki shortcut on the key the market panel is bound to.
    let mut c = cfg_with(&[("CTRL+1", "/hideout")], &[("F10", "https://www.poe2wiki.net/index.php?search={name}")]);
    c.hotkey_market = "F10".into();
    let r = resolve(&c);
    let market = r.bindings.extra.iter().find(|(id, _)| id == "market").expect("the market panel stays bound");
    assert_eq!(market.1, "F10");
    assert_eq!(r.actions.get("market"), Some(&Action::Market));
    assert!(
        !r.actions.values().any(|a| matches!(a, Action::Shortcut(_))),
        "the shortcut lost F10 and is not bound at all"
    );
    assert_eq!(r.conflicts.len(), 1);
    let conflict = &r.conflicts[0];
    assert_eq!((conflict.loser, conflict.winner), (Slot::Shortcut(0), Slot::Market));
    assert!(conflict.message.contains("F10") && conflict.message.contains("Market panel"), "{}", conflict.message);
    // The settings window marks the losing row, and only that one.
    assert!(conflict_for(&r.conflicts, Slot::Shortcut(0)).is_some());
    assert!(conflict_for(&r.conflicts, Slot::Market).is_none());
    assert!(conflict_for(&r.conflicts, Slot::Macro(0)).is_none());
}

#[test]
fn the_keys_of_removed_features_are_free_for_other_actions() {
    // The owner's live config: a wiki shortcut on F10, the leveling guide's
    // old default, from a file that still holds the old F8 and F10 keys.
    let mut c = Config::from_toml("league = \"X\"\nhotkey_overlay = \"F8\"\nhotkey_leveling = \"F10\"\n").unwrap();
    c.hotkey_market = "F8".into();
    c.resource_shortcuts =
        vec![ResourceShortcut { key: "F10".into(), url: "https://www.poe2wiki.net/index.php?search={name}".into() }];
    let r = resolve(&c);
    assert!(r.conflicts.is_empty(), "{:?}", r.conflicts);
    assert!(r.bindings.extra.contains(&("market".to_string(), "F8".to_string())));
    let (id, _) = r.bindings.extra.iter().find(|(_, key)| key == "F10").expect("the shortcut is bound on F10");
    assert!(matches!(r.actions.get(id), Some(Action::Shortcut(_))));
}

#[test]
fn built_ins_win_over_everything_and_case_does_not_matter() {
    let mut c = cfg_with(&[("f7", "/oops")], &[]);
    c.hotkey_market = "f12".into(); // collides with the settings panel
    // The removed reference panel's key claims nothing, whatever it holds.
    c.hotkey_reference = "F9".into();
    let r = resolve(&c);
    assert_eq!(r.bindings.price_check, "F7");
    assert!(r.bindings.extra.contains(&("settings".to_string(), "F12".to_string())));
    assert!(!r.bindings.extra.iter().any(|(id, _)| id == "market" || id == "reference"));
    assert!(!r.bindings.extra.iter().any(|(_, key)| key == "F9"));
    let losers: Vec<Slot> = r.conflicts.iter().map(|c| c.loser).collect();
    assert_eq!(losers, [Slot::Market, Slot::Macro(0)]);
}

#[test]
fn a_reordered_macro_list_cannot_fire_a_different_message() {
    let before = cfg_with(&[("CTRL+1", "/hideout"), ("CTRL+2", "thanks!")], &[]);
    let after = cfg_with(&[("CTRL+2", "thanks!"), ("CTRL+1", "/hideout")], &[]);
    let (a, b) = (resolve(&before), resolve(&after));
    let id_of = |r: &khaloni_poe2::bindings::Resolved, msg: &str| {
        r.actions
            .iter()
            .find(|(_, a)| **a == Action::Macro(msg.to_string()))
            .map(|(id, _)| id.clone())
            .expect("bound")
    };
    // The id belongs to the macro, not to its position in the list: a key
    // press from the set bound before the reorder resolves to the same
    // message in the set bound after it.
    assert_eq!(id_of(&a, "/hideout"), id_of(&b, "/hideout"));
    assert_eq!(id_of(&a, "thanks!"), id_of(&b, "thanks!"));
    assert_ne!(id_of(&a, "/hideout"), id_of(&a, "thanks!"));
    // An edited message is a new id; the old one resolves to nothing.
    let edited = resolve(&cfg_with(&[("CTRL+1", "/kick me")], &[]));
    assert!(!edited.actions.contains_key(&id_of(&a, "/hideout")));
}

#[test]
fn unbound_rows_never_collide() {
    let mut c = cfg_with(&[("", "a"), ("", "b")], &[("", "https://x/{name}")]);
    c.hotkey_upgrade = String::new();
    let r = resolve(&c);
    assert!(r.conflicts.is_empty());
    assert!(!r.actions.values().any(|a| matches!(a, Action::Macro(_) | Action::Shortcut(_) | Action::Upgrade)));
    assert!(r.bindings.extra.iter().all(|(_, t)| !t.is_empty()));
}
