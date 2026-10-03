//! The Evaluate panel: a three-column item card modelled on the game's own
//! item tooltip, replacing the flat appraisal list.
//!
//! Layout, left to right:
//!
//! ```text
//!   TIERING │        item card          │ SCORING │  filters
//!     P9    │  23 to Accuracy Rating    │   0.8   │ [23][max]
//!     S1    │  24% to Critical Damage   │   4.0   │ [24][max]
//! ```
//!
//! The card reads like the tooltip a player already knows (name header,
//! rarity/level block, then mods); the gutters add what the game does not
//! show — which affix family and tier each mod is, and how good the roll
//! is within that tier ladder — and the filter column on the right is what
//! actually goes to the trade search.
//!
//! Under the card, what the search found: the poe.ninja line,
//! the price ladder, the listings table with a hover card beside the panel
//! for the row under the pointer, the closest listings, what each mod is
//! worth, the bulk offers for a stackable. Every figure there is read off
//! listings the user can see; nothing is a model's output.
//!
//! Same discipline as the rest of the overlay: this module owns the pure
//! model, the geometry, and the hit-testing, and the renderer draws from
//! THIS geometry, so pixels and click targets cannot drift apart.
//!
//! On scoring, deliberately: it is roll quality — where this roll sits in
//! its own tier ladder — NOT an estimate of how much the mod contributes
//! to price. A price-contribution number would need a model we do not have
//! and could not justify, and the closed-source overlays' invented numbers
//! are exactly what their users learned to distrust.

use crate::config::Rect;
use crate::pricing::Denom;
use khaloni_poe2_core::listing::GroupedListing;
use khaloni_poe2_core::ninja::PriceTable;
use khaloni_poe2_core::value::format_amount;

pub use khaloni_poe2_core::listing::{Card, CardLine, LineKind, SellerState};

const PAD: i32 = 12;
/// The name line is drawn in the large font; this is its whole band.
const TITLE_H: i32 = 30;
/// Rarity and the item-level lines: plain text, no hit targets.
const LINE_H: i32 = 18;
const ROW_H: i32 = 26;
const CHECK: i32 = 16;
/// Left gutter: wide enough for "P12" plus breathing room, fixed so every
/// badge lands on the same column and the card text starts flush.
const GUT_L: i32 = 34;
/// Right gutter: "5.0" plus room, likewise fixed.
const GUT_R: i32 = 40;
const GAP: i32 = 8;
const BOX_W: i32 = 48;
const BOX_H: i32 = 20;
const BOX_GAP: i32 = 6;
/// Air on each side of the hairline separating row groups.
const DIV_GAP: i32 = 4;
/// The one-off "TIERING"/"SCORING" heading line above the row band.
const HEAD_H: i32 = 20;
/// Separation between the card's blocks (header, rows, controls).
const BLOCK_GAP: i32 = 10;
const TOGGLE_H: i32 = 22;
const RADIO_H: i32 = 22;
const RADIO_GAP: i32 = 10;
const BTN_H: i32 = 28;
const BTN_GAP: i32 = 10;
const SEARCH_W: i32 = 110;
const OPEN_SITE_W: i32 = 130;
/// Where the status line starts: right of both buttons.
const STATUS_X: i32 = PAD + SEARCH_W + BTN_GAP + OPEN_SITE_W + BTN_GAP;
/// A listings-table row, a bulk-offer row, and any other line in the row
/// face: the mod rows' text size with tighter leading, since these rows
/// carry no checkbox or value box.
pub const LISTING_H: i32 = 22;
const CLOSE: i32 = 20;
/// A block's small-caps heading line ("poe.ninja", "Closest listings").
const SECTION_H: i32 = 22;
/// A column-caption line over a table.
const CAPTION_H: i32 = 18;
/// A small dim line: the ladder, the ninja detail, a "nearest differs" note.
const NOTE_H: i32 = 18;
/// The likely-price-fixed strip, with its button inside.
const STRIP_H: i32 = 28;
/// Air between the table's columns.
const COL_GAP: i32 = 16;
/// Between the panel's edge and the hover card beside it.
const CARD_GAP: i32 = 12;
const CARD_MIN_W: i32 = 240;
/// The card's title line, drawn in the large face.
const CARD_TITLE_H: i32 = 28;
const LABEL_MIN_W: i32 = 170;
const WIDTH_MIN: i32 = 480;
const WIDTH_MAX: i32 = 1600;

/// Which affix family a mod belongs to, for the tiering gutter's badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AffixKind {
    Prefix,
    Suffix,
    /// Implicit, corrupted, or otherwise not a rollable prefix/suffix.
    Other,
}

impl AffixKind {
    /// Badge prefix character: "P9", "S1", "—".
    pub fn letter(self) -> &'static str {
        match self {
            AffixKind::Prefix => "P",
            AffixKind::Suffix => "S",
            AffixKind::Other => "",
        }
    }
}

/// The left-gutter badge for a row, when the affix is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TierBadge {
    pub kind: AffixKind,
    /// 1 is the best tier, matching how players say "T1".
    pub tier: u8,
}

/// What a searchable row feeds when its checkbox is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// Index into the Query's stat filters (item mods, pseudo totals).
    Stat(usize),
    /// One of the trade site's computed item figures (DPS, armour, spirit,
    /// rune sockets). These are open-ended minimums ("this much DPS or
    /// more"), so a row carrying one gets a min box and no max box.
    Equipment(EquipKey),
    /// Index into `Panel::extras`: a modifier line EE2 gives no row of its
    /// own. Its filter joins the search only while the row is ticked (see
    /// [`search_query`]), so the search EE2 opened with is never touched.
    Extra(usize),
}

pub use khaloni_poe2_core::props::EquipKey;

impl Target {
    /// Whether the row's bound has an upper end the user can set. A
    /// computed figure is an open-ended minimum and has none.
    pub fn has_max(self) -> bool {
        matches!(self, Target::Stat(_) | Target::Extra(_))
    }
}

/// Which tooltip block a row belongs to. Rows sort Property, then
/// Implicit, then Explicit - the order the game's own tooltip uses - and
/// the layout draws a hairline where the block changes so the groups read
/// apart at a glance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum RowGroup {
    /// Computed figures and waystone properties (DPS, Monster Effectiveness).
    Property,
    Implicit,
    #[default]
    Explicit,
}

/// One line of the card. Covers both derived stats (DPS, total Attributes)
/// and real mods; `target` is what makes a row searchable.
#[derive(Debug, Clone, PartialEq)]
pub struct StatRow {
    /// Text as the game words it, e.g. "23 to Accuracy Rating".
    pub label: String,
    /// Left gutter badge; None for rows with no known affix (derived
    /// stats, unmatched mods).
    pub badge: Option<TierBadge>,
    /// Right gutter roll-quality score, 0.0..=5.0. None when the affix has
    /// no tier ladder to score against — never a fabricated value.
    pub score: Option<f32>,
    /// Search bounds. `min` is prefilled from the item's own roll; `None`
    /// is a filter with no lower bound, drawn as an empty box. A "0" there
    /// would claim a bound the search does not send.
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub enabled: bool,
    /// What the row drives in the search; None for display-only rows.
    pub target: Option<Target>,
    /// Rows the player rarely filters on, collapsed behind "Show N more"
    /// so the card stays as short as the tooltip it imitates.
    pub hidden: bool,
    pub group: RowGroup,
    /// Why a display-only mod line has no checkbox ("counted in Elemental
    /// DPS, Total DPS"). Drawn after the label, and the whole row recedes.
    pub note: Option<String>,
}

impl StatRow {
    /// The row's text as drawn and measured.
    pub fn text(&self) -> String {
        match &self.note {
            Some(note) => format!("{} \u{2014} {note}", self.label),
            None => self.label.clone(),
        }
    }
}

/// The block a row of this mod type sits in.
pub fn group_of(tag: &str) -> RowGroup {
    match tag {
        "implicit" | "skill" | "enchant" | "rune" => RowGroup::Implicit,
        "map" | "pseudo" => RowGroup::Property,
        _ => RowGroup::Explicit,
    }
}

/// The item's lines EE2 gives no row of their own: the ones a figure or a
/// total already counted, and the ones it cannot search. Each is a row
/// like any other, unticked, its filter pushed onto `extras` for its
/// `Target::Extra` to address; the note says which total already counts
/// it. A line no catalog lists keeps its row with the reason and no
/// checkbox. They are never collapsed: a hidden line is what these rows
/// exist to prevent.
pub fn extra_rows(
    extra: &[khaloni_poe2_core::ee2::request::ExtraRow],
    extras: &mut Vec<khaloni_poe2_core::trade::StatFilter>,
) -> Vec<StatRow> {
    extra
        .iter()
        .map(|x| {
            let filter = x.filter();
            let row = StatRow {
                label: x.text.clone(),
                badge: None,
                score: None,
                min: filter.as_ref().and_then(|f| f.value.min),
                max: filter.as_ref().and_then(|f| f.value.max),
                enabled: false,
                target: filter.as_ref().map(|_| Target::Extra(extras.len())),
                hidden: false,
                group: group_of(x.tag),
                note: x.note(),
            };
            extras.extend(filter);
            row
        })
        .collect()
}

