//! EE2's clipboard parser (`renderer/src/parser`), kept in its shape: the
//! same ordered list of section parsers, each consuming at most one section,
//! because which parser claims a section decides what the item is read as.

use super::data::{BaseType, Better, Ee2Data, Matcher, Stat};
use std::sync::Arc;

/// Item categories as `items.ndjson` spells them.
pub mod cat {
    pub const MAP: &str = "Map";
    pub const HELMET: &str = "Helmet";
    pub const BODY_ARMOUR: &str = "Body Armour";
    pub const GLOVES: &str = "Gloves";
    pub const BOOTS: &str = "Boots";
    pub const SHIELD: &str = "Shield";
    pub const AMULET: &str = "Amulet";
    pub const BELT: &str = "Belt";
    pub const RING: &str = "Ring";
    pub const FLASK: &str = "Flask";
    pub const ABYSS_JEWEL: &str = "Abyss Jewel";
    pub const JEWEL: &str = "Jewel";
    pub const QUIVER: &str = "Quiver";
    pub const CLAW: &str = "Claw";
    pub const BOW: &str = "Bow";
    pub const SCEPTRE: &str = "Sceptre";
    pub const WAND: &str = "Wand";
    pub const FISHING_ROD: &str = "Fishing Rod";
    pub const STAFF: &str = "Staff";
    pub const WARSTAFF: &str = "Warstaff";
    pub const DAGGER: &str = "Dagger";
    pub const RUNE_DAGGER: &str = "Rune Dagger";
    pub const ONE_HAND_AXE: &str = "One Hand Axe";
    pub const TWO_HAND_AXE: &str = "Two Hand Axe";
    pub const ONE_HAND_MACE: &str = "One Hand Mace";
    pub const TWO_HAND_MACE: &str = "Two Hand Mace";
    pub const ONE_HAND_SWORD: &str = "One Hand Sword";
    pub const TWO_HAND_SWORD: &str = "Two Hand Sword";
    pub const CLUSTER_JEWEL: &str = "Cluster Jewel";
    pub const HEIST_BLUEPRINT: &str = "Heist Blueprint";
    pub const HEIST_CONTRACT: &str = "Heist Contract";
    pub const INVITATION: &str = "Invitation";
    pub const GEM: &str = "Gem";
    pub const CURRENCY: &str = "Currency";
    pub const DIVINATION_CARD: &str = "Divination Card";
    pub const SENTINEL: &str = "Sentinel";
    pub const MEMORY_LINE: &str = "Memory Line";
    pub const SANCTUM_RELIC: &str = "Sanctum Relic";
    pub const TINCTURE: &str = "Tincture";
    pub const CHARM: &str = "Charm";
    pub const CROSSBOW: &str = "Crossbow";
    pub const SUPPORT_GEM: &str = "Support Gem";
    pub const META_GEM: &str = "Meta Gem";
    pub const UNCUT_GEM: &str = "UncutSkillGem";
    pub const FOCUS: &str = "Focus";
    pub const RELIC: &str = "Relic";
    pub const TABLET: &str = "TowerAugment";
    pub const SPEAR: &str = "Spear";
    pub const FLAIL: &str = "Flail";
    pub const BUCKLER: &str = "Buckler";
    pub const MAP_FRAGMENT: &str = "MapFragment";
    pub const TALISMAN: &str = "Talisman";
    pub const WOMBGIFT: &str = "BrequelFruit";

    pub fn is_gem(c: &str) -> bool {
        matches!(c, GEM | META_GEM | SUPPORT_GEM)
    }

    pub fn is_armour(c: &str) -> bool {
        matches!(c, BODY_ARMOUR | BOOTS | GLOVES | HELMET | SHIELD | FOCUS | BUCKLER)
    }

