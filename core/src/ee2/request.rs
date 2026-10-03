//! From EE2's filter selection to our [`Query`]: the half of
//! `createTradeRequest` that reads the item and the rows. The other half,
//! the JSON itself, is `Query::to_body`, so a row the user toggles later is
//! still sent the way EE2 would send it.

use super::data::Ee2Data;
use super::filters::{area_level_by_ascendancy_points, create_presets, line_filter, FilterRoll, ItemFilters, UiStat};
use super::parse::{
    cat, parse_clipboard, ModType, ParseError, ParsedItem, ParsedModifier, ParsedStat, Rarity, StatCalculated,
    StatRoll, StatSource, UnknownModifier,
};
use crate::props::{EquipKey, PropFilter};
use crate::trade::{
    normalize_mod_text, Bound, EquipmentFilters, FilterLabel, FilterRole, FilterValue, Query, StatFilter, StatIndex,
};

pub fn category_trade_id(category: &str) -> Option<&'static str> {
    Some(match category {
        cat::MAP => "map",
        cat::ABYSS_JEWEL => "jewel.abyss",
        cat::AMULET => "accessory.amulet",
        cat::BELT => "accessory.belt",
        cat::BODY_ARMOUR => "armour.chest",
        cat::BOOTS => "armour.boots",
        cat::BOW => "weapon.bow",
        cat::CLAW => "weapon.claw",
        cat::DAGGER => "weapon.dagger",
        cat::FISHING_ROD => "weapon.rod",
        cat::FLASK => "flask",
        cat::GLOVES => "armour.gloves",
        cat::HELMET => "armour.helmet",
        cat::JEWEL => "jewel",
        cat::ONE_HAND_AXE => "weapon.oneaxe",
        cat::ONE_HAND_MACE => "weapon.onemace",
        cat::ONE_HAND_SWORD => "weapon.onesword",
        cat::QUIVER => "armour.quiver",
        cat::RING => "accessory.ring",
        cat::RUNE_DAGGER => "weapon.runedagger",
        cat::SCEPTRE => "weapon.sceptre",
        cat::SHIELD => "armour.shield",
        cat::STAFF => "weapon.staff",
        cat::TWO_HAND_AXE => "weapon.twoaxe",
        cat::TWO_HAND_MACE => "weapon.twomace",
        cat::TWO_HAND_SWORD => "weapon.twosword",
        cat::WAND => "weapon.wand",
        cat::WARSTAFF => "weapon.warstaff",
        cat::CLUSTER_JEWEL => "jewel.cluster",
        cat::HEIST_BLUEPRINT => "heistmission.blueprint",
        cat::HEIST_CONTRACT => "heistmission.contract",
        "Heist Tool" => "heistequipment.heisttool",
        "Heist Brooch" => "heistequipment.heistreward",
        "Heist Gear" => "heistequipment.heistweapon",
        "Heist Cloak" => "heistequipment.heistutility",
        "Trinket" => "accessory.trinket",
        cat::SANCTUM_RELIC => "sanctum.relic",
        cat::TINCTURE => "tincture",
        cat::CHARM => "flask.charm",
        cat::CROSSBOW => "weapon.crossbow",
        "Skill Gem" => "gem.activegem",
        cat::SUPPORT_GEM => "gem.supportgem",
        cat::META_GEM => "gem.metagem",
        cat::FOCUS => "armour.focus",
        cat::SPEAR => "weapon.spear",
        cat::FLAIL => "weapon.flail",
        cat::BUCKLER => "armour.buckler",
        cat::TABLET => "map.tablet",
        cat::MAP_FRAGMENT => "map.fragment",
        cat::TALISMAN => "weapon.talisman",
        "Waystone" => "map.waystone",
        _ => return None,
    })
}

/// A price check as EE2 would open it.
#[derive(Debug, Clone, PartialEq)]
pub struct Built {
    /// EE2's name for the selection: `filters.preset_pseudo` (rolls and
    /// totals) or `filters.preset_exact` (the base itself).
    pub preset: &'static str,
    pub query: Query,
    /// Index-aligned with `query.filters`.
    pub labels: Vec<FilterLabel>,
    /// The item's computed figures, searched through `query.equipment`.
    pub props: Vec<PropFilter>,
    /// Modifier lines no stat is known for, and rows whose stat has no
    /// trade id under their mod type. They are shown, never searched.
    pub unsearchable: Vec<String>,
    /// Index-aligned with `unsearchable`: the item lines each entry is.
    pub unsearchable_lines: Vec<Vec<String>>,
    /// Modifier lines with no row of their own. EE2 takes a stat out of its
    /// list once a figure or a total has counted it, so the search is right
    /// without them, but a card that leaves them out cannot be told from one
    /// that never read them.
    pub counted: Vec<CountedLine>,
    /// Every modifier line EE2 gives no row of its own (the counted lines,
    /// the unsearchable ones, the unread and unknown ones), each as a row
    /// the user can add to the search. They are no part of `query`: EE2's
    /// search stays exactly EE2's, and a row joins it only when ticked (see
    /// [`ExtraRow::filter`]).
    pub extra: Vec<ExtraRow>,
    pub item: ParsedItem,
}

