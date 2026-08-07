//! Tier-mapping tests: joining the repoe-fork PoE2 `mods.json` export onto
//! EE2 affixes by internal stat id. Fixtures are small inline JSON mirroring
//! the real files' shapes; the rules under test are the conservative-join
//! filters (a mod contributes tiers only when the join is unambiguous).

use khaloni_poe2_core::refdata::{parse_affixes, parse_affixes_tiered, AffixKind};
use khaloni_poe2_core::rollquality;

/// EE2 stats.ndjson shape: `ref` readable text, `trade.ids.*`, and the
/// internal stat id under `id` (the join key). One line has no `id`.
const STATS: &str = concat!(
    r##"{"ref": "# to Strength", "trade": {"ids": {"explicit": ["explicit.stat_4080418644"]}}, "id": "additional_strength"}"##,
    "\n",
    r##"{"ref": "# to maximum Life", "trade": {"ids": {"explicit": ["explicit.stat_3299347043"]}}, "id": "base_maximum_life"}"##,
    "\n",
    r##"{"ref": "Adds # to # Physical Damage", "trade": {"ids": {"explicit": ["explicit.stat_1940865751"]}}, "id": "local_minimum_added_physical_damage"}"##,
    "\n",
    r##"{"ref": "# to maximum Mana", "trade": {"ids": {"explicit": ["explicit.stat_1050105434"]}}, "id": "base_maximum_mana"}"##,
    "\n",
    r##"{"ref": "#% increased Rarity of Items found", "trade": {"ids": {"explicit": ["explicit.stat_3917489142"]}}}"##,
);

fn wrap_mods(entries: &[&str]) -> String {
    format!("{{{}}}", entries.join(","))
}

/// A minimal eligible mod: item-domain prefix/suffix with a positive spawn
/// weight and a single-line text.
fn simple_mod(gen: &str, rlvl: u32, stat: &str, min: i64, max: i64) -> String {
    format!(
        r##"{{"domain":"item","generation_type":"{gen}","is_essence_only":false,"required_level":{rlvl},"spawn_weights":[{{"tag":"ring","weight":1}}],"stats":[{{"id":"{stat}","min":{min},"max":{max}}}],"text":"one line"}}"##
    )
}

