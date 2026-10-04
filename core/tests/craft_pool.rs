//! The craft planner's data layer on real data: the eligible pool follows
//! the first-matching-tag rule, groups and the level window; tiers number
//! like the trade site; an item state reads from a copied item; essences
//! join their entries by text. The fixtures under `fixtures/craft/` are
//! slices of the cached mod database and essence texts (item and
//! desecrated prefixes and suffixes, the fields the planner reads), so no
//! test reads the user's cache.

use khaloni_poe2_core::craft::data::CraftData;
use khaloni_poe2_core::craft::pool::{desecrated_pool, pool, tier_of};
use khaloni_poe2_core::craft::types::{
    AffixKind, Candidate, EssenceOutcome, ItemState, Lich, ModOn, PoolView, Rarity, Source,
};
use khaloni_poe2_core::ee2::{self, data, Ee2Data};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn craft() -> &'static CraftData {
    static DATA: OnceLock<CraftData> = OnceLock::new();
    DATA.get_or_init(|| {
        let dir = fixtures();
        CraftData::load(
            &read(&dir.join("craft/mods_slice.json")),
            &read(&dir.join("craft_base_items_sample.json")),
            &read(&dir.join("craft/essences_slice.json")),
        )
        .expect("the craft fixtures load")
    })
}

fn ee2_data() -> &'static Ee2Data {
    static DATA: OnceLock<Ee2Data> = OnceLock::new();
    DATA.get_or_init(|| {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tools/ee2-parity/data");
        let mut db = Ee2Data::from_ndjson(&read(&dir.join("stats.ndjson")), &read(&dir.join("items.ndjson")))
            .expect("pinned ndjson parses");
        db.trade_stats = Some(data::TradeStatTexts::from_json(&read(&dir.join("trade-stats.json"))).expect("trade stats"));
        db.trade_items = Some(data::trade_item_names(&read(&dir.join("trade-items.json"))).expect("trade items"));
        db
    })
}

fn state_of(path: &Path) -> Result<ItemState, String> {
    let built = ee2::request::build(&read(path), ee2_data()).unwrap_or_else(|e| panic!("{}: {e:?}", path.display()));
    ItemState::from_built(&built, craft())
}

fn corpus_item(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../tools/ee2-parity/items").join(name)
}

/// A normal item of `base` at `ilvl` carrying the given modifiers.
fn normal(base: &str, ilvl: u32, mods: Vec<ModOn>) -> ItemState {
    let b = craft().base(base).unwrap_or_else(|| panic!("{base} is in the base sample"));
    ItemState {
        class: b.class.clone(),
        base: b.name.clone(),
        base_tags: b.tags.clone(),
        item_level: ilvl,
        rarity: Rarity::Normal,
        corrupted: false,
        mirrored: false,
        sanctified: false,
        mods,
        sockets: 0,
    }
}

/// The modifier the entry `id` puts on an item of `base`.
fn on(id: &str, base: &str) -> ModOn {
    let data = craft();
    let entry = data.entry(id).unwrap_or_else(|| panic!("{id} is in the mod slice"));
    let tags: Vec<&str> = data.base(base).unwrap().tags.iter().map(String::as_str).collect();
    data.candidate(entry, &tags).to_mod(Source::Random)
}

fn ids(pool: &[Candidate]) -> Vec<&str> {
    pool.iter().map(|c| c.entry_id.as_str()).collect()
}

fn has(pool: &[Candidate], id: &str) -> bool {
    pool.iter().any(|c| c.entry_id == id)
}

