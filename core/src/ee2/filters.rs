//! EE2's default filter selection (`web/price-check/filters`): which item
//! filters and stat rows a price check opens with, each row's bounds, and
//! which rows start switched on.

use super::data::{plain_stat, Better, Ee2Data, Stat};
use super::parse::{
    cat, max_sockets, plain_matcher, Generation, ModInfo, ModType, ParsedItem, ParsedStat, Rarity, Roll,
    StatCalculated, StatRoll, StatSource,
};
use std::sync::Arc;

/// EE2's `searchStatRange` default: a roll is searched ten percent under
/// its value.
pub const SEARCH_STAT_RANGE: f64 = 10.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Toggle {
    pub value: f64,
    pub disabled: bool,
}

/// The item-level half of a search: what EE2 calls `ItemFilters`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ItemFilters {
    pub name: Option<String>,
    pub base_type: Option<String>,
    /// Category to search instead of the exact name/base, and whether that
    /// is switched off.
    pub category: Option<(String, bool)>,
    pub rarity: Option<&'static str>,
    /// (value, exact): a clean item excludes corrupted ones; `exact` pins
    /// the flag either way.
    pub corrupted: Option<(bool, bool)>,
    pub fractured_false: bool,
    pub mirrored: bool,
    pub sanctified: bool,
    pub item_level: Option<Toggle>,
    pub requires_level: Option<Toggle>,
    pub quality: Option<Toggle>,
    pub augment_sockets: Option<Toggle>,
    pub map_tier: Option<Toggle>,
    pub gem_level: Option<Toggle>,
    pub gem_level_max: Option<f64>,
    pub socket_number: Option<Toggle>,
    pub area_level: Option<Toggle>,
    pub awarded_ascendancy_points: Option<Toggle>,
    pub unidentified: Option<Toggle>,
    pub unidentified_tier: Option<Toggle>,
    /// (unrevealed mod count, disabled)
    pub veiled: Option<(f64, bool)>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FilterRoll {
    pub value: f64,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub default_min: f64,
    pub default_max: f64,
    pub bounds: Option<(f64, f64)>,
    pub dp: bool,
    pub trade_invert: bool,
}

/// One row of the price-check card: EE2's `StatFilter`.
#[derive(Debug, Clone, PartialEq)]
pub struct UiStat {
    /// `None` when the stat has no trade id under its mod type: the row
    /// cannot be searched.
    pub trade_ids: Option<Vec<String>>,
    pub stat_ref: String,
    pub text: String,
    pub tag: &'static str,
    pub sources: Vec<StatSource>,
    pub roll: Option<FilterRoll>,
    pub option: Option<f64>,
    pub oils: Option<Vec<usize>>,
    pub disabled: bool,
    pub hidden: Option<&'static str>,
}

pub struct Preset {
    pub id: &'static str,
    pub filters: ItemFilters,
    pub stats: Vec<UiStat>,
}

struct Ctx<'a> {
    item: &'a ParsedItem,
    data: &'a Ee2Data,
    search_in_range: f64,
    filters: Vec<UiStat>,
    stats_by_type: Vec<StatCalculated>,
}

fn max_useful_item_level(category: Option<&str>) -> f64 {
    match category {
        Some(cat::WAND | cat::STAFF) => 81.0,
        Some(cat::RELIC) => 80.0,
        Some(cat::TABLET | cat::JEWEL | cat::MAP) => 1.0,
        _ => 82.0,
    }
}

fn likely_finished_item(item: &ParsedItem) -> bool {
    item.rarity == Some(Rarity::Unique)
        || item.stats_by_type.iter().any(|c| c.type_ == ModType::Crafted)
        || (item.quality == Some(20.0) && !item.quality_typed)
        || !item.is_modifiable()
}

fn has_crafting_value(item: &ParsedItem) -> bool {
    item.is_modifiable()
        && (item.is_fractured
            || item.category.as_deref() == Some(cat::CLUSTER_JEWEL)
            || (item.category.as_deref() == Some(cat::JEWEL) && item.rarity == Some(Rarity::Magic))
            || item.item_level.is_some_and(|l| l >= max_useful_item_level(item.category.as_deref()) - 15.0)
            || item.augment_sockets.is_some_and(|s| s.current > s.normal)
            || item.quality.is_some_and(|q| q > 20.0))
}

/// (prefixes, suffixes, total) among the mods that take an affix slot.
fn explicit_modifier_count(item: &ParsedItem) -> (f64, f64, f64) {
    let random = item.new_mods.iter().filter(|m| m.info.type_.is_explicit_family());
    let (mut p, mut s) = (0.0, 0.0);
    for m in random {
        match m.info.generation {
            Some(Generation::Prefix) => p += 1.0,
            Some(Generation::Suffix) => s += 1.0,
            _ => {}
        }
    }
    (p, s, p + s)
}

pub fn create_presets(item: &ParsedItem, data: &Ee2Data) -> (Vec<Preset>, &'static str) {
    let c = item.category.as_deref();
    let exact_kind = matches!(
        c,
        Some(
            cat::FLASK | cat::RELIC | cat::TINCTURE | cat::MEMORY_LINE | cat::INVITATION
                | cat::HEIST_CONTRACT | cat::HEIST_BLUEPRINT | cat::SENTINEL | cat::TABLET | cat::WOMBGIFT
        )
    );
    let trial = c == Some(cat::CURRENCY)
        && item.trials.as_ref().is_some_and(|t| t.number_of_trials.is_some_and(|n| n != 0.0));
    if item.is_unidentified
        || item.rarity == Some(Rarity::Normal)
        || (item.info.craftable.is_none() && item.rarity != Some(Rarity::Unique))
        || (exact_kind && item.rarity != Some(Rarity::Unique))
        || trial
    {
        let preset = Preset {
            id: "filters.preset_exact",
            filters: create_filters(item, true),
            stats: create_exact_stat_filters(item, data),
        };
        return (vec![preset], "filters.preset_exact");
    }
    let pseudo = Preset {
        id: "filters.preset_pseudo",
        filters: create_filters(item, false),
        stats: init_ui_mod_filters(item, data),
    };
    if likely_finished_item(item) || !has_crafting_value(item) {
        return (vec![pseudo], "filters.preset_pseudo");
    }
    let base = Preset {
        id: "filters.preset_base_item",
        filters: create_filters(item, true),
        stats: create_exact_stat_filters(item, data),
    };
    (vec![pseudo, base], "filters.preset_pseudo")
}

fn category_has_trade_id(c: &str) -> bool {
    super::request::category_trade_id(c).is_some()
}

const ASCENDANCY_POINTS: [(&str, &[f64]); 2] =
    [("Inscribed Ultimatum", &[1.0, 60.0, 75.0]), ("Djinn Barya", &[1.0, 45.0, 60.0, 75.0])];

pub fn area_level_by_ascendancy_points(ref_name: &str, points: f64) -> f64 {
    let Some((_, levels)) = ASCENDANCY_POINTS.iter().find(|(n, _)| *n == ref_name) else { return 0.0 };
    if points < 1.0 {
        1.0
    } else if points > levels.len() as f64 {
        75.0
    } else {
        levels[points as usize - 1]
    }
}

fn ascendancy_points_by_area_level(ref_name: &str, area_level: f64) -> f64 {
    let Some((_, levels)) = ASCENDANCY_POINTS.iter().find(|(n, _)| *n == ref_name) else { return 0.0 };
    if area_level < 1.0 {
        return 1.0;
    }
    if area_level > 80.0 {
        return levels.len() as f64;
    }
    for i in (1..=levels.len()).rev() {
        if area_level >= levels[i - 1] {
            return i as f64;
        }
    }
    1.0
}

