//! Contract tests for the platform-neutral trigger-string parser. Both
//! hotkey backends route config strings through it, so the accepted
//! grammar is pinned here rather than in either backend.

use khaloni_poe2::platform::triggers::{parse, Trigger};

fn t(ctrl: bool, alt: bool, shift: bool, key: &str) -> Trigger {
    Trigger {
        ctrl,
        alt,
        shift,
        key: key.into(),
    }
}

#[test]
fn bare_key() {
    assert_eq!(parse("F7"), Some(t(false, false, false, "F7")));
}

#[test]
fn single_modifier() {
    assert_eq!(parse("CTRL+1"), Some(t(true, false, false, "1")));
    assert_eq!(parse("ALT+Q"), Some(t(false, true, false, "Q")));
}

#[test]
fn stacked_modifiers() {
    assert_eq!(parse("CTRL+SHIFT+F2"), Some(t(true, false, true, "F2")));
    assert_eq!(
        parse("CTRL+ALT+SHIFT+Z"),
        Some(t(true, true, true, "Z"))
    );
}

#[test]
fn case_insensitive_and_key_uppercased() {
    assert_eq!(parse("ctrl+shift+f2"), Some(t(true, false, true, "F2")));
    assert_eq!(parse("Alt+q"), Some(t(false, true, false, "Q")));
    assert_eq!(parse("f7"), Some(t(false, false, false, "F7")));
}

#[test]
fn whitespace_around_tokens_ignored() {
    assert_eq!(parse(" CTRL + 1 "), Some(t(true, false, false, "1")));
}

#[test]
fn unknown_modifier_rejected() {
    assert_eq!(parse("SUPER+F7"), None);
    assert_eq!(parse("META+1"), None);
    assert_eq!(parse("CTRL+FOO+1"), None);
}

#[test]
fn empty_and_degenerate_inputs_rejected() {
    assert_eq!(parse(""), None);
    assert_eq!(parse("   "), None);
    assert_eq!(parse("+"), None);
    // Trailing '+' means an empty key token, not a '+' key.
    assert_eq!(parse("CTRL+"), None);
    assert_eq!(parse("++"), None);
    // Empty modifier token ("+F7" splits to ["", "F7"]).
    assert_eq!(parse("+F7"), None);
}

#[test]
fn unknown_keys_still_parse() {
    // Key validity is the backend's call (portal accepts more than
    // RegisterHotKey); the parser only vets structure and modifiers.
    assert_eq!(parse("CTRL+NUMPAD1"), Some(t(true, false, false, "NUMPAD1")));
    // No '+' at all: the whole string is one (unmappable) key token.
    assert_eq!(
        parse("CTRL SHIFT F2"),
        Some(t(false, false, false, "CTRL SHIFT F2"))
    );
}

#[test]
fn garbage_rejected() {
    assert_eq!(parse("CTRL+SHIFT+"), None);
    assert_eq!(parse("!!+@@"), None);
    assert_eq!(parse("CTRL++1"), None);
}

#[test]
fn dedupe_keeps_the_first_claim_and_reports_conflicts() {
    use khaloni_poe2::platform::triggers::dedupe;
    let b = |id: &str, k: &str| (id.to_string(), k.to_string());
    let (kept, conflicts) = dedupe(vec![
        b("price-check", "F7"),
        b("market", "f7"),
        b("url-0", "F10"),
        b("craft", "F10"),
        b("settings", "F12"),
    ]);
    let ids: Vec<&str> = kept.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(ids, vec!["price-check", "url-0", "settings"]);
    assert_eq!(conflicts.len(), 2);
    assert!(conflicts[0].contains("market") && conflicts[0].contains("price-check"));
    assert!(conflicts[1].contains("craft") && conflicts[1].contains("url-0"));
    // Unbound actions never conflict with each other.
    let (kept, conflicts) = dedupe(vec![b("a", ""), b("b", "")]);
    assert_eq!(kept.len(), 2);
    assert!(conflicts.is_empty());
}