/// Why an extra row has no checkbox. Only a line whose stat no catalog
/// lists under its type says it; a guessed id would search something else.
pub const NO_TRADE_STAT: &str = "the trade site indexes no stat for this line";

/// One modifier line of the item as a search row of its own, for a line
/// EE2's selection leaves without one.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtraRow {
    /// The line as the item words it, roll annotations removed.
    pub text: String,
    /// "implicit", "explicit", "crafted", "desecrated", "rune", ...: the
    /// line's own mod type.
    pub tag: &'static str,
    /// The first number on the line, for the tier badge.
    pub rolled: Option<f64>,
    /// The line as the item text has it (several for a stat that spans
    /// them).
    pub lines: Vec<String>,
    /// The rows on the card that already count the line ("Elemental DPS",
    /// "Total Life"); empty when none does.
    pub into: Vec<String>,
    /// The trade catalog group the ids come from: the line's own type
    /// ("crafted" for a crafted line), "pseudo" for an unrevealed mod.
    pub group: &'static str,
    /// Trade ids under `group`, primary first. More than one is searched as
    /// "any of these", the way the EE2 rows send a stat with a local twin.
    /// Empty when no catalog lists the line under its type.
    pub ids: Vec<String>,
    /// An option stat's chosen option, sent as `id|option`.
    pub option: Option<f64>,
    /// The bounds, as EE2 bounds its own rows under the same preset.
    pub value: FilterValue,
    /// Texts, in the catalog's own form, the line may be listed under;
    /// tried in order within `group` when the ids are not in the catalog.
    pub lookup: Vec<String>,
    /// The stat's catalog keys ("stat_3299347043") under any type. The
    /// trade site keys a stat alike in every group (`explicit.stat_N`,
    /// `crafted.stat_N`), so a type the pinned data has no id for is looked
    /// up under the same key, and taken only when the catalog lists it.
    pub stat_keys: Vec<String>,
}

impl ExtraRow {
    /// The filter the row adds to the search, switched off until ticked;
    /// `None` for a line no catalog lists.
    pub fn filter(&self) -> Option<StatFilter> {
        let with_option = |id: &String| match self.option {
            Some(o) => format!("{id}|{}", format_number(o)),
            None => id.clone(),
        };
        let (first, rest) = self.ids.split_first()?;
        Some(StatFilter {
            id: with_option(first),
            alt_ids: rest.iter().map(with_option).collect(),
            value: self.value,
            disabled: true,
            role: FilterRole::Stat,
            not_id: None,
        })
    }

    /// The small note the card draws beside the row: which total already
    /// counts the line, and why a row cannot be searched.
    pub fn note(&self) -> Option<String> {
        let counted = (!self.into.is_empty()).then(|| format!("counted in {}", self.into.join(", ")));
        let reason = self.ids.is_empty().then_some(NO_TRADE_STAT);
        match (counted, reason) {
            (Some(c), Some(r)) => Some(format!("{c}; {r}")),
            (Some(c), None) => Some(c),
            (None, r) => r.map(str::to_string),
        }
    }

    /// Takes the row's ids from the trade site's own catalog: the ones it
    /// lists of those the pinned data gave, else the stat's key under the
    /// row's group, else the line's text within that group. The catalog is
    /// what the site searches, so an id it does not list is dropped, and a
    /// line it lists nowhere under its type keeps no id.
    pub fn resolve(&mut self, catalog: &StatIndex) {
        let listed = |id: &String| catalog.entry_by_id(id).is_some();
        let mut ids: Vec<String> = self.ids.iter().filter(|id| listed(id)).cloned().collect();
        if ids.is_empty() {
            ids = self.stat_keys.iter().map(|k| format!("{}.{k}", self.group)).filter(listed).collect();
        }
        if ids.is_empty() {
            ids = self
                .lookup
                .iter()
                .map(|text| catalog.find_in(self.group, text))
                .find(|found| !found.is_empty())
                .map(|found| found.into_iter().map(|e| e.id.clone()).collect())
                .unwrap_or_default();
        }
        self.ids = ids;
    }
}

