//! One rule per currency, omen, essence and bone of the rulebook
//! (`docs/notes/specs/2026-09-26-poe2-crafting-rulebook.md`, pinned to
//! 0.5.5). Each row is `Live` with its preconditions, its outcome and the
//! assumptions it rests on, or `Unavailable`/`Unknown` with the reason; a
//! row that is not live has no outcome function at all, so it can never
//! put a figure on screen.
//!
//! Rules act only through the two seams of `types`: the pool says what can
//! roll, the draw says which one did. Everything here is a pure function of
//! the action, the item, the pool and the draw.

use std::collections::HashSet;

use super::types::{AffixKind, Candidate, Confidence, Draw, EssenceOutcome, ItemState, Lich, ModOn, Outcome, PoolView, Rarity, Source};

/// The rulebook's patch pin.
pub const PATCH: &str = "0.5.5";

// Assumptions, worded once so a plan can show them and a test can match
// them. Each fills a gap on the rulebook's "Unknown / could not source"
// list or rests on a community-only row.

/// Unknown list: "Transmute prefix/suffix split".
pub const A_TRANSMUTE_SIDE: &str = "Transmute picks prefix or suffix 50/50";
/// Unknown list: "Chaos side re-fill rule".
pub const A_CHAOS_REFILL: &str = "Chaos re-fills either side";
/// Unknown list: "omen consumption on failed precondition".
pub const A_OMEN_CONSUMED: &str = "an omen whose condition cannot be met is assumed consumed";
/// Unknown list: "Whittling tie-break".
pub const A_WHITTLING_TIES: &str = "Whittling ties break uniformly";
/// Section 1: the fallback clause is community-sourced.
pub const A_FLOOR_FALLBACK: &str = "the Min-Modifier-Level fallback keeps a family's highest tier rollable";
/// Section 2, Whittling: "fractured/desecrated handling [unknown]".
pub const A_WHITTLING_SCOPE: &str = "Whittling passes over fractured modifiers and ranks desecrated ones like any other";
/// Section 9: "fractured mod immune to Chaos/Annul/Divine [comm]".
pub const A_FRACTURED_KEPT: &str = "a fractured modifier is never removed";
/// Section 1, Fracturing Orb: "uniform over mods (4 -> 25%) [comm]".
pub const A_FRACTURE_UNIFORM: &str = "Fracturing Orb picks uniformly among the item's modifiers";
/// Unknown list: "real spawn weights"; the rulebook gives no split for
/// Alchemy's four modifiers.
pub const A_ALCHEMY_DRAWS: &str = "Orb of Alchemy draws its four modifiers one at a time from every side with room";
/// Unknown list: "essence ilvl/group-collision rules".
pub const A_ESSENCE_LEVEL: &str = "an essence is assumed usable at any item level";
/// Section 4: "Behaviour on a 6-mod item (random removal) [comm]".
pub const A_DESECRATE_FULL: &str = "a desecration on an item with no open affix removes a random modifier first";
/// Section 9: no source says whether a Crystallisation omen steers an
/// Alloy's removal; the omens' text names Perfect and Corrupted Essences only.
pub const A_ALLOY_REMOVAL: &str = "an Alloy's removal takes any modifier: no omen is known to steer it";
/// As for essences, no source gives an item-level rule for an Alloy.
pub const A_ALLOY_LEVEL: &str = "an Alloy is assumed usable at any item level";
/// The lich an unrevealed modifier belongs to was set by the omens used at
/// its desecration, which its copied text does not show.
pub const A_REVEAL_LICH: &str = "an unrevealed modifier may be of any lich: the omens used at its desecration are not on the item";
/// The name a plan gives the reveal of an unrevealed modifier.
pub const REVEAL_NAME: &str = "Reveal at the Well of Souls";

// ------------------------------------------------------------------ rows

/// The grade of an orb that comes in Greater and Perfect forms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Grade {
    Normal,
    Greater,
    Perfect,
}

/// Every currency item of the rulebook's section 1 that is used on an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Orb {
    Transmutation,
    Augmentation,
    Regal,
    Exalted,
    Chaos,
    Annulment,
    Alchemy,
    Divine,
    Vaal,
    Chance,
    Fracturing,
    Artificer,
    HinekorasLock,
    Mirror,
}

impl Orb {
    pub const ALL: [Orb; 14] = [
        Orb::Transmutation,
        Orb::Augmentation,
        Orb::Regal,
        Orb::Exalted,
        Orb::Chaos,
        Orb::Annulment,
        Orb::Alchemy,
        Orb::Divine,
        Orb::Vaal,
        Orb::Chance,
        Orb::Fracturing,
        Orb::Artificer,
        Orb::HinekorasLock,
        Orb::Mirror,
    ];

    fn base_name(self) -> &'static str {
        match self {
            Orb::Transmutation => "Orb of Transmutation",
            Orb::Augmentation => "Orb of Augmentation",
            Orb::Regal => "Regal Orb",
            Orb::Exalted => "Exalted Orb",
            Orb::Chaos => "Chaos Orb",
            Orb::Annulment => "Orb of Annulment",
            Orb::Alchemy => "Orb of Alchemy",
            Orb::Divine => "Divine Orb",
            Orb::Vaal => "Vaal Orb",
            Orb::Chance => "Orb of Chance",
            Orb::Fracturing => "Fracturing Orb",
            Orb::Artificer => "Artificer's Orb",
            Orb::HinekorasLock => "Hinekora's Lock",
            Orb::Mirror => "Mirror of Kalandra",
        }
    }

    /// Only these five come in Greater and Perfect forms.
    pub fn graded(self) -> bool {
        matches!(self, Orb::Transmutation | Orb::Augmentation | Orb::Regal | Orb::Exalted | Orb::Chaos)
    }
}

/// The Minimum Modifier Level a graded orb rolls from: Greater and Perfect
/// Transmutation/Augmentation 44/70 (44 since 0.5.0), Greater and Perfect
/// Regal/Exalted/Chaos 35/50. Plain orbs and ungraded currency have none.
pub fn floor(orb: Orb, grade: Grade) -> u32 {
    match (orb, grade) {
        (_, Grade::Normal) => 0,
        (Orb::Transmutation | Orb::Augmentation, Grade::Greater) => 44,
        (Orb::Transmutation | Orb::Augmentation, Grade::Perfect) => 70,
        (Orb::Regal | Orb::Exalted | Orb::Chaos, Grade::Greater) => 35,
        (Orb::Regal | Orb::Exalted | Orb::Chaos, Grade::Perfect) => 50,
        _ => 0,
    }
}

/// The four Vaal Infusers of 0.5.0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Infuser {
    Armourer,
    Blacksmith,
    Arcanist,
    Catalysing,
}

impl Infuser {
    pub const ALL: [Infuser; 4] = [Infuser::Armourer, Infuser::Blacksmith, Infuser::Arcanist, Infuser::Catalysing];

    fn name(self) -> &'static str {
        match self {
            Infuser::Armourer => "Armourer's Vaal Infuser",
            Infuser::Blacksmith => "Blacksmith's Vaal Infuser",
            Infuser::Arcanist => "Arcanist's Vaal Infuser",
            Infuser::Catalysing => "Catalysing Vaal Infuser",
        }
    }
}

/// Every omen of the rulebook's section 2. An omen rides on the action it
/// changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Omen {
    SinistralExaltation,
    DextralExaltation,
    GreaterExaltation,
    HomogenisingExaltation,
    CatalysingExaltation,
    SinistralCoronation,
    DextralCoronation,
    HomogenisingCoronation,
    SinistralAnnulment,
    DextralAnnulment,
    GreaterAnnulment,
    Light,
    SinistralErasure,
    DextralErasure,
    Whittling,
    ChaoticRarity,
    ChaoticQuantity,
    ChaoticMonsters,
    ChaoticEffectiveness,
    SinistralAlchemy,
    DextralAlchemy,
    SinistralCrystallisation,
    DextralCrystallisation,
    Sanctification,
    Blessed,
    Corruption,
    Chance,
    Ancients,
    SinistralNecromancy,
    DextralNecromancy,
    AbyssalEchoes,
    Blackblooded,
    Liege,
    Sovereign,
    Putrefaction,
    Recombination,
}