pub fn create_filters(item: &ParsedItem, exact: bool) -> ItemFilters {
    let mut f = ItemFilters::default();
    let c = item.category.as_deref();
    let on = |value: f64| Toggle { value, disabled: false };

    if c.is_some_and(cat::is_gem) && item.info.trade_tag.is_none() {
        f.base_type = Some(item.info.ref_name.clone());
        f.corrupted = Some((item.is_corrupted, false));
        if let Some(n) = item.gem_sockets {
            f.socket_number = Some(Toggle { value: n as f64, disabled: n < 3 });
        }
        if let Some(q) = item.quality.filter(|q| *q != 0.0) {
            f.quality = Some(Toggle { value: q, disabled: q < 16.0 });
        }
        let level = item.gem_level.unwrap_or(f64::NAN);
        // A level that could not be read compares false, which switches
        // the filter on with nothing to send.
        f.gem_level = Some(Toggle { value: level, disabled: level < 19.0 });
        return f;
    }
    if c == Some(cat::UNCUT_GEM) {
        f.base_type = Some(item.info.ref_name.clone());
        let level = item.gem_level.unwrap_or(f64::NAN);
        let range = if level < 18.0 && item.info.ref_name != "Uncut Support Gem" { 1.0 } else { 0.0 };
        f.gem_level = Some(on(level - range));
        if range != 0.0 {
            f.gem_level_max = Some(level + range);
        }
        return f;
    }
    if c == Some(cat::CURRENCY) && item.trials.is_some() {
        f.base_type = Some(item.info.ref_name.clone());
        let area = item.area_level.unwrap_or(f64::NAN);
        f.awarded_ascendancy_points = Some(on(ascendancy_points_by_area_level(&item.info.ref_name, area)));
        f.area_level = Some(on(area));
        return f;
    }
    if c == Some(cat::INVITATION) || c == Some(cat::DIVINATION_CARD) || c == Some(cat::CURRENCY) {
        f.base_type = Some(item.info.ref_name.clone());
        return f;
    }

    if c == Some(cat::MAP) {
        if let (Some(Rarity::Unique), Some(u)) = (item.rarity, &item.info.unique) {
            f.name = Some(item.info.ref_name.clone());
            f.base_type = Some(u.base.clone());
        } else {
            f.base_type = Some(item.info.ref_name.clone());
            f.category = Some((cat::MAP.to_string(), false));
        }
        f.map_tier = Some(on(item.map_tier.unwrap_or(f64::NAN)));
    } else if let (Some(Rarity::Unique), Some(u)) = (item.rarity, &item.info.unique) {
        f.name = Some(item.info.ref_name.clone());
        f.base_type = Some(u.base.clone());
    } else {
        f.base_type = Some(item.info.ref_name.clone());
        if let Some(cat_name) = c.filter(|c| category_has_trade_id(c)) {
            let disabled = match cat_name {
                cat::CLUSTER_JEWEL => true,
                cat::SANCTUM_RELIC | cat::CHARM => false,
                _ => exact,
            };
            f.category = Some((cat_name.to_string(), disabled));
        }
    }

    if let Some(q) = item.quality.filter(|q| *q != 0.0) {
        let useful = c.is_some_and(|c| {
            matches!(c, cat::FLASK | cat::CHARM | cat::TINCTURE) || cat::is_weapon(c) || cat::is_armour(c)
        });
        if q >= 20.0 && c == Some(cat::FLASK) {
            f.quality = Some(Toggle { value: q, disabled: q <= 20.0 });
        } else if q > 20.0 && useful {
            // A rare is most likely finished; its quality is not the point.
            f.quality = Some(Toggle { value: q, disabled: item.rarity == Some(Rarity::Rare) });
        } else if c == Some(cat::CHARM) {
            f.quality = Some(Toggle { value: q, disabled: q < 10.0 });
        }
    }

    if let Some(s) = item.augment_sockets.filter(|s| s.current != 0) {
        f.augment_sockets =
            Some(Toggle { value: s.current as f64, disabled: s.current <= s.normal && !item.is_corrupted });
    }

    if let (Some(level), Some(Rarity::Rare), false) = (item.requires_level, item.rarity, exact) {
        if level != 0.0 && level <= 75.0 && item.item_level.is_some_and(|l| l != 0.0 && l <= 75.0) {
            f.requires_level = Some(Toggle { value: level, disabled: true });
        }
    }

    let for_adorned_jewel =
        item.rarity == Some(Rarity::Magic) && matches!(c, Some(cat::JEWEL | cat::ABYSS_JEWEL));

    if !item.is_unmodifiable && c != Some(cat::MAP) && item.rarity.is_some() {
        f.corrupted = Some((item.is_corrupted, for_adorned_jewel));
    }

    f.rarity = if for_adorned_jewel {
        Some("magic")
    } else if item.rarity == Some(Rarity::Normal) && item.info.ref_name != "Idol of Estazunti" && exact {
        // A Chance Orb only works on a normal item.
        Some("normal")
    } else if item.rarity == Some(Rarity::Magic) && exact && c != Some(cat::TABLET) {
        Some("magic")
    } else if matches!(item.rarity, Some(Rarity::Normal | Rarity::Magic | Rarity::Rare)) {
        Some("nonunique")
    } else {
        None
    };

    f.mirrored = item.is_mirrored;
    f.sanctified = item.is_sanctified;
    f.fractured_false = !item.is_fractured && exact;

    if let Some(level) = item.item_level.filter(|l| *l != 0.0) {
        let cap = max_useful_item_level(c);
        if cap != 1.0
            && item.rarity != Some(Rarity::Unique)
            && !matches!(c, Some(cat::MAP | cat::HEIST_BLUEPRINT | cat::HEIST_CONTRACT | cat::MEMORY_LINE))
        {
            f.item_level = Some(Toggle {
                value: level.min(cap),
                disabled: !exact || matches!(c, Some(cat::FLASK | cat::CHARM)),
            });
        }
    }

    if item.is_unidentified {
        match item.unidentified_tier.filter(|t| *t != 0.0) {
            Some(tier) => f.unidentified_tier = Some(Toggle { value: tier, disabled: tier < 5.0 }),
            None => {
                f.unidentified = Some(Toggle { value: 1.0, disabled: item.rarity != Some(Rarity::Unique) })
            }
        }
    }

    if item.is_veiled {
        let count = item.new_mods.iter().filter(|m| m.info.type_ == ModType::Veiled).count() as f64;
        f.veiled = Some((count, item.rarity != Some(Rarity::Unique)));
        if item.rarity != Some(Rarity::Unique) {
            if let Some(l) = &mut f.item_level {
                l.disabled = false;
            }
        }
    }
    f
}

// --- rolls -----------------------------------------------------------------

fn decimal_places(value: f64, dp: bool) -> i32 {
    if !dp || value.abs() >= 10.0 {
        0
    } else if value.abs() < 2.3 {
        2
    } else {
        1
    }
}

fn round_roll(value: f64, dp: bool) -> f64 {
    let r = 10f64.powi(decimal_places(value, dp));
    (value * r).trunc() / r
}

fn percent_roll(value: f64, p: f64, ceil: bool, dp: bool) -> f64 {
    let res = value + (value.abs() * p) / 100.0;
    let r = 10f64.powi(decimal_places(value, dp));
    let scaled = (res + f64::EPSILON) * r;
    (if ceil { scaled.ceil() } else { scaled.floor() }) / r
}

fn percent_roll_delta(value: f64, delta: f64, p: f64, ceil: bool, dp: bool) -> f64 {
    let res = value + (delta * p) / 100.0;
    let r = 10f64.powi(decimal_places(value, dp));
    let scaled = (res + f64::EPSILON) * r;
    (if ceil { scaled.ceil() } else { scaled.floor() }) / r
}

fn sources_total(sources: &[StatSource]) -> Option<StatRoll> {
    if sources.len() == 1 {
        return sources[0].contributes;
    }
    let mut sum = StatRoll::default();
    for s in sources {
        let c = s.contributes.unwrap_or(StatRoll { value: 1.0, min: 1.0, max: 1.0, option: None });
        sum.value += c.value;
        sum.min += c.min;
        sum.max += c.max;
        sum.option = c.option;
    }
    Some(sum)
}

fn sign(v: f64) -> i8 {
    if v > 0.0 {
        1
    } else if v < 0.0 {
        -1
    } else {
        0
    }
}