/// One modifier line of the item that is shown and never searched.
#[derive(Debug, Clone, PartialEq)]
pub struct CountedLine {
    /// The line as the item words it, roll annotations removed.
    pub text: String,
    /// The rows on the card that counted it ("Elemental DPS", "Total
    /// Life"). Empty when nothing did: EE2 searches nothing for the line.
    pub into: Vec<String>,
    /// "implicit", "explicit", "rune": the block it belongs to.
    pub tag: &'static str,
    /// The first number on the line, for the tier badge.
    pub rolled: Option<f64>,
    /// The line as the item text has it (several for a stat that spans
    /// them).
    pub lines: Vec<String>,
}

impl CountedLine {
    /// What the card appends to the line.
    pub fn note(&self) -> String {
        if self.into.is_empty() {
            "not searched".to_string()
        } else {
            format!("counted in {}", self.into.join(", "))
        }
    }
}

/// "+45(40-49) to maximum Life" as the tooltip words it: "+45 to maximum
/// Life". Only a range that follows a number is removed; other brackets are
/// part of the line.
pub fn display_line(line: &str) -> String {
    let line = line.strip_suffix(" \u{2014} Unscalable Value").unwrap_or(line);
    let src: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < src.len() {
        if src[i] == '(' && i > 0 && src[i - 1].is_ascii_digit() {
            let close = src[i..].iter().position(|c| *c == ')').map(|n| i + n);
            let range = |c: &char| c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | '\u{2013}');
            if let Some(close) = close.filter(|close| *close > i + 1 && src[i + 1..*close].iter().all(range)) {
                i = close + 1;
                continue;
            }
        }
        out.push(src[i]);
        i += 1;
    }
    out
}

/// The name a total goes by in "counted in ...": its row text without the
/// number placeholder.
fn total_name(stat: &UiStat) -> String {
    let text = stat.text.as_str();
    if stat.tag == "property" {
        return text.split(':').next().unwrap_or(text).trim().to_string();
    }
    let bare = text.trim_start_matches(['+', '#', '%', ' ']);
    let bare = bare.strip_prefix("total to all ").map(|r| format!("all {r}")).unwrap_or_else(|| {
        let r = bare.strip_prefix("total ").unwrap_or(bare);
        let r = r.strip_prefix("to ").unwrap_or(r);
        r.strip_prefix("maximum ").unwrap_or(r).to_string()
    });
    if text.contains("total") {
        format!("Total {}", bare.trim())
    } else {
        bare.strip_prefix("increased ").unwrap_or(&bare).trim().to_string()
    }
}

impl Built {
    /// True when the item carries modifier lines and not one of them is
    /// behind anything the search can use: no mod row, no total that
    /// counted a line. That is what a simple-format copy looks like (a
    /// chat-linked item, or the game's advanced descriptions switched off):
    /// EE2 reads modifiers only under their `{ ... }` headers, so the
    /// search would be "any item of this category" and its listings would
    /// say nothing about this item. A caller must not price from it.
    pub fn reads_no_modifier(&self) -> bool {
        let lines = self.counted.len() + self.unsearchable_lines.len();
        lines > 0
            && self.counted.iter().all(|c| c.into.is_empty())
            && self.labels.iter().all(|l| l.lines.is_empty())
    }

    /// Resolves every extra row against the trade site's live catalog
    /// (see [`ExtraRow::resolve`]).
    pub fn resolve_extra(&mut self, catalog: &StatIndex) {
        for row in &mut self.extra {
            row.resolve(catalog);
        }
    }

    /// The unsearchable rows that stand for no line of the item (a total
    /// with no trade id). The lines themselves are extra rows.
    pub fn unsearchable_totals(&self) -> Vec<String> {
        self.unsearchable
            .iter()
            .zip(&self.unsearchable_lines)
            .filter(|(_, lines)| lines.is_empty())
            .map(|(text, _)| text.clone())
            .collect()
    }
}

pub fn build(clipboard: &str, data: &Ee2Data) -> Result<Built, ParseError> {
    let item = parse_clipboard(clipboard, data)?;
    let (mut presets, active) = create_presets(&item, data);
    let at = presets.iter().position(|p| p.id == active).unwrap_or(0);
    let preset = presets.swap_remove(at);
    Ok(to_query(preset.id, &preset.filters, &preset.stats, item, data))
}

/// Switches on every row EE2 shows by default (its `defaultAllSelected`
/// setting), through the same two writes the card's checkboxes make: a
/// stat row's `disabled` flag, a property's equipment bound.
pub fn select_all(built: &mut Built) {
    for (f, l) in built.query.filters.iter_mut().zip(&built.labels) {
        if !l.hidden {
            f.disabled = false;
        }
    }
    let equipment = built.query.equipment.get_or_insert_with(Default::default);
    for p in built.props.iter_mut().filter(|p| !p.hidden && p.key != EquipKey::RuneSockets) {
        p.enabled = true;
        equipment.set(p.key, Some(p.min));
    }
    if equipment.is_empty() {
        built.query.equipment = None;
    }
}