#[test]
fn ladder_is_grouped_sorted_and_range_formatted() {
    let mods = wrap_mods(&[
        // Deliberately out of level order to prove sorting.
        &format!(r##""Strength2":{}"##, simple_mod("suffix", 11, "additional_strength", 9, 12)),
        &format!(r##""Strength1":{}"##, simple_mod("suffix", 1, "additional_strength", 5, 8)),
        &format!(r##""Strength3":{}"##, simple_mod("suffix", 22, "additional_strength", 13, 13)),
    ]);
    let affixes = parse_affixes_tiered(STATS, &mods);
    let s = affixes.iter().find(|a| a.text == "# to Strength").unwrap();
    assert_eq!(s.tiers.len(), 3);
    let ilvls: Vec<u32> = s.tiers.iter().map(|t| t.ilvl).collect();
    assert_eq!(ilvls, [1, 11, 22], "tiers sorted ascending by required level");
    assert_eq!(s.tiers[0].range, "5-8");
    assert_eq!(s.tiers[2].range, "13", "fixed roll collapses to a single number");
    assert_eq!(s.kind, AffixKind::Suffix, "family comes from the mods' generation_type");
    assert!(s.tiers.iter().all(|t| t.kind == AffixKind::Suffix), "every rung is classified");
    // Affixes the mods file does not cover keep an empty ladder.
    let life = affixes.iter().find(|a| a.text == "# to maximum Life").unwrap();
    assert!(life.tiers.is_empty());
    assert_eq!(life.kind, AffixKind::Other, "no ladder -> no family, never a guess");
    // As does the line with no internal id at all.
    let rarity = affixes.iter().find(|a| a.text.contains("Rarity")).unwrap();
    assert!(rarity.tiers.is_empty());
    // trade ids still parse as before.
    assert_eq!(s.trade_ids, ["explicit.stat_4080418644"]);
}

#[test]
fn ineligible_mods_contribute_no_tiers() {
    let unspawnable = r##"{"domain":"item","generation_type":"suffix","is_essence_only":false,"required_level":1,"spawn_weights":[{"tag":"default","weight":0}],"stats":[{"id":"additional_strength","min":1,"max":2}],"text":"t"}"##;
    let essence_only = r##"{"domain":"item","generation_type":"suffix","is_essence_only":true,"required_level":1,"spawn_weights":[{"tag":"ring","weight":1}],"stats":[{"id":"additional_strength","min":1,"max":2}],"text":"t"}"##;
    let unique_gen = r##"{"domain":"item","generation_type":"unique","is_essence_only":false,"required_level":1,"spawn_weights":[{"tag":"ring","weight":1}],"stats":[{"id":"additional_strength","min":1,"max":2}],"text":"t"}"##;
    let wrong_domain = r##"{"domain":"area","generation_type":"suffix","is_essence_only":false,"required_level":1,"spawn_weights":[{"tag":"ring","weight":1}],"stats":[{"id":"additional_strength","min":1,"max":2}],"text":"t"}"##;
    // The export nulls `text` on internal-only mods; those can't be verified
    // as a single display line and must not contribute (nor break parsing).
    let null_text = r##"{"domain":"item","generation_type":"suffix","is_essence_only":false,"required_level":1,"spawn_weights":[{"tag":"ring","weight":1}],"stats":[{"id":"additional_strength","min":1,"max":2}],"text":null}"##;
    let mods = wrap_mods(&[
        &format!(r##""A":{unspawnable}"##),
        &format!(r##""B":{essence_only}"##),
        &format!(r##""C":{unique_gen}"##),
        &format!(r##""D":{wrong_domain}"##),
        &format!(r##""E":{null_text}"##),
    ]);
    let affixes = parse_affixes_tiered(STATS, &mods);
    let s = affixes.iter().find(|a| a.text == "# to Strength").unwrap();
    assert!(s.tiers.is_empty(), "unspawnable/essence-only/unique/off-domain/textless mods are all excluded");
    assert_eq!(s.kind, AffixKind::Other, "an excluded mod's generation_type classifies nothing");
}

#[test]
fn generation_type_sets_the_affix_family() {
    let mods = wrap_mods(&[
        &format!(r##""IncreasedLife1":{}"##, simple_mod("prefix", 1, "base_maximum_life", 10, 19)),
        &format!(r##""IncreasedLife2":{}"##, simple_mod("prefix", 11, "base_maximum_life", 20, 29)),
        &format!(r##""Strength1":{}"##, simple_mod("suffix", 1, "additional_strength", 5, 8)),
    ]);
    let affixes = parse_affixes_tiered(STATS, &mods);
    let life = affixes.iter().find(|a| a.text == "# to maximum Life").unwrap();
    assert_eq!(life.kind, AffixKind::Prefix);
    assert_eq!(life.tiers.len(), 2);
    let str_ = affixes.iter().find(|a| a.text == "# to Strength").unwrap();
    assert_eq!(str_.kind, AffixKind::Suffix);
    // Untouched affixes stay Other, and the enum's default is Other too.
    let mana = affixes.iter().find(|a| a.text == "# to maximum Mana").unwrap();
    assert_eq!(mana.kind, AffixKind::Other);
    assert_eq!(AffixKind::default(), AffixKind::Other);
}

#[test]
fn a_ladder_that_disagrees_with_itself_has_no_family() {
    // Same ladder key group ("Mixed"), one rung a prefix and one a suffix:
    // the rungs keep what they are, but the affix refuses to pick.
    let mods = wrap_mods(&[
        &format!(r##""Mixed1":{}"##, simple_mod("prefix", 1, "additional_strength", 5, 8)),
        &format!(r##""Mixed2":{}"##, simple_mod("suffix", 11, "additional_strength", 9, 12)),
    ]);
    let affixes = parse_affixes_tiered(STATS, &mods);
    let s = affixes.iter().find(|a| a.text == "# to Strength").unwrap();
    assert_eq!(s.tiers.len(), 2, "the ladder itself is still built");
    assert_eq!(s.tiers[0].kind, AffixKind::Prefix);
    assert_eq!(s.tiers[1].kind, AffixKind::Suffix);
    assert_eq!(s.kind, AffixKind::Other, "a self-contradicting ladder is unclassified");
}

/// The tiering badge end to end: a ladder parsed out of the mods export, then
/// a rolled value placed on it by [`rollquality`].
#[test]
fn parsed_ladders_score_rolled_values() {
    let mods = wrap_mods(&[
        &format!(r##""Strength1":{}"##, simple_mod("suffix", 1, "additional_strength", 5, 8)),
        &format!(r##""Strength2":{}"##, simple_mod("suffix", 11, "additional_strength", 9, 12)),
        &format!(r##""Strength3":{}"##, simple_mod("suffix", 22, "additional_strength", 13, 20)),
    ]);
    let affixes = parse_affixes_tiered(STATS, &mods);
    let s = affixes.iter().find(|a| a.text == "# to Strength").unwrap();
    // Highest-ilvl rung is T1, matching how players read the badge.
    assert_eq!(rollquality::tier_of(&s.tiers, 20.0), Some(1));
    assert_eq!(rollquality::tier_of(&s.tiers, 6.0), Some(3));
    assert!((rollquality::score(&s.tiers, 20.0).unwrap() - 5.0).abs() < 1e-6);
    assert!(rollquality::score(&s.tiers, 5.0).unwrap().abs() < 1e-6);
    // Nothing to score against -> no number.
    let life = affixes.iter().find(|a| a.text == "# to maximum Life").unwrap();
    assert_eq!(rollquality::score(&life.tiers, 80.0), None);
}

#[test]
fn multi_stat_single_line_mod_maps_but_hybrids_do_not() {
    // Added phys: two stats, but EE2 only knows the min stat and the mod is a
    // single display line -> it belongs to the "Adds # to #" affix.
    let added_phys = r##"{"domain":"item","generation_type":"prefix","is_essence_only":false,"required_level":40,"spawn_weights":[{"tag":"sword","weight":1}],"stats":[{"id":"local_minimum_added_physical_damage","min":10,"max":15},{"id":"local_maximum_added_physical_damage","min":20,"max":30}],"text":"Adds (10-15) to (20-30) Physical Damage"}"##;
    // Hybrid life+mana: BOTH stats are EE2-known, and its ladder is neither
    // the pure-life nor the pure-mana ladder -> excluded from both.
    let hybrid = r##"{"domain":"item","generation_type":"prefix","is_essence_only":false,"required_level":30,"spawn_weights":[{"tag":"ring","weight":1}],"stats":[{"id":"base_maximum_life","min":10,"max":20},{"id":"base_maximum_mana","min":10,"max":20}],"text":"line one\nline two"}"##;
    let mods = wrap_mods(&[
        &format!(r##""AddedPhysicalDamage1":{added_phys}"##),
        &format!(r##""HybridLifeMana1":{hybrid}"##),
    ]);
    let affixes = parse_affixes_tiered(STATS, &mods);
    let phys = affixes.iter().find(|a| a.text.starts_with("Adds")).unwrap();
    assert_eq!(phys.tiers.len(), 1);
    assert_eq!(phys.tiers[0].range, "10-15, 20-30", "both stat ranges shown for one-line mods");
    assert_eq!(phys.tiers[0].ilvl, 40);
    let life = affixes.iter().find(|a| a.text == "# to maximum Life").unwrap();
    let mana = affixes.iter().find(|a| a.text == "# to maximum Mana").unwrap();
    assert!(life.tiers.is_empty() && mana.tiers.is_empty(), "hybrid ladders map to neither affix");
}

#[test]
fn parallel_ladders_keep_only_the_widest_one() {
    // Jewellery ladder: 2 tiers over 2 item-class tags. Staff ladder: 3 tiers
    // over 1 tag. The wider (more tags) jewellery ladder wins even though the
    // staff one is longer — the compact suffix describes the most items.
    let jew = |rlvl: u32, min: i64, max: i64| {
        format!(
            r##"{{"domain":"item","generation_type":"prefix","is_essence_only":false,"required_level":{rlvl},"spawn_weights":[{{"tag":"ring","weight":1}},{{"tag":"amulet","weight":1}}],"stats":[{{"id":"base_maximum_mana","min":{min},"max":{max}}}],"text":"t"}}"##
        )
    };
    let mods = wrap_mods(&[
        &format!(r##""IncreasedMana1":{}"##, jew(1, 10, 14)),
        &format!(r##""IncreasedMana2":{}"##, jew(6, 15, 24)),
        &format!(
            r##""IncreasedManaTwoHandWeapon1":{}"##,
            simple_mod("prefix", 1, "base_maximum_mana", 20, 28)
        ),
        &format!(
            r##""IncreasedManaTwoHandWeapon2_":{}"##,
            simple_mod("prefix", 6, "base_maximum_mana", 29, 48)
        ),
        &format!(
            r##""IncreasedManaTwoHandWeapon3":{}"##,
            simple_mod("prefix", 16, "base_maximum_mana", 49, 68)
        ),
    ]);
    let affixes = parse_affixes_tiered(STATS, &mods);
    let mana = affixes.iter().find(|a| a.text == "# to maximum Mana").unwrap();
    assert_eq!(mana.tiers.len(), 2, "only the dominant ladder's tiers are kept");
    assert_eq!(mana.tiers[1].range, "15-24", "ranges come from the jewellery ladder");
}

#[test]
fn join_records_spawn_weight_and_required_level() {
    // Life's base rung leads with a zero-weight entry: the recorded weight
    // must be the FIRST POSITIVE entry (poe2db's displayed number), not the
    // literal first.
    let life_base = r##"{"domain":"item","generation_type":"prefix","is_essence_only":false,"required_level":5,"spawn_weights":[{"tag":"boots","weight":0},{"tag":"default","weight":1000}],"stats":[{"id":"base_maximum_life","min":10,"max":19}],"text":"t"}"##;
    let mods = wrap_mods(&[
        // Strength rungs out of level order: the weight must follow the
        // lowest-required_level rung, not insertion order.
        &format!(r##""Strength2":{}"##, simple_mod("suffix", 11, "additional_strength", 9, 12)),
        &format!(r##""Strength1":{}"##, simple_mod("suffix", 1, "additional_strength", 5, 8)),
        &format!(r##""IncreasedLife1":{life_base}"##),
    ]);
    let affixes = parse_affixes_tiered(STATS, &mods);
    let s = affixes.iter().find(|a| a.text == "# to Strength").unwrap();
    assert_eq!(s.weight, Some(1), "simple_mod's ring:1 entry, from the rlvl-1 base rung");
    assert_eq!(s.required_level, Some(1), "the ladder's lowest rung");
    let life = affixes.iter().find(|a| a.text == "# to maximum Life").unwrap();
    assert_eq!(life.weight, Some(1000), "zero-weight entries are skipped");
    assert_eq!(life.required_level, Some(5));
    // No joined ladder -> no weight and no level, never a guess.
    let mana = affixes.iter().find(|a| a.text == "# to maximum Mana").unwrap();
    assert_eq!((mana.weight, mana.required_level), (None, None));
}

#[test]
fn plain_parse_and_garbage_mods_yield_empty_tiers() {
    for affix in parse_affixes(STATS) {
        assert!(affix.tiers.is_empty(), "parse_affixes never attaches tiers");
    }
    let affixes = parse_affixes_tiered(STATS, "not json at all");
    assert!(affixes.iter().all(|a| a.tiers.is_empty()), "unparseable mods degrade to no tiers");
    assert_eq!(affixes.len(), 5, "affix text itself still parses");
}


#[test]
fn a_rolled_line_finds_its_template() {
    use khaloni_poe2_core::refdata::normalize_mod_text as n;
    // The rolled value, the sign, and the case must all fall away so the
    // line and its template agree.
    assert_eq!(n("+23 to Accuracy Rating"), n("+# to Accuracy Rating"));
    assert_eq!(n("23 to Accuracy Rating"), n("# to Accuracy Rating"));
    assert_eq!(n("24% to Critical Damage Bonus"), n("#% to Critical Damage Bonus"));
    // Decimals and multi-number mods collapse the same way.
    assert_eq!(n("Adds 26 to 39 Physical Damage"), n("Adds # to # Physical Damage"));
    assert_eq!(n("1.45% increased Attack Speed"), n("#% increased Attack Speed"));
    // Genuinely different mods must not collide.
    assert_ne!(n("+23 to Accuracy Rating"), n("+# to maximum Life"));
}
