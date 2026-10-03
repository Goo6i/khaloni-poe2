//! Prints the craft planner's figures as JSON, offline.
//!
//! `craft_report --pool <base name> <item level> <prefix|suffix> <floor> [group ...]`
//! prints `{"entries": [ids], "tiers": {"<id>": n}}` for a normal item of
//! that base with the given groups treated as already on the item. The pool
//! goes through `core::craft::pool`, the path the planner uses. The mod
//! database and essence texts come from the cache directory and the bases
//! from the sample fixture (run from the repository root); `--mods`,
//! `--bases` and `--essences` point elsewhere.
//!
//! `craft_report --sim <fixture>` runs the orb-chain strategy through the
//! planner's simulator on a fixed pool and prints
//! `{"orb-chain": {...}, "orb-chain-clear": {...}}`, each `{"expected", "median", "p90", "give_up"}`. The fixture
//! lists the pool (`prefixes` and `suffixes` of `{id, family, tier}`, every
//! entry eligible, one family per group), the `target` (`{family,
//! min_tier}`) and the `prices` of each currency and omen.

use khaloni_poe2_core::craft::data::CraftData;
use khaloni_poe2_core::craft::model::Model;
use khaloni_poe2_core::craft::pool::pool;
use khaloni_poe2_core::craft::rules::Grade;
use khaloni_poe2_core::craft::sim::{simulate, Cap, SimConfig, DEFAULT_SEED};
use khaloni_poe2_core::craft::strategy::{Strategy, Target, Want};
use khaloni_poe2_core::craft::types::{
    AffixKind, Candidate, EssenceOutcome, ItemState, Lich, ModOn, PoolView, Rarity, Source,
};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::PathBuf;

const USAGE: &str = "usage: craft_report --pool <base name> <item level> <prefix|suffix> <floor> [group ...] \
                     [--mods <file>] [--bases <file>] [--essences <file>]\n       \
                     craft_report --sim <fixture>\n       \
                     craft_report --essences-on <base name> [--mods <file>] [--bases <file>] [--essences <file>]";

/// Runs the simulation mode on this many runs.
const SIM_RUNS: usize = 40_000;

/// Exalted Orbs a simulated run may use before it gives up.
const SIM_EXALTS: u32 = 60;

/// The item level of the fixture's item: above every entry's level.
const SIM_ITEM_LEVEL: u32 = 100;

/// A pool over the fixture's entries: every entry eligible, one family per
/// group, levels from the tier (a better tier needs a higher level).
struct FixturePool {
    entries: Vec<Candidate>,
}

impl PoolView for FixturePool {
    fn eligible(&self, state: &ItemState, kind: AffixKind, floor: u32) -> Vec<Candidate> {
        self.entries
            .iter()
            .filter(|c| c.kind == kind && floor <= c.required_level && c.required_level <= state.item_level)
            .filter(|c| !c.groups.iter().any(|g| state.groups().any(|on| on == g)))
            .cloned()
            .collect()
    }

    fn desecrated(&self, _: &ItemState, _: Option<AffixKind>, _: Option<Lich>, _: u32) -> Vec<Candidate> {
        Vec::new()
    }

    fn essence(&self, essence: &str, _: &ItemState) -> EssenceOutcome {
        EssenceOutcome::Unknown(format!("the fixture pool has no essences ({essence})"))
    }

    fn alloy(&self, alloy: &str, _: &ItemState) -> EssenceOutcome {
        EssenceOutcome::Unknown(format!("the fixture pool has no Alloys ({alloy})"))
    }
}

fn sim_entries(fixture: &Value, key: &str, kind: AffixKind) -> anyhow::Result<Vec<Candidate>> {
    let list = fixture[key].as_array().ok_or_else(|| anyhow::anyhow!("the fixture has no {key} list"))?;
    list.iter()
        .map(|e| {
            let id = e["id"].as_str().ok_or_else(|| anyhow::anyhow!("a {key} entry has no id"))?;
            let family = e["family"].as_str().ok_or_else(|| anyhow::anyhow!("{id} has no family"))?;
            let tier = e["tier"]
                .as_u64()
                .and_then(|t| u8::try_from(t).ok())
                .filter(|t| *t >= 1)
                .ok_or_else(|| anyhow::anyhow!("{id} has no tier of 1 or more"))?;
            Ok(Candidate {
                entry_id: id.to_string(),
                family: family.to_string(),
                groups: vec![family.to_string()],
                kind,
                tier,
                required_level: SIM_ITEM_LEVEL.saturating_sub(5 * u32::from(tier)),
                adds_tags: Vec::new(),
                text: id.to_string(),
                desecrated: None,
            })
        })
        .collect()
}