/// The query a search sends: EE2's, with the filter of every ticked extra
/// row after its own. Search and "Open site" both go through here, so the
/// site opens exactly what was searched. Two unrevealed lines of one kind
/// carry the same count; ticked together they are one filter.
pub fn search_query(panel: &Panel, query: &khaloni_poe2_core::trade::Query) -> khaloni_poe2_core::trade::Query {
    let mut q = query.clone();
    for f in panel.extras.iter().filter(|f| !f.disabled) {
        let f = khaloni_poe2_core::trade::StatFilter { disabled: false, ..f.clone() };
        if !q.filters[query.filters.len()..].contains(&f) {
            q.filters.push(f);
        }
    }
    q
}

/// How hard the search should be relaxed before it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Strictness {
    /// Every kept mod must meet the item's own roll.
    #[default]
    Quick,
    /// All minimums relaxed by 10%, which finds the comparable items an
    /// exact-roll search misses.
    Broad,
}

impl Strictness {
    pub fn label(self) -> &'static str {
        match self {
            Strictness::Quick => "Quick Price",
            Strictness::Broad => "Broad (-10%)",
        }
    }
}

/// One row of the listings table, after folding: the price as the panel
/// shows it, the listing's own facts, and the card for when it is hovered.
#[derive(Debug, Clone, PartialEq)]
pub struct ListingRow {
    /// The price in the panel's currency ("5 div", "38 ex"); empty for a
    /// listing without a price.
    pub price: String,
    /// The listing's own price when it is in another currency ("1500
    /// exalted", "2 aug"), shown beside `price`; empty when it is the same.
    pub raw: String,
    pub age: String,
    pub seller: String,
    pub state: SellerState,
    /// The seller is the configured account.
    pub mine: bool,
    /// How many listings the row stands for (EE2's "×N" folding).
    pub times: u32,
    pub stack: Option<u32>,
    pub ilvl: Option<u32>,
    pub quality: Option<u32>,
    pub gem_level: Option<u32>,
    pub corrupted: bool,
    /// The listing carries a fee: it can be bought without a whisper.
    pub instant_buyout: bool,
    pub card: Card,
}

/// The table's column captions, in cell order.
pub const LISTING_CAPTIONS: [&str; 5] = ["PRICE", "AGE", "SELLER", "STATUS", "DETAILS"];

/// A folded fetch row as the table shows it, priced in the listing's own
/// currency: no table, no conversion. [`ListingRow::priced`] is the same
/// row with the price put into the panel's currency.
impl From<&GroupedListing> for ListingRow {
    fn from(g: &GroupedListing) -> ListingRow {
        let v = &g.view;
        ListingRow {
            price: v.price.as_ref().map(|(amount, currency)| format!("{} {currency}", format_amount(*amount))).unwrap_or_default(),
            raw: String::new(),
            age: v.age_text.clone(),
            seller: v.seller.clone(),
            state: v.state,
            mine: v.is_mine,
            times: g.times,
            stack: v.stack,
            ilvl: v.ilvl,
            quality: v.quality,
            gem_level: v.gem_level,
            corrupted: v.corrupted,
            instant_buyout: v.instant_buyout,
            card: v.card.clone(),
        }
    }
}

impl ListingRow {
    /// The row with its price in the panel's currency when `exalted` (the
    /// listing's price in exalted, through the currency table) is known;
    /// see [`listing_price_text`].
    pub fn priced(g: &GroupedListing, exalted: Option<f64>, table: &PriceTable, divine_threshold: f64) -> ListingRow {
        let mut row = ListingRow::from(g);
        if let Some((amount, currency)) = &g.view.price {
            let (price, raw) = listing_price_text(*amount, currency, exalted, table, divine_threshold);
            row.price = price;
            row.raw = raw;
        }
        row
    }

    /// The row's five cells: price with the listing's own currency and the
    /// folded count, age, seller, state, and the item facts that are
    /// present. "x3" rather than "×3": the overlay face has no
    /// multiplication sign, and a missing glyph draws as a gap.
    pub fn cells(&self) -> [String; 5] {
        let mut price = if self.price.is_empty() { "no price".to_string() } else { self.price.clone() };
        if !self.raw.is_empty() {
            price.push_str(&format!(" ({})", self.raw));
        }
        if self.times > 1 {
            price.push_str(&format!(" x{}", self.times));
        }
        let seller = if self.mine { format!("{} (you)", self.seller) } else { self.seller.clone() };
        let mut details: Vec<String> = Vec::new();
        if let Some(n) = self.stack {
            details.push(format!("stack {n}"));
        }
        if let Some(n) = self.ilvl {
            details.push(format!("ilvl {n}"));
        }
        if let Some(n) = self.quality {
            details.push(format!("q{n}"));
        }
        if let Some(n) = self.gem_level {
            details.push(format!("gem {n}"));
        }
        if self.corrupted {
            details.push("corrupted".into());
        }
        if self.instant_buyout {
            details.push("instant".into());
        }
        [price, self.age.clone(), seller, self.state.as_str().to_string(), details.join("  ")]
    }
}

/// A listing's price as the table shows it: in the panel's currency when
/// `exalted` (the price in exalted, through the currency table) is known,
/// with the listing's own price beside it when that is in another
/// currency. Without a rate the listing's own price stands alone, since
/// there is no honest conversion to show.
pub fn listing_price_text(
    amount: f64,
    currency: &str,
    exalted: Option<f64>,
    table: &PriceTable,
    divine_threshold: f64,
) -> (String, String) {
    let own = format!("{} {currency}", format_amount(amount));
    let Some(ex) = exalted else { return (own, String::new()) };
    let (denom, shown) = crate::pricing::denom_amount(&table.price_from_exalted(ex), 1, divine_threshold);
    let (unit, id) = match denom {
        Denom::Divine => ("div", "divine"),
        Denom::Chaos => ("chaos", "chaos"),
        Denom::Exalted => ("ex", "exalted"),
        Denom::None => return (own, String::new()),
    };
    let raw = if currency == id { String::new() } else { own };
    (format!("{shown} {unit}"), raw)
}

/// The poe.ninja line for an item the market view grades: the price in
/// the panel's currency, the week's band and direction, the traded
/// volume, and the market view's own caveat ("thin market", "not enough
/// history") when it has one. Worded by `core::market` so the two views
/// cannot disagree.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NinjaBlock {
    pub price: String,
    pub band: String,
    pub direction: String,
    pub volume: String,
    pub note: String,
}

/// The closest listings to the checked item, each line naming listings
/// and their prices, and how the nearest one differs when none is close.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClosestBlock {
    pub lines: Vec<String>,
    pub nearest: Option<String>,
}

/// What one mod is worth: the cheapest listing with the mod's filter kept
/// against the cheapest with it dropped, and the two listings named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttributionRow {
    pub label: String,
    pub with: String,
    pub without: String,
    pub text: String,
}

/// The likely-price-fixed strip: the numbers, and the button that re-runs
/// the search priced in exalted and divine only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriceFixedStrip {
    pub text: String,
    pub button: String,
}

/// One exchange offer as the bulk view shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BulkOffer {
    pub have: String,
    pub want: String,
    pub stock: String,
    pub seller: String,
    pub state: SellerState,
}

pub const BULK_CAPTIONS: [&str; 4] = ["HAVE", "WANT", "STOCK", "SELLER"];

/// The exchange offers for a stackable, cheapest first, and a note on
/// what they add up to.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BulkBlock {
    pub offers: Vec<BulkOffer>,
    pub note: String,
}

/// The item header block, worded exactly as the tooltip does.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ItemHeader {
    pub name: String,
    /// "Rare", "Magic", … drives the name's colour.
    pub rarity: String,
    pub item_level: Option<u32>,
    pub requires_level: Option<u32>,
    /// Category constraint toggle, when the search has a category.
    pub base: Option<BaseToggle>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BaseToggle {
    pub label: String,
    pub enabled: bool,
}

/// The panel: the item card and its controls, then what the search found
/// under it. The blocks are empty by default, so a panel that has not
/// searched yet shows the card alone.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Panel {
    pub header: ItemHeader,
    pub rows: Vec<StatRow>,
    /// The filters of the extra rows, addressed by `Target::Extra`; a
    /// ticked one has `disabled` false. Kept here and not in the query, so
    /// the query stays EE2's own.
    pub extras: Vec<khaloni_poe2_core::trade::StatFilter>,
    /// Whether hidden rows are currently expanded.
    pub show_hidden: bool,
    pub strictness: Strictness,
    pub status: String,
    pub search_id: Option<String>,
    /// A search for this panel is running: the Search button is drawn
    /// switched off and a press on it is not sent, so impatient clicks do
    /// not each queue a rate-limited search behind the first.
    pub searching: bool,
    /// The listings table, folded.
    pub listings: Vec<ListingRow>,
    /// The row under the pointer; its card is laid out beside the panel.
    pub hover: Option<usize>,
    pub ninja: Option<NinjaBlock>,
    /// "cheapest 2 ex, then 5, 6, 6, 8 · 20 of 1,934 matched"; empty
    /// before a search.
    pub ladder: String,
    pub closest: Option<ClosestBlock>,
    pub attribution: Vec<AttributionRow>,
    pub price_fixed: Option<PriceFixedStrip>,
    pub bulk: Option<BulkBlock>,
    /// "37 x 1.8 chaos = 67 chaos" under the header when the item is a stack.
    pub stack_value: Option<String>,
    /// "searches 4/30 (5 min)", from the server's own counters.
    pub budget_text: String,
    /// Within five of the cap: the budget text turns red.
    pub budget_low: bool,
    /// The attribution button can be pressed (the budget has room). It is
    /// still drawn, switched off, when not: the search is on request only
    /// and the user should see it is there.
    pub attribute_enabled: bool,
    /// Logical pixels from the panel's left edge to the output's right
    /// edge, when the caller knows it: the hover card goes left of the
    /// panel when it would not fit on the right. None puts it on the right.
    pub screen_right: Option<i32>,
}

