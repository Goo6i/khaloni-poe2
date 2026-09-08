//! Portal-style trigger strings ("F7", "CTRL+SHIFT+F2") parsed into a
//! neutral modifiers+key form. The Linux portal backend passes the strings
//! through untouched (the compositor parses them itself); the Windows
//! backend maps the parsed form onto RegisterHotKey. Lives here — not under
//! windows/ — and stays dependency-free so a future Linux fallback (X11
//! grab, manual evdev matching) can reuse it.

/// One trigger, one action. Registering a key twice makes the desktop
/// portal's behavior undefined (KDE has been seen firing both, or storing
/// "none" for later conflicting entries, which reads as a dead hotkey with
/// no error anywhere). The first claim in `bindings` order wins; every
/// loser is dropped and described in the returned conflict lines, so the
/// log names the collision. Unbound actions (empty trigger) never collide.
pub fn dedupe(bindings: Vec<(String, String)>) -> (Vec<(String, String)>, Vec<String>) {
    use std::collections::hash_map::Entry;
    let mut taken: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut kept = Vec::new();
    let mut conflicts = Vec::new();
    for (id, trigger) in bindings {
        let key = trigger.trim().to_lowercase();
        if key.is_empty() {
            kept.push((id, trigger));
            continue;
        }
        match taken.entry(key) {
            Entry::Vacant(e) => {
                e.insert(id.clone());
                kept.push((id, trigger));
            }
            Entry::Occupied(e) => conflicts.push(format!(
                "hotkey conflict: {trigger} is bound to {} - {id} disabled; change one of them in Settings",
                e.get()
            )),
        }
    }
    (kept, conflicts)
}

/// One parsed trigger. `key` is the final '+'-separated token, uppercased
/// ("F7", "1", "Q"); which keys are actually bindable is the backend's
/// concern, so unknown keys still parse — only unknown *modifiers* reject
/// the whole string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trigger {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub key: String,
}

/// Parse "MOD+...+KEY". Case-insensitive; surrounding whitespace per token
/// is ignored. Returns None on an empty string, an empty token ("CTRL+"),
/// or a modifier other than CTRL/ALT/SHIFT — a trigger we can't fully
/// honor must not half-bind.
pub fn parse(s: &str) -> Option<Trigger> {
    let tokens: Vec<&str> = s.split('+').map(str::trim).collect();
    let (&key, mods) = tokens.split_last()?;
    if key.is_empty() {
        return None;
    }
    let mut t = Trigger {
        ctrl: false,
        alt: false,
        shift: false,
        key: key.to_ascii_uppercase(),
    };
    for m in mods {
        match m.to_ascii_uppercase().as_str() {
            "CTRL" => t.ctrl = true,
            "ALT" => t.alt = true,
            "SHIFT" => t.shift = true,
            _ => return None,
        }
    }
    Some(t)
}