/// What an omen acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Host {
    Orb(Orb),
    /// A Perfect or corrupted essence.
    ReplacingEssence,
    Desecration,
    Recombinator,
}

impl Omen {
    pub const ALL: [Omen; 36] = [
        Omen::SinistralExaltation,
        Omen::DextralExaltation,
        Omen::GreaterExaltation,
        Omen::HomogenisingExaltation,
        Omen::CatalysingExaltation,
        Omen::SinistralCoronation,
        Omen::DextralCoronation,
        Omen::HomogenisingCoronation,
        Omen::SinistralAnnulment,
        Omen::DextralAnnulment,
        Omen::GreaterAnnulment,
        Omen::Light,
        Omen::SinistralErasure,
        Omen::DextralErasure,
        Omen::Whittling,
        Omen::ChaoticRarity,
        Omen::ChaoticQuantity,
        Omen::ChaoticMonsters,
        Omen::ChaoticEffectiveness,
        Omen::SinistralAlchemy,
        Omen::DextralAlchemy,
        Omen::SinistralCrystallisation,
        Omen::DextralCrystallisation,
        Omen::Sanctification,
        Omen::Blessed,
        Omen::Corruption,
        Omen::Chance,
        Omen::Ancients,
        Omen::SinistralNecromancy,
        Omen::DextralNecromancy,
        Omen::AbyssalEchoes,
        Omen::Blackblooded,
        Omen::Liege,
        Omen::Sovereign,
        Omen::Putrefaction,
        Omen::Recombination,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Omen::SinistralExaltation => "Omen of Sinistral Exaltation",
            Omen::DextralExaltation => "Omen of Dextral Exaltation",
            Omen::GreaterExaltation => "Omen of Greater Exaltation",
            Omen::HomogenisingExaltation => "Omen of Homogenising Exaltation",
            Omen::CatalysingExaltation => "Omen of Catalysing Exaltation",
            Omen::SinistralCoronation => "Omen of Sinistral Coronation",
            Omen::DextralCoronation => "Omen of Dextral Coronation",
            Omen::HomogenisingCoronation => "Omen of Homogenising Coronation",
            Omen::SinistralAnnulment => "Omen of Sinistral Annulment",
            Omen::DextralAnnulment => "Omen of Dextral Annulment",
            Omen::GreaterAnnulment => "Omen of Greater Annulment",
            Omen::Light => "Omen of Light",
            Omen::SinistralErasure => "Omen of Sinistral Erasure",
            Omen::DextralErasure => "Omen of Dextral Erasure",
            Omen::Whittling => "Omen of Whittling",
            Omen::ChaoticRarity => "Omen of Chaotic Rarity",
            Omen::ChaoticQuantity => "Omen of Chaotic Quantity",
            Omen::ChaoticMonsters => "Omen of Chaotic Monsters",
            Omen::ChaoticEffectiveness => "Omen of Chaotic Effectiveness",
            Omen::SinistralAlchemy => "Omen of Sinistral Alchemy",
            Omen::DextralAlchemy => "Omen of Dextral Alchemy",
            Omen::SinistralCrystallisation => "Omen of Sinistral Crystallisation",
            Omen::DextralCrystallisation => "Omen of Dextral Crystallisation",
            Omen::Sanctification => "Omen of Sanctification",
            Omen::Blessed => "Omen of the Blessed",
            Omen::Corruption => "Omen of Corruption",
            Omen::Chance => "Omen of Chance",
            Omen::Ancients => "Omen of the Ancients",
            Omen::SinistralNecromancy => "Omen of Sinistral Necromancy",
            Omen::DextralNecromancy => "Omen of Dextral Necromancy",
            Omen::AbyssalEchoes => "Omen of Abyssal Echoes",
            Omen::Blackblooded => "Omen of the Blackblooded",
            Omen::Liege => "Omen of the Liege",
            Omen::Sovereign => "Omen of the Sovereign",
            Omen::Putrefaction => "Omen of Putrefaction",
            Omen::Recombination => "Omen of Recombination",
        }
    }

    fn host(self) -> Host {
        use Omen::*;
        match self {
            SinistralExaltation | DextralExaltation | GreaterExaltation | HomogenisingExaltation | CatalysingExaltation => {
                Host::Orb(Orb::Exalted)
            }
            SinistralCoronation | DextralCoronation | HomogenisingCoronation => Host::Orb(Orb::Regal),
            SinistralAnnulment | DextralAnnulment | GreaterAnnulment | Light => Host::Orb(Orb::Annulment),
            SinistralErasure | DextralErasure | Whittling | ChaoticRarity | ChaoticQuantity | ChaoticMonsters
            | ChaoticEffectiveness => Host::Orb(Orb::Chaos),
            SinistralAlchemy | DextralAlchemy => Host::Orb(Orb::Alchemy),
            SinistralCrystallisation | DextralCrystallisation => Host::ReplacingEssence,
            Sanctification | Blessed => Host::Orb(Orb::Divine),
            Corruption => Host::Orb(Orb::Vaal),
            Chance | Ancients => Host::Orb(Orb::Chance),
            SinistralNecromancy | DextralNecromancy | AbyssalEchoes | Blackblooded | Liege | Sovereign | Putrefaction => {
                Host::Desecration
            }
            Recombination => Host::Recombinator,
        }
    }

    /// What the omen does, in its item text (rulebook section 2), for the
    /// omens a plan can use.
    pub fn effect(self) -> Option<&'static str> {
        use Omen::*;
        Some(match self {
            SinistralExaltation => "Exalted Orb will add only prefix modifiers",
            DextralExaltation => "Exalted Orb will add only suffix modifiers",
            GreaterExaltation => "Exalted Orb will add two random modifiers",
            SinistralAnnulment => "Orb of Annulment will remove only prefix modifiers",
            DextralAnnulment => "Orb of Annulment will remove only suffix modifiers",
            Light => "Orb of Annulment will remove only Desecrated modifiers",
            SinistralErasure => "Chaos Orb will remove only prefix modifiers",
            DextralErasure => "Chaos Orb will remove only suffix modifiers",
            Whittling => "Chaos Orb will remove the lowest level modifier",
            SinistralCrystallisation => "Perfect or Corrupted Essence will remove only Prefix modifiers",
            DextralCrystallisation => "Perfect or Corrupted Essence will remove only Suffix modifiers",
            SinistralNecromancy => "Desecration attempt will add only prefix modifiers",
            DextralNecromancy => "Desecration attempt will add only suffix modifiers",
            AbyssalEchoes => "when you reveal Desecrated modifiers you can reroll the options once",
            Blackblooded => "Weapon or Jewellery Desecration attempt will guarantee a random Kurgal modifier",
            Liege => "Weapon or Jewellery Desecration attempt will guarantee a random Amanamu modifier",
            Sovereign => "Weapon or Jewellery Desecration attempt will guarantee a random Ulaman modifier",
            _ => return None,
        })
    }

    /// The side a Sinistral/Dextral omen restricts its orb to.
    pub fn side(self) -> Option<AffixKind> {
        use Omen::*;
        match self {
            SinistralExaltation | SinistralCoronation | SinistralAnnulment | SinistralErasure | SinistralAlchemy
            | SinistralCrystallisation | SinistralNecromancy => Some(AffixKind::Prefix),
            DextralExaltation | DextralCoronation | DextralAnnulment | DextralErasure | DextralAlchemy
            | DextralCrystallisation | DextralNecromancy => Some(AffixKind::Suffix),
            _ => None,
        }
    }

    /// The lich a desecration omen guarantees.
    pub fn lich(self) -> Option<Lich> {
        match self {
            Omen::Blackblooded => Some(Lich::Kurgal),
            Omen::Liege => Some(Lich::Amanamu),
            Omen::Sovereign => Some(Lich::Ulaman),
            _ => None,
        }
    }

    /// Why an omen cannot be used in 0.5.5, when it cannot.
    fn unavailable(self) -> Option<&'static str> {
        match self {
            Omen::HomogenisingExaltation | Omen::HomogenisingCoronation => Some(
                "Homogenising omens are Standard-only: they \"only appear on the Currency Exchange in Standard Leagues\" (0.5.0 patch notes)",
            ),
            Omen::Corruption => {
                Some("Omen of Corruption is no longer obtainable since 0.5.0: it trades on the Standard exchange only")
            }
            Omen::Recombination => Some(
                "\"The Omen of Recombination has been removed. Existing Omens of Recombination will be deleted upon logging in.\" (0.5.0 patch notes)",
            ),
            _ => None,
        }
    }

    /// Why an omen cannot be modelled, when the rulebook leaves it open.
    fn unknown(self) -> Option<&'static str> {
        use Omen::*;
        match self {
            SinistralCoronation | DextralCoronation | GreaterAnnulment | SinistralAlchemy | DextralAlchemy => Some(
                "not in the Forbidden Rites price feed, so whether it can be had in 0.5.5 is unknown",
            ),
            CatalysingExaltation => Some(
                "it \"will consume all Catalyst Quality to increase the chance of the corresponding type of Modifier\", and the magnitude of that increase is unknown",
            ),
            Sanctification => Some(
                "it Sanctifies the item and, since 0.5.0, \"multiplies each modifier value based on the current value\"; the multiplier range is unknown",
            ),
            Putrefaction => Some(
                "it \"will replace all modifiers … up to 6 Unrevealed modifiers and Corrupting the item\", which conflicts with the 0.5.0 one-desecrated-mod cap; its current behaviour is unknown",
            ),
            ChaoticRarity | ChaoticQuantity | ChaoticMonsters | ChaoticEffectiveness => Some(
                "it replaces all modifiers on a Waystone by what they grant, its function was inverted in 0.5.0 after the cached text was taken, and which Waystone modifiers grant what is not in the mod pool",
            ),
            _ => None,
        }
    }
}