/// A table under the card: the caption line, the left edge of each
/// column, and one hit rect per row.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TableGeom {
    pub caption_pos: (i32, i32),
    pub cols: Vec<i32>,
    pub rows: Vec<Rect>,
}

/// The poe.ninja block: heading, the price line, the detail line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NinjaGeom {
    pub head_pos: (i32, i32),
    pub price_pos: (i32, i32),
    pub detail_pos: (i32, i32),
}

/// The likely-price-fixed strip and the button inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StripGeom {
    pub strip: Rect,
    pub text_pos: (i32, i32),
    pub button: Rect,
}

/// A heading with lines under it; `note_pos` is the small line after them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockGeom {
    pub head_pos: (i32, i32),
    pub lines: Vec<(i32, i32)>,
    /// Index-aligned with `lines`: the block's lines wrapped to the panel.
    pub texts: Vec<String>,
    pub note_pos: Option<(i32, i32)>,
}

/// The bulk block: heading, the offers table, the note under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BulkGeom {
    pub head_pos: (i32, i32),
    pub table: TableGeom,
    pub note_pos: Option<(i32, i32)>,
}

/// The what-a-mod-is-worth block: heading, the button beside it, and per
/// row the baselines of the figures line and of the line naming the
/// listings, with the three column edges the figures line uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttributionGeom {
    pub head_pos: (i32, i32),
    pub button: Rect,
    /// The column captions, once there are rows to head.
    pub caption_pos: Option<(i32, i32)>,
    pub cols: [i32; 3],
    pub rows: Vec<(i32, i32)>,
}

/// The hover card beside the panel, in panel-local pixels (its x is
/// negative when it sits to the left).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardGeom {
    pub rect: Rect,
    pub title_pos: (i32, i32),
    /// The base line under the name, when the item has a name of its own.
    pub base_pos: Option<(i32, i32)>,
    pub rarity_pos: (i32, i32),
    pub figures: Vec<(i32, i32)>,
    /// Hairline y positions: under the header, under the figures, and
    /// where the mod blocks change.
    pub rules: Vec<i32>,
    pub badge_x: i32,
    pub text_x: i32,
    /// One baseline per card line.
    pub lines: Vec<i32>,
}

/// Everything clickable or drawable, in panel-local logical pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub size: (i32, i32),
    pub close: Rect,
    /// Name, rarity line, and the level lines.
    pub name_pos: (i32, i32),
    pub rarity_pos: (i32, i32),
    pub level_pos: Vec<(i32, String)>,
    /// The stack value line, when the item is a stack.
    pub stack_pos: Option<(i32, i32)>,
    pub base_check: Option<Rect>,
    pub base_label_pos: (i32, i32),
    /// Column headings drawn once above the rows.
    pub tiering_head_pos: (i32, i32),
    pub scoring_head_pos: (i32, i32),
    /// One entry per VISIBLE row, parallel to `visible_rows`.
    pub rows: Vec<RowGeom>,
    /// Indices into `Panel::rows` that `rows` describes.
    pub visible_rows: Vec<usize>,
    /// Hairline y positions between row groups (implicit vs explicit).
    pub dividers: Vec<i32>,
    /// "Show N more" / "Hide" toggle, when the item has hidden rows.
    pub hidden_toggle: Option<(Rect, String)>,
    pub strictness: Vec<(Rect, Strictness)>,
    pub buttons: Vec<(Rect, Action, &'static str)>,
    pub status_pos: (i32, i32),
    /// The budget text, right-aligned on the status line.
    pub budget_pos: Option<(i32, i32)>,
    pub ninja: Option<NinjaGeom>,
    pub ladder_pos: Option<(i32, i32)>,
    pub price_fixed: Option<StripGeom>,
    pub table: Option<TableGeom>,
    pub closest: Option<BlockGeom>,
    pub attribution: Option<AttributionGeom>,
    pub bulk: Option<BulkGeom>,
    pub card: Option<CardGeom>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RowGeom {
    pub check: Rect,
    /// Left gutter badge baseline; drawn only when the row has a badge.
    pub badge_pos: (i32, i32),
    pub label_pos: (i32, i32),
    /// Right gutter score baseline.
    pub score_pos: (i32, i32),
    pub min_box: Rect,
    pub max_box: Rect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Min,
    Max,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    ToggleRow(usize),
    ToggleBase,
    Edit(usize, Field),
    SetStrictness(Strictness),
    ToggleHidden,
    Search,
    OpenSite,
    Close,
    /// A click on this listing row: the same as pointing at it, so a
    /// click opens the card too (see `hover_hit` for the pointer).
    HoverRow(usize),
    /// Re-run the search priced in exalted and divine only.
    PriceFixedFilter,
    /// Price the strongest ticked mods one by one (on request only).
    Attribute,
}

/// Text drawn in the left gutter for a badge. "Other" affixes have no tier
/// ladder worth naming, so they get a dash rather than a made-up "O3".
pub fn badge_text(b: &TierBadge) -> String {
    match b.kind {
        AffixKind::Other => "—".to_string(),
        _ => format!("{}{}", b.kind.letter(), b.tier),
    }
}

/// Right-gutter score text; one decimal keeps the column narrow and stops
/// float noise from implying more precision than the ladder gives.
pub fn score_text(s: f32) -> String {
    format!("{s:.1}")
}

/// The card's property lines, in the tooltip's order: the item's computed
/// figures, each searchable as an `equipment_filters` minimum. The ones a
/// buyer shops by arrive switched on (see `ee2::filters`); the
/// box shows the floor that is searched, not the item's own figure. Chaos
/// DPS has no trade filter, so it is a display-only line: two rows driving
/// one bound would fight over it (the old card pointed Chaos DPS at the
/// total-DPS bound).
pub fn property_rows(props: &[khaloni_poe2_core::props::PropFilter], chaos_dps: f64) -> Vec<StatRow> {
    let row = |label: String, min: f64, enabled: bool, target: Option<Target>, hidden: bool| StatRow {
        label,
        badge: None,
        score: None,
        min: Some(min),
        max: None,
        enabled,
        target,
        hidden,
        group: RowGroup::Property,
        note: None,
    };
    let mut rows: Vec<StatRow> = props
        .iter()
        .map(|p| {
            // One decimal at most: 420.75 as a bound reads as noise.
            let value = (p.value * 10.0).round() / 10.0;
            row(format!("{}: {value}", p.key.label()), p.min, p.enabled, Some(Target::Equipment(p.key)), p.hidden)
        })
        .collect();
    if chaos_dps > 0.0 {
        let value = (chaos_dps * 10.0).round() / 10.0;
        rows.push(row(format!("Chaos DPS: {value}"), value, false, None, false));
    }
    rows
}

/// The status line under a finished search. It names the strictness the
/// listings came from: after a Broad search the value boxes still show the
/// user's own numbers while every bound that was sent sat 10% looser, and
/// nothing else on the card says so.
pub fn result_status(shown: usize, total: Option<u64>, searched: Strictness) -> String {
    let count = match total.filter(|t| *t > shown as u64) {
        Some(t) => format!("{shown} of {t} shown"),
        None => format!("{shown} shown"),
    };
    match searched {
        Strictness::Quick => count,
        Strictness::Broad => format!("Broad search, bounds -10%: {count}"),
    }
}

/// A keypress as a value box understands it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKey {
    Digit(char),
    Dot,
    Minus,
    Backspace,
}

const EDIT_MAX_LEN: usize = 8;

/// Applies one keypress to a value box's text. One decimal point, and a
/// minus only in front: bounds like -12 (reduced requirements, negative
/// resistances) and 3.5 are typeable, anything unparsable is not.
pub fn edit_key(buf: &mut String, key: EditKey) {
    match key {
        EditKey::Backspace => {
            buf.pop();
        }
        _ if buf.len() >= EDIT_MAX_LEN => {}
        EditKey::Digit(c) if c.is_ascii_digit() => buf.push(c),
        EditKey::Digit(_) => {}
        EditKey::Dot => {
            if !buf.contains('.') {
                if buf.is_empty() || buf == "-" {
                    buf.push('0');
                }
                buf.push('.');
            }
        }
        EditKey::Minus => {
            if buf.is_empty() {
                buf.push('-');
            }
        }
    }
}

