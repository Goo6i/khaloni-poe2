//! The rules table against the 0.5.5 rulebook: every row present with its
//! status and label, and each live rule's preconditions and outcome shape
//! on hand-built items with a small fake pool.

use std::collections::{HashMap, VecDeque};

use khaloni_poe2_core::craft::rules::{
    all_rows, attempt, floor, rule, Action, BoneGrade, BoneKind, EssenceTier, Grade, Omen, Orb, Rule, A_ALLOY_REMOVAL,
    A_CHAOS_REFILL, A_FLOOR_FALLBACK, A_OMEN_CONSUMED, A_TRANSMUTE_SIDE, A_WHITTLING_TIES, ALLOYS,
};
use khaloni_poe2_core::craft::types::{
    AffixKind, Candidate, Confidence, Draw, EssenceOutcome, ItemState, Lich, ModOn, Outcome, PoolView, Rarity, Source,
};

use AffixKind::{Prefix, Suffix};

// ---------------------------------------------------------------- fakes

fn cand(id: &str, family: &str, kind: AffixKind, tier: u8, level: u32) -> Candidate {
    Candidate {
        entry_id: id.to_string(),
        family: family.to_string(),
        groups: vec![family.to_string()],
        kind,
        tier,
        required_level: level,
        adds_tags: vec![],
        text: format!("{family} {tier}"),
        desecrated: None,
    }
}

fn dcand(id: &str, family: &str, kind: AffixKind, lich: Lich) -> Candidate {
    Candidate { desecrated: Some(lich), ..cand(id, family, kind, 1, 65) }
}

/// A pool over a fixed list, filtering like the real one: kind, group not
/// on the item, `floor <= required_level <= item level`.
struct FakePool {
    entries: Vec<Candidate>,
    desecrated: Vec<Candidate>,
    essences: HashMap<String, EssenceOutcome>,
}

impl FakePool {
    fn new() -> FakePool {
        FakePool {
            entries: vec![
                cand("Life1", "Life", Prefix, 1, 80),
                cand("Life2", "Life", Prefix, 2, 60),
                cand("Life3", "Life", Prefix, 3, 20),
                cand("Armour1", "Armour", Prefix, 1, 75),
                cand("Armour2", "Armour", Prefix, 2, 30),
                cand("Thorns1", "Thorns", Prefix, 1, 10),
                cand("Fire1", "FireRes", Suffix, 1, 70),
                cand("Fire2", "FireRes", Suffix, 2, 40),
                cand("Cold1", "ColdRes", Suffix, 1, 20),
                cand("Str1", "Strength", Suffix, 1, 55),
                cand("Rarity1", "Rarity", Suffix, 1, 5),
            ],
            desecrated: vec![
                dcand("UlaP", "UlamanPrefix", Prefix, Lich::Ulaman),
                dcand("UlaS", "UlamanSuffix", Suffix, Lich::Ulaman),
                dcand("AmaP", "AmanamuPrefix", Prefix, Lich::Amanamu),
                dcand("AmaS", "AmanamuSuffix", Suffix, Lich::Amanamu),
                dcand("KurP", "KurgalPrefix", Prefix, Lich::Kurgal),
                dcand("KurS", "KurgalSuffix", Suffix, Lich::Kurgal),
            ],
            essences: HashMap::new(),
        }
    }

    fn with_essence(mut self, name: &str, outcome: EssenceOutcome) -> FakePool {
        self.essences.insert(name.to_string(), outcome);
        self
    }
}

fn fits(state: &ItemState, c: &Candidate, floor: u32) -> bool {
    let taken = c.groups.iter().any(|g| state.groups().any(|h| h == g));
    !taken && floor <= c.required_level && c.required_level <= state.item_level
}

impl PoolView for FakePool {
    fn eligible(&self, state: &ItemState, kind: AffixKind, floor: u32) -> Vec<Candidate> {
        self.entries.iter().filter(|c| c.kind == kind && fits(state, c, floor)).cloned().collect()
    }

    fn desecrated(&self, state: &ItemState, kind: Option<AffixKind>, lich: Option<Lich>, floor: u32) -> Vec<Candidate> {
        self.desecrated
            .iter()
            .filter(|c| kind.is_none_or(|k| c.kind == k))
            .filter(|c| lich.is_none_or(|l| c.desecrated == Some(l)))
            .filter(|c| fits(state, c, floor))
            .cloned()
            .collect()
    }

    fn essence(&self, essence: &str, _state: &ItemState) -> EssenceOutcome {
        self.essences
            .get(essence)
            .cloned()
            .unwrap_or_else(|| EssenceOutcome::Unknown(format!("{essence} names no modifier for this class")))
    }

    fn alloy(&self, alloy: &str, _state: &ItemState) -> EssenceOutcome {
        self.essences
            .get(alloy)
            .cloned()
            .unwrap_or_else(|| EssenceOutcome::Unknown(format!("{alloy} names no modifier for this class")))
    }
}

/// A draw that replays scripted answers and falls back to the first
/// option; it records how many options each call was offered.
#[derive(Default)]
struct Script {
    below: VecDeque<usize>,
    pick: VecDeque<usize>,
    offered_below: Vec<usize>,
    offered_pick: Vec<Vec<String>>,
}

impl Script {
    fn below(answers: &[usize]) -> Script {
        Script { below: answers.iter().copied().collect(), ..Script::default() }
    }
    fn picks(answers: &[usize]) -> Script {
        Script { pick: answers.iter().copied().collect(), ..Script::default() }
    }
}

impl Draw for Script {
    fn below(&mut self, n: usize) -> usize {
        self.offered_below.push(n);
        self.below.pop_front().unwrap_or(0).min(n - 1)
    }
    fn pick(&mut self, candidates: &[Candidate]) -> Option<usize> {
        self.offered_pick.push(candidates.iter().map(|c| c.entry_id.clone()).collect());
        if candidates.is_empty() {
            return None;
        }
        Some(self.pick.pop_front().unwrap_or(0).min(candidates.len() - 1))
    }
}

// ---------------------------------------------------------------- items

fn body(rarity: Rarity, mods: Vec<ModOn>) -> ItemState {
    ItemState {
        class: "Body Armour".into(),
        base: "Soldier Cuirass".into(),
        base_tags: vec!["str_armour".into(), "body_armour".into(), "armour".into(), "default".into()],
        item_level: 82,
        rarity,
        corrupted: false,
        mirrored: false,
        sanctified: false,
        mods,
        sockets: 0,
    }
}

fn with_tags(mut s: ItemState, class: &str, tags: &[&str]) -> ItemState {
    s.class = class.into();
    s.base_tags = tags.iter().map(|t| t.to_string()).collect();
    s
}

fn m(family: &str, kind: AffixKind, level: u32, source: Source) -> ModOn {
    let mut c = cand(&format!("{family}X"), family, kind, 1, level).to_mod(source);
    c.text = family.to_string();
    c
}

fn rnd(family: &str, kind: AffixKind, level: u32) -> ModOn {
    m(family, kind, level, Source::Random)
}

fn live(a: &Action) -> khaloni_poe2_core::craft::rules::Spec {
    match rule(a) {
        Rule::Live(spec) => spec,
        other => panic!("{} is not live: {other:?}", a.name()),
    }
}

fn run(a: &Action, s: &ItemState, pool: &FakePool, draw: &mut Script) -> Outcome {
    attempt(a, s, pool, draw)
}

fn applied(o: Outcome) -> ItemState {
    match o {
        Outcome::Applied(s) => s,
        other => panic!("expected an applied outcome, got {other:?}"),
    }
}

