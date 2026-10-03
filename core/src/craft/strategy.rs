//! The strategy library: chains of actions with stop and repeat rules,
//! parameterised by the target.
//!
//! | id | shape | applies to |
//! |---|---|---|
//! | orb-chain | Transmute, Augment, Regal, then Exalted slams (a Sinistral/Dextral omen when every missing family is on one side with room), Annulment when the item is full | any item that is not corrupted, mirrored or sanctified |
//! | essence-start | (Transmute if normal), an essence with a wanted family, then Exalted slams | a normal or magic item, when a wanted family has an essence |
//! | perfect-essence-fix | a Perfect or corrupted essence with a Crystallisation omen on the bad modifier's side | a rare with exactly one bad modifier and an essence for what is missing |
//! | alloy-fix | an Alloy (it removes a random modifier and adds its own in the crafted slot), then Exalted slams | a rare with its crafted slot free, when an Alloy adds a wanted family at a wanted tier |
//! | chaos-spam | Chaos with Erasure or Whittling until the target holds; Annulment for a last blocker | a rare with at least three wanted modifiers present |
//! | desecrate | a bone and its reveal (Abyssal Echoes reroll) with Necromancy and lich omens for the one lich modifier | a wanted family that only desecration adds |
//! | fracture-then-chaos | Fracturing Orb, then chaos-spam | a rare of 4+ modifiers whose only wanted modifier is a tier 1 |
//! | alchemy-start | Alchemy (four modifiers), then Annulment and Exalted slams | a normal item with three or more wanted families |
//! | buy | the cheapest listings of a trade search for the finished item | always: the baseline every plan is compared with |
//!
//! Every strategy is also left out when its own actions can never add a
//! wanted family (every run would give up), and Greater and Perfect orbs
//! are tried where their Minimum Modifier Level removes tiers the target
//! does not want and keeps every tier it does. Actions go through the rules
//! table; nothing here decides what an orb does.

use super::model::Model;
use super::rules::{self, Action, BoneGrade, BoneKind, EssenceTier, Grade, Omen, Orb, Rule, ALLOYS, CORRUPTED_ESSENCES, TIERED_ESSENCES};
use super::sim;
use super::types::{AffixKind, Candidate, EssenceOutcome, ItemState, ModOn, Outcome, PoolView, Rarity, Source};

/// One wanted family with its worst acceptable tier.
#[derive(Debug, Clone, PartialEq)]
pub struct Want {
    /// The mod database's family ("IncreasedLife").
    pub family: String,
    pub kind: AffixKind,
    /// Trade-site numbering: 1 is the best tier, so a tier at or below this
    /// number is acceptable.
    pub min_tier: u8,
}

impl Want {
    pub fn met_by(&self, m: &ModOn) -> bool {
        m.family == self.family && m.kind == self.kind && m.tier.is_some_and(|t| t <= self.min_tier)
    }

    pub fn accepts(&self, c: &Candidate) -> bool {
        c.family == self.family && c.kind == self.kind && c.tier <= self.min_tier
    }
}

/// The finished item: every wanted family at its minimum tier or better.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Target {
    pub wants: Vec<Want>,
}

impl Target {
    pub fn holds(&self, state: &ItemState) -> bool {
        self.wants.iter().all(|w| state.mods.iter().any(|m| w.met_by(m)))
    }

    /// Whether `m` meets one of the wants.
    pub fn wanted(&self, m: &ModOn) -> bool {
        self.wants.iter().any(|w| w.met_by(m))
    }

    /// Wants no modifier on the item meets yet.
    pub fn missing(&self, state: &ItemState) -> Vec<&Want> {
        self.wants.iter().filter(|w| !state.mods.iter().any(|m| w.met_by(m))).collect()
    }

    /// Whether a want of `kind` has no modifier of its family on the item
    /// at all (at any tier).
    fn absent_on(&self, state: &ItemState, kind: AffixKind) -> bool {
        self.wants.iter().any(|w| w.kind == kind && !state.mods.iter().any(|m| m.family == w.family))
    }
}

/// The library's entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StrategyId {
    OrbChain,
    EssenceStart,
    PerfectEssenceFix,
    AlloyFix,
    ChaosSpam,
    Desecrate,
    FractureThenChaos,
    AlchemyStart,
    Buy,
}

impl StrategyId {
    pub const ALL: [StrategyId; 9] = [
        StrategyId::OrbChain,
        StrategyId::EssenceStart,
        StrategyId::PerfectEssenceFix,
        StrategyId::AlloyFix,
        StrategyId::ChaosSpam,
        StrategyId::Desecrate,
        StrategyId::FractureThenChaos,
        StrategyId::AlchemyStart,
        StrategyId::Buy,
    ];

    pub fn name(self) -> &'static str {
        match self {
            StrategyId::OrbChain => "orb-chain",
            StrategyId::EssenceStart => "essence-start",
            StrategyId::PerfectEssenceFix => "perfect-essence-fix",
            StrategyId::AlloyFix => "alloy-fix",
            StrategyId::ChaosSpam => "chaos-spam",
            StrategyId::Desecrate => "desecrate",
            StrategyId::FractureThenChaos => "fracture-then-chaos",
            StrategyId::AlchemyStart => "alchemy-start",
            StrategyId::Buy => "buy",
        }
    }
}

