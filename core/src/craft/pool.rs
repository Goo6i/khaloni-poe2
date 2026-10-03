//! The eligible pool of an item and its tier numbering.
//!
//! An entry can roll on an item when all of these hold: it is a prefix or
//! suffix of the wanted kind in the item domain, not essence-only; the
//! first tag in its own spawn-weight list that the item carries (the base's
//! tags plus the tags the item's mods add) has weight 1; no mod on the item
//! shares one of its groups; and `floor <= required_level <= item level`,
//! where the floor is the Minimum Modifier Level of a Greater or Perfect
//! orb (0 for the plain ones). The desecrated pool follows the same rules
//! over the desecrated domain, narrowed to one lich when a bone or omen
//! names it.
//!
//! Each entry is one tier of one family, so the pool is a list of (family,
//! tier) entries; how likely each is lies outside the client's data.

use super::data::{Base, CraftData, Domain, Entry};
use super::types::{AffixKind, Candidate, EssenceOutcome, ItemState, Lich, PoolView};

/// Whether `entry` passes the group and level rules on `state`.
fn open_on(entry: &Entry, state: &ItemState, floor: u32) -> bool {
    let level = entry.required_level;
    floor <= level && level <= state.item_level && !entry.groups.iter().any(|g| state.groups().any(|on| on == g))
}

/// The item's tags as one list: the base's, then those its mods add.
fn item_tags(state: &ItemState) -> Vec<&str> {
    state.tags().collect()
}

/// Entries of `kind` that can roll on `state` (see the module docs), as
/// candidates numbered on the item's base.
pub fn pool(data: &CraftData, state: &ItemState, kind: AffixKind, floor: u32) -> Vec<Candidate> {
    let tags = item_tags(state);
    let base_tags: Vec<&str> = state.base_tags.iter().map(String::as_str).collect();
    data.entries()
        .iter()
        .filter(|e| e.domain == Domain::Item && e.kind == kind && !e.essence_only)
        .filter(|e| open_on(e, state, floor) && e.eligible(tags.iter().copied()))
        .map(|e| data.candidate(e, &base_tags))
        .collect()
}

/// Desecrated entries for `state`, narrowed to one kind and one lich when
/// given, with the pool's tag, group and level rules.
pub fn desecrated_pool(
    data: &CraftData,
    state: &ItemState,
    kind: Option<AffixKind>,
    lich: Option<Lich>,
    floor: u32,
) -> Vec<Candidate> {
    let tags = item_tags(state);
    let base_tags: Vec<&str> = state.base_tags.iter().map(String::as_str).collect();
    data.entries()
        .iter()
        .filter(|e| e.domain == Domain::Desecrated && kind.is_none_or(|k| e.kind == k) && !e.essence_only)
        .filter(|e| lich.is_none_or(|l| e.lich == Some(l)))
        .filter(|e| open_on(e, state, floor) && e.eligible(tags.iter().copied()))
        .map(|e| data.candidate(e, &base_tags))
        .collect()
}

/// Trade-site numbering of `entry_id` on `base`: 1 is the best tier of its
/// family there, and each distinct required level above it on that base
/// adds one. 0 when the id is not in the data.
pub fn tier_of(data: &CraftData, entry_id: &str, base: &Base) -> u8 {
    let tags: Vec<&str> = base.tags.iter().map(String::as_str).collect();
    data.entry(entry_id).map_or(0, |e| data.tier_on(e, &tags))
}

impl PoolView for CraftData {
    fn eligible(&self, state: &ItemState, kind: AffixKind, floor: u32) -> Vec<Candidate> {
        pool(self, state, kind, floor)
    }

    fn desecrated(&self, state: &ItemState, kind: Option<AffixKind>, lich: Option<Lich>, floor: u32) -> Vec<Candidate> {
        desecrated_pool(self, state, kind, lich, floor)
    }

    fn essence(&self, essence: &str, state: &ItemState) -> EssenceOutcome {
        let base = Base { name: state.base.clone(), class: state.class.clone(), tags: state.base_tags.clone() };
        self.essence_outcome(essence, &base)
    }

    fn alloy(&self, alloy: &str, state: &ItemState) -> EssenceOutcome {
        let base = Base { name: state.base.clone(), class: state.class.clone(), tags: state.base_tags.clone() };
        self.alloy_outcome(alloy, &base)
    }
}