fn refused(o: Outcome) -> String {
    match o {
        Outcome::Refused(r) => r,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn families(s: &ItemState) -> Vec<&str> {
    s.mods.iter().map(|m| m.family.as_str()).collect()
}

fn orb(o: Orb) -> Action {
    Action::orb(o)
}

// ---------------------------------------------------------------- L1

/// What the rulebook says each row's status is: "live", "unavailable" or
/// "unknown".
fn expected_status(a: &Action) -> &'static str {
    use Omen::*;
    let omens = a.omens();
    if omens.iter().any(|o| matches!(o, HomogenisingExaltation | HomogenisingCoronation | Corruption | Recombination))
        || matches!(a, Action::Recombinator { .. })
    {
        return "unavailable";
    }
    let unknown_omen = omens.iter().any(|o| {
        matches!(
            o,
            SinistralCoronation
                | DextralCoronation
                | GreaterAnnulment
                | SinistralAlchemy
                | DextralAlchemy
                | CatalysingExaltation
                | Sanctification
                | Putrefaction
                | ChaoticRarity
                | ChaoticQuantity
                | ChaoticMonsters
                | ChaoticEffectiveness
        )
    });
    if unknown_omen {
        return "unknown";
    }
    match a {
        Action::Orb { orb: Orb::Vaal | Orb::Chance | Orb::Artificer | Orb::HinekorasLock, .. } => "unknown",
        Action::Infuser(_) => "unknown",
        Action::Bone { kind: BoneKind::Cranium, .. } => "unknown",
        _ => "live",
    }
}

#[test]
fn every_currency_and_omen_of_the_rulebook_has_a_row() {
    let rows = all_rows();
    let names: Vec<String> = rows.iter().map(Action::name).collect();
    let has = |needle: &str| names.iter().any(|n| n == needle || n.contains(&format!("{needle} +")) || n.ends_with(&format!("+ {needle}")));

    // Section 1: every currency, with the Greater and Perfect grades.
    for name in [
        "Orb of Transmutation",
        "Greater Orb of Transmutation",
        "Perfect Orb of Transmutation",
        "Orb of Augmentation",
        "Greater Orb of Augmentation",
        "Perfect Orb of Augmentation",
        "Regal Orb",
        "Greater Regal Orb",
        "Perfect Regal Orb",
        "Exalted Orb",
        "Greater Exalted Orb",
        "Perfect Exalted Orb",
        "Chaos Orb",
        "Greater Chaos Orb",
        "Perfect Chaos Orb",
        "Orb of Annulment",
        "Orb of Alchemy",
        "Divine Orb",
        "Vaal Orb",
        "Orb of Chance",
        "Fracturing Orb",
        "Artificer's Orb",
        "Hinekora's Lock",
        "Mirror of Kalandra",
        "Armourer's Vaal Infuser",
        "Blacksmith's Vaal Infuser",
        "Arcanist's Vaal Infuser",
        "Catalysing Vaal Infuser",
    ] {
        assert!(has(name), "no row for {name}");
    }

    // Section 2: every omen of the table, on its orb.
    for name in [
        "Omen of Sinistral Exaltation",
        "Omen of Dextral Exaltation",
        "Omen of Greater Exaltation",
        "Omen of Homogenising Exaltation",
        "Omen of Homogenising Coronation",
        "Omen of Sinistral Coronation",
        "Omen of Dextral Coronation",
        "Omen of Sinistral Annulment",
        "Omen of Dextral Annulment",
        "Omen of Greater Annulment",
        "Omen of Light",
        "Omen of Sinistral Erasure",
        "Omen of Dextral Erasure",
        "Omen of Whittling",
        "Omen of Chaotic Rarity",
        "Omen of Chaotic Quantity",
        "Omen of Chaotic Monsters",
        "Omen of Chaotic Effectiveness",
        "Omen of Sinistral Alchemy",
        "Omen of Dextral Alchemy",
        "Omen of Sinistral Crystallisation",
        "Omen of Dextral Crystallisation",
        "Omen of Catalysing Exaltation",
        "Omen of Sanctification",
        "Omen of the Blessed",
        "Omen of Corruption",
        "Omen of Chance",
        "Omen of the Ancients",
        "Omen of Sinistral Necromancy",
        "Omen of Dextral Necromancy",
        "Omen of Abyssal Echoes",
        "Omen of the Blackblooded",
        "Omen of the Liege",
        "Omen of the Sovereign",
        "Omen of Putrefaction",
        "Omen of Recombination",
    ] {
        assert!(has(name), "no row for {name}");
    }

    // Section 3: nineteen essences in four tiers and the five corrupted
    // ones, as the essence data lists them.
    let essences: Vec<&Action> = rows.iter().filter(|a| matches!(a, Action::Essence { .. }) && a.omens().is_empty()).collect();
    assert_eq!(essences.len(), 19 * 4 + 5);
    for name in [
        "Lesser Essence of the Body",
        "Essence of the Body",
        "Greater Essence of the Body",
        "Perfect Essence of the Body",
        "Greater Essence of Thawing",
        "Perfect Essence of Command",
        "Essence of Hysteria",
        "Essence of Delirium",
        "Essence of Horror",
        "Essence of Insanity",
        "Essence of the Abyss",
    ] {
        assert!(has(name), "no row for {name}");
    }

    // Section 4: the bones the game has (Cranium and Vertebrae come only
    // Preserved).
    let bones: Vec<String> = rows.iter().filter(|a| matches!(a, Action::Bone { .. }) && a.omens().is_empty()).map(Action::name).collect();
    let mut want = vec![
        "Gnawed Jawbone",
        "Gnawed Rib",
        "Gnawed Collarbone",
        "Preserved Jawbone",
        "Preserved Rib",
        "Preserved Collarbone",
        "Preserved Cranium",
        "Preserved Vertebrae",
        "Ancient Jawbone",
        "Ancient Rib",
        "Ancient Collarbone",
    ];
    want.sort();
    let mut got = bones.clone();
    got.sort();
    assert_eq!(got, want);

    // Section 5 and 9: the Recombinator and the Alloys.
    assert!(has("Recombinator"));
    for name in ALLOYS {
        assert!(has(name), "no row for {name}");
    }
    assert!(rows.contains(&Action::Buy));

    // Every row resolves to the status and label the rulebook gives it.
    for a in &rows {
        let got = match rule(a) {
            Rule::Live(spec) => {
                // The table's labels: in-game text for every live row but
                // buying, which rests on real trade responses, and the reveal
                // of an unrevealed modifier, which the 0.3.0 patch notes word.
                let want = match a {
                    Action::Buy => Confidence::Data,
                    Action::Reveal { .. } => Confidence::Patch,
                    _ => Confidence::Game,
                };
                assert_eq!(spec.source, want, "{}", a.name());
                "live"
            }
            Rule::Unavailable(r) => {
                assert!(!r.trim().is_empty(), "{} has no reason", a.name());
                "unavailable"
            }
            Rule::Unknown(r) => {
                assert!(!r.trim().is_empty(), "{} has no reason", a.name());
                "unknown"
            }
        };
        assert_eq!(got, expected_status(a), "{}", a.name());
    }

    // No two rows are the same action.
    let mut sorted = names.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), names.len());
}