/// One strategy with its variant: the orb grades, the essence or the bone
/// it uses.
#[derive(Debug, Clone, PartialEq)]
pub enum Strategy {
    /// `magic` grades Transmutation and Augmentation, `rare` grades Regal
    /// and Exalted. `clear`: how a full side that blocks a missing family is
    /// handled (see [`finish`]).
    OrbChain { magic: Grade, rare: Grade, clear: bool },
    /// `groups` are the groups of the essence's modifier, which a magic
    /// item's own modifier must not hold.
    EssenceStart { essence: Action, groups: Vec<String>, rare: Grade, clear: bool },
    /// The essence with its Crystallisation omen.
    PerfectEssenceFix { essence: Action },
    /// The Alloy, and the side of the modifier it adds.
    AlloyFix { alloy: Action, kind: AffixKind },
    ChaosSpam { grade: Grade },
    /// The bone with its omens.
    Desecrate { bone: Action },
    FractureThenChaos { grade: Grade },
    AlchemyStart { rare: Grade, clear: bool },
    Buy,
}

/// What a strategy does next.
#[derive(Debug, Clone, PartialEq)]
pub enum Next {
    Act(Action),
    /// The action repeats from an equivalent item until one use completes
    /// the target: a geometric trial, costed by its closed form.
    Retry(Action),
    /// The strategy cannot go on from this item, and why.
    Stop(String),
}

fn grade_word(g: Grade) -> &'static str {
    match g {
        Grade::Normal => "",
        Grade::Greater => "Greater",
        Grade::Perfect => "Perfect",
    }
}

impl Strategy {
    pub fn id(&self) -> StrategyId {
        match self {
            Strategy::OrbChain { .. } => StrategyId::OrbChain,
            Strategy::EssenceStart { .. } => StrategyId::EssenceStart,
            Strategy::PerfectEssenceFix { .. } => StrategyId::PerfectEssenceFix,
            Strategy::AlloyFix { .. } => StrategyId::AlloyFix,
            Strategy::ChaosSpam { .. } => StrategyId::ChaosSpam,
            Strategy::Desecrate { .. } => StrategyId::Desecrate,
            Strategy::FractureThenChaos { .. } => StrategyId::FractureThenChaos,
            Strategy::AlchemyStart { .. } => StrategyId::AlchemyStart,
            Strategy::Buy => StrategyId::Buy,
        }
    }

    /// The strategy as a plan names it: its id, then what sets this variant
    /// apart ("orb-chain (Greater Regal and Exalted)").
    pub fn name(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        match self {
            Strategy::OrbChain { magic, rare, .. } => {
                if *magic != Grade::Normal {
                    parts.push(format!("{} Transmutation and Augmentation", grade_word(*magic)));
                }
                if *rare != Grade::Normal {
                    parts.push(format!("{} Regal and Exalted", grade_word(*rare)));
                }
            }
            Strategy::EssenceStart { essence, rare, .. } => {
                parts.push(essence.name());
                if *rare != Grade::Normal {
                    parts.push(format!("{} Exalted", grade_word(*rare)));
                }
            }
            Strategy::PerfectEssenceFix { essence } => parts.push(essence.name()),
            Strategy::AlloyFix { alloy, .. } => parts.push(alloy.name()),
            Strategy::ChaosSpam { grade } | Strategy::FractureThenChaos { grade } => {
                if *grade != Grade::Normal {
                    parts.push(format!("{} Chaos", grade_word(*grade)));
                }
            }
            Strategy::Desecrate { bone } => parts.push(bone.name()),
            Strategy::AlchemyStart { rare, .. } => {
                if *rare != Grade::Normal {
                    parts.push(format!("{} Exalted", grade_word(*rare)));
                }
            }
            Strategy::Buy => {}
        }
        if self.clears_blockers() == Some(true) {
            parts.push("clearing blockers first".to_string());
        }
        let id = self.id().name();
        if parts.is_empty() {
            id.to_string()
        } else {
            format!("{id} ({})", parts.join(", "))
        }
    }

    /// Whether this strategy clears a blocking side first, for the
    /// strategies that finish with slams; `None` for the others.
    pub fn clears_blockers(&self) -> Option<bool> {
        match self {
            Strategy::OrbChain { clear, .. } | Strategy::EssenceStart { clear, .. } | Strategy::AlchemyStart { clear, .. } => {
                Some(*clear)
            }
            _ => None,
        }
    }

    /// The same strategy handling a blocking side the other way, for the
    /// strategies that finish with slams.
    pub fn other_blocker_route(&self) -> Option<Strategy> {
        let mut other = self.clone();
        match &mut other {
            Strategy::OrbChain { clear, .. } | Strategy::EssenceStart { clear, .. } | Strategy::AlchemyStart { clear, .. } => {
                *clear = !*clear;
                Some(other)
            }
            _ => None,
        }
    }

