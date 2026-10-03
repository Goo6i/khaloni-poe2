//! One trade fetch entry as the price-check table shows it: the row (price,
//! age, seller and state, stack, item level, quality, gem level) and the
//! hover card (name, base, figures, every mod line with its tier badges).
//! Rows fold and flag price-fixing by EE2's rules (trade-api.ts grouping,
//! TradeListing.vue's isLikelyPriceFixed), so the table reads the same as
//! the tool the owner is used to.

use crate::runs::days_from_civil;
use serde_json::Value;

/// Whether the seller is at the keyboard, as the API reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SellerState {
    Online,
    Afk,
    Offline,
}

impl SellerState {
    pub fn as_str(self) -> &'static str {
        match self {
            SellerState::Online => "online",
            SellerState::Afk => "afk",
            SellerState::Offline => "offline",
        }
    }
}

/// Where a card line comes from: the `domain` the API stamps on each mod,
/// which is what colours it on the card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Implicit,
    Explicit,
    Rune,
    Desecrated,
    Crafted,
    Fractured,
    Enchant,
    Mutated,
    Pseudo,
}

impl LineKind {
    pub fn as_str(self) -> &'static str {
        match self {
            LineKind::Implicit => "implicit",
            LineKind::Explicit => "explicit",
            LineKind::Rune => "rune",
            LineKind::Desecrated => "desecrated",
            LineKind::Crafted => "crafted",
            LineKind::Fractured => "fractured",
            LineKind::Enchant => "enchant",
            LineKind::Mutated => "mutated",
            LineKind::Pseudo => "pseudo",
        }
    }

    fn from_domain(domain: &str) -> Option<Self> {
        Some(match domain {
            "implicit" => LineKind::Implicit,
            "explicit" => LineKind::Explicit,
            "rune" => LineKind::Rune,
            "desecrated" => LineKind::Desecrated,
            "crafted" => LineKind::Crafted,
            "fractured" => LineKind::Fractured,
            "enchant" => LineKind::Enchant,
            "mutated" => LineKind::Mutated,
            "pseudo" => LineKind::Pseudo,
            _ => return None,
        })
    }
}

/// One mod line of the card: display text, the tier badges of the mods
/// behind it ("P1", "S3"; two when the line merges two mods), and its kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardLine {
    pub text: String,
    pub tiers: Vec<String>,
    pub kind: LineKind,
}

/// The hover card of one listing.
#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub name: String,
    pub base: String,
    pub rarity: String,
    /// The item's own computed figures from `extended`: (label, value).
    pub figures: Vec<(String, String)>,
    pub lines: Vec<CardLine>,
}

/// One row of the listings table.
#[derive(Debug, Clone, PartialEq)]
pub struct ListingView {
    /// Amount and the currency id as the API names it ("divine", "exalted").
    pub price: Option<(f64, String)>,
    pub indexed_unix: i64,
    pub age_text: String,
    pub seller: String,
    pub state: SellerState,
    pub is_mine: bool,
    pub stack: Option<u32>,
    pub ilvl: Option<u32>,
    pub quality: Option<u32>,
    pub gem_level: Option<u32>,
    pub corrupted: bool,
    pub has_note: bool,
    pub instant_buyout: bool,
    pub card: Card,
}

/// A table row after folding: the first listing of the group and how many
/// listings it stands for.
#[derive(Debug, Clone, PartialEq)]
pub struct GroupedListing {
    pub view: ListingView,
    pub times: u32,
}

/// Why the table looks price-fixed: how many rows are in a currency the
/// site treats as noise, which currency the cheapest of them uses, and the
/// first price in a currency that counts.
#[derive(Debug, Clone, PartialEq)]
pub struct PriceFixed {
    pub count: usize,
    pub under_currency: Option<String>,
    pub next_price: Option<(f64, String)>,
}

/// Seconds since the listing was indexed, in the units the row has room
/// for. A listing indexed after `now` (clock skew) reads as just now.
pub fn age_text(indexed_unix: i64, now_unix: i64) -> String {
    let secs = now_unix - indexed_unix;
    if secs < 60 {
        "just now".to_string()
    } else if secs < 3_600 {
        format!("{} min", secs / 60)
    } else if secs < 86_400 {
        format!("{} h", secs / 3_600)
    } else {
        format!("{} d", secs / 86_400)
    }
}