#[test]
fn standard_only_and_removed_items_are_unavailable_with_their_reason() {
    let cases = [
        (orb(Orb::Exalted).with(Omen::HomogenisingExaltation), "Standard"),
        (orb(Orb::Regal).with(Omen::HomogenisingCoronation), "Standard"),
        (orb(Orb::Vaal).with(Omen::Corruption), "Standard"),
        (Action::Recombinator { omens: vec![] }, "disabled"),
        (Action::Recombinator { omens: vec![Omen::Recombination] }, "removed"),
    ];
    let pool = FakePool::new();
    let item = body(Rarity::Rare, vec![rnd("Life", Prefix, 80)]);
    for (a, word) in cases {
        match rule(&a) {
            Rule::Unavailable(r) => assert!(r.contains(word), "{}: {r}", a.name()),
            other => panic!("{} should be unavailable: {other:?}", a.name()),
        }
        let r = refused(run(&a, &item, &pool, &mut Script::default()));
        assert!(r.starts_with("unavailable: "), "{r}");
    }

    // Rows that do not exist in the game are refused the same way.
    for a in [
        Action::graded(Orb::Annulment, Grade::Greater),
        Action::bone(BoneKind::Cranium, BoneGrade::Gnawed),
        Action::bone(BoneKind::Vertebrae, BoneGrade::Ancient),
        orb(Orb::Chaos).with(Omen::SinistralExaltation),
        Action::essence("the Body", EssenceTier::Greater).with(Omen::SinistralCrystallisation),
    ] {
        assert!(matches!(rule(&a), Rule::Unavailable(_)), "{}", a.name());
    }
}

#[test]
fn the_unsourced_rules_are_unknown_with_their_reason() {
    let cases = [
        (orb(Orb::Chance), "success"),
        (orb(Orb::Chance).with(Omen::Chance), "success"),
        (orb(Orb::Chance).with(Omen::Ancients), "success"),
        (orb(Orb::Exalted).with(Omen::CatalysingExaltation), "magnitude"),
        (Action::bone(BoneKind::Rib, BoneGrade::Preserved).with(Omen::Putrefaction), "one-desecrated"),
        (orb(Orb::Divine).with(Omen::Sanctification), "multiplier"),
        (orb(Orb::Regal).with(Omen::SinistralCoronation), "price feed"),
        (orb(Orb::Regal).with(Omen::DextralCoronation), "price feed"),
        (orb(Orb::Annulment).with(Omen::GreaterAnnulment), "price feed"),
        (orb(Orb::Alchemy).with(Omen::SinistralAlchemy), "price feed"),
        (orb(Orb::Alchemy).with(Omen::DextralAlchemy), "price feed"),
        (orb(Orb::Chaos).with(Omen::ChaoticRarity), "Waystone"),
        (orb(Orb::Chaos).with(Omen::ChaoticEffectiveness), "Waystone"),
        (orb(Orb::Vaal), "percentages"),
        (orb(Orb::Artificer), "Socket cap"),
        (orb(Orb::HinekorasLock), "foresee"),
        (Action::Infuser(khaloni_poe2_core::craft::rules::Infuser::Armourer), "Corrupting"),
        (Action::alloy("Gilded Alloy"), "Alloy table"),
        (Action::bone(BoneKind::Cranium, BoneGrade::Preserved), "jewel"),
        (orb(Orb::Exalted).with(Omen::SinistralExaltation).with(Omen::DextralExaltation), "both"),
        (Action::essence("Brilliance", EssenceTier::Greater), "not in the rulebook"),
    ];
    let pool = FakePool::new();
    let items = [
        body(Rarity::Normal, vec![]),
        body(Rarity::Magic, vec![rnd("Life", Prefix, 80)]),
        body(Rarity::Rare, vec![rnd("Life", Prefix, 80), rnd("FireRes", Suffix, 70)]),
    ];
    for (a, word) in cases {
        match rule(&a) {
            Rule::Unknown(r) => assert!(r.contains(word), "{}: {r}", a.name()),
            other => panic!("{} should be unknown: {other:?}", a.name()),
        }
        // An unknown rule never yields an item, on any item.
        for item in &items {
            let r = refused(run(&a, item, &pool, &mut Script::default()));
            assert!(r.starts_with("unknown: "), "{r}");
        }
    }
}

// ---------------------------------------------------------------- L2

#[test]
fn transmute_augment_regal_exalt_follow_the_affix_caps() {
    let pool = FakePool::new();
    let normal = body(Rarity::Normal, vec![]);

    // Transmute: normal to magic with one modifier; the side is a coin flip.
    let s = applied(run(&orb(Orb::Transmutation), &normal, &pool, &mut Script::below(&[0])));
    assert_eq!(s.rarity, Rarity::Magic);
    assert_eq!(s.mods.len(), 1);
    assert_eq!(s.mods[0].kind, Prefix);
    let mut d = Script::below(&[1]);
    let s = applied(run(&orb(Orb::Transmutation), &normal, &pool, &mut d));
    assert_eq!(s.mods[0].kind, Suffix);
    assert_eq!(d.offered_below, vec![2]);
    assert_eq!(s.mods[0].source, Source::Random);
    assert!(refused(run(&orb(Orb::Transmutation), &body(Rarity::Magic, vec![]), &pool, &mut Script::default())).contains("Normal"));

    // Augment: fills the one open side of a magic item.
    let magic = body(Rarity::Magic, vec![rnd("Life", Prefix, 80)]);
    let mut d = Script::default();
    let s = applied(run(&orb(Orb::Augmentation), &magic, &pool, &mut d));
    assert_eq!(s.rarity, Rarity::Magic);
    assert_eq!(s.count(Prefix), 1);
    assert_eq!(s.count(Suffix), 1);
    assert!(d.offered_pick[0].iter().all(|id| ["Fire1", "Fire2", "Cold1", "Str1", "Rarity1"].contains(&id.as_str())));
    let full_magic = body(Rarity::Magic, vec![rnd("Life", Prefix, 80), rnd("FireRes", Suffix, 70)]);
    assert!(refused(run(&orb(Orb::Augmentation), &full_magic, &pool, &mut Script::default())).contains("open"));

    // Regal: magic to rare keeping the mods, one more from either side.
    let s = applied(run(&orb(Orb::Regal), &full_magic, &pool, &mut Script::default()));
    assert_eq!(s.rarity, Rarity::Rare);
    assert_eq!(s.mods.len(), 3);
    assert_eq!(&families(&s)[..2], &["Life", "FireRes"]);
    assert!(refused(run(&orb(Orb::Regal), &normal, &pool, &mut Script::default())).contains("Magic"));

    // Exalt: only the side with room is offered.
    let three_prefixes =
        body(Rarity::Rare, vec![rnd("Life", Prefix, 80), rnd("Armour", Prefix, 75), rnd("Thorns", Prefix, 10)]);
    let mut d = Script::default();
    let s = applied(run(&orb(Orb::Exalted), &three_prefixes, &pool, &mut d));
    assert_eq!(s.mods.len(), 4);
    assert_eq!(s.mods[3].kind, Suffix);
    assert!(d.offered_pick[0].iter().all(|id| !id.starts_with("Life") && !id.starts_with("Armour") && id != "Thorns1"));
    let six = body(
        Rarity::Rare,
        vec![
            rnd("Life", Prefix, 80),
            rnd("Armour", Prefix, 75),
            rnd("Thorns", Prefix, 10),
            rnd("FireRes", Suffix, 70),
            rnd("ColdRes", Suffix, 20),
            rnd("Strength", Suffix, 55),
        ],
    );
    assert!(refused(run(&orb(Orb::Exalted), &six, &pool, &mut Script::default())).contains("open"));
    assert!(refused(run(&orb(Orb::Exalted), &magic, &pool, &mut Script::default())).contains("Rare"));

    // A group already on the item keeps its rivals out: the pool, not the
    // rule, filters it, and the rule offers exactly the pool's answer.
    let mut d = Script::default();
    applied(run(&orb(Orb::Exalted), &body(Rarity::Rare, vec![rnd("Life", Prefix, 80)]), &pool, &mut d));
    assert!(!d.offered_pick[0].iter().any(|id| id.starts_with("Life")));

    // Locked items take nothing.
    for lock in 0..3 {
        let mut s = three_prefixes.clone();
        match lock {
            0 => s.corrupted = true,
            1 => s.mirrored = true,
            _ => s.sanctified = true,
        }
        for a in [orb(Orb::Exalted), orb(Orb::Chaos), orb(Orb::Annulment), orb(Orb::Divine), orb(Orb::Mirror)] {
            assert!(!refused(run(&a, &s, &pool, &mut Script::default())).is_empty());
        }
    }

    // Jewels have no sourced prefix/suffix split, so nothing adds to them.
    let jewel = with_tags(body(Rarity::Rare, vec![rnd("Life", Prefix, 80)]), "Jewel", &["jewel", "default"]);
    assert!(refused(run(&orb(Orb::Exalted), &jewel, &pool, &mut Script::default())).starts_with("unknown: "));
}