/// The wording a summed roll is shown under, and whether that wording is
/// the stat's negated ("reduced") form.
fn translate_with_roll(calc: &StatCalculated, roll: Option<StatRoll>) -> (String, bool) {
    let matchers = &calc.stat.matchers;
    let plain = || matchers.iter().find(|m| m.value.is_none());
    let chosen = match roll {
        None => plain().or(matchers.first()),
        Some(r) => {
            let wanted = r.option.filter(|o| *o != 0.0).unwrap_or(r.value);
            let exact = if r.option.is_some_and(|o| o != 0.0) {
                matchers.iter().find(|m| m.value == Some(wanted))
            } else {
                matchers.iter().find(|m| m.value == Some(r.value))
            };
            exact
                .or_else(|| {
                    if sign(r.min) == sign(r.max) {
                        matchers.iter().find(|m| m.value.is_none() && m.negate == (r.value < 0.0))
                    } else {
                        matchers.iter().find(|m| m.value.is_none() && !m.negate)
                    }
                })
                .or_else(plain)
        }
    };
    match chosen {
        Some(m) => (m.string.clone(), m.negate),
        None => (format!("BUG_STAT_ID: {}", calc.stat.ref_), false),
    }
}

fn decode_oils(calc: &StatCalculated) -> Option<Vec<usize>> {
    if calc.type_ != ModType::Enchant {
        return None;
    }
    let encoded = calc.sources.first()?.stat.translation.oils.as_ref()?;
    Some(encoded.split(',').filter_map(|n| n.trim().parse().ok()).collect())
}

fn calculated_stat_to_filter(calc: &StatCalculated, mut percent: f64, item: &ParsedItem) -> UiStat {
    let stat = &calc.stat;
    let roll = sources_total(&calc.sources);
    let (text, negate) = translate_with_roll(calc, roll);
    let trade_ids = stat.ids(calc.type_.trade_key()).cloned();

    let mut filter = UiStat {
        trade_ids,
        stat_ref: stat.ref_.clone(),
        text,
        tag: calc.type_.as_str(),
        sources: calc.sources.clone(),
        roll: None,
        option: None,
        oils: decode_oils(calc),
        disabled: true,
        hidden: None,
    };
    if stat.option {
        if roll.is_some_and(|r| r.option == Some(r.value)) {
            filter.text = calc.sources[0].stat.translation.string.clone();
        }
        filter.tag = if calc.type_ == ModType::Enchant { "enchant" } else { "variant" };
        filter.option = calc.sources.first().and_then(|s| s.contributes).and_then(|c| c.option);
        filter.disabled = false;
    }

    let fixed_stats = item.info.unique.as_ref().and_then(|u| u.fixed_stats.as_ref());
    let named = |names: &[&str]| {
        calc.sources.iter().any(|s| s.modifier.name.as_deref().is_some_and(|n| names.contains(&n)))
    };
    match calc.type_ {
        ModType::Implicit => {
            let runemastered = item.info.unique.as_ref().is_some_and(|u| u.base.starts_with("Runemastered"));
            if let (true, Some(fixed)) = (runemastered, fixed_stats) {
                if !fixed.contains(&filter.stat_ref) {
                    filter.tag = "variant";
                }
            }
        }
        ModType::Explicit => {
            if item.rarity == Some(Rarity::Unique)
                && calc.sources.iter().any(|s| s.modifier.generation == Some(Generation::Mutated))
            {
                filter.tag = "mutated";
            }
            if let Some(fixed) = fixed_stats {
                if !fixed.contains(&filter.stat_ref) {
                    filter.tag = "variant";
                }
            } else if named(&["Subterranean", "of the Underground"]) {
                filter.tag = "explicit-delve";
            } else if named(&["Chosen", "of the Order"]) {
            } else if named(&[
                "Guatelitzi's", "Xopec's", "Topotante's", "Tacati's", "Matatl's", "of Matatl", "Citaqualotl's",
                "of Citaqualotl", "of Tacati", "of Guatelitzi", "of Puhuarte",
            ]) {
                filter.tag = "explicit-incursion";
            }
        }
        ModType::Enchant if calc.sources.iter().any(|s| s.modifier.generation == Some(Generation::Corrupted)) => {
            filter.tag = "corrupted";
        }
        _ => {}
    }

    if let Some(roll) = roll.filter(|r| filter.option.is_none() || r.option != Some(r.value)) {
        let better = stat.better;
        let magic_fixed = item.rarity == Some(Rarity::Magic) && (item.is_unmodifiable || !item.is_modifiable());
        if magic_fixed || stat.ref_ == "Has # Charm Slot" {
            percent = 0.0;
        } else if item.rarity == Some(Rarity::Unique)
            || (item.rarity == Some(Rarity::Magic)
                && matches!(item.category.as_deref(), Some(cat::JEWEL | cat::TABLET)))
            || calc.sources.iter().any(|s| s.modifier.tier == Some(1) && s.modifier.type_ == ModType::Fractured)
        {
            // A perfect roll is searched as itself: nothing rolls above it.
            let perfect = (better == Better::Positive && roll.value >= roll.max)
                || (better == Better::Negative && roll.value <= roll.min);
            if perfect {
                percent = 0.0;
            }
        }

        let dp = stat.dp
            || calc.sources.iter().any(|s| s.stat.stat.ref_ == stat.ref_ && s.stat.roll.is_some_and(|r| r.dp));
        let bounds = (percent_roll(roll.min, -0.0, false, dp), percent_roll(roll.max, 0.0, true, dp));
        let (mut dmin, mut dmax) = if better == Better::NotComparable {
            (roll.value, roll.value)
        } else if item.rarity == Some(Rarity::Unique) {
            let delta = roll.max - roll.min;
            (
                percent_roll_delta(roll.value, delta, -percent, false, dp),
                percent_roll_delta(roll.value, delta, percent, true, dp),
            )
        } else {
            (percent_roll(roll.value, -percent, false, dp), percent_roll(roll.value, percent, true, dp))
        };
        dmin = dmin.max(bounds.0);
        dmax = dmax.min(bounds.1);

        let mut fr = FilterRoll {
            value: round_roll(roll.value, dp),
            min: None,
            max: None,
            default_min: dmin,
            default_max: dmax,
            bounds: (item.rarity == Some(Rarity::Unique)
                && roll.min != roll.max
                && better != Better::NotComparable)
                .then_some(bounds),
            dp,
            trade_invert: stat.inverted,
        };
        let reload = stat.ids("pseudo").is_some_and(|ids| ids.len() == 1 && ids[0] == "item.reload_time");
        fill_min_max(&mut fr, if reload { Better::Negative } else { better });
        if negate {
            // A "reduced" wording shows and searches the mirrored numbers.
            fr.trade_invert = !fr.trade_invert;
            let old = fr;
            fr.bounds = old.bounds.map(|b| (-b.1, -b.0));
            fr.default_min = -old.default_max;
            fr.default_max = -old.default_min;
            fr.value = -old.value;
            fr.min = old.max.map(|v| -v);
            fr.max = old.min.map(|v| -v);
        }
        filter.roll = Some(fr);
    }

    hide_not_variable_stat(&mut filter, item);
    filter
}

/// One modifier line as a row of its own, worded and bounded the way EE2
/// words and bounds its rows under the same preset: the ten percent under
/// the roll a pseudo preset searches, the two an exact one does (none on a
/// tablet), a normal item's whole range. For the lines EE2 lists no row of
/// their own for, so a row the user adds is searched like the ones EE2
/// opened with.
pub(crate) fn line_filter(calc: &StatCalculated, item: &ParsedItem, exact: bool) -> UiStat {
    let percent = if exact {
        if item.category.as_deref() == Some(cat::TABLET) {
            0.0
        } else {
            SEARCH_STAT_RANGE.min(2.0)
        }
    } else if item.rarity == Some(Rarity::Normal) {
        100.0
    } else {
        SEARCH_STAT_RANGE
    };
    calculated_stat_to_filter(calc, percent, item)
}

fn fill_min_max(roll: &mut FilterRoll, better: Better) {
    match better {
        Better::Positive => roll.min = Some(roll.default_min),
        Better::Negative => roll.max = Some(roll.default_max),
        Better::NotComparable => {
            roll.min = Some(roll.default_min);
            roll.max = Some(roll.default_max);
        }
    }
}