fn equip_key(trade_id: &str) -> Option<EquipKey> {
    Some(match trade_id {
        "item.armour" => EquipKey::Armour,
        "item.evasion_rating" => EquipKey::Evasion,
        "item.energy_shield" => EquipKey::EnergyShield,
        "item.runic_ward" => EquipKey::RunicWard,
        "item.block" => EquipKey::Block,
        "item.total_dps" => EquipKey::Dps,
        "item.physical_dps" => EquipKey::Pdps,
        "item.elemental_dps" => EquipKey::Edps,
        "item.crit" => EquipKey::Crit,
        "item.aps" => EquipKey::Aps,
        "item.spirit" => EquipKey::Spirit,
        "item.reload_time" => EquipKey::ReloadTime,
        _ => return None,
    })
}

fn map_filter_key(trade_id: &str) -> Option<&'static str> {
    Some(match trade_id {
        "item.map_revives" => "map_revives",
        "item.map_pack_size" => "map_packsize",
        "item.map_magic_monsters" => "map_magic_monsters",
        "item.map_rare_monsters" => "map_rare_monsters",
        "item.map_drop_chance" => "map_bonus",
        "item.map_item_rarity" => "map_iir",
        "item.map_gold" => "map_gold",
        _ => return None,
    })
}

/// The bounds as the trade site wants them: a stat the site indexes with
/// the opposite sign ("reduced" wordings, inverted ids) swaps ends.
fn min_max(roll: Option<&FilterRoll>) -> FilterValue {
    match roll {
        None => FilterValue::default(),
        Some(r) if r.trade_invert => FilterValue { min: r.max.map(|v| -v), max: r.min.map(|v| -v) },
        Some(r) => FilterValue { min: r.min, max: r.max },
    }
}

fn label_text(stat: &UiStat) -> String {
    match stat.roll {
        Some(r) if stat.text.matches('#').count() == 1 => stat.text.replace('#', &format_number(r.value)),
        _ => stat.text.clone(),
    }
    .replace('\n', " / ")
}