#[test]
fn chaos_removes_one_and_adds_one() {
    let pool = FakePool::new();
    let item = body(
        Rarity::Rare,
        vec![rnd("Life", Prefix, 80), rnd("Armour", Prefix, 75), rnd("FireRes", Suffix, 70), rnd("ColdRes", Suffix, 20)],
    );
    // Remove the third modifier (FireRes), then add from either side.
    let mut d = Script::below(&[2]);
    let s = applied(run(&orb(Orb::Chaos), &item, &pool, &mut d));
    assert_eq!(s.mods.len(), 4);
    assert!(!families(&s).contains(&"FireRes") || s.mods[3].family == "FireRes");
    assert_eq!(d.offered_below, vec![4]);
    let offered = &d.offered_pick[0];
    assert!(offered.iter().any(|id| id == "Thorns1"), "prefixes offered: {offered:?}");
    assert!(offered.iter().any(|id| id == "Str1"), "suffixes offered: {offered:?}");

    // Fractured modifiers are never removed.
    let mut fractured = item.clone();
    fractured.mods[0].source = Source::Fractured;
    let mut d = Script::below(&[0]);
    let s = applied(run(&orb(Orb::Chaos), &fractured, &pool, &mut d));
    assert_eq!(d.offered_below, vec![3]);
    assert_eq!(s.mods[0].family, "Life");
    assert_eq!(s.mods[0].source, Source::Fractured);

    // Erasure narrows the removal to one side.
    let mut d = Script::below(&[1]);
    let s = applied(run(&orb(Orb::Chaos).with(Omen::DextralErasure), &item, &pool, &mut d));
    assert_eq!(d.offered_below, vec![2]);
    assert!(!families(&s).contains(&"ColdRes") || s.mods.last().unwrap().family == "ColdRes");
    assert!(families(&s).contains(&"Life") && families(&s).contains(&"Armour"));
    let no_suffix = body(Rarity::Rare, vec![rnd("Life", Prefix, 80)]);
    assert!(refused(run(&orb(Orb::Chaos).with(Omen::DextralErasure), &no_suffix, &pool, &mut Script::default()))
        .contains("suffix"));

    // Whittling takes the lowest-level modifier: ColdRes at 20.
    let mut d = Script::default();
    let s = applied(run(&orb(Orb::Chaos).with(Omen::Whittling), &item, &pool, &mut d));
    assert_eq!(&families(&s)[..3], &["Life", "Armour", "FireRes"]);
    assert!(d.offered_below.is_empty(), "no tie, no draw");
    // A tie is broken by a uniform draw among the tied.
    let tied = body(Rarity::Rare, vec![rnd("Life", Prefix, 20), rnd("Armour", Prefix, 75), rnd("ColdRes", Suffix, 20)]);
    let mut d = Script::below(&[1]);
    let s = applied(run(&orb(Orb::Chaos).with(Omen::Whittling), &tied, &pool, &mut d));
    assert_eq!(d.offered_below, vec![2]);
    assert_eq!(&families(&s)[..2], &["Life", "Armour"]);
    // An unknown level cannot be ranked.
    let mut unknown_level = tied.clone();
    unknown_level.mods[1].required_level = None;
    assert!(refused(run(&orb(Orb::Chaos).with(Omen::Whittling), &unknown_level, &pool, &mut Script::default()))
        .contains("level"));

    // Chaos is for rares only, and needs a removable modifier.
    assert!(refused(run(&orb(Orb::Chaos), &body(Rarity::Magic, vec![rnd("Life", Prefix, 80)]), &pool, &mut Script::default()))
        .contains("Rare"));
    let all_fractured = body(Rarity::Rare, vec![m("Life", Prefix, 80, Source::Fractured)]);
    assert!(!refused(run(&orb(Orb::Chaos), &all_fractured, &pool, &mut Script::default())).is_empty());
}

#[test]
fn annul_removes_one_and_alchemy_makes_four() {
    let pool = FakePool::new();
    let item = body(
        Rarity::Rare,
        vec![
            rnd("Life", Prefix, 80),
            m("KurgalPrefix", Prefix, 65, Source::Desecrated),
            rnd("FireRes", Suffix, 70),
            m("Crafted", Suffix, 60, Source::Crafted),
        ],
    );
    let mut d = Script::below(&[3]);
    let s = applied(run(&orb(Orb::Annulment), &item, &pool, &mut d));
    assert_eq!(families(&s), vec!["Life", "KurgalPrefix", "FireRes"]);
    assert_eq!(s.rarity, Rarity::Rare);
    assert_eq!(d.offered_below, vec![4]);

    // Annulment works on magic items too, and never on a fractured mod.
    let magic = body(Rarity::Magic, vec![m("Life", Prefix, 80, Source::Fractured), rnd("FireRes", Suffix, 70)]);
    let s = applied(run(&orb(Orb::Annulment), &magic, &pool, &mut Script::default()));
    assert_eq!(families(&s), vec!["Life"]);
    assert_eq!(s.rarity, Rarity::Magic);
    assert!(!refused(run(&orb(Orb::Annulment), &body(Rarity::Normal, vec![]), &pool, &mut Script::default())).is_empty());

    // Side omens and Light narrow the removal.
    let mut d = Script::default();
    let s = applied(run(&orb(Orb::Annulment).with(Omen::SinistralAnnulment), &item, &pool, &mut d));
    assert_eq!(d.offered_below, vec![2]);
    assert_eq!(s.count(Prefix), 1);
    let mut d = Script::default();
    let s = applied(run(&orb(Orb::Annulment).with(Omen::Light), &item, &pool, &mut d));
    assert!(d.offered_below.is_empty() || d.offered_below == vec![1]);
    assert!(!s.desecrated_slot_used());
    assert_eq!(s.mods.len(), 3);
    let clean = body(Rarity::Rare, vec![rnd("Life", Prefix, 80)]);
    assert!(refused(run(&orb(Orb::Annulment).with(Omen::Light), &clean, &pool, &mut Script::default())).contains("Desecrated"));

    // Alchemy: normal or magic to rare with four modifiers, magic ones gone.
    for start in [body(Rarity::Normal, vec![]), body(Rarity::Magic, vec![rnd("Rarity", Suffix, 5)])] {
        let s = applied(run(&orb(Orb::Alchemy), &start, &pool, &mut Script::default()));
        assert_eq!(s.rarity, Rarity::Rare);
        assert_eq!(s.mods.len(), 4);
        assert!(s.count(Prefix) <= 3 && s.count(Suffix) <= 3);
        assert!(s.mods.iter().all(|m| m.source == Source::Random));
        let mut fams = families(&s);
        fams.sort();
        fams.dedup();
        assert_eq!(fams.len(), 4, "one mod per group");
    }
    assert!(refused(run(&orb(Orb::Alchemy), &item, &pool, &mut Script::default())).contains("Normal or Magic"));

    // Divine keeps every tier (values are not part of the planned item);
    // Fracturing locks one of four or more mods; Mirror makes it mirrored.
    let s = applied(run(&orb(Orb::Divine), &item, &pool, &mut Script::default()));
    assert_eq!(s, item);
    let s = applied(run(&orb(Orb::Divine).with(Omen::Blessed), &item, &pool, &mut Script::default()));
    assert_eq!(s, item);
    let mut d = Script::below(&[2]);
    let s = applied(run(&orb(Orb::Fracturing), &item, &pool, &mut d));
    assert_eq!(d.offered_below, vec![4]);
    assert_eq!(s.mods[2].source, Source::Fractured);
    assert!(refused(run(&orb(Orb::Fracturing), &s, &pool, &mut Script::default())).contains("fractured"));
    let three = body(Rarity::Rare, vec![rnd("Life", Prefix, 80), rnd("Armour", Prefix, 75), rnd("FireRes", Suffix, 70)]);
    assert!(refused(run(&orb(Orb::Fracturing), &three, &pool, &mut Script::default())).contains("4"));
    let s = applied(run(&orb(Orb::Mirror), &item, &pool, &mut Script::default()));
    assert!(s.mirrored && s.locked());
}