#[test]
fn the_first_matching_tag_in_the_entrys_own_order_decides() {
    let data = craft();
    // LocalIncreasedPhysicalDamageReductionRating8 lists gloves: 0 and
    // boots: 0 before str_armour: 1. Vaal Mitts carry str_armour first in
    // their own tag list, but the entry's order decides: gloves comes first
    // there, so the tier cannot roll on them, while a Soldier Cuirass (no
    // gloves or boots tag) meets str_armour and rolls it.
    let entry = data.entry("LocalIncreasedPhysicalDamageReductionRating8").unwrap();
    assert_eq!(entry.spawn_weights[0], ("gloves".to_string(), 0));
    let mitts = data.base("Vaal Mitts").unwrap();
    assert_eq!(mitts.tags[0], "str_armour", "the base lists str_armour before gloves");

    let gloves = pool(data, &normal("Vaal Mitts", 82, vec![]), AffixKind::Prefix, 0);
    let cuirass = pool(data, &normal("Soldier Cuirass", 82, vec![]), AffixKind::Prefix, 0);
    assert!(!has(&gloves, "LocalIncreasedPhysicalDamageReductionRating8"), "gloves: 0 comes first and blocks it");
    assert!(has(&cuirass, "LocalIncreasedPhysicalDamageReductionRating8"), "str_armour: 1 lets it roll on body armour");
    // The lower rungs of the same family carry no gloves weight, so they
    // still roll on the mitts.
    assert!(has(&gloves, "LocalIncreasedPhysicalDamageReductionRating7__"));
    // No entry of another kind or domain slips in, and essence-only entries
    // never do.
    for c in gloves.iter().chain(&cuirass) {
        let e = data.entry(&c.entry_id).unwrap();
        assert_eq!(e.kind, AffixKind::Prefix, "{}", e.id);
        assert!(!e.essence_only, "{}", e.id);
        assert!(c.desecrated.is_none(), "{}", e.id);
    }
    // A base with none of an entry's tags never gets it.
    let ring = pool(data, &normal("Gold Ring", 82, vec![]), AffixKind::Prefix, 0);
    assert!(!has(&ring, "LocalIncreasedPhysicalDamageReductionRating1"));
    assert!(has(&ring, "IncreasedLife6"));
}

#[test]
fn a_group_already_on_the_item_is_excluded() {
    let data = craft();
    let bare = pool(data, &normal("Soldier Cuirass", 82, vec![]), AffixKind::Prefix, 0);
    let life_tiers = ids(&bare).into_iter().filter(|id| id.starts_with("IncreasedLife")).count();
    assert_eq!(life_tiers, 13, "all thirteen life tiers roll on a level 82 cuirass");

    let with_life = normal("Soldier Cuirass", 82, vec![on("IncreasedLife3", "Soldier Cuirass")]);
    let after = pool(data, &with_life, AffixKind::Prefix, 0);
    assert!(
        after.iter().all(|c| !c.groups.iter().any(|g| g == "IncreasedLife")),
        "every entry of the IncreasedLife group leaves the pool"
    );
    assert!(has(&after, "LocalIncreasedPhysicalDamageReductionRating8"), "other groups stay");
    assert_eq!(after.len(), bare.len() - life_tiers);

    // A group blocks its rivals of the other side too: the pool is keyed by
    // group, not by kind.
    let with_cold = normal("Soldier Cuirass", 82, vec![on("ColdResist2", "Soldier Cuirass")]);
    let suffixes = pool(data, &with_cold, AffixKind::Suffix, 0);
    assert!(!ids(&suffixes).iter().any(|id| id.starts_with("ColdResist")));
    assert!(has(&suffixes, "FireResist8"));
}

#[test]
fn the_level_window_and_a_greater_orb_floor_bound_the_pool() {
    let data = craft();
    // Life tiers sit at levels 1, 6, 16, 24, 33, 38, 46, 54, 60, 65, 70, 75, 80.
    let life = |ilvl: u32, floor: u32| -> Vec<String> {
        let mut v: Vec<String> = pool(data, &normal("Soldier Cuirass", ilvl, vec![]), AffixKind::Prefix, floor)
            .into_iter()
            .filter(|c| c.family == "IncreasedLife")
            .map(|c| c.entry_id)
            .collect();
        v.sort_by_key(|id| data.entry(id).unwrap().required_level);
        v
    };
    assert_eq!(
        life(45, 0),
        ["IncreasedLife1", "IncreasedLife2", "IncreasedLife3", "IncreasedLife4", "IncreasedLife5", "IncreasedLife6"],
        "item level 45 stops below the level 46 tier"
    );
    assert_eq!(life(46, 0).last().map(String::as_str), Some("IncreasedLife7"), "the bound is inclusive");
    // A Greater Regal, Exalted or Chaos Orb rolls only entries of level 35
    // and above; a Perfect one 50 and above.
    assert_eq!(life(45, 35), ["IncreasedLife6"]);
    assert_eq!(life(82, 50), ["IncreasedLife8", "IncreasedLife9", "IncreasedLife10", "IncreasedLife11", "IncreasedLife12", "IncreasedLife13"]);
    assert_eq!(life(80, 80), ["IncreasedLife13"], "the floor is inclusive too");
    // A floor above the item level leaves nothing.
    assert!(pool(data, &normal("Soldier Cuirass", 40, vec![]), AffixKind::Prefix, 44).is_empty());
    for c in pool(data, &normal("Gold Ring", 82, vec![]), AffixKind::Suffix, 35) {
        assert!((35..=82).contains(&c.required_level), "{} at {}", c.entry_id, c.required_level);
    }
}