fn format_number(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

fn to_query(preset: &'static str, f: &ItemFilters, stats: &[UiStat], item: ParsedItem, data: &Ee2Data) -> Built {
    let enabled = |t: &Option<super::filters::Toggle>| t.filter(|t| !t.disabled).map(|t| t.value);
    let whole = |v: f64| (!v.is_nan()).then_some(v as u32);
    let plain_rarity = matches!(item.rarity, Some(Rarity::Normal | Rarity::Magic | Rarity::Rare));

    let mut q = Query {
        category: f.category.as_ref().and_then(|(c, _)| category_trade_id(c)).map(str::to_string),
        category_enabled: f.category.as_ref().is_some_and(|(_, disabled)| !disabled),
        category_replaces_type: true,
        name: f.name.clone(),
        type_name: f.base_type.clone(),
        rarity: f.rarity.map(str::to_string),
        ilvl_min: enabled(&f.item_level).and_then(whole),
        quality_min: enabled(&f.quality).and_then(whole),
        req_level_max: enabled(&f.requires_level).and_then(whole),
        map_tier: enabled(&f.map_tier).filter(|t| !t.is_nan()).map(|t| t as i64),
        gem_level: enabled(&f.gem_level)
            .map(|v| Bound { min: Some(v).filter(|v| !v.is_nan()), max: f.gem_level_max }),
        gem_sockets_min: enabled(&f.socket_number).and_then(whole),
        identified: enabled(&f.unidentified).map(|_| false),
        unidentified_tier_min: enabled(&f.unidentified_tier).and_then(whole),
        fractured: f.fractured_false.then_some(false),
        collapse: true,
        ..Default::default()
    };

    if let Some(level) = enabled(&f.area_level) {
        // A trial key is better the lower its area: the item's level caps
        // the search, and the ascendancy points it awards set the floor.
        q.area_level = Some(if f.awarded_ascendancy_points.is_some() {
            Bound { min: None, max: Some(level) }
        } else {
            Bound::at_least(level)
        });
    }
    if let Some(points) = enabled(&f.awarded_ascendancy_points) {
        let floor = area_level_by_ascendancy_points(&item.info.ref_name, points);
        q.area_level.get_or_insert_with(Bound::default).min = Some(floor);
    }

    // Only ever "not corrupted" (or, for a magic jewel, the exact state):
    // a corrupted item is still fairly compared with clean ones.
    if let Some((value, exact)) = f.corrupted {
        if (!value || exact) && !f.sanctified {
            q.corrupted = Some(value);
        }
    }
    if !f.mirrored && plain_rarity {
        q.mirrored = Some(false);
    }
    let corrupted_loosely = f.corrupted.is_some_and(|(value, exact)| value && !exact);
    if !f.sanctified && !corrupted_loosely && plain_rarity {
        q.sanctified = Some(false);
    }

    let mut labels: Vec<FilterLabel> = Vec::new();
    let mut props: Vec<PropFilter> = Vec::new();
    let mut equipment = EquipmentFilters::default();
    let mut unsearchable: Vec<String> = item.unknown_modifiers.iter().map(|u| display_line(&u.text)).collect();
    let mut unsearchable_lines: Vec<Vec<String>> =
        item.unknown_modifiers.iter().map(|u| vec![u.text.clone()]).collect();
    // Where each of EE2's rows ended up on the card, for the accounting of
    // the item's lines below.
    let mut placed: Vec<Placed> = Vec::with_capacity(stats.len());

    let flask_without_effect = item.category.as_deref() == Some(cat::FLASK)
        && !stats.iter().any(|s| s.stat_ref == "#% increased effect");

    for stat in stats {
        // Exactly one `placed` entry per row, whichever branch takes it.
        placed.push(Placed::Nowhere);
        let at = placed.len() - 1;
        let Some(ids) = stat.trade_ids.as_ref().filter(|ids| !ids.is_empty()) else {
            placed[at] = Placed::Unsearchable(unsearchable.len());
            unsearchable.push(label_text(stat));
            unsearchable_lines.push(Vec::new());
            continue;
        };
        let first = ids[0].as_str();
        let roll = stat.roll.as_ref();
        let label = |tag: &'static str, value: &FilterValue| FilterLabel {
            text: label_text(stat),
            tier: stat.sources.iter().find_map(|s| s.modifier.tier).and_then(|t| u8::try_from(t).ok()),
            min: value.min.or(value.max).unwrap_or(0.0).floor() as i64,
            rolled: roll.map(|r| r.value),
            tag,
            hidden: stat.hidden.is_some(),
            lines: Vec::new(),
        };

        if let Some(key) = equip_key(first) {
            let bound = roll.and_then(|r| if key.lower_is_better() { r.max } else { r.min });
            if let (Some(r), Some(bound)) = (roll, bound) {
                props.push(PropFilter {
                    key,
                    value: r.value,
                    min: bound,
                    enabled: !stat.disabled,
                    hidden: stat.hidden.is_some(),
                });
                if !stat.disabled {
                    equipment.set(key, Some(bound));
                }
                placed[at] = Placed::Property;
            }
            continue;
        }
        if let Some(key) = map_filter_key(first) {
            let value = FilterValue { min: roll.and_then(|r| r.min), max: None };
            placed[at] = Placed::Label(labels.len());
            labels.push(label("map", &value));
            q.filters.push(StatFilter {
                id: key.to_string(),
                alt_ids: Vec::new(),
                value,
                disabled: stat.disabled,
                role: FilterRole::Stat,
                not_id: None,
            });
            continue;
        }
        if first == "item.has_empty_modifier" {
            let refs = ["# Empty Modifiers", "# Empty Prefix Modifiers", "# Empty Suffix Modifiers"];
            let slot = stat.option.unwrap_or(0.0) as usize;
            let id = data.stats.by_ref(refs[slot.min(2)]).and_then(|s| s.ids("pseudo").and_then(|i| i.first().cloned()));
            let Some(id) = id else {
                placed[at] = Placed::Unsearchable(unsearchable.len());
                unsearchable.push(label_text(stat));
                unsearchable_lines.push(Vec::new());
                continue;
            };
            let value = min_max(roll);
            placed[at] = Placed::Label(labels.len());
            labels.push(label(stat.tag, &value));
            q.filters.push(StatFilter {
                id,
                alt_ids: Vec::new(),
                value,
                disabled: stat.disabled,
                role: FilterRole::EmptyModifier,
                not_id: None,
            });
            continue;
        }
        if first == "item.rarity_magic" {
            let value = FilterValue::default();
            placed[at] = Placed::Label(labels.len());
            labels.push(label(stat.tag, &value));
            q.filters.push(StatFilter {
                id: first.to_string(),
                alt_ids: Vec::new(),
                value,
                disabled: stat.disabled,
                role: FilterRole::RarityMagic,
                not_id: None,
            });
            continue;
        }

        let with_option = |id: &String| match stat.option {
            Some(o) => format!("{id}|{}", format_number(o)),
            None => id.clone(),
        };
        let not_id = (flask_without_effect && stat.stat_ref == "#% increased Charge Recovery")
            .then(|| data.stats.by_ref("#% increased effect"))
            .flatten()
            .and_then(|s| s.ids("explicit").and_then(|i| i.first().cloned()));
        let value = min_max(roll);
        placed[at] = Placed::Label(labels.len());
        labels.push(label(stat.tag, &value));
        q.filters.push(StatFilter {
            id: with_option(&ids[0]),
            alt_ids: ids[1..].iter().map(with_option).collect(),
            value,
            disabled: stat.disabled,
            role: FilterRole::Stat,
            not_id,
        });
    }

    if let Some(sockets) = f.augment_sockets {
        props.push(PropFilter {
            key: EquipKey::RuneSockets,
            value: sockets.value,
            min: sockets.value,
            enabled: !sockets.disabled,
            hidden: false,
        });
        if !sockets.disabled {
            equipment.set(EquipKey::RuneSockets, Some(sockets.value));
        }
    }
    q.equipment = Some(equipment).filter(|e| !e.is_empty());

    if let Some((count, false)) = f.veiled {
        // An unrevealed desecrated mod: searched as "has this many".
        q.veiled = Some(true);
        labels.push(FilterLabel {
            text: "Unrevealed Modifiers".to_string(),
            tier: None,
            min: count as i64,
            rolled: Some(count),
            tag: "pseudo",
            hidden: false,
            // This row is the unrevealed mods' own.
            lines: item.unread_modifiers.iter().filter(|u| u.type_ == ModType::Veiled).map(|u| u.text.clone()).collect(),
        });
        q.filters.push(StatFilter::at_least("pseudo.pseudo_number_of_unrevealed_mods", count, false));
    }

    let exact = preset == "filters.preset_exact";
    let veiled_has_row = labels.iter().any(|l| l.text == "Unrevealed Modifiers");
    let unread: Vec<&UnknownModifier> =
        item.unread_modifiers.iter().filter(|u| !(veiled_has_row && u.type_ == ModType::Veiled)).collect();
    // An unrevealed mod is searched as a count of its kind, over every
    // unrevealed line the item shows, wherever the parser left it.
    let unrevealed: Vec<(&'static str, &'static str)> = item
        .unknown_modifiers
        .iter()
        .chain(unread.iter().copied())
        .filter(|u| is_unrevealed(u))
        .map(|u| unrevealed_kind(&u.text))
        .collect();
    let extra_of = |u: &UnknownModifier| {
        if is_unrevealed(u) {
            let kind = unrevealed_kind(&u.text);
            unrevealed_extra(u, kind, unrevealed.iter().filter(|k| **k == kind).count())
        } else {
            text_extra(u, data)
        }
    };
    // The lines with no row of their own, in the order the card lists
    // them: the unknown ones (which lead `unsearchable`), then the parsed
    // ones, then the unread ones.
    let mut extra: Vec<ExtraRow> = item.unknown_modifiers.iter().map(extra_of).collect();
    let mut counted: Vec<CountedLine> = Vec::new();
    let is_total = |s: &UiStat| matches!(s.tag, "property" | "pseudo");
    for (modifier, parsed) in item.new_mods.iter().flat_map(|m| m.stats.iter().map(move |s| (m, s))) {
        let holds = |s: &UiStat| s.sources.iter().any(|src| src.stat.ordinal == parsed.ordinal);
        let line = parsed.lines.join("\n");
        let shown = display_line(&line).replace('\n', " / ");
        let on_card = |i: &usize| placed[*i] != Placed::Nowhere;
        // A total whose one source reads exactly like it (movement speed)
        // is that line's own row, not a sum of anything.
        let own_row = |s: &UiStat| !is_total(s) || (s.sources.len() == 1 && label_text(s) == shown);
        let rows: Vec<usize> = (0..stats.len()).filter(|i| holds(&stats[*i])).filter(on_card).collect();
        // A fractured roll has a row under "explicit" with every source and
        // one of its own; the line belongs to its own.
        let own = rows
            .iter()
            .copied()
            .filter(|i| own_row(&stats[*i]) && placed[*i] != Placed::Property)
            .min_by_key(|i| stats[*i].tag != modifier.info.type_.as_str());
        match own.map(|i| placed[i]) {
            Some(Placed::Label(l)) => labels[l].lines.push(line),
            Some(Placed::Unsearchable(u)) => {
                unsearchable_lines[u].push(line);
                extra.push(stat_extra(modifier, parsed, &item, data, exact, shown, Vec::new()));
            }
            _ => {
                // The totals the card shows open speak for the line; the
                // folded-away ones (a per-element total that repeats the
                // elemental one) are named only when they are all there is.
                let totals: Vec<usize> = rows.iter().copied().filter(|i| !own_row(&stats[*i])).collect();
                let open = totals.iter().any(|i| stats[*i].hidden.is_none());
                let into: Vec<String> = totals
                    .iter()
                    .filter(|i| !open || stats[**i].hidden.is_none())
                    .map(|i| total_name(&stats[*i]))
                    .collect();
                extra.push(stat_extra(modifier, parsed, &item, data, exact, shown.clone(), into.clone()));
                counted.push(CountedLine {
                    into,
                    text: shown,
                    tag: modifier.info.type_.as_str(),
                    rolled: parsed.roll.map(|r| r.value),
                    lines: parsed.lines.clone(),
                });
            }
        }
    }
    extra.extend(unread.iter().map(|u| extra_of(u)));
    for unread in unread {
        counted.push(CountedLine {
            text: display_line(&unread.text),
            into: Vec::new(),
            tag: unread.type_.as_str(),
            rolled: None,
            lines: vec![unread.text.clone()],
        });
    }

    Built { preset, query: q, labels, props, unsearchable, unsearchable_lines, counted, extra, item }
}