/// The number a value box's text stands for; empty (or a lone "-") is "no
/// bound". A trailing "." ("3.") still reads as 3.
pub fn parse_edit(buf: &str) -> Option<f64> {
    let cleaned = buf.trim().trim_end_matches('.');
    if cleaned.is_empty() || cleaned == "-" {
        return None;
    }
    cleaned.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Writes one equipment bound into the query, dropping the whole section
/// when the last bound clears so an empty block never serializes.
pub fn set_equipment_bound(query: &mut khaloni_poe2_core::trade::Query, key: EquipKey, min: Option<f64>) {
    let eq = query.equipment.get_or_insert_with(Default::default);
    eq.set(key, min);
    if eq.is_empty() {
        query.equipment = None;
    }
}

/// Commits a value box: the typed text becomes the row's bound and the
/// bound of the filter behind it, from one parse, so the drawn box and the
/// search cannot differ. Enter runs this, and so does anything that takes
/// focus away from the box (a Search press, a click on another box):
/// leaving a box without Enter used to search with the old number while
/// the new one was still showing.
pub fn commit_edit(
    panel: &mut Panel,
    query: &mut khaloni_poe2_core::trade::Query,
    row_i: usize,
    field: Field,
    buf: &str,
) {
    let parsed = parse_edit(buf);
    let Some(row) = panel.rows.get_mut(row_i) else { return };
    match field {
        Field::Min => row.min = parsed,
        Field::Max => row.max = parsed,
    }
    let bound = |f: &mut khaloni_poe2_core::trade::StatFilter| match field {
        Field::Min => f.value.min = parsed,
        Field::Max => f.value.max = parsed,
    };
    match row.target {
        Some(Target::Stat(fi)) => {
            if let Some(f) = query.filters.get_mut(fi) {
                bound(f);
            }
        }
        Some(Target::Extra(xi)) => {
            if let Some(f) = panel.extras.get_mut(xi) {
                bound(f);
            }
        }
        // Equipment bounds are minimums only (hit() never yields a Max edit
        // for them), and only a row that is switched on has a live bound.
        Some(Target::Equipment(key)) if row.enabled && field == Field::Min => {
            set_equipment_bound(query, key, row.min);
        }
        Some(Target::Equipment(_)) | None => {}
    }
}

/// Flips a row's checkbox, and the filter behind it with it: the two are
/// one state shown twice and are written together.
pub fn toggle_row(panel: &mut Panel, query: &mut khaloni_poe2_core::trade::Query, row_i: usize) {
    let Some(row) = panel.rows.get_mut(row_i) else { return };
    row.enabled = !row.enabled;
    match row.target {
        Some(Target::Stat(fi)) => {
            if let Some(f) = query.filters.get_mut(fi) {
                f.disabled = !row.enabled;
            }
        }
        Some(Target::Equipment(key)) => {
            set_equipment_bound(query, key, row.min.filter(|_| row.enabled));
        }
        Some(Target::Extra(xi)) => {
            if let Some(f) = panel.extras.get_mut(xi) {
                f.disabled = !row.enabled;
            }
        }
        None => {}
    }
}

impl Panel {
    /// Rows currently drawn: everything, minus the collapsed ones.
    pub fn visible_rows(&self) -> Vec<usize> {
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, r)| !r.hidden || self.show_hidden)
            .map(|(i, _)| i)
            .collect()
    }

    pub fn hidden_count(&self) -> usize {
        self.rows.iter().filter(|r| r.hidden).count()
    }
}

/// Column left edges for a table whose cells are `rows` of strings under
/// `captions`: each column as wide as its widest cell or caption, plus the
/// gap. The last entry is the right edge of the last column.
fn table_columns(captions: &[&str], rows: &[Vec<String>], measure: &dyn Fn(&str) -> i32) -> Vec<i32> {
    let mut cols = Vec::with_capacity(captions.len() + 1);
    let mut x = PAD;
    for (ci, caption) in captions.iter().enumerate() {
        cols.push(x);
        let widest = rows.iter().filter_map(|r| r.get(ci)).map(|c| measure(c)).max().unwrap_or(0).max(measure(caption));
        x += widest + COL_GAP;
    }
    cols.push(x - COL_GAP);
    cols
}

fn listing_cells(panel: &Panel) -> Vec<Vec<String>> {
    panel.listings.iter().map(|r| r.cells().to_vec()).collect()
}

fn bulk_cells(b: &BulkBlock) -> Vec<Vec<String>> {
    b.offers.iter().map(|o| vec![o.have.clone(), o.want.clone(), o.stock.clone(), o.seller.clone()]).collect()
}

/// The what-a-mod-is-worth block is offered for an item with a ticked,
/// searchable mod (a rare or magic with its mods known, or a line EE2 gave
/// no row ticked into the search), and whenever it already has results to
/// show.
fn attribution_offered(panel: &Panel) -> bool {
    !panel.attribution.is_empty()
        || panel
            .rows
            .iter()
            .any(|r| r.enabled && matches!(r.target, Some(Target::Stat(_) | Target::Extra(_))))
}

/// The attribution block's heading, and the button that runs it: one
/// search per mod, so the button says what it spends.
pub const ATTRIBUTE_LABEL: &str = "What each mod is worth";
pub const ATTRIBUTE_BUTTON: &str = "Search per mod";
pub const ATTRIBUTION_CAPTIONS: [&str; 3] = ["MOD", "WITH", "WITHOUT"];

fn content_width(panel: &Panel, measure: &dyn Fn(&str) -> i32) -> i32 {
    let f = panel;
    // Every row is measured, hidden ones included, so expanding "Show N more"
    // cannot reflow the card out from under the cursor.
    let label_w = panel
        .rows
        .iter()
        .map(|r| measure(&r.text()))
        .chain(panel.header.base.iter().map(|b| measure(&b.label)))
        .max()
        .unwrap_or(0)
        .max(LABEL_MIN_W);
    // The name draws in the large font; approximate its advance so a long
    // rare name does not overrun the close box.
    let name_w = measure(&panel.header.name) * 3 / 2 + CLOSE + GAP;
    let row_w = PAD
        + GUT_L
        + CHECK
        + GAP
        + label_w
        + GAP
        + GUT_R
        + GAP
        + BOX_W * 2
        + BOX_GAP
        + PAD;
    // The status line starts to the right of the two buttons, and it is
    // where errors and the "Broad, N of M" account are read: it has to fit,
    // and so does the budget text right-aligned after it.
    let budget_w = if f.budget_text.is_empty() { 0 } else { GAP * 2 + measure(&f.budget_text) };
    let status_w = STATUS_X + measure(&panel.status) + budget_w + PAD;
    // Whatever is drawn as one line under the card must fit too. The
    // small dim lines are measured in the row face, which overstates them:
    // a line that fits the measure fits the panel.
    let mut lines: Vec<String> = Vec::new();
    lines.extend(f.stack_value.iter().cloned());
    if let Some(n) = &f.ninja {
        lines.push(format!("poe.ninja {} {}", n.price, n.direction));
        lines.push(ninja_detail(n));
    }
    lines.push(f.ladder.clone());
    // The closest lines wrap to whatever width the rest sets: a long "differs
    // by" list must not stretch the whole panel across the screen.
    if let Some(c) = &f.closest {
        lines.extend(c.nearest.iter().cloned());
    }
    lines.extend(f.attribution.iter().map(|a| a.text.clone()));
    lines.extend(f.bulk.iter().map(|b| b.note.clone()));
    let mut wide: Vec<i32> = lines.iter().map(|l| PAD + measure(l) + PAD).collect();
    if let Some(p) = &f.price_fixed {
        wide.push(PAD + 8 + measure(&p.text) + GAP + strip_button_w(&p.button, measure) + 4 + PAD);
    }
    if !f.listings.is_empty() {
        wide.push(*table_columns(&LISTING_CAPTIONS, &listing_cells(panel), measure).last().unwrap() + PAD);
    }
    if attribution_offered(panel) {
        wide.push(PAD + measure(ATTRIBUTE_LABEL) + GAP + button_w(ATTRIBUTE_BUTTON, measure) + PAD);
        let cells: Vec<Vec<String>> =
            f.attribution.iter().map(|a| vec![a.label.clone(), a.with.clone(), a.without.clone()]).collect();
        wide.push(*table_columns(&ATTRIBUTION_CAPTIONS, &cells, measure).last().unwrap() + PAD);
    }
    if let Some(b) = &f.bulk {
        wide.push(*table_columns(&BULK_CAPTIONS, &bulk_cells(b), measure).last().unwrap() + PAD);
    }
    let line_w = wide.into_iter().max().unwrap_or(0);
    row_w.max(status_w).max(line_w).max(PAD + name_w + PAD).clamp(WIDTH_MIN, WIDTH_MAX)
}

/// `text` in lines that measure at most `max_w`, broken between words; a
/// single word wider than that keeps a line of its own.
pub fn wrap_words(text: &str, max_w: i32, measure: &dyn Fn(&str) -> i32) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split(' ') {
        let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
        if line.is_empty() || measure(&candidate) <= max_w {
            line = candidate;
        } else {
            lines.push(std::mem::replace(&mut line, word.to_string()));
        }
    }
    lines.push(line);
    lines
}

/// The ninja block's second line: band, volume and the caveat, the parts
/// that are present, joined by a middle dot.
pub fn ninja_detail(n: &NinjaBlock) -> String {
    let volume = if n.volume.is_empty() { String::new() } else { format!("{} traded", n.volume) };
    [n.band.as_str(), volume.as_str(), n.note.as_str()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" · ")
}

fn button_w(label: &str, measure: &dyn Fn(&str) -> i32) -> i32 {
    measure(label) + PAD * 2
}

fn strip_button_w(label: &str, measure: &dyn Fn(&str) -> i32) -> i32 {
    measure(label) + GAP * 2
}