#[test]
fn an_added_tag_from_a_rolled_mod_blocks_its_rivals() {
    let data = craft();
    // An increased-chaos-damage prefix adds no_cold_spell_mods (and the
    // fire, lightning and physical ones). The cold spell suffixes list that
    // tag at weight 0 before wand: 1, so they leave the pool although they
    // share no group with the prefix; the chaos ones stay.
    let chaos = on("ChaosDamagePrefixOnWeapon4", "Attuned Wand");
    assert!(chaos.adds_tags.iter().any(|t| t == "no_cold_spell_mods"));
    let before = pool(data, &normal("Attuned Wand", 81, vec![]), AffixKind::Suffix, 0);
    let after = pool(data, &normal("Attuned Wand", 81, vec![chaos.clone()]), AffixKind::Suffix, 0);
    for blocked in ["FreezeDamageIncrease3", "GlobalColdSpellGemsLevelWeapon3", "IgniteChanceIncrease2", "ShockChanceIncrease4"] {
        assert!(has(&before, blocked), "{blocked} rolls on a bare wand");
        assert!(!has(&after, blocked), "{blocked} is blocked by the added tag");
        let e = data.entry(blocked).unwrap();
        assert!(!e.groups.iter().any(|g| chaos.groups.contains(g)), "{blocked} shares no group with the prefix");
    }
    assert!(has(&after, "GlobalChaosSpellGemsLevelWeapon3"), "the chaos spell suffix stays");
    // The prefix side: rival damage-type prefixes are blocked by both the
    // group and their own no_*_spell_mods weight.
    let prefixes = pool(data, &normal("Attuned Wand", 81, vec![chaos]), AffixKind::Prefix, 0);
    assert!(!has(&prefixes, "ColdDamagePrefixOnWeapon4"));
}

#[test]
fn tiers_number_from_the_top_of_the_familys_ladder_on_that_base() {
    let data = craft();
    let cuirass = data.base("Soldier Cuirass").unwrap();
    let mitts = data.base("Vaal Mitts").unwrap();
    // Thirteen life rungs on body armour: the level 80 tier is T1, the
    // level 1 tier T13.
    assert_eq!(tier_of(data, "IncreasedLife13", cuirass), 1);
    assert_eq!(tier_of(data, "IncreasedLife7", cuirass), 7);
    assert_eq!(tier_of(data, "IncreasedLife1", cuirass), 13);
    // Gloves stop at the level 60 tier, so the same entry numbers higher up
    // its ladder there.
    assert_eq!(tier_of(data, "IncreasedLife9", mitts), 1);
    assert_eq!(tier_of(data, "IncreasedLife7", mitts), 3);
    // The armour ladder on gloves loses its top four rungs to gloves: 0.
    assert_eq!(tier_of(data, "LocalIncreasedPhysicalDamageReductionRating8", cuirass), 4);
    assert_eq!(tier_of(data, "LocalIncreasedPhysicalDamageReductionRating7__", mitts), 1);
    assert_eq!(tier_of(data, "NoSuchEntry", cuirass), 0, "an id outside the data has no tier");

    // The pool's candidates carry the same numbering, and on each base a
    // family's tiers run 1..=n without a gap.
    let prefixes = pool(data, &normal("Soldier Cuirass", 82, vec![]), AffixKind::Prefix, 0);
    let life: Vec<&Candidate> = prefixes.iter().filter(|c| c.family == "IncreasedLife").collect();
    let mut tiers: Vec<u8> = life.iter().map(|c| c.tier).collect();
    tiers.sort_unstable();
    assert_eq!(tiers, (1..=13).collect::<Vec<u8>>());
    for c in &life {
        assert_eq!(c.tier, tier_of(data, &c.entry_id, cuirass));
    }
}

