//! The strategy library and plans on real data (the craft fixtures): each
//! strategy applies exactly where the spec's table says, buying is always
//! the baseline, Greater and Perfect orbs are tried where their floor
//! helps, and five golden plans on real bases hold their ranking and steps.
//!
//! The golden files under `fixtures/craft_golden/` are written by running
//! this test with `CRAFT_GOLDEN_WRITE=1`; every write is read against the
//! rulebook before it is kept.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use khaloni_poe2_core::craft::data::CraftData;
use khaloni_poe2_core::craft::model::Model;
use khaloni_poe2_core::craft::plan::{plan, BuyQuote, Plan};
use khaloni_poe2_core::craft::rules::{Action, BoneGrade, BoneKind, EssenceTier, Grade, Omen, Orb, CORRUPTED_ESSENCES, TIERED_ESSENCES};
use khaloni_poe2_core::craft::sim::SimConfig;
use khaloni_poe2_core::craft::strategy::{applicable, library, Next, Strategy, StrategyId, Target, Want};
use khaloni_poe2_core::craft::types::{AffixKind, ItemState, ModOn, Rarity, Source};
use serde_json::{json, Value};

use AffixKind::{Prefix, Suffix};

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

fn item(base: &str, ilvl: u32, rarity: Rarity, mods: Vec<ModOn>) -> ItemState {
    let b = craft().base(base).unwrap_or_else(|| panic!("{base} is in the base sample"));
    ItemState {
        class: b.class.clone(),
        base: b.name.clone(),
        base_tags: b.tags.clone(),
        item_level: ilvl,
        rarity,
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

fn want(family: &str, kind: AffixKind, min_tier: u8) -> Want {
    Want { family: family.into(), kind, min_tier }
}

/// A fixed price snapshot, in Exalted Orbs, for every item a plan here can
/// use.
fn prices() -> HashMap<String, f64> {
    let mut table: HashMap<String, f64> = [
        ("Orb of Transmutation", 0.05),
        ("Greater Orb of Transmutation", 0.6),
        ("Perfect Orb of Transmutation", 4.0),
        ("Orb of Augmentation", 0.08),
        ("Greater Orb of Augmentation", 0.7),
        ("Perfect Orb of Augmentation", 4.5),
        ("Regal Orb", 0.4),
        ("Greater Regal Orb", 2.0),
        ("Perfect Regal Orb", 9.0),
        ("Exalted Orb", 1.0),
        ("Greater Exalted Orb", 3.5),
        ("Perfect Exalted Orb", 16.0),
        ("Chaos Orb", 1.3),
        ("Greater Chaos Orb", 4.0),
        ("Perfect Chaos Orb", 18.0),
        ("Orb of Annulment", 6.0),
        ("Orb of Alchemy", 0.3),
        ("Fracturing Orb", 30.0),
        ("Omen of Sinistral Exaltation", 9.0),
        ("Omen of Dextral Exaltation", 11.0),
        ("Omen of Sinistral Annulment", 7.0),
        ("Omen of Dextral Annulment", 8.0),
        ("Omen of Sinistral Erasure", 3.0),
        ("Omen of Dextral Erasure", 3.5),
        ("Omen of Whittling", 25.0),
        ("Omen of Light", 4.0),
        ("Omen of Sinistral Crystallisation", 5.0),
        ("Omen of Dextral Crystallisation", 5.0),
        ("Omen of Sinistral Necromancy", 6.0),
        ("Omen of Dextral Necromancy", 6.0),
        ("Omen of Abyssal Echoes", 10.0),
        ("Omen of the Sovereign", 12.0),
        ("Omen of the Liege", 12.0),
        ("Omen of the Blackblooded", 12.0),
        ("Preserved Collarbone", 2.0),
        ("Preserved Rib", 2.0),
        ("Preserved Jawbone", 2.0),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    for of in TIERED_ESSENCES {
        table.insert(format!("Lesser Essence of {of}"), 0.2);
        table.insert(format!("Essence of {of}"), 0.8);
        table.insert(format!("Greater Essence of {of}"), 3.0);
        table.insert(format!("Perfect Essence of {of}"), 45.0);
    }
    for of in CORRUPTED_ESSENCES {
        table.insert(format!("Essence of {of}"), 25.0);
    }
    table
}

fn ids(strategies: &[Strategy]) -> Vec<StrategyId> {
    let mut out: Vec<StrategyId> = strategies.iter().map(Strategy::id).collect();
    out.dedup();
    out
}

fn applying(state: &ItemState, target: &Target) -> Vec<StrategyId> {
    StrategyId::ALL.into_iter().filter(|id| applicable(*id, state, target, craft())).collect()
}

fn life_and_resists() -> Target {
    Target {
        wants: vec![want("IncreasedLife", Prefix, 3), want("FireResistance", Suffix, 2), want("ColdResistance", Suffix, 2)],
    }
}

// ---------------------------------------------------------------- L4

#[test]
fn each_strategy_applies_only_where_the_spec_says() {
    use StrategyId::*;
    let cuirass = item("Soldier Cuirass", 80, Rarity::Normal, vec![]);

    // A normal base with three wanted families: orb-chain, an essence for
    // the fire resistance (Greater Essence of Insulation adds +(31-35)%,
    // tier 3 on armour), alchemy for the many targets, buying.
    let three = Target {
        wants: vec![want("IncreasedLife", Prefix, 3), want("FireResistance", Suffix, 3), want("ColdResistance", Suffix, 2)],
    };
    assert_eq!(applying(&cuirass, &three), vec![OrbChain, EssenceStart, AlchemyStart, Buy]);

    // No essence reaches life T3 or the resistances at T2 (Greater Essence
    // of the Body adds +(100-119), tier 6): no essence-start.
    assert_eq!(applying(&cuirass, &life_and_resists()), vec![OrbChain, AlchemyStart, Buy]);

    // Two families are not "many": no alchemy-start.
    let two = Target { wants: vec![want("FireResistance", Suffix, 3), want("ColdResistance", Suffix, 2)] };
    assert_eq!(applying(&cuirass, &two), vec![OrbChain, EssenceStart, Buy]);

    // A family no essence gives, on a normal item: no essence-start.
    let thorns = Target { wants: vec![want("ThornsPhysicalDamage", Prefix, 2)] };
    assert_eq!(applying(&cuirass, &thorns), vec![OrbChain, Buy]);

    // A corrupted item takes no crafting: only buying.
    let mut corrupted = cuirass.clone();
    corrupted.corrupted = true;
    assert_eq!(applying(&corrupted, &life_and_resists()), vec![Buy]);
    let mut mirrored = cuirass.clone();
    mirrored.mirrored = true;
    assert_eq!(applying(&mirrored, &life_and_resists()), vec![Buy]);

    // A rare with three wanted modifiers and unwanted ones: chaos-spam and
    // the orb-chain's rare phase; no essence or alchemy start.
    let b = "Soldier Cuirass";
    let rare = item(
        b,
        82,
        Rarity::Rare,
        vec![
            on("IncreasedLife13", b),
            on("LocalIncreasedPhysicalDamageReductionRating10", b),
            on("FireResist7", b),
            on("Strength2", b),
            on("StunThreshold3", b),
        ],
    );
    let four = Target {
        wants: vec![
            want("IncreasedLife", Prefix, 1),
            want("LocalPhysicalDamageReductionRating", Prefix, 2),
            want("FireResistance", Suffix, 3),
            want("ColdResistance", Suffix, 3),
        ],
    };
    assert_eq!(applying(&rare, &four), vec![OrbChain, ChaosSpam, Buy]);
    // Two wanted modifiers present are not enough for chaos-spam.
    let mut fewer = rare.clone();
    fewer.mods.retain(|m| m.family != "FireResistance");
    assert!(!applying(&fewer, &four).contains(&ChaosSpam));

    // A rare with exactly one bad modifier and a Perfect essence for what
    // is missing (Perfect Essence of the Body adds maximum Life percent on
    // body armour): perfect-essence-fix. Nothing random rolls that family,
    // so the random strategies are out.
    let fix = item(
        b,
        82,
        Rarity::Rare,
        vec![
            on("IncreasedLife13", b),
            on("LocalIncreasedPhysicalDamageReductionRating10", b),
            on("FireResist7", b),
            on("ColdResist7", b),
            on("Strength2", b),
        ],
    );
    let with_percent = Target {
        wants: vec![
            want("IncreasedLife", Prefix, 1),
            want("LocalPhysicalDamageReductionRating", Prefix, 2),
            want("FireResistance", Suffix, 3),
            want("ColdResistance", Suffix, 3),
            want("MaximumLifeIncreasePercent", Prefix, 1),
        ],
    };
    assert_eq!(applying(&fix, &with_percent), vec![PerfectEssenceFix, Buy]);
    // Two bad modifiers: not a one-modifier fix.
    let mut two_bad = fix.clone();
    two_bad.mods.push(on("StunThreshold3", b));
    assert!(!applying(&two_bad, &with_percent).contains(&PerfectEssenceFix));
    // The crafted slot already used: no essence can go on.
    let mut crafted = fix.clone();
    crafted.mods[4].source = Source::Crafted;
    assert!(!applying(&crafted, &with_percent).contains(&PerfectEssenceFix));

    // A 4+ modifier rare whose only wanted modifier is a tier 1 and the
    // rest bad: fracture-then-chaos.
    let lone = item(
        b,
        82,
        Rarity::Rare,
        vec![on("IncreasedLife13", b), on("Strength2", b), on("StunThreshold3", b), on("LifeRegeneration3", b)],
    );
    assert_eq!(on("IncreasedLife13", b).tier, Some(1));
    let fracture_target = Target { wants: vec![want("IncreasedLife", Prefix, 1), want("FireResistance", Suffix, 2)] };
    assert_eq!(applying(&lone, &fracture_target), vec![OrbChain, FractureThenChaos, Buy]);
    // Its wanted modifier at tier 2 instead: not a T1 to lock.
    let mut t2 = lone.clone();
    t2.mods[0] = on("IncreasedLife12", b);
    let loose = Target { wants: vec![want("IncreasedLife", Prefix, 2), want("FireResistance", Suffix, 2)] };
    assert!(!applying(&t2, &loose).contains(&FractureThenChaos));
    // Only three modifiers: the Fracturing Orb needs four.
    let mut three = lone.clone();
    three.mods.pop();
    assert!(!applying(&three, &fracture_target).contains(&FractureThenChaos));

    // A wanted family only desecration adds (skill speed on a ring, an
    // Ulaman modifier): desecrate. On a normal ring the orb-chain cannot
    // add it, so only desecrate and buying apply.
    let ring = item("Gold Ring", 82, Rarity::Normal, vec![]);
    let speed = Target { wants: vec![want("IncreasedSkillSpeed", Suffix, 1), want("FireResistance", Suffix, 2)] };
    assert_eq!(applying(&ring, &speed), vec![Desecrate, Buy]);
    let bones = library(&ring, &speed, craft(), &Model::Uniform);
    let Some(Strategy::Desecrate { bone }) = bones.first() else { panic!("{bones:?}") };
    assert_eq!(
        bone,
        &Action::bone(BoneKind::Collarbone, BoneGrade::Preserved)
            .with(Omen::DextralNecromancy)
            .with(Omen::Sovereign)
            .with(Omen::AbyssalEchoes)
    );
    // An unwanted desecrated modifier already in the one slot: desecrate
    // still applies, and its first step clears the slot with Omen of Light.
    let mut used = item("Gold Ring", 82, Rarity::Rare, vec![on("FireResist8", "Gold Ring")]);
    let mut dese = on("AbyssModRingAmuletKurgalSuffixCooldownRecoveryRate", "Gold Ring");
    dese.source = Source::Desecrated;
    used.mods.push(dese);
    assert!(applying(&used, &speed).contains(&Desecrate));
    let strategy = library(&used, &speed, craft(), &Model::Uniform).into_iter().find(|s| s.id() == Desecrate).unwrap();
    assert_eq!(strategy.next(&used, &speed), Next::Act(Action::orb(Orb::Annulment).with(Omen::Light)));

    // Buying applies to anything.
    for s in [&cuirass, &corrupted, &rare, &fix, &lone, &ring] {
        assert!(applicable(Buy, s, &life_and_resists(), craft()));
    }
}

#[test]
fn buy_is_always_the_baseline_and_a_dearer_plan_is_marked() {
    let table = prices();
    let price = |name: &str| table.get(name).copied();
    let start = item("Attuned Wand", 81, Rarity::Normal, vec![]);
    let target = Target { wants: vec![want("WeaponSpellDamage", Prefix, 4), want("IncreasedCastSpeed", Suffix, 4)] };
    let cfg = SimConfig { runs: 400, ..SimConfig::default() };

    // Without a trade search the buy line is there and says its price is
    // unknown; nothing is marked against a price nobody has.
    let blind = plan(&start, &target, craft(), None, &price, None, &cfg);
    assert_eq!(blind.buy.price, None);
    assert!(blind.buy.unknown.as_deref().is_some_and(|u| u.starts_with("unknown: ")));
    assert!(blind.unknowns.contains(blind.buy.unknown.as_ref().unwrap()));
    assert!(blind.strategies.iter().all(|r| !r.not_worth_crafting));
    assert!(blind.strategies.iter().all(|r| r.costed.id != "buy"), "buying is the baseline line, not a ranked craft");
    let best = blind.strategies[0].costed.per_finished.expect("the cheapest craft has a total");

    // A listing cheaper than every craft: all of them are marked, and the
    // cheapest way is to buy.
    let cheap = plan(&start, &target, craft(), None, &price, Some(BuyQuote { price: best / 2.0, listings: 3 }), &cfg);
    assert_eq!(cheap.buy.price, Some(best / 2.0));
    assert_eq!(cheap.buy.listings, 3);
    assert_eq!(cheap.buy.unknown, None);
    assert!(!cheap.strategies.is_empty());
    assert!(cheap.strategies.iter().all(|r| r.not_worth_crafting || r.costed.per_finished.is_none()));
    assert_eq!(cheap.cheapest.as_deref(), Some("buy"));

    // A listing above the cheapest craft: that craft is the cheapest way
    // and unmarked, and exactly the crafts dearer than the listing carry
    // the mark.
    let quote = best * 1.5;
    let dear = plan(&start, &target, craft(), None, &price, Some(BuyQuote { price: quote, listings: 2 }), &cfg);
    let first = &dear.strategies[0];
    assert!(!first.not_worth_crafting);
    assert_eq!(dear.cheapest.as_deref(), Some(first.costed.strategy.as_str()));
    for r in &dear.strategies {
        assert_eq!(r.not_worth_crafting, r.costed.per_finished.is_some_and(|e| e > quote), "{}", r.costed.strategy);
    }
    assert!(dear.strategies.iter().any(|r| r.not_worth_crafting), "some craft is dearer than half again the cheapest");

    // A corrupted item: nothing to craft, and buying is still the line.
    let mut corrupted = start.clone();
    corrupted.corrupted = true;
    let locked = plan(&corrupted, &target, craft(), None, &price, Some(BuyQuote { price: 9.0, listings: 1 }), &cfg);
    assert!(locked.strategies.is_empty());
    assert_eq!(locked.cheapest.as_deref(), Some("buy"));
}

#[test]
fn greater_and_perfect_variants_are_explored_where_a_floor_helps() {
    let cuirass = item("Soldier Cuirass", 80, Rarity::Normal, vec![]);
    // Top tiers only (life 75+, resistances 60+): Greater (35) and Perfect
    // (50) Regal and Exalted remove only tiers the target does not want,
    // and Greater (44) and Perfect (70) Transmutation too.
    let strict = Target {
        wants: vec![want("IncreasedLife", Prefix, 2), want("FireResistance", Suffix, 2), want("ColdResistance", Suffix, 2)],
    };
    let variants: Vec<Strategy> =
        library(&cuirass, &strict, craft(), &Model::Uniform).into_iter().filter(|s| matches!(s, Strategy::OrbChain { .. })).collect();
    for rare in [Grade::Normal, Grade::Greater, Grade::Perfect] {
        assert!(variants.contains(&Strategy::OrbChain { magic: Grade::Normal, rare, clear: false }), "{rare:?} in {variants:?}");
    }
    assert!(variants.contains(&Strategy::OrbChain { magic: Grade::Greater, rare: Grade::Normal, clear: false }));
    // Each grade is tried on its own steps, the other steps plain.
    assert!(!variants.iter().any(|v| matches!(v, Strategy::OrbChain { magic, rare, .. } if *magic != Grade::Normal && *rare != Grade::Normal)));
    assert_eq!(
        Strategy::OrbChain { magic: Grade::Greater, rare: Grade::Perfect, clear: false }.name(),
        "orb-chain (Greater Transmutation and Augmentation, Perfect Regal and Exalted)"
    );

    // A target that wants low tiers too (any life, any fire resistance):
    // the floors take away wanted tiers and help nothing, so only the
    // plain orbs are tried.
    let loose = Target { wants: vec![want("IncreasedLife", Prefix, 13), want("FireResistance", Suffix, 7)] };
    let plain: Vec<Strategy> =
        library(&cuirass, &loose, craft(), &Model::Uniform).into_iter().filter(|s| matches!(s, Strategy::OrbChain { .. })).collect();
    assert_eq!(plain, vec![Strategy::OrbChain { magic: Grade::Normal, rare: Grade::Normal, clear: false }]);

    // The same holds for chaos-spam's Chaos Orb and alchemy's slams.
    let alchemy: Vec<Strategy> =
        library(&cuirass, &strict, craft(), &Model::Uniform).into_iter().filter(|s| matches!(s, Strategy::AlchemyStart { .. })).collect();
    assert!(alchemy.len() > 1, "{alchemy:?}");
}

#[test]
fn a_target_an_alloy_adds_is_planned_through_the_alloy() {
    use khaloni_poe2_core::craft::types::PoolView;
    let b = "Stocky Mitts";
    let empty = item(b, 82, Rarity::Rare, vec![]);
    // A rare pair of gloves with a wanted prefix and an unwanted suffix,
    // and a target that also wants the attack speed Adaptive Alloy
    // guarantees on gloves: no orb rolls that family, so only the Alloy
    // (and buying) reaches the target.
    let prefix = craft().eligible(&empty, Prefix, 0).into_iter().next().expect("a prefix rolls on gloves");
    let suffix = craft()
        .eligible(&empty, Suffix, 0)
        .into_iter()
        .find(|c| !c.groups.iter().any(|g| g == "IncreasedAttackSpeed"))
        .expect("a suffix rolls on gloves");
    let start = item(b, 82, Rarity::Rare, vec![prefix.to_mod(Source::Random), suffix.to_mod(Source::Random)]);
    let target = Target {
        wants: vec![want(&prefix.family, Prefix, prefix.tier), want("AttackSpeedWhileMissingRunicWard", Suffix, 1)],
    };
    assert_eq!(applying(&start, &target), vec![StrategyId::AlloyFix, StrategyId::Buy]);
    let fix = library(&start, &target, craft(), &Model::Uniform);
    assert_eq!(fix[0], Strategy::AlloyFix { alloy: Action::alloy("Adaptive Alloy"), kind: Suffix });
    assert_eq!(fix[0].name(), "alloy-fix (Adaptive Alloy)");
    assert_eq!(fix[0].next(&start, &target), Next::Act(Action::alloy("Adaptive Alloy")));
    // The crafted slot already taken: no Alloy can go on.
    let mut crafted = start.clone();
    crafted.mods[1].source = Source::Crafted;
    assert!(!applying(&crafted, &target).contains(&StrategyId::AlloyFix));

    // With prices the plan costs it: the Alloy, then slams back whatever
    // wanted modifier its removal took (and the Alloy again when an
    // Annulment takes its modifier).
    let mut table = prices();
    table.insert("Adaptive Alloy".to_string(), 20.0);
    let price = |name: &str| table.get(name).copied();
    let cfg = SimConfig { runs: 400, ..SimConfig::default() };
    let p = plan(&start, &target, craft(), None, &price, None, &cfg);
    let row = p.strategies.iter().find(|r| r.costed.id == "alloy-fix").expect("an alloy-fix plan");
    let total = row.costed.per_finished.expect("the alloy-fix plan has a total");
    assert!(total >= 20.0, "the Alloy alone costs 20: {total}");
    assert!(row.costed.currency().iter().any(|(name, n)| name == "Adaptive Alloy" && *n >= 1.0), "{:?}", row.costed.currency());
    assert!(row.costed.unknowns.is_empty(), "{:?}", row.costed.unknowns);
}

// ---------------------------------------------------------------- golden plans

struct Case {
    name: &'static str,
    start: ItemState,
    target: Target,
    buy: BuyQuote,
}

fn cases() -> Vec<Case> {
    let b = "Soldier Cuirass";
    vec![
        Case {
            name: "soldier-cuirass-life-two-resists",
            start: item(b, 80, Rarity::Normal, vec![]),
            target: Target {
                wants: vec![want("IncreasedLife", Prefix, 3), want("FireResistance", Suffix, 3), want("ColdResistance", Suffix, 3)],
            },
            buy: BuyQuote { price: 250.0, listings: 3 },
        },
        Case {
            name: "gold-ring-fire-cold",
            start: item("Gold Ring", 82, Rarity::Normal, vec![]),
            target: Target { wants: vec![want("FireResistance", Suffix, 2), want("ColdResistance", Suffix, 2)] },
            buy: BuyQuote { price: 60.0, listings: 4 },
        },
        Case {
            name: "attuned-wand-spell-cast-essence",
            start: item("Attuned Wand", 81, Rarity::Normal, vec![]),
            target: Target { wants: vec![want("WeaponSpellDamage", Prefix, 3), want("IncreasedCastSpeed", Suffix, 3)] },
            buy: BuyQuote { price: 400.0, listings: 5 },
        },
        Case {
            name: "dunerunner-sandals-movement-lightning",
            start: item("Dunerunner Sandals", 80, Rarity::Normal, vec![]),
            target: Target { wants: vec![want("MovementVelocity", Prefix, 2), want("LightningResistance", Suffix, 3)] },
            buy: BuyQuote { price: 45.0, listings: 3 },
        },
        Case {
            name: "soldier-cuirass-rare-perfect-essence",
            start: item(
                b,
                82,
                Rarity::Rare,
                vec![
                    on("IncreasedLife13", b),
                    on("LocalIncreasedPhysicalDamageReductionRating10", b),
                    on("FireResist7", b),
                    on("ColdResist7", b),
                    on("Strength2", b),
                ],
            ),
            target: Target {
                wants: vec![
                    want("IncreasedLife", Prefix, 1),
                    want("LocalPhysicalDamageReductionRating", Prefix, 2),
                    want("FireResistance", Suffix, 3),
                    want("ColdResistance", Suffix, 3),
                    want("MaximumLifeIncreasePercent", Prefix, 1),
                ],
            },
            buy: BuyQuote { price: 400.0, listings: 2 },
        },
    ]
}

/// Four significant digits: enough to catch a changed plan, few enough to
/// read.
fn round(x: f64) -> Value {
    if x == 0.0 || !x.is_finite() {
        return json!(x);
    }
    let digits = 3 - x.abs().log10().floor() as i32;
    let scale = 10f64.powi(digits);
    json!((x * scale).round() / scale)
}

fn opt(x: Option<f64>) -> Value {
    x.map_or(Value::Null, round)
}

fn golden(case: &Case, p: &Plan) -> Value {
    let wants: Vec<Value> = case
        .target
        .wants
        .iter()
        .map(|w| json!({ "family": w.family, "kind": format!("{:?}", w.kind), "min_tier": w.min_tier }))
        .collect();
    let mods: Vec<Value> = case.start.mods.iter().map(|m| json!(format!("{} ({:?} T{})", m.family, m.kind, m.tier.unwrap_or(0)))).collect();
    let strategies: Vec<Value> = p
        .strategies
        .iter()
        .map(|r| {
            let c = &r.costed;
            let steps: Vec<Value> = c
                .steps
                .iter()
                .map(|s| {
                    json!({
                        "step": s.name,
                        "uses": round(s.expected_tries),
                        "p_success": opt(s.p_success),
                        "cost": opt(s.cost),
                        "closed_form": s.closed_form,
                    })
                })
                .collect();
            json!({
                "strategy": c.strategy,
                "per_finished": opt(c.per_finished),
                "expected": opt(c.expected),
                "median": opt(c.median),
                "p90": opt(c.p90),
                "give_up_rate": round(c.give_up_rate),
                "not_worth_crafting": r.not_worth_crafting,
                "steps": steps,
                "assumptions": c.assumptions,
                "unknowns": c.unknowns,
            })
        })
        .collect();
    json!({
        "case": case.name,
        "base": case.start.base,
        "item_level": case.start.item_level,
        "rarity": format!("{:?}", case.start.rarity),
        "mods": mods,
        "target": wants,
        "patch": p.patch,
        "model_line": p.model_line,
        "buy": { "price": opt(p.buy.price), "listings": p.buy.listings },
        "cheapest": p.cheapest,
        "strategies": strategies,
    })
}

/// Runs per strategy for the golden plans: enough for stable rankings,
/// few enough for a debug test run.
const GOLDEN_RUNS: usize = 500;

#[test]
fn golden_plans_for_five_targets_match() {
    let table = prices();
    let price = |name: &str| table.get(name).copied();
    let dir = fixtures().join("craft_golden");
    let write = std::env::var_os("CRAFT_GOLDEN_WRITE").is_some();
    let cfg = SimConfig { runs: GOLDEN_RUNS, ..SimConfig::default() };
    let all = cases();
    assert_eq!(all.len(), 5);
    let mut essence_start_seen = false;
    for case in &all {
        let p = plan(&case.start, &case.target, craft(), None, &price, Some(case.buy), &cfg);
        let got = golden(case, &p);
        let path = dir.join(format!("{}.json", case.name));
        if write {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&path, serde_json::to_string_pretty(&got).unwrap() + "\n").unwrap();
        }
        let want: Value = serde_json::from_str(&read(&path)).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let names = |v: &Value| -> Vec<String> {
            v["strategies"].as_array().unwrap().iter().map(|s| s["strategy"].as_str().unwrap().to_string()).collect()
        };
        assert_eq!(names(&got), names(&want), "{}: ranking", case.name);
        for (g, w) in got["strategies"].as_array().unwrap().iter().zip(want["strategies"].as_array().unwrap()) {
            let steps = |v: &Value| -> Vec<String> {
                v["steps"].as_array().unwrap().iter().map(|s| s["step"].as_str().unwrap().to_string()).collect()
            };
            assert_eq!(steps(g), steps(w), "{}: steps of {}", case.name, g["strategy"]);
        }
        assert_eq!(got, want, "{}: the whole plan", case.name);
        essence_start_seen |= p.strategies.iter().any(|r| r.costed.id == "essence-start" && r.costed.expected.is_some());
        assert!(p.strategies.iter().any(|r| r.costed.expected.is_some()), "{}: some strategy has a total", case.name);
    }
    assert!(essence_start_seen, "one case is costed by essence-start");
    // The essence the wand case starts from is a real upgrading essence.
    let wand = &all[2];
    let lib = library(&wand.start, &wand.target, craft(), &Model::Uniform);
    assert!(lib.iter().any(|s| matches!(s, Strategy::EssenceStart { essence: Action::Essence { tier, .. }, .. } if *tier != EssenceTier::Perfect)));
    assert_eq!(ids(&lib).last(), Some(&StrategyId::Buy));
}