/// The fixture's price names, by the in-game name the simulator asks for.
const SIM_PRICES: [(&str, &str); 11] = [
    ("Orb of Transmutation", "transmute"),
    ("Orb of Augmentation", "augment"),
    ("Regal Orb", "regal"),
    ("Exalted Orb", "exalt"),
    ("Chaos Orb", "chaos"),
    ("Orb of Annulment", "annul"),
    ("Omen of Dextral Exaltation", "omen_dextral"),
    ("Omen of Sinistral Exaltation", "omen_sinistral"),
    ("Omen of Sinistral Annulment", "omen_sinistral_annul"),
    ("Omen of Dextral Annulment", "omen_dextral_annul"),
    ("Omen of Whittling", "omen_whittling"),
];

fn run_sim(path: &str) -> anyhow::Result<()> {
    let fixture: Value =
        serde_json::from_str(&read(&PathBuf::from(path))?).map_err(|e| anyhow::anyhow!("{path}: {e}"))?;
    let mut entries = sim_entries(&fixture, "prefixes", AffixKind::Prefix)?;
    entries.extend(sim_entries(&fixture, "suffixes", AffixKind::Suffix)?);

    let wants = fixture["target"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("the fixture has no target list"))?
        .iter()
        .map(|t| {
            let family = t["family"].as_str().ok_or_else(|| anyhow::anyhow!("a target has no family"))?;
            let min_tier = t["min_tier"]
                .as_u64()
                .and_then(|n| u8::try_from(n).ok())
                .ok_or_else(|| anyhow::anyhow!("{family} has no min_tier"))?;
            let kind = entries
                .iter()
                .find(|c| c.family == family)
                .map(|c| c.kind)
                .ok_or_else(|| anyhow::anyhow!("{family} is not in the pool"))?;
            Ok(Want { family: family.to_string(), kind, min_tier })
        })
        .collect::<anyhow::Result<Vec<Want>>>()?;
    let target = Target { wants };

    let table = fixture["prices"].as_object().ok_or_else(|| anyhow::anyhow!("the fixture has no prices"))?;
    let prices: HashMap<&str, f64> = SIM_PRICES
        .iter()
        .filter_map(|(name, key)| table.get(*key).and_then(Value::as_f64).map(|p| (*name, p)))
        .collect();
    let price = |item: &str| prices.get(item).copied();

    let start = ItemState {
        class: "Fixture".to_string(),
        base: "Fixture".to_string(),
        base_tags: vec!["default".to_string()],
        item_level: SIM_ITEM_LEVEL,
        rarity: Rarity::Normal,
        corrupted: false,
        mirrored: false,
        sanctified: false,
        mods: Vec::new(),
        sockets: 0,
    };
    let pool = FixturePool { entries };
    let config = SimConfig { runs: SIM_RUNS, seed: DEFAULT_SEED, cap: Cap::Exalts(SIM_EXALTS) };
    // Both routes past a blocking side, the ones a plan chooses between.
    let mut report = Map::new();
    for (key, clear) in [("orb-chain", false), ("orb-chain-clear", true)] {
        let strategy = Strategy::OrbChain { magic: Grade::Normal, rare: Grade::Normal, clear };
        let costed = simulate(&strategy, &start, &target, &pool, &Model::Uniform, &price, &config);
        let (Some(expected), Some(median), Some(p90)) = (costed.expected, costed.median, costed.p90) else {
            anyhow::bail!("{key}: the simulation has no figures: {}", costed.unknowns.join("; "));
        };
        report.insert(key.to_string(), json!({ "expected": expected, "median": median, "p90": p90, "give_up": costed.give_up_rate }));
    }
    println!("{}", Value::Object(report));
    Ok(())
}

