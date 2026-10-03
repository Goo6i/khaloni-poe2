//! An item state read from a copied item.
//!
//! The copy must carry the modifier headers (Ctrl+Alt+C): the header is the
//! only place the item says which side a modifier sits on. Each modifier
//! joins its mod database entry by the affix name in its header and its
//! text, with rolled values put back as the ranges the database words. The
//! tier is computed from the data on the item's base, trade-site style, and
//! never taken from the tooltip, whose direction in the current client is
//! unverified. A modifier that joins no entry keeps its text and no entry.

use super::data::{canonical_ranges, range_bounds, CraftData, Entry};
use super::types::{AffixKind, ItemState, ModOn, Rarity, Source};
use crate::ee2::parse::{Generation, ModType};
use crate::ee2::request::Built;

const MARKERS: [&str; 3] = [" (crafted)", " (desecrated)", " (fractured)"];
const UNSCALABLE: &str = " \u{2014} Unscalable Value";

impl ItemState {
    /// The planner's view of a parsed item. Fails, with the reason, when the
    /// copy lacks what the planner needs: a rarity, an item level, a base in
    /// the base item data, identified modifiers, and the modifier headers.
    pub fn from_built(built: &Built, data: &CraftData) -> Result<ItemState, String> {
        let item = &built.item;
        let rarity = item.rarity.ok_or("the copied text has no rarity line")?;
        if item.is_unidentified {
            return Err("the item is unidentified, so its modifiers cannot be read".to_string());
        }
        let names = [item.base_type.as_deref(), Some(item.info.ref_name.as_str()), Some(item.info.name.as_str())];
        let base = names.iter().flatten().find_map(|n| data.base(n)).ok_or_else(|| {
            let shown = names.iter().flatten().find(|n| !n.is_empty()).copied().unwrap_or(item.name.as_str());
            format!("{shown} is not a base in the base item data")
        })?;
        let item_level = item
            .item_level
            .filter(|l| l.is_finite() && *l >= 0.0)
            .map(|l| l as u32)
            .ok_or("the copied text shows no item level")?;

        let headers: Vec<_> = item
            .new_mods
            .iter()
            .filter(|m| matches!(m.info.generation, Some(Generation::Prefix | Generation::Suffix)))
            .map(|m| &m.info)
            .collect();
        // A magic or rare item always has a modifier, so one without a single
        // prefix or suffix header was copied without them.
        let headless = item
            .new_mods
            .iter()
            .any(|m| m.info.type_.is_explicit_family() && m.info.generation.is_none())
            || (headers.is_empty() && matches!(rarity, Rarity::Magic | Rarity::Rare));
        if headless {
            return Err("the item was copied without its modifier headers; copy it with Ctrl+Alt+C".to_string());
        }
        let blocks = affix_blocks(&item.raw_text);
        if headers.len() != blocks.len() {
            return Err(format!(
                "the copied text holds {} prefix and suffix headers but {} were read",
                blocks.len(),
                headers.len()
            ));
        }

        let base_tags: Vec<&str> = base.tags.iter().map(String::as_str).collect();
        let mut mods = Vec::with_capacity(headers.len());
        for (info, lines) in headers.into_iter().zip(blocks) {
            let kind = match info.generation {
                Some(Generation::Suffix) => AffixKind::Suffix,
                _ => AffixKind::Prefix,
            };
            let text = lines.join("\n");
            let mut source = match info.type_ {
                ModType::Crafted => Source::Crafted,
                ModType::Desecrated | ModType::Veiled => Source::Desecrated,
                ModType::Fractured => Source::Fractured,
                _ => Source::Random,
            };
            // An unrevealed desecrated modifier has no text to join yet.
            let entry = if info.type_ == ModType::Veiled {
                None
            } else {
                let ranged = lines.iter().map(|l| ranged_line(l)).collect::<Vec<_>>().join("\n");
                join(data, &base_tags, kind, info.name.as_deref(), &ranged)
            };
            // Older essence modifiers carry no crafted marker, but their entry
            // is one no tag lets roll, so only crafting can have put it there.
            if source == Source::Random && entry.is_some_and(|e| !e.rollable_anywhere()) {
                source = Source::Crafted;
            }
            mods.push(match entry {
                Some(e) => ModOn {
                    entry_id: Some(e.id.clone()),
                    family: e.family.clone(),
                    groups: e.groups.clone(),
                    kind,
                    tier: Some(data.tier_on(e, &base_tags)),
                    required_level: Some(e.required_level),
                    adds_tags: e.adds_tags.clone(),
                    source,
                    text,
                },
                None => ModOn {
                    entry_id: None,
                    family: text.clone(),
                    groups: Vec::new(),
                    kind,
                    tier: None,
                    required_level: None,
                    adds_tags: Vec::new(),
                    source,
                    text,
                },
            });
        }

        Ok(ItemState {
            class: base.class.clone(),
            base: base.name.clone(),
            base_tags: base.tags.clone(),
            item_level,
            rarity,
            corrupted: item.is_corrupted,
            mirrored: item.is_mirrored,
            sanctified: item.is_sanctified,
            mods,
            sockets: item.augment_sockets.as_ref().map_or(0, |s| s.current),
        })
    }
}

