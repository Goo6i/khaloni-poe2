//! The searchable item properties a price check leads with: a weapon's DPS
//! figures, an armour piece's defences, a sceptre's spirit, its rune
//! sockets. The trade site computes these itself and filters on them under
//! `equipment_filters`, apart from the item's mods.
//!
//! A rare's price follows these totals, not the three or four local mods
//! that add up to them: a 3075-armour cuirass competes with every other
//! ~3000-armour cuirass however each got there. Searching the mods one by
//! one instead matched 569-armour pieces that happened to share a suffix
//! (observed live 2026-09-18).
//!
//! The figures and their floors come from `ee2::filters`, which projects
//! each to 20% quality and floors it the way Exiled Exchange 2 does; this
//! module is the vocabulary the query and the card share for them.

/// The trade site's `equipment_filters` keys a search can bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EquipKey {
    Dps,
    Pdps,
    Edps,
    Crit,
    Aps,
    Armour,
    Evasion,
    EnergyShield,
    Spirit,
    RuneSockets,
    Block,
    RunicWard,
    ReloadTime,
}

impl EquipKey {
    pub fn trade_key(self) -> &'static str {
        match self {
            EquipKey::Dps => "dps",
            EquipKey::Pdps => "pdps",
            EquipKey::Edps => "edps",
            EquipKey::Crit => "crit",
            EquipKey::Aps => "aps",
            EquipKey::Armour => "ar",
            EquipKey::Evasion => "ev",
            EquipKey::EnergyShield => "es",
            EquipKey::Spirit => "spirit",
            EquipKey::RuneSockets => "rune_sockets",
            EquipKey::Block => "block",
            EquipKey::RunicWard => "ward",
            EquipKey::ReloadTime => "reload_time",
        }
    }

    /// Reload time is the one figure bounded from above.
    pub fn lower_is_better(self) -> bool {
        self == EquipKey::ReloadTime
    }

    pub fn label(self) -> &'static str {
        match self {
            EquipKey::Dps => "Total DPS",
            EquipKey::Pdps => "Physical DPS",
            EquipKey::Edps => "Elemental DPS",
            EquipKey::Crit => "Critical Hit Chance",
            EquipKey::Aps => "Attacks per Second",
            EquipKey::Armour => "Armour",
            EquipKey::Evasion => "Evasion Rating",
            EquipKey::EnergyShield => "Energy Shield",
            EquipKey::Spirit => "Spirit",
            EquipKey::RuneSockets => "Rune Sockets",
            EquipKey::Block => "Block",
            EquipKey::RunicWard => "Runic Ward",
            EquipKey::ReloadTime => "Reload Time",
        }
    }
}

/// One property line of the price-check card: the item's own figure, the
/// floor a search uses for it, and whether it is searched by default.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PropFilter {
    pub key: EquipKey,
    /// The item's figure at the compared quality.
    pub value: f64,
    /// The bound that is searched: a floor, or for reload time (where
    /// shorter is better) a ceiling.
    pub min: f64,
    pub enabled: bool,
    /// A figure EE2 folds away because it says little about this item
    /// (physical DPS on a mostly elemental weapon).
    pub hidden: bool,
}

/// Drops the trailing source marker the game appends to some mod lines
/// ("+256(226-256) to Armour (desecrated)").
pub fn strip_mod_markers(text: &str) -> &str {
    const MARKERS: [&str; 5] =
        [" (desecrated)", " (fractured)", " (crafted)", " (rune)", " (enchant)"];
    let mut t = text.trim_end();
    while let Some(rest) = MARKERS.iter().find_map(|m| t.strip_suffix(m)) {
        t = rest.trim_end();
    }
    t
}