/// `YYYY-MM-DDTHH:MM:SSZ` as unix seconds; None for anything else.
fn iso_to_unix(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 20 {
        return None;
    }
    let num = |from: usize, to: usize| -> Option<i64> {
        b[from..to].iter().try_fold(0i64, |acc, c| c.is_ascii_digit().then(|| acc * 10 + i64::from(c - b'0')))
    };
    let sep = |i: usize, c: u8| b[i] == c;
    if !(sep(4, b'-') && sep(7, b'-') && sep(10, b'T') && sep(13, b':') && sep(16, b':')) {
        return None;
    }
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi, sec) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    Some(days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + sec)
}

/// The game's `[Tag|Display]` brackets down to the display half, a bare
/// `[Word]` to the word; an empty display half keeps the tag, as EE2's
/// parseAffixStrings does.
pub fn display_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(open) = rest.find('[') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find(']') else {
            out.push_str(&rest[open..]);
            return out;
        };
        let inner = &after[..close];
        let shown = match inner.split_once('|') {
            Some((tag, "")) => tag,
            Some((_, display)) => display,
            None => inner,
        };
        out.push_str(shown);
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

/// The first run of digits in a property value ("+20%" -> 20, "12 (Max)" -> 12).
fn leading_number(s: &str) -> Option<u32> {
    let start = s.find(|c: char| c.is_ascii_digit())?;
    let digits: String = s[start..].chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// The first value of the property with this `type` id.
fn property(item: &Value, type_id: u64) -> Option<&str> {
    item["properties"]
        .as_array()?
        .iter()
        .find(|p| p["type"].as_u64() == Some(type_id))?["values"][0][0]
        .as_str()
}

fn lines(block: &Value, fallback: LineKind, out: &mut Vec<CardLine>) {
    let Some(entries) = block.as_array() else { return };
    for entry in entries {
        match entry {
            Value::String(s) => out.push(CardLine { text: display_text(s), tiers: Vec::new(), kind: fallback }),
            Value::Object(_) => {
                let tiers = entry["mods"]
                    .as_array()
                    .map(|mods| {
                        mods.iter().filter_map(|m| m["tier"].as_str()).filter(|t| !t.is_empty()).map(str::to_string).collect()
                    })
                    .unwrap_or_default();
                let kind = entry["domain"].as_str().and_then(LineKind::from_domain).unwrap_or(fallback);
                out.push(CardLine { text: display_text(entry["description"].as_str().unwrap_or("")), tiers, kind });
            }
            _ => {}
        }
    }
}

fn figures(extended: &Value) -> Vec<(String, String)> {
    const KEYS: [(&str, &str); 6] =
        [("dps", "DPS"), ("pdps", "pDPS"), ("edps", "eDPS"), ("ar", "Armour"), ("ev", "Evasion"), ("es", "Energy Shield")];
    KEYS.iter()
        .filter_map(|(key, label)| {
            let n = extended[key].as_f64()?;
            let text = if n.fract() == 0.0 { format!("{}", n as i64) } else { format!("{n:.1}") };
            Some((label.to_string(), text))
        })
        .collect()
}

/// One fetch entry as a row. None for the `null` the API sends in place of
/// a listing that vanished between search and fetch (and for anything
/// without a listing and an item); a listing without a price is a row
/// with `price: None`.
pub fn parse_entry(v: &Value, now_unix: i64, my_account: &str) -> Option<ListingView> {
    let listing = v.get("listing")?.as_object()?;
    let item = v.get("item")?;
    let price = listing.get("price").and_then(|p| {
        Some((p.get("amount")?.as_f64()?, p.get("currency")?.as_str()?.to_string()))
    });
    let indexed_unix = listing.get("indexed").and_then(Value::as_str).and_then(iso_to_unix).unwrap_or(now_unix);
    let account = listing.get("account").unwrap_or(&Value::Null);
    let seller = account["name"].as_str().unwrap_or("").to_string();
    let state = match account["online"].as_object() {
        None => SellerState::Offline,
        Some(online) if online.get("status").and_then(Value::as_str) == Some("afk") => SellerState::Afk,
        Some(_) => SellerState::Online,
    };

    let mut card_lines = Vec::new();
    lines(&item["runeMods"], LineKind::Rune, &mut card_lines);
    lines(&item["implicitMods"], LineKind::Implicit, &mut card_lines);
    lines(&item["explicitMods"], LineKind::Explicit, &mut card_lines);
    let card = Card {
        name: item["name"].as_str().unwrap_or("").to_string(),
        base: [&item["typeLine"], &item["baseType"]]
            .into_iter()
            .filter_map(Value::as_str)
            .find(|s| !s.is_empty())
            .unwrap_or("")
            .to_string(),
        rarity: item["rarity"].as_str().unwrap_or("").to_string(),
        figures: figures(&item["extended"]),
        lines: card_lines,
    };

    Some(ListingView {
        price,
        indexed_unix,
        age_text: age_text(indexed_unix, now_unix),
        is_mine: seller == my_account,
        seller,
        state,
        stack: item["stackSize"].as_u64().map(|n| n as u32),
        ilvl: property(item, 78).and_then(leading_number).or_else(|| item["ilvl"].as_u64().map(|n| n as u32)),
        quality: property(item, 6).and_then(leading_number),
        gem_level: property(item, 5).and_then(leading_number),
        corrupted: item["corrupted"].as_bool().unwrap_or(false),
        has_note: !item["note"].is_null(),
        instant_buyout: listing.get("fee").is_some_and(|fee| !fee.is_null()),
        card,
    })
}

/// Every entry of a fetch body's `result` as a row, in order, with how many
/// entries were nothing to show: nulls, plus listings without a price
/// (kept as rows, counted so the panel can say why the table is short).
pub fn parse_fetch_body(body: &Value, now_unix: i64, my_account: &str) -> (Vec<ListingView>, usize) {
    let mut rows = Vec::new();
    let mut dropped = 0;
    for entry in body["result"].as_array().into_iter().flatten() {
        match parse_entry(entry, now_unix, my_account) {
            Some(view) => {
                if view.price.is_none() {
                    dropped += 1;
                }
                rows.push(view);
            }
            None => dropped += 1,
        }
    }
    (rows, dropped)
}

/// EE2's folding rule over row indices: walking the rows in order, a row
/// joins an earlier group when it has that group's seller and price, or its
/// seller and the group is one of the last two. Returns (first row, count)
/// per group, in table order.
pub fn group_indices(rows: &[ListingView]) -> Vec<(usize, u32)> {
    let mut out: Vec<(usize, u32)> = Vec::new();
    for (idx, row) in rows.iter().enumerate() {
        let len = out.len();
        let found = out.iter_mut().enumerate().find(|(gi, (first, _))| {
            let head = &rows[*first];
            head.seller == row.seller && (head.price == row.price || len - gi <= 2)
        });
        match found {
            Some((_, group)) => group.1 += 1,
            None => out.push((idx, 1)),
        }
    }
    out
}

/// The rows folded by `group_indices`. A stack that folds adds to the
/// group's stack instead of its count, so a seller's supply shows as one
/// row with the whole quantity, as EE2 shows it.
pub fn group(rows: Vec<ListingView>) -> Vec<GroupedListing> {
    let mut out: Vec<GroupedListing> = Vec::new();
    for row in rows {
        let len = out.len();
        let found = out.iter_mut().enumerate().find(|(gi, g)| {
            g.view.seller == row.seller && (g.view.price == row.price || len - gi <= 2)
        });
        match found {
            Some((_, g)) => match g.view.stack {
                Some(have) if have > 0 => g.view.stack = Some(have + row.stack.unwrap_or(0)),
                _ => g.times += 1,
            },
            None => out.push(GroupedListing { view: row, times: 1 }),
        }
    }
    out
}

/// A price in a currency the site counts as real: chaos, exalted, divine in
/// any form, or fewer than 30 of the low orbs (aug, regal, transmute, but
/// not their greater and perfect versions).
fn common_currency(price: &Option<(f64, String)>) -> bool {
    let Some((amount, currency)) = price else { return false };
    let lower = currency.to_ascii_lowercase();
    ["chaos", "exalted", "divine"].iter().any(|c| lower.contains(c))
        || (matches!(currency.as_str(), "aug" | "regal" | "transmute") && *amount < 30.0)
}

/// EE2's isLikelyPriceFixed: with more than 15 rows, fewer than five of
/// them priced in a currency that counts means the cheap end of the table
/// is bait. Says how many rows are bait, in what, and the first real price.
pub fn price_fixed(rows: &[GroupedListing]) -> Option<PriceFixed> {
    if rows.len() <= 15 {
        return None;
    }
    let common: Vec<&GroupedListing> = rows.iter().filter(|g| common_currency(&g.view.price)).collect();
    if common.len() >= 5 {
        return None;
    }
    Some(PriceFixed {
        count: rows.len() - common.len(),
        under_currency: rows
            .iter()
            .find(|g| !common_currency(&g.view.price))
            .and_then(|g| g.view.price.as_ref().map(|(_, c)| c.clone())),
        next_price: common.first().and_then(|g| g.view.price.clone()),
    })
}
