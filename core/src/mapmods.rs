//! Waystone / map modifier analysis: classify a map's rolled mods as
//! dangerous or rewarding against an extensible rule set, and build a stash
//! search regex from the rewarding ones. Rules are substring needles
//! (case-insensitive) so they survive OCR/clipboard wording drift and can
//! be extended from config without code changes; the built-in seed covers
//! the clearly-dangerous and clearly-rewarding PoE2 mod families and is a
//! starting point, not a complete database.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModKind {
    /// Raises map difficulty / player risk (suffix-family mods).
    Danger,
    /// Raises rewards (quantity, rarity, pack size, extra content).
    Good,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModRule {
    /// Lowercase substring matched against a mod line.
    pub needle: String,
    pub kind: ModKind,
}

impl ModRule {
    fn new(needle: &str, kind: ModKind) -> ModRule {
        ModRule { needle: needle.to_string(), kind }
    }
}

/// Built-in starter rules. Danger needles target the mod families most
/// players avoid; Good needles target the reward-scaling mods worth
/// searching for. Extend via config rather than editing here.
pub fn default_rules() -> Vec<ModRule> {
    use ModKind::{Danger, Good};
    vec![
        // Reward scaling.
        ModRule::new("increased quantity", Good),
        ModRule::new("increased rarity", Good),
        ModRule::new("pack size", Good),
        ModRule::new("additional pack", Good),
        ModRule::new("increased magic monster", Good),
        ModRule::new("increased rare monster", Good),
        ModRule::new("increased number of", Good),
        // Danger families.
        ModRule::new("as extra", Danger),        // "... Damage as Extra <element>"
        ModRule::new("reduced maximum", Danger),  // reduced max resistances
        ModRule::new("less recovery", Danger),
        ModRule::new("less cooldown recovery", Danger),
        ModRule::new("cannot regenerate", Danger),
        ModRule::new("no life regeneration", Danger),
        ModRule::new("additional projectile", Danger),
        ModRule::new("monsters fire", Danger),
        ModRule::new("increased area of effect", Danger),
        ModRule::new("increased attack and cast speed", Danger),
        ModRule::new("increased critical", Danger),
        ModRule::new("ailment", Danger),
        ModRule::new("reduced flask", Danger),
        ModRule::new("armour and evasion", Danger),
        ModRule::new("less accuracy", Danger),
    ]
}

/// The kind of the first rule whose needle occurs in `line` (case-
/// insensitive), or `None` if no rule matches.
pub fn classify(line: &str, rules: &[ModRule]) -> Option<ModKind> {
    let l = line.to_lowercase();
    rules.iter().find(|r| l.contains(&r.needle)).map(|r| r.kind)
}

/// Each mod line paired with its classification, keeping only the lines that
/// matched a rule. Input order is preserved.
pub fn analyze(mod_lines: &[&str], rules: &[ModRule]) -> Vec<(String, ModKind)> {
    mod_lines
        .iter()
        .filter_map(|l| classify(l, rules).map(|k| (l.trim().to_string(), k)))
        .collect()
}

/// A stash-search regex ORing the distinctive needles of the rewarding mods
/// present on this map, so similar high-reward maps can be found. Empty when
/// no rewarding mod is present. Needles are regex-escaped and de-duplicated.
pub fn search_regex(mod_lines: &[&str], rules: &[ModRule]) -> String {
    let mut needles: Vec<&str> = Vec::new();
    for line in mod_lines {
        let l = line.to_lowercase();
        for r in rules {
            if r.kind == ModKind::Good && l.contains(&r.needle) && !needles.contains(&r.needle.as_str())
            {
                needles.push(&r.needle);
            }
        }
    }
    needles
        .iter()
        .map(|n| regex_escape(n))
        .collect::<Vec<_>>()
        .join("|")
}

fn regex_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if "\\.+*?()|[]{}^$".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_flags_danger_and_good() {
        let r = default_rules();
        assert_eq!(
            classify("25% increased Quantity of Items found in this Area", &r),
            Some(ModKind::Good)
        );
        assert_eq!(
            classify("Monsters deal 40% of their Damage as Extra Fire Damage", &r),
            Some(ModKind::Danger)
        );
        assert_eq!(
            classify("Players have 25% reduced maximum Resistances", &r),
            Some(ModKind::Danger)
        );
        assert_eq!(classify("Nothing notable here", &r), None);
    }

    #[test]
    fn analyze_keeps_matched_lines_in_order() {
        let r = default_rules();
        let lines = [
            "20% increased Rarity of Items found in this Area",
            "Flavour text, no mod",
            "Monsters fire 2 additional Projectiles",
        ];
        let got = analyze(&lines, &r);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].1, ModKind::Good);
        assert_eq!(got[1].1, ModKind::Danger);
    }

    #[test]
    fn search_regex_ors_reward_needles() {
        let r = default_rules();
        let lines = [
            "18% increased Quantity of Items found in this Area",
            "30% increased Rarity of Items found in this Area",
            "Players have 25% reduced maximum Resistances",
        ];
        let rx = search_regex(&lines, &r);
        // Only the two reward mods contribute; danger is excluded.
        assert_eq!(rx, "increased quantity|increased rarity");
    }

    #[test]
    fn search_regex_empty_without_reward_mods() {
        let r = default_rules();
        let lines = ["Players have 25% reduced maximum Resistances"];
        assert_eq!(search_regex(&lines, &r), "");
    }
}
