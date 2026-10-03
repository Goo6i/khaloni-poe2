//! A plan built end to end from the real fixtures: a Soldier Cuirass aimed
//! at life and two resistances, costed under the uniform model and under an
//! observed model recorded from the listing fixture through the same join
//! the app uses, each figure carrying its model's label.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use khaloni_poe2_core::craft::data::CraftData;
use khaloni_poe2_core::craft::model::UNIFORM_LABEL;
use khaloni_poe2_core::craft::observed::{listing_entries, Observed};
use khaloni_poe2_core::craft::plan::{plan, BuyQuote};
use khaloni_poe2_core::craft::sim::SimConfig;
use khaloni_poe2_core::craft::strategy::{Target, Want};
use khaloni_poe2_core::craft::types::{AffixKind, ItemState, Rarity};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn prices() -> HashMap<String, f64> {
    [
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
        ("Orb of Annulment", 6.0),
        ("Orb of Alchemy", 0.3),
        ("Omen of Sinistral Exaltation", 9.0),
        ("Omen of Dextral Exaltation", 11.0),
        ("Omen of Sinistral Annulment", 7.0),
        ("Omen of Dextral Annulment", 8.0),
        ("Greater Essence of Insulation", 3.0),
        ("Greater Essence of Thawing", 3.0),
        ("Greater Essence of the Body", 3.0),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect()
}

#[test]
fn a_real_base_plans_under_uniform_and_observed_models_and_labels_both() {
    let dir = fixtures();
    let data = CraftData::load(
        &read(&dir.join("craft/mods_slice.json")),
        &read(&dir.join("craft_base_items_sample.json")),
        &read(&dir.join("craft/essences_slice.json")),
    )
    .expect("the craft fixtures load");

    // Seed the store from the real fetch through the app's join.
    let fetch: serde_json::Value = serde_json::from_str(&read(&dir.join("trade_fetch_full.json"))).unwrap();
    let mut store = Observed::new("Forbidden Rites");
    let mut recorded = 0;
    for entry in fetch["result"].as_array().unwrap() {
        if let Ok(read) = listing_entries(entry, &data) {
            if store.record(read.listing) {
                recorded += 1;
            }
        }
    }
    assert!(recorded >= 10, "the fixture's rare body armours record ({recorded})");
    let class = "Body Armour";
    assert_eq!(store.listings(class), recorded as u32);
    // The real threshold: this sample is far too small to head a plan.
    assert!(store.model(class).is_none(), "under 200 listings there is no observed model");
    store.set_min_listings(10);
    let observed = store.model(class).expect("with the threshold lowered the sample makes a model");
    assert!(observed.label().starts_with(&format!("observed on {recorded} listings of Body Armour")));

    let base = data.base("Soldier Cuirass").unwrap();
    let start = ItemState {
        class: base.class.clone(),
        base: base.name.clone(),
        base_tags: base.tags.clone(),
        item_level: 80,
        rarity: Rarity::Normal,
        corrupted: false,
        mirrored: false,
        sanctified: false,
        mods: Vec::new(),
        sockets: 0,
    };
    let target = Target {
        wants: vec![
            Want { family: "IncreasedLife".into(), kind: AffixKind::Prefix, min_tier: 3 },
            Want { family: "FireResistance".into(), kind: AffixKind::Suffix, min_tier: 3 },
            Want { family: "ColdResistance".into(), kind: AffixKind::Suffix, min_tier: 3 },
        ],
    };
    let table = prices();
    let price = |name: &str| table.get(name).copied();
    let config = SimConfig { runs: 300, seed: 7, ..SimConfig::default() };
    let buy = Some(BuyQuote { price: 900.0, listings: 3 });

    // Uniform only: every figure says it is the optimistic floor.
    let uniform = plan(&start, &target, &data, None, &price, buy, &config);
    assert_eq!(uniform.patch, "0.5.5");
    assert!(uniform.model_line.contains(UNIFORM_LABEL));
    assert!(!uniform.strategies.is_empty(), "a normal cuirass has strategies to cost");
    for r in &uniform.strategies {
        assert_eq!(r.costed.model_label, UNIFORM_LABEL, "{}", r.costed.strategy);
        assert!(r.uniform.is_none(), "no second column without an observed model");
    }
    assert_eq!(uniform.buy.price, Some(900.0));
    assert!(uniform.strategies.iter().any(|r| r.costed.per_finished.is_some()), "at least one uniform plan has a total");

    // Observed headline with the uniform column beside it, never blended.
    let both = plan(&start, &target, &data, Some(&observed), &price, buy, &config);
    assert!(both.model_line.starts_with(&observed.label()));
    assert!(both.model_line.contains(UNIFORM_LABEL));
    for r in &both.strategies {
        assert_eq!(r.costed.model_label, observed.label(), "{}", r.costed.strategy);
        let u = r.uniform.as_ref().expect("the uniform column sits beside every observed plan");
        assert_eq!(u.model_label, UNIFORM_LABEL);
        // A figure the sample cannot back is an unknown, never a number.
        if r.costed.per_finished.is_none() {
            assert!(!r.costed.unknowns.is_empty(), "{} has no observed total and no reason", r.costed.strategy);
            assert!(r.costed.unknowns.iter().all(|u| u.starts_with("unknown: ")));
        }
    }
    // Each plan takes, per strategy, the cheaper route past a blocking side
    // under its own headline model, so two plans may differ in route; the
    // strategies they cost are the same.
    let route = ", clearing blockers first)";
    let base = |name: &str| name.replace(route, ")").replace(" (clearing blockers first)", "");
    let same_strategies = |p: &khaloni_poe2_core::craft::plan::Plan| {
        let mut v: Vec<String> = p.strategies.iter().map(|r| base(&r.costed.strategy)).collect();
        v.sort();
        v
    };
    assert_eq!(same_strategies(&uniform), same_strategies(&both), "both models cost the same strategies");
    // Within one plan the uniform column is the same route as its headline.
    for r in &both.strategies {
        assert_eq!(r.uniform.as_ref().map(|u| u.strategy.as_str()), Some(r.costed.strategy.as_str()));
    }
}