    /// The next action from `state`, which the target does not hold on yet.
    pub fn next(&self, state: &ItemState, target: &Target) -> Next {
        if let Some(reason) = locked(state) {
            return Next::Stop(reason);
        }
        match self {
            Strategy::OrbChain { magic, rare, clear } => match state.rarity {
                Rarity::Normal => Next::Act(Action::graded(Orb::Transmutation, *magic)),
                Rarity::Magic if has_room(state) => Next::Act(Action::graded(Orb::Augmentation, *magic)),
                Rarity::Magic => Next::Act(Action::graded(Orb::Regal, *rare)),
                Rarity::Rare => finish(state, target, *rare, *clear),
                Rarity::Unique => Next::Stop(UNIQUE.into()),
            },
            Strategy::EssenceStart { essence, groups, rare, clear } => match state.rarity {
                Rarity::Normal => Next::Act(Action::orb(Orb::Transmutation)),
                // The essence's modifier cannot join a modifier of its own
                // group, so that modifier goes first.
                Rarity::Magic if state.groups().any(|g| groups.iter().any(|h| h == g)) => {
                    Next::Act(Action::orb(Orb::Annulment))
                }
                Rarity::Magic => Next::Act(essence.clone()),
                Rarity::Rare => finish(state, target, *rare, *clear),
                Rarity::Unique => Next::Stop(UNIQUE.into()),
            },
            Strategy::PerfectEssenceFix { essence } => match state.rarity {
                Rarity::Rare if !state.crafted_slot_used() => Next::Act(essence.clone()),
                Rarity::Rare => finish(state, target, Grade::Normal, false),
                _ => Next::Stop("the essence swaps a modifier on a rare item only".into()),
            },
            Strategy::AlloyFix { alloy, kind } => match state.rarity {
                Rarity::Rare if !state.crafted_slot_used() => alloy_step(state, target, alloy, *kind),
                Rarity::Rare => finish(state, target, Grade::Normal, false),
                _ => Next::Stop("the Alloy swaps a modifier on a rare item only".into()),
            },
            Strategy::ChaosSpam { grade } => chaos_spam(state, target, *grade),
            Strategy::FractureThenChaos { grade } => {
                if state.rarity == Rarity::Rare && !state.fractured() && state.mods.len() >= 4 {
                    Next::Act(Action::orb(Orb::Fracturing))
                } else {
                    chaos_spam(state, target, *grade)
                }
            }
            Strategy::AlchemyStart { rare, clear } => match state.rarity {
                Rarity::Normal | Rarity::Magic => Next::Act(Action::orb(Orb::Alchemy)),
                Rarity::Rare => finish(state, target, *rare, *clear),
                Rarity::Unique => Next::Stop(UNIQUE.into()),
            },
            Strategy::Desecrate { bone } => match state.rarity {
                Rarity::Normal => Next::Act(Action::orb(Orb::Transmutation)),
                Rarity::Magic if has_room(state) => Next::Act(Action::orb(Orb::Augmentation)),
                Rarity::Magic => Next::Act(Action::orb(Orb::Regal)),
                Rarity::Rare => match state.mods.iter().find(|m| m.source == Source::Desecrated) {
                    None => Next::Act(bone.clone()),
                    // A modifier desecrated earlier and never revealed is
                    // revealed first: it is free, and its options may hold
                    // the wanted one. Abyssal Echoes rerolls them as it
                    // would a fresh bone's.
                    Some(m) if m.unrevealed() => {
                        let reveal = Action::Reveal { omens: vec![] };
                        Next::Act(if bone.omens().contains(&Omen::AbyssalEchoes) { reveal.with(Omen::AbyssalEchoes) } else { reveal })
                    }
                    // A desecrated modifier the target does not want holds
                    // the one desecrated slot; Omen of Light takes only it.
                    Some(m) if !target.wanted(m) => Next::Act(Action::orb(Orb::Annulment).with(Omen::Light)),
                    Some(_) => finish(state, target, Grade::Normal, false),
                },
                Rarity::Unique => Next::Stop(UNIQUE.into()),
            },
            Strategy::Buy => Next::Stop("buying is priced by a trade search, not by crafting steps".into()),
        }
    }

    /// Whether this strategy adds `want` other than by a random roll from
    /// the item's pool: an essence's guaranteed modifier, or a desecration.
    pub fn grants(&self, want: &Want, state: &ItemState, pool: &dyn PoolView) -> bool {
        match self {
            Strategy::EssenceStart { essence, .. } | Strategy::PerfectEssenceFix { essence } => {
                essence_entry(essence, state, pool).is_some_and(|c| want.accepts(&c))
            }
            Strategy::AlloyFix { alloy, .. } => alloy_entry(alloy, state, pool).is_some_and(|c| want.accepts(&c)),
            Strategy::Desecrate { .. } => desecrated_only(state, want, pool),
            _ => false,
        }
    }

    /// Whether a desecration reveal option is one to keep: it holds a
    /// desecrated modifier the target wants.
    pub fn accept(&self, option: &ItemState, target: &Target) -> bool {
        option.mods.iter().any(|m| m.source == Source::Desecrated && target.wanted(m))
    }
}