#[test]
fn greater_and_perfect_orbs_carry_their_floors() {
    use Grade::*;
    for (o, g, want) in [
        (Orb::Transmutation, Normal, 0),
        (Orb::Transmutation, Greater, 44),
        (Orb::Transmutation, Perfect, 70),
        (Orb::Augmentation, Greater, 44),
        (Orb::Augmentation, Perfect, 70),
        (Orb::Regal, Greater, 35),
        (Orb::Regal, Perfect, 50),
        (Orb::Exalted, Normal, 0),
        (Orb::Exalted, Greater, 35),
        (Orb::Exalted, Perfect, 50),
        (Orb::Chaos, Greater, 35),
        (Orb::Chaos, Perfect, 50),
    ] {
        assert_eq!(floor(o, g), want, "{o:?} {g:?}");
    }

    let pool = FakePool::new();
    let rare = body(Rarity::Rare, vec![rnd("Strength", Suffix, 55)]);
    // Perfect Exalted (50): Life 80/60, Armour 75, FireRes 70; Thorns (10
    // only) and Cold (20 only) keep their best tier by the fallback, and
    // the lower tiers of Life, Armour and FireRes are gone.
    let mut d = Script::default();
    applied(run(&Action::graded(Orb::Exalted, Perfect), &rare, &pool, &mut d));
    let mut offered = d.offered_pick[0].clone();
    offered.sort();
    let mut want: Vec<String> =
        ["Life1", "Life2", "Armour1", "Thorns1", "Fire1", "Cold1", "Rarity1"].iter().map(|s| s.to_string()).collect();
    want.sort();
    assert_eq!(offered, want);

    // Greater Transmutation (44): a family with a tier above the floor
    // loses its low tiers; a family entirely below keeps only its best.
    let mut d = Script::below(&[0]);
    applied(run(&Action::graded(Orb::Transmutation, Greater), &body(Rarity::Normal, vec![]), &pool, &mut d));
    let mut offered = d.offered_pick[0].clone();
    offered.sort();
    let mut want: Vec<String> = ["Life1", "Life2", "Armour1", "Thorns1"].iter().map(|s| s.to_string()).collect();
    want.sort();
    assert_eq!(offered, want);

    // Floors also narrow Chaos's add.
    let mut d = Script::default();
    applied(run(&Action::graded(Orb::Chaos, Greater), &rare, &pool, &mut d));
    assert!(!d.offered_pick[0].contains(&"Life3".to_string()));

    // Only these five orbs come graded.
    for o in [Orb::Annulment, Orb::Alchemy, Orb::Divine, Orb::Fracturing, Orb::Mirror] {
        assert!(matches!(rule(&Action::graded(o, Greater)), Rule::Unavailable(_)));
        assert!(matches!(rule(&Action::graded(o, Perfect)), Rule::Unavailable(_)));
    }
}

#[test]
fn each_omen_narrows_its_orb_as_its_text_says() {
    let pool = FakePool::new();
    let one_each = body(Rarity::Rare, vec![rnd("Life", Prefix, 80), rnd("FireRes", Suffix, 70)]);

    // Sinistral / Dextral Exaltation: only prefixes / suffixes offered.
    let mut d = Script::default();
    let s = applied(run(&orb(Orb::Exalted).with(Omen::SinistralExaltation), &one_each, &pool, &mut d));
    assert_eq!(s.mods[2].kind, Prefix);
    assert!(d.offered_pick[0].iter().all(|id| ["Armour1", "Armour2", "Thorns1"].contains(&id.as_str())));
    let s = applied(run(&orb(Orb::Exalted).with(Omen::DextralExaltation), &one_each, &pool, &mut Script::default()));
    assert_eq!(s.mods[2].kind, Suffix);
    let three_prefixes =
        body(Rarity::Rare, vec![rnd("Life", Prefix, 80), rnd("Armour", Prefix, 75), rnd("Thorns", Prefix, 10)]);
    assert!(refused(run(&orb(Orb::Exalted).with(Omen::SinistralExaltation), &three_prefixes, &pool, &mut Script::default()))
        .contains("prefix"));

    // Greater Exaltation: two modifiers; with Dextral, two suffixes.
    let s = applied(run(&orb(Orb::Exalted).with(Omen::GreaterExaltation), &one_each, &pool, &mut Script::default()));
    assert_eq!(s.mods.len(), 4);
    let s = applied(run(
        &orb(Orb::Exalted).with(Omen::GreaterExaltation).with(Omen::DextralExaltation),
        &one_each,
        &pool,
        &mut Script::default(),
    ));
    assert_eq!((s.count(Prefix), s.count(Suffix)), (1, 3));
    // With room for only one, it adds one and the omen is spent.
    let five = body(
        Rarity::Rare,
        vec![
            rnd("Life", Prefix, 80),
            rnd("Armour", Prefix, 75),
            rnd("Thorns", Prefix, 10),
            rnd("FireRes", Suffix, 70),
            rnd("ColdRes", Suffix, 20),
        ],
    );
    let s = applied(run(&orb(Orb::Exalted).with(Omen::GreaterExaltation), &five, &pool, &mut Script::default()));
    assert_eq!(s.mods.len(), 6);

    // Crystallisation steers a Perfect essence's removal.
    let crafted = cand("Ess", "EssenceLife", Prefix, 1, 60);
    let pool = FakePool::new().with_essence("Perfect Essence of the Body", EssenceOutcome::Entry(crafted));
    let full = body(
        Rarity::Rare,
        vec![
            rnd("Life", Prefix, 80),
            rnd("Armour", Prefix, 75),
            rnd("Thorns", Prefix, 10),
            rnd("FireRes", Suffix, 70),
            rnd("ColdRes", Suffix, 20),
            rnd("Strength", Suffix, 55),
        ],
    );
    let perfect = Action::essence("the Body", EssenceTier::Perfect).with(Omen::SinistralCrystallisation);
    let mut d = Script::below(&[2]);
    let s = applied(run(&perfect, &full, &pool, &mut d));
    assert_eq!(d.offered_below, vec![3]);
    assert_eq!(families(&s), vec!["Life", "Armour", "FireRes", "ColdRes", "Strength", "EssenceLife"]);

    // Necromancy and the lich omens narrow the desecration pool.
    let pool = FakePool::new();
    let ring = with_tags(body(Rarity::Rare, vec![rnd("Life", Prefix, 80)]), "Ring", &["ring", "default"]);
    let mut d = Script::default();
    let a = Action::bone(BoneKind::Collarbone, BoneGrade::Preserved).with(Omen::DextralNecromancy).with(Omen::Liege);
    match run(&a, &ring, &pool, &mut d) {
        Outcome::Reveal(options) => {
            assert_eq!(options.len(), 1);
            assert_eq!(options[0].mods.last().unwrap().family, "AmanamuSuffix");
        }
        other => panic!("{other:?}"),
    }
    for (omen, fam) in [(Omen::Blackblooded, "Kurgal"), (Omen::Sovereign, "Ulaman"), (Omen::Liege, "Amanamu")] {
        let mut d = Script::default();
        let a = Action::bone(BoneKind::Collarbone, BoneGrade::Preserved).with(omen);
        assert!(matches!(run(&a, &ring, &pool, &mut d), Outcome::Reveal(_)));
        assert!(d.offered_pick[0].iter().all(|id| id.starts_with(&fam[..3])), "{omen:?}: {:?}", d.offered_pick[0]);
    }
    // The lich omens act on weapons and jewellery only.
    let armour = body(Rarity::Rare, vec![rnd("Life", Prefix, 80)]);
    let a = Action::bone(BoneKind::Rib, BoneGrade::Preserved).with(Omen::Sovereign);
    assert!(refused(run(&a, &armour, &pool, &mut Script::default())).contains("Weapon or Jewellery"));

    // Omens on the wrong orb are not a row.
    assert!(matches!(rule(&orb(Orb::Exalted).with(Omen::Whittling)), Rule::Unavailable(_)));
    assert!(matches!(rule(&orb(Orb::Regal).with(Omen::Light)), Rule::Unavailable(_)));
}