/// The card, its controls, and every block the panel carries under them.
pub fn layout(panel: &Panel, measure: &dyn Fn(&str) -> i32) -> Layout {
    let f = panel;
    let w = content_width(panel, measure);
    let h = &panel.header;

    // --- title bar -------------------------------------------------------
    let name_pos = (PAD, PAD + TITLE_H - 10);
    let close = Rect { x: w - PAD - CLOSE, y: PAD, w: CLOSE as u32, h: CLOSE as u32 };
    let mut y = PAD + TITLE_H;

    // --- header block: rarity, then only the level lines the item has ----
    let rarity_pos = (PAD, y + LINE_H - 5);
    y += LINE_H;
    let mut level_pos = Vec::new();
    if let Some(il) = h.item_level {
        level_pos.push((y + LINE_H - 5, format!("Item Level: {il}")));
        y += LINE_H;
    }
    if let Some(rl) = h.requires_level {
        level_pos.push((y + LINE_H - 5, format!("Requires Level: {rl}")));
        y += LINE_H;
    }
    // A stack's worth, on the header like the level lines.
    let stack_pos = f.stack_value.as_ref().map(|_| {
        let p = (PAD, y + LINE_H - 5);
        y += LINE_H;
        p
    });

    // --- base-type toggle, on its own line under the header --------------
    let (base_check, base_label_pos) = if h.base.is_some() {
        y += BLOCK_GAP;
        let check = Rect { x: PAD, y: y + (ROW_H - CHECK) / 2, w: CHECK as u32, h: CHECK as u32 };
        let label_pos = (PAD + CHECK + GAP, y + ROW_H - 8);
        y += ROW_H;
        (Some(check), label_pos)
    } else {
        (None, (0, 0))
    };

    // --- column geometry, shared by the headings and every row -----------
    let check_x = PAD + GUT_L;
    let label_x = check_x + CHECK + GAP;
    let max_box_x = w - PAD - BOX_W;
    let min_box_x = max_box_x - BOX_GAP - BOX_W;
    let score_x = min_box_x - GAP - GUT_R;

    y += BLOCK_GAP;
    let head_baseline = y + HEAD_H - 6;
    let tiering_head_pos = (PAD, head_baseline);
    let scoring_head_pos = (score_x, head_baseline);
    y += HEAD_H;

    // --- row band --------------------------------------------------------
    let visible_rows = panel.visible_rows();
    let mut rows = Vec::with_capacity(visible_rows.len());
    let mut dividers = Vec::new();
    let mut prev_group: Option<RowGroup> = None;
    for &ri in &visible_rows {
        let group = panel.rows[ri].group;
        // A block change gets a hairline with air on both sides, so the
        // implicit and explicit blocks read apart like the tooltip's own.
        if prev_group.is_some_and(|p| p != group) {
            y += DIV_GAP;
            dividers.push(y);
            y += DIV_GAP;
        }
        prev_group = Some(group);
        let baseline = y + ROW_H - 8;
        rows.push(RowGeom {
            check: Rect {
                x: check_x,
                y: y + (ROW_H - CHECK) / 2,
                w: CHECK as u32,
                h: CHECK as u32,
            },
            badge_pos: (PAD, baseline),
            label_pos: (label_x, baseline),
            score_pos: (score_x, baseline),
            min_box: Rect {
                x: min_box_x,
                y: y + (ROW_H - BOX_H) / 2,
                w: BOX_W as u32,
                h: BOX_H as u32,
            },
            max_box: Rect {
                x: max_box_x,
                y: y + (ROW_H - BOX_H) / 2,
                w: BOX_W as u32,
                h: BOX_H as u32,
            },
        });
        y += ROW_H;
    }

    // --- collapse toggle, only when there is something collapsed ---------
    let nhidden = panel.hidden_count();
    let hidden_toggle = (nhidden > 0).then(|| {
        let text = if panel.show_hidden {
            format!("Hide {nhidden}")
        } else {
            format!("Show {nhidden} more")
        };
        let r = Rect {
            x: label_x,
            y,
            w: (measure(&text) + GAP * 2).max(60) as u32,
            h: TOGGLE_H as u32,
        };
        y += TOGGLE_H;
        (r, text)
    });

    // --- strictness radios ----------------------------------------------
    y += BLOCK_GAP;
    let mut strictness = Vec::new();
    let mut rx = PAD;
    for s in [Strictness::Quick, Strictness::Broad] {
        let rw = CHECK + GAP + measure(s.label()) + GAP;
        strictness.push((Rect { x: rx, y, w: rw as u32, h: RADIO_H as u32 }, s));
        rx += rw + RADIO_GAP;
    }
    y += RADIO_H + 8;

    // --- the actions, with the status and the budget beside them ---------
    let buttons = vec![
        (Rect { x: PAD, y, w: SEARCH_W as u32, h: BTN_H as u32 }, Action::Search, "Search"),
        (
            Rect { x: PAD + SEARCH_W + BTN_GAP, y, w: OPEN_SITE_W as u32, h: BTN_H as u32 },
            Action::OpenSite,
            "Open site",
        ),
    ];
    let status_pos = (STATUS_X, y + BTN_H - 9);
    let budget_pos = (!f.budget_text.is_empty()).then(|| (w - PAD - measure(&f.budget_text), status_pos.1));
    y += BTN_H + 6;

    // --- what the search found, block by block ---------------------------
    let ninja = f.ninja.as_ref().map(|_| {
        y += BLOCK_GAP;
        let head_pos = (PAD, y + SECTION_H - 6);
        y += SECTION_H;
        let price_pos = (PAD, y + LISTING_H - 6);
        y += LISTING_H;
        let detail_pos = (PAD, y + NOTE_H - 5);
        y += NOTE_H;
        NinjaGeom { head_pos, price_pos, detail_pos }
    });
    let ladder_pos = (!f.ladder.is_empty()).then(|| {
        y += DIV_GAP;
        let p = (PAD, y + NOTE_H - 5);
        y += NOTE_H;
        p
    });
    let price_fixed = f.price_fixed.as_ref().map(|p| {
        y += BLOCK_GAP;
        let strip = Rect { x: PAD, y, w: (w - PAD * 2) as u32, h: STRIP_H as u32 };
        let bw = strip_button_w(&p.button, measure);
        let button = Rect { x: w - PAD - 4 - bw, y: y + 3, w: bw as u32, h: (STRIP_H - 6) as u32 };
        let text_pos = (PAD + 8, y + STRIP_H - 9);
        y += STRIP_H;
        StripGeom { strip, text_pos, button }
    });
    let table = (!f.listings.is_empty()).then(|| {
        y += BLOCK_GAP;
        let caption_pos = (PAD, y + CAPTION_H - 5);
        y += CAPTION_H;
        let cols = table_columns(&LISTING_CAPTIONS, &listing_cells(panel), measure);
        let rows = f
            .listings
            .iter()
            .map(|_| {
                let r = Rect { x: PAD, y, w: (w - PAD * 2) as u32, h: LISTING_H as u32 };
                y += LISTING_H;
                r
            })
            .collect();
        TableGeom { caption_pos, cols, rows }
    });
    let closest = f.closest.as_ref().map(|c| {
        y += BLOCK_GAP;
        let head_pos = (PAD, y + SECTION_H - 6);
        y += SECTION_H;
        let texts: Vec<String> = c.lines.iter().flat_map(|l| wrap_words(l, w - PAD * 2, measure)).collect();
        let lines = texts
            .iter()
            .map(|_| {
                let p = (PAD, y + LISTING_H - 6);
                y += LISTING_H;
                p
            })
            .collect();
        let note_pos = c.nearest.as_ref().map(|_| {
            let p = (PAD, y + NOTE_H - 5);
            y += NOTE_H;
            p
        });
        BlockGeom { head_pos, lines, texts, note_pos }
    });
    let attribution = attribution_offered(panel).then(|| {
        y += BLOCK_GAP;
        let bw = button_w(ATTRIBUTE_BUTTON, measure);
        let button = Rect { x: w - PAD - bw, y, w: bw as u32, h: BTN_H as u32 };
        let head_pos = (PAD, y + BTN_H - 9);
        y += BTN_H;
        let cells: Vec<Vec<String>> =
            f.attribution.iter().map(|a| vec![a.label.clone(), a.with.clone(), a.without.clone()]).collect();
        let c = table_columns(&ATTRIBUTION_CAPTIONS, &cells, measure);
        let caption_pos = (!cells.is_empty()).then(|| {
            let p = (PAD, y + CAPTION_H - 5);
            y += CAPTION_H;
            p
        });
        let rows = f
            .attribution
            .iter()
            .map(|_| {
                let figures = y + LISTING_H - 6;
                y += LISTING_H;
                let named = y + NOTE_H - 5;
                y += NOTE_H;
                (figures, named)
            })
            .collect();
        AttributionGeom { head_pos, button, caption_pos, cols: [c[0], c[1], c[2]], rows }
    });
    let bulk = f.bulk.as_ref().map(|b| {
        y += BLOCK_GAP;
        let head_pos = (PAD, y + SECTION_H - 6);
        y += SECTION_H;
        let caption_pos = (PAD, y + CAPTION_H - 5);
        y += CAPTION_H;
        let cols = table_columns(&BULK_CAPTIONS, &bulk_cells(b), measure);
        let rows = b
            .offers
            .iter()
            .map(|_| {
                let r = Rect { x: PAD, y, w: (w - PAD * 2) as u32, h: LISTING_H as u32 };
                y += LISTING_H;
                r
            })
            .collect();
        let note_pos = (!b.note.is_empty()).then(|| {
            let p = (PAD, y + NOTE_H - 5);
            y += NOTE_H;
            p
        });
        BulkGeom { head_pos, table: TableGeom { caption_pos, cols, rows }, note_pos }
    });
    let size = (w, y + PAD);

    // --- the hover card, beside the panel --------------------------------
    let card = f.hover.and_then(|i| {
        let row = table.as_ref()?.rows.get(i)?;
        Some(card_layout(&f.listings[i].card, row.y, size, f.screen_right, measure))
    });

    Layout {
        size,
        close,
        name_pos,
        rarity_pos,
        level_pos,
        stack_pos,
        base_check,
        base_label_pos,
        tiering_head_pos,
        scoring_head_pos,
        rows,
        visible_rows,
        dividers,
        hidden_toggle,
        strictness,
        buttons,
        status_pos,
        budget_pos,
        ninja,
        ladder_pos,
        price_fixed,
        table,
        closest,
        attribution,
        bulk,
        card,
    }
}