    pub fn is_weapon(c: &str) -> bool {
        matches!(
            c,
            STAFF | FISHING_ROD | SCEPTRE | WAND | ONE_HAND_AXE | ONE_HAND_MACE | ONE_HAND_SWORD
                | CLAW | DAGGER | RUNE_DAGGER | SPEAR | FLAIL | BOW | CROSSBOW | TWO_HAND_AXE
                | TWO_HAND_MACE | TWO_HAND_SWORD | WARSTAFF | TALISMAN
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rarity {
    Normal,
    Magic,
    Rare,
    Unique,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModType {
    Pseudo,
    Explicit,
    Implicit,
    Crafted,
    Enchant,
    Scourge,
    Veiled,
    Fractured,
    Augment,
    AddedAugment,
    Sanctum,
    Desecrated,
    Skill,
}

impl ModType {
    /// The key of this type in a stat's trade ids, which is also the tag
    /// the filter row carries.
    pub fn as_str(self) -> &'static str {
        match self {
            ModType::Pseudo => "pseudo",
            ModType::Explicit => "explicit",
            ModType::Implicit => "implicit",
            ModType::Crafted => "crafted",
            ModType::Enchant => "enchant",
            ModType::Scourge => "scourge",
            ModType::Veiled => "veiled",
            ModType::Fractured => "fractured",
            ModType::Augment => "rune",
            ModType::AddedAugment => "added-rune",
            ModType::Sanctum => "sanctum",
            ModType::Desecrated => "desecrated",
            ModType::Skill => "skill",
        }
    }

    /// Socketed runes added in EE2's editor search under the rune ids.
    pub fn trade_key(self) -> &'static str {
        match self {
            ModType::AddedAugment => "rune",
            other => other.as_str(),
        }
    }

    /// The types that share the explicit affix slots and sum into one stat.
    pub fn is_explicit_family(self) -> bool {
        matches!(
            self,
            ModType::Explicit
                | ModType::Fractured
                | ModType::Veiled
                | ModType::Desecrated
                | ModType::Crafted
                | ModType::Sanctum
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Generation {
    Prefix,
    Suffix,
    Corrupted,
    Mutated,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModInfo {
    pub type_: ModType,
    pub generation: Option<Generation>,
    pub name: Option<String>,
    pub tier: Option<u32>,
    pub tags: Vec<String>,
    /// "12% Increased" on the header: a catalyst's scaling of the rolls.
    pub roll_incr: Option<f64>,
}

impl ModInfo {
    pub fn bare(type_: ModType) -> ModInfo {
        ModInfo { type_, generation: None, name: None, tier: None, tags: Vec::new(), roll_incr: None }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Roll {
    pub unscalable: bool,
    pub dp: bool,
    pub value: f64,
    pub min: f64,
    pub max: f64,
    pub option: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedStat {
    pub stat: Arc<Stat>,
    pub translation: Matcher,
    pub roll: Option<Roll>,
    /// The item's own text for this stat (several lines for a stat that
    /// spans them), type marker removed. EE2 keeps no such thing; the card
    /// needs it to show that every line of the item was accounted for.
    pub lines: Vec<String>,
    /// Position among the item's parsed stats. Two identical runes give two
    /// identical lines, and only this tells them apart.
    pub ordinal: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedModifier {
    pub info: ModInfo,
    pub stats: Vec<ParsedStat>,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct StatRoll {
    pub value: f64,
    pub min: f64,
    pub max: f64,
    pub option: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StatSource {
    pub modifier: ModInfo,
    pub stat: ParsedStat,
    pub contributes: Option<StatRoll>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StatCalculated {
    pub stat: Arc<Stat>,
    pub type_: ModType,
    pub sources: Vec<StatSource>,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct AugmentSockets {
    pub current: u32,
    pub normal: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UnknownModifier {
    pub text: String,
    pub type_: ModType,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Trials {
    pub number_of_trials: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedItem {
    pub rarity: Option<Rarity>,
    pub category: Option<String>,
    pub name: String,
    pub base_type: Option<String>,
    pub info: BaseType,
    pub info_variants: Vec<BaseType>,
    pub raw_text: String,
    pub is_unidentified: bool,
    pub unidentified_tier: Option<f64>,
    pub is_corrupted: bool,
    pub is_unmodifiable: bool,
    pub is_mirrored: bool,
    pub is_sanctified: bool,
    pub is_fractured: bool,
    pub is_veiled: bool,
    pub is_foil: bool,
    pub item_level: Option<f64>,
    pub requires_level: Option<f64>,
    pub quality: Option<f64>,
    /// Quality that belongs to a catalyst rather than to the base.
    pub quality_typed: bool,
    pub gem_level: Option<f64>,
    pub gem_sockets: Option<u32>,
    pub stack_size: Option<(f64, f64)>,
    pub map_tier: Option<f64>,
    pub map_revives: Option<f64>,
    pub map_pack_size: Option<f64>,
    pub map_magic_monsters: Option<f64>,
    pub map_rare_monsters: Option<f64>,
    pub map_drop_chance: Option<f64>,
    pub map_item_rarity: Option<f64>,
    pub map_monster_rarity: Option<f64>,
    pub map_effectiveness: Option<f64>,
    pub area_level: Option<f64>,
    pub trials: Option<Trials>,
    pub talisman_tier: Option<f64>,
    pub armour_ar: Option<f64>,
    pub armour_ev: Option<f64>,
    pub armour_es: Option<f64>,
    pub armour_rw: Option<f64>,
    pub armour_block: Option<f64>,
    pub weapon_crit: Option<f64>,
    pub weapon_as: Option<f64>,
    pub weapon_physical: Option<f64>,
    pub weapon_elemental: Option<f64>,
    pub weapon_fire: Option<f64>,
    pub weapon_cold: Option<f64>,
    pub weapon_lightning: Option<f64>,
    pub weapon_reload: Option<f64>,
    pub weapon_spirit: Option<f64>,
    pub augment_sockets: Option<AugmentSockets>,
    pub new_mods: Vec<ParsedModifier>,
    pub stats_by_type: Vec<StatCalculated>,
    pub unknown_modifiers: Vec<UnknownModifier>,
    /// Modifier lines EE2's parser passes over without a word: an
    /// unrevealed desecrated mod, and whatever sits in a section no parser
    /// claimed (a plain Ctrl+C copy's explicit block, a sixth modifier
    /// section). Nothing is searched from them; the card lists them so a
    /// missing row is never mistaken for a considered one.
    pub unread_modifiers: Vec<UnknownModifier>,
}

impl ParsedItem {
    /// Whether the item can still be crafted on.
    pub fn is_modifiable(&self) -> bool {
        self.info.craftable.is_some() && !self.is_corrupted && !self.is_mirrored && !self.is_sanctified
    }

    fn category_is(&self, c: &str) -> bool {
        self.category.as_deref() == Some(c)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    #[error("not an item text")]
    NotAnItem,
    #[error("item is not in the reference data: {0}")]
    UnknownItem(String),
    #[error("unreadable requirements line")]
    Requirements,
    #[error("unreadable modifier header: {0}")]
    ModifierHeader(String),
}

const ENCHANT_LINE: &str = " (enchant)";
const SCOURGE_LINE: &str = " (scourge)";
const AUGMENT_LINE: &str = " (rune)";
const ADDED_AUGMENT_LINE: &str = " (added rune)";
const IMPLICIT_LINE: &str = " (implicit)";
const DESECRATED_LINE: &str = " (desecrated)";
const CRAFTED_LINE: &str = " (crafted)";
const FRACTURED_LINE: &str = " (fractured)";
const UNSCALABLE_VALUE: &str = " \u{2014} Unscalable Value";
const GRANTS_SKILL: &str = "Grants Skill: ";

/// JavaScript's `parseInt(s, 10)`; `None` is NaN.
fn js_parse_int(s: &str) -> Option<f64> {
    let t = s.trim_start();
    let (neg, rest) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    let v: f64 = digits.parse().ok()?;
    Some(if neg { -v } else { v })
}

/// JavaScript's `parseFloat`, for the decimal forms item text uses.
fn js_parse_float(s: &str) -> Option<f64> {
    let t = s.trim_start();
    let b = t.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'-' || b[i] == b'+') {
        i += 1;
    }
    let int_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let mut had_digits = i > int_start;
    if i < b.len() && b[i] == b'.' {
        let frac_start = i + 1;
        let mut j = frac_start;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > frac_start || had_digits {
            had_digits = true;
            i = j;
        }
    }
    if !had_digits {
        return None;
    }
    t[..i].parse().ok().or_else(|| t[..i].trim_end_matches('.').parse().ok())
}

/// JavaScript's `Number(s)`; `None` is NaN.
fn js_number(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        return Some(0.0);
    }
    let ok = t.chars().all(|c| c.is_ascii_digit() || matches!(c, '+' | '-' | '.' | 'e' | 'E'));
    if !ok {
        return None;
    }
    t.parse().ok()
}

pub fn roll_or_minmax_avg(values: &[f64]) -> Option<f64> {
    match values.len() {
        4 => Some((values[0] + values[1] + values[2] + values[3]) / 4.0),
        2 => Some((values[0] + values[1]) / 2.0),
        _ => values.first().copied(),
    }
}

fn text_to_sections(text: &str) -> Vec<Vec<String>> {
    let mut lines: Vec<&str> = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    let mut sections: Vec<Vec<String>> = vec![Vec::new()];
    for line in lines {
        if line == "--------" {
            sections.push(Vec::new());
        } else {
            sections.last_mut().unwrap().push(line.to_string());
        }
    }
    sections.retain(|s| !s.is_empty());
    sections
}

fn markup_condition(text: &str) -> String {
    // `<<set:..>>` markers carry no text; of `<if:..>{a}<elif:..>{b}<else>{c}`
    // the first branch is the one the client shows.
    let mut out = String::new();
    let mut rest = text;
    while let Some(p) = rest.find("<<set:") {
        match rest[p..].find(">>") {
            Some(e) => {
                out.push_str(&rest[..p]);
                rest = &rest[p + e + 2..];
            }
            None => break,
        }
    }
    out.push_str(rest);
    let src = out;
    let mut out = String::new();
    let mut rest = src.as_str();
    while let Some(p) = rest.find('<') {
        let tail = &rest[p + 1..];
        let kind = ["if:", "elif:", "else"].iter().find(|k| tail.starts_with(**k));
        let parsed = kind.and_then(|k| {
            let close = tail.find(">{")?;
            if *k == "else" && close != 4 {
                return None;
            }
            let body_end = tail[close + 2..].find('}')?;
            if body_end == 0 {
                return None;
            }
            Some((k.starts_with("if:"), close + 2, close + 2 + body_end))
        });
        match parsed {
            Some((keep, body_start, body_end)) => {
                out.push_str(&rest[..p]);
                if keep {
                    out.push_str(&tail[body_start..body_end]);
                }
                rest = &tail[body_end + 1..];
            }
            None => {
                out.push_str(&rest[..p + 1]);
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    out
}

enum Step {
    Parsed,
    SectionSkipped,
    ParserSkipped,
}

type SectionParser = fn(&[String], &mut ParsedItem, &Ee2Data) -> Result<Step, ParseError>;

enum Parser {
    Section(SectionParser),
    Virtual(fn(&mut ParsedItem, &Ee2Data) -> Result<(), ParseError>),
}

pub fn parse_clipboard(clipboard: &str, data: &Ee2Data) -> Result<ParsedItem, ParseError> {
    let mut sections = text_to_sections(clipboard);
    if sections.is_empty() {
        return Err(ParseError::NotAnItem);
    }
    if sections[0].get(2).map(String::as_str)
        == Some("You cannot use this item. Its stats will be ignored")
        && sections.len() > 1
    {
        let mut head = sections.remove(0);
        head.pop();
        head.append(&mut sections[0]);
        sections[0] = head;
    }
    let mut item = parse_name_plate(&sections[0])?;
    sections.remove(0);
    item.raw_text = clipboard.to_string();

    let parsers: [Parser; 38] = [
        Parser::Section(parse_unidentified),
        Parser::Virtual(parse_superior),
        Parser::Virtual(normalize_name),
        Parser::Virtual(find_in_database),
        Parser::Section(parse_item_level),
        Parser::Section(parse_requirements),
        Parser::Section(parse_talisman_tier),
        Parser::Section(parse_gem),
        Parser::Section(parse_armour),
        Parser::Section(parse_weapon),
        Parser::Section(parse_caster),
        Parser::Section(parse_flask),
        Parser::Section(parse_jewelery),
        Parser::Section(parse_charm_slots),
        Parser::Section(parse_spirit),
        Parser::Section(parse_price_note),
        Parser::Section(parse_timelost_radius),
        Parser::Section(parse_stack_size),
        Parser::Section(parse_corrupted),
        Parser::Section(parse_foil),
        Parser::Section(parse_map),
        Parser::Section(parse_waystone),
        Parser::Section(parse_sockets),
        Parser::Section(parse_augment_sockets),
        Parser::Section(parse_trials),
        Parser::Section(parse_mirrored),
        Parser::Section(parse_sanctified),
        Parser::Section(parse_modifiers),
        Parser::Section(parse_modifiers),
        Parser::Section(parse_modifiers),
        Parser::Section(parse_modifiers),
        Parser::Section(parse_modifiers),
        Parser::Virtual(sum_stats),
        Parser::Virtual(parse_fractured),
        Parser::Virtual(apply_elemental_added),
        Parser::Virtual(pick_correct_variant),
        Parser::Virtual(noop),
        Parser::Virtual(noop),
    ];

    for parser in &parsers {
        match parser {
            Parser::Virtual(f) => f(&mut item, data)?,
            Parser::Section(f) => {
                let mut claimed = None;
                for (i, section) in sections.iter().enumerate() {
                    match f(section, &mut item, data)? {
                        Step::Parsed => {
                            claimed = Some(i);
                            break;
                        }
                        Step::ParserSkipped => break,
                        Step::SectionSkipped => {}
                    }
                }
                if let Some(i) = claimed {
                    sections.remove(i);
                }
            }
        }
    }
    collect_unread(&sections, &mut item, data);
    Ok(item)
}

/// Whatever is still in `sections` was claimed by no parser. Of those lines
/// the ones that are modifiers are kept for display: a line under a `{ }`
/// header, a line with a type marker, a granted skill, or a line the stat
/// data translates.
fn collect_unread(sections: &[Vec<String>], item: &mut ParsedItem, data: &Ee2Data) {
    if item.rarity.is_none() {
        return;
    }
    for section in sections {
        let headed = section.first().is_some_and(|l| is_mod_info_line(l));
        let mut reminder = false;
        for line in section {
            if is_mod_info_line(line) || line.is_empty() {
                continue;
            }
            if is_reminder_open(line) {
                reminder = true;
            }
            if reminder {
                reminder = !is_reminder_close(line);
                continue;
            }
            let (type_, stripped) = parse_mod_type(std::slice::from_ref(line));
            let text = stripped.into_iter().next().unwrap_or_default();
            let is_modifier = headed
                || type_ != ModType::Explicit
                || text.starts_with(GRANTS_SKILL)
                || try_parse_translation(&text, false, ModType::Explicit, data).is_some();
            if is_modifier {
                item.unread_modifiers.push(UnknownModifier { text, type_ });
            }
        }
    }
}

fn noop(_: &mut ParsedItem, _: &Ee2Data) -> Result<(), ParseError> {
    Ok(())
}

fn parse_name_plate(section: &[String]) -> Result<ParsedItem, ParseError> {
    let mut lines: std::collections::VecDeque<&str> = section.iter().map(String::as_str).collect();
    let first = lines.pop_front();
    let mut missing_item_class = false;
    if !first.is_some_and(|l| l.starts_with("Item Class: ")) {
        // Meta skill gems copy without an "Item Class:" line.
        let Some(line) = first else { return Err(ParseError::NotAnItem) };
        lines.push_front(line);
        let ok = (2..=3).contains(&lines.len()) && lines[0].starts_with("Rarity: ");
        if !ok {
            return Err(ParseError::NotAnItem);
        }
        missing_item_class = true;
    }
    let mut line = lines.pop_front();
    let mut rarity_text = None;
    if let Some(l) = line.filter(|l| l.starts_with("Rarity: ")) {
        rarity_text = Some(&l["Rarity: ".len()..]);
        line = lines.pop_front();
    }
    let name = markup_condition(line.ok_or(ParseError::NotAnItem)?);
    let base_type = lines.pop_front().map(markup_condition).filter(|b| !b.is_empty());

    let (mut rarity, mut category) = (None, None);
    match rarity_text {
        Some("Currency") => category = Some(cat::CURRENCY.to_string()),
        Some("Divination Card") => category = Some(cat::DIVINATION_CARD.to_string()),
        Some("Gem") => category = Some(cat::GEM.to_string()),
        Some("Normal") | Some("Quest") => rarity = Some(Rarity::Normal),
        Some("Magic") => rarity = Some(Rarity::Magic),
        Some("Rare") => rarity = Some(Rarity::Rare),
        Some("Unique") => rarity = Some(Rarity::Unique),
        _ => {}
    }
    if missing_item_class {
        category = Some(cat::GEM.to_string());
    }
    Ok(ParsedItem {
        rarity,
        category,
        name,
        base_type,
        info: BaseType::default(),
        info_variants: Vec::new(),
        raw_text: String::new(),
        is_unidentified: false,
        unidentified_tier: None,
        is_corrupted: false,
        is_unmodifiable: false,
        is_mirrored: false,
        is_sanctified: false,
        is_fractured: false,
        is_veiled: false,
        is_foil: false,
        item_level: None,
        requires_level: None,
        quality: None,
        quality_typed: false,
        gem_level: None,
        gem_sockets: None,
        stack_size: None,
        map_tier: None,
        map_revives: None,
        map_pack_size: None,
        map_magic_monsters: None,
        map_rare_monsters: None,
        map_drop_chance: None,
        map_item_rarity: None,
        map_monster_rarity: None,
        map_effectiveness: None,
        area_level: None,
        trials: None,
        talisman_tier: None,
        armour_ar: None,
        armour_ev: None,
        armour_es: None,
        armour_rw: None,
        armour_block: None,
        weapon_crit: None,
        weapon_as: None,
        weapon_physical: None,
        weapon_elemental: None,
        weapon_fire: None,
        weapon_cold: None,
        weapon_lightning: None,
        weapon_reload: None,
        weapon_spirit: None,
        augment_sockets: None,
        new_mods: Vec::new(),
        stats_by_type: Vec::new(),
        unknown_modifiers: Vec::new(),
        unread_modifiers: Vec::new(),
    })
}

fn parse_unidentified(section: &[String], item: &mut ParsedItem, _: &Ee2Data) -> Result<Step, ParseError> {
    let Some(rest) = section[0].strip_prefix("Unidentified") else { return Ok(Step::SectionSkipped) };
    if rest.is_empty() {
        item.is_unidentified = true;
        return Ok(Step::Parsed);
    }
    let tier = rest
        .trim_start()
        .strip_prefix("(Tier")
        .map(str::trim_start)
        .and_then(|t| t.strip_suffix(')'))
        .filter(|t| !t.is_empty() && t.chars().all(|c| c.is_ascii_digit()));
    match tier {
        Some(t) => {
            item.is_unidentified = true;
            item.unidentified_tier = t.parse().ok();
            Ok(Step::Parsed)
        }
        None => Ok(Step::SectionSkipped),
    }
}

fn parse_superior(item: &mut ParsedItem, _: &Ee2Data) -> Result<(), ParseError> {
    if item.rarity == Some(Rarity::Normal) || (item.rarity.is_some() && item.is_unidentified) {
        for prefix in ["Superior ", "Exceptional "] {
            if let Some(rest) = item.name.strip_prefix(prefix) {
                item.name = rest.to_string();
            }
        }
    }
    Ok(())
}

/// A magic item's name is its base wrapped in affix words; the base is the
/// longest run of words that names a craftable base.
fn magic_basetype(name: &str, data: &Ee2Data) -> Option<String> {
    let words: Vec<&str> = name.split(' ').collect();
    // (name, known only to the trade site)
    let mut best: Option<(String, bool)> = None;
    for start in 0..words.len() {
        for end in start..words.len() {
            let candidate = words[start..=end].join(" ");
            let mut found = data.items.by_ref("ITEM", &candidate);
            if found.is_empty() {
                found = data.items.by_translated("ITEM", &candidate);
            }
            let trade_only = found.is_empty();
            let craftable = match found.first() {
                Some(b) => b.craftable.is_some(),
                None => data.trade_items.as_ref().is_some_and(|n| n.contains(&candidate)),
            };
            if !craftable {
                continue;
            }
            // A base items.ndjson knows beats one only the trade site
            // lists; among equals the longer name wins, the first on a tie.
            let better = best.as_ref().is_none_or(|(b, b_trade)| {
                (*b_trade && !trade_only) || (*b_trade == trade_only && candidate.len() > b.len())
            });
            if better {
                best = Some((candidate, trade_only));
            }
        }
    }
    best.map(|(name, _)| name)
}

fn normalize_name(item: &mut ParsedItem, data: &Ee2Data) -> Result<(), ParseError> {
    if item.rarity == Some(Rarity::Magic) {
        if let Some(base) = magic_basetype(&item.name, data) {
            item.name = base;
        }
    }
    Ok(())
}

/// An item items.ndjson does not list but the trade site does: enough of a
/// record to search it by name.
fn trade_item_by_ref(item: &ParsedItem, data: &Ee2Data) -> Option<BaseType> {
    let names = data.trade_items.as_ref()?;
    let base = |name: &str, ns: &str| BaseType {
        name: name.to_string(),
        ref_name: name.to_string(),
        namespace: ns.to_string(),
        ..Default::default()
    };
    let craftable = |category: &str| Some(super::data::Craftable { category: category.to_string() });
    if item.category.as_deref().is_some_and(cat::is_gem) {
        return names
            .contains(&item.name)
            .then(|| BaseType { gem: Some(Default::default()), ..base(&item.name, "GEM") });
    }
    if item.rarity == Some(Rarity::Unique) {
        let base_type = item.base_type.clone().unwrap_or_default();
        return names.contains(&format!("{} {}", item.name, base_type)).then(|| BaseType {
            unique: Some(super::data::UniqueInfo { base: base_type, fixed_stats: None }),
            ..base(&item.name, "UNIQUE")
        });
    }
    match &item.base_type {
        None => names.contains(&item.name).then(|| BaseType {
            craftable: item.category.as_deref().and_then(craftable),
            ..base(&item.name, "ITEM")
        }),
        Some(b) => names.contains(b).then(|| BaseType { craftable: craftable("Unknown"), ..base(b, "ITEM") }),
    }
}

fn find_in_database(item: &mut ParsedItem, data: &Ee2Data) -> Result<(), ParseError> {
    let is_gem = item.category.as_deref().is_some_and(cat::is_gem);
    let lookup_name = item.base_type.clone().unwrap_or_else(|| item.name.clone());
    let (ns, name) = if item.category_is(cat::DIVINATION_CARD) {
        ("DIVINATION_CARD", item.name.clone())
    } else if is_gem {
        ("GEM", item.name.clone())
    } else if item.rarity == Some(Rarity::Unique) && !item.is_unidentified {
        ("UNIQUE", item.name.clone())
    } else {
        ("ITEM", lookup_name)
    };
    let mut info: Vec<BaseType> = data.items.by_ref(ns, &name).into_iter().cloned().collect();
    if info.is_empty() {
        info = data.items.by_translated(ns, &name).into_iter().cloned().collect();
    }
    if info.is_empty() {
        info = trade_item_by_ref(item, data).into_iter().collect();
    }
    if info.is_empty() {
        return Err(ParseError::UnknownItem(name));
    }
    if info[0].unique.is_some() {
        let same_base: Vec<BaseType> = info
            .iter()
            .filter(|i| i.unique.as_ref().map(|u| &u.base) == item.base_type.as_ref())
            .cloned()
            .collect();
        if !same_base.is_empty() {
            info = same_base;
        } else if let Some(base) = &item.base_type {
            if let Some(base_info) = data.items.by_translated("ITEM", base).first() {
                let ref_name = base_info.ref_name.clone();
                info.retain(|i| i.unique.as_ref().is_some_and(|u| u.base == ref_name));
            }
        }
    }
    let Some(first) = info.first().cloned() else { return Err(ParseError::UnknownItem(name)) };
    item.info_variants = info;
    item.info = first;
    let wombgift = item.info.craftable.as_ref().is_some_and(|c| c.category == cat::WOMBGIFT);
    if item.category.is_none() || wombgift {
        if let Some(c) = &item.info.craftable {
            item.category = Some(c.category.clone());
        } else if let Some(u) = &item.info.unique {
            let base = data.items.by_ref("ITEM", &u.base);
            let category = base.first().and_then(|b| b.craftable.as_ref()).map(|c| c.category.clone());
            item.category = Some(category.ok_or_else(|| ParseError::UnknownItem(u.base.clone()))?);
        }
    }
    Ok(())
}

fn parse_item_level(section: &[String], item: &mut ParsedItem, _: &Ee2Data) -> Result<Step, ParseError> {
    for line in section {
        if let Some(v) = line.strip_prefix("Item Level: ") {
            item.item_level = js_number(v);
            return Ok(Step::Parsed);
        }
    }
    Ok(Step::SectionSkipped)
}

fn parse_requirements(section: &[String], item: &mut ParsedItem, _: &Ee2Data) -> Result<Step, ParseError> {
    let Some(rest) = section[0].strip_prefix("Requires") else { return Ok(Step::SectionSkipped) };
    if item.category.as_deref().is_some_and(cat::is_gem) {
        return Ok(Step::SectionSkipped);
    }
    // "Requires: Level 78, 89 Str, 89 Dex": only the level is searched on.
    let rest = rest.strip_prefix(": ").ok_or(ParseError::Requirements)?;
    let level = rest.trim_start().strip_prefix("Level").map(|after| {
        let digits: String = after
            .chars()
            .skip_while(|c| !c.is_ascii_digit() && *c != ',')
            .take_while(|c| c.is_ascii_digit())
            .collect();
        digits.parse::<f64>().unwrap_or(0.0)
    });
    item.requires_level = Some(level.unwrap_or(0.0));
    Ok(Step::Parsed)
}

fn parse_talisman_tier(section: &[String], item: &mut ParsedItem, _: &Ee2Data) -> Result<Step, ParseError> {
    match section[0].strip_prefix("Talisman Tier: ") {
        Some(v) => {
            item.talisman_tier = js_number(v);
            Ok(Step::Parsed)
        }
        None => Ok(Step::SectionSkipped),
    }
}

fn parse_quality_nested(section: &[String], item: &mut ParsedItem) {
    for line in section {
        if let Some(v) = line.strip_prefix("Quality: ") {
            item.quality = js_parse_int(v);
            if item.category_is(cat::RING) || item.category_is(cat::AMULET) || item.category_is(cat::JEWEL) {
                const CATALYST_TAGS: [&str; 15] = [
                    "Life", "Mana", "Armour", "Evasion", "Energy Shield", "Physical", "Fire", "Cold",
                    "Lightning", "Chaos", "Attack", "Caster", "Speed", "Attribute", "Minion",
                ];
                if CATALYST_TAGS.iter().any(|t| line.contains(t)) {
                    item.quality_typed = true;
                }
            }
            return;
        }
    }
}

fn parse_gem(section: &[String], item: &mut ParsedItem, _: &Ee2Data) -> Result<Step, ParseError> {
    let gem = item.category_is(cat::GEM) || item.category_is(cat::META_GEM);
    if !gem && !item.category_is(cat::UNCUT_GEM) {
        return Ok(Step::ParserSkipped);
    }
    let line_no = if gem { 1 } else { 0 };
    if let Some(v) = section.get(line_no).and_then(|l| l.strip_prefix("Level: ")) {
        item.gem_level = js_parse_int(v);
        parse_quality_nested(section, item);
        return Ok(Step::Parsed);
    }
    Ok(Step::SectionSkipped)
}

fn parse_armour(section: &[String], item: &mut ParsedItem, _: &Ee2Data) -> Result<Step, ParseError> {
    let mut parsed = false;
    for line in section {
        let fields: [(&str, &mut Option<f64>); 5] = [
            ("Armour: ", &mut item.armour_ar),
            ("Evasion Rating: ", &mut item.armour_ev),
            ("Energy Shield: ", &mut item.armour_es),
            ("Block chance: ", &mut item.armour_block),
            ("Runic Ward: ", &mut item.armour_rw),
        ];
        for (prefix, slot) in fields {
            if let Some(v) = line.strip_prefix(prefix) {
                *slot = js_parse_int(v);
                parsed = true;
                break;
            }
        }
    }
    if parsed {
        parse_quality_nested(section, item);
    }
    // A unique's defences follow from its name; they are not compared.
    if item.rarity == Some(Rarity::Unique) {
        item.armour_ar = None;
        item.armour_ev = None;
        item.armour_es = None;
        item.armour_rw = None;
        item.armour_block = None;
    }
    Ok(if parsed { Step::Parsed } else { Step::SectionSkipped })
}

fn damage_range_avg(text: &str) -> f64 {
    text.split(", ")
        .map(|element| {
            let nums: Vec<f64> = element
                .split('-')
                .map(|s| {
                    let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
                    digits.parse::<f64>().unwrap_or(f64::NAN)
                })
                .collect();
            roll_or_minmax_avg(&nums).unwrap_or(f64::NAN)
        })
        .sum()
}

fn parse_weapon(section: &[String], item: &mut ParsedItem, _: &Ee2Data) -> Result<Step, ParseError> {
    let mut parsed = false;
    for line in section {
        if let Some(v) = line.strip_prefix("Critical Hit Chance: ") {
            item.weapon_crit = js_parse_float(v);
        } else if let Some(v) = line.strip_prefix("Attacks per Second: ") {
            item.weapon_as = js_parse_float(v);
        } else if let Some(v) = line.strip_prefix("Physical Damage: ") {
            item.weapon_physical = Some(damage_range_avg(v)).filter(|x| !x.is_nan());
        } else if let Some(v) = line.strip_prefix("Elemental Damage: ") {
            item.weapon_elemental = Some(damage_range_avg(v)).filter(|x| !x.is_nan());
        } else if let Some(v) = ["Fire Damage: ", "Cold Damage: ", "Lightning Damage: "]
            .iter()
            .find_map(|p| line.strip_prefix(p))
        {
            let dmg = damage_range_avg(v);
            // Zero reads as absent, the way the sum starts over in EE2.
            let so_far = item.weapon_elemental.filter(|e| *e != 0.0).unwrap_or(0.0);
            item.weapon_elemental = Some(dmg + so_far).filter(|x| !x.is_nan());
        } else if let Some(v) = line.strip_prefix("Reload Time: ") {
            item.weapon_reload = js_parse_float(v);
        } else if let Some(v) = line.strip_prefix("Spirit: ") {
            item.weapon_spirit = js_parse_int(v);
        } else {
            continue;
        }
        parsed = true;
    }
    if parsed {
        parse_quality_nested(section, item);
    }
    if item.rarity == Some(Rarity::Unique) {
        item.weapon_elemental = None;
        item.weapon_as = None;
        item.weapon_physical = None;
        item.weapon_crit = None;
        item.weapon_reload = None;
        item.weapon_spirit = None;
    }
    Ok(if parsed { Step::Parsed } else { Step::SectionSkipped })
}

fn parse_caster(section: &[String], item: &mut ParsedItem, _: &Ee2Data) -> Result<Step, ParseError> {
    if !(item.category_is(cat::WAND) || item.category_is(cat::SCEPTRE) || item.category_is(cat::STAFF)) {
        return Ok(Step::ParserSkipped);
    }
    if section.len() == 1 && section[0].starts_with("Quality: ") {
        parse_quality_nested(section, item);
        return Ok(Step::Parsed);
    }
    Ok(Step::SectionSkipped)
}

fn parse_flask(section: &[String], item: &mut ParsedItem, _: &Ee2Data) -> Result<Step, ParseError> {
    // Claims the "Currently has N Charges" section so the flask's buff text
    // is not read as modifiers.
    let parsed = section.iter().any(|line| {
        line.strip_prefix("Currently has ")
            .and_then(|r| r.strip_suffix(" Charges"))
            .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
    });
    // EE2 reads quality here off every section this parser is shown, flask
    // or not; it is how a quality line outside the property block of a
    // weapon or armour piece still reaches the item.
    parse_quality_nested(section, item);
    Ok(if parsed { Step::Parsed } else { Step::SectionSkipped })
}

fn parse_jewelery(section: &[String], item: &mut ParsedItem, _: &Ee2Data) -> Result<Step, ParseError> {
    if !(item.category_is(cat::AMULET) || item.category_is(cat::RING) || item.category_is(cat::BELT)) {
        return Ok(Step::ParserSkipped);
    }
    Ok(if section.iter().any(|l| l.starts_with("Quality")) { Step::Parsed } else { Step::SectionSkipped })
}

fn parse_charm_slots(section: &[String], item: &mut ParsedItem, _: &Ee2Data) -> Result<Step, ParseError> {
    if !item.category_is(cat::BELT) {
        return Ok(Step::ParserSkipped);
    }
    Ok(if section.iter().any(|l| l.starts_with("Charm Slots: ")) { Step::Parsed } else { Step::SectionSkipped })
}

fn parse_spirit(section: &[String], item: &mut ParsedItem, _: &Ee2Data) -> Result<Step, ParseError> {
    if !item.category_is(cat::SCEPTRE) {
        return Ok(Step::ParserSkipped);
    }
    Ok(if section.iter().any(|l| l.starts_with("Spirit: ")) { Step::Parsed } else { Step::SectionSkipped })
}

fn parse_price_note(section: &[String], _: &mut ParsedItem, _: &Ee2Data) -> Result<Step, ParseError> {
    Ok(if section.iter().any(|l| l.starts_with("Note: ")) { Step::Parsed } else { Step::SectionSkipped })
}

fn parse_timelost_radius(section: &[String], item: &mut ParsedItem, _: &Ee2Data) -> Result<Step, ParseError> {
    if !item.category_is(cat::JEWEL) {
        return Ok(Step::ParserSkipped);
    }
    Ok(if section.iter().any(|l| l.starts_with("Radius: ")) { Step::Parsed } else { Step::SectionSkipped })
}

fn parse_stack_size(section: &[String], item: &mut ParsedItem, _: &Ee2Data) -> Result<Step, ParseError> {
    if item.rarity != Some(Rarity::Normal)
        && !item.category_is(cat::CURRENCY)
        && !item.category_is(cat::DIVINATION_CARD)
        && !item.category_is(cat::MAP_FRAGMENT)
    {
        return Ok(Step::ParserSkipped);
    }
    let Some(v) = section[0].strip_prefix("Stack Size: ") else { return Ok(Step::SectionSkipped) };
    // "2[separator]448/40": everything but digits and the slash is noise.
    let cleaned: String = v.chars().filter(|c| c.is_ascii_digit() || *c == '/').collect();
    let mut parts = cleaned.split('/').map(|p| js_number(p).unwrap_or(f64::NAN));
    let value = parts.next().unwrap_or(f64::NAN);
    let max = parts.next().unwrap_or(f64::NAN);
    if item.info.ref_name != "Idol of Estazunti" {
        item.stack_size = Some((value, max));
    }
    Ok(Step::Parsed)
}

fn parse_corrupted(section: &[String], item: &mut ParsedItem, _: &Ee2Data) -> Result<Step, ParseError> {
    let first = section[0].trim();
    if first == "Corrupted" || first == "Twice Corrupted" {
        item.is_corrupted = true;
        Ok(Step::Parsed)
    } else if section[0] == "Unmodifiable" {
        item.is_corrupted = true;
        item.is_unmodifiable = true;
        Ok(Step::Parsed)
    } else {
        Ok(Step::SectionSkipped)
    }
}

fn parse_foil(section: &[String], item: &mut ParsedItem, _: &Ee2Data) -> Result<Step, ParseError> {
    if item.rarity != Some(Rarity::Unique) {
        return Ok(Step::ParserSkipped);
    }
    if section[0] == "Foil Unique" {
        item.is_foil = true;
        return Ok(Step::Parsed);
    }
    Ok(Step::SectionSkipped)
}

fn parse_map(section: &[String], item: &mut ParsedItem, _: &Ee2Data) -> Result<Step, ParseError> {
    match section[0].strip_prefix("Map Tier: ") {
        Some(v) => {
            item.map_tier = js_number(v);
            Ok(Step::Parsed)
        }
        None => Ok(Step::SectionSkipped),
    }
}

fn parse_waystone(section: &[String], item: &mut ParsedItem, _: &Ee2Data) -> Result<Step, ParseError> {
    let tier = item.info.map.as_ref().and_then(|m| m.tier).filter(|t| *t != 0.0);
    let (true, Some(tier)) = (section[0].starts_with("Revives Available: "), tier) else {
        return Ok(Step::SectionSkipped);
    };
    item.map_tier = Some(tier);
    for line in section {
        let fields: [(&str, &mut Option<f64>); 8] = [
            ("Revives Available: ", &mut item.map_revives),
            ("Pack Size: ", &mut item.map_pack_size),
            ("Magic Monsters: ", &mut item.map_magic_monsters),
            ("Rare Monsters: ", &mut item.map_rare_monsters),
            ("Waystone Drop Chance: ", &mut item.map_drop_chance),
            ("Item Rarity: ", &mut item.map_item_rarity),
            ("Monster Rarity: ", &mut item.map_monster_rarity),
            ("Monster Effectiveness: ", &mut item.map_effectiveness),
        ];
        for (prefix, slot) in fields {
            if let Some(v) = line.strip_prefix(prefix) {
                *slot = js_parse_int(v);
                break;
            }
        }
    }
    Ok(Step::Parsed)
}

fn parse_sockets(section: &[String], item: &mut ParsedItem, _: &Ee2Data) -> Result<Step, ParseError> {
    if !item.category.as_deref().is_some_and(cat::is_gem) {
        return Ok(Step::SectionSkipped);
    }
    let Some(v) = section[0].strip_prefix("Sockets: ") else { return Ok(Step::SectionSkipped) };
    item.gem_sockets = Some(v.trim_end().chars().filter(|c| *c != ' ' && *c != '-').count() as u32);
    Ok(Step::Parsed)
}

/// Rune sockets a base of this kind comes with.
pub fn max_sockets(item: &ParsedItem) -> u32 {
    match item.info.ref_name.as_str() {
        "Atziri's Splendour" => return 6,
        "Runeseeker's Call" => return 5,
        "Greymake" | "Morior Invictus" | "The Bringer of Rain" => return 4,
        "Serle's Grit" => return 3,
        "Darkness Enthroned" | "Mahuxotl's Machination" => return 2,
        "Grasping Ring" | "Corona Amulet" | "Stalking Belt" => return 1,
        _ => {}
    }
    match item.category.as_deref() {
        Some(
            cat::BODY_ARMOUR | cat::TWO_HAND_AXE | cat::TWO_HAND_MACE | cat::TWO_HAND_SWORD
            | cat::CROSSBOW | cat::BOW | cat::WARSTAFF | cat::STAFF | cat::TALISMAN,
        ) => 2,
        Some(
            cat::HELMET | cat::SHIELD | cat::GLOVES | cat::BOOTS | cat::ONE_HAND_AXE
            | cat::ONE_HAND_MACE | cat::ONE_HAND_SWORD | cat::CLAW | cat::DAGGER | cat::FOCUS
            | cat::SPEAR | cat::FLAIL | cat::WAND | cat::BUCKLER | cat::SCEPTRE,
        ) => 1,
        _ => 0,
    }
}

fn parse_augment_sockets(section: &[String], item: &mut ParsedItem, _: &Ee2Data) -> Result<Step, ParseError> {
    let normal = max_sockets(item);
    if normal == 0 {
        return Ok(Step::ParserSkipped);
    }
    if let Some(v) = section[0].strip_prefix("Sockets: ") {
        let current = v.trim_end().matches('S').count() as u32;
        item.augment_sockets = Some(AugmentSockets { current, normal });
        return Ok(Step::Parsed);
    }
    if item.is_modifiable() {
        item.augment_sockets = Some(AugmentSockets { current: 0, normal });
    }
    Ok(Step::SectionSkipped)
}

fn parse_trials(section: &[String], item: &mut ParsedItem, _: &Ee2Data) -> Result<Step, ParseError> {
    if item.info.ref_name != "Djinn Barya" && item.info.ref_name != "Inscribed Ultimatum" {
        return Ok(Step::ParserSkipped);
    }
    for line in section {
        if let Some(v) = line.strip_prefix("Area Level: ") {
            item.area_level = js_number(v);
            break;
        }
    }
    if item.area_level.is_none_or(|l| l == 0.0) {
        return Ok(Step::SectionSkipped);
    }
    let mut trials = Trials::default();
    for line in section {
        if let Some(v) = line.strip_prefix("Number of Trials: ") {
            trials.number_of_trials = js_number(v);
        }
    }
    item.trials = Some(trials);
    Ok(Step::Parsed)
}

fn parse_mirrored(section: &[String], item: &mut ParsedItem, _: &Ee2Data) -> Result<Step, ParseError> {
    if section.len() == 1 && section[0] == "Mirrored" {
        item.is_mirrored = true;
        return Ok(Step::Parsed);
    }
    Ok(Step::SectionSkipped)
}

fn parse_sanctified(section: &[String], item: &mut ParsedItem, _: &Ee2Data) -> Result<Step, ParseError> {
    if section.len() == 1 && section[0] == "Sanctified" {
        item.is_sanctified = true;
        return Ok(Step::Parsed);
    }
    Ok(Step::SectionSkipped)
}

fn is_mod_info_line(line: &str) -> bool {
    line.starts_with('{') && line.ends_with('}')
}

fn strip_ending(lines: &[String], ending: &str) -> Vec<String> {
    lines.iter().map(|l| l.strip_suffix(ending).unwrap_or(l).to_string()).collect()
}

fn parse_mod_type(lines: &[String]) -> (ModType, Vec<String>) {
    if lines.first().is_some_and(|l| l == "Desecrated Prefix" || l == "Desecrated Suffix") {
        return (ModType::Veiled, lines.to_vec());
    }
    // The order is EE2's: a line pair that mixes markers takes the first
    // marker in this list.
    let order = [
        (SCOURGE_LINE, ModType::Scourge),
        (ENCHANT_LINE, ModType::Enchant),
        (IMPLICIT_LINE, ModType::Implicit),
        (FRACTURED_LINE, ModType::Fractured),
        (CRAFTED_LINE, ModType::Crafted),
        (AUGMENT_LINE, ModType::Augment),
        (ADDED_AUGMENT_LINE, ModType::AddedAugment),
        (DESECRATED_LINE, ModType::Desecrated),
    ];
    for (ending, type_) in order {
        if lines.iter().any(|l| l.ends_with(ending)) {
            return (type_, strip_ending(lines, ending));
        }
    }
    (ModType::Explicit, lines.to_vec())
}

fn parse_mod_info_line(line: &str, mut type_: ModType) -> Result<ModInfo, ParseError> {
    let bad = || ParseError::ModifierHeader(line.to_string());
    let inner: String = {
        let chars: Vec<char> = line.chars().collect();
        chars[1..chars.len() - 1].iter().collect()
    };
    let parts: Vec<&str> = inner.split('\u{2014}').map(str::trim).collect();
    let mod_text = parts[0];

    // `<type> "<name>" (Tier: n) (Rank: n)`, everything after the type
    // optional. Without a quoted name the whole text is the type.
    let (mut kind, name, tier) = match mod_text.find('"') {
        None => (mod_text.to_string(), None, None),
        Some(q) => {
            let before = &mod_text[..q];
            let last = before.chars().last().filter(|c| c.is_whitespace()).ok_or_else(bad)?;
            let kind = before[..before.len() - last.len_utf8()].to_string();
            if kind.is_empty() {
                return Err(bad());
            }
            let after = &mod_text[q + 1..];
            let close = after.find('"').ok_or_else(bad)?;
            let name = &after[..close];
            let mut rest = after[close + 1..].trim_start();
            let mut tier = None;
            if let Some(t) = rest.strip_prefix("(Tier: ") {
                let end = t.find(')').ok_or_else(bad)?;
                tier = t[..end].parse::<u32>().ok();
                if tier.is_none() {
                    return Err(bad());
                }
                rest = t[end + 1..].trim_start();
            }
            if let Some(r) = rest.strip_prefix("(Rank: ") {
                let end = r.find(')').ok_or_else(bad)?;
                rest = r[end + 1..].trim_start();
            }
            if !rest.is_empty() {
                return Err(bad());
            }
            (kind, Some(name.to_string()).filter(|n| !n.is_empty()), tier.filter(|t| *t != 0))
        }
    };

    if let Some(rest) = kind.strip_prefix("Fractured") {
        kind = rest.trim().to_string();
        type_ = ModType::Fractured;
    }
    if let Some(rest) = kind.strip_prefix("Desecrated") {
        kind = rest.trim().to_string();
        if type_ != ModType::Fractured {
            type_ = ModType::Desecrated;
        }
    } else if let Some(rest) = kind.strip_prefix("Crafted") {
        kind = rest.trim().to_string();
        if type_ != ModType::Fractured {
            type_ = ModType::Crafted;
        }
    }

    let mut generation = None;
    match kind.as_str() {
        "Prefix Modifier" => generation = Some(Generation::Prefix),
        "Suffix Modifier" => generation = Some(Generation::Suffix),
        "Corruption Enhancement" => {
            generation = Some(Generation::Corrupted);
            type_ = ModType::Enchant;
        }
        "Implicit Modifier" => type_ = ModType::Implicit,
        "Enhancement" => type_ = ModType::Enchant,
        "Vaal Unique Modifier" => generation = Some(Generation::Mutated),
        _ => {}
    }

    let increased = |t: &str| t.strip_suffix("% Increased").map(str::to_string);
    let incr_text = match (parts.get(1), parts.get(2)) {
        (_, Some(x3)) => Some(*x3),
        (Some(x2), None) if increased(x2).is_some() => Some(*x2),
        _ => None,
    };
    let tags_text = parts.get(1).filter(|x2| Some(**x2) != incr_text);
    let tags = tags_text
        .filter(|t| !t.is_empty())
        .map(|t| t.split(", ").map(str::to_string).collect())
        .unwrap_or_default();
    let roll_incr = incr_text.and_then(increased).and_then(|n| js_number(&n));

    Ok(ModInfo { type_, generation, name, tier, tags, roll_incr })
}

fn parse_modifiers(section: &[String], item: &mut ParsedItem, data: &Ee2Data) -> Result<Step, ParseError> {
    if item.rarity.is_none() {
        return Ok(Step::ParserSkipped);
    }
    let recognized = section.iter().find(|line| {
        line.ends_with(ENCHANT_LINE)
            || line.ends_with(AUGMENT_LINE)
            || line.starts_with(GRANTS_SKILL)
            || is_mod_info_line(line)
    });
    let Some(recognized) = recognized else { return Ok(Step::SectionSkipped) };

    if is_mod_info_line(recognized) {
        if !is_mod_info_line(&section[0]) {
            return Ok(Step::Parsed);
        }
        let mut groups: Vec<(String, Vec<String>)> = Vec::new();
        for line in section {
            if is_mod_info_line(line) {
                groups.push((line.clone(), Vec::new()));
            } else if let Some(last) = groups.last_mut() {
                last.1.push(line.clone());
            }
        }
        for (mod_line, stat_lines) in groups {
            let (mod_type, lines) = parse_mod_type(&stat_lines);
            let mut info = parse_mod_info_line(&mod_line, mod_type)?;
            if item.category_is(cat::RELIC) && info.type_ == ModType::Explicit {
                info.type_ = ModType::Sanctum;
            }
            parse_stats_from_mod(&lines, item, info, data);
            if mod_type == ModType::Veiled {
                item.is_veiled = true;
            }
        }
    } else {
        let (_, lines) = parse_mod_type(section);
        let type_ = if recognized.ends_with(ENCHANT_LINE) {
            ModType::Enchant
        } else if recognized.starts_with(GRANTS_SKILL) {
            ModType::Skill
        } else {
            ModType::Augment
        };
        parse_stats_from_mod(&lines, item, ModInfo::bare(type_), data);
    }
    Ok(Step::Parsed)
}

fn is_reminder_open(line: &str) -> bool {
    line.trim_start().starts_with(['(', '\u{FF08}'])
}

fn is_reminder_close(line: &str) -> bool {
    line.trim_end().ends_with([')', '\u{FF09}'])
}

fn parse_stats_from_mod(lines: &[String], item: &mut ParsedItem, info: ModInfo, data: &Ee2Data) {
    let mod_type = info.type_;
    let mut modifier = ParsedModifier { info, stats: Vec::new() };
    if mod_type == ModType::Veiled {
        item.new_mods.push(modifier);
        item.unread_modifiers
            .extend(lines.iter().filter(|l| !l.is_empty()).map(|l| UnknownModifier { text: l.clone(), type_: mod_type }));
        return;
    }
    let mut not_parsed: Vec<String> = Vec::new();
    let mut reminder = false;
    let mut start = 0;
    while start < lines.len() {
        if is_reminder_open(&lines[start]) {
            reminder = true;
        }
        if reminder && is_reminder_close(&lines[start]) {
            reminder = false;
            start += 1;
            continue;
        }
        if reminder {
            start += 1;
            continue;
        }
        // A stat may span several lines: the line alone first, then with
        // each following line joined on, until a translation is found.
        let found = (start..lines.len()).find_map(|end| {
            let mut text = lines[start..=end].join("\n");
            let unscalable = text.ends_with(UNSCALABLE_VALUE);
            if unscalable {
                text.truncate(text.len() - UNSCALABLE_VALUE.len());
            }
            if item.info.ref_name == "From Nothing" {
                text = text.replacen("()", "", 1);
            }
            try_parse_translation(&text, unscalable, mod_type, data).map(|parsed| (end, parsed))
        });
        if let Some((end, mut parsed)) = found {
            parsed.lines = lines[start..=end].to_vec();
            parsed.ordinal = item.new_mods.iter().map(|m| m.stats.len()).sum::<usize>() + modifier.stats.len();
            // Skills every weapon of the kind grants are not a modifier.
            let builtin = matches!(
                parsed.stat.ref_.as_str(),
                "Grants Skill: Parry" | "Grants Skill: Raise Shield" | "Grants Skill: Spear Throw"
            );
            if !builtin {
                modifier.stats.push(parsed);
            }
            start = end + 1;
            continue;
        }
        not_parsed.push(lines[start].clone());
        start += 1;
    }
    item.new_mods.push(modifier);
    item.unknown_modifiers.extend(
        not_parsed
            .into_iter()
            .filter(|l| !l.is_empty())
            .map(|text| UnknownModifier { text, type_: mod_type }),
    );
}

#[derive(Debug, Clone)]
struct Captured {
    roll: f64,
    roll_str: String,
    decimal: bool,
    bounds: Option<(f64, f64)>,
}

/// Which captured numbers stay literal in the match string, tried in this
/// order; the rest become `#` and are the roll.
fn placeholder_map(n: usize) -> &'static [&'static [usize]] {
    match n {
        0 => &[&[]],
        1 => &[&[0], &[]],
        2 => &[&[0, 1], &[0], &[1], &[]],
        3 => &[&[0, 1, 2], &[1, 2], &[0, 2], &[0, 1], &[2], &[1], &[0], &[]],
        4 => &[
            &[0, 1, 2, 3],
            &[1, 2, 3],
            &[0, 2, 3],
            &[0, 1, 3],
            &[0, 1, 2],
            &[2, 3],
            &[1, 3],
            &[1, 2],
            &[0, 3],
            &[0, 2],
            &[0, 1],
            &[3],
            &[2],
            &[1],
            &[0],
            &[],
        ],
        _ => &[],
    }
}

/// Replaces every number, with its `(lo-hi)` annotation, by `#`.
fn capture_numbers(stat: &str) -> (String, Vec<Captured>) {
    let src: Vec<char> = stat.replace("()", "").chars().collect();
    let mut out = String::new();
    let mut found = Vec::new();
    let mut i = 0;
    while i < src.len() {
        let after_number = i > 0 && (src[i - 1].is_ascii_digit() || src[i - 1] == ')');
        let mut j = i;
        if !after_number && j < src.len() && (src[j] == '+' || src[j] == '-') {
            j += 1;
        }
        let digits_start = j;
        while !after_number && j < src.len() && src[j].is_ascii_digit() {
            j += 1;
        }
        if after_number || j == digits_start {
            out.push(src[i]);
            i += 1;
            continue;
        }
        if j + 1 < src.len() && src[j] == '.' && src[j + 1].is_ascii_digit() {
            j += 1;
            while j < src.len() && src[j].is_ascii_digit() {
                j += 1;
            }
        }
        let roll_str: String = src[i..j].iter().collect();
        // Optional "(min)" or "(min-max)".
        let mut bounds_text: Option<(String, Option<String>)> = None;
        let mut end = j;
        if j + 1 < src.len() && src[j] == '(' && src[j + 1] != '\n' {
            let mut k = j + 2;
            while k < src.len() && src[k] != ')' && src[k] != '-' {
                k += 1;
            }
            if k < src.len() && src[k] == ')' {
                bounds_text = Some((src[j + 1..k].iter().collect(), None));
                end = k + 1;
            } else if k < src.len() {
                let mut m = k + 1;
                while m < src.len() && src[m] != ')' {
                    m += 1;
                }
                if m < src.len() && m > k + 1 {
                    bounds_text = Some((src[j + 1..k].iter().collect(), Some(src[k + 1..m].iter().collect())));
                    end = m + 1;
                }
            }
        }
        let (min_s, max_s) = match &bounds_text {
            Some((min, max)) => (Some(min.clone()), Some(max.clone().unwrap_or_else(|| min.clone()))),
            None => (None, None),
        };
        let decimal = roll_str.contains('.')
            || min_s.as_ref().is_some_and(|s| s.contains('.'))
            || max_s.as_ref().is_some_and(|s| s.contains('.'));
        let bounds = match (&min_s, &max_s) {
            (Some(a), Some(b)) => js_number(a).zip(js_number(b)),
            _ => None,
        };
        out.push('#');
        if bounds.is_none() {
            if let (Some(a), Some(b)) = (&min_s, &max_s) {
                out.push_str(&format!("({a}-{b})"));
            }
        }
        found.push(Captured { roll: js_number(&roll_str).unwrap_or(f64::NAN), roll_str, decimal, bounds });
        i = end;
    }
    (out, found)
}

fn try_parse_translation(text: &str, unscalable: bool, mod_type: ModType, data: &Ee2Data) -> Option<ParsedStat> {
    let (with_placeholders, captured) = capture_numbers(text);
    let mut combinations: Vec<(String, Vec<Captured>)> = Vec::new();
    for literal in placeholder_map(captured.len()) {
        let mut idx = 0usize;
        let mut replaced = String::new();
        for c in with_placeholders.chars() {
            if c == '#' {
                match captured.get(idx) {
                    Some(cap) if literal.contains(&idx) => replaced.push_str(&cap.roll_str),
                    _ => replaced.push('#'),
                }
                idx += 1;
            } else {
                replaced.push(c);
            }
        }
        let values = captured
            .iter()
            .enumerate()
            .filter(|(i, _)| !literal.contains(i))
            .map(|(_, c)| c.clone())
            .collect();
        combinations.push((replaced, values));
    }
    combinations.push((text.to_string(), Vec::new()));

    let mut backup: Option<(ParsedStat, Vec<Captured>)> = None;
    for (match_str, values) in combinations {
        let found = data.stats.by_match_str(&match_str).filter(|(stat, _)| {
            stat.ids(mod_type.trade_key()).is_some() || stat.ref_.starts_with(GRANTS_SKILL)
        });
        match found {
            Some((stat, matcher)) => {
                let roll = parse_roll(&stat, &matcher, values, unscalable);
                return Some(ParsedStat { stat, translation: matcher, roll, lines: Vec::new(), ordinal: 0 });
            }
            None if backup.is_none() => {
                // The trade site's catalog lists the line under this exact
                // text even though stats.ndjson does not.
                if let Some(ids) = data.trade_stats.as_ref().and_then(|t| t.get(&match_str)) {
                    let stat = Arc::new(Stat {
                        ref_: match_str.clone(),
                        better: Better::Positive,
                        dp: false,
                        matchers: vec![plain_matcher(&match_str)],
                        trade_ids: ids.clone(),
                        inverted: false,
                        option: false,
                    });
                    let parsed =
                        ParsedStat { stat, translation: plain_matcher(&match_str), roll: None, lines: Vec::new(), ordinal: 0 };
                    backup = Some((parsed, values));
                }
            }
            None => {}
        }
    }
    backup.map(|(mut parsed, values)| {
        parsed.roll = parse_roll(&parsed.stat, &parsed.translation, values, unscalable);
        parsed
    })
}

pub fn plain_matcher(s: &str) -> Matcher {
    Matcher { string: s.to_string(), advanced: None, negate: false, value: None, oils: None }
}

fn parse_roll(stat: &Stat, matcher: &Matcher, mut values: Vec<Captured>, unscalable: bool) -> Option<Roll> {
    if matcher.negate {
        for v in &mut values {
            v.roll *= -1.0;
            if let Some(b) = &mut v.bounds {
                b.0 *= -1.0;
                b.1 *= -1.0;
            }
        }
    }
    if stat.ref_ == "# uses remaining" {
        if let Some(uses) = values.first_mut() {
            uses.bounds = Some((1.0, uses.bounds.map_or(uses.roll, |b| b.1)));
        }
    }
    for v in &mut values {
        let Some((mut lo, mut hi)) = v.bounds else { continue };
        // A "reduced" line annotates its range high-to-low.
        if lo > hi {
            std::mem::swap(&mut lo, &mut hi);
        }
        // A roll outside its range is a legacy roll; the range grows to it.
        hi = hi.max(v.roll);
        lo = lo.min(v.roll);
        v.bounds = Some((lo, hi));
    }
    if values.is_empty() {
        if let Some(fixed) = matcher.value {
            values.push(Captured { roll: fixed, roll_str: String::new(), decimal: false, bounds: Some((fixed, fixed)) });
        }
    }
    if values.is_empty() {
        return None;
    }
    let rolls: Vec<f64> = values.iter().map(|v| v.roll).collect();
    let mins: Vec<f64> = values.iter().map(|v| v.bounds.map_or(v.roll, |b| b.0)).collect();
    let maxs: Vec<f64> = values.iter().map(|v| v.bounds.map_or(v.roll, |b| b.1)).collect();
    Some(Roll {
        unscalable,
        dp: stat.dp || values.iter().any(|v| v.decimal),
        value: roll_or_minmax_avg(&rolls)?,
        min: roll_or_minmax_avg(&mins)?,
        max: roll_or_minmax_avg(&maxs)?,
        option: matcher.value,
    })
}

fn incr_roll(value: f64, p: f64, dp: i32) -> f64 {
    let res = value + (value * p) / 100.0;
    let rounding = 10f64.powi(dp);
    ((res + f64::EPSILON) * rounding).trunc() / rounding
}

fn apply_incr(info: &ModInfo, roll: Roll) -> Roll {
    match info.roll_incr.filter(|p| *p != 0.0 && !roll.unscalable) {
        Some(p) => {
            let dp = if roll.dp { 2 } else { 0 };
            Roll {
                value: incr_roll(roll.value, p, dp),
                min: incr_roll(roll.min, p, dp),
                max: incr_roll(roll.max, p, dp),
                option: None,
                ..roll
            }
        }
        None => roll,
    }
}

fn types_can_be_grouped(a: ModType, b: ModType) -> bool {
    a == b || (a.is_explicit_family() && b.is_explicit_family())
}

/// One entry per stat and mod family, with every modifier that grants it.
pub fn sum_stats_by_mod_type(mods: &[ParsedModifier]) -> Vec<StatCalculated> {
    let mut out: Vec<StatCalculated> = Vec::new();
    // Two "Allocates #" lines are two passives, never one summed stat.
    let per_line = |r: &str| r == "Allocates #" || r == "Legacy of #";
    for mod_a in mods {
        for stat_a in &mod_a.stats {
            let ref_a = stat_a.stat.ref_.as_str();
            let merged = out.iter().any(|m| {
                m.stat.ref_ == ref_a
                    && types_can_be_grouped(m.type_, mod_a.info.type_)
                    && (!per_line(ref_a)
                        || m.sources.iter().any(|s| s.stat.translation.string == stat_a.translation.string))
            });
            if merged {
                continue;
            }
            let mut has_fractured = false;
            let mut sources = Vec::new();
            for mod_b in mods.iter().filter(|m| types_can_be_grouped(m.info.type_, mod_a.info.type_)) {
                let target = mod_b.stats.iter().find(|s| {
                    s.stat.ref_ == ref_a && (!per_line(ref_a) || s.translation.string == stat_a.translation.string)
                });
                let Some(target) = target else { continue };
                has_fractured |= mod_b.info.type_ == ModType::Fractured;
                let contributes = target.roll.map(|r| {
                    let r = apply_incr(&mod_b.info, r);
                    StatRoll { value: r.value, min: r.min, max: r.max, option: r.option }
                });
                sources.push(StatSource { modifier: mod_b.info.clone(), stat: target.clone(), contributes });
            }
            out.push(StatCalculated {
                stat: stat_a.stat.clone(),
                type_: if has_fractured { ModType::Fractured } else { mod_a.info.type_ },
                sources,
            });
        }
    }
    out
}

fn sum_stats(item: &mut ParsedItem, _: &Ee2Data) -> Result<(), ParseError> {
    item.stats_by_type = sum_stats_by_mod_type(&item.new_mods);
    Ok(())
}

fn parse_fractured(item: &mut ParsedItem, _: &Ee2Data) -> Result<(), ParseError> {
    if item.new_mods.iter().any(|m| m.info.type_ == ModType::Fractured) {
        item.is_fractured = true;
    }
    Ok(())
}

fn apply_elemental_added(item: &mut ParsedItem, _: &Ee2Data) -> Result<(), ParseError> {
    if item.weapon_elemental.is_none_or(|e| e == 0.0) || item.rarity == Some(Rarity::Unique) {
        return Ok(());
    }
    for calc in &item.stats_by_type {
        let slot = match calc.stat.ref_.as_str() {
            "Adds # to # Lightning Damage" => &mut item.weapon_lightning,
            "Adds # to # Cold Damage" => &mut item.weapon_cold,
            "Adds # to # Fire Damage" => &mut item.weapon_fire,
            _ => continue,
        };
        for source in &calc.sources {
            let v = source.contributes.map_or(f64::NAN, |c| c.value);
            *slot = Some(v + slot.filter(|s| *s != 0.0).unwrap_or(0.0));
        }
    }
    Ok(())
}

fn pick_correct_variant(item: &mut ParsedItem, _: &Ee2Data) -> Result<(), ParseError> {
    if item.info.disc.is_none() {
        return Ok(());
    }
    let has = |v: Option<f64>| v.is_some_and(|x| x != 0.0);
    for variant in item.info_variants.clone() {
        let Some(cond) = &variant.disc else { continue };
        if (cond.prop_ar && !has(item.armour_ar))
            || (cond.prop_ev && !has(item.armour_ev))
            || (cond.prop_es && !has(item.armour_es))
        {
            continue;
        }
        let tier = item.map_tier.unwrap_or(f64::NAN);
        let tier_ok = match cond.map_tier.as_deref() {
            Some("W") => tier <= 5.0,
            Some("Y") => (6.0..=10.0).contains(&tier),
            Some("R") => tier >= 11.0,
            _ => true,
        };
        if !tier_ok {
            continue;
        }
        let has_stat = |type_: ModType, r: &str| {
            item.stats_by_type.iter().any(|c| c.type_ == type_ && c.stat.ref_ == r)
        };
        if cond.has_implicit.as_ref().is_some_and(|r| !has_stat(ModType::Implicit, &r.ref_))
            || cond.has_explicit.as_ref().is_some_and(|r| !has_stat(ModType::Explicit, &r.ref_))
            || cond.section_text.as_ref().is_some_and(|t| !item.raw_text.contains(t))
        {
            continue;
        }
        item.info = variant;
    }
    Ok(())
}