#[test]
fn essences_upgrade_or_replace_per_tier() {
    let guaranteed = cand("EssLife", "EssenceLife", Prefix, 3, 44);
    let pool = FakePool::new()
        .with_essence("Greater Essence of the Body", EssenceOutcome::Entry(guaranteed.clone()))
        .with_essence("Lesser Essence of the Body", EssenceOutcome::Entry(guaranteed.clone()))
        .with_essence("Essence of Hysteria", EssenceOutcome::Entry(cand("Hys", "Hysteria", Suffix, 1, 70)));

    // Lesser / normal / Greater: magic to rare, mods kept, the guaranteed
    // one added as the crafted modifier.
    let magic = body(Rarity::Magic, vec![rnd("FireRes", Suffix, 70)]);
    for tier in [EssenceTier::Lesser, EssenceTier::Greater] {
        let s = applied(run(&Action::essence("the Body", tier), &magic, &pool, &mut Script::default()));
        assert_eq!(s.rarity, Rarity::Rare);
        assert_eq!(families(&s), vec!["FireRes", "EssenceLife"]);
        assert_eq!(s.mods[1].source, Source::Crafted);
        assert!(s.crafted_slot_used());
    }
    // The data knows no outcome for the plain Essence of the Body here: a
    // refusal with the data's reason, never a guess.
    let r = refused(run(&Action::essence("the Body", EssenceTier::Normal), &magic, &pool, &mut Script::default()));
    assert!(r.starts_with("unknown: ") && r.contains("names no modifier"), "{r}");
    // Rarity and the crafted slot.
    let rare = body(Rarity::Rare, vec![rnd("FireRes", Suffix, 70)]);
    assert!(refused(run(&Action::essence("the Body", EssenceTier::Greater), &rare, &pool, &mut Script::default())).contains("Magic"));
    let crafted_magic = body(Rarity::Magic, vec![m("Crafted", Suffix, 60, Source::Crafted)]);
    assert!(refused(run(&Action::essence("the Body", EssenceTier::Greater), &crafted_magic, &pool, &mut Script::default()))
        .contains("crafted"));
    // The guaranteed mod's group already on the item: the rulebook has no
    // rule, so no outcome.
    let life_magic = body(Rarity::Magic, vec![rnd("EssenceLife", Prefix, 44)]);
    assert!(refused(run(&Action::essence("the Body", EssenceTier::Greater), &life_magic, &pool, &mut Script::default()))
        .starts_with("unknown: "));

    // Perfect and corrupted: on rares, remove one, add the guaranteed one.
    let four = body(
        Rarity::Rare,
        vec![rnd("Life", Prefix, 80), rnd("Armour", Prefix, 75), rnd("FireRes", Suffix, 70), rnd("ColdRes", Suffix, 20)],
    );
    let mut d = Script::below(&[0]);
    let s = applied(run(&Action::essence("Hysteria", EssenceTier::Corrupted), &four, &pool, &mut d));
    assert_eq!(d.offered_below, vec![4]);
    assert_eq!(families(&s), vec!["Armour", "FireRes", "ColdRes", "Hysteria"]);
    assert_eq!(s.mods[3].source, Source::Crafted);
    assert!(refused(run(&Action::essence("Hysteria", EssenceTier::Corrupted), &magic, &pool, &mut Script::default()))
        .contains("Rare"));
    // A removal that leaves the essence's side full has no sourced rule.
    let full = body(
        Rarity::Rare,
        vec![
            rnd("Life", Prefix, 80),
            rnd("Armour", Prefix, 75),
            rnd("Thorns", Prefix, 10),
            rnd("FireRes", Suffix, 70),
            rnd("ColdRes", Suffix, 20),
            rnd("Strength", Suffix, 55),
        ],
    );
    let r = refused(run(&Action::essence("Hysteria", EssenceTier::Corrupted), &full, &pool, &mut Script::below(&[0])));
    assert!(r.starts_with("unknown: "), "{r}");
    let s = applied(run(&Action::essence("Hysteria", EssenceTier::Corrupted), &full, &pool, &mut Script::below(&[3])));
    assert_eq!(s.mods.len(), 6);
    // Unknown data outcome for a Perfect essence: refused with the reason.
    let r = refused(run(&Action::essence("the Mind", EssenceTier::Perfect), &four, &pool, &mut Script::default()));
    assert!(r.starts_with("unknown: "), "{r}");
    // Crystallisation needs a removable mod on its side.
    let no_prefix = body(Rarity::Rare, vec![rnd("FireRes", Suffix, 70), m("Life", Prefix, 80, Source::Fractured)]);
    let a = Action::essence("Hysteria", EssenceTier::Corrupted).with(Omen::SinistralCrystallisation);
    assert!(refused(run(&a, &no_prefix, &pool, &mut Script::default())).contains("prefix"));
}