/// The tier of an essence. Corrupted covers Hysteria, Delirium, Horror,
/// Insanity and the Abyss, which have no tier in their name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EssenceTier {
    Lesser,
    Normal,
    Greater,
    Perfect,
    Corrupted,
}

impl EssenceTier {
    /// Perfect and corrupted essences remove a modifier from a rare; the
    /// others upgrade a magic item.
    pub fn replaces(self) -> bool {
        matches!(self, EssenceTier::Perfect | EssenceTier::Corrupted)
    }
}

/// The essences that come in Lesser, normal, Greater and Perfect tiers, as
/// the cached essence data lists them.
pub const TIERED_ESSENCES: [&str; 19] = [
    "the Body",
    "the Mind",
    "Enhancement",
    "Abrasion",
    "Flames",
    "Ice",
    "Electricity",
    "Ruin",
    "Battle",
    "Sorcery",
    "Haste",
    "the Infinite",
    "Seeking",
    "Insulation",
    "Thawing",
    "Grounding",
    "Alacrity",
    "Opulence",
    "Command",
];

/// The corrupted essences.
pub const CORRUPTED_ESSENCES: [&str; 5] = ["Hysteria", "Delirium", "Horror", "Insanity", "the Abyss"];

/// The Alloys, as the Alloy table (`core/data/alloys.json`) names them:
/// every Alloy poe.ninja prices.
pub const ALLOYS: [&str; 13] = [
    "Adaptive Alloy",
    "Celestial Alloy",
    "Expansive Alloy",
    "Protective Alloy",
    "Runic Alloy",
    "Sovereign Alloy",
    "Swift Alloy",
    "The Runefather's Alloy",
    "Transcendent Alloy",
    "Cyclonic Alloy",
    "Mystic Alloy",
    "Prismatic Alloy",
    "The Runebinder's Alloy",
];

/// The item a bone desecrates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BoneKind {
    /// A Rare Weapon or Quiver.
    Jawbone,
    /// Rare Armour.
    Rib,
    /// A Rare Amulet, Ring or Belt.
    Collarbone,
    /// A Rare Jewel.
    Cranium,
    /// A Rare Waystone.
    Vertebrae,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BoneGrade {
    /// "Maximum Item Level: 64".
    Gnawed,
    /// No restriction.
    Preserved,
    /// "Minimum Modifier Level: 40".
    Ancient,
}

impl BoneKind {
    pub const ALL: [BoneKind; 5] = [BoneKind::Jawbone, BoneKind::Rib, BoneKind::Collarbone, BoneKind::Cranium, BoneKind::Vertebrae];

    fn name(self) -> &'static str {
        match self {
            BoneKind::Jawbone => "Jawbone",
            BoneKind::Rib => "Rib",
            BoneKind::Collarbone => "Collarbone",
            BoneKind::Cranium => "Cranium",
            BoneKind::Vertebrae => "Vertebrae",
        }
    }

    /// The item text's wording of what the bone takes.
    fn target(self) -> &'static str {
        match self {
            BoneKind::Jawbone => "a Rare Weapon or Quiver",
            BoneKind::Rib => "Rare Armour",
            BoneKind::Collarbone => "a Rare Amulet, Ring or Belt",
            BoneKind::Cranium => "a Rare Jewel",
            BoneKind::Vertebrae => "a Rare Waystone",
        }
    }

    /// Whether the item's base carries a tag of what the bone takes. Caster
    /// weapons carry their own tag instead of "weapon"; waystones carry
    /// "map"; shields and foci count as armour on the trade site.
    fn takes(self, state: &ItemState) -> bool {
        let tags: &[&str] = match self {
            BoneKind::Jawbone => &["weapon", "wand", "staff", "sceptre", "talisman", "quiver"],
            BoneKind::Rib => &["armour", "shield", "focus"],
            BoneKind::Collarbone => &["amulet", "ring", "belt"],
            BoneKind::Cranium => &["jewel"],
            BoneKind::Vertebrae => &["map"],
        };
        has_tag(state, tags)
    }
}

impl BoneGrade {
    pub const ALL: [BoneGrade; 3] = [BoneGrade::Gnawed, BoneGrade::Preserved, BoneGrade::Ancient];

    fn name(self) -> &'static str {
        match self {
            BoneGrade::Gnawed => "Gnawed",
            BoneGrade::Preserved => "Preserved",
            BoneGrade::Ancient => "Ancient",
        }
    }

    fn floor(self) -> u32 {
        match self {
            BoneGrade::Ancient => 40,
            _ => 0,
        }
    }
}

/// The bones that exist: the Cranium and the Vertebrae come Preserved only.
fn bone_exists(kind: BoneKind, grade: BoneGrade) -> bool {
    !matches!(kind, BoneKind::Cranium | BoneKind::Vertebrae) || grade == BoneGrade::Preserved
}

/// One item-changing step a plan can take.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// A currency orb, with its grade and the omens active for it.
    Orb { orb: Orb, grade: Grade, omens: Vec<Omen> },
    Infuser(Infuser),
    /// `of` is the name after "Essence of" ("the Body", "Hysteria").
    Essence { of: String, tier: EssenceTier, omens: Vec<Omen> },
    /// A desecration with a bone, then its reveal at the Well of Souls.
    Bone { kind: BoneKind, grade: BoneGrade, omens: Vec<Omen> },
    /// Revealing a desecrated modifier the item already carries unrevealed:
    /// free at the Well of Souls, one of three options, rerolled once under
    /// Omen of Abyssal Echoes.
    Reveal { omens: Vec<Omen> },
    /// One of the 0.5.0 Alloys, by its item name ("Adaptive Alloy").
    Alloy { name: String },
    Recombinator { omens: Vec<Omen> },
    /// Buying the item from trade. Rules leave the item as it is; the
    /// strategy that buys supplies the listing and its price.
    Buy,
}

impl Action {
    pub fn orb(orb: Orb) -> Action {
        Action::Orb { orb, grade: Grade::Normal, omens: vec![] }
    }

    pub fn graded(orb: Orb, grade: Grade) -> Action {
        Action::Orb { orb, grade, omens: vec![] }
    }

    pub fn essence(of: &str, tier: EssenceTier) -> Action {
        Action::Essence { of: of.to_string(), tier, omens: vec![] }
    }

    pub fn bone(kind: BoneKind, grade: BoneGrade) -> Action {
        Action::Bone { kind, grade, omens: vec![] }
    }

    pub fn alloy(name: &str) -> Action {
        Action::Alloy { name: name.to_string() }
    }