const UNIQUE: &str = "a unique item takes none of these crafting steps";

fn locked(state: &ItemState) -> Option<String> {
    if state.corrupted {
        Some("the item is corrupted and cannot be modified".into())
    } else if state.mirrored {
        Some("the item is mirrored and cannot be modified".into())
    } else if state.sanctified {
        Some("the item is Sanctified, which blocks further crafting".into())
    } else {
        None
    }
}

fn has_room(state: &ItemState) -> bool {
    state.open(AffixKind::Prefix) + state.open(AffixKind::Suffix) > 0
}

fn side_omen_exalt(kind: AffixKind) -> Omen {
    match kind {
        AffixKind::Prefix => Omen::SinistralExaltation,
        AffixKind::Suffix => Omen::DextralExaltation,
    }
}

fn side_omen_annul(kind: AffixKind) -> Omen {
    match kind {
        AffixKind::Prefix => Omen::SinistralAnnulment,
        AffixKind::Suffix => Omen::DextralAnnulment,
    }
}

fn side_omen_erasure(kind: AffixKind) -> Omen {
    match kind {
        AffixKind::Prefix => Omen::SinistralErasure,
        AffixKind::Suffix => Omen::DextralErasure,
    }
}

fn side_omen_crystallisation(kind: AffixKind) -> Omen {
    match kind {
        AffixKind::Prefix => Omen::SinistralCrystallisation,
        AffixKind::Suffix => Omen::DextralCrystallisation,
    }
}

/// The Alloy, once its modifier's side can take it whatever the Alloy
/// removes: while that side is full and the other side holds a modifier
/// the Alloy could take instead, an unwanted modifier of the full side is
/// annulled first.
fn alloy_step(state: &ItemState, target: &Target, alloy: &Action, kind: AffixKind) -> Next {
    let removable_elsewhere = state.mods.iter().any(|m| m.kind != kind && m.source != Source::Fractured);
    if state.open(kind) > 0 || !removable_elsewhere {
        return Next::Act(alloy.clone());
    }
    if !junk(state, target).iter().any(|m| m.kind == kind) {
        return Next::Stop("the Alloy's side is full of wanted modifiers, and its removal may take one from the other side".into());
    }
    Next::Act(Action::orb(Orb::Annulment).with(side_omen_annul(kind)))
}

/// Modifiers a removal can take (never a fractured one) that the target
/// does not want.
fn junk<'a>(state: &'a ItemState, target: &Target) -> Vec<&'a ModOn> {
    state.mods.iter().filter(|m| m.source != Source::Fractured && !target.wanted(m)).collect()
}

/// An Exalted Orb for the next missing family: with a Sinistral or Dextral
/// Exaltation omen when every family missing from the item is on one side
/// and that side has room.
fn slam(state: &ItemState, target: &Target, grade: Grade) -> Action {
    let exalt = Action::graded(Orb::Exalted, grade);
    let want_prefix = target.absent_on(state, AffixKind::Prefix);
    let want_suffix = target.absent_on(state, AffixKind::Suffix);
    match (want_prefix, want_suffix) {
        (true, false) if state.open(AffixKind::Prefix) > 0 => exalt.with(side_omen_exalt(AffixKind::Prefix)),
        (false, true) if state.open(AffixKind::Suffix) > 0 => exalt.with(side_omen_exalt(AffixKind::Suffix)),
        _ => exalt,
    }
}

/// The rare phase every orb strategy ends with: slam while there is room;
/// when the item is full, annul an unwanted modifier, with a Sinistral or
/// Dextral Annulment omen when every unwanted one is on one side. A wanted
/// modifier on that side can still be the one removed.
/// Slams to the target, annulling when the item is full. A missing family
/// whose side is full can only arrive once something there goes, and there
/// are two ways to get there. With `clear`, the blocker is cleared at once
/// (the spec's "Annul/Chaos to clear a blocker"): an Annulment, with a side
/// omen when the other side holds modifiers. Without it, slams fill the
/// other side first and the Annulment comes when the item is full. Which is
/// cheaper depends on prices (an Annulment and an omen against a few more
/// Exalted Orbs), so a plan costs both and keeps the cheaper.
fn finish(state: &ItemState, target: &Target, grade: Grade, clear: bool) -> Next {
    let junk = junk(state, target);
    let blocked = target.missing(state).into_iter().map(|w| w.kind).find(|k| {
        clear && state.open(*k) == 0 && junk.iter().any(|m| m.kind == *k)
    });
    if let Some(side) = blocked {
        let others_there = state.mods.iter().any(|m| m.kind != side && m.source != Source::Fractured);
        let annul = Action::orb(Orb::Annulment);
        return Next::Act(if others_there { annul.with(side_omen_annul(side)) } else { annul });
    }
    if has_room(state) {
        return Next::Act(slam(state, target, grade));
    }
    if junk.is_empty() {
        return Next::Stop("every modifier the item can lose is wanted, and the target needs one more".into());
    }
    let prefix = junk.iter().any(|m| m.kind == AffixKind::Prefix);
    let suffix = junk.iter().any(|m| m.kind == AffixKind::Suffix);
    let annul = Action::orb(Orb::Annulment);
    Next::Act(match (prefix, suffix) {
        (true, false) => annul.with(side_omen_annul(AffixKind::Prefix)),
        (false, true) => annul.with(side_omen_annul(AffixKind::Suffix)),
        _ => annul,
    })
}