/// A unique's fixed lines say nothing a search by its name does not.
fn hide_not_variable_stat(filter: &mut UiStat, item: &ParsedItem) {
    if item.rarity != Some(Rarity::Unique) {
        return;
    }
    if filter.tag == "implicit" && item.category.as_deref() == Some(cat::JEWEL) {
        return;
    }
    let from_enchant = filter.sources.iter().any(|s| s.modifier.type_ == ModType::Enchant);
    if !matches!(filter.tag, "implicit" | "explicit" | "pseudo") || (filter.tag == "pseudo" && from_enchant) {
        return;
    }
    if filter.stat_ref == "# uses remaining" {
        return;
    }
    match &mut filter.roll {
        None => filter.hidden = Some("filters.hide_const_roll"),
        Some(roll) if roll.bounds.is_none() => {
            roll.min = None;
            roll.max = None;
            filter.hidden = Some("filters.hide_const_roll");
        }
        _ => {}
    }
}

// --- item properties ---------------------------------------------------------

struct StatRefs {
    flat: &'static [&'static str],
    incr: &'static [&'static str],
}

const Q_ARMOUR: StatRefs = StatRefs {
    flat: &["# to Armour"],
    incr: &[
        "#% increased Armour",
        "#% increased Armour and Energy Shield",
        "#% increased Armour and Evasion",
        "#% increased Armour, Evasion and Energy Shield",
    ],
};
const Q_EVASION: StatRefs = StatRefs {
    flat: &["# to Evasion Rating"],
    incr: &[
        "#% increased Evasion Rating",
        "#% increased Armour and Evasion",
        "#% increased Evasion and Energy Shield",
        "#% increased Armour, Evasion and Energy Shield",
    ],
};
const Q_ENERGY_SHIELD: StatRefs = StatRefs {
    flat: &["# to maximum Energy Shield"],
    incr: &[
        "#% increased Energy Shield",
        "#% increased Armour and Energy Shield",
        "#% increased Evasion and Energy Shield",
        "#% increased Armour, Evasion and Energy Shield",
    ],
};
const Q_PHYSICAL: StatRefs =
    StatRefs { flat: &["Adds # to # Physical Damage"], incr: &["#% increased Physical Damage"] };
const Q_WARD: StatRefs = StatRefs { flat: &["# to maximum Runic Ward"], incr: &["#% increased Runic Ward"] };

struct PropBase {
    incr: StatRoll,
    flat: StatRoll,
    sources: Vec<StatSource>,
}

fn calc_prop_base(refs: &StatRefs, item: &ParsedItem) -> PropBase {
    let mut base = PropBase { incr: StatRoll::default(), flat: StatRoll::default(), sources: Vec::new() };
    for calc in &item.stats_by_type {
        let r = calc.stat.ref_.as_str();
        let total = if refs.flat.contains(&r) {
            &mut base.flat
        } else if refs.incr.contains(&r) {
            &mut base.incr
        } else {
            continue;
        };
        let roll = sources_total(&calc.sources).unwrap_or(StatRoll { value: f64::NAN, ..Default::default() });
        total.value += roll.value;
        total.min += roll.min;
        total.max += roll.max;
        base.sources.extend(calc.sources.iter().cloned());
    }
    base
}

/// `(figure, increased %, more %) -> figure`
type Scale = fn(f64, f64, f64) -> f64;

fn calc_flat(total: f64, incr: f64, more: f64) -> f64 {
    total / (1.0 + more / 100.0) / (1.0 + incr / 100.0)
}

fn calc_increased(flat: f64, incr: f64, more: f64) -> f64 {
    flat * (1.0 + incr / 100.0) * (1.0 + more / 100.0)
}

/// The figure as it would read at 20% quality (or the item's own when it
/// can no longer be raised), with the range its local mods could roll.
fn prop_at_20_quality(total: f64, refs: &StatRefs, item: &ParsedItem) -> (StatRoll, Vec<StatSource>) {
    let b = calc_prop_base(refs, item);
    let own_quality = item.quality.unwrap_or(0.0);
    let base = calc_flat(total, b.incr.value, own_quality) - b.flat.value;
    let quality = if item.is_modifiable() { own_quality.max(20.0) } else { own_quality };
    let roll = StatRoll {
        value: calc_increased(base + b.flat.value, b.incr.value, quality),
        min: calc_increased(base + b.flat.min, b.incr.min, quality),
        max: calc_increased(base + b.flat.max, b.incr.max, quality),
        option: None,
    };
    (roll, strip_contributions(b.sources))
}

fn strip_contributions(sources: Vec<StatSource>) -> Vec<StatSource> {
    sources.into_iter().map(|s| StatSource { contributes: None, ..s }).collect()
}

fn calc_prop_bounds(total: f64, refs: &StatRefs, item: &ParsedItem, inverted: bool) -> (StatRoll, Vec<StatSource>) {
    let b = calc_prop_base(refs, item);
    // Reload time falls as attack speed rises, so its rebuild is mirrored.
    let (down, up): (Scale, Scale) =
        if inverted { (calc_increased, calc_flat) } else { (calc_flat, calc_increased) };
    let base = down(total, b.incr.value, 0.0) - b.flat.value;
    let roll = StatRoll {
        value: up(base + b.flat.value, b.incr.value, 0.0),
        min: up(base + b.flat.min, b.incr.min, 0.0),
        max: up(base + b.flat.max, b.incr.max, 0.0),
        option: None,
    };
    let sources = if !refs.incr.is_empty() && !refs.flat.is_empty() {
        strip_contributions(b.sources)
    } else {
        b.sources
    };
    (roll, sources)
}

struct Prop {
    ref_: &'static str,
    trade_id: &'static str,
    roll: StatRoll,
    sources: Vec<StatSource>,
    dp: bool,
    disabled: bool,
    hidden: Option<&'static str>,
}

fn prop_to_filter(p: Prop, ctx: &Ctx) -> UiStat {
    let stat: Arc<Stat> = Arc::new(plain_stat(p.ref_, "pseudo", p.trade_id));
    let source = StatSource {
        modifier: ModInfo::bare(ModType::Pseudo),
        stat: ParsedStat {
            stat: stat.clone(),
            translation: plain_matcher(p.ref_),
            roll: Some(Roll {
                unscalable: false,
                dp: p.dp,
                value: p.roll.value,
                min: p.roll.min,
                max: p.roll.max,
                option: None,
            }),
            lines: Vec::new(),
            // A property is no line of the item; this never matches one.
            ordinal: usize::MAX,
        },
        contributes: Some(p.roll),
    };
    let calc = StatCalculated { stat, type_: ModType::Pseudo, sources: vec![source] };
    let mut filter = calculated_stat_to_filter(&calc, ctx.search_in_range, ctx.item);
    filter.tag = "property";
    filter.sources = p.sources;
    filter.disabled = p.disabled;
    if p.hidden.is_some() {
        filter.hidden = p.hidden;
    }
    filter
}

fn remove_used_stats(ctx: &mut Ctx, used: &[&str]) {
    ctx.stats_by_type.retain(|m| !used.contains(&m.stat.ref_.as_str()));
}

fn present(v: Option<f64>) -> Option<f64> {
    v.filter(|x| *x != 0.0 && !x.is_nan())
}

fn armour_props(ctx: &mut Ctx) {
    let item = ctx.item;
    let defences = [
        (item.armour_ar, &Q_ARMOUR, "Armour: #", "item.armour"),
        (item.armour_ev, &Q_EVASION, "Evasion Rating: #", "item.evasion_rating"),
        (item.armour_es, &Q_ENERGY_SHIELD, "Energy Shield: #", "item.energy_shield"),
    ];
    for (total, refs, ref_, trade_id) in defences {
        if let Some(total) = present(total) {
            let (roll, sources) = prop_at_20_quality(total, refs, item);
            let f = prop_to_filter(Prop { ref_, trade_id, roll, sources, dp: false, disabled: false, hidden: None }, ctx);
            ctx.filters.push(f);
        }
    }
    if let Some(block) = present(item.armour_block) {
        let refs = StatRefs { flat: &[], incr: &["#% increased Block chance"] };
        let (roll, sources) = calc_prop_bounds(block, &refs, item, false);
        let f = prop_to_filter(
            Prop { ref_: "Block: #%", trade_id: "item.block", roll, sources, dp: false, disabled: true, hidden: None },
            ctx,
        );
        ctx.filters.push(f);
    }
    if let Some(ward) = present(item.armour_rw) {
        let (roll, sources) = calc_prop_bounds(ward, &Q_WARD, item, false);
        let f = prop_to_filter(
            Prop { ref_: "Runic Ward: #", trade_id: "item.runic_ward", roll, sources, dp: false, disabled: true, hidden: None },
            ctx,
        );
        ctx.filters.push(f);
    }
    let any = [item.armour_ar, item.armour_ev, item.armour_es, item.armour_rw, item.armour_block]
        .into_iter()
        .any(|v| present(v).is_some());
    if any {
        let mut used: Vec<&str> = vec!["#% increased Block chance"];
        for refs in [&Q_ARMOUR, &Q_EVASION, &Q_ENERGY_SHIELD, &Q_WARD] {
            used.extend(refs.flat);
            used.extend(refs.incr);
        }
        remove_used_stats(ctx, &used);
    }
}