/// The entry a modifier is: entries with the header's affix name, of the
/// right side, whose text is the modifier's. When several share both, the
/// one that can roll on this base is taken; when that still leaves more
/// than one, or none match, the modifier joins nothing.
fn join<'a>(data: &'a CraftData, tags: &[&str], kind: AffixKind, name: Option<&str>, text: &str) -> Option<&'a Entry> {
    let key = canonical_ranges(text);
    let found: Vec<&Entry> = match name {
        Some(n) => data.entries_named(n).filter(|e| e.kind == kind && canonical_ranges(&e.text) == key).collect(),
        None => data.entries_with_text(text).filter(|e| e.kind == kind).collect(),
    };
    match found.as_slice() {
        [] => None,
        [only] => Some(only),
        several => {
            let here: Vec<&Entry> = several.iter().copied().filter(|e| e.eligible(tags.iter().copied())).collect();
            match here.as_slice() {
                [only] => Some(only),
                _ => None,
            }
        }
    }
}

/// The lines under each prefix and suffix header of a copied item, in
/// order, type markers removed. Reminder text (a bracketed run of lines) is
/// not part of a modifier.
fn affix_blocks(raw: &str) -> Vec<Vec<String>> {
    let text = raw.replace("\r\n", "\n");
    let mut out = Vec::new();
    let mut current: Option<Vec<String>> = None;
    let mut reminder = false;
    for line in text.lines().map(str::trim_end) {
        if line.starts_with('{') && line.ends_with('}') {
            out.extend(current.take());
            if line.contains("Prefix Modifier") || line.contains("Suffix Modifier") {
                current = Some(Vec::new());
            }
            reminder = false;
            continue;
        }
        if line == "--------" {
            out.extend(current.take());
            reminder = false;
            continue;
        }
        let Some(block) = current.as_mut() else { continue };
        if line.is_empty() {
            continue;
        }
        if line.trim_start().starts_with(['(', '\u{FF08}']) {
            reminder = true;
        }
        if reminder {
            if line.ends_with([')', '\u{FF09}']) {
                reminder = false;
            }
            continue;
        }
        let mut plain = line.strip_suffix(UNSCALABLE).unwrap_or(line);
        plain = MARKERS.iter().find_map(|m| plain.strip_suffix(m)).unwrap_or(plain);
        block.push(plain.to_string());
    }
    out.extend(current);
    out
}

/// A modifier line with each rolled value replaced by the range after it,
/// the way the mod database words it: `+78(70-84) to maximum Life` reads
/// `+(70-84) to maximum Life`.
fn ranged_line(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(open) = rest.find('(') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let is_range = after.find(')').is_some_and(|close| range_bounds(&after[..close]).is_some());
        if is_range {
            let kept = out.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.').len();
            if kept < out.len() {
                out.truncate(kept);
                // A negative roll's sign goes with it, unless it is part of a word.
                if out.ends_with('-') && !out[..out.len() - 1].ends_with(|c: char| c.is_alphanumeric()) {
                    out.pop();
                }
            }
        }
        out.push('(');
        rest = after;
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::{affix_blocks, ranged_line};

    #[test]
    fn a_rolled_value_gives_way_to_its_range() {
        assert_eq!(ranged_line("+78(70-84) to maximum Life"), "+(70-84) to maximum Life");
        assert_eq!(ranged_line("Adds 9(7-11) to 17(13-20) Physical Damage"), "Adds (7-11) to (13-20) Physical Damage");
        assert_eq!(ranged_line("+3.12(3.11-3.8)% to Critical Hit Chance"), "+(3.11-3.8)% to Critical Hit Chance");
        assert_eq!(ranged_line("+3 to Level of all Attack Skills"), "+3 to Level of all Attack Skills");
        assert_eq!(ranged_line("20% reduced Attribute Requirements"), "20% reduced Attribute Requirements");
    }

    #[test]
    fn blocks_follow_the_affix_headers_only() {
        let raw = "Item Class: Rings\n--------\n{ Implicit Modifier }\n+9(7-10)% to all Elemental Resistances\n--------\n\
                   { Prefix Modifier \"Robust\" (Tier: 3) — Life }\n+78(70-84) to maximum Life\n\n\
                   { Crafted Suffix Modifier \"of Calamity\" }\n+3.12(3.11-3.8)% to Critical Hit Chance (crafted)\n\
                   (a reminder\nthat spans lines)\n--------\nCorrupted";
        assert_eq!(
            affix_blocks(raw),
            vec![vec!["+78(70-84) to maximum Life".to_string()], vec!["+3.12(3.11-3.8)% to Critical Hit Chance".to_string()]]
        );
    }
}