/// Chaos until the target holds. The removal is steered at the unwanted
/// modifiers: Erasure when they are all on one side and a wanted one sits
/// on the other; Whittling when the lowest-level modifier is the only one
/// of its level and unwanted. With nothing unwanted to remove and room
/// left, an Exalted Orb fills the room instead. A last unwanted modifier
/// that holds a missing family's group is annulled first when the other
/// side has room a Chaos Orb's new modifier could land in.
fn chaos_spam(state: &ItemState, target: &Target, grade: Grade) -> Next {
    if state.rarity != Rarity::Rare {
        return Next::Stop("chaos-spam works on a rare item".into());
    }
    let junk = junk(state, target);
    let missing = target.missing(state);
    if junk.is_empty() {
        return if has_room(state) {
            Next::Act(slam(state, target, Grade::Normal))
        } else {
            Next::Stop("every modifier the item can lose is wanted, and the target needs one more".into())
        };
    }
    let removable: Vec<&ModOn> = state.mods.iter().filter(|m| m.source != Source::Fractured).collect();
    let wanted_removable: Vec<&ModOn> = removable.iter().copied().filter(|m| target.wanted(m)).collect();

    if let [last] = junk.as_slice() {
        let blocker = missing.iter().any(|w| w.family == last.family);
        if blocker && state.open(last.kind.other()) > 0 {
            let annul = Action::orb(Orb::Annulment);
            let others_there = removable.iter().any(|m| m.kind != last.kind);
            return Next::Act(if others_there { annul.with(side_omen_annul(last.kind)) } else { annul });
        }
    }

    let chaos = Action::graded(Orb::Chaos, grade);
    let junk_side = match (junk.iter().all(|m| m.kind == AffixKind::Prefix), junk.iter().all(|m| m.kind == AffixKind::Suffix)) {
        (true, _) => Some(AffixKind::Prefix),
        (_, true) => Some(AffixKind::Suffix),
        _ => None,
    };
    let lowest_is_junk = || {
        let levels: Option<Vec<u32>> = removable.iter().map(|m| m.required_level).collect();
        let levels = levels?;
        let min = *levels.iter().min()?;
        let at: Vec<usize> = levels.iter().enumerate().filter(|(_, l)| **l == min).map(|(i, _)| i).collect();
        match at.as_slice() {
            [only] if !target.wanted(removable[*only]) => Some(()),
            _ => None,
        }
    };
    let (action, removes): (Action, Vec<&ModOn>) = match junk_side {
        Some(side) if wanted_removable.iter().any(|m| m.kind != side) => {
            (chaos.with(side_omen_erasure(side)), removable.iter().copied().filter(|m| m.kind == side).collect())
        }
        _ if !wanted_removable.is_empty() && lowest_is_junk().is_some() => (chaos.with(Omen::Whittling), Vec::new()),
        _ => (chaos, removable.clone()),
    };

    // A geometric trial: the removal can only take the one unwanted
    // modifier, the new one can only land on its side, and one wanted roll
    // there completes the target. Each miss leaves an equivalent item.
    let geometric = match (junk.as_slice(), removes.as_slice(), missing.as_slice()) {
        ([j], [r], [w]) => std::ptr::eq(*j, *r) && state.open(j.kind.other()) == 0 && w.kind == j.kind,
        _ => false,
    };
    if geometric {
        Next::Retry(action)
    } else {
        Next::Act(action)
    }
}

// ------------------------------------------------------------------ applicability

/// The item with its modifiers taken off, at `rarity`: what "can this
/// family roll at all" is asked of.
fn cleared(state: &ItemState, rarity: Rarity) -> ItemState {
    let mut s = state.clone();
    s.mods.clear();
    s.rarity = rarity;
    s
}

/// Whether random rolls can ever add `want` on this item.
fn rollable(state: &ItemState, want: &Want, pool: &dyn PoolView) -> bool {
    pool.eligible(&cleared(state, Rarity::Rare), want.kind, 0).iter().any(|c| want.accepts(c))
}

/// Whether only a desecration can add `want`: the desecrated pool has an
/// acceptable entry and the random pool has no entry of its family.
fn desecrated_only(state: &ItemState, want: &Want, pool: &dyn PoolView) -> bool {
    let base = cleared(state, Rarity::Rare);
    pool.desecrated(&base, Some(want.kind), None, 0).iter().any(|c| want.accepts(c))
        && !pool.eligible(&base, want.kind, 0).iter().any(|c| c.family == want.family)
}

/// Whether every missing want except `except` can come from random rolls.
fn rest_rollable(state: &ItemState, target: &Target, pool: &dyn PoolView, except: Option<&Want>) -> bool {
    target.missing(state).into_iter().filter(|w| except != Some(*w)).all(|w| rollable(state, w, pool))
}

