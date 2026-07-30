use khaloni_poe2_core::item::{parse_item, ModKind, Rarity};

const BOW: &str = include_str!("fixtures/item1-inventory-rare-bow.txt");
const JEWEL: &str = include_str!("fixtures/item2-stash-rare-jewel.txt");
const AMULET: &str = include_str!("fixtures/item3-chatlink-rare-amulet.txt");
const EXALT: &str = include_str!("fixtures/item4-currency-exalted.txt");

#[test]
fn parses_rare_bow_advanced_format() {
    let it = parse_item(BOW).unwrap();
    assert_eq!(it.item_class, "Bows");
    assert_eq!(it.rarity, Rarity::Rare);
    assert_eq!(it.name, "Horror Bane");
    assert_eq!(it.base_type.as_deref(), Some("Obliterator Bow"));
    assert_eq!(it.item_level, Some(81));
    assert_eq!(it.stack_size, None);

    assert_eq!(it.implicits.len(), 1);
    assert_eq!(it.implicits[0].text, "50% reduced Projectile Range");
    assert_eq!(it.implicits[0].header.as_ref().unwrap().kind, ModKind::Implicit);

    // 6 explicit mod lines plus the rune line
    let rune: Vec<_> = it
        .explicits
        .iter()
        .filter(|m| m.header.as_ref().map(|h| h.kind) == Some(ModKind::Rune))
        .collect();
    assert_eq!(rune.len(), 1);
    assert_eq!(rune[0].text, "38% increased Physical Damage");

    let explicit: Vec<_> = it
        .explicits
        .iter()
        .filter(|m| m.header.as_ref().map(|h| h.kind) != Some(ModKind::Rune))
        .collect();
    assert_eq!(explicit.len(), 6);

    let tyr = explicit
        .iter()
        .find(|m| m.text == "157(155-169)% increased Physical Damage")
        .unwrap();
    let h = tyr.header.as_ref().unwrap();
    assert_eq!(h.kind, ModKind::Prefix);
    assert_eq!(h.name.as_deref(), Some("Tyrannical"));
    assert_eq!(h.tier, Some(2));
    assert_eq!(h.tags, vec!["Damage", "Physical", "Attack"]);
    assert!(!h.crafted);
    assert!(!h.desecrated);

    let gleam = explicit
        .iter()
        .find(|m| m.text == "Adds 13(10-15) to 26(18-26) Physical Damage")
        .unwrap();
    assert!(gleam.header.as_ref().unwrap().desecrated);

    let marks = explicit
        .iter()
        .find(|m| m.text == "+3 to Level of all Projectile Skills")
        .unwrap();
    let mh = marks.header.as_ref().unwrap();
    assert_eq!(mh.name.as_deref(), Some("of the Marksman"));
    assert_eq!(mh.tier, Some(2));
    assert!(mh.tags.is_empty());

    let craft = explicit
        .iter()
        .find(|m| m.text == "+3.48(3.11-3.8)% to Critical Hit Chance")
        .unwrap();
    assert!(craft.header.as_ref().unwrap().crafted);
}

#[test]
fn parses_stash_jewel_and_ignores_help_text() {
    let it = parse_item(JEWEL).unwrap();
    assert_eq!(it.item_class, "Jewels");
    assert_eq!(it.name, "Ghoul Essence");
    assert_eq!(it.base_type.as_deref(), Some("Emerald"));
    assert_eq!(it.implicits.len(), 0);
    assert_eq!(it.explicits.len(), 4);
    let track = it
        .explicits
        .iter()
        .find(|m| m.text == "Mark Skills have 20(18-32)% increased Skill Effect Duration")
        .unwrap();
    assert_eq!(track.header.as_ref().unwrap().kind, ModKind::Suffix);
    // the "Place into an allocated Jewel Socket..." line is not a mod
    assert!(it.explicits.iter().all(|m| !m.text.starts_with("Place into")));
}

#[test]
fn parses_chat_amulet_simple_format() {
    let it = parse_item(AMULET).unwrap();
    assert_eq!(it.item_class, "Amulets");
    assert_eq!(it.rarity, Rarity::Rare);
    assert_eq!(it.name, "Pain Locket");
    assert_eq!(it.base_type.as_deref(), Some("Stellar Amulet"));
    assert_eq!(it.item_level, Some(82));

    assert_eq!(it.implicits.len(), 1);
    assert_eq!(it.implicits[0].text, "+5 to all Attributes");
    assert!(it.implicits[0].header.is_none());

    let texts: Vec<&str> = it.explicits.iter().map(|m| m.text.as_str()).collect();
    assert_eq!(
        texts,
        vec![
            "24% increased Armour",
            "+59 to maximum Mana",
            "+28% to Fire Resistance",
            "24.7 Life Regeneration per second",
        ]
    );
    assert!(it.explicits.iter().all(|m| m.header.is_none()));
}

#[test]
fn parses_currency_with_stack_size() {
    let it = parse_item(EXALT).unwrap();
    assert_eq!(it.item_class, "Stackable Currency");
    assert_eq!(it.rarity, Rarity::Currency);
    assert_eq!(it.name, "Exalted Orb");
    assert_eq!(it.base_type, None);
    assert_eq!(it.stack_size, Some((2, 20)));
    assert!(it.explicits.is_empty());
    assert!(it.implicits.is_empty());
}

#[test]
fn rejects_garbage() {
    assert!(parse_item("").is_err());
    assert!(parse_item("hello world\nno item here").is_err());
}
