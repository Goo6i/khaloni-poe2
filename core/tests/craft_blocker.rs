//! Orb-chain clears a blocker the way the spec says ("Annul/Chaos to clear
//! a blocker"): on its clearing route, when a missing family's side is full
//! and holds an unwanted modifier, the next step annuls on that side instead
//! of slamming the other side full first. Which route a plan takes is its
//! cheaper one at the plan's prices.

use std::path::{Path, PathBuf};

use khaloni_poe2_core::craft::data::{CraftData, Domain};
use khaloni_poe2_core::craft::rules::{Action, Grade, Omen, Orb};
use khaloni_poe2_core::craft::strategy::{Next, Strategy, Target, Want};
use khaloni_poe2_core::craft::types::{AffixKind, ItemState, ModOn, Rarity, Source};

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn data() -> CraftData {
    let dir: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    CraftData::load(
        &read(&dir.join("craft/mods_slice.json")),
        &read(&dir.join("craft_base_items_sample.json")),
        &read(&dir.join("craft/essences_slice.json")),
    )
    .expect("the craft fixtures load")
}

/// The best tier of `family` a Soldier Cuirass rolls, as a modifier.
fn best(data: &CraftData, tags: &[&str], family: &str) -> ModOn {
    let e = data
        .entries()
        .iter()
        .filter(|e| e.domain == Domain::Item && e.family == family && e.eligible(tags.iter().copied()))
        .max_by_key(|e| e.required_level)
        .unwrap_or_else(|| panic!("{family} rolls on the cuirass"));
    data.candidate(e, tags).to_mod(Source::Random)
}

fn cuirass(data: &CraftData, mods: Vec<ModOn>) -> ItemState {
    let base = data.base("Soldier Cuirass").unwrap();
    ItemState {
        class: base.class.clone(),
        base: base.name.clone(),
        base_tags: base.tags.clone(),
        item_level: 82,
        rarity: Rarity::Rare,
        corrupted: false,
        mirrored: false,
        sanctified: false,
        mods,
        sockets: 0,
    }
}

#[test]
fn orb_chain_annuls_a_blocker_on_the_wanted_side_before_slamming() {
    let data = data();
    let base = data.base("Soldier Cuirass").unwrap();
    let tags: Vec<&str> = base.tags.iter().map(String::as_str).collect();
    let chain = Strategy::OrbChain { magic: Grade::Normal, rare: Grade::Normal, clear: true };
    let fill = Strategy::OrbChain { magic: Grade::Normal, rare: Grade::Normal, clear: false };
    let life = Target { wants: vec![Want { family: "IncreasedLife".into(), kind: AffixKind::Prefix, min_tier: 3 }] };

    // Three unwanted prefixes and one suffix: the wanted prefix has no room,
    // the suffix side has two slots. The step clears a prefix, restricted
    // to prefixes because a suffix is there too.
    let full_prefixes = vec![
        best(&data, &tags, "LocalPhysicalDamageReductionRating"),
        best(&data, &tags, "LocalPhysicalDamageReductionRatingPercent"),
        best(&data, &tags, "BaseSpirit"),
        best(&data, &tags, "FireResistance"),
    ];
    let item = cuirass(&data, full_prefixes);
    assert_eq!((item.open(AffixKind::Prefix), item.open(AffixKind::Suffix)), (0, 2));
    match chain.next(&item, &life) {
        Next::Act(a) => assert_eq!(a, Action::orb(Orb::Annulment).with(Omen::SinistralAnnulment), "{}", a.name()),
        other => panic!("expected a prefix annulment, got {other:?}"),
    }
    // The other route fills the suffix side first; a plan costs both and
    // keeps the cheaper at its prices.
    assert!(matches!(fill.next(&item, &life), Next::Act(Action::Orb { orb: Orb::Exalted, .. })));
    assert_eq!(chain.other_blocker_route(), Some(fill.clone()));
    assert!(chain.name().ends_with("clearing blockers first)"), "{}", chain.name());
    assert_eq!(fill.name(), "orb-chain");

    // A worse tier of the wanted family itself is the blocker: same step.
    let worst_life = data
        .entries()
        .iter()
        .filter(|e| e.domain == Domain::Item && e.family == "IncreasedLife" && e.eligible(tags.iter().copied()))
        .min_by_key(|e| e.required_level)
        .unwrap();
    let low = data.candidate(worst_life, &tags).to_mod(Source::Random);
    let item = cuirass(
        &data,
        vec![low, best(&data, &tags, "LocalPhysicalDamageReductionRating"), best(&data, &tags, "BaseSpirit"), best(&data, &tags, "FireResistance")],
    );
    assert!(matches!(chain.next(&item, &life), Next::Act(a) if a == Action::orb(Orb::Annulment).with(Omen::SinistralAnnulment)));

    // Room on the wanted side: a slam, as before.
    let item = cuirass(&data, vec![best(&data, &tags, "BaseSpirit"), best(&data, &tags, "FireResistance")]);
    assert!(matches!(chain.next(&item, &life), Next::Act(Action::Orb { orb: Orb::Exalted, .. })));

    // Only prefixes on the item: the plain orb already hits a prefix, so no
    // omen is paid for.
    let item = cuirass(
        &data,
        vec![best(&data, &tags, "LocalPhysicalDamageReductionRating"), best(&data, &tags, "LocalPhysicalDamageReductionRatingPercent"), best(&data, &tags, "BaseSpirit")],
    );
    assert!(matches!(chain.next(&item, &life), Next::Act(a) if a == Action::orb(Orb::Annulment)));
}