fn times(a: StatRoll, b: StatRoll) -> StatRoll {
    StatRoll { value: a.value * b.value, min: a.min * b.min, max: a.max * b.max, option: None }
}

fn weapon_props(ctx: &mut Ctx) {
    let item = ctx.item;
    let aps_refs = StatRefs { flat: &[], incr: &["#% increased Attack Speed"] };
    let (attack_speed, aps_sources) = calc_prop_bounds(item.weapon_as.unwrap_or(0.0), &aps_refs, item, false);
    let (phys, phys_sources) = prop_at_20_quality(item.weapon_physical.unwrap_or(0.0), &Q_PHYSICAL, item);
    let pdps = times(phys, attack_speed);
    let ele_refs = StatRefs {
        flat: &["Adds # to # Lightning Damage", "Adds # to # Cold Damage", "Adds # to # Fire Damage"],
        incr: &[],
    };
    let (ele, ele_sources) = calc_prop_bounds(item.weapon_elemental.unwrap_or(0.0), &ele_refs, item, false);
    let edps = times(ele, attack_speed);
    let dps = StatRoll { value: pdps.value + edps.value, min: pdps.min + edps.min, max: pdps.max + edps.max, option: None };

    if present(item.weapon_elemental).is_some() {
        let mut sources = ele_sources.clone();
        sources.extend(phys_sources.iter().cloned());
        let f = prop_to_filter(
            Prop { ref_: "Total DPS: #", trade_id: "item.total_dps", roll: dps, sources, dp: false, disabled: false, hidden: None },
            ctx,
        );
        ctx.filters.push(f);
        let minor = edps.value / dps.value < 0.15;
        let f = prop_to_filter(
            Prop {
                ref_: "Elemental DPS: #",
                trade_id: "item.elemental_dps",
                roll: edps,
                sources: ele_sources,
                dp: false,
                disabled: minor,
                hidden: minor.then_some("filters.hide_ele_dps"),
            },
            ctx,
        );
        ctx.filters.push(f);
    }
    if present(item.weapon_physical).is_some() {
        let minor = pdps.value / dps.value < 0.67;
        let f = prop_to_filter(
            Prop {
                ref_: "Physical DPS: #",
                trade_id: "item.physical_dps",
                roll: pdps,
                sources: phys_sources,
                dp: false,
                disabled: !is_pdps_important(item) || minor,
                hidden: minor.then_some("filters.hide_phys_dps"),
            },
            ctx,
        );
        ctx.filters.push(f);
    }
    if present(item.weapon_as).is_some() {
        let f = prop_to_filter(
            Prop {
                ref_: "Attacks per Second: #",
                trade_id: "item.aps",
                roll: attack_speed,
                sources: aps_sources,
                dp: true,
                disabled: true,
                hidden: None,
            },
            ctx,
        );
        ctx.filters.push(f);
    }
    if let Some(crit) = present(item.weapon_crit) {
        let refs = StatRefs { flat: &["#% to Critical Hit Chance"], incr: &["#% increased Critical Hit Chance"] };
        let (roll, sources) = calc_prop_bounds(crit, &refs, item, false);
        let f = prop_to_filter(
            Prop { ref_: "Critical Hit Chance: #%", trade_id: "item.crit", roll, sources, dp: true, disabled: true, hidden: None },
            ctx,
        );
        ctx.filters.push(f);
    }
    if let Some(reload) = present(item.weapon_reload) {
        let (roll, sources) = calc_prop_bounds(reload, &aps_refs, item, true);
        let f = prop_to_filter(
            Prop { ref_: "Reload Time: #", trade_id: "item.reload_time", roll, sources, dp: true, disabled: true, hidden: None },
            ctx,
        );
        ctx.filters.push(f);
    }
    if let Some(spirit) = present(item.weapon_spirit) {
        let refs = StatRefs { flat: &[], incr: &["#% increased Spirit"] };
        let (roll, sources) = calc_prop_bounds(spirit, &refs, item, false);
        let f = prop_to_filter(
            Prop { ref_: "Spirit: #%", trade_id: "item.spirit", roll, sources, dp: false, disabled: false, hidden: None },
            ctx,
        );
        ctx.filters.push(f);
    }
    let any = [item.weapon_as, item.weapon_crit, item.weapon_elemental, item.weapon_physical, item.weapon_spirit]
        .into_iter()
        .any(|v| present(v).is_some());
    if any {
        remove_used_stats(
            ctx,
            &[
                "Adds # to # Physical Damage",
                "#% increased Physical Damage",
                "#% increased Attack Speed",
                "#% to Critical Hit Chance",
                "Adds # to # Lightning Damage",
                "Adds # to # Cold Damage",
                "Adds # to # Fire Damage",
                "#% increased Spirit",
            ],
        );
    }
}

fn is_pdps_important(item: &ParsedItem) -> bool {
    matches!(
        item.category.as_deref(),
        Some(
            cat::ONE_HAND_AXE | cat::TWO_HAND_AXE | cat::ONE_HAND_SWORD | cat::TWO_HAND_SWORD
                | cat::ONE_HAND_MACE | cat::TWO_HAND_MACE | cat::BOW | cat::WARSTAFF | cat::CROSSBOW
                | cat::SPEAR | cat::FLAIL
        )
    )
}

fn map_props(ctx: &mut Ctx) {
    let item = ctx.item;
    let none = StatRefs { flat: &[], incr: &[] };
    let props = [
        (item.map_revives, "Revives Available: #", "item.map_revives", Some("filters.hide_revives")),
        (item.map_pack_size, "Monster Pack Size: #", "item.map_pack_size", None),
        (item.map_magic_monsters, "Magic Monsters: #", "item.map_magic_monsters", None),
        (item.map_rare_monsters, "Rare Monsters: #", "item.map_rare_monsters", None),
        (item.map_drop_chance, "Waystone Drop Chance: #%", "item.map_drop_chance", None),
        (item.map_item_rarity, "Map Item Rarity: #%", "item.map_item_rarity", None),
        // The trade site reuses the retired rare/magic monster filters for
        // monster rarity and effectiveness.
        (item.map_monster_rarity, "Monster Rarity: #%", "item.map_rare_monsters", None),
        (item.map_effectiveness, "Monster Effectiveness: #%", "item.map_magic_monsters", None),
    ];
    for (value, ref_, trade_id, hidden) in props {
        if let Some(v) = present(value) {
            let (roll, sources) = calc_prop_bounds(v, &none, item, false);
            let f = prop_to_filter(Prop { ref_, trade_id, roll, sources, dp: false, disabled: true, hidden }, ctx);
            ctx.filters.push(f);
        }
    }
}

fn filter_item_prop(ctx: &mut Ctx) {
    let c = ctx.item.category.as_deref().unwrap_or("");
    if cat::is_armour(c) {
        armour_props(ctx);
    }
    if cat::is_weapon(c) {
        weapon_props(ctx);
    }
    if c == cat::MAP {
        map_props(ctx);
    }
}

// --- pseudo totals -------------------------------------------------------------

struct Resistance {
    ref_: &'static str,
    elements: &'static [usize],
    chaos: bool,
}

const FIRE: usize = 0;
const COLD: usize = 1;
const LIGHTNING: usize = 2;