/// The entry an essence adds on this item, when the essence data names one.
fn essence_entry(action: &Action, state: &ItemState, pool: &dyn PoolView) -> Option<Candidate> {
    match pool.essence(&action.essence_name()?, state) {
        EssenceOutcome::Entry(c) => Some(c),
        EssenceOutcome::Unknown(_) => None,
    }
}

/// The entry an Alloy adds on this item, when the Alloy table names one.
fn alloy_entry(action: &Action, state: &ItemState, pool: &dyn PoolView) -> Option<Candidate> {
    let Action::Alloy { name } = action else { return None };
    match pool.alloy(name, state) {
        EssenceOutcome::Entry(c) => Some(c),
        EssenceOutcome::Unknown(_) => None,
    }
}

/// The Alloys whose modifier meets a missing want on this item.
fn alloys_for(state: &ItemState, target: &Target, pool: &dyn PoolView) -> Vec<(Action, Candidate, Want)> {
    let mut out = Vec::new();
    for want in target.missing(state) {
        for name in ALLOYS {
            let action = Action::alloy(name);
            if let Some(c) = alloy_entry(&action, state, pool) {
                if want.accepts(&c) {
                    out.push((action, c, want.clone()));
                }
            }
        }
    }
    out
}

/// Whether the Alloy's modifier (of `kind`) always has room once the Alloy
/// has removed one: its side has room, or every modifier it can remove is
/// on that side, or an unwanted one there can be annulled first.
fn alloy_fits(state: &ItemState, target: &Target, kind: AffixKind) -> bool {
    state.open(kind) > 0
        || !state.mods.iter().any(|m| m.kind != kind && m.source != Source::Fractured)
        || junk(state, target).iter().any(|m| m.kind == kind)
}

/// For each missing want, the cheapest-tier upgrading essence (Lesser,
/// then normal, then Greater) whose modifier meets it.
fn upgrading_essences(state: &ItemState, target: &Target, pool: &dyn PoolView) -> Vec<(Action, Candidate, Want)> {
    let mut out = Vec::new();
    for want in target.missing(state) {
        'found: for tier in [EssenceTier::Lesser, EssenceTier::Normal, EssenceTier::Greater] {
            for of in TIERED_ESSENCES {
                let action = Action::essence(of, tier);
                if let Some(c) = essence_entry(&action, state, pool) {
                    if want.accepts(&c) {
                        out.push((action, c, want.clone()));
                        break 'found;
                    }
                }
            }
        }
    }
    out
}

/// Perfect and corrupted essences whose modifier meets a missing want.
fn replacing_essences(state: &ItemState, target: &Target, pool: &dyn PoolView) -> Vec<(Action, Candidate, Want)> {
    let mut out = Vec::new();
    for want in target.missing(state) {
        let perfect = TIERED_ESSENCES.iter().map(|of| Action::essence(of, EssenceTier::Perfect));
        let corrupted = CORRUPTED_ESSENCES.iter().map(|of| Action::essence(of, EssenceTier::Corrupted));
        for action in perfect.chain(corrupted) {
            if let Some(c) = essence_entry(&action, state, pool) {
                if want.accepts(&c) {
                    out.push((action, c, want.clone()));
                }
            }
        }
    }
    out
}

/// The one desecrated-only want, when there is exactly one (an item holds
/// one desecrated modifier).
fn desecrated_want<'a>(state: &ItemState, target: &'a Target, pool: &dyn PoolView) -> Option<&'a Want> {
    let found: Vec<&Want> = target.missing(state).into_iter().filter(|w| desecrated_only(state, w, pool)).collect();
    match found.as_slice() {
        [one] => Some(*one),
        _ => None,
    }
}

/// Whether the rules table accepts `action` on `state` (preconditions
/// only; nothing is drawn).
fn allowed(action: &Action, state: &ItemState) -> bool {
    match rules::rule(action) {
        Rule::Live(spec) => (spec.preconditions)(action, state).is_ok(),
        _ => false,
    }
}