    /// The same action with one more omen active. Infusers, Alloys and
    /// buying take no omen and come back unchanged.
    pub fn with(mut self, omen: Omen) -> Action {
        match &mut self {
            Action::Orb { omens, .. }
            | Action::Essence { omens, .. }
            | Action::Bone { omens, .. }
            | Action::Reveal { omens }
            | Action::Recombinator { omens } => omens.push(omen),
            Action::Infuser(_) | Action::Alloy { .. } | Action::Buy => {}
        }
        self
    }

    pub fn omens(&self) -> &[Omen] {
        match self {
            Action::Orb { omens, .. }
            | Action::Essence { omens, .. }
            | Action::Bone { omens, .. }
            | Action::Reveal { omens }
            | Action::Recombinator { omens } => omens,
            Action::Infuser(_) | Action::Alloy { .. } | Action::Buy => &[],
        }
    }

    /// The essence's item name, as the essence data keys it ("Greater
    /// Essence of the Body", "Essence of Hysteria").
    pub fn essence_name(&self) -> Option<String> {
        match self {
            Action::Essence { of, tier, .. } => Some(match tier {
                EssenceTier::Lesser => format!("Lesser Essence of {of}"),
                EssenceTier::Normal | EssenceTier::Corrupted => format!("Essence of {of}"),
                EssenceTier::Greater => format!("Greater Essence of {of}"),
                EssenceTier::Perfect => format!("Perfect Essence of {of}"),
            }),
            _ => None,
        }
    }

    /// The action as a plan names it: the item, then each omen.
    pub fn name(&self) -> String {
        let head = match self {
            Action::Orb { orb, grade, .. } => match grade {
                Grade::Normal => orb.base_name().to_string(),
                Grade::Greater => format!("Greater {}", orb.base_name()),
                Grade::Perfect => format!("Perfect {}", orb.base_name()),
            },
            Action::Infuser(i) => i.name().to_string(),
            Action::Essence { .. } => self.essence_name().unwrap_or_default(),
            Action::Bone { kind, grade, .. } => format!("{} {}", grade.name(), kind.name()),
            Action::Reveal { .. } => REVEAL_NAME.to_string(),
            Action::Alloy { name } => name.clone(),
            Action::Recombinator { .. } => "Recombinator".to_string(),
            Action::Buy => "Buy".to_string(),
        };
        self.omens().iter().fold(head, |acc, o| format!("{acc} + {}", o.name()))
    }

    fn has(&self, omen: Omen) -> bool {
        self.omens().contains(&omen)
    }

    /// The side an omen restricts this action to, if any.
    fn side(&self) -> Option<AffixKind> {
        self.omens().iter().find_map(|o| o.side())
    }

    fn lich(&self) -> Option<Lich> {
        self.omens().iter().find_map(|o| o.lich())
    }

    fn orb_floor(&self) -> u32 {
        match self {
            Action::Orb { orb, grade, .. } => floor(*orb, *grade),
            Action::Bone { grade, .. } => grade.floor(),
            _ => 0,
        }
    }
}

/// Every row of the rulebook, for iterating the table: each currency in
/// each grade it has, each omen on its orb (and Greater Exaltation with
/// each side omen), every essence in every tier, every bone that exists,
/// every Alloy, the Recombinator, and buying.
pub fn all_rows() -> Vec<Action> {
    let mut rows = Vec::new();
    for orb in Orb::ALL {
        rows.push(Action::orb(orb));
        if orb.graded() {
            rows.push(Action::graded(orb, Grade::Greater));
            rows.push(Action::graded(orb, Grade::Perfect));
        }
    }
    rows.extend(Infuser::ALL.map(Action::Infuser));
    for omen in Omen::ALL {
        let row = match omen.host() {
            Host::Orb(orb) => Action::orb(orb).with(omen),
            Host::ReplacingEssence => Action::essence("the Body", EssenceTier::Perfect).with(omen),
            Host::Desecration => {
                // The lich omens act on weapons and jewellery.
                let kind = if omen.lich().is_some() { BoneKind::Jawbone } else { BoneKind::Rib };
                Action::bone(kind, BoneGrade::Preserved).with(omen)
            }
            Host::Recombinator => Action::Recombinator { omens: vec![omen] },
        };
        rows.push(row);
    }
    rows.push(Action::orb(Orb::Exalted).with(Omen::GreaterExaltation).with(Omen::SinistralExaltation));
    rows.push(Action::orb(Orb::Exalted).with(Omen::GreaterExaltation).with(Omen::DextralExaltation));
    for of in TIERED_ESSENCES {
        for tier in [EssenceTier::Lesser, EssenceTier::Normal, EssenceTier::Greater, EssenceTier::Perfect] {
            rows.push(Action::essence(of, tier));
        }
    }
    for of in CORRUPTED_ESSENCES {
        rows.push(Action::essence(of, EssenceTier::Corrupted));
    }
    for kind in BoneKind::ALL {
        for grade in BoneGrade::ALL {
            if bone_exists(kind, grade) {
                rows.push(Action::bone(kind, grade));
            }
        }
    }
    rows.push(Action::Reveal { omens: vec![] });
    rows.push(Action::Reveal { omens: vec![Omen::AbyssalEchoes] });
    rows.extend(ALLOYS.map(Action::alloy));
    rows.push(Action::Recombinator { omens: vec![] });
    rows.push(Action::Buy);
    rows
}

// ------------------------------------------------------------------ rules

/// Checks whether an action can be used on an item; the error is the
/// reason, worded for the panel.
pub type CheckFn = fn(&Action, &ItemState) -> Result<(), String>;

/// Applies an action to an item whose preconditions hold, drawing from the
/// pool through the draw.
pub type ApplyFn = fn(&Action, &ItemState, &dyn PoolView, &mut dyn Draw) -> Outcome;

/// A live rule. The functions take the action so one function serves every
/// grade and omen combination of an orb; `Spec::run` checks before it
/// applies.
#[derive(Debug, Clone)]
pub struct Spec {
    pub preconditions: CheckFn,
    pub apply: ApplyFn,
    /// For a desecration with Omen of Abyssal Echoes: draws the options
    /// again. Call it with any option of the first reveal.
    pub reroll: Option<ApplyFn>,
    /// Every assumption the outcome rests on, worded as the plan shows it.
    pub assumptions: Vec<&'static str>,
    pub source: Confidence,
}