const RESISTANCES: [Resistance; 12] = [
    Resistance { ref_: "#% to All Resistances", elements: &[FIRE, COLD, LIGHTNING], chaos: true },
    Resistance { ref_: "#% to all Elemental Resistances", elements: &[FIRE, COLD, LIGHTNING], chaos: false },
    Resistance { ref_: "#% to Fire Resistance", elements: &[FIRE], chaos: false },
    Resistance { ref_: "#% to Cold Resistance", elements: &[COLD], chaos: false },
    Resistance { ref_: "#% to Lightning Resistance", elements: &[LIGHTNING], chaos: false },
    Resistance { ref_: "#% to Fire and Lightning Resistances", elements: &[FIRE, LIGHTNING], chaos: false },
    Resistance { ref_: "#% to Fire and Cold Resistances", elements: &[FIRE, COLD], chaos: false },
    Resistance { ref_: "#% to Cold and Lightning Resistances", elements: &[COLD, LIGHTNING], chaos: false },
    Resistance { ref_: "#% to Chaos Resistance", elements: &[], chaos: true },
    Resistance { ref_: "#% to Fire and Chaos Resistances", elements: &[FIRE], chaos: true },
    Resistance { ref_: "#% to Cold and Chaos Resistances", elements: &[COLD], chaos: true },
    Resistance { ref_: "#% to Lightning and Chaos Resistances", elements: &[LIGHTNING], chaos: true },
];

const STR: usize = 0;
const DEX: usize = 1;
const INT: usize = 2;

const ATTRIBUTES: [(&str, &[usize]); 7] = [
    ("# to all Attributes", &[STR, DEX, INT]),
    ("# to Strength", &[STR]),
    ("# to Dexterity", &[DEX]),
    ("# to Intelligence", &[INT]),
    ("# to Strength and Intelligence", &[STR, INT]),
    ("# to Strength and Dexterity", &[STR, DEX]),
    ("# to Dexterity and Intelligence", &[DEX, INT]),
];

#[derive(Clone, Copy, PartialEq)]
enum Mutate {
    None,
    HideAllRes,
    Chaos,
    MovementSpeed,
}

struct PseudoRule {
    pseudo: &'static str,
    group: Option<&'static str>,
    disabled: Option<bool>,
    /// (ref, multiplier, required)
    stats: Vec<(&'static str, f64, bool)>,
    mutate: Mutate,
}

fn pseudo_rules() -> Vec<PseudoRule> {
    let res = |keep: &dyn Fn(&Resistance) -> bool, weighted: bool| -> Vec<(&'static str, f64, bool)> {
        RESISTANCES
            .iter()
            .filter(|r| keep(r))
            .map(|r| (r.ref_, if weighted { r.elements.len() as f64 } else { 1.0 }, false))
            .collect()
    };
    let attr = |a: usize, multiplier: f64| -> Vec<(&'static str, f64, bool)> {
        ATTRIBUTES.iter().filter(|(_, attrs)| attrs.contains(&a)).map(|(r, _)| (*r, multiplier, false)).collect()
    };
    let rule = |pseudo, group, disabled, stats, mutate| PseudoRule { pseudo, group, disabled, stats, mutate };
    let with_required = |first: &'static str, mut rest: Vec<(&'static str, f64, bool)>| {
        rest.insert(0, (first, 1.0, true));
        rest
    };
    vec![
        rule(
            "#% total to all Elemental Resistances",
            Some("to_all_res"),
            Some(true),
            res(&|r| !r.elements.is_empty(), false),
            Mutate::HideAllRes,
        ),
        rule("#% total Elemental Resistance", None, Some(false), res(&|r| !r.elements.is_empty(), true), Mutate::None),
        rule("#% total to Fire Resistance", Some("to_x_ele_res"), None, res(&|r| r.elements.contains(&FIRE), false), Mutate::None),
        rule("#% total to Cold Resistance", Some("to_x_ele_res"), None, res(&|r| r.elements.contains(&COLD), false), Mutate::None),
        rule(
            "#% total to Lightning Resistance",
            Some("to_x_ele_res"),
            None,
            res(&|r| r.elements.contains(&LIGHTNING), false),
            Mutate::None,
        ),
        rule("#% total to Chaos Resistance", None, None, res(&|r| r.chaos, false), Mutate::Chaos),
        rule("+# total to all Attributes", Some("to_all_attrs"), None, vec![("# to all Attributes", 1.0, false)], Mutate::None),
        rule("+# total to Strength", Some("to_x_attr"), None, attr(STR, 1.0), Mutate::None),
        rule("+# total to Dexterity", Some("to_x_attr"), None, attr(DEX, 1.0), Mutate::None),
        rule("+# total to Intelligence", Some("to_x_attr"), None, attr(INT, 1.0), Mutate::None),
        rule("+# total maximum Life", None, Some(false), with_required("# to maximum Life", attr(STR, 2.0)), Mutate::None),
        rule("+# total maximum Mana", None, None, with_required("# to maximum Mana", attr(INT, 2.0)), Mutate::None),
        rule(
            "#% total increased maximum Energy Shield",
            None,
            None,
            vec![("#% increased maximum Energy Shield", 1.0, false)],
            Mutate::None,
        ),
        rule("+# total maximum Energy Shield", None, None, vec![("# to maximum Energy Shield", 1.0, false)], Mutate::None),
        rule(
            "#% increased Movement Speed",
            None,
            None,
            vec![("#% increased Movement Speed", 1.0, false)],
            Mutate::MovementSpeed,
        ),
        rule(
            "# uses remaining",
            None,
            Some(false),
            [
                "Adds Irradiated to a Map \n# use remaining",
                "Adds Ritual Altars to a Map \n# use remaining",
                "Adds a Kalguuran Expedition to a Map \n# use remaining",
                "Adds a Mirror of Delirium to a Map \n# use remaining",
                "Adds an Otherworldy Breach to a Map \n# use remaining",
                "Empowers the Map Boss of a Map \n# use remaining",
                "Adds Abysses to a Map \n# use remaining",
            ]
            .into_iter()
            .map(|r| (r, 1.0, false))
            .collect(),
            Mutate::None,
        ),
    ]
}