/// The bones that take this item as a rare, with the omens the desecrate
/// strategy uses for `want`: Necromancy for its side, the lich omen when
/// every acceptable entry is of one lich and the omen acts on this item,
/// and Abyssal Echoes for the reroll.
fn bones_for(state: &ItemState, want: &Want, pool: &dyn PoolView, model: &Model) -> Vec<Action> {
    let rare = cleared(state, Rarity::Rare);
    let accepting: Vec<Candidate> =
        pool.desecrated(&rare, Some(want.kind), None, 0).into_iter().filter(|c| want.accepts(c)).collect();
    let liches: Vec<_> = accepting.iter().map(|c| c.desecrated).collect();
    let lich_omen = match liches.first() {
        Some(Some(l)) if liches.iter().all(|x| x == &Some(*l)) => Some(match l {
            super::types::Lich::Kurgal => Omen::Blackblooded,
            super::types::Lich::Amanamu => Omen::Liege,
            super::types::Lich::Ulaman => Omen::Sovereign,
        }),
        _ => None,
    };
    let necromancy = match want.kind {
        AffixKind::Prefix => Omen::SinistralNecromancy,
        AffixKind::Suffix => Omen::DextralNecromancy,
    };
    let mut out = Vec::new();
    for kind in BoneKind::ALL {
        let plain = Action::bone(kind, BoneGrade::Preserved);
        if !allowed(&plain, &rare) {
            continue;
        }
        let mut grades = vec![BoneGrade::Preserved];
        if state.item_level <= 64 {
            grades.push(BoneGrade::Gnawed);
        }
        if floor_helps(&Action::bone(kind, BoneGrade::Ancient), &plain, &rare, &Target { wants: vec![want.clone()] }, pool, model) {
            grades.push(BoneGrade::Ancient);
        }
        for grade in grades {
            let mut bone = Action::bone(kind, grade).with(necromancy);
            if let Some(omen) = lich_omen {
                let with_lich = bone.clone().with(omen);
                if allowed(&with_lich, &rare) {
                    bone = with_lich;
                }
            }
            let bone = bone.with(Omen::AbyssalEchoes);
            if allowed(&bone, &rare) {
                out.push(bone);
            }
        }
    }
    out
}

/// The entries one use of `action` on `state` can add, split into those
/// the target wants and the rest; `None` when the outcomes cannot be
/// walked.
fn added_entries(action: &Action, state: &ItemState, target: &Target, pool: &dyn PoolView, model: &Model) -> Option<(Vec<String>, Vec<String>)> {
    let all = sim::outcomes(action, state, pool, model).ok()?;
    let (mut wanted, mut other) = (Vec::new(), Vec::new());
    let mut note = |s: &ItemState| {
        for m in s.mods.iter().filter(|m| !state.mods.contains(m)) {
            let id = m.entry_id.clone().unwrap_or_else(|| m.text.clone());
            let list = if target.wanted(m) { &mut wanted } else { &mut other };
            if !list.contains(&id) {
                list.push(id);
            }
        }
    };
    for (p, o) in &all {
        if *p <= 0.0 {
            continue;
        }
        match o {
            Outcome::Applied(s) => note(s),
            Outcome::Reveal(options) => options.iter().for_each(&mut note),
            Outcome::Refused(_) => {}
        }
    }
    wanted.sort_unstable();
    other.sort_unstable();
    Some((wanted, other))
}

/// Whether `graded`'s Minimum Modifier Level helps over `plain` on
/// `state`: it removes tiers the target does not want and keeps every
/// wanted one (a family's fallback tier included).
fn floor_helps(graded: &Action, plain: &Action, state: &ItemState, target: &Target, pool: &dyn PoolView, model: &Model) -> bool {
    match (added_entries(graded, state, target, pool, model), added_entries(plain, state, target, pool, model)) {
        (Some((g_wanted, g_other)), Some((p_wanted, p_other))) => {
            !g_wanted.is_empty() && g_wanted == p_wanted && g_other.len() < p_other.len()
        }
        _ => false,
    }
}

/// The grades of `orb` worth trying on `state`: Normal, and Greater or
/// Perfect where the floor helps.
fn grades(orb: Orb, state: &ItemState, target: &Target, pool: &dyn PoolView, model: &Model) -> Vec<Grade> {
    let mut out = vec![Grade::Normal];
    let plain = Action::orb(orb);
    for grade in [Grade::Greater, Grade::Perfect] {
        if floor_helps(&Action::graded(orb, grade), &plain, state, target, pool, model) {
            out.push(grade);
        }
    }
    out
}

/// The rare-phase grades: an Exalted Orb on the item cleared to a rare. A
/// Regal and a Chaos Orb roll from the same floor as an Exalted Orb.
fn rare_grades(state: &ItemState, target: &Target, pool: &dyn PoolView, model: &Model) -> Vec<Grade> {
    grades(Orb::Exalted, &cleared(state, Rarity::Rare), target, pool, model)
}