#[test]
fn the_state_of_a_rare_reads_its_mods_kinds_tiers_and_open_slots() {
    let data = craft();
    let state = state_of(&fixtures().join("craft/rare-soldier-cuirass.txt")).expect("the rare reads");
    assert_eq!(state.class, "Body Armour");
    assert_eq!(state.base, "Soldier Cuirass");
    assert_eq!(state.base_tags, data.base("Soldier Cuirass").unwrap().tags);
    assert_eq!(state.item_level, 82);
    assert_eq!(state.rarity, Rarity::Rare);
    assert!(!state.locked());

    let seen: Vec<(Option<&str>, AffixKind, Option<u8>, Source)> =
        state.mods.iter().map(|m| (m.entry_id.as_deref(), m.kind, m.tier, m.source)).collect();
    // The tooltip's tier numbers (8, 8, 6, 4, 6) are not trusted: each tier
    // is computed on the base, 1 = best.
    assert_eq!(
        seen,
        vec![
            (Some("IncreasedLife8"), AffixKind::Prefix, Some(6), Source::Random),
            (Some("LocalIncreasedPhysicalDamageReductionRating8"), AffixKind::Prefix, Some(4), Source::Random),
            (Some("ColdResist6"), AffixKind::Suffix, Some(3), Source::Random),
            (Some("FireResist4"), AffixKind::Suffix, Some(5), Source::Random),
            (Some("Strength6"), AffixKind::Suffix, Some(3), Source::Random),
        ]
    );
    let life = &state.mods[0];
    assert_eq!(life.family, "IncreasedLife");
    assert_eq!(life.groups, ["IncreasedLife"]);
    assert_eq!(life.required_level, Some(54));
    assert_eq!(life.text, "+104(100-119) to maximum Life");

    assert_eq!((state.count(AffixKind::Prefix), state.count(AffixKind::Suffix)), (2, 3));
    assert_eq!((state.open(AffixKind::Prefix), state.open(AffixKind::Suffix)), (1, 0));
    assert!(!state.crafted_slot_used() && !state.desecrated_slot_used() && !state.fractured());

    // The groups on the item keep their rivals out of what can roll next.
    let next = data.eligible(&state, AffixKind::Prefix, 0);
    assert!(next.iter().all(|c| c.family != "IncreasedLife" && !c.groups.iter().any(|g| g == "BaseLocalDefences")));
    assert!(!next.is_empty());

    // A magic item from the corpus: one of each side, both slots full.
    let magic = state_of(&corpus_item("new-magic-vest-two-mod.txt")).expect("the magic vest reads");
    assert_eq!(magic.rarity, Rarity::Magic);
    assert_eq!(magic.base, "Exquisite Vest");
    assert_eq!((magic.open(AffixKind::Prefix), magic.open(AffixKind::Suffix)), (0, 0));
    assert!(magic.mods.iter().all(|m| m.entry_id.is_some() && m.tier.is_some()));

    // A modifier the data has no entry for keeps its text and no entry.
    let text = read(&fixtures().join("craft/rare-soldier-cuirass.txt"))
        .replace("+26(25-27) to Strength", "+26(25-27) to Wombat Handling");
    let built = ee2::request::build(&text, ee2_data()).unwrap();
    let odd = ItemState::from_built(&built, data).unwrap();
    let last = odd.mods.last().unwrap();
    assert_eq!((last.entry_id.as_deref(), last.tier, last.kind), (None, None, AffixKind::Suffix));
    assert_eq!(last.family, "+26(25-27) to Wombat Handling");

    // A copy without the modifier headers cannot say which side a modifier
    // is on, and a base outside the data cannot be tagged: both refuse.
    let plain = state_of(&corpus_item("edge-plain-copy-ring.txt"));
    assert!(plain.is_err(), "{plain:?}");
    let spear = state_of(&corpus_item("new-spear-phys-crafted.txt")).unwrap_err();
    assert!(spear.contains("Spiked Spear"), "{spear}");
}

