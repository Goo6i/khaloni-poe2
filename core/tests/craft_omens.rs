//! Every omen a plan uses is named with what it does and, where the item's
//! pools decide it, how it moves the odds of its step, counted from the
//! same pools the simulation draws from.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use khaloni_poe2_core::craft::data::CraftData;
use khaloni_poe2_core::craft::plan::plan;
use khaloni_poe2_core::craft::sim::SimConfig;
use khaloni_poe2_core::craft::strategy::{Target, Want};
use khaloni_poe2_core::craft::types::{AffixKind, ItemState, PoolView, Rarity};

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

#[test]
fn a_plan_names_each_omen_it_uses_with_its_effect_and_the_odds_it_buys() {
    let data = data();
    let base = data.base("Gold Ring").unwrap();
    let ring = ItemState {
        class: base.class.clone(),
        base: base.name.clone(),
        base_tags: base.tags.clone(),
        item_level: 82,
        rarity: Rarity::Normal,
        corrupted: false,
        mirrored: false,
        sanctified: false,
        mods: Vec::new(),
        sockets: 0,
    };
    let target = Target {
        wants: vec![
            Want { family: "FireResistance".into(), kind: AffixKind::Suffix, min_tier: 2 },
            Want { family: "ColdResistance".into(), kind: AffixKind::Suffix, min_tier: 2 },
        ],
    };
    let prices: HashMap<&str, f64> = [
        ("Orb of Transmutation", 0.05),
        ("Orb of Augmentation", 0.08),
        ("Regal Orb", 0.4),
        ("Exalted Orb", 1.0),
        ("Orb of Annulment", 6.0),
        ("Omen of Dextral Exaltation", 11.0),
        ("Omen of Sinistral Exaltation", 9.0),
        ("Omen of Dextral Annulment", 8.0),
        ("Omen of Sinistral Annulment", 7.0),
    ]
    .into_iter()
    .collect();
    let price = |n: &str| prices.get(n).copied();
    let p = plan(&ring, &target, &data, None, &price, None, &SimConfig { runs: 200, seed: 5, ..SimConfig::default() });
    let chain = p
        .strategies
        .iter()
        .find(|r| r.costed.strategy == "orb-chain")
        .unwrap_or_else(|| panic!("a plain orb-chain: {:?}", p.strategies.iter().map(|r| r.costed.strategy.clone()).collect::<Vec<_>>()));
    let dextral = chain
        .omens
        .iter()
        .find(|o| o.starts_with("Omen of Dextral Exaltation: "))
        .unwrap_or_else(|| panic!("the omen is named: {:?}", chain.omens));
    assert!(dextral.contains("Exalted Orb will add only suffix modifiers"), "{dextral}");

    // The odds, counted here from the pool: wanted suffixes over the suffix
    // pool, against wanted over both pools.
    let rare = ItemState { rarity: Rarity::Rare, ..ring.clone() };
    let suffixes = data.eligible(&rare, AffixKind::Suffix, 0);
    let prefixes = data.eligible(&rare, AffixKind::Prefix, 0);
    let wanted = suffixes.iter().filter(|c| target.wants.iter().any(|w| w.accepts(c))).count();
    assert!(wanted > 0);
    let with = format!("1 in {:.0}", suffixes.len() as f64 / wanted as f64);
    let without = format!("1 in {:.0}", (suffixes.len() + prefixes.len()) as f64 / wanted as f64);
    assert!(dextral.ends_with(&format!("a wanted modifier per orb: {with} instead of {without}")), "{dextral}");
    // Every omen of every plan is explained.
    for r in &p.strategies {
        let used: std::collections::BTreeSet<String> =
            r.costed.steps.iter().flat_map(|s| s.action.omens().iter().map(|o| o.name().to_string())).collect();
        for name in used {
            assert!(r.omens.iter().any(|o| o.starts_with(&format!("{name}: "))), "{} leaves {name} unexplained", r.costed.strategy);
        }
    }
}
