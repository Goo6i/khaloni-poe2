//! The shapes every part of the craft planner shares: an item as the
//! planner sees it, one rollable entry of a pool, and the two seams the
//! rules act through (the pool that says what can roll, and the draw that
//! says which one did). Nothing here reads a file or touches a price.

pub use crate::ee2::parse::Rarity;

/// A mod id of the mod database ("IncreasedLife7"). Each tier of a family
/// is its own entry.
pub type EntryId = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AffixKind {
    Prefix,
    Suffix,
}

impl AffixKind {
    pub fn other(self) -> AffixKind {
        match self {
            AffixKind::Prefix => AffixKind::Suffix,
            AffixKind::Suffix => AffixKind::Prefix,
        }
    }
}

/// Where a modifier on the item came from. Crafted covers essence and
/// alloy mods: since 0.5.0 they share the one crafted slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    Random,
    Crafted,
    Desecrated,
    Fractured,
}

/// The lich families of desecrated modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lich {
    Ulaman,
    Amanamu,
    Kurgal,
}

impl Lich {
    /// The tag the mod database puts on this lich's desecrated entries.
    pub fn tag(self) -> &'static str {
        match self {
            Lich::Ulaman => "ulaman_mod",
            Lich::Amanamu => "amanamu_mod",
            Lich::Kurgal => "kurgal_mod",
        }
    }
}

/// How a rule is known, as the rulebook labels it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Confidence {
    /// In-game item text.
    Game,
    /// Official patch notes.
    Patch,
    /// The mod database or a real trade response.
    Data,
    /// Community sources only.
    Community,
}

/// One explicit modifier on the item.
#[derive(Debug, Clone, PartialEq)]
pub struct ModOn {
    /// The mod database entry, when the item's text joined to one.
    pub entry_id: Option<EntryId>,
    /// The entry's family (the mod database's `type`, "IncreasedLife"), or
    /// the modifier's text when no entry joined.
    pub family: String,
    /// The entry's `groups`; a group on the item keeps its rivals out.
    pub groups: Vec<String>,
    pub kind: AffixKind,
    /// Trade-site numbering: 1 is the family's best tier on this base.
    pub tier: Option<u8>,
    pub required_level: Option<u32>,
    /// Tags the entry adds to the item (`adds_tags`), which block rivals.
    pub adds_tags: Vec<String>,
    pub source: Source,
    /// The line as the item words it.
    pub text: String,
}

impl ModOn {
    /// A desecrated modifier not yet revealed: the copied item shows only
    /// "Desecrated Prefix" or "Desecrated Suffix", so no entry joins it.
    pub fn unrevealed(&self) -> bool {
        self.source == Source::Desecrated && self.entry_id.is_none()
    }
}

/// An item as the planner sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemState {
    /// The item class ("Body Armour").
    pub class: String,
    /// The base name ("Soldier Cuirass").
    pub base: String,
    /// The base's tags from the base item data, in the data's order.
    pub base_tags: Vec<String>,
    pub item_level: u32,
    pub rarity: Rarity,
    pub corrupted: bool,
    pub mirrored: bool,
    pub sanctified: bool,
    pub mods: Vec<ModOn>,
    pub sockets: u32,
}

impl ItemState {
    /// Corrupted, mirrored and sanctified items take no further crafting.
    pub fn locked(&self) -> bool {
        self.corrupted || self.mirrored || self.sanctified
    }

    /// Affixes of one kind the item's rarity allows: none on a normal item,
    /// one of each on a magic item, three of each on a rare.
    pub fn cap(&self, _kind: AffixKind) -> usize {
        match self.rarity {
            Rarity::Magic => 1,
            Rarity::Rare => 3,
            Rarity::Normal | Rarity::Unique => 0,
        }
    }

    pub fn count(&self, kind: AffixKind) -> usize {
        self.mods.iter().filter(|m| m.kind == kind).count()
    }

    /// Open affixes of one kind.
    pub fn open(&self, kind: AffixKind) -> usize {
        self.cap(kind).saturating_sub(self.count(kind))
    }

    pub fn crafted_slot_used(&self) -> bool {
        self.mods.iter().any(|m| m.source == Source::Crafted)
    }

    pub fn desecrated_slot_used(&self) -> bool {
        self.mods.iter().any(|m| m.source == Source::Desecrated)
    }

    pub fn fractured(&self) -> bool {
        self.mods.iter().any(|m| m.source == Source::Fractured)
    }

    /// Every group already on the item.
    pub fn groups(&self) -> impl Iterator<Item = &str> {
        self.mods.iter().flat_map(|m| m.groups.iter().map(String::as_str))
    }

    /// The base's tags followed by the tags the item's mods add.
    pub fn tags(&self) -> impl Iterator<Item = &str> {
        self.base_tags
            .iter()
            .map(String::as_str)
            .chain(self.mods.iter().flat_map(|m| m.adds_tags.iter().map(String::as_str)))
    }
}

/// One entry that can roll: a (family, tier) of the pool.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub entry_id: EntryId,
    pub family: String,
    pub groups: Vec<String>,
    pub kind: AffixKind,
    /// Trade-site numbering on the item's base.
    pub tier: u8,
    pub required_level: u32,
    pub adds_tags: Vec<String>,
    pub text: String,
    pub desecrated: Option<Lich>,
}

impl Candidate {
    /// The modifier this entry puts on an item.
    pub fn to_mod(&self, source: Source) -> ModOn {
        ModOn {
            entry_id: Some(self.entry_id.clone()),
            family: self.family.clone(),
            groups: self.groups.clone(),
            kind: self.kind,
            tier: Some(self.tier),
            required_level: Some(self.required_level),
            adds_tags: self.adds_tags.clone(),
            source,
            text: self.text.clone(),
        }
    }
}

/// What an essence guarantees on an item of a class.
#[derive(Debug, Clone, PartialEq)]
pub enum EssenceOutcome {
    Entry(Candidate),
    Unknown(String),
}

/// What can roll on an item right now. The data layer implements it over
/// the mod database; tests implement it over a handful of entries.
pub trait PoolView {
    /// Entries of `kind` that can roll on `state`: eligible by the first
    /// matching tag, group not on the item, `floor <= required_level <=
    /// item level`.
    fn eligible(&self, state: &ItemState, kind: AffixKind, floor: u32) -> Vec<Candidate>;

    /// Desecrated entries for `state`, narrowed to one kind and one lich
    /// when an omen says so, with the same group and level rules.
    fn desecrated(&self, state: &ItemState, kind: Option<AffixKind>, lich: Option<Lich>, floor: u32) -> Vec<Candidate>;

    /// The entry an essence guarantees on this item's class.
    fn essence(&self, essence: &str, state: &ItemState) -> EssenceOutcome;

    /// The entry an Alloy guarantees on this item's class.
    fn alloy(&self, alloy: &str, state: &ItemState) -> EssenceOutcome;
}

/// The randomness a rule draws on. The simulator supplies a seeded one
/// whose `pick` weighs candidates by the active model.
pub trait Draw {
    /// A uniformly random index below `n` (`n > 0`).
    fn below(&mut self, n: usize) -> usize;

    /// One candidate by the active model's weights; `None` when empty.
    fn pick(&mut self, candidates: &[Candidate]) -> Option<usize>;
}

/// The result of applying one action.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Applied(ItemState),
    /// A desecration reveal: the options to choose from.
    Reveal(Vec<ItemState>),
    /// The action could not be used on this item, and why.
    Refused(String),
}
