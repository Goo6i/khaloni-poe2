use thiserror::Error;

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("empty clipboard text")]
    Empty,
    #[error("not an item: missing '{0}' header")]
    MissingHeader(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rarity {
    Normal,
    Magic,
    Rare,
    Unique,
    Currency,
    Gem,
    Quest,
    Other(String),
}

impl Rarity {
    fn parse(s: &str) -> Rarity {
        match s {
            "Normal" => Rarity::Normal,
            "Magic" => Rarity::Magic,
            "Rare" => Rarity::Rare,
            "Unique" => Rarity::Unique,
            "Currency" => Rarity::Currency,
            "Gem" => Rarity::Gem,
            "Quest" => Rarity::Quest,
            other => Rarity::Other(other.to_string()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModKind {
    Prefix,
    Suffix,
    Implicit,
    Rune,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModHeader {
    pub kind: ModKind,
    pub name: Option<String>,
    pub tier: Option<u8>,
    pub tags: Vec<String>,
    pub crafted: bool,
    pub desecrated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemMod {
    pub text: String,
    pub header: Option<ModHeader>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub item_class: String,
    pub rarity: Rarity,
    pub name: String,
    pub base_type: Option<String>,
    pub stack_size: Option<(u32, u32)>,
    pub item_level: Option<u32>,
    pub implicits: Vec<ItemMod>,
    pub explicits: Vec<ItemMod>,
    pub sections: Vec<Vec<String>>,
}

pub fn parse_item(text: &str) -> Result<Item, ParseError> {
    let sections = split_sections(text);
    if sections.is_empty() {
        return Err(ParseError::Empty);
    }
    let head = &sections[0];
    let item_class =
        field(head, "Item Class: ").ok_or(ParseError::MissingHeader("Item Class"))?;
    let rarity_s = field(head, "Rarity: ").ok_or(ParseError::MissingHeader("Rarity"))?;
    let rarity = Rarity::parse(&rarity_s);

    let names: Vec<&String> = head
        .iter()
        .filter(|l| !l.starts_with("Item Class: ") && !l.starts_with("Rarity: "))
        .collect();
    let name = names
        .first()
        .map(|s| s.to_string())
        .ok_or(ParseError::MissingHeader("name"))?;
    let base_type = names.get(1).map(|s| s.to_string());

    let mut stack_size = None;
    let mut item_level = None;
    for sec in &sections {
        for line in sec {
            if let Some(v) = line.strip_prefix("Stack Size: ") {
                let mut parts = v.split('/');
                if let (Some(a), Some(b)) = (parts.next(), parts.next()) {
                    let a = a.trim().replace(',', "").parse();
                    let b = b.trim().replace(',', "").parse();
                    if let (Ok(a), Ok(b)) = (a, b) {
                        stack_size = Some((a, b));
                    }
                }
            } else if let Some(v) = line.strip_prefix("Item Level: ") {
                item_level = v.trim().parse().ok();
            }
        }
    }

    let mut implicits = Vec::new();
    let mut explicits = Vec::new();
    for sec in sections.iter().skip(1) {
        classify_section(sec, &mut implicits, &mut explicits);
    }

    Ok(Item {
        item_class,
        rarity,
        name,
        base_type,
        stack_size,
        item_level,
        implicits,
        explicits,
        sections,
    })
}

fn split_sections(text: &str) -> Vec<Vec<String>> {
    let mut sections = Vec::new();
    let mut cur = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim_end();
        if !trimmed.is_empty() && trimmed.chars().all(|c| c == '-') {
            if !cur.is_empty() {
                sections.push(std::mem::take(&mut cur));
            }
        } else if !trimmed.is_empty() {
            cur.push(trimmed.to_string());
        }
    }
    if !cur.is_empty() {
        sections.push(cur);
    }
    sections
}

fn field(section: &[String], prefix: &str) -> Option<String> {
    section
        .iter()
        .find_map(|l| l.strip_prefix(prefix).map(|v| v.trim().to_string()))
}

fn classify_section(sec: &[String], implicits: &mut Vec<ItemMod>, explicits: &mut Vec<ItemMod>) {
    if sec.iter().any(|l| l.starts_with("{ ")) {
        // advanced format: header lines followed by mod text lines
        let mut header: Option<ModHeader> = None;
        for line in sec {
            if line.starts_with("{ ") {
                header = parse_header(line);
            } else if let Some(h) = header.clone() {
                let m = ItemMod {
                    text: line.clone(),
                    header: Some(h.clone()),
                };
                if h.kind == ModKind::Implicit {
                    implicits.push(m);
                } else {
                    explicits.push(m);
                }
            }
        }
        return;
    }
    // simple format: bare mod lines, possibly marked with a trailing tag
    for line in sec {
        if let Some(stripped) = line.strip_suffix(" (implicit)") {
            implicits.push(ItemMod {
                text: stripped.to_string(),
                header: None,
            });
        } else if let Some(stripped) = line.strip_suffix(" (rune)") {
            explicits.push(ItemMod {
                text: stripped.to_string(),
                header: Some(ModHeader {
                    kind: ModKind::Rune,
                    name: None,
                    tier: None,
                    tags: Vec::new(),
                    crafted: false,
                    desecrated: false,
                }),
            });
        } else if is_bare_mod_line(line) {
            explicits.push(ItemMod {
                text: line.clone(),
                header: None,
            });
        }
    }
}

/// Heuristic for simple-format explicit mod lines: no "Key: value" property
/// shape, and either starts with a signed/numeric value or contains a mod verb.
fn is_bare_mod_line(line: &str) -> bool {
    if line.contains(": ") {
        return false;
    }
    const HELP_PREFIXES: [&str; 4] =
        ["Place into", "Right click", "Shift click", "Can be used"];
    if HELP_PREFIXES.iter().any(|p| line.starts_with(p)) {
        return false;
    }
    let starts_numeric = line
        .chars()
        .next()
        .map(|c| c == '+' || c == '-' || c.is_ascii_digit())
        .unwrap_or(false);
    const MOD_VERBS: [&str; 6] = [
        "increased ", "reduced ", "Adds ", " per second", "additional ", " to maximum ",
    ];
    starts_numeric && (line.contains('%') || MOD_VERBS.iter().any(|v| line.contains(v)) || line.starts_with('+'))
}

/// Parses `{ Crafted Suffix Modifier "of Calamity" (Tier: 3) — Attack, Critical }`.
/// The separator before tags is an em-dash, part of the game's format.
fn parse_header(line: &str) -> Option<ModHeader> {
    let inner = line.strip_prefix("{ ")?.strip_suffix(" }")?;
    let (main, tags) = match inner.split_once(" \u{2014} ") {
        Some((m, t)) => (m, t.split(", ").map(|s| s.to_string()).collect()),
        None => (inner, Vec::new()),
    };
    let crafted = main.starts_with("Crafted ");
    let desecrated = main.starts_with("Desecrated ");
    let kind = if main.contains("Prefix Modifier") {
        ModKind::Prefix
    } else if main.contains("Suffix Modifier") {
        ModKind::Suffix
    } else if main.contains("Implicit Modifier") {
        ModKind::Implicit
    } else if main.contains("Rune") {
        ModKind::Rune
    } else {
        ModKind::Other
    };
    let name = main.split('"').nth(1).map(|s| s.to_string());
    let tier = main
        .split("(Tier: ")
        .nth(1)
        .and_then(|t| t.split(')').next())
        .and_then(|t| t.trim().parse().ok());
    Some(ModHeader {
        kind,
        name,
        tier,
        tags,
        crafted,
        desecrated,
    })
}