fn cache_dir() -> PathBuf {
    directories::ProjectDirs::from("", "", "khaloni-poe2")
        .map(|d| d.cache_dir().to_path_buf())
        .unwrap_or_else(std::env::temp_dir)
}

fn read(path: &PathBuf) -> anyhow::Result<String> {
    std::fs::read_to_string(path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))
}

fn main() -> anyhow::Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--sim") {
        let [_, fixture] = args.as_slice() else {
            anyhow::bail!("{USAGE}");
        };
        return run_sim(fixture);
    }
    let mut take = |flag: &str| -> anyhow::Result<Option<String>> {
        let Some(i) = args.iter().position(|a| a == flag) else { return Ok(None) };
        if i + 1 >= args.len() {
            anyhow::bail!("{flag} needs a value\n{USAGE}");
        }
        let value = args.remove(i + 1);
        args.remove(i);
        Ok(Some(value))
    };
    let mods = take("--mods")?.map_or_else(|| cache_dir().join("repoe_mods.json"), PathBuf::from);
    let bases = take("--bases")?.map_or_else(|| PathBuf::from("core/tests/fixtures/craft_base_items_sample.json"), PathBuf::from);
    let essences = take("--essences")?.map_or_else(|| cache_dir().join("xile_essences.json"), PathBuf::from);

    if args.first().map(String::as_str) == Some("--essences-on") {
        let [_, base] = args.as_slice() else {
            anyhow::bail!("{USAGE}");
        };
        let data = CraftData::load(&read(&mods)?, &read(&bases)?, &read(&essences)?).map_err(anyhow::Error::msg)?;
        let found = data.base(base).ok_or_else(|| anyhow::anyhow!("{base} is not a base in {}", bases.display()))?;
        // Every essence's outcome on the base: the entry it guarantees, or
        // why none could be named.
        let rows: Vec<serde_json::Value> = data
            .essences()
            .iter()
            .map(|e| match data.essence_outcome(&e.name, found) {
                EssenceOutcome::Entry(c) => serde_json::json!({"essence": e.name, "entry": c.entry_id, "family": c.family, "tier": c.tier}),
                EssenceOutcome::Unknown(why) => serde_json::json!({"essence": e.name, "unknown": why}),
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    let [flag, base, ilvl, kind, floor, groups @ ..] = args.as_slice() else {
        anyhow::bail!("{USAGE}");
    };
    if flag != "--pool" {
        anyhow::bail!("{USAGE}");
    }
    let item_level: u32 = ilvl.parse().map_err(|_| anyhow::anyhow!("item level {ilvl:?} is not a whole number"))?;
    let floor: u32 = floor.parse().map_err(|_| anyhow::anyhow!("floor {floor:?} is not a whole number"))?;
    let kind = match kind.as_str() {
        "prefix" => AffixKind::Prefix,
        "suffix" => AffixKind::Suffix,
        other => anyhow::bail!("{other:?} is neither prefix nor suffix\n{USAGE}"),
    };

    let data = CraftData::load(&read(&mods)?, &read(&bases)?, &read(&essences)?).map_err(anyhow::Error::msg)?;
    let found = data.base(base).ok_or_else(|| anyhow::anyhow!("{base} is not a base in {}", bases.display()))?;
    // Each named group stands for a modifier already on the item; only its
    // group matters to the pool.
    let held = groups
        .iter()
        .map(|g| ModOn {
            entry_id: None,
            family: g.clone(),
            groups: vec![g.clone()],
            kind,
            tier: None,
            required_level: None,
            adds_tags: Vec::new(),
            source: Source::Random,
            text: g.clone(),
        })
        .collect();
    let state = ItemState {
        class: found.class.clone(),
        base: found.name.clone(),
        base_tags: found.tags.clone(),
        item_level,
        rarity: Rarity::Normal,
        corrupted: false,
        mirrored: false,
        sanctified: false,
        mods: held,
        sockets: 0,
    };

    let candidates = pool(&data, &state, kind, floor);
    let entries: Vec<Value> = candidates.iter().map(|c| Value::from(c.entry_id.clone())).collect();
    let tiers: Map<String, Value> = candidates.iter().map(|c| (c.entry_id.clone(), Value::from(c.tier))).collect();
    println!("{}", json!({ "entries": entries, "tiers": tiers }));
    Ok(())
}