/// A parsed modifier line as a row of its own: its stat under the line's
/// own type, bounded the way EE2 bounds that stat's row.
fn stat_extra(
    modifier: &ParsedModifier,
    parsed: &ParsedStat,
    item: &ParsedItem,
    data: &Ee2Data,
    exact: bool,
    text: String,
    into: Vec<String>,
) -> ExtraRow {
    let type_ = modifier.info.type_;
    let group = type_.trade_key();
    // The line's own share of the stat, with the catalyst scaling the sum
    // applied to it; only this line, not the others the stat sums.
    let source = item
        .stats_by_type
        .iter()
        .flat_map(|c| &c.sources)
        .find(|s| s.stat.ordinal == parsed.ordinal)
        .cloned()
        .unwrap_or_else(|| StatSource {
            modifier: modifier.info.clone(),
            stat: parsed.clone(),
            contributes: parsed.roll.map(|r| StatRoll { value: r.value, min: r.min, max: r.max, option: r.option }),
        });
    let row = line_filter(&StatCalculated { stat: parsed.stat.clone(), type_, sources: vec![source] }, item, exact);
    let prefix = format!("{group}.");
    let ids: Vec<String> =
        row.trade_ids.iter().flatten().filter(|id| id.starts_with(&prefix)).cloned().collect();
    // A "reduced" wording is the negated form of a stat the catalog lists
    // as "increased": looked up by its words it would find nothing, or a
    // stat of its own searched with the mirrored bounds.
    let lookup = if parsed.translation.negate {
        Vec::new()
    } else {
        let mut texts = vec![parsed.translation.string.clone()];
        texts.extend(lookup_texts(&parsed.lines.join("\n")));
        dedup(texts)
    };
    let stat_keys = dedup(
        parsed
            .stat
            .trade_ids
            .iter()
            .flat_map(|(_, ids)| ids)
            .filter_map(|id| id.split_once('.').map(|(_, key)| key.to_string()))
            .collect(),
    );
    let mut extra = ExtraRow {
        text,
        tag: type_.as_str(),
        rolled: parsed.roll.map(|r| r.value),
        lines: parsed.lines.clone(),
        into,
        group,
        ids,
        option: row.option,
        value: extra_value(&row, parsed),
        lookup,
        stat_keys,
    };
    if extra.ids.is_empty() {
        extra.ids = text_ids(data, group, &extra.lookup);
    }
    extra
}