#[test]
fn bones_respect_class_level_and_the_one_desecrated_slot() {
    let pool = FakePool::new();
    let armour = body(Rarity::Rare, vec![rnd("Life", Prefix, 80), rnd("FireRes", Suffix, 70)]);

    // A reveal offers three options, each the item plus one desecrated mod.
    let mut d = Script::default();
    let rib = Action::bone(BoneKind::Rib, BoneGrade::Preserved);
    match run(&rib, &armour, &pool, &mut d) {
        Outcome::Reveal(options) => {
            assert_eq!(options.len(), 3);
            let mut picked: Vec<&str> = options.iter().map(|o| o.mods.last().unwrap().family.as_str()).collect();
            for o in &options {
                assert_eq!(o.mods.len(), 3);
                assert_eq!(o.mods.last().unwrap().source, Source::Desecrated);
                assert!(o.desecrated_slot_used());
            }
            picked.sort();
            picked.dedup();
            assert_eq!(picked.len(), 3, "three different options");
        }
        other => panic!("{other:?}"),
    }
    assert!(live(&rib).reroll.is_none());

    // Class: a Rib on a ring, a Collarbone on armour, a Jawbone on a belt.
    let ring = with_tags(armour.clone(), "Ring", &["ring", "default"]);
    let bow = with_tags(armour.clone(), "Bow", &["bow", "two_hand_weapon", "weapon", "default"]);
    let waystone = with_tags(armour.clone(), "Waystone", &["map", "default"]);
    assert!(refused(run(&rib, &ring, &pool, &mut Script::default())).contains("Armour"));
    assert!(refused(run(&Action::bone(BoneKind::Collarbone, BoneGrade::Preserved), &armour, &pool, &mut Script::default()))
        .contains("Amulet"));
    assert!(matches!(run(&Action::bone(BoneKind::Jawbone, BoneGrade::Preserved), &bow, &pool, &mut Script::default()), Outcome::Reveal(_)));
    assert!(matches!(
        run(&Action::bone(BoneKind::Vertebrae, BoneGrade::Preserved), &waystone, &pool, &mut Script::default()),
        Outcome::Reveal(_)
    ));
    assert!(!refused(run(&Action::bone(BoneKind::Jawbone, BoneGrade::Ancient), &ring, &pool, &mut Script::default())).is_empty());

    // Level: Gnawed only up to item level 64; Ancient floors at 40.
    assert!(refused(run(&Action::bone(BoneKind::Rib, BoneGrade::Gnawed), &armour, &pool, &mut Script::default())).contains("64"));
    let mut low = armour.clone();
    low.item_level = 64;
    let mut low_pool = FakePool::new();
    for c in &mut low_pool.desecrated {
        c.required_level = 60;
    }
    assert!(matches!(run(&Action::bone(BoneKind::Rib, BoneGrade::Gnawed), &low, &low_pool, &mut Script::default()), Outcome::Reveal(_)));
    let mut ancient_pool = FakePool::new();
    ancient_pool.desecrated[0].required_level = 30;
    ancient_pool.desecrated[1].required_level = 45;
    let mut d = Script::default();
    run(&Action::bone(BoneKind::Rib, BoneGrade::Ancient), &armour, &ancient_pool, &mut d);
    assert!(d.offered_pick[0].contains(&"UlaS".to_string()));
    // UlaP (30) sits below 40, but it is its family's only tier, so the
    // fallback keeps it rollable.
    assert!(d.offered_pick[0].contains(&"UlaP".to_string()));

    // One desecrated modifier per item.
    let mut desecrated = armour.clone();
    desecrated.mods.push(m("KurgalSuffix", Suffix, 65, Source::Desecrated));
    assert!(refused(run(&rib, &desecrated, &pool, &mut Script::default())).contains("Desecrated"));
    // Rare only; locked items refused.
    assert!(refused(run(&rib, &body(Rarity::Magic, vec![]), &pool, &mut Script::default())).contains("Rare"));
    let mut corrupt = armour.clone();
    corrupt.corrupted = true;
    assert!(!refused(run(&rib, &corrupt, &pool, &mut Script::default())).is_empty());

    // A full item loses a random non-fractured modifier first.
    let mut full = body(
        Rarity::Rare,
        vec![
            rnd("Life", Prefix, 80),
            rnd("Armour", Prefix, 75),
            rnd("Thorns", Prefix, 10),
            rnd("FireRes", Suffix, 70),
            rnd("ColdRes", Suffix, 20),
            rnd("Strength", Suffix, 55),
        ],
    );
    full.mods[0].source = Source::Fractured;
    let mut d = Script::below(&[0]);
    match run(&rib, &full, &pool, &mut d) {
        Outcome::Reveal(options) => {
            assert_eq!(d.offered_below, vec![5]);
            for o in &options {
                assert_eq!(o.mods.len(), 6);
                assert!(!families(o).contains(&"Armour"));
                assert_eq!(o.mods.last().unwrap().kind, Prefix);
            }
        }
        other => panic!("{other:?}"),
    }
    // Necromancy on a full item removes on its own side.
    let mut d = Script::below(&[0]);
    match run(&rib.clone().with(Omen::DextralNecromancy), &full, &pool, &mut d) {
        Outcome::Reveal(options) => {
            assert_eq!(d.offered_below, vec![3]);
            assert!(options.iter().all(|o| !families(o).contains(&"FireRes") && o.mods.last().unwrap().kind == Suffix));
        }
        other => panic!("{other:?}"),
    }

    // Abyssal Echoes: one reroll of the options from any first option.
    let echoes = rib.clone().with(Omen::AbyssalEchoes);
    let spec = live(&echoes);
    let reroll = spec.reroll.expect("Abyssal Echoes allows a reroll");
    let first = match run(&echoes, &armour, &pool, &mut Script::default()) {
        Outcome::Reveal(o) => o,
        other => panic!("{other:?}"),
    };
    match reroll(&echoes, &first[0], &pool, &mut Script::picks(&[5, 4, 3])) {
        Outcome::Reveal(again) => {
            assert_eq!(again.len(), 3);
            for o in &again {
                assert_eq!(o.mods.len(), 3);
                assert_eq!(&families(o)[..2], &["Life", "FireRes"]);
            }
        }
        other => panic!("{other:?}"),
    }
    // A reroll without the omen is refused.
    assert!(!refused(reroll(&rib, &first[0], &pool, &mut Script::default())).is_empty());
}

// ---------------------------------------------------------------- L3