fn filter_pseudo(ctx: &mut Ctx) {
    let rules = pseudo_rules();
    // Filters are tracked by position; removals are applied at the end so
    // the positions stay valid while the rules run.
    let first = ctx.filters.len();
    let mut made: Vec<(UiStat, Option<&'static str>)> = Vec::new();

    for rule in &rules {
        let mut sources = Vec::new();
        for calc in &ctx.stats_by_type {
            let Some((_, multi, _)) = rule.stats.iter().find(|(r, _, _)| *r == calc.stat.ref_) else { continue };
            for source in &calc.sources {
                // A listed stat always carries a roll; one without cannot
                // be summed and EE2 stops on it.
                let c = source.contributes.unwrap_or(StatRoll { value: f64::NAN, min: f64::NAN, max: f64::NAN, option: None });
                sources.push(StatSource {
                    contributes: Some(StatRoll { value: c.value * multi, min: c.min * multi, max: c.max * multi, option: None }),
                    ..source.clone()
                });
            }
        }
        if sources.is_empty() {
            continue;
        }
        let missing_required = rule
            .stats
            .iter()
            .any(|(r, _, required)| *required && !sources.iter().any(|s| s.stat.stat.ref_ == *r));
        if missing_required {
            continue;
        }
        let Some(stat) = ctx.data.stats.by_ref(rule.pseudo) else { continue };
        let calc = StatCalculated { stat, type_: ModType::Pseudo, sources };
        let mut filter = calculated_stat_to_filter(&calc, ctx.search_in_range, ctx.item);
        filter.disabled = rule.disabled.unwrap_or(true);
        match rule.mutate {
            Mutate::None => {}
            Mutate::HideAllRes => filter.hidden = Some("filters.hide_total_all_res"),
            Mutate::Chaos => {
                let rune_only = filter.sources.len() == 1
                    && matches!(filter.sources[0].modifier.type_, ModType::Augment | ModType::AddedAugment);
                if rune_only {
                    filter.hidden = Some("filters.hide_crafted_chaos");
                } else {
                    filter.disabled = false;
                }
            }
            Mutate::MovementSpeed => {
                let implicit_only =
                    filter.sources.len() == 1 && filter.sources[0].modifier.type_ == ModType::Implicit;
                if !implicit_only {
                    filter.disabled = false;
                }
            }
        }
        made.push((filter, rule.group));
    }

    ctx.stats_by_type
        .retain(|m| !rules.iter().any(|rule| rule.stats.iter().any(|(r, _, _)| *r == m.stat.ref_)));

    let group = |made: &[(UiStat, Option<&'static str>)], name: &str| -> Vec<usize> {
        made.iter().enumerate().filter(|(_, (_, g))| *g == Some(name)).map(|(i, _)| i).collect()
    };
    let value = |made: &[(UiStat, Option<&'static str>)], i: usize| made[i].0.roll.map_or(f64::NAN, |r| r.value);
    let mut removed: Vec<usize> = Vec::new();

    // Of the per-element totals only a single highest survives, hidden;
    // "total Elemental Resistance" already speaks for the rest.
    let mut res = group(&made, "to_x_ele_res");
    if !res.is_empty() {
        res.sort_by(|a, b| value(&made, *b).partial_cmp(&value(&made, *a)).unwrap_or(std::cmp::Ordering::Equal));
        let tie = res.len() > 1 && value(&made, res[0]) == value(&made, res[1]);
        let keep = (!tie).then_some(res[0]);
        if let Some(k) = keep {
            made[k].0.hidden = Some("filters.hide_ele_res");
        }
        removed.extend(res.iter().filter(|i| Some(**i) != keep));
    }

    let mut attrs = group(&made, "to_x_attr");
    if !attrs.is_empty() {
        attrs.sort_by(|a, b| value(&made, *b).partial_cmp(&value(&made, *a)).unwrap_or(std::cmp::Ordering::Equal));
        if attrs.len() == 3 {
            let to_all = group(&made, "to_all_attrs");
            let all_equal = attrs.iter().all(|i| value(&made, *i) == value(&made, attrs[0]));
            if all_equal && !to_all.is_empty() {
                removed.extend(&attrs);
            } else {
                removed.extend(&to_all);
                if value(&made, attrs[2]) / value(&made, attrs[0]) < 0.3 {
                    if value(&made, attrs[1]) == value(&made, attrs[2]) {
                        made[attrs[1]].0.hidden = Some("hide_attr_same_2nd_n_3rd");
                        made[attrs[2]].0.hidden = Some("hide_attr_same_2nd_n_3rd");
                    } else {
                        made[attrs[2]].0.hidden = Some("hide_attr_smallest_total");
                    }
                }
            }
        }
    }

    // "to all Elemental Resistances" is only as high as the weakest
    // element, so it is rebuilt from the sources rather than summed.
    if let Some(&idx) = group(&made, "to_all_res").first() {
        let mut all = StatRoll::default();
        let mut elements = [StatRoll::default(); 3];
        for source in &made[idx].0.sources {
            let Some(info) = RESISTANCES.iter().find(|r| r.ref_ == source.stat.stat.ref_) else { continue };
            let c = source.contributes.unwrap_or_default();
            let targets: Vec<&mut StatRoll> = if info.elements.len() == 3 {
                vec![&mut all]
            } else {
                elements.iter_mut().enumerate().filter(|(i, _)| info.elements.contains(i)).map(|(_, e)| e).collect()
            };
            for t in targets {
                t.value += c.value;
                t.min += c.min;
                t.max += c.max;
            }
        }
        let mut weakest = FIRE;
        if elements[COLD].value < elements[weakest].value {
            weakest = COLD;
        }
        if elements[LIGHTNING].value < elements[weakest].value {
            weakest = LIGHTNING;
        }
        let total = StatRoll {
            value: all.value + elements[weakest].value,
            min: all.min + elements[weakest].min,
            max: all.max + elements[weakest].max,
            option: None,
        };
        if total.value == 0.0 {
            removed.push(idx);
        } else {
            let bounds = (percent_roll(total.min, -0.0, false, false), percent_roll(total.max, 0.0, true, false));
            let dmin = percent_roll(total.value, -ctx.search_in_range, false, false).max(bounds.0);
            let dmax = percent_roll(total.value, ctx.search_in_range, true, false).min(bounds.1);
            made[idx].0.roll = Some(FilterRoll {
                value: round_roll(total.value, false),
                min: Some(dmin),
                max: None,
                default_min: dmin,
                default_max: dmax,
                bounds: None,
                dp: false,
                trade_invert: false,
            });
        }
    }

    debug_assert_eq!(first, ctx.filters.len());
    ctx.filters.extend(made.into_iter().enumerate().filter(|(i, _)| !removed.contains(i)).map(|(_, (f, _))| f));
}

// --- presets -------------------------------------------------------------------

fn enable_all(filters: &mut [UiStat]) {
    for f in filters.iter_mut().filter(|f| f.hidden.is_none()) {
        f.disabled = false;
    }
}

fn apply_flask_rules(filters: &mut [UiStat]) {
    let enkindled = filters.iter().any(|f| f.stat_ref == "Gains no Charges during Flask Effect");
    for f in filters.iter_mut().filter(|f| f.tag == "enchant" && !enkindled) {
        f.hidden = Some("hide_harvest_and_instilling");
        f.disabled = true;
    }
}

/// Which affix slot is open (0 any, 1 prefix, 2 suffix) and how many are.
fn has_empty_modifier(ctx: &Ctx) -> Option<(usize, [f64; 3])> {
    let item = ctx.item;
    if !item.is_modifiable() || item.category.as_deref() == Some(cat::MAP) {
        return None;
    }
    if !matches!(item.rarity, Some(Rarity::Rare | Rarity::Magic)) {
        return None;
    }
    let (prefixes, suffixes, total) = explicit_modifier_count(item);
    let base = match (item.rarity, item.category.as_deref()) {
        (Some(Rarity::Magic), _) => 1.0,
        (_, Some(cat::JEWEL | cat::TABLET | cat::RELIC | cat::SANCTUM_RELIC)) => 2.0,
        _ => 3.0,
    };
    let (mut max_prefix, mut max_suffix) = (base, base);
    for f in &ctx.filters {
        let v = f.roll.map_or(0.0, |r| r.value);
        match f.stat_ref.as_str() {
            "# Prefix Modifier allowed" => max_prefix += v,
            "# Suffix Modifier allowed" => max_suffix += v,
            _ => {}
        }
    }
    let (max_prefix, max_suffix) = (max_prefix.max(0.0), max_suffix.max(0.0));
    let max_any = max_prefix + max_suffix;
    if total == max_any || total == 0.0 {
        return None;
    }
    let empty = if suffixes == max_suffix {
        1
    } else if prefixes == max_prefix {
        2
    } else {
        0
    };
    Some((empty, [max_any - total, max_prefix - prefixes, max_suffix - suffixes]))
}

fn empty_modifier_filter(empty: usize, counts: [f64; 3], disabled: bool) -> UiStat {
    let roll = counts[empty];
    UiStat {
        trade_ids: Some(vec!["item.has_empty_modifier".to_string()]),
        stat_ref: "# Empty Modifier".to_string(),
        text: "# Empty Modifier".to_string(),
        tag: "pseudo",
        sources: Vec::new(),
        roll: Some(FilterRoll {
            value: roll,
            min: Some(roll),
            max: None,
            default_min: roll,
            default_max: roll,
            bounds: None,
            dp: false,
            trade_invert: false,
        }),
        option: Some(empty as f64),
        oils: None,
        disabled,
        hidden: (disabled).then_some("filters.hide_empty_mod"),
    }
}

fn create_exact_stat_filters(item: &ParsedItem, data: &Ee2Data) -> Vec<UiStat> {
    let c = item.category.as_deref();
    let search_in_range = if c == Some(cat::TABLET) { 0.0 } else { SEARCH_STAT_RANGE.min(2.0) };
    if c == Some(cat::INVITATION) {
        return Vec::new();
    }
    if item.is_unidentified && item.rarity == Some(Rarity::Unique) {
        return item
            .stats_by_type
            .iter()
            .filter(|m| m.type_ == ModType::Implicit)
            .map(|m| UiStat { disabled: false, ..calculated_stat_to_filter(m, search_in_range, item) })
            .collect();
    }

    let mut keep = vec![
        ModType::Pseudo,
        ModType::Fractured,
        ModType::Desecrated,
        ModType::Crafted,
        ModType::Enchant,
        ModType::Sanctum,
        ModType::Skill,
    ];
    if !item.is_fractured && c != Some(cat::TINCTURE) {
        keep.push(ModType::Implicit);
    }
    let few_mods = item.rarity == Some(Rarity::Magic)
        || (item.rarity == Some(Rarity::Rare) && explicit_modifier_count(item).2 < 5.0);
    if few_mods && !matches!(c, Some(cat::CLUSTER_JEWEL | cat::MAP | cat::HEIST_CONTRACT | cat::HEIST_BLUEPRINT | cat::SENTINEL)) {
        keep.push(ModType::Explicit);
    }

    let mut ctx = Ctx {
        item,
        data,
        search_in_range,
        filters: Vec::new(),
        stats_by_type: item.stats_by_type.iter().filter(|calc| keep.contains(&calc.type_)).cloned().collect(),
    };
    // A tablet is worth its remaining uses.
    if c == Some(cat::TABLET) {
        filter_pseudo(&mut ctx);
    }
    let rows: Vec<UiStat> =
        ctx.stats_by_type.iter().map(|m| calculated_stat_to_filter(m, search_in_range, item)).collect();
    ctx.filters.extend(rows);

    for f in &mut ctx.filters {
        f.hidden = None;
        if (item.is_fractured && f.tag == "explicit") || (c == Some(cat::TABLET) && f.tag == "implicit") {
            continue;
        }
        if c == Some(cat::TABLET) {
            f.disabled = false;
        } else if f.tag == "explicit" {
            f.disabled = !f.sources.iter().any(|s| s.modifier.tier.is_some_and(|t| t <= 2));
        } else if f.tag != "property" {
            f.disabled = false;
        }
        if f.stat_ref == "# uses remaining" {
            if let Some(r) = &mut f.roll {
                r.min = Some(r.value);
                r.default_min = r.value;
                r.default_max = r.value;
            }
        }
    }

    // A "Fractured Item" none of whose mods is marked: every explicit is
    // offered as the fractured one.
    if item.is_fractured && !ctx.filters.iter().any(|f| f.tag == "fractured") {
        let as_fractured: Vec<UiStat> = item
            .stats_by_type
            .iter()
            .filter(|calc| calc.type_ == ModType::Explicit)
            .map(|m| calculated_stat_to_filter(m, search_in_range, item))
            .filter(|f| f.tag == "explicit")
            .map(|f| UiStat { tag: "fractured", ..f })
            .collect();
        ctx.filters.extend(as_fractured);
    }

    if let Some((empty, counts)) = has_empty_modifier(&ctx) {
        ctx.filters.push(empty_modifier_filter(empty, counts, false));
    }

    match c {
        Some(cat::FLASK) => apply_flask_rules(&mut ctx.filters),
        Some(cat::MEMORY_LINE | cat::SANCTUM_RELIC | cat::CHARM | cat::RELIC) => enable_all(&mut ctx.filters),
        _ => {}
    }
    ctx.filters
}

fn init_ui_mod_filters(item: &ParsedItem, data: &Ee2Data) -> Vec<UiStat> {
    let special = |t: ModType| matches!(t, ModType::Fractured | ModType::Desecrated | ModType::Crafted);
    let mut ctx = Ctx {
        item,
        data,
        filters: Vec::new(),
        search_in_range: if item.rarity == Some(Rarity::Normal) { 100.0 } else { SEARCH_STAT_RANGE },
        // A fractured, desecrated or crafted roll counts toward properties
        // and totals like any explicit one.
        stats_by_type: item
            .stats_by_type
            .iter()
            .map(|calc| {
                if special(calc.type_) && calc.stat.ids("explicit").is_some() {
                    StatCalculated { type_: ModType::Explicit, ..calc.clone() }
                } else {
                    calc.clone()
                }
            })
            .collect(),
    };

    if item.info.ref_name != "Split Personality" {
        filter_item_prop(&mut ctx);
        // A unique with rune sockets is left to its own lines: its totals
        // would count whatever runes the seller happened to socket.
        if item.rarity != Some(Rarity::Unique) || max_sockets(item) == 0 {
            filter_pseudo(&mut ctx);
        }
    }

    if item.is_modifiable() {
        // On an item that can still be crafted the special mods come back
        // as rows of their own, each with only its own rolls.
        ctx.stats_by_type.retain(|m| !special(m.type_));
        let own = item.stats_by_type.iter().filter(|m| special(m.type_)).map(|m| StatCalculated {
            sources: m.sources.iter().filter(|s| s.modifier.type_ == m.type_).cloned().collect(),
            ..m.clone()
        });
        ctx.stats_by_type.extend(own);
    }
    if item.is_veiled {
        ctx.stats_by_type.retain(|m| m.type_ != ModType::Veiled);
    }

    let rows: Vec<UiStat> =
        ctx.stats_by_type.iter().map(|m| calculated_stat_to_filter(m, ctx.search_in_range, item)).collect();
    ctx.filters.extend(rows);

    if item.is_veiled {
        for f in &mut ctx.filters {
            f.disabled = true;
        }
    }
    final_filter_tweaks(&mut ctx);
    ctx.filters
}

fn final_filter_tweaks(ctx: &mut Ctx) {
    let item = ctx.item;
    let c = item.category.as_deref();
    if c == Some(cat::FLASK) {
        apply_flask_rules(&mut ctx.filters);
    }

    // Runes in a unique are the seller's choice, not the item.
    if item.rarity == Some(Rarity::Unique)
        && item.info.ref_name != "Morior Invictus"
        && item.info.ref_name != "Darkness Enthroned"
    {
        for f in ctx.filters.iter_mut().filter(|f| matches!(f.tag, "rune" | "added-rune")) {
            f.disabled = true;
            if f.stat_ref != "Destroys all Augment Sockets on the item to create a Jewel Socket" {
                f.hidden = Some("filters.hide_const_roll");
            }
        }
    }

    if let Some((empty, counts)) = has_empty_modifier(ctx) {
        ctx.filters.push(empty_modifier_filter(empty, counts, true));
    }

    if matches!(c, Some(cat::AMULET | cat::RING)) {
        // Only the three dearest emotions make an anointment worth a filter.
        const HIGH_VALUE_OILS: [usize; 3] = [9, 11, 12];
        if let Some(anoint) = ctx.filters.iter_mut().find(|f| f.oils.is_some()) {
            if item.talisman_tier.is_some_and(|t| t != 0.0) {
                anoint.disabled = false;
            } else if item.is_modifiable()
                && !anoint.oils.as_ref().is_some_and(|o| o.iter().any(|i| HIGH_VALUE_OILS.contains(i)))
            {
                anoint.hidden = Some("filters.hide_anointment");
                anoint.disabled = true;
            }
        }
    }

    for f in &mut ctx.filters {
        if matches!(f.tag, "fractured" | "desecrated" | "crafted") {
            let has_explicit = item
                .stats_by_type
                .iter()
                .find(|m| m.stat.ref_ == f.stat_ref)
                .is_some_and(|m| m.stat.ids("explicit").is_some());
            if has_explicit {
                f.hidden = Some("filters.hide_for_crafting");
            }
        } else if let ("skill", Some(roll)) = (f.tag, f.roll) {
            f.disabled = roll.value < 19.0;
            if f.disabled {
                f.hidden = Some("filters.hide_not_max_level");
            }
        }
        if c == Some(cat::MAP) && f.tag != "property" && f.tag != "desecrated" {
            f.disabled = true;
            f.hidden = Some("filters.hide_for_map");
        }
        if c == Some(cat::TABLET) && f.stat_ref == "# uses remaining" {
            f.hidden = None;
        }
    }

    if item.rarity == Some(Rarity::Magic)
        && item.is_modifiable()
        && item.item_level.is_some_and(|l| l != 0.0 && max_useful_item_level(c) - 3.0 > l)
    {
        ctx.filters.push(UiStat {
            trade_ids: Some(vec!["item.rarity_magic".to_string()]),
            stat_ref: "Rarity: Magic".to_string(),
            text: "Rarity: Magic".to_string(),
            tag: "pseudo",
            sources: Vec::new(),
            roll: None,
            option: None,
            oils: None,
            disabled: true,
            hidden: Some("filters.hide_low_ilvl"),
        });
    }

    if item.rarity == Some(Rarity::Unique) || c == Some(cat::RELIC) {
        let visible = ctx.filters.iter().filter(|f| f.hidden.is_none()).count();
        if visible <= 3 {
            enable_all(&mut ctx.filters);
        }
    }
}
