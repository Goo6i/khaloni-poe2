//! The set of global hotkeys a config asks for: who gets a contested key,
//! what each bound id does, and what to tell the user about the losers.
//!
//! One trigger, one action (see `triggers::dedupe`). Claims are made in a
//! fixed order - price check, the panel and tool keys, then chat macros,
//! then resource shortcuts - so a macro or a wiki shortcut can never take
//! a key away from a built-in action. The keys an old file still holds
//! for removed features (the reference panel, the leveling guide, the
//! overlay on/off switch) claim nothing: they are free for the rest. The
//! loser is left unbound and is named on screen and in Settings, not only
//! in the log.
//!
//! Macros and shortcuts are bound under an id made from their own content,
//! and the action is looked up in the map captured with those bindings. An
//! index-based id ("macro-0") read from the live config fired a different
//! message once the list had been reordered in Settings.

use std::collections::HashMap;

use crate::config::Config;
use crate::platform::{triggers, HotkeyBindings};

/// What a fired id does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    PriceCheck,
    Settings,
    Market,
    Upgrade,
    /// Open the craft planner on the hovered item.
    Craft,
    /// Type this chat message.
    Macro(String),
    /// Open this URL template for the hovered item.
    Shortcut(String),
}

/// Which row of the settings window a binding belongs to, so a conflict can
/// be marked where the user would fix it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    PriceCheck,
    Settings,
    Market,
    Upgrade,
    Craft,
    Macro(usize),
    Shortcut(usize),
}

/// A binding that lost its key to an earlier claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub trigger: String,
    pub loser: Slot,
    pub winner: Slot,
    /// One line for a note or a settings label.
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Resolved {
    /// What the hotkey backend binds. Losers are absent.
    pub bindings: HotkeyBindings,
    /// id -> action, for exactly the ids in `bindings`.
    pub actions: HashMap<String, Action>,
    pub conflicts: Vec<Conflict>,
}

const PRICE_CHECK_ID: &str = "price-check";

/// FNV-1a. The std hasher's output may change between Rust releases, and a
/// changed id is a changed binding set, which costs the user a KDE approval
/// dialog.
fn fnv1a(parts: &[&str]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for b in part.bytes().chain([0u8]) {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h
}

fn content_id(kind: &str, key: &str, content: &str) -> String {
    format!("{kind}-{:016x}", fnv1a(&[key.trim(), content]))
}

fn slot_label(slot: Slot, cfg: &Config) -> String {
    let clip = |s: &str| -> String {
        let mut out: String = s.chars().take(28).collect();
        if s.chars().count() > 28 {
            out.push_str("...");
        }
        out
    };
    match slot {
        Slot::PriceCheck => "Price check".into(),
        Slot::Settings => "Settings panel".into(),
        Slot::Market => "Market panel".into(),
        Slot::Upgrade => "Upgrade check".into(),
        Slot::Craft => "Craft planner".into(),
        Slot::Macro(i) => {
            format!("macro \"{}\"", clip(cfg.macros.get(i).map(|m| m.message.as_str()).unwrap_or("")))
        }
        Slot::Shortcut(i) => {
            format!("shortcut {}", clip(cfg.resource_shortcuts.get(i).map(|s| s.url.as_str()).unwrap_or("")))
        }
    }
}

/// Every binding the config asks for, in claim order.
fn claims(cfg: &Config) -> Vec<(String, String, Slot, Action)> {
    let mut out = vec![
        (PRICE_CHECK_ID.to_string(), cfg.hotkey_price_check.clone(), Slot::PriceCheck, Action::PriceCheck),
        ("settings".to_string(), cfg.hotkey_settings.clone(), Slot::Settings, Action::Settings),
        ("market".to_string(), cfg.hotkey_market.clone(), Slot::Market, Action::Market),
        ("upgrade".to_string(), cfg.hotkey_upgrade.clone(), Slot::Upgrade, Action::Upgrade),
        ("craft".to_string(), cfg.hotkey_craft.clone(), Slot::Craft, Action::Craft),
    ];
    for (i, m) in cfg.macros.iter().enumerate() {
        out.push((
            content_id("macro", &m.key, &m.message),
            m.key.clone(),
            Slot::Macro(i),
            Action::Macro(m.message.clone()),
        ));
    }
    for (i, s) in cfg.resource_shortcuts.iter().enumerate() {
        out.push((
            content_id("url", &s.key, &s.url),
            s.key.clone(),
            Slot::Shortcut(i),
            Action::Shortcut(s.url.clone()),
        ));
    }
    out
}

pub fn resolve(cfg: &Config) -> Resolved {
    let claims = claims(cfg);
    let (kept, _) = triggers::dedupe(claims.iter().map(|(id, t, _, _)| (id.clone(), t.clone())).collect());
    let mut resolved = Resolved::default();
    for (id, trigger) in kept {
        if trigger.trim().is_empty() {
            continue;
        }
        // Two rows with the same key and the same content share an id; the
        // first of them is the one that was kept.
        if let Some((_, _, _, action)) = claims.iter().find(|(cid, ..)| *cid == id) {
            resolved.actions.insert(id.clone(), action.clone());
        }
        match id.as_str() {
            PRICE_CHECK_ID => resolved.bindings.price_check = trigger,
            _ => resolved.bindings.extra.push((id, trigger)),
        }
    }
    // Who lost to whom, in the same first-claim-wins order `dedupe` used.
    let mut winners: HashMap<String, Slot> = HashMap::new();
    for (_, trigger, slot, _) in &claims {
        let key = trigger.trim().to_lowercase();
        if key.is_empty() {
            continue;
        }
        match winners.get(&key) {
            None => {
                winners.insert(key, *slot);
            }
            Some(&winner) => resolved.conflicts.push(Conflict {
                trigger: trigger.trim().to_string(),
                loser: *slot,
                winner,
                message: format!(
                    "{} is taken by {}: {} is off",
                    trigger.trim(),
                    slot_label(winner, cfg),
                    slot_label(*slot, cfg)
                ),
            }),
        }
    }
    resolved
}

/// The conflict `slot` lost, if any: the settings window marks that row.
pub fn conflict_for(conflicts: &[Conflict], slot: Slot) -> Option<&Conflict> {
    conflicts.iter().find(|c| c.loser == slot)
}