/// Whether strategy `id` applies to `state` and `target`, per the table in
/// the module docs.
pub fn applicable(id: StrategyId, state: &ItemState, target: &Target, pool: &dyn PoolView) -> bool {
    if id == StrategyId::Buy {
        return true;
    }
    if locked(state).is_some() || state.rarity == Rarity::Unique {
        return false;
    }
    let rare = state.rarity == Rarity::Rare;
    match id {
        StrategyId::OrbChain => rest_rollable(state, target, pool, None),
        StrategyId::EssenceStart => {
            matches!(state.rarity, Rarity::Normal | Rarity::Magic)
                && upgrading_essences(state, target, pool).iter().any(|(_, _, w)| rest_rollable(state, target, pool, Some(w)))
        }
        StrategyId::PerfectEssenceFix => {
            let bad: Vec<&ModOn> = state.mods.iter().filter(|m| !target.wanted(m)).collect();
            rare && !state.crafted_slot_used()
                && matches!(bad.as_slice(), [one] if one.source != Source::Fractured)
                && replacing_essences(state, target, pool)
                    .iter()
                    .any(|(_, c, w)| (c.kind == bad[0].kind || state.open(c.kind) > 0) && rest_rollable(state, target, pool, Some(w)))
        }
        StrategyId::AlloyFix => {
            rare && !state.crafted_slot_used()
                && state.mods.iter().any(|m| m.source != Source::Fractured)
                && alloys_for(state, target, pool)
                    .iter()
                    .any(|(_, c, w)| alloy_fits(state, target, c.kind) && rest_rollable(state, target, pool, Some(w)))
        }
        StrategyId::ChaosSpam => {
            rare && state.mods.iter().filter(|m| target.wanted(m)).count() >= 3 && rest_rollable(state, target, pool, None)
        }
        StrategyId::Desecrate => match desecrated_want(state, target, pool) {
            // An unwanted desecrated modifier in the one slot is no bar: Omen
            // of Light clears it first.
            Some(w) => rest_rollable(state, target, pool, Some(w)) && {
                let rare_copy = cleared(state, Rarity::Rare);
                BoneKind::ALL.iter().any(|k| allowed(&Action::bone(*k, BoneGrade::Preserved), &rare_copy))
            },
            None => false,
        },
        StrategyId::FractureThenChaos => {
            let wanted: Vec<&ModOn> = state.mods.iter().filter(|m| target.wanted(m)).collect();
            rare && !state.fractured()
                && state.mods.len() >= 4
                && matches!(wanted.as_slice(), [one] if one.tier == Some(1))
                && rest_rollable(state, target, pool, None)
        }
        StrategyId::AlchemyStart => {
            state.rarity == Rarity::Normal && target.wants.len() >= 3 && rest_rollable(state, target, pool, None)
        }
        StrategyId::Buy => true,
    }
}

/// Every applicable strategy with each variant worth trying, in library
/// order, ending with buy.
pub fn library(state: &ItemState, target: &Target, pool: &dyn PoolView, model: &Model) -> Vec<Strategy> {
    let mut out = Vec::new();
    for id in StrategyId::ALL {
        if !applicable(id, state, target, pool) {
            continue;
        }
        match id {
            StrategyId::OrbChain => {
                let magic = match state.rarity {
                    Rarity::Normal => grades(Orb::Transmutation, &cleared(state, Rarity::Normal), target, pool, model),
                    Rarity::Magic if has_room(state) => grades(Orb::Augmentation, state, target, pool, model),
                    _ => vec![Grade::Normal],
                };
                // Each grade is tried on its own steps with the other steps
                // plain: the magic steps are one Transmutation and one
                // Augmentation, too few to change which rare grade wins.
                for r in rare_grades(state, target, pool, model) {
                    out.push(Strategy::OrbChain { magic: Grade::Normal, rare: r, clear: false });
                }
                for m in magic.into_iter().filter(|m| *m != Grade::Normal) {
                    out.push(Strategy::OrbChain { magic: m, rare: Grade::Normal, clear: false });
                }
            }
            StrategyId::EssenceStart => {
                let rare = rare_grades(state, target, pool, model);
                for (essence, entry, want) in upgrading_essences(state, target, pool) {
                    if !rest_rollable(state, target, pool, Some(&want)) {
                        continue;
                    }
                    for r in &rare {
                        out.push(Strategy::EssenceStart { essence: essence.clone(), groups: entry.groups.clone(), rare: *r, clear: false });
                    }
                }
            }
            StrategyId::PerfectEssenceFix => {
                let Some(bad) = state.mods.iter().find(|m| !target.wanted(m)) else { continue };
                for (essence, entry, want) in replacing_essences(state, target, pool) {
                    if (entry.kind == bad.kind || state.open(entry.kind) > 0) && rest_rollable(state, target, pool, Some(&want)) {
                        out.push(Strategy::PerfectEssenceFix { essence: essence.with(side_omen_crystallisation(bad.kind)) });
                    }
                }
            }
            StrategyId::AlloyFix => {
                for (alloy, entry, want) in alloys_for(state, target, pool) {
                    if alloy_fits(state, target, entry.kind) && rest_rollable(state, target, pool, Some(&want)) {
                        out.push(Strategy::AlloyFix { alloy, kind: entry.kind });
                    }
                }
            }
            StrategyId::ChaosSpam | StrategyId::FractureThenChaos => {
                for g in rare_grades(state, target, pool, model) {
                    out.push(if id == StrategyId::ChaosSpam {
                        Strategy::ChaosSpam { grade: g }
                    } else {
                        Strategy::FractureThenChaos { grade: g }
                    });
                }
            }
            StrategyId::Desecrate => {
                if let Some(want) = desecrated_want(state, target, pool) {
                    for bone in bones_for(state, want, pool, model) {
                        out.push(Strategy::Desecrate { bone });
                    }
                }
            }
            StrategyId::AlchemyStart => {
                for r in rare_grades(state, target, pool, model) {
                    out.push(Strategy::AlchemyStart { rare: r, clear: false });
                }
            }
            StrategyId::Buy => out.push(Strategy::Buy),
        }
    }
    out
}
