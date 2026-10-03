//! The simulator on a small hand-built pool: seeded and repeatable, its
//! statistics consistent with each other, geometric steps costed by their
//! closed form, the give-up cap stated, and no figure where a price or a
//! rule is unknown.

use std::collections::HashMap;

use khaloni_poe2_core::craft::model::{Model, UNIFORM_LABEL};
use khaloni_poe2_core::craft::plan::{plan, BuyQuote};
use khaloni_poe2_core::craft::rules::{Action, Grade, Omen, Orb};
use khaloni_poe2_core::craft::sim::{
    closed_form_geometric, items, outcomes, simulate, Cap, Costed, SimConfig, A_DESECRATED_UNOBSERVED,
};
use khaloni_poe2_core::craft::strategy::{Next, Strategy, Target, Want};
use khaloni_poe2_core::craft::types::{
    AffixKind, Candidate, EssenceOutcome, ItemState, Lich, ModOn, Outcome, PoolView, Rarity, Source,
};

use AffixKind::{Prefix, Suffix};

// ---------------------------------------------------------------- fakes

fn cand(family: &str, kind: AffixKind, tier: u8, level: u32) -> Candidate {
    Candidate {
        entry_id: format!("{family}{tier}"),
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

/// A pool over a fixed list, filtering like the real one: kind, group not
/// on the item, `floor <= required_level <= item level`.
struct FakePool {
    entries: Vec<Candidate>,
    desecrated: Vec<Candidate>,
}

fn fake() -> FakePool {
    FakePool {
        entries: vec![
            cand("Life", Prefix, 1, 80),
            cand("Life", Prefix, 2, 60),
            cand("Life", Prefix, 3, 40),
            cand("Life", Prefix, 4, 20),
            cand("Armour", Prefix, 1, 75),
            cand("Armour", Prefix, 2, 45),
            cand("Armour", Prefix, 3, 10),
            cand("Thorns", Prefix, 1, 30),
            cand("Thorns", Prefix, 2, 5),
            cand("Fire", Suffix, 1, 70),
            cand("Fire", Suffix, 2, 45),
            cand("Fire", Suffix, 3, 15),
            cand("Cold", Suffix, 1, 70),
            cand("Cold", Suffix, 2, 45),
            cand("Cold", Suffix, 3, 15),
            cand("Str", Suffix, 1, 55),
            cand("Str", Suffix, 2, 11),
        ],
        desecrated: vec![Candidate { desecrated: Some(Lich::Ulaman), ..cand("Speed", Suffix, 1, 65) }],
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
            .filter(|c| kind.is_none_or(|k| c.kind == k) && lich.is_none_or(|l| c.desecrated == Some(l)) && fits(state, c, floor))
            .cloned()
            .collect()
    }

    fn essence(&self, essence: &str, _: &ItemState) -> EssenceOutcome {
        EssenceOutcome::Unknown(format!("the fake pool has no essences ({essence})"))
    }

    fn alloy(&self, alloy: &str, _: &ItemState) -> EssenceOutcome {
        EssenceOutcome::Unknown(format!("the fake pool has no Alloys ({alloy})"))
    }
}

fn item(rarity: Rarity, mods: Vec<ModOn>) -> ItemState {
    ItemState {
        class: "Body Armour".into(),
        base: "Test Cuirass".into(),
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

fn on(family: &str, kind: AffixKind, tier: u8) -> ModOn {
    let c = fake().entries.into_iter().find(|c| c.family == family && c.tier == tier).expect("in the fake pool");
    assert_eq!(c.kind, kind);
    c.to_mod(Source::Random)
}

fn want(family: &str, kind: AffixKind, min_tier: u8) -> Want {
    Want { family: family.into(), kind, min_tier }
}

/// Life T2+, Fire T2+ and Cold T2+ on a normal item: an orb-chain has to
/// slam and annul its way there.
fn three_mod_target() -> Target {
    Target { wants: vec![want("Life", Prefix, 2), want("Fire", Suffix, 2), want("Cold", Suffix, 2)] }
}

fn price_table() -> HashMap<String, f64> {
    [
        ("Orb of Transmutation", 0.1),
        ("Orb of Augmentation", 0.2),
        ("Regal Orb", 0.8),
        ("Exalted Orb", 1.0),
        ("Greater Exalted Orb", 3.0),
        ("Perfect Exalted Orb", 12.0),
        ("Greater Regal Orb", 2.5),
        ("Perfect Regal Orb", 9.0),
        ("Chaos Orb", 1.5),
        ("Orb of Annulment", 4.0),
        ("Omen of Sinistral Exaltation", 6.0),
        ("Omen of Dextral Exaltation", 7.0),
        ("Omen of Sinistral Annulment", 5.0),
        ("Omen of Dextral Annulment", 5.5),
        ("Omen of Sinistral Erasure", 2.0),
        ("Omen of Dextral Erasure", 2.5),
        ("Omen of Whittling", 9.0),
        ("Omen of Light", 3.0),
        ("Preserved Rib", 2.0),
        ("Omen of Dextral Necromancy", 4.0),
        ("Omen of Abyssal Echoes", 6.0),
        ("Orb of Alchemy", 0.7),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect()
}

fn config(runs: usize, seed: u64) -> SimConfig {
    SimConfig { runs, seed, ..SimConfig::default() }
}

fn orb_chain() -> Strategy {
    Strategy::OrbChain { magic: Grade::Normal, rare: Grade::Normal, clear: false }
}

fn run(strategy: &Strategy, start: &ItemState, target: &Target, model: &Model, cfg: &SimConfig) -> Costed {
    let table = price_table();
    let prices = |name: &str| table.get(name).copied();
    simulate(strategy, start, target, &fake(), model, &prices, cfg)
}

fn close(a: f64, b: f64, rel: f64) -> bool {
    (a - b).abs() <= rel * b.abs().max(1e-12)
}

// ---------------------------------------------------------------- L1

#[test]
fn the_same_seed_gives_the_same_plan() {
    let start = item(Rarity::Normal, vec![]);
    let target = three_mod_target();
    let a = run(&orb_chain(), &start, &target, &Model::Uniform, &config(4_000, 7));
    let b = run(&orb_chain(), &start, &target, &Model::Uniform, &config(4_000, 7));
    assert_eq!(a, b, "the same seed repeats every figure");
    assert!(a.expected.is_some() && a.median.is_some() && a.p90.is_some());
    let c = run(&orb_chain(), &start, &target, &Model::Uniform, &config(4_000, 8));
    assert_ne!(a.expected, c.expected, "another seed draws other runs");

    // A whole plan repeats too.
    let table = price_table();
    let prices = |name: &str| table.get(name).copied();
    let quote = Some(BuyQuote { price: 40.0, listings: 3 });
    let p1 = plan(&start, &target, &fake(), None, &prices, quote, &config(1_500, 11));
    let p2 = plan(&start, &target, &fake(), None, &prices, quote, &config(1_500, 11));
    assert_eq!(p1, p2);
    assert!(!p1.strategies.is_empty());
}

#[test]
fn the_statistics_follow_the_percentile_rule() {
    let start = item(Rarity::Normal, vec![]);
    let costed = run(&orb_chain(), &start, &three_mod_target(), &Model::Uniform, &config(6_000, 3));
    let (median, p90, expected) = (costed.median.unwrap(), costed.p90.unwrap(), costed.expected.unwrap());
    assert!(median <= p90, "median {median} above p90 {p90}");
    // The spend is skewed: a long tail of unlucky runs pulls the mean above
    // the median.
    assert!(expected > median, "expected {expected} not above median {median}");
    assert_eq!(costed.runs, 6_000);
    assert!(costed.reached > 0 && costed.reached <= costed.runs);
}

/// A rare holding the three wanted prefixes and one unwanted suffix, with
/// fire resistance still missing: Chaos with Dextral Erasure can only take
/// the unwanted suffix and can only add a suffix, so every miss leaves an
/// equivalent item.
fn geometric_case() -> (ItemState, Target) {
    let start = item(
        Rarity::Rare,
        vec![on("Life", Prefix, 1), on("Armour", Prefix, 1), on("Thorns", Prefix, 1), on("Str", Suffix, 2)],
    );
    let target = Target {
        wants: vec![want("Life", Prefix, 1), want("Armour", Prefix, 1), want("Thorns", Prefix, 1), want("Fire", Suffix, 2)],
    };
    (start, target)
}

#[test]
fn a_geometric_step_agrees_with_its_closed_form_within_five_percent() {
    assert_eq!(closed_form_geometric(0.25, 2.0), 8.0);
    assert!(closed_form_geometric(0.0, 2.0).is_infinite());

    let (start, target) = geometric_case();
    let strategy = Strategy::ChaosSpam { grade: Grade::Normal };
    let chaos = Action::orb(Orb::Chaos).with(Omen::DextralErasure);
    assert_eq!(strategy.next(&start, &target), Next::Retry(chaos.clone()));

    // After the unwanted suffix goes, eight suffix entries can roll (its
    // own group is free again) and two of them are fire at T2 or better.
    let all = outcomes(&chaos, &start, &fake(), &Model::Uniform).unwrap();
    let p: f64 = all.iter().filter(|(_, o)| matches!(o, Outcome::Applied(s) if target.holds(s))).map(|(p, _)| p).sum();
    assert!(close(p, 0.25, 1e-12), "p = {p}");
    assert!(close(all.iter().map(|(p, _)| p).sum::<f64>(), 1.0, 1e-12));

    let costed = run(&strategy, &start, &target, &Model::Uniform, &config(20_000, 99));
    assert_eq!(costed.steps.len(), 1, "{:?}", costed.steps);
    let step = &costed.steps[0];
    assert_eq!(step.action, chaos);
    let unit = 1.5 + 2.5;
    assert_eq!(step.unit_price, Some(unit));
    assert!(step.closed_form);
    assert!(close(step.p_success.unwrap(), 0.25, 1e-12));
    let closed = closed_form_geometric(0.25, unit);
    assert!(close(step.cost.unwrap(), closed, 1e-9), "the closed form is the step's figure");
    let simulated = step.simulated_cost.unwrap();
    assert!(close(simulated, closed, 0.05), "simulated {simulated} vs closed form {closed}");
    assert!(close(step.expected_tries, 4.0, 1e-9));
    assert!(close(costed.expected.unwrap(), closed, 1e-9));
    // Ten times the median is 30 tries, which about 0.75^30 of runs need.
    assert!(costed.give_up_rate < 0.001, "give-up {}", costed.give_up_rate);
}

#[test]
fn the_give_up_cap_is_reported_not_hidden() {
    let start = item(Rarity::Normal, vec![]);
    let target = three_mod_target();

    // Five Exalted Orbs are often not enough: some runs give up, and the
    // figures cover only the runs that reached the target.
    let tight = SimConfig { runs: 5_000, seed: 5, cap: Cap::Exalts(5) };
    let costed = run(&orb_chain(), &start, &target, &Model::Uniform, &tight);
    assert!(costed.give_up_rate > 0.05 && costed.give_up_rate < 0.95, "give-up {}", costed.give_up_rate);
    assert!(close(costed.give_up_rate, (costed.runs - costed.reached) as f64 / costed.runs as f64, 1e-12));
    assert_eq!(costed.cap, "a run gives up after 5 Exalted Orbs");
    // The currency of the runs that gave up is not hidden either: a
    // finished item costs more than the mean of the lucky runs.
    let (expected, finished) = (costed.expected.unwrap(), costed.per_finished.unwrap());
    assert!(finished > expected * 1.05, "per finished {finished} vs expected {expected}");

    // The default cap is stated with the figure it came to.
    let costed = run(&orb_chain(), &start, &target, &Model::Uniform, &config(5_000, 5));
    assert!(costed.cap.starts_with("a run gives up past 10x the median number of currency uses ("), "{}", costed.cap);
    // With nothing given up the two figures agree.
    if costed.give_up_rate == 0.0 {
        assert_eq!(costed.per_finished, costed.expected);
    }

    // When no run gets there, there is no figure, and the plan says why.
    let none = SimConfig { runs: 2_000, seed: 5, cap: Cap::Exalts(0) };
    let costed = run(&orb_chain(), &start, &target, &Model::Uniform, &none);
    assert_eq!(costed.reached, 0);
    assert_eq!(costed.give_up_rate, 1.0);
    assert_eq!((costed.expected, costed.median, costed.p90), (None, None, None));
    assert!(costed.unknowns.iter().any(|u| u.starts_with("unknown: none of ") && u.ends_with("runs reached the target before giving up")), "{:?}", costed.unknowns);
}

#[test]
fn per_step_figures_sum_to_the_total() {
    let start = item(Rarity::Normal, vec![]);
    let costed = run(&orb_chain(), &start, &three_mod_target(), &Model::Uniform, &config(6_000, 21));
    let expected = costed.expected.unwrap();
    let sum: f64 = costed.steps.iter().map(|s| s.cost.unwrap()).sum();
    assert!(close(sum, expected, 1e-9), "steps {sum} vs total {expected}");
    // Without a geometric step the total is the simulated mean itself.
    assert!(costed.steps.iter().all(|s| !s.closed_form));
    assert!(close(expected, costed.simulated_mean.unwrap(), 1e-9));
    for s in &costed.steps {
        assert!(close(s.cost.unwrap(), s.unit_price.unwrap() * s.count, 1e-9), "{}", s.name);
        assert_eq!(s.p_success, None);
    }
    let names: Vec<&str> = costed.steps.iter().map(|s| s.name.as_str()).collect();
    for step in ["Orb of Transmutation", "Orb of Augmentation", "Regal Orb"] {
        assert!(names.contains(&step), "{names:?}");
    }
    // One transmutation, augmentation and regal per run.
    for s in costed.steps.iter().filter(|s| s.name.starts_with("Orb of T") || s.name.starts_with("Orb of Au") || s.name == "Regal Orb") {
        assert!(close(s.count, 1.0, 1e-12), "{} {}", s.name, s.count);
    }
    // The currency list adds up each item across the steps that use it.
    let currency = costed.currency();
    let exalts: f64 = costed.steps.iter().filter(|s| items(&s.action)[0] == "Exalted Orb").map(|s| s.count).sum();
    let listed = currency.iter().find(|(n, _)| n == "Exalted Orb").map(|(_, n)| *n).unwrap();
    assert!(close(listed, exalts, 1e-12));
}

// ---------------------------------------------------------------- L3

#[test]
fn a_plan_needing_an_unknown_rule_has_no_total() {
    // The rulebook has no prefix and suffix limits for jewels, so a jewel's
    // first Transmutation is an unknown rule.
    let mut jewel = item(Rarity::Normal, vec![]);
    jewel.class = "Jewel".into();
    jewel.base_tags = vec!["jewel".into(), "default".into()];
    let costed = run(&orb_chain(), &jewel, &three_mod_target(), &Model::Uniform, &config(500, 1));
    assert_eq!((costed.expected, costed.median, costed.p90), (None, None, None));
    assert!(
        costed.unknowns.iter().any(|u| u.starts_with("unknown: the rulebook has no sourced prefix and suffix limits for jewels")),
        "{:?}",
        costed.unknowns
    );

    // A missing price is an unknown too: the known steps keep their
    // figures, the plan has no total, and the missing price is named.
    let table: HashMap<String, f64> =
        price_table().into_iter().filter(|(k, _)| k != "Omen of Dextral Exaltation").collect();
    let prices = |name: &str| table.get(name).copied();
    let start = item(Rarity::Normal, vec![]);
    let costed = simulate(&orb_chain(), &start, &three_mod_target(), &fake(), &Model::Uniform, &prices, &config(3_000, 2));
    assert_eq!(costed.expected, None);
    assert_eq!((costed.median, costed.p90), (None, None));
    assert!(costed.unknowns.contains(&"unknown: no price for Omen of Dextral Exaltation".to_string()), "{:?}", costed.unknowns);
    let dextral = costed.steps.iter().find(|s| s.name == "Exalted Orb + Omen of Dextral Exaltation").expect("the step is listed");
    assert_eq!(dextral.cost, None);
    assert!(dextral.count > 0.0);
    let transmute = costed.steps.iter().find(|s| s.name == "Orb of Transmutation").unwrap();
    assert!(close(transmute.cost.unwrap(), 0.1, 1e-12));
}

#[test]
fn the_uniform_model_is_labelled_an_optimistic_floor() {
    assert_eq!(UNIFORM_LABEL, "optimistic floor: assumes every tier equally likely");
    let start = item(Rarity::Normal, vec![]);
    let costed = run(&orb_chain(), &start, &three_mod_target(), &Model::Uniform, &config(1_000, 1));
    assert_eq!(costed.model_label, UNIFORM_LABEL);

    let table = price_table();
    let prices = |name: &str| table.get(name).copied();
    let p = plan(&start, &three_mod_target(), &fake(), None, &prices, None, &config(800, 1));
    assert!(p.model_line.contains("optimistic floor: assumes every tier equally likely"), "{}", p.model_line);
    assert!(p.model_line.starts_with("uniform model"));
    for r in &p.strategies {
        assert_eq!(r.costed.model_label, UNIFORM_LABEL);
        assert!(r.uniform.is_none(), "with no observed model the uniform one is the headline");
    }
    assert_eq!(p.patch, "0.5.5");
}

/// Listings of the class where low tiers dominate, as real listings do.
fn observed_model() -> Model {
    let mut counts = HashMap::new();
    let mut totals = HashMap::new();
    for c in fake().entries {
        let n = match c.tier {
            1 => 3,
            2 => 12,
            _ => 30,
        };
        counts.insert(c.entry_id.clone(), n);
        *totals.entry(c.kind).or_insert(0) += n;
    }
    Model::Observed { class: "Body Armour".into(), counts, totals, listings: 412 }
}

#[test]
fn the_observed_model_is_the_headline_when_present() {
    let observed = observed_model();
    let start = item(Rarity::Normal, vec![]);
    let target = three_mod_target();
    let table = price_table();
    let prices = |name: &str| table.get(name).copied();
    let p = plan(&start, &target, &fake(), Some(&observed), &prices, Some(BuyQuote { price: 500.0, listings: 4 }), &config(1_500, 4));
    assert!(p.model_line.starts_with("observed on 412 listings of Body Armour"), "{}", p.model_line);
    assert!(p.model_line.contains(UNIFORM_LABEL));
    assert!(!p.strategies.is_empty());
    for r in &p.strategies {
        assert_eq!(r.costed.model_label, "observed on 412 listings of Body Armour");
        let uniform = r.uniform.as_ref().expect("both models are costed");
        assert_eq!(uniform.model_label, UNIFORM_LABEL);
        assert_eq!(r.costed.strategy, uniform.strategy);
    }
    // Never blended: the observed figures are their own, and dearer here
    // because the listings favour low tiers.
    // The plain orb-chain, on whichever route past a blocking side the plan
    // found cheaper.
    let orb = p
        .strategies
        .iter()
        .find(|r| r.costed.strategy == "orb-chain" || r.costed.strategy == "orb-chain (clearing blockers first)")
        .unwrap();
    let (o, u) = (orb.costed.expected.unwrap(), orb.uniform.as_ref().unwrap().expected.unwrap());
    assert!(o > u, "observed {o} vs uniform {u}");
    // The ranking follows the observed figures.
    let totals: Vec<f64> = p.strategies.iter().filter_map(|r| r.costed.per_finished).collect();
    assert!(totals.windows(2).all(|w| w[0] <= w[1]), "{totals:?}");
}

#[test]
fn an_unseen_wanted_tier_has_no_observed_figure() {
    // The listings never showed fire resistance at all.
    let Model::Observed { class, mut counts, totals, listings } = observed_model() else { unreachable!() };
    for tier in 1..=3 {
        counts.remove(&format!("Fire{tier}"));
    }
    let observed = Model::Observed { class, counts, totals, listings };
    let start = item(Rarity::Normal, vec![]);
    let costed = run(&orb_chain(), &start, &three_mod_target(), &observed, &config(1_000, 1));
    assert_eq!(costed.expected, None);
    assert!(
        costed.unknowns.iter().any(|u| u.starts_with("unknown: Fire at tier 2 or better has not been seen on the observed listings of Body Armour")),
        "{:?}",
        costed.unknowns
    );
    assert!(!costed.unknowns.iter().any(|u| u.contains("runs reached the target")), "not reported as an impossible craft");

    // A draw with nothing observed at all is an unknown, not a give-up.
    let empty = Model::Observed { class: "Body Armour".into(), counts: HashMap::new(), totals: HashMap::new(), listings: 300 };
    let lone = Target { wants: vec![want("Life", Prefix, 4)] };
    let costed = run(&orb_chain(), &start, &lone, &empty, &config(200, 1));
    assert_eq!(costed.expected, None);
    assert!(costed.unknowns.iter().any(|u| u.contains("has not been seen on the observed listings")), "{:?}", costed.unknowns);
}

#[test]
fn a_desecration_under_the_observed_model_uses_the_uniform_model_and_says_so() {
    let observed = observed_model();
    let start = item(Rarity::Rare, vec![on("Life", Prefix, 1), on("Fire", Suffix, 1)]);
    let target = Target { wants: vec![want("Life", Prefix, 1), want("Fire", Suffix, 1), want("Speed", Suffix, 1)] };
    let bone = Action::bone(khaloni_poe2_core::craft::rules::BoneKind::Rib, khaloni_poe2_core::craft::rules::BoneGrade::Preserved)
        .with(Omen::DextralNecromancy)
        .with(Omen::AbyssalEchoes);
    let strategy = Strategy::Desecrate { bone };
    let costed = run(&strategy, &start, &target, &observed, &config(1_000, 6));
    assert!(costed.expected.is_some(), "{:?}", costed.unknowns);
    assert_eq!(costed.give_up_rate, 0.0);
    assert!(costed.assumptions.contains(&A_DESECRATED_UNOBSERVED.to_string()), "{:?}", costed.assumptions);
    // Under the uniform model the note is not needed.
    let costed = run(&strategy, &start, &target, &Model::Uniform, &config(1_000, 6));
    assert!(!costed.assumptions.contains(&A_DESECRATED_UNOBSERVED.to_string()));
}

#[test]
fn a_roll_sees_whole_modifiers_and_keeps_groups_apart() {
    // Greater Exaltation adds two modifiers in one roll: the second draw
    // must see the first one's group, and what comes out is the whole
    // modifier (its groups and text), not a trimmed copy.
    let start = item(Rarity::Rare, vec![on("Life", Prefix, 1)]);
    let action = Action::orb(Orb::Exalted).with(Omen::GreaterExaltation);
    let all = outcomes(&action, &start, &fake(), &Model::Uniform).unwrap();
    assert!(close(all.iter().map(|(p, _)| p).sum::<f64>(), 1.0, 1e-12));
    for (_, o) in &all {
        let Outcome::Applied(s) = o else { panic!("{o:?}") };
        assert_eq!(s.mods.len(), 3);
        let mut families: Vec<&str> = s.mods.iter().map(|m| m.family.as_str()).collect();
        families.sort_unstable();
        families.dedup();
        assert_eq!(families.len(), 3, "one group each: {:?}", s.mods);
        for m in &s.mods {
            assert_eq!(m.groups, vec![m.family.clone()]);
            assert!(!m.text.is_empty());
        }
    }

}