/// The card's title: the item's name, or its base when it has none.
pub fn card_title(card: &Card) -> &str {
    if card.name.is_empty() {
        &card.base
    } else {
        &card.name
    }
}

/// The block a card line belongs to, for the hairline between blocks: the
/// game's tooltip rules runes, implicits and enchants off from the rest.
fn card_block(kind: LineKind) -> u8 {
    match kind {
        LineKind::Rune | LineKind::Implicit | LineKind::Enchant => 0,
        _ => 1,
    }
}

/// Lays the hover card out beside the panel: to the right, or to the left
/// when `screen_right` says it would not fit. Its top follows the hovered
/// row (`row_y`) as far as the panel's height allows, so the card always
/// spans the row it belongs to.
fn card_layout(card: &Card, row_y: i32, panel: (i32, i32), screen_right: Option<i32>, measure: &dyn Fn(&str) -> i32) -> CardGeom {
    let gutter = card.lines.iter().map(|l| measure(&l.tiers.join(" "))).max().unwrap_or(0);
    let badge_x = PAD;
    let text_x = if gutter > 0 { PAD + gutter + GAP } else { PAD };
    // The title draws in the large face, about a quarter wider than the
    // row face measures it.
    let title_w = measure(card_title(card)) * 5 / 4;
    let figure_w = card.figures.iter().map(|(k, v)| measure(&format!("{k}: {v}"))).max().unwrap_or(0);
    let line_w = card.lines.iter().map(|l| measure(&l.text)).max().unwrap_or(0);
    let w = (text_x + line_w)
        .max(PAD + title_w)
        .max(PAD + measure(&card.base))
        .max(PAD + measure(&format!("Rarity: {}", card.rarity)))
        .max(PAD + figure_w)
        .max(CARD_MIN_W - PAD)
        + PAD;

    let mut y = PAD;
    let title_pos = (PAD, y + CARD_TITLE_H - 8);
    y += CARD_TITLE_H;
    let base_pos = (!card.name.is_empty() && !card.base.is_empty()).then(|| {
        let p = (PAD, y + LINE_H - 5);
        y += LINE_H;
        p
    });
    let rarity_pos = (PAD, y + LINE_H - 5);
    y += LINE_H;
    let mut rules = Vec::new();
    if !card.figures.is_empty() {
        y += DIV_GAP;
        rules.push(y);
        y += DIV_GAP;
    }
    let figures: Vec<(i32, i32)> = card
        .figures
        .iter()
        .map(|_| {
            let p = (PAD, y + LINE_H - 5);
            y += LINE_H;
            p
        })
        .collect();
    if !card.lines.is_empty() {
        y += DIV_GAP;
        rules.push(y);
        y += DIV_GAP;
    }
    let mut lines = Vec::with_capacity(card.lines.len());
    let mut prev: Option<u8> = None;
    for l in &card.lines {
        let block = card_block(l.kind);
        if prev.is_some_and(|p| p != block) {
            y += DIV_GAP;
            rules.push(y);
            y += DIV_GAP;
        }
        prev = Some(block);
        lines.push(y + LISTING_H - 6);
        y += LISTING_H;
    }
    let h = y + PAD;

    let fits_right = screen_right.is_none_or(|edge| panel.0 + CARD_GAP + w <= edge);
    let x = if fits_right { panel.0 + CARD_GAP } else { -(w + CARD_GAP) };
    let top = row_y.min((panel.1 - h).max(0));
    let shift = |p: (i32, i32)| (p.0 + x, p.1 + top);
    CardGeom {
        rect: Rect { x, y: top, w: w as u32, h: h as u32 },
        title_pos: shift(title_pos),
        base_pos: base_pos.map(shift),
        rarity_pos: shift(rarity_pos),
        figures: figures.into_iter().map(shift).collect::<Vec<_>>(),
        rules: rules.into_iter().map(|r| r + top).collect(),
        badge_x: badge_x + x,
        text_x: text_x + x,
        lines: lines.into_iter().map(|l| l + top).collect(),
    }
}

fn row_under(lay: &Layout, x: i32, y: i32) -> Option<usize> {
    lay.table.as_ref().and_then(|t| t.rows.iter().position(|r| inside(r, x, y)))
}

/// Where the pointer is over the listings table: the row under it, or
/// none. Polled on motion, unlike `hit`, which answers clicks.
pub fn hover_hit(lay: &Layout, x: i32, y: i32) -> Option<usize> {
    row_under(lay, x, y)
}

/// Every string the panel draws, for checks against the overlay face and
/// for the preview's label listing. Order follows the layout, top down,
/// then the hover card.
pub fn all_text(panel: &Panel, measure: &dyn Fn(&str) -> i32) -> Vec<String> {
    let f = panel;
    let lay = layout(panel, measure);
    let mut out = vec!["x".to_string(), panel.header.name.clone(), format!("Rarity: {}", panel.header.rarity)];
    out.extend(lay.level_pos.iter().map(|(_, t)| t.clone()));
    out.extend(f.stack_value.iter().cloned());
    out.extend(panel.header.base.iter().map(|b| b.label.clone()));
    out.extend(["TIERING".to_string(), "SCORING".to_string()]);
    for &i in &lay.visible_rows {
        let row = &panel.rows[i];
        out.push(row.text());
        out.extend(row.badge.as_ref().map(badge_text));
        out.extend(row.score.map(score_text));
    }
    out.extend(lay.hidden_toggle.iter().map(|(_, t)| t.clone()));
    out.extend([Strictness::Quick, Strictness::Broad].iter().map(|s| s.label().to_string()));
    out.extend(lay.buttons.iter().map(|(_, _, l)| l.to_string()));
    out.push(panel.status.clone());
    if lay.budget_pos.is_some() {
        out.push(f.budget_text.clone());
    }
    if let Some(n) = &f.ninja {
        out.extend(["poe.ninja".to_string(), n.price.clone(), n.direction.clone(), n.band.clone(), n.note.clone()]);
        if !n.volume.is_empty() {
            out.push(format!("{} traded", n.volume));
        }
    }
    if lay.ladder_pos.is_some() {
        out.push(f.ladder.clone());
    }
    if let Some(p) = &f.price_fixed {
        out.extend([p.text.clone(), p.button.clone()]);
    }
    if lay.table.is_some() {
        out.extend(LISTING_CAPTIONS.iter().map(|c| c.to_string()));
        for r in &f.listings {
            out.extend(r.cells().into_iter().filter(|c| !c.is_empty()));
        }
    }
    if let (Some(g), Some(c)) = (&lay.closest, &f.closest) {
        out.push("Closest listings".to_string());
        out.extend(g.texts.iter().cloned());
        out.extend(c.nearest.iter().cloned());
    }
    if lay.attribution.is_some() {
        out.extend([ATTRIBUTE_LABEL.to_string(), ATTRIBUTE_BUTTON.to_string()]);
        if !f.attribution.is_empty() {
            out.extend(ATTRIBUTION_CAPTIONS.iter().map(|c| c.to_string()));
        }
        for a in &f.attribution {
            out.extend([a.label.clone(), a.with.clone(), a.without.clone(), a.text.clone()]);
        }
    }
    if let Some(b) = &f.bulk {
        out.push("Bulk offers".to_string());
        out.extend(BULK_CAPTIONS.iter().map(|c| c.to_string()));
        for o in &b.offers {
            out.extend([o.have.clone(), o.want.clone(), o.stock.clone(), o.seller.clone()]);
        }
        if !b.note.is_empty() {
            out.push(b.note.clone());
        }
    }
    if let Some(card) = f.hover.and_then(|i| f.listings.get(i)).map(|r| &r.card).filter(|_| lay.card.is_some()) {
        out.push(card_title(card).to_string());
        if !card.name.is_empty() {
            out.push(card.base.clone());
        }
        out.push(format!("Rarity: {}", card.rarity));
        out.extend(card.figures.iter().map(|(k, v)| format!("{k}: {v}")));
        for l in &card.lines {
            out.push(l.text.clone());
            out.extend(l.tiers.iter().cloned());
        }
    }
    out.retain(|s| !s.is_empty());
    out
}

fn inside(r: &Rect, x: i32, y: i32) -> bool {
    x >= r.x && x < r.x + r.w as i32 && y >= r.y && y < r.y + r.h as i32
}

