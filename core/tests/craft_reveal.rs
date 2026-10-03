//! An amulet that already carries an unrevealed desecrated suffix: the
//! planner reveals it first (free at the Well of Souls, three options, one
//! reroll under Omen of Abyssal Echoes) before any new bone, and a target
//! with a desecrated modifier and a rolled one gets a costed plan.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use khaloni_poe2_core::craft::data::{CraftData, Domain};
use khaloni_poe2_core::craft::plan::plan;
use khaloni_poe2_core::craft::rules::{attempt, rule, Action, Omen, Rule, REVEAL_NAME};
use khaloni_poe2_core::craft::sim::SimConfig;
use khaloni_poe2_core::craft::strategy::{Target, Want};
use khaloni_poe2_core::craft::types::{AffixKind, Draw, ItemState, ModOn, Outcome, Rarity, Source};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn data() -> CraftData {
    let dir = fixtures();
    CraftData::load(
        &read(&dir.join("craft/mods_slice.json")),
        &read(&dir.join("craft_base_items_sample.json")),
        &read(&dir.join("craft/essences_slice.json")),
    )
    .expect("the craft fixtures load")
}

/// The best entry of `family` the amulet rolls at `ilvl`, as a modifier.
fn rolled(data: &CraftData, tags: &[&str], family: &str, ilvl: u32) -> ModOn {
    let e = data
        .entries()
        .iter()
        .filter(|e| e.domain == Domain::Item && e.family == family && e.eligible(tags.iter().copied()) && e.required_level <= ilvl)
        .max_by_key(|e| e.required_level)
        .unwrap_or_else(|| panic!("{family} rolls on an amulet"));
    data.candidate(e, tags).to_mod(Source::Random)
}

fn unrevealed_suffix() -> ModOn {
    ModOn {
        entry_id: None,
        family: "Desecrated Suffix".into(),
        groups: Vec::new(),
        kind: AffixKind::Suffix,
        tier: None,
        required_level: None,
        adds_tags: Vec::new(),
        source: Source::Desecrated,
        text: "Desecrated Suffix".into(),
    }
}

fn amulet(data: &CraftData) -> ItemState {
    let base = data.base("Absent Amulet").expect("Absent Amulet is in the base sample");
    let tags: Vec<&str> = base.tags.iter().map(String::as_str).collect();
    ItemState {
        class: base.class.clone(),
        base: base.name.clone(),
        base_tags: base.tags.clone(),
        item_level: 80,
        rarity: Rarity::Rare,
        corrupted: false,
        mirrored: false,
        sanctified: false,
        mods: vec![
            rolled(data, &tags, "BaseSpirit", 80),
            rolled(data, &tags, "LightningResistance", 80),
            unrevealed_suffix(),
        ],
        sockets: 0,
    }
}

struct First;
impl Draw for First {
    fn below(&mut self, _: usize) -> usize {
        0
    }
    fn pick(&mut self, candidates: &[khaloni_poe2_core::craft::types::Candidate]) -> Option<usize> {
        (!candidates.is_empty()).then_some(0)
    }
}

#[test]
fn an_unrevealed_desecrated_modifier_reveals_into_three_options_of_its_side() {
    let data = data();
    let item = amulet(&data);
    assert!(item.mods[2].unrevealed());
    assert!(item.desecrated_slot_used(), "the unrevealed modifier holds the one desecrated slot");
    let reveal = Action::Reveal { omens: vec![] };
    assert_eq!(reveal.name(), REVEAL_NAME);
    let Rule::Live(spec) = rule(&reveal) else { panic!("the reveal is a live rule") };
    assert!(spec.assumptions.iter().any(|a| a.contains("any lich")));
    match attempt(&reveal, &item, &data, &mut First) {
        Outcome::Reveal(options) => {
            assert_eq!(options.len(), 3);
            for o in &options {
                let d: Vec<&ModOn> = o.mods.iter().filter(|m| m.source == Source::Desecrated).collect();
                assert_eq!(d.len(), 1, "one desecrated modifier per option");
                assert!(!d[0].unrevealed(), "revealed");
                assert_eq!(d[0].kind, AffixKind::Suffix, "on the unrevealed one's side");
                assert_eq!(o.mods.len(), item.mods.len());
            }
        }
        other => panic!("a reveal gives options, got {other:?}"),
    }
    // Only Abyssal Echoes acts at a reveal; the others act on the bone.
    assert!(matches!(rule(&Action::Reveal { omens: vec![Omen::AbyssalEchoes] }), Rule::Live(s) if s.reroll.is_some()));
    assert!(matches!(rule(&Action::Reveal { omens: vec![Omen::Blackblooded] }), Rule::Unavailable(_)));
    // Nothing to reveal: refused.
    let mut plain = item.clone();
    plain.mods.pop();
    assert!(matches!(attempt(&reveal, &plain, &data, &mut First), Outcome::Refused(_)));
}

#[test]
fn a_plan_reveals_the_unrevealed_suffix_before_any_bone() {
    let data = data();
    let item = amulet(&data);
    let target = Target {
        wants: vec![
            Want { family: "GlobalIncreaseProjectileSkillGemLevel".into(), kind: AffixKind::Suffix, min_tier: 1 },
            Want { family: "GlobalSkillGemQuality".into(), kind: AffixKind::Suffix, min_tier: 1 },
        ],
    };
    let prices: HashMap<&str, f64> = [
        ("Exalted Orb", 1.0),
        ("Greater Exalted Orb", 3.5),
        ("Perfect Exalted Orb", 16.0),
        ("Orb of Annulment", 6.0),
        ("Chaos Orb", 1.3),
        ("Omen of Sinistral Exaltation", 9.0),
        ("Omen of Dextral Exaltation", 11.0),
        ("Omen of Sinistral Annulment", 7.0),
        ("Omen of Dextral Annulment", 8.0),
        ("Omen of Light", 4.0),
        ("Omen of Abyssal Echoes", 10.0),
        ("Omen of Dextral Necromancy", 6.0),
        ("Omen of Sinistral Necromancy", 6.0),
        ("Omen of the Blackblooded", 12.0),
        ("Omen of the Liege", 12.0),
        ("Omen of the Sovereign", 12.0),
        ("Preserved Collarbone", 2.0),
        ("Ancient Collarbone", 9.0),
        ("Gnawed Collarbone", 1.0),
    ]
    .into_iter()
    .collect();
    let price = |n: &str| prices.get(n).copied();
    let p = plan(&item, &target, &data, None, &price, None, &SimConfig { runs: 300, seed: 11, ..SimConfig::default() });
    let desecrate: Vec<_> = p.strategies.iter().filter(|r| r.costed.id == "desecrate").collect();
    assert!(!desecrate.is_empty(), "a desecrate plan: {:?}", p.strategies.iter().map(|r| r.costed.strategy.clone()).collect::<Vec<_>>());
    for r in &desecrate {
        let first = r.costed.steps.first().expect("steps");
        assert!(first.action.name().starts_with(REVEAL_NAME), "{} starts with {}", r.costed.strategy, first.action.name());
    }
    assert!(desecrate.iter().any(|r| r.costed.per_finished.is_some()), "a costed desecrate plan: {:?}", desecrate.iter().map(|r| r.costed.unknowns.clone()).collect::<Vec<_>>());
}