impl Spec {
    /// Checks the preconditions, then applies.
    pub fn run(&self, action: &Action, state: &ItemState, pool: &dyn PoolView, draw: &mut dyn Draw) -> Outcome {
        match (self.preconditions)(action, state) {
            Ok(()) => (self.apply)(action, state, pool, draw),
            Err(reason) => Outcome::Refused(reason),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Rule {
    Live(Spec),
    /// Cannot be used in 0.5.5 (Standard-only, removed, disabled, or no
    /// such item); a plan never proposes it.
    Unavailable(String),
    /// The rulebook cannot say what it does; a plan that needs it shows the
    /// reason in place of a figure.
    Unknown(String),
}

/// Resolves and runs an action: a live rule's outcome, or a refusal that
/// starts with "unavailable: " or "unknown: " and carries the reason.
pub fn attempt(action: &Action, state: &ItemState, pool: &dyn PoolView, draw: &mut dyn Draw) -> Outcome {
    match rule(action) {
        Rule::Live(spec) => spec.run(action, state, pool, draw),
        Rule::Unavailable(reason) => Outcome::Refused(format!("unavailable: {reason}")),
        Rule::Unknown(reason) => Outcome::Refused(format!("unknown: {reason}")),
    }
}

/// The rule for an action.
pub fn rule(action: &Action) -> Rule {
    if let Some(rule) = row_problem(action) {
        return rule;
    }
    match action {
        Action::Orb { orb, grade, .. } => orb_rule(action, *orb, *grade),
        Action::Infuser(i) => Rule::Unknown(format!(
            "{} \"Improves the quality … above 20% with a chance of Corrupting it\"; that chance is not in the rulebook, and quality is not part of the planned item",
            i.name()
        )),
        Action::Essence { tier, .. } => essence_rule(action, *tier),
        Action::Bone { kind, .. } => bone_rule(action, *kind),
        Action::Reveal { .. } => {
            let mut assumptions = vec![A_REVEAL_LICH];
            if !action.omens().is_empty() {
                assumptions.push(A_OMEN_CONSUMED);
            }
            let reroll: Option<ApplyFn> = if action.has(Omen::AbyssalEchoes) { Some(reroll_desecration) } else { None };
            Rule::Live(Spec { preconditions: check_reveal, apply: apply_reveal, reroll, assumptions, source: Confidence::Patch })
        }
        Action::Alloy { .. } => Rule::Live(Spec {
            preconditions: check_alloy,
            apply: apply_alloy,
            reroll: None,
            assumptions: vec![A_ALLOY_REMOVAL, A_ALLOY_LEVEL, A_FRACTURED_KEPT],
            source: Confidence::Game,
        }),
        Action::Recombinator { .. } => Rule::Unavailable(
            "\"The Recombinator has been disabled\" (0.5.0 patch notes), and nothing since re-enables it".to_string(),
        ),
        Action::Buy => Rule::Live(Spec {
            preconditions: |_, _| Ok(()),
            apply: |_, s, _, _| Outcome::Applied(s.clone()),
            reroll: None,
            assumptions: vec![],
            source: Confidence::Data,
        }),
    }
}

/// Rows that do not exist, omens on the wrong action, and omens that are
/// unavailable or unknown. Unavailable wins over unknown: a plan must never
/// propose an unavailable step.
fn row_problem(action: &Action) -> Option<Rule> {
    let name = action.name();
    match action {
        Action::Orb { orb, grade, .. } if *grade != Grade::Normal && !orb.graded() => {
            return Some(Rule::Unavailable(format!("there is no {name}: only Transmutation, Augmentation, Regal, Exalted and Chaos come in Greater and Perfect forms")));
        }
        Action::Bone { kind, grade, .. } if !bone_exists(*kind, *grade) => {
            return Some(Rule::Unavailable(format!("there is no {} {}: that bone comes Preserved only", grade.name(), kind.name())));
        }
        Action::Alloy { name } if !ALLOYS.contains(&name.as_str()) => {
            return Some(Rule::Unknown(format!("{name} is not in the Alloy table")));
        }
        Action::Essence { of, tier, .. } => {
            let known = if *tier == EssenceTier::Corrupted { CORRUPTED_ESSENCES.contains(&of.as_str()) } else { TIERED_ESSENCES.contains(&of.as_str()) };
            if !known {
                return Some(Rule::Unknown(format!("{} is not in the rulebook's essence list", action.essence_name().unwrap_or_default())));
            }
        }
        _ => {}
    }

    let omens = action.omens();
    for (i, omen) in omens.iter().enumerate() {
        let fits = match (omen.host(), action) {
            (Host::Orb(want), Action::Orb { orb, .. }) => want == *orb,
            (Host::ReplacingEssence, Action::Essence { tier, .. }) => tier.replaces(),
            (Host::Desecration, Action::Bone { .. }) => true,
            // At the reveal only Abyssal Echoes still acts; the side and lich
            // omens act on the desecration itself.
            (Host::Desecration, Action::Reveal { .. }) => *omen == Omen::AbyssalEchoes,
            (Host::Recombinator, Action::Recombinator { .. }) => true,
            _ => false,
        };
        if !fits {
            return Some(Rule::Unavailable(format!("{} does not act on {}", omen.name(), name.split(" + ").next().unwrap_or(&name))));
        }
        if omens[..i].contains(omen) {
            return Some(Rule::Unavailable(format!("{} is listed twice; one is used per action", omen.name())));
        }
    }
    if let Some(reason) = omens.iter().find_map(|o| o.unavailable()) {
        return Some(Rule::Unavailable(reason.to_string()));
    }
    if let Some(o) = omens.iter().find(|o| o.unknown().is_some()) {
        return Some(Rule::Unknown(format!("{}: {}", o.name(), o.unknown().unwrap_or_default())));
    }
    let sides: HashSet<AffixKind> = omens.iter().filter_map(|o| o.side()).collect();
    if sides.len() > 1 {
        return Some(Rule::Unknown("the rulebook does not say what happens with both a Sinistral and a Dextral omen active".to_string()));
    }
    if omens.iter().filter(|o| o.lich().is_some()).count() > 1 {
        return Some(Rule::Unknown("the rulebook does not say what happens with two lich omens active".to_string()));
    }
    None
}

fn orb_rule(action: &Action, orb: Orb, grade: Grade) -> Rule {
    let graded = grade != Grade::Normal;
    let omened = !action.omens().is_empty();
    let mut assumptions: Vec<&'static str> = Vec::new();
    let mut add = |on: bool, a: &'static str| {
        if on {
            assumptions.push(a);
        }
    };
    let (preconditions, apply): (CheckFn, ApplyFn) = match orb {
        Orb::Transmutation => {
            add(true, A_TRANSMUTE_SIDE);
            (check_transmute, apply_transmute)
        }
        Orb::Augmentation => (check_augment, apply_augment),
        Orb::Regal => (check_regal, apply_regal),
        Orb::Exalted => (check_exalt, apply_exalt),
        Orb::Chaos => {
            add(true, A_CHAOS_REFILL);
            add(true, A_FRACTURED_KEPT);
            add(action.has(Omen::Whittling), A_WHITTLING_TIES);
            add(action.has(Omen::Whittling), A_WHITTLING_SCOPE);
            (check_chaos, apply_chaos)
        }
        Orb::Annulment => {
            add(true, A_FRACTURED_KEPT);
            (check_annul, apply_annul)
        }
        Orb::Alchemy => {
            add(true, A_ALCHEMY_DRAWS);
            (check_alchemy, apply_alchemy)
        }
        // Modifier values are not part of the planned item, so a Divine Orb
        // (or its Blessed form, which rerolls implicits only) leaves the
        // item as it was: tiers are kept.
        Orb::Divine => (check_divine, |_, s, _, _| Outcome::Applied(s.clone())),
        Orb::Fracturing => {
            add(true, A_FRACTURE_UNIFORM);
            (check_fracture, apply_fracture)
        }
        Orb::Mirror => (check_unlocked, |_, s, _, _| {
            let mut copy = s.clone();
            copy.mirrored = true;
            Outcome::Applied(copy)
        }),
        Orb::Vaal => {
            return Rule::Unknown(
                "Vaal Orb \"Modifies an item unpredictably and Corrupts it\"; its outcomes on gear are community-listed without percentages".to_string(),
            )
        }
        Orb::Chance => {
            return Rule::Unknown(
                "Orb of Chance \"Unpredictably either upgrades a Normal item to Unique rarity or destroys it\"; its success probability is unknown (no source)".to_string(),
            )
        }
        Orb::Artificer => {
            return Rule::Unknown(
                "Artificer's Orb \"Adds an Augment Socket\"; the Socket cap per item class is unknown".to_string(),
            )
        }
        Orb::HinekorasLock => {
            return Rule::Unknown(
                "Hinekora's Lock lets an item \"foresee the result of the next Currency item used on it\"; whether it previews omen, essence or bone results is unknown".to_string(),
            )
        }
    };
    add(graded, A_FLOOR_FALLBACK);
    add(omened, A_OMEN_CONSUMED);
    Rule::Live(Spec { preconditions, apply, reroll: None, assumptions, source: Confidence::Game })
}

fn essence_rule(action: &Action, tier: EssenceTier) -> Rule {
    let mut assumptions = vec![A_ESSENCE_LEVEL];
    let (preconditions, apply): (CheckFn, ApplyFn) = if tier.replaces() {
        assumptions.push(A_FRACTURED_KEPT);
        (check_replacing_essence, apply_replacing_essence)
    } else {
        (check_upgrading_essence, apply_upgrading_essence)
    };
    if !action.omens().is_empty() {
        assumptions.push(A_OMEN_CONSUMED);
    }
    Rule::Live(Spec { preconditions, apply, reroll: None, assumptions, source: Confidence::Game })
}

fn bone_rule(action: &Action, kind: BoneKind) -> Rule {
    if kind == BoneKind::Cranium {
        return Rule::Unknown(
            "a Preserved Cranium desecrates a Rare Jewel, and a jewel's prefix/suffix split is unknown (rare jewels hold at most 4 modifiers by community sources only)".to_string(),
        );
    }
    let mut assumptions = vec![A_DESECRATE_FULL, A_FRACTURED_KEPT];
    if action.orb_floor() > 0 {
        assumptions.push(A_FLOOR_FALLBACK);
    }
    if !action.omens().is_empty() {
        assumptions.push(A_OMEN_CONSUMED);
    }
    let reroll: Option<ApplyFn> = if action.has(Omen::AbyssalEchoes) { Some(reroll_desecration) } else { None };
    Rule::Live(Spec { preconditions: check_bone, apply: apply_bone, reroll, assumptions, source: Confidence::Game })
}

// ------------------------------------------------------------------ shared pieces

fn has_tag(state: &ItemState, tags: &[&str]) -> bool {
    state.base_tags.iter().any(|t| tags.contains(&t.as_str()))
}

fn side_word(kind: AffixKind) -> &'static str {
    match kind {
        AffixKind::Prefix => "prefix",
        AffixKind::Suffix => "suffix",
    }
}

fn check_unlocked(_: &Action, state: &ItemState) -> Result<(), String> {
    if state.corrupted {
        Err("the item is corrupted and cannot be modified".into())
    } else if state.mirrored {
        Err("the item is mirrored and cannot be modified".into())
    } else if state.sanctified {
        Err("the item is Sanctified, which blocks further crafting".into())
    } else {
        Ok(())
    }
}

/// Adding a modifier needs the item's affix limits, which the rulebook has
/// only for equipment and waystones.
fn check_limits_known(state: &ItemState) -> Result<(), String> {
    if has_tag(state, &["jewel", "flask", "charm", "tablet"]) {
        return Err("unknown: the rulebook has no sourced prefix and suffix limits for jewels, flasks, charms and tablets".into());
    }
    Ok(())
}

fn check_rarity(state: &ItemState, want: Rarity, word: &str) -> Result<(), String> {
    if state.rarity == want {
        Ok(())
    } else {
        Err(format!("needs a {word} item"))
    }
}

/// Positions of the modifiers a removal may take: never a fractured one,
/// narrowed to one side and to desecrated modifiers when asked.
fn removable(state: &ItemState, side: Option<AffixKind>, desecrated_only: bool) -> Vec<usize> {
    state
        .mods
        .iter()
        .enumerate()
        .filter(|(_, m)| m.source != Source::Fractured)
        .filter(|(_, m)| side.is_none_or(|k| m.kind == k))
        .filter(|(_, m)| !desecrated_only || m.source == Source::Desecrated)
        .map(|(i, _)| i)
        .collect()
}

/// Uniform over `n` choices; one choice takes no draw.
fn uniform(draw: &mut dyn Draw, n: usize) -> usize {
    if n <= 1 {
        0
    } else {
        draw.below(n).min(n - 1)
    }
}

/// Removes one of `positions` uniformly and returns it.
fn remove_one(state: &mut ItemState, positions: &[usize], draw: &mut dyn Draw) -> Option<ModOn> {
    if positions.is_empty() {
        return None;
    }
    let at = positions[uniform(draw, positions.len())];
    Some(state.mods.remove(at))
}

/// Entries `query` returns at `floor`, plus, for each family none of whose
/// rollable tiers reaches the floor, that family's highest tier. The pool
/// sees only tiers the item level allows, so "every tier below the floor"
/// is judged over those.
fn with_fallback(floor: u32, query: impl Fn(u32) -> Vec<Candidate>) -> Vec<Candidate> {
    let mut out = query(floor);
    if floor == 0 {
        return out;
    }
    let passing: HashSet<String> = out.iter().map(|c| c.family.clone()).collect();
    let mut best: Vec<Candidate> = Vec::new();
    for c in query(0) {
        if passing.contains(&c.family) {
            continue;
        }
        match best.iter_mut().find(|b| b.family == c.family) {
            Some(b) if c.required_level > b.required_level || (c.required_level == b.required_level && c.tier < b.tier) => *b = c,
            Some(_) => {}
            None => best.push(c),
        }
    }
    out.extend(best);
    out
}

/// Every random entry that can roll on `state` on the sides in `sides`
/// that have room, with the Minimum Modifier Level applied.
fn rollable(state: &ItemState, sides: &[AffixKind], floor: u32, pool: &dyn PoolView) -> Vec<Candidate> {
    sides
        .iter()
        .filter(|k| state.open(**k) > 0)
        .flat_map(|k| with_fallback(floor, |f| pool.eligible(state, *k, f)))
        .collect()
}

const BOTH: [AffixKind; 2] = [AffixKind::Prefix, AffixKind::Suffix];

fn sides_for(side: Option<AffixKind>) -> Vec<AffixKind> {
    match side {
        Some(k) => vec![k],
        None => BOTH.to_vec(),
    }
}

/// Adds one random modifier from `sides`; the error says nothing can roll.
fn roll_one(state: &mut ItemState, sides: &[AffixKind], floor: u32, pool: &dyn PoolView, draw: &mut dyn Draw) -> Result<(), String> {
    let candidates = rollable(state, sides, floor, pool);
    let picked = draw.pick(&candidates).and_then(|i| candidates.get(i)).ok_or_else(|| "no modifier can roll on this item".to_string())?;
    state.mods.push(picked.to_mod(Source::Random));
    Ok(())
}

fn room_on(state: &ItemState, side: Option<AffixKind>) -> Result<(), String> {
    match side {
        Some(k) if state.open(k) == 0 => Err(format!("no open {} on the item", side_word(k))),
        None if state.open(AffixKind::Prefix) + state.open(AffixKind::Suffix) == 0 => Err("no open affix on the item".into()),
        _ => Ok(()),
    }
}

fn done(result: Result<ItemState, String>) -> Outcome {
    match result {
        Ok(s) => Outcome::Applied(s),
        Err(r) => Outcome::Refused(r),
    }
}

// ------------------------------------------------------------------ orbs

fn check_transmute(a: &Action, s: &ItemState) -> Result<(), String> {
    check_unlocked(a, s)?;
    check_limits_known(s)?;
    check_rarity(s, Rarity::Normal, "Normal")
}

/// "Upgrades a Normal item to a Magic item with 1 modifier". The side is a
/// coin flip; when the drawn side has nothing to roll, the other side is
/// used.
fn apply_transmute(a: &Action, s: &ItemState, pool: &dyn PoolView, draw: &mut dyn Draw) -> Outcome {
    let mut next = s.clone();
    next.rarity = Rarity::Magic;
    let first = if draw.below(2) == 0 { AffixKind::Prefix } else { AffixKind::Suffix };
    let floor = a.orb_floor();
    let side = if rollable(&next, &[first], floor, pool).is_empty() { first.other() } else { first };
    done(roll_one(&mut next, &[side], floor, pool, draw).map(|_| next))
}

fn check_augment(a: &Action, s: &ItemState) -> Result<(), String> {
    check_unlocked(a, s)?;
    check_limits_known(s)?;
    check_rarity(s, Rarity::Magic, "Magic")?;
    room_on(s, None)
}

/// "Augments a Magic item with a new random modifier": from a side with room.
fn apply_augment(a: &Action, s: &ItemState, pool: &dyn PoolView, draw: &mut dyn Draw) -> Outcome {
    let mut next = s.clone();
    done(roll_one(&mut next, &BOTH, a.orb_floor(), pool, draw).map(|_| next))
}

fn check_regal(a: &Action, s: &ItemState) -> Result<(), String> {
    check_unlocked(a, s)?;
    check_limits_known(s)?;
    check_rarity(s, Rarity::Magic, "Magic")
}

/// "Upgrades a Magic item to a Rare item, adding 1 modifier … Current
/// modifiers are retained".
fn apply_regal(a: &Action, s: &ItemState, pool: &dyn PoolView, draw: &mut dyn Draw) -> Outcome {
    let mut next = s.clone();
    next.rarity = Rarity::Rare;
    done(roll_one(&mut next, &BOTH, a.orb_floor(), pool, draw).map(|_| next))
}

fn check_exalt(a: &Action, s: &ItemState) -> Result<(), String> {
    check_unlocked(a, s)?;
    check_limits_known(s)?;
    check_rarity(s, Rarity::Rare, "Rare")?;
    room_on(s, a.side())
}

/// "Augments a Rare item with a new random modifier"; Sinistral/Dextral
/// Exaltation keep it to one side, Greater Exaltation adds two. When only
/// one fits, one is added and the omen is spent.
fn apply_exalt(a: &Action, s: &ItemState, pool: &dyn PoolView, draw: &mut dyn Draw) -> Outcome {
    let mut next = s.clone();
    let sides = sides_for(a.side());
    let floor = a.orb_floor();
    let adds = if a.has(Omen::GreaterExaltation) { 2 } else { 1 };
    for n in 0..adds {
        if let Err(reason) = roll_one(&mut next, &sides, floor, pool, draw) {
            if n == 0 {
                return Outcome::Refused(reason);
            }
            break;
        }
    }
    Outcome::Applied(next)
}

fn check_chaos(a: &Action, s: &ItemState) -> Result<(), String> {
    check_unlocked(a, s)?;
    check_limits_known(s)?;
    check_rarity(s, Rarity::Rare, "Rare")?;
    let side = a.side();
    let targets = removable(s, side, false);
    if targets.is_empty() {
        return Err(match side {
            Some(k) => format!("no {} the Chaos Orb can remove", side_word(k)),
            None => "no modifier the Chaos Orb can remove".into(),
        });
    }
    if a.has(Omen::Whittling) {
        if let Some(m) = targets.iter().map(|&i| &s.mods[i]).find(|m| m.required_level.is_none()) {
            return Err(format!("the level of \"{}\" is not known, so which modifier Whittling removes cannot be told", m.text));
        }
    }
    Ok(())
}

/// "Removes a random modifier and augments a Rare item with a new random
/// modifier". Erasure keeps the removal to one side; Whittling removes the
/// lowest-level modifier. The new modifier comes from any side with room.
fn apply_chaos(a: &Action, s: &ItemState, pool: &dyn PoolView, draw: &mut dyn Draw) -> Outcome {
    let mut next = s.clone();
    let mut targets = removable(&next, a.side(), false);
    if a.has(Omen::Whittling) {
        let lowest = targets.iter().filter_map(|&i| next.mods[i].required_level).min();
        targets.retain(|&i| next.mods[i].required_level == lowest);
    }
    if remove_one(&mut next, &targets, draw).is_none() {
        return Outcome::Refused("no modifier the Chaos Orb can remove".into());
    }
    done(roll_one(&mut next, &BOTH, a.orb_floor(), pool, draw).map(|_| next))
}

fn check_annul(a: &Action, s: &ItemState) -> Result<(), String> {
    check_unlocked(a, s)?;
    let light = a.has(Omen::Light);
    if removable(s, a.side(), light).is_empty() {
        return Err(match (a.side(), light) {
            (_, true) => "no Desecrated modifier the Orb of Annulment can remove".into(),
            (Some(k), false) => format!("no {} the Orb of Annulment can remove", side_word(k)),
            (None, false) => "no modifier the Orb of Annulment can remove".into(),
        });
    }
    Ok(())
}

/// "Removes a random modifier from an item"; the side omens and Light
/// narrow which.
fn apply_annul(a: &Action, s: &ItemState, _: &dyn PoolView, draw: &mut dyn Draw) -> Outcome {
    let mut next = s.clone();
    let targets = removable(&next, a.side(), a.has(Omen::Light));
    match remove_one(&mut next, &targets, draw) {
        Some(_) => Outcome::Applied(next),
        None => Outcome::Refused("no modifier the Orb of Annulment can remove".into()),
    }
}

fn check_alchemy(a: &Action, s: &ItemState) -> Result<(), String> {
    check_unlocked(a, s)?;
    check_limits_known(s)?;
    match s.rarity {
        Rarity::Normal | Rarity::Magic => Ok(()),
        _ => Err("needs a Normal or Magic item".into()),
    }
}

/// "Upgrades a Normal or Magic item to a Rare item with 4 random
/// modifiers"; a magic item's modifiers are not kept.
fn apply_alchemy(_: &Action, s: &ItemState, pool: &dyn PoolView, draw: &mut dyn Draw) -> Outcome {
    let mut next = s.clone();
    next.rarity = Rarity::Rare;
    next.mods.clear();
    for _ in 0..4 {
        if let Err(reason) = roll_one(&mut next, &BOTH, 0, pool, draw) {
            return Outcome::Refused(reason);
        }
    }
    Outcome::Applied(next)
}

fn check_divine(a: &Action, s: &ItemState) -> Result<(), String> {
    check_unlocked(a, s)?;
    if s.mods.is_empty() && !a.has(Omen::Blessed) {
        return Err("the item has no modifier to reroll".into());
    }
    Ok(())
}

fn check_fracture(a: &Action, s: &ItemState) -> Result<(), String> {
    check_unlocked(a, s)?;
    check_rarity(s, Rarity::Rare, "Rare")?;
    if s.fractured() {
        return Err("the item already has a fractured modifier".into());
    }
    if s.mods.len() < 4 {
        return Err("needs a rare item with at least 4 modifiers".into());
    }
    Ok(())
}

/// "Fracture a random modifier on a rare item with at least 4 modifiers,
/// locking it in place."
fn apply_fracture(_: &Action, s: &ItemState, _: &dyn PoolView, draw: &mut dyn Draw) -> Outcome {
    let mut next = s.clone();
    let at = uniform(draw, next.mods.len());
    next.mods[at].source = Source::Fractured;
    Outcome::Applied(next)
}

// ------------------------------------------------------------------ essences

fn essence_entry(a: &Action, s: &ItemState, pool: &dyn PoolView) -> Result<Candidate, String> {
    let name = a.essence_name().unwrap_or_default();
    match pool.essence(&name, s) {
        EssenceOutcome::Entry(c) => Ok(c),
        EssenceOutcome::Unknown(reason) => Err(format!("unknown: {reason}")),
    }
}

fn group_taken(state: &ItemState, c: &Candidate) -> bool {
    c.groups.iter().any(|g| state.groups().any(|h| h == g))
}

const ESSENCE_GROUP_UNKNOWN: &str =
    "unknown: the essence's modifier shares a group with one already on the item, and the rulebook does not say what an essence does then";

/// `what` is "an essence" or "an Alloy".
fn check_crafted_slot(s: &ItemState, what: &str) -> Result<(), String> {
    if s.crafted_slot_used() {
        return Err(format!("unknown: the item already has its one crafted modifier, and the rulebook does not say what {what} does then"));
    }
    Ok(())
}

fn check_upgrading_essence(a: &Action, s: &ItemState) -> Result<(), String> {
    check_unlocked(a, s)?;
    check_limits_known(s)?;
    check_rarity(s, Rarity::Magic, "Magic")?;
    check_crafted_slot(s, "an essence")
}

/// "Upgrades a Magic item to a Rare item, adding a guaranteed modifier":
/// the modifier the essence data names for the item's class, in the one
/// crafted slot.
fn apply_upgrading_essence(a: &Action, s: &ItemState, pool: &dyn PoolView, _: &mut dyn Draw) -> Outcome {
    let entry = match essence_entry(a, s, pool) {
        Ok(c) => c,
        Err(r) => return Outcome::Refused(r),
    };
    if group_taken(s, &entry) {
        return Outcome::Refused(ESSENCE_GROUP_UNKNOWN.into());
    }
    let mut next = s.clone();
    next.rarity = Rarity::Rare;
    next.mods.push(entry.to_mod(Source::Crafted));
    Outcome::Applied(next)
}

fn check_replacing_essence(a: &Action, s: &ItemState) -> Result<(), String> {
    check_unlocked(a, s)?;
    check_limits_known(s)?;
    check_rarity(s, Rarity::Rare, "Rare")?;
    check_crafted_slot(s, "an essence")?;
    if removable(s, a.side(), false).is_empty() {
        return Err(match a.side() {
            Some(k) => format!("no {} the essence can remove", side_word(k)),
            None => "no modifier the essence can remove".into(),
        });
    }
    Ok(())
}

/// "Removes a random modifier and augments a Rare item with a new
/// guaranteed modifier"; Crystallisation keeps the removal to one side.
fn apply_replacing_essence(a: &Action, s: &ItemState, pool: &dyn PoolView, draw: &mut dyn Draw) -> Outcome {
    let entry = match essence_entry(a, s, pool) {
        Ok(c) => c,
        Err(r) => return Outcome::Refused(r),
    };
    let mut next = s.clone();
    let targets = removable(&next, a.side(), false);
    if remove_one(&mut next, &targets, draw).is_none() {
        return Outcome::Refused("no modifier the essence can remove".into());
    }
    if next.open(entry.kind) == 0 {
        return Outcome::Refused(format!(
            "unknown: the removed modifier left no open {} for the essence's modifier, and the rulebook does not say what happens then",
            side_word(entry.kind)
        ));
    }
    if group_taken(&next, &entry) {
        return Outcome::Refused(ESSENCE_GROUP_UNKNOWN.into());
    }
    next.mods.push(entry.to_mod(Source::Crafted));
    Outcome::Applied(next)
}

// ------------------------------------------------------------------ alloys

fn check_alloy(a: &Action, s: &ItemState) -> Result<(), String> {
    check_unlocked(a, s)?;
    check_limits_known(s)?;
    check_rarity(s, Rarity::Rare, "Rare")?;
    check_crafted_slot(s, "an Alloy")?;
    if removable(s, None, false).is_empty() {
        return Err("no modifier the Alloy can remove".into());
    }
    Ok(())
}

/// "Removes a random modifier and augments a Rare item with a new
/// guaranteed modifier" (every Alloy's item text): the modifier the Alloy
/// table names for the item's class, in the one crafted slot. An Alloy
/// with no line for the class, or a line no entry joins, is refused with
/// the reason before anything is removed.
fn apply_alloy(a: &Action, s: &ItemState, pool: &dyn PoolView, draw: &mut dyn Draw) -> Outcome {
    let Action::Alloy { name } = a else {
        return Outcome::Refused("not an Alloy".into());
    };
    let entry = match pool.alloy(name, s) {
        EssenceOutcome::Entry(c) => c,
        EssenceOutcome::Unknown(reason) => return Outcome::Refused(format!("unknown: {reason}")),
    };
    let mut next = s.clone();
    let targets = removable(&next, None, false);
    if remove_one(&mut next, &targets, draw).is_none() {
        return Outcome::Refused("no modifier the Alloy can remove".into());
    }
    if next.open(entry.kind) == 0 {
        return Outcome::Refused(format!(
            "unknown: the removed modifier left no open {} for the Alloy's modifier, and the rulebook does not say what happens then",
            side_word(entry.kind)
        ));
    }
    if group_taken(&next, &entry) {
        return Outcome::Refused(
            "unknown: the Alloy's modifier shares a group with one already on the item, and the rulebook does not say what an Alloy does then".into(),
        );
    }
    next.mods.push(entry.to_mod(Source::Crafted));
    Outcome::Applied(next)
}

// ------------------------------------------------------------------ desecration

fn check_bone(a: &Action, s: &ItemState) -> Result<(), String> {
    let Action::Bone { kind, grade, .. } = a else {
        return Err("not a desecration".into());
    };
    check_unlocked(a, s)?;
    check_limits_known(s)?;
    if s.rarity != Rarity::Rare || !kind.takes(s) {
        return Err(format!("a {} {} desecrates {}", grade.name(), kind.name(), kind.target()));
    }
    if *grade == BoneGrade::Gnawed && s.item_level > 64 {
        return Err(format!("a Gnawed bone has \"Maximum Item Level: 64\"; this item is level {}", s.item_level));
    }
    if s.desecrated_slot_used() {
        return Err("the item already has its one Desecrated modifier".into());
    }
    if a.lich().is_some() {
        let weapon = has_tag(s, &["weapon", "wand", "staff", "sceptre", "talisman"]);
        let jewellery = has_tag(s, &["amulet", "ring"]);
        if !weapon && !jewellery {
            return Err("the lich omens act on a Weapon or Jewellery Desecration only".into());
        }
    }
    let side = a.side();
    if room_on(s, side).is_err() && removable(s, side, false).is_empty() {
        return Err("the item has no open affix and no modifier the desecration can replace".into());
    }
    Ok(())
}

/// Three different desecrated options for `base`, which already has room.
fn reveal(a: &Action, base: &ItemState, pool: &dyn PoolView, draw: &mut dyn Draw) -> Outcome {
    let open: Vec<AffixKind> = sides_for(a.side()).into_iter().filter(|k| base.open(*k) > 0).collect();
    let kind = match open.as_slice() {
        [one] => Some(*one),
        _ => None,
    };
    reveal_options(base, kind, a.lich(), a.orb_floor(), pool, draw)
}

/// Three different desecrated options of `kind` (either side when `None`)
/// and `lich` for `base`.
fn reveal_options(
    base: &ItemState,
    kind: Option<AffixKind>,
    lich: Option<Lich>,
    floor: u32,
    pool: &dyn PoolView,
    draw: &mut dyn Draw,
) -> Outcome {
    let mut candidates = with_fallback(floor, |f| pool.desecrated(base, kind, lich, f));
    let mut options = Vec::new();
    while options.len() < 3 {
        let Some(i) = draw.pick(&candidates).filter(|i| *i < candidates.len()) else {
            break;
        };
        let chosen = candidates.remove(i);
        let mut option = base.clone();
        option.mods.push(chosen.to_mod(Source::Desecrated));
        options.push(option);
    }
    if options.is_empty() {
        return Outcome::Refused("no desecrated modifier can roll on this item".into());
    }
    Outcome::Reveal(options)
}

/// A bone adds one unrevealed desecrated modifier; at the Well of Souls it
/// is revealed as one of three options. Necromancy keeps it to one side,
/// the lich omens to one lich. An item with no room on that side first
/// loses a random modifier there.
fn apply_bone(a: &Action, s: &ItemState, pool: &dyn PoolView, draw: &mut dyn Draw) -> Outcome {
    let mut base = s.clone();
    let side = a.side();
    if room_on(&base, side).is_err() {
        let targets = removable(&base, side, false);
        if remove_one(&mut base, &targets, draw).is_none() {
            return Outcome::Refused("the item has no open affix and no modifier the desecration can replace".into());
        }
    }
    reveal(a, &base, pool, draw)
}

/// Omen of Abyssal Echoes: "you can reroll the options once". Given any
/// option of the first reveal, draws three fresh options for the same item.
fn reroll_desecration(a: &Action, option: &ItemState, pool: &dyn PoolView, draw: &mut dyn Draw) -> Outcome {
    if !matches!(a, Action::Bone { .. } | Action::Reveal { .. }) || !a.has(Omen::AbyssalEchoes) {
        return Outcome::Refused("only a reveal with Omen of Abyssal Echoes can reroll its options".into());
    }
    let side = option.mods.iter().find(|m| m.source == Source::Desecrated).map(|m| m.kind);
    let mut base = option.clone();
    base.mods.retain(|m| m.source != Source::Desecrated);
    match a {
        // An unrevealed modifier's options stay on its own side.
        Action::Reveal { .. } => reveal_options(&base, side, None, 0, pool, draw),
        _ => reveal(a, &base, pool, draw),
    }
}

/// A reveal needs a desecrated modifier the item carries unrevealed.
fn check_reveal(a: &Action, s: &ItemState) -> Result<(), String> {
    check_unlocked(a, s)?;
    if s.mods.iter().any(|m| m.unrevealed()) {
        Ok(())
    } else {
        Err("the item carries no unrevealed desecrated modifier".into())
    }
}

/// "you can reveal the Desecrated modifier and choose to transform it by
/// selecting one of three different options" (0.3.0): the options are
/// desecrated modifiers of the unrevealed one's side.
fn apply_reveal(_: &Action, s: &ItemState, pool: &dyn PoolView, draw: &mut dyn Draw) -> Outcome {
    let mut base = s.clone();
    let Some(at) = base.mods.iter().position(|m| m.unrevealed()) else {
        return Outcome::Refused("the item carries no unrevealed desecrated modifier".into());
    };
    let kind = base.mods.remove(at).kind;
    reveal_options(&base, Some(kind), None, 0, pool, draw)
}