/// The row's bounds: EE2's for the stat. Where EE2 sends none though the
/// line has a number (a unique's constant roll, which EE2 leaves to the
/// unique's name), the rolled number itself, on the end the stat is
/// better at and mirrored for a stat the site indexes negated.
fn extra_value(row: &UiStat, parsed: &ParsedStat) -> FilterValue {
    let value = min_max(row.roll.as_ref());
    if value.min.is_some() || value.max.is_some() || row.option.is_some() {
        return value;
    }
    let Some(v) = parsed.roll.map(|r| r.value) else { return value };
    let (min, max) =
        if parsed.stat.better == super::data::Better::Negative { (None, Some(v)) } else { (Some(v), None) };
    if parsed.stat.inverted {
        FilterValue { min: max.map(|v| -v), max: min.map(|v| -v) }
    } else {
        FilterValue { min, max }
    }
}

/// A line no stat of the pinned data reads: searched by its text under its
/// own type, from its number.
fn text_extra(u: &UnknownModifier, data: &Ee2Data) -> ExtraRow {
    let shown = display_line(&u.text);
    let group = u.type_.trade_key();
    let lookup = lookup_texts(&u.text);
    let numbers = numbers(&shown);
    ExtraRow {
        rolled: numbers.first().copied(),
        value: FilterValue { min: searched_number(&numbers, &lookup), max: None },
        ids: text_ids(data, group, &lookup),
        text: shown,
        tag: u.type_.as_str(),
        lines: vec![u.text.clone()],
        into: Vec::new(),
        group,
        option: None,
        lookup,
        stat_keys: Vec::new(),
    }
}