#[test]
fn an_action_lists_its_assumptions_verbatim() {
    assert_eq!(A_TRANSMUTE_SIDE, "Transmute picks prefix or suffix 50/50");
    assert_eq!(A_CHAOS_REFILL, "Chaos re-fills either side");
    assert_eq!(A_OMEN_CONSUMED, "an omen whose condition cannot be met is assumed consumed");
    assert_eq!(A_WHITTLING_TIES, "Whittling ties break uniformly");
    assert_eq!(A_FLOOR_FALLBACK, "the Min-Modifier-Level fallback keeps a family's highest tier rollable");

    let has = |a: &Action, text: &str| live(a).assumptions.contains(&text);
    assert!(has(&orb(Orb::Transmutation), A_TRANSMUTE_SIDE));
    assert!(!has(&orb(Orb::Transmutation), A_FLOOR_FALLBACK));
    assert!(has(&Action::graded(Orb::Transmutation, Grade::Greater), A_FLOOR_FALLBACK));
    assert!(has(&orb(Orb::Chaos), A_CHAOS_REFILL));
    assert!(!has(&orb(Orb::Chaos), A_WHITTLING_TIES));
    assert!(has(&orb(Orb::Chaos).with(Omen::Whittling), A_WHITTLING_TIES));
    assert!(has(&orb(Orb::Chaos).with(Omen::Whittling), A_OMEN_CONSUMED));
    assert!(has(&orb(Orb::Exalted).with(Omen::GreaterExaltation), A_OMEN_CONSUMED));
    assert!(!has(&orb(Orb::Exalted), A_OMEN_CONSUMED));
    assert!(has(&Action::graded(Orb::Exalted, Grade::Perfect), A_FLOOR_FALLBACK));
    assert!(has(&Action::bone(BoneKind::Rib, BoneGrade::Ancient), A_FLOOR_FALLBACK));

    // Every live row's assumptions are nonblank, never repeated, and no
    // user-facing text of the table uses the words this tool avoids.
    for a in all_rows() {
        let text = match rule(&a) {
            Rule::Live(spec) => {
                let mut seen = spec.assumptions.clone();
                seen.sort();
                seen.dedup();
                assert_eq!(seen.len(), spec.assumptions.len(), "{}", a.name());
                assert!(spec.assumptions.iter().all(|s| !s.trim().is_empty()));
                spec.assumptions.join(" ")
            }
            Rule::Unavailable(r) | Rule::Unknown(r) => r,
        };
        let lower = text.to_lowercase();
        assert!(!lower.contains("estimate") && !lower.contains("prediction"), "{}: {text}", a.name());
    }
}

// ---------------------------------------------------------------- alloys

fn craft_data() -> &'static khaloni_poe2_core::craft::data::CraftData {
    static DATA: std::sync::OnceLock<khaloni_poe2_core::craft::data::CraftData> = std::sync::OnceLock::new();
    DATA.get_or_init(|| {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let read = |p: std::path::PathBuf| std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        khaloni_poe2_core::craft::data::CraftData::load(
            &read(dir.join("craft/mods_slice.json")),
            &read(dir.join("craft_base_items_sample.json")),
            &read(dir.join("craft/essences_slice.json")),
        )
        .expect("the craft fixtures load")
    })
}

/// A rare of `base` from the base sample holding `mods`.
fn rare_of(base: &str, mods: Vec<ModOn>) -> ItemState {
    let b = craft_data().base(base).unwrap_or_else(|| panic!("{base} is in the base sample"));
    ItemState {
        class: b.class.clone(),
        base: b.name.clone(),
        base_tags: b.tags.clone(),
        item_level: 82,
        rarity: Rarity::Rare,
        corrupted: false,
        mirrored: false,
        sanctified: false,
        mods,
        sockets: 0,
    }
}

/// The first entry that rolls on `base` as a `kind` and shares no group
/// with `avoid`, as a random modifier.
fn rolled_on(base: &str, kind: AffixKind, avoid: &[&str]) -> ModOn {
    let empty = rare_of(base, vec![]);
    craft_data()
        .eligible(&empty, kind, 0)
        .into_iter()
        .find(|c| !c.groups.iter().any(|g| avoid.contains(&g.as_str())))
        .unwrap_or_else(|| panic!("something rolls on {base}"))
        .to_mod(Source::Random)
}

#[test]
fn an_alloy_replaces_a_modifier_with_its_guaranteed_one_per_class() {
    let data = craft_data();
    // The rules list the table's Alloys, in its order.
    let names: Vec<&str> = data.alloys().iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ALLOYS.to_vec());
    let adaptive = Action::alloy("Adaptive Alloy");
    let spec = live(&adaptive);
    assert_eq!(spec.source, Confidence::Game);
    assert!(spec.assumptions.contains(&A_ALLOY_REMOVAL));

    // On a wand: the one-handed fire line, "Gain (21-26)% of Damage as
    // Extra Fire Damage while you are missing Runic Ward", in the crafted
    // slot, in place of the item's only modifier.
    let before = rolled_on("Attuned Wand", Suffix, &[]);
    let wand = rare_of("Attuned Wand", vec![before.clone()]);
    let after = applied(attempt(&adaptive, &wand, data, &mut Script::default()));
    assert!(!after.mods.contains(&before), "the modifier the Alloy removed is gone");
    let [added] = after.mods.as_slice() else { panic!("{:?}", after.mods) };
    assert_eq!(added.entry_id.as_deref(), Some("AlloyDamageAsExtraFireWhileMissingRunicWard1"));
    assert_eq!(added.source, Source::Crafted);
    assert_eq!(added.kind, Prefix);

    // On gloves: "(10-15)% increased Attack Speed while missing Runic
    // Ward", a suffix; the removal is one of the two modifiers at random.
    let gloves = rare_of(
        "Stocky Mitts",
        vec![rolled_on("Stocky Mitts", Prefix, &[]), rolled_on("Stocky Mitts", Suffix, &["IncreasedAttackSpeed"])],
    );
    for pick in [0, 1] {
        let after = applied(attempt(&adaptive, &gloves, data, &mut Script::below(&[pick])));
        assert_eq!(after.mods.len(), 2);
        assert!(!after.mods.contains(&gloves.mods[pick]));
        let crafted: Vec<&ModOn> = after.mods.iter().filter(|m| m.source == Source::Crafted).collect();
        assert_eq!(crafted.len(), 1);
        assert_eq!(crafted[0].entry_id.as_deref(), Some("AlloyAttackSpeedIfMissingWardRecently1"));
    }

    // On a sceptre the same Alloy guarantees the Puppet Master line.
    let sceptre = data.base("Rattling Sceptre").expect("a sceptre in the sample");
    match data.alloy_outcome("Adaptive Alloy", sceptre) {
        EssenceOutcome::Entry(c) => assert_eq!(c.entry_id, "AlloyPuppetMasterChance1"),
        other => panic!("{other:?}"),
    }

    // A class the Alloy has no line for is refused with the reason, and
    // nothing is removed on the way.
    let ring = rare_of("Iron Ring", vec![rolled_on("Iron Ring", Suffix, &[])]);
    let why = refused(attempt(&adaptive, &ring, data, &mut Script::default()));
    assert_eq!(why, "unknown: Adaptive Alloy has no modifier for Ring");

    // The crafted slot taken, a non-rare, or an Alloy not in the table:
    // refused before any draw.
    let mut crafted = wand.clone();
    crafted.mods[0].source = Source::Crafted;
    assert!(refused(attempt(&adaptive, &crafted, data, &mut Script::default())).contains("one crafted modifier"));
    let mut magic = wand.clone();
    magic.rarity = Rarity::Magic;
    assert_eq!(refused(attempt(&adaptive, &magic, data, &mut Script::default())), "needs a Rare item");
    assert!(refused(attempt(&Action::alloy("Gilded Alloy"), &wand, data, &mut Script::default())).starts_with("unknown: "));

    // Every line of every Alloy joins to the mod database's own Alloy
    // entry on each class of the base sample it names; the others say the
    // Alloy has nothing for them.
    let mut joined = 0;
    for alloy in data.alloys() {
        let mut seen = std::collections::HashSet::new();
        for base in data.bases() {
            if !seen.insert(base.class.clone()) {
                continue;
            }
            match data.alloy_outcome(&alloy.name, base) {
                EssenceOutcome::Entry(c) => {
                    assert!(c.entry_id.starts_with("Alloy"), "{} on {}: {}", alloy.name, base.class, c.entry_id);
                    joined += 1;
                }
                EssenceOutcome::Unknown(why) => {
                    assert_eq!(why, format!("{} has no modifier for {}", alloy.name, base.class))
                }
            }
        }
    }
    assert!(joined >= 30, "only {joined} Alloy lines joined on the sample's classes");
}