pub fn hit(panel: &Panel, lay: &Layout, x: i32, y: i32) -> Option<Action> {
    if inside(&lay.close, x, y) {
        return Some(Action::Close);
    }
    // The blocks under the card: the price-fixed button, the attribution
    // button (which answers even when switched off: the caller knows the
    // budget and decides, the way it does for Search), and the rows.
    if lay.price_fixed.as_ref().is_some_and(|s| inside(&s.button, x, y)) {
        return Some(Action::PriceFixedFilter);
    }
    if lay.attribution.as_ref().is_some_and(|a| inside(&a.button, x, y)) {
        return Some(Action::Attribute);
    }
    if let Some(i) = row_under(lay, x, y) {
        return Some(Action::HoverRow(i));
    }
    for (rect, action, _) in &lay.buttons {
        if inside(rect, x, y) {
            return Some(*action);
        }
    }
    // Base-type row: the whole line, checkbox and label alike, toggles it.
    if let Some(check) = &lay.base_check {
        let row = Rect {
            x: PAD,
            y: check.y - (ROW_H - CHECK) / 2,
            w: (lay.size.0 - 2 * PAD) as u32,
            h: ROW_H as u32,
        };
        if inside(&row, x, y) {
            return Some(Action::ToggleBase);
        }
    }
    if let Some((rect, _)) = &lay.hidden_toggle {
        if inside(rect, x, y) {
            return Some(Action::ToggleHidden);
        }
    }
    for (rect, s) in &lay.strictness {
        if inside(rect, x, y) {
            return Some(Action::SetStrictness(*s));
        }
    }
    for (i, g) in lay.rows.iter().enumerate() {
        // Actions carry the index into `panel.rows`, never the visible
        // position: collapsing rows must not renumber what a click means.
        let idx = lay.visible_rows[i];
        let Some(target) = panel.rows[idx].target else {
            // Display-only line (unmatched mod, unsearchable stat): it has
            // no filter to drive, so it swallows nothing and offers nothing.
            continue;
        };
        if inside(&g.min_box, x, y) {
            return Some(Action::Edit(idx, Field::Min));
        }
        // Equipment bounds are minimums only; their max box is not drawn and
        // must not be editable.
        if target.has_max() && inside(&g.max_box, x, y) {
            return Some(Action::Edit(idx, Field::Max));
        }
        let band = Rect {
            x: PAD,
            y: g.check.y - (ROW_H - CHECK) / 2,
            w: (g.min_box.x - BOX_GAP - PAD).max(CHECK) as u32,
            h: ROW_H as u32,
        };
        if inside(&band, x, y) {
            return Some(Action::ToggleRow(idx));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stand-in measurer: a fixed advance per char, enough to exercise the
    /// geometry without a font.
    fn m(s: &str) -> i32 {
        7 * s.len() as i32
    }

    fn row(i: usize) -> StatRow {
        StatRow {
            label: format!("{i} to Accuracy Rating"),
            badge: Some(TierBadge { kind: AffixKind::Suffix, tier: 3 }),
            score: Some(4.0),
            min: Some(i as f64),
            max: None,
            enabled: true,
            target: Some(Target::Stat(i)),
            hidden: false,
            group: RowGroup::default(),
            note: None,
        }
    }

    fn hidden(i: usize) -> StatRow {
        StatRow { hidden: true, ..row(i) }
    }

    /// A derived line (DPS, total attributes): drawn, never searched.
    fn display_only(i: usize) -> StatRow {
        StatRow { target: None, badge: None, score: None, ..row(i) }
    }

    fn panel(rows: Vec<StatRow>) -> Panel {
        Panel {
            header: ItemHeader {
                name: "Horror Bane".into(),
                rarity: "Rare".into(),
                item_level: Some(82),
                requires_level: Some(65),
                base: None,
            },
            rows,
            ..Panel::default()
        }
    }

    #[test]
    fn hidden_rows_stay_collapsed_until_the_toggle_is_flipped() {
        let mut p = panel(vec![row(0), hidden(1), hidden(2), row(3)]);
        let lay = layout(&p, &m);
        assert_eq!(lay.visible_rows, vec![0, 3]);
        assert_eq!(lay.rows.len(), lay.visible_rows.len());
        let (_, label) = lay.hidden_toggle.clone().expect("two rows are hidden");
        assert_eq!(label, "Show 2 more");

        p.show_hidden = true;
        let lay = layout(&p, &m);
        assert_eq!(lay.visible_rows, vec![0, 1, 2, 3]);
        assert_eq!(lay.rows.len(), 4);
        assert_eq!(lay.hidden_toggle.expect("still collapsible").1, "Hide 2");
    }

    #[test]
    fn no_toggle_when_nothing_is_hidden() {
        assert!(layout(&panel(vec![row(0), row(1)]), &m).hidden_toggle.is_none());
    }

    #[test]
    fn clicks_resolve_to_the_original_row_index_not_the_visible_one() {
        // Hidden rows interleaved: visible position 1 is panel row 3.
        let p = panel(vec![row(0), hidden(1), hidden(2), row(3)]);
        let lay = layout(&p, &m);
        let g = &lay.rows[1];
        assert_eq!(hit(&p, &lay, g.check.x + 2, g.check.y + 2), Some(Action::ToggleRow(3)));
        assert_eq!(
            hit(&p, &lay, g.min_box.x + 2, g.min_box.y + 2),
            Some(Action::Edit(3, Field::Min))
        );
        assert_eq!(
            hit(&p, &lay, g.max_box.x + 2, g.max_box.y + 2),
            Some(Action::Edit(3, Field::Max))
        );
    }

    #[test]
    fn display_only_rows_are_inert() {
        let p = panel(vec![display_only(0), row(1)]);
        let lay = layout(&p, &m);
        let g = &lay.rows[0];
        assert_eq!(hit(&p, &lay, g.check.x + 2, g.check.y + 2), None);
        assert_eq!(hit(&p, &lay, g.min_box.x + 2, g.min_box.y + 2), None);
        assert_eq!(hit(&p, &lay, g.max_box.x + 2, g.max_box.y + 2), None);
        // The searchable row below it still works.
        let g = &lay.rows[1];
        assert_eq!(hit(&p, &lay, g.check.x + 2, g.check.y + 2), Some(Action::ToggleRow(1)));
    }

    fn extra(text: &str, into: &[&str], ids: &[&str]) -> khaloni_poe2_core::ee2::request::ExtraRow {
        khaloni_poe2_core::ee2::request::ExtraRow {
            text: text.into(),
            tag: "explicit",
            rolled: Some(27.0),
            lines: vec![text.into()],
            into: into.iter().map(|s| s.to_string()).collect(),
            group: "explicit",
            ids: ids.iter().map(|s| s.to_string()).collect(),
            option: None,
            value: khaloni_poe2_core::trade::FilterValue { min: Some(25.0), max: None },
            lookup: Vec::new(),
            stat_keys: Vec::new(),
        }
    }

    /// A line a total counted is a row of its own: checkbox, value boxes,
    /// unticked, the total named in its note. Ticking it puts exactly its
    /// filter on the search; unticking takes it off again. The query EE2
    /// built is never written.
    #[test]
    fn a_counted_line_is_a_row_the_user_can_tick_into_the_search() {
        let mut extras = Vec::new();
        let rows = extra_rows(
            &[
                extra("Adds 27 to 36 Fire Damage", &["Total DPS", "Elemental DPS"], &["explicit.stat_709508406"]),
                extra("12% increased Wombat Summoning Speed", &[], &[]),
            ],
            &mut extras,
        );
        assert_eq!(rows[0].text(), "Adds 27 to 36 Fire Damage \u{2014} counted in Total DPS, Elemental DPS");
        assert_eq!(
            (rows[0].target, rows[0].min, rows[0].max, rows[0].enabled, rows[0].hidden),
            (Some(Target::Extra(0)), Some(25.0), None, false, false)
        );
        assert_eq!(extras.len(), 1, "only a line with a trade id has a filter");
        assert!(extras[0].disabled, "unticked until the user ticks it");

        let mut p = Panel { extras, ..panel(vec![rows[0].clone(), row(1), hidden(2)]) };
        let lay = layout(&p, &m);
        assert_eq!(lay.visible_rows, [0, 1], "the extra row is drawn without asking for the hidden ones");
        let g = &lay.rows[0];
        assert_eq!(hit(&p, &lay, g.check.x + 2, g.check.y + 2), Some(Action::ToggleRow(0)));
        assert_eq!(hit(&p, &lay, g.min_box.x + 2, g.min_box.y + 2), Some(Action::Edit(0, Field::Min)));
        assert_eq!(hit(&p, &lay, g.max_box.x + 2, g.max_box.y + 2), Some(Action::Edit(0, Field::Max)));
        // The card is wide enough for the line and its note together.
        assert!(lay.size.0 >= g.label_pos.0 + m(&p.rows[0].text()));

        let mut query = khaloni_poe2_core::trade::Query {
            filters: vec![khaloni_poe2_core::trade::StatFilter::at_least("explicit.stat_1", 10.0, false)],
            ..Default::default()
        };
        let before = query.clone();
        assert_eq!(search_query(&p, &query), before, "unticked, the search is EE2's");
        toggle_row(&mut p, &mut query, 0);
        assert_eq!(query, before, "ticking an extra row leaves EE2's query alone");
        let searched = search_query(&p, &query);
        assert_eq!(searched.filters.len(), 2);
        assert_eq!(searched.filters[1].id, "explicit.stat_709508406");
        assert!(!searched.filters[1].disabled);
        commit_edit(&mut p, &mut query, 0, Field::Min, "30");
        assert_eq!(search_query(&p, &query).filters[1].value.min, Some(30.0), "the box is the bound searched");
        toggle_row(&mut p, &mut query, 0);
        assert_eq!(search_query(&p, &query), before, "unticked again, gone again");
    }

    /// A line no catalog lists keeps its row, with the reason and nothing
    /// to click.
    #[test]
    fn a_line_without_a_trade_id_says_why_and_offers_nothing() {
        let mut extras = Vec::new();
        let rows = extra_rows(&[extra("12% increased Wombat Summoning Speed", &[], &[])], &mut extras);
        assert!(extras.is_empty());
        assert_eq!(
            rows[0].text(),
            format!("12% increased Wombat Summoning Speed \u{2014} {}", khaloni_poe2_core::ee2::request::NO_TRADE_STAT)
        );
        assert_eq!((rows[0].target, rows[0].min, rows[0].max), (None, None, None));
        let p = panel(vec![rows[0].clone(), row(1)]);
        let lay = layout(&p, &m);
        let g = &lay.rows[0];
        for (x, y) in [
            (g.check.x + 2, g.check.y + 2),
            (g.label_pos.0 + 5, g.check.y + 2),
            (g.min_box.x + 2, g.min_box.y + 2),
            (g.max_box.x + 2, g.max_box.y + 2),
        ] {
            assert_eq!(hit(&p, &lay, x, y), None, "({x},{y}) on a row with no trade id does something");
        }
    }

    #[test]
    fn hidden_toggle_hit_tests() {
        let p = panel(vec![row(0), hidden(1)]);
        let lay = layout(&p, &m);
        let (r, _) = lay.hidden_toggle.clone().unwrap();
        assert_eq!(hit(&p, &lay, r.x + 3, r.y + 3), Some(Action::ToggleHidden));
    }

    #[test]
    fn strictness_rects_resolve_to_their_own_variant() {
        let p = panel(vec![row(0)]);
        let lay = layout(&p, &m);
        assert_eq!(lay.strictness.len(), 2);
        for (r, s) in &lay.strictness {
            assert_eq!(hit(&p, &lay, r.x + 2, r.y + 2), Some(Action::SetStrictness(*s)));
        }
        assert_eq!(lay.strictness[0].1, Strictness::Quick);
        assert_eq!(lay.strictness[1].1, Strictness::Broad);
        // Distinct targets, not stacked on one another.
        assert!(lay.strictness[1].0.x >= lay.strictness[0].0.x + lay.strictness[0].0.w as i32);
    }

    /// The value box used to sit between the radios and the buttons; the
    /// blocks under the card replaced it. The buttons now follow the
    /// radios directly, and the blocks come after them.
    #[test]
    fn the_buttons_follow_the_radios_and_the_blocks_follow_the_buttons() {
        let p = panel(vec![row(0)]);
        let bare = layout(&p, &m);
        let radios_bottom = bare.strictness[0].0.y + RADIO_H;
        assert_eq!(bare.buttons[0].0.y, radios_bottom + 8);

        let found = Panel {
            ninja: Some(NinjaBlock { price: "4 div".into(), ..Default::default() }),
            ladder: "cheapest 2 ex".into(),
            ..p.clone()
        };
        let lay = layout(&found, &m);
        assert_eq!(lay.buttons[0].0.y, bare.buttons[0].0.y, "the blocks do not move the buttons");
        let n = lay.ninja.unwrap();
        assert!(n.head_pos.1 > lay.buttons[0].0.y + BTN_H);
        assert!(lay.ladder_pos.unwrap().1 > n.detail_pos.1);
        assert!(lay.size.1 > bare.size.1);
    }

    #[test]
    fn headings_and_gutters_frame_the_card() {
        let p = panel(vec![row(0)]);
        let lay = layout(&p, &m);
        let g = &lay.rows[0];
        // TIERING sits over the badge column, SCORING over the score column.
        assert_eq!(lay.tiering_head_pos.0, g.badge_pos.0);
        assert_eq!(lay.scoring_head_pos.0, g.score_pos.0);
        assert!(lay.tiering_head_pos.1 < g.label_pos.1, "headings sit above the rows");
        // Left gutter, checkbox, label, right gutter, then the boxes.
        assert!(g.badge_pos.0 < g.check.x);
        assert!(g.check.x + CHECK <= g.label_pos.0);
        assert!(g.label_pos.0 < g.score_pos.0);
        assert!(g.score_pos.0 + GUT_R <= g.min_box.x);
        assert!(g.min_box.x + BOX_W <= g.max_box.x);
        assert!(g.max_box.x + BOX_W + PAD <= lay.size.0);
    }

    #[test]
    fn header_lines_appear_only_when_the_item_has_them() {
        let mut p = panel(vec![row(0)]);
        assert_eq!(layout(&p, &m).level_pos.len(), 2);
        p.header.requires_level = None;
        let lay = layout(&p, &m);
        assert_eq!(lay.level_pos.len(), 1);
        assert!(lay.level_pos[0].1.contains("82"));
        p.header.item_level = None;
        assert!(layout(&p, &m).level_pos.is_empty());
    }

    #[test]
    fn base_toggle_row_sits_above_the_rows_and_hit_tests() {
        let mut p = panel(vec![row(0)]);
        assert!(layout(&p, &m).base_check.is_none());
        p.header.base = Some(BaseToggle { label: "Advanced Zealot Bow".into(), enabled: true });
        let lay = layout(&p, &m);
        let c = lay.base_check.expect("base row present");
        assert!(c.y < lay.rows[0].check.y);
        assert_eq!(hit(&p, &lay, c.x + 1, c.y + 1), Some(Action::ToggleBase));
    }

    #[test]
    fn width_grows_with_the_longest_label() {
        let narrow = layout(&panel(vec![row(0)]), &m).size.0;
        let mut wide = panel(vec![row(0)]);
        wide.rows[0].label =
            "a very long mod description that must widen the panel considerably".into();
        assert!(layout(&wide, &m).size.0 > narrow);
        // Collapsed rows are measured too, so expanding never reflows.
        let mut collapsed = panel(vec![row(0), hidden(1)]);
        collapsed.rows[1].label = wide.rows[0].label.clone();
        let a = layout(&collapsed, &m).size.0;
        collapsed.show_hidden = true;
        assert_eq!(a, layout(&collapsed, &m).size.0);
    }

    #[test]
    fn buttons_and_close_resolve() {
        let p = panel(vec![row(0)]);
        let lay = layout(&p, &m);
        assert_eq!(hit(&p, &lay, lay.buttons[0].0.x + 5, lay.buttons[0].0.y + 5), Some(Action::Search));
        assert_eq!(hit(&p, &lay, lay.buttons[1].0.x + 5, lay.buttons[1].0.y + 5), Some(Action::OpenSite));
        assert_eq!(hit(&p, &lay, lay.close.x + 5, lay.close.y + 5), Some(Action::Close));
    }

    #[test]
    fn dead_space_is_dead() {
        let p = panel(vec![row(0)]);
        let lay = layout(&p, &m);
        // The rarity/level text block carries no controls.
        assert_eq!(hit(&p, &lay, lay.rarity_pos.0 + 2, lay.rarity_pos.1 - 4), None);
        // Nor does the strip to the right of a row's boxes.
        assert_eq!(hit(&p, &lay, lay.size.0 - 2, lay.rows[0].check.y + 2), None);
    }

    #[test]
    fn badge_and_score_text_stay_honest() {
        assert_eq!(badge_text(&TierBadge { kind: AffixKind::Prefix, tier: 9 }), "P9");
        assert_eq!(badge_text(&TierBadge { kind: AffixKind::Suffix, tier: 1 }), "S1");
        assert_eq!(badge_text(&TierBadge { kind: AffixKind::Other, tier: 1 }), "—");
        assert_eq!(score_text(0.75), "0.8");
    }
}

#[cfg(test)]
mod group_divider_tests {
    use super::*;

    fn grouped_row(label: &str, group: RowGroup, i: usize) -> StatRow {
        StatRow {
            label: label.into(),
            badge: None,
            score: None,
            min: Some(1.0),
            max: None,
            enabled: true,
            target: Some(Target::Stat(i)),
            hidden: false,
            group,
            note: None,
        }
    }

    fn panel_of(rows: Vec<StatRow>) -> Panel {
        Panel {
            header: ItemHeader {
                name: "T".into(),
                rarity: "Rare".into(),
                item_level: None,
                requires_level: None,
                base: None,
            },
            rows,
            ..Panel::default()
        }
    }

    #[test]
    fn a_hairline_separates_blocks_and_only_blocks() {
        let p = panel_of(vec![
            grouped_row("implicit A", RowGroup::Implicit, 0),
            grouped_row("implicit B", RowGroup::Implicit, 1),
            grouped_row("explicit C", RowGroup::Explicit, 2),
        ]);
        let lay = layout(&p, &|s| s.len() as i32 * 7);
        // One divider: between B and C, never between A and B.
        assert_eq!(lay.dividers.len(), 1);
        let b_bottom = lay.rows[1].check.y + lay.rows[1].check.h as i32;
        let c_top = lay.rows[2].check.y;
        assert!(lay.dividers[0] > b_bottom && lay.dividers[0] < c_top);

        // A single-block card draws no divider at all.
        let p = panel_of(vec![
            grouped_row("A", RowGroup::Explicit, 0),
            grouped_row("B", RowGroup::Explicit, 1),
        ]);
        assert!(layout(&p, &|s| s.len() as i32 * 7).dividers.is_empty());
    }
}