/// An unrevealed desecrated mod: the game prints a placeholder where its
/// stat will be ("Desecrated Suffix"), under a veiled header or under a
/// desecrated one.
fn is_unrevealed(u: &UnknownModifier) -> bool {
    u.type_ == ModType::Veiled
        || matches!(u.text.trim(), "Desecrated Prefix" | "Desecrated Suffix" | "Desecrated Modifier")
}

/// Which of the site's unrevealed-mod counts an unrevealed line is: its
/// catalog text and id.
fn unrevealed_kind(text: &str) -> (&'static str, &'static str) {
    if text.contains("Prefix") {
        ("# Unrevealed Prefix Modifiers", "pseudo.pseudo_number_of_unrevealed_prefix_mods")
    } else if text.contains("Suffix") {
        ("# Unrevealed Suffix Modifiers", "pseudo.pseudo_number_of_unrevealed_suffix_mods")
    } else {
        ("# Unrevealed Modifiers", "pseudo.pseudo_number_of_unrevealed_mods")
    }
}

/// An unrevealed desecrated mod has no stat until it is revealed; the site
/// counts them instead, and the row asks for at least as many of the kind
/// as the item has.
fn unrevealed_extra(u: &UnknownModifier, (text, id): (&'static str, &'static str), count: usize) -> ExtraRow {
    ExtraRow {
        text: display_line(&u.text),
        tag: u.type_.as_str(),
        rolled: None,
        lines: vec![u.text.clone()],
        into: Vec::new(),
        group: "pseudo",
        ids: vec![id.to_string()],
        option: None,
        value: FilterValue { min: Some(count as f64), max: None },
        lookup: vec![text.to_string()],
        stat_keys: Vec::new(),
    }
}

/// The ids the pinned trade texts list for the first of `lookup` they
/// know, under `group` only.
fn text_ids(data: &Ee2Data, group: &str, lookup: &[String]) -> Vec<String> {
    let Some(texts) = data.trade_stats.as_ref() else { return Vec::new() };
    lookup
        .iter()
        .find_map(|t| texts.get(t)?.iter().find(|(g, _)| g == group).map(|(_, ids)| ids.clone()))
        .unwrap_or_default()
}

/// The catalog-form texts an item line may be listed under: numbers as
/// `#` with their sign dropped (the catalog carries the `+` where it
/// belongs), and with it kept, for the few texts that print a sign of their
/// own ("Chain +# times").
fn lookup_texts(line: &str) -> Vec<String> {
    let shown = display_line(line);
    dedup(vec![normalize_mod_text(&shown), hash_numbers(&shown)])
}

fn dedup(items: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(items.len());
    for s in items {
        if !out.contains(&s) {
            out.push(s);
        }
    }
    out
}

/// Every number on a line, signs kept.
fn numbers(line: &str) -> Vec<f64> {
    let chars: Vec<char> = line.chars().collect();
    let digit_at = |i: usize| chars.get(i).is_some_and(|c| c.is_ascii_digit());
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if digit_at(i) || (chars[i] == '-' && digit_at(i + 1)) {
            let start = i;
            i += 1;
            while digit_at(i) || (chars.get(i) == Some(&'.') && digit_at(i + 1)) {
                i += 1;
            }
            if let Ok(v) = chars[start..i].iter().collect::<String>().parse() {
                out.push(v);
            }
        } else {
            i += 1;
        }
    }
    out
}

/// Numbers as `#`, signs kept.
fn hash_numbers(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let digit_at = |i: usize| chars.get(i).is_some_and(|c| c.is_ascii_digit());
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < chars.len() {
        if digit_at(i) {
            while digit_at(i) || (chars.get(i) == Some(&'.') && digit_at(i + 1)) {
                i += 1;
            }
            out.push('#');
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// The number the site searches a line of no known stat by: the one it
/// carries, or the average of an "Adds # to #" range, which is how the
/// site indexes damage. A line with several unrelated numbers is searched
/// by its first.
fn searched_number(numbers: &[f64], lookup: &[String]) -> Option<f64> {
    match numbers {
        [a, b] if lookup.iter().any(|t| t.contains("# to #")) => Some((a + b) / 2.0),
        [first, ..] => Some(*first),
        [] => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Placed {
    Nowhere,
    Label(usize),
    Property,
    Unsearchable(usize),
}