#[test]
fn a_crafted_and_a_desecrated_mod_take_their_single_slots() {
    let data = craft();
    let state = state_of(&fixtures().join("craft/rare-soldier-cuirass-crafted-desecrated.txt")).expect("the rare reads");
    let seen: Vec<(Option<&str>, AffixKind, Source)> =
        state.mods.iter().map(|m| (m.entry_id.as_deref(), m.kind, m.source)).collect();
    assert_eq!(
        seen,
        vec![
            (Some("IncreasedLife7"), AffixKind::Prefix, Source::Random),
            (Some("ColdResist6"), AffixKind::Suffix, Source::Crafted),
            (Some("AbyssModArmourJewelleryUlamanSuffixLightningChaosResistance"), AffixKind::Suffix, Source::Desecrated),
        ]
    );
    assert!(state.crafted_slot_used(), "the essence mod takes the one crafted slot");
    assert!(state.desecrated_slot_used(), "the lich mod takes the one desecrated slot");
    assert_eq!((state.open(AffixKind::Prefix), state.open(AffixKind::Suffix)), (2, 1));
    // The desecrated mod's tier is read on its own domain's ladder.
    assert_eq!(state.mods[2].tier, Some(1));

    // The desecrated pool on this base: armour-eligible lich entries, one
    // lich when named, none whose group is already on the item.
    let all = desecrated_pool(data, &state, Some(AffixKind::Suffix), None, 0);
    assert!(!all.is_empty());
    assert!(!has(&all, "AbyssModArmourJewelleryUlamanSuffixLightningChaosResistance"), "its group is on the item");
    let kurgal = data.desecrated(&state, Some(AffixKind::Suffix), Some(Lich::Kurgal), 0);
    assert!(!kurgal.is_empty());
    assert!(kurgal.iter().all(|c| c.desecrated == Some(Lich::Kurgal) && c.kind == AffixKind::Suffix));
    assert!(has(&kurgal, "AbyssModArmourJewelleryKurgalSuffixColdChaosResistance"));
    assert!(kurgal.len() < all.len());
    for c in &all {
        let e = data.entry(&c.entry_id).unwrap();
        assert!(e.eligible(state.base_tags.iter().map(String::as_str)), "{} rolls on body armour", e.id);
        assert!(c.required_level <= state.item_level);
    }

    // A real corpus bow: the fractured prefix reads as fractured, and the two
    // older essence suffixes, which carry no crafted marker, read as crafted
    // because their entries are ones no tag lets roll.
    let bow = state_of(&corpus_item("ee2-FracturedItem.txt")).expect("the bow reads");
    assert_eq!(bow.mods[0].source, Source::Fractured);
    assert_eq!(bow.mods[0].entry_id.as_deref(), Some("LocalAddedPhysicalDamage9"));
    assert!(bow.fractured());
    let crafted: Vec<&ModOn> = bow.mods.iter().filter(|m| m.source == Source::Crafted).collect();
    assert_eq!(crafted.len(), 2, "{:?}", bow.mods);
    assert!(crafted.iter().all(|m| !data.entry(m.entry_id.as_deref().unwrap()).unwrap().rollable_anywhere()));

    // An unrevealed desecrated modifier holds the slot before it is known.
    // Line endings unified first: a Windows checkout gives the fixture CRLF.
    let text = read(&fixtures().join("craft/rare-soldier-cuirass-crafted-desecrated.txt")).replace("\r\n", "\n").replace(
        "{ Desecrated Suffix Modifier \"of Ulaman\" (Tier: 1) — Elemental, Lightning, Chaos, Resistance }\n+15(13-17)% to Lightning and Chaos Resistances (desecrated)",
        "{ Desecrated Suffix Modifier }\nDesecrated Suffix",
    );
    let veiled = ItemState::from_built(&ee2::request::build(&text, ee2_data()).unwrap(), data).unwrap();
    let last = veiled.mods.last().unwrap();
    assert_eq!((last.source, last.entry_id.as_deref(), last.kind), (Source::Desecrated, None, AffixKind::Suffix));
    assert!(veiled.desecrated_slot_used());
}

#[test]
fn a_corrupted_or_mirrored_item_is_locked() {
    let corrupted = state_of(&corpus_item("new-body-ar-es-3socket-corrupted.txt")).expect("reads");
    assert!(corrupted.corrupted && corrupted.locked());
    assert_eq!(corrupted.sockets, 3);
    let mirrored = state_of(&corpus_item("new-amulet-allres-attributes-mirrored.txt")).expect("reads");
    assert!(mirrored.mirrored && mirrored.locked());
    let sanctified = state_of(&corpus_item("new-body-es-2socket-sanctified.txt")).expect("reads");
    assert!(sanctified.sanctified && sanctified.locked());
    let open = state_of(&fixtures().join("craft/rare-soldier-cuirass.txt")).unwrap();
    assert!(!open.corrupted && !open.mirrored && !open.sanctified && !open.locked());
}

