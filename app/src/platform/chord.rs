//! The key chord that makes the game copy the hovered item, shared by both
//! injectors so the order and the clean-up are written, and tested, once.
//!
//! Ctrl+Alt+C copies the item with its advanced mod descriptions whatever
//! the game's own setting says; plain Ctrl+C follows that setting, and with
//! it off yields the simple format, from which no modifier can be read (the
//! price check then refuses the item). Ctrl+Alt+C is what Exiled Exchange 2
//! sends. The plain chord stays available as a setting.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChordKey {
    Ctrl,
    Alt,
    C,
}

/// The transitions of one copy, in order: `(key, down)`.
pub fn copy_chord(advanced: bool) -> Vec<(ChordKey, bool)> {
    use ChordKey::*;
    if advanced {
        vec![(Ctrl, true), (Alt, true), (C, true), (C, false), (Alt, false), (Ctrl, false)]
    } else {
        vec![(Ctrl, true), (C, true), (C, false), (Ctrl, false)]
    }
}

/// Sends the chord through `emit`. When a transition fails, every key
/// already down is still released, newest first, before the error is
/// returned: a modifier left down on the virtual keyboard would stay down
/// for the game and the desktop alike (a held Alt turns the next click
/// into a window move), and the wait for the user's own modifiers does not
/// look at the virtual keyboard, so nothing else would ever notice.
pub fn press_copy_chord<E>(
    advanced: bool,
    emit: &mut dyn FnMut(ChordKey, bool) -> Result<(), E>,
) -> Result<(), E> {
    let mut down: Vec<ChordKey> = Vec::new();
    for (key, press) in copy_chord(advanced) {
        if let Err(e) = emit(key, press) {
            // The failed transition may or may not have landed; a release
            // for a key that is up is harmless, a missing one is not.
            if press {
                down.push(key);
            }
            for held in down.into_iter().rev() {
                let _ = emit(held, false);
            }
            return Err(e);
        }
        if press {
            down.push(key);
        } else {
            down.retain(|k| *k != key);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::ChordKey::*;
    use super::*;

    fn record(advanced: bool, fail_at: Option<usize>) -> (Vec<(ChordKey, bool)>, Result<(), &'static str>) {
        let mut sent = Vec::new();
        let mut emit = |key: ChordKey, down: bool| {
            let n = sent.len();
            sent.push((key, down));
            if Some(n) == fail_at {
                Err("uinput write failed")
            } else {
                Ok(())
            }
        };
        let result = press_copy_chord(advanced, &mut emit);
        (sent, result)
    }

    fn held_at_the_end(sent: &[(ChordKey, bool)]) -> Vec<ChordKey> {
        let mut held = Vec::new();
        for (key, down) in sent {
            held.retain(|k| k != key);
            if *down {
                held.push(*key);
            }
        }
        held
    }

    #[test]
    fn the_item_copy_holds_alt_by_default() {
        assert!(crate::config::Config::default().advanced_copy);
        let (sent, result) = record(crate::config::Config::default().advanced_copy, None);
        assert_eq!(result, Ok(()));
        assert_eq!(sent, [(Ctrl, true), (Alt, true), (C, true), (C, false), (Alt, false), (Ctrl, false)]);
    }

    #[test]
    fn the_plain_chord_is_one_setting_away() {
        let cfg = crate::config::Config::from_toml("league = \"L\"\nadvanced_copy = false").unwrap();
        assert!(!cfg.advanced_copy);
        let (sent, result) = record(cfg.advanced_copy, None);
        assert_eq!(result, Ok(()));
        assert_eq!(sent, [(Ctrl, true), (C, true), (C, false), (Ctrl, false)]);
        // The setting survives the file's own format, and a file without
        // the key gets the advanced chord.
        let again = crate::config::Config::from_toml(&toml::to_string_pretty(&cfg).unwrap()).unwrap();
        assert!(!again.advanced_copy);
        assert!(crate::config::Config::from_toml("league = \"L\"").unwrap().advanced_copy);
    }

    #[test]
    fn alt_is_released_even_when_the_copy_fails() {
        // Whichever transition fails, nothing is left down, and the error
        // still reaches the caller.
        for advanced in [true, false] {
            for fail_at in 0..copy_chord(advanced).len() {
                let (sent, result) = record(advanced, Some(fail_at));
                assert_eq!(result, Err("uinput write failed"), "advanced={advanced} fail_at={fail_at}");
                assert_eq!(held_at_the_end(&sent), [], "advanced={advanced} fail_at={fail_at}: {sent:?}");
            }
        }
        // The case the name is about: C never goes down, Alt and Ctrl do
        // come up, Alt first.
        let (sent, _) = record(true, Some(2));
        assert_eq!(sent, [(Ctrl, true), (Alt, true), (C, true), (C, false), (Alt, false), (Ctrl, false)]);
    }
}