#[test]
fn an_essence_names_its_entry_per_item_class_by_text() {
    let data = craft();
    let entry_of = |essence: &str, base: &str| -> String {
        match data.essence_outcome(essence, data.base(base).unwrap()) {
            EssenceOutcome::Entry(c) => c.entry_id,
            EssenceOutcome::Unknown(why) => panic!("{essence} on {base}: {why}"),
        }
    };
    // One essence, a different entry per class line.
    assert_eq!(entry_of("Lesser Essence of the Body", "Soldier Cuirass"), "IncreasedLife3");
    assert_eq!(entry_of("Lesser Essence of the Body", "Heavy Belt"), "IncreasedLife3");
    assert_eq!(entry_of("Lesser Essence of the Body", "Gold Ring"), "IncreasedLife2");
    assert_eq!(entry_of("Greater Essence of the Body", "Soldier Cuirass"), "IncreasedLife8");
    assert_eq!(entry_of("Greater Essence of the Body", "Vaal Mitts"), "IncreasedLife7");
    assert_eq!(entry_of("Greater_Essence_of_Thawing", "Gold Ring"), "ColdResist6");
    // Two entries read "+(61-84) to Accuracy Rating"; on a bow the local one
    // is the one that can roll.
    assert_eq!(entry_of("Lesser Essence of Battle", "Crude Bow"), "LocalIncreasedAccuracy3");
    // Four entries read "+3 to Level of all Spell Skills"; exactly one of them
    // is one no tag lets roll, so it is the essence's own.
    assert_eq!(entry_of("Perfect Essence of Sorcery", "Attuned Wand"), "EssenceSpellSkillLevel1H1");
    // The candidate carries the entry's side and its tier on the base.
    match data.essence_outcome("Greater Essence of the Body", data.base("Soldier Cuirass").unwrap()) {
        EssenceOutcome::Entry(c) => {
            assert_eq!((c.kind, c.tier, c.family.as_str()), (AffixKind::Prefix, 6, "IncreasedLife"));
        }
        other => panic!("{other:?}"),
    }
    // Through the pool view, from an item state.
    let state = normal("Gold Ring", 80, vec![]);
    assert!(matches!(data.essence("Essence of the Mind", &state), EssenceOutcome::Entry(c) if c.entry_id == "IncreasedMana7"));
}

#[test]
fn an_essence_text_that_matches_no_entry_is_unknown_not_guessed() {
    let data = craft();
    let unknown = |essence: &str, base: &str| -> String {
        match data.essence_outcome(essence, data.base(base).unwrap()) {
            EssenceOutcome::Unknown(why) => why,
            EssenceOutcome::Entry(c) => panic!("{essence} on {base} guessed {}", c.entry_id),
        }
    };
    // "Strength, Dexterity or Intelligence": all three roll on a ring and the
    // card does not say which one it gives, so it stays unknown and names
    // the three.
    let why = unknown("Essence of the Infinite", "Gold Ring");
    assert!(why.contains("to Strength") && why.contains("to Dexterity") && why.contains("not stated"), "{why}");
    // An essence with no line for the class says so.
    let why = unknown("Essence of Sorcery", "Soldier Cuirass");
    assert!(why.contains("Body Armour"), "{why}");
    // An essence the data does not have.
    let why = unknown("Essence of Wombats", "Soldier Cuirass");
    assert!(why.contains("Essence of Wombats"), "{why}");
    // Every essence line either names an entry or is unknown, on every base
    // of the sample: nothing in between, and never a panic.
    for base in data.bases() {
        for essence in data.essences() {
            if let EssenceOutcome::Entry(c) = data.essence_outcome(&essence.name, base) {
                assert!(data.entry(&c.entry_id).is_some());
            }
        }
    }
}

#[test]
fn an_essence_worded_its_own_way_joins_by_its_rolls_and_meaning() {
    let data = craft();
    let entry_of = |essence: &str, base: &str| match data.essence_outcome(essence, data.base(base).unwrap()) {
        EssenceOutcome::Entry(c) => c.entry_id,
        EssenceOutcome::Unknown(why) => panic!("{essence} on {base}: {why}"),
    };
    // "Global Defences" is the card's word for "Global Armour, Evasion and
    // Energy Shield": the essence's own entry, same rolls.
    assert_eq!(entry_of("Perfect Essence of Enhancement", "Absent Amulet"), "EssenceGlobalDefences1");
    // "Armour, Evasion or Energy Shield" is whichever the base carries.
    assert_eq!(entry_of("Lesser Essence of Enhancement", "Soldier Cuirass"), "LocalIncreasedPhysicalDamageReductionRatingPercent2");
    assert_eq!(entry_of("Greater Essence of Enhancement", "Soldier Cuirass"), "LocalIncreasedPhysicalDamageReductionRatingPercent5");
    // The card's "+3" for one-handers and bows predates 0.5.0; the entry for
    // that hand is the "1H" one, never the two-handed entry the old number
    // happens to read as.
    assert_eq!(entry_of("Perfect Essence of Battle", "Crude Bow"), "EssenceAttackSkillLevel1H1");
}
