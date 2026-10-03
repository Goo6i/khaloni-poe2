//! The craft planner panel: the picker, ranked plans with their steps, and
//! the flip list. Pure model, layout and hit-testing in the shape of the
//! market panel: the renderer draws from THIS geometry and the click
//! handler resolves actions from THIS geometry. Coordinates are
//! panel-local logical pixels.
//!
//! Every figure comes from `core::craft`, which also decides whether there
//! is a figure at all. Where it has none the panel prints its reason
//! ("unknown: ...") in the figure's place and never fills one in. Money is
//! carried in Exalted Orbs, the unit the price table is read in, and shown
//! in the price panel's units (divine above the threshold, then chaos, then
//! exalted).
//!
//! Running the planner is the caller's: `Action::Plan` only switches the
//! panel to its waiting state, and the caller runs `core::craft::plan::plan`
//! on [`Panel::target`] and hands the result back through
//! [`Panel::set_plan`]. The flip list arrives the same way, as
//! [`FlipList`] rows the caller builds from its scan.
//!
//! A calibration or a scan sends trade searches, so neither runs on a
//! click: `Action::Calibrate` asks the caller for the request's cost, which
//! comes back as a [`Prompt`] through [`Panel::ask`]; the panel shows the
//! statement (or why the budget refuses it) with a Run button, and
//! `Action::Run` is the caller's to carry out.

use std::sync::Arc;

use khaloni_poe2_core::craft::data::{CraftData, Domain};
use khaloni_poe2_core::craft::plan::{model_line, Plan};
use khaloni_poe2_core::craft::rules::PATCH;
use khaloni_poe2_core::craft::sim::Costed;
use khaloni_poe2_core::craft::strategy::{Target, Want};
use khaloni_poe2_core::craft::types::{AffixKind, Candidate, EssenceOutcome, ItemState, Lich, PoolView, Rarity, Source};
use khaloni_poe2_core::ninja::Price;
use khaloni_poe2_core::value::{format_amount, pick_unit, Unit};

use crate::config::Rect;

const PAD: i32 = 12;
const TITLE_H: i32 = 30;
const CLOSE: i32 = 20;
const STATUS_H: i32 = 18;
const TAB_H: i32 = 22;
const TAB_PAD_X: i32 = 10;
const TAB_GAP: i32 = 6;
const ROW_H: i32 = 22;
const SMALL_H: i32 = 17;
const SECTION_H: i32 = 24;
const HEADING_H: i32 = 26;
const COL_GAP: i32 = 24;
const CHECK: i32 = 14;
/// The label starts this far into a picker row, past the checkbox.
const LABEL_X: i32 = CHECK + 8;
const ARROW_W: i32 = 18;
const TIER_TEXT_W: i32 = 36;
/// The minimum-tier box: an arrow, the tier, an arrow.
const TIER_W: i32 = ARROW_W + TIER_TEXT_W + ARROW_W;
/// A small line under a picker row: the chosen tier's rolls, or a section
/// heading.
const DETAIL_H: i32 = 18;
const BUTTON_H: i32 = 24;
const INDENT: i32 = 16;
const WIDTH_MIN: i32 = 560;
const WIDTH_MAX: i32 = 1100;
/// Plans and candidates per page. Paging, not scrolling: the panel has no
/// keyboard, and a long ranking with one row open would outgrow the screen.
pub const PAGE_ROWS: usize = 6;

/// Size of the small text relative to the body text `measure` speaks for.
/// The renderer draws small text at exactly this ratio.
pub const SMALL_RATIO: f32 = 14.0 / 18.0;

/// Views of the panel, in tab order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum View {
    #[default]
    Picker,
    Plans,
    Flips,
}

pub const TABS: [(View, &str); 3] = [(View::Picker, "Picker"), (View::Plans, "Plans"), (View::Flips, "Flips")];

// ------------------------------------------------------------------ money

/// What turns an amount in Exalted Orbs into the price panel's units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rates {
    pub exalted_per_divine: f64,
    pub chaos_per_divine: f64,
    /// Amounts at or above this many divines are shown in divines.
    pub divine_threshold: f64,
}

impl Rates {
    fn price(&self, exalted: f64) -> Price {
        if self.exalted_per_divine > 0.0 {
            let divine = exalted / self.exalted_per_divine;
            Price { divine, exalted, chaos: divine * self.chaos_per_divine }
        } else {
            // Without an exchange rate the amount stays in exalted.
            Price { divine: 0.0, exalted, chaos: 0.0 }
        }
    }

    /// The unit `exalted` is shown in.
    fn unit(&self, exalted: f64) -> Unit {
        pick_unit(&self.price(exalted), 1, self.divine_threshold.max(f64::MIN_POSITIVE)).0
    }

    /// The bare number of `exalted` in `unit`.
    fn number_in(&self, exalted: f64, unit: Unit) -> String {
        let p = self.price(exalted);
        format_amount(match unit {
            Unit::Divine => p.divine,
            Unit::Chaos => p.chaos,
            Unit::Exalted => p.exalted,
        })
    }

    /// "3.1 div", "40 chaos", "12 ex".
    pub fn money(&self, exalted: f64) -> String {
        let unit = self.unit(exalted);
        format!("{} {}", self.number_in(exalted, unit), unit.suffix())
    }
}

/// "2%", "under 1%" for a share above nothing that rounds to nothing.
fn percent(share: f64) -> String {
    let pct = share * 100.0;
    if pct > 0.0 && pct < 0.5 {
        "under 1%".to_string()
    } else {
        format!("{}%", pct.round() as i64)
    }
}

/// A count of uses: whole counts as they are, others rounded with a "~".
fn count_text(n: f64) -> String {
    if (n - n.round()).abs() < 1e-9 {
        format!("{}", n.round() as i64)
    } else if n >= 2.0 {
        format!("~{}", n.round() as i64)
    } else {
        format!("~{:.1}", n)
    }
}

// ------------------------------------------------------------------ picker

/// One family the base can roll, with the tiers the item level allows.
#[derive(Debug, Clone, PartialEq)]
pub struct FamilyRow {
    /// The mod database's family ("IncreasedLife").
    pub family: String,
    pub kind: AffixKind,
    /// The modifier with its numbers as "#" ("+# to maximum Life").
    pub label: String,
    /// The best tier this item level can roll, in trade-site numbering.
    pub best: u8,
    /// The family's worst tier on the base.
    pub worst: u8,
    /// The tier of this family on the item now.
    pub current: Option<u8>,
    /// Why the family cannot roll at this item level, when it cannot.
    pub unavailable: Option<String>,
    /// The mod groups of the family's entries: an item holds one modifier
    /// per group, so two families sharing one cannot both be wanted.
    pub groups: Vec<String>,
    /// Every tier of the family on this base, best first, with what it
    /// rolls: the minimum-tier box reads from these.
    pub tiers: Vec<TierInfo>,
    /// How the family reaches the item: rolled, desecrated, an essence or
    /// an Alloy.
    pub origin: Origin,
}

/// One tier of a family on the base.
#[derive(Debug, Clone, PartialEq)]
pub struct TierInfo {
    /// Trade-site numbering: 1 is the best.
    pub tier: u8,
    /// The tier's rolls with the words left out: "100-119", "5-8 / 12-15".
    pub rolls: String,
    pub required_level: u32,
    /// The essence that guarantees this tier on the base, if one does.
    pub essence: Option<String>,
    /// The Alloy that guarantees this tier on the base, if one does.
    pub alloy: Option<String>,
}

/// How a family can reach an item.
#[derive(Debug, Clone, PartialEq)]
pub enum Origin {
    /// Random rolls (orbs); `essence` or `alloy` when an essence or an
    /// Alloy can also guarantee it.
    Rolled { essence: bool, alloy: bool },
    /// Only a desecration adds it: a bone's reveal of this lich's modifiers.
    Desecrated(Option<Lich>),
    /// Only an essence adds it; random rolls never do.
    Essence,
    /// Only an Alloy adds it; random rolls never do.
    Alloy,
}

impl FamilyRow {
    /// The label with how the family reaches the item, as the picker draws
    /// it.
    pub fn shown(&self) -> String {
        match &self.origin {
            Origin::Rolled { essence, alloy } => {
                let mut out = self.label.clone();
                if *essence {
                    out.push_str(" · essence");
                }
                if *alloy {
                    out.push_str(" · alloy");
                }
                out
            }
            Origin::Essence | Origin::Alloy | Origin::Desecrated(None) => self.label.clone(),
            Origin::Desecrated(Some(l)) => format!("{}: {}", lich_name(*l), self.label),
        }
    }

    /// Which section the row belongs to: 0 rolled, 1 only an essence, 2
    /// only a desecration, 3 only an Alloy.
    pub fn section(&self) -> u8 {
        FamilyRow::section_of(&self.origin)
    }

    pub fn section_of(origin: &Origin) -> u8 {
        match origin {
            Origin::Rolled { .. } => 0,
            Origin::Essence => 1,
            Origin::Desecrated(_) => 2,
            Origin::Alloy => 3,
        }
    }

    /// Where the row's section sits in its column: rolled families, then
    /// those only an essence adds, then those only an Alloy adds (both take
    /// the one crafted slot), then those only a desecration adds.
    pub fn place(&self) -> u8 {
        match self.section() {
            3 => 2,
            2 => 3,
            other => other,
        }
    }

    /// Whether the row's section fills the one crafted slot: an essence's
    /// modifier and an Alloy's are both the item's crafted modifier.
    pub fn crafted_slot(&self) -> bool {
        matches!(self.section(), 1 | 3)
    }

    /// "100-119 · ilvl 54" for tier `tier`, with the essence or Alloy that
    /// grants it.
    pub fn tier_detail(&self, tier: u8) -> String {
        match self.tiers.iter().find(|t| t.tier == tier) {
            Some(t) => {
                let mut out = format!("{} · ilvl {}", t.rolls, t.required_level);
                for name in [&t.essence, &t.alloy].into_iter().flatten() {
                    out.push_str(&format!(" · {name}"));
                }
                out
            }
            None => String::new(),
        }
    }
}

fn lich_name(l: Lich) -> &'static str {
    match l {
        Lich::Ulaman => "Ulaman",
        Lich::Amanamu => "Amanamu",
        Lich::Kurgal => "Kurgal",
    }
}

/// A modifier's numbers without its words: every range and number in the
/// text, "+(100-119) to maximum Life" reads "100-119".
pub fn rolls_of(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '(' && chars.peek().is_some_and(|n| n.is_ascii_digit() || *n == '-') {
            let mut range = String::new();
            for n in chars.by_ref() {
                if n == ')' {
                    break;
                }
                range.push(n);
            }
            out.push(range);
        } else if c.is_ascii_digit() {
            let mut number = c.to_string();
            while let Some(n) = chars.peek().copied().filter(|n| n.is_ascii_digit() || *n == '.') {
                number.push(n);
                chars.next();
            }
            out.push(number);
        }
    }
    out.join(" / ")
}

/// The sub-headings of a picker column over the families only an essence,
/// only an Alloy or only a desecration adds.
pub const ESSENCE_SECTION: &str = "only from an essence";
pub const ALLOY_SECTION: &str = "only from an alloy";
pub const DESECRATED_SECTION: &str = "only from a desecration (bone reveal)";

/// The heading over an opened plan's omens.
pub const OMENS_HEADING: &str = "omens that raise the odds (rates on this item, uniform model)";
/// A target no strategy of the library reaches from this item.
pub const NO_STRATEGY: &str = "No strategy in the library reaches this target from this item: buying is the only way here. A family ticked above that only one kind of craft adds, beside others it cannot reach, is the usual reason.";

/// Shown beside a desecrated, essence-only or alloy-only family when
/// another family taking the same slot is ticked: since 0.5.0 an item holds
/// one desecrated modifier and one crafted modifier, and an essence's or an
/// Alloy's modifier is its crafted one.
pub const ONE_DESECRATED_NOTE: &str = "one desecrated modifier per item";
pub const ONE_CRAFTED_NOTE: &str = "one crafted (essence or alloy) modifier per item";

/// Shown beside a family that shares a mod group with a ticked one.
pub const SAME_GROUP_NOTE: &str = "same group as a ticked family";

/// The item and every family its base can roll.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Picker {
    /// "Normal Soldier Cuirass · item level 80".
    pub title: String,
    /// The item class ("Body Armour"), which a calibration gathers
    /// listings of; empty when no item has been read.
    pub class: String,
    pub rarity: Option<Rarity>,
    /// The item's modifiers as the price panel shows them, with their tier.
    pub mods: Vec<String>,
    /// Prefixes, then suffixes.
    pub families: Vec<FamilyRow>,
}

/// "P3" for a prefix tier, "S3" for a suffix one.
pub fn tier_text(kind: AffixKind, tier: u8) -> String {
    match kind {
        AffixKind::Prefix => format!("P{tier}"),
        AffixKind::Suffix => format!("S{tier}"),
    }
}

/// A modifier's text with every number and range as "#", as the trade
/// site names a stat: "+(85-99) to maximum Life" reads "+# to maximum Life".
pub fn family_label(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '(' && chars.peek().is_some_and(|n| n.is_ascii_digit() || *n == '-') {
            // A range: skip to its closing parenthesis.
            for n in chars.by_ref() {
                if n == ')' {
                    break;
                }
            }
            out.push('#');
        } else if c.is_ascii_digit() {
            while chars.peek().is_some_and(|n| n.is_ascii_digit() || *n == '.') {
                chars.next();
            }
            out.push('#');
        } else if c == '\n' {
            out.push_str(" / ");
        } else {
            out.push(c);
        }
    }
    out
}

fn rarity_word(r: Rarity) -> &'static str {
    match r {
        Rarity::Normal => "Normal",
        Rarity::Magic => "Magic",
        Rarity::Rare => "Rare",
        Rarity::Unique => "Unique",
    }
}

impl Picker {
    /// Every family the base can roll (by the first matching tag of each
    /// entry, as the pool decides), every desecrated family a bone can
    /// reveal on it and every modifier an essence or an Alloy guarantees on
    /// it, whatever
    /// the item level; a family whose every tier needs a higher item level
    /// is kept and says so. Families on the item are marked with their tier.
    pub fn build(data: &CraftData, state: &ItemState) -> Picker {
        let tags: Vec<&str> = state.base_tags.iter().map(String::as_str).collect();
        let mut families: Vec<FamilyRow> = Vec::new();
        let add = |families: &mut Vec<FamilyRow>, c: &Candidate, origin: Origin, essence: Option<String>, alloy: Option<String>| {
            // A family can reach the item more than one way (the essence's
            // and a lich's Global Defences are one family): each way is a
            // row of its own, in its own section.
            let section = FamilyRow::section_of(&origin);
            let at = match families.iter().position(|f| f.family == c.family && f.kind == c.kind && f.section() == section) {
                Some(i) => i,
                None => {
                    families.push(FamilyRow {
                        family: c.family.clone(),
                        kind: c.kind,
                        label: String::new(),
                        best: u8::MAX,
                        worst: 0,
                        current: None,
                        unavailable: None,
                        groups: Vec::new(),
                        tiers: Vec::new(),
                        origin,
                    });
                    families.len() - 1
                }
            };
            let f = &mut families[at];
            for g in &c.groups {
                if !f.groups.contains(g) {
                    f.groups.push(g.clone());
                }
            }
            match f.tiers.iter_mut().find(|t| t.tier == c.tier) {
                Some(t) => {
                    if t.essence.is_none() {
                        t.essence = essence;
                    }
                    if t.alloy.is_none() {
                        t.alloy = alloy;
                    }
                }
                None => f.tiers.push(TierInfo { tier: c.tier, rolls: rolls_of(&c.text), required_level: c.required_level, essence, alloy }),
            }
            // Every tier words the family alike; the best one names it.
            if f.tiers.iter().all(|t| t.tier >= c.tier) && !c.text.is_empty() {
                f.label = family_label(&c.text);
            }
        };

        // Random rolls: the item domain by the first matching tag.
        for e in data.entries() {
            if e.domain != Domain::Item || e.essence_only || !e.eligible(tags.iter().copied()) {
                continue;
            }
            add(&mut families, &data.candidate(e, &tags), Origin::Rolled { essence: false, alloy: false }, None, None);
        }
        // Desecrations: what a bone can reveal on this base at any level.
        let probe = ItemState { mods: Vec::new(), item_level: u32::MAX, rarity: Rarity::Rare, ..state.clone() };
        for kind in [AffixKind::Prefix, AffixKind::Suffix] {
            for c in data.desecrated(&probe, Some(kind), None, 0) {
                let rolled = families.iter().any(|f| f.family == c.family && f.kind == c.kind && matches!(f.origin, Origin::Rolled { .. }));
                if !rolled {
                    add(&mut families, &c, Origin::Desecrated(c.desecrated), None, None);
                }
            }
        }
        // Essences: the modifier each guarantees on this base.
        if let Some(base) = data.base(&state.base) {
            for essence in data.essences() {
                if let EssenceOutcome::Entry(c) = data.essence_outcome(&essence.name, base) {
                    let rolled = families.iter_mut().find(|f| f.family == c.family && f.kind == c.kind && f.section() == 0);
                    match rolled {
                        // An orb can roll it too: the rolled row says an
                        // essence also guarantees it, and names it per tier.
                        Some(f) => {
                            if let Origin::Rolled { essence, .. } = &mut f.origin {
                                *essence = true;
                            }
                            let origin = f.origin.clone();
                            add(&mut families, &c, origin, Some(essence.name.clone()), None);
                        }
                        // A family no orb rolls here is one only the essence adds.
                        None => add(&mut families, &c, Origin::Essence, Some(essence.name.clone()), None),
                    }
                }
            }
            // Alloys: the modifier each guarantees on this base, the same way.
            for alloy in data.alloys() {
                if let EssenceOutcome::Entry(c) = data.alloy_outcome(&alloy.name, base) {
                    let rolled = families.iter_mut().find(|f| f.family == c.family && f.kind == c.kind && f.section() == 0);
                    match rolled {
                        Some(f) => {
                            if let Origin::Rolled { alloy, .. } = &mut f.origin {
                                *alloy = true;
                            }
                            let origin = f.origin.clone();
                            add(&mut families, &c, origin, None, Some(alloy.name.clone()));
                        }
                        None => add(&mut families, &c, Origin::Alloy, None, Some(alloy.name.clone())),
                    }
                }
            }
        }

        for f in families.iter_mut() {
            f.tiers.sort_by_key(|t| t.tier);
            f.worst = f.tiers.iter().map(|t| t.tier).max().unwrap_or(1);
            f.best = f.tiers.iter().filter(|t| t.required_level <= state.item_level).map(|t| t.tier).min().unwrap_or(u8::MAX);
            if f.best == u8::MAX {
                let lowest = f.tiers.iter().map(|t| t.required_level).min().unwrap_or(0);
                f.best = f.worst;
                f.unavailable = Some(format!("needs item level {lowest}"));
            }
            if f.label.is_empty() {
                f.label = f.family.clone();
            }
            f.current = state.mods.iter().find(|m| m.family == f.family && m.kind == f.kind).and_then(|m| m.tier);
        }
        // Rolled families first, then essence-only, alloy-only and
        // desecrated, each alphabetical by the words, so "#% increased
        // Armour" and "+# to Armour" sort by what they are about, not by
        // their sign.
        let rank = FamilyRow::place;
        let words = |f: &FamilyRow| f.label.chars().filter(|c| c.is_alphabetic() || *c == ' ').collect::<String>().trim().to_lowercase();
        families.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| rank(a).cmp(&rank(b))).then_with(|| words(a).cmp(&words(b))));

        let mods = state
            .mods
            .iter()
            .map(|m| {
                if m.unrevealed() {
                    let side = if m.kind == AffixKind::Prefix { "prefix" } else { "suffix" };
                    return format!("unrevealed desecrated {side} (reveal it at the Well of Souls)");
                }
                let mut line = match m.tier {
                    Some(t) => format!("{}  {}", tier_text(m.kind, t), m.text),
                    None => m.text.clone(),
                };
                match m.source {
                    Source::Random => {}
                    Source::Crafted => line.push_str(" (crafted)"),
                    Source::Desecrated => line.push_str(" (desecrated)"),
                    Source::Fractured => line.push_str(" (fractured)"),
                }
                line
            })
            .collect();
        Picker {
            title: format!("{} {} · item level {}", rarity_word(state.rarity), state.base, state.item_level),
            class: state.class.clone(),
            rarity: Some(state.rarity),
            mods,
            families,
        }
    }
}

// ------------------------------------------------------------------ plans

/// One strategy of a plan, worded.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanRow {
    /// "orb-chain (Greater Regal and Exalted)".
    pub name: String,
    /// "expected 3.1 div · half under 2.2 · one in ten over 8 · gives up
    /// 2%", or `None` when the strategy has no total.
    pub figures: Option<String>,
    /// Its cost of a finished item is above the buy price.
    pub not_worth_crafting: bool,
    /// Under an observed headline: which sample the figures rest on, and
    /// the uniform model's figure beside them.
    pub second: Option<String>,
    /// Each reason a figure is missing, "unknown: ...".
    pub unknowns: Vec<String>,
    /// One line per step, with its figures.
    pub steps: Vec<String>,
    /// "currency: ~14 Exalted Orb, 3 Omen of Dextral Exaltation".
    pub currency: Option<String>,
    /// How a run gives up.
    pub cap: String,
    pub assumptions: Vec<String>,
    /// Each omen the plan uses: what it does and how it moves the odds.
    pub omens: Vec<String>,
}

/// A plan as the panel shows it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PlanView {
    /// "Buy one instead: cheapest 4.5 div (3 listings)", or its unknown.
    pub buy: String,
    /// "cheapest way to the finished item: orb-chain".
    pub cheapest: Option<String>,
    pub model: String,
    pub rows: Vec<PlanRow>,
    /// The assumptions of the cheapest crafting strategy.
    pub assumptions: Vec<String>,
}

/// Figures of a costed strategy: the cost of a finished item (the figure
/// plans are ranked and compared with buying by), its median and 90th
/// percentile in the same unit, and the give-up rate. `None` without a
/// total.
fn figures(c: &Costed, rates: &Rates) -> Option<String> {
    let total = c.per_finished?;
    let unit = rates.unit(total);
    let mut parts = vec![format!("expected {}", rates.money(total))];
    if let Some(m) = c.median {
        parts.push(format!("half under {}", rates.number_in(m, unit)));
    }
    if let Some(p) = c.p90 {
        parts.push(format!("one in ten over {}", rates.number_in(p, unit)));
    }
    parts.push(format!("gives up {}", percent(c.give_up_rate)));
    Some(parts.join(" · "))
}

/// The reason a step's cost is missing: the plan's own "no price for" line
/// for one of the items the step uses.
fn step_unknown(c: &Costed, step: &khaloni_poe2_core::craft::sim::StepStat) -> String {
    let items = khaloni_poe2_core::craft::sim::items(&step.action);
    c.unknowns
        .iter()
        .find(|u| items.iter().any(|i| u.ends_with(&format!("no price for {i}"))))
        .cloned()
        .unwrap_or_else(|| "unknown: a price this step needs is missing".to_string())
}

/// One line per step of a costed strategy: its uses per finished item, its
/// chance a try when it is a plain repeated trial, and its cost, or the
/// reason the cost is missing. With no run reaching the target there are no
/// per-step figures, so only the step's name is shown.
pub fn step_lines(c: &Costed, rates: &Rates) -> Vec<String> {
    c.steps
        .iter()
        .map(|s| {
            if c.reached == 0 {
                return s.name.clone();
            }
            let uses = s.expected_tries;
            let mut parts = vec![s.name.clone(), format!("{} {}", count_text(uses), if uses == 1.0 { "use" } else { "uses" })];
            if let Some(p) = s.p_success {
                parts.push(format!("{} a try", percent(p)));
            }
            parts.push(match s.cost {
                Some(cost) => rates.money(cost),
                None => step_unknown(c, s),
            });
            parts.join(" · ")
        })
        .collect()
}

/// "currency: ~14 Exalted Orb, 3 Omen of Dextral Exaltation", per run that
/// reached the target; `None` when no run did.
pub fn currency_line(c: &Costed) -> Option<String> {
    let list = c.currency();
    if c.reached == 0 || list.is_empty() {
        return None;
    }
    let items: Vec<String> = list.iter().map(|(name, n)| format!("{} {name}", count_text(*n))).collect();
    Some(format!("currency: {}", items.join(", ")))
}

impl PlanView {
    pub fn build(plan: &Plan, rates: &Rates) -> PlanView {
        let buy = match (plan.buy.price, &plan.buy.unknown) {
            (Some(price), _) => {
                let n = plan.buy.listings;
                format!("Buy one instead: cheapest {} ({n} {})", rates.money(price), if n == 1 { "listing" } else { "listings" })
            }
            (None, Some(reason)) => format!("Buy one instead: {reason}"),
            (None, None) => "Buy one instead: unknown: no price for the finished item".to_string(),
        };
        let rows = plan
            .strategies
            .iter()
            .map(|r| {
                let c = &r.costed;
                let second = r.uniform.as_ref().map(|u| {
                    let uniform = figures(u, rates)
                        .or_else(|| u.unknowns.first().cloned())
                        .unwrap_or_else(|| "unknown: no figure".to_string());
                    format!("{} · uniform model: {uniform}", c.model_label)
                });
                PlanRow {
                    name: c.strategy.clone(),
                    figures: figures(c, rates),
                    not_worth_crafting: r.not_worth_crafting,
                    second,
                    unknowns: c.unknowns.clone(),
                    steps: step_lines(c, rates),
                    currency: currency_line(c),
                    cap: c.cap.clone(),
                    assumptions: c.assumptions.clone(),
                    omens: r.omens.clone(),
                }
            })
            .collect();
        PlanView {
            buy,
            cheapest: {
                // A plan without a total could be cheaper than any named
                // here, so the claim covers only the costed ones.
                let lead = if plan.strategies.iter().any(|r| r.costed.per_finished.is_none()) {
                    "cheapest of the costed ways to the finished item"
                } else {
                    "cheapest way to the finished item"
                };
                match plan.cheapest.as_deref() {
                    Some("buy") => Some(format!("{lead}: buy one")),
                    Some(c) => Some(format!("{lead}: {c}")),
                    None if !plan.strategies.is_empty() => Some("no plan has a total, so the cheapest way is unknown".to_string()),
                    None => None,
                }
            },
            model: plan.model_line.clone(),
            rows,
            assumptions: plan.assumptions.clone(),
        }
    }
}

/// Where the plans view stands.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum PlanState {
    /// Nothing planned yet.
    #[default]
    None,
    /// The caller is running the planner.
    Planning,
    Ready(Arc<PlanView>),
}

// ------------------------------------------------------------------ flips

/// The cost of fixing a candidate into the profile, by its cheapest plan
/// under the uniform model. Amounts in Exalted Orbs.
#[derive(Debug, Clone, PartialEq)]
pub struct Fix {
    pub cost: f64,
    /// The strategy's id ("orb-chain").
    pub strategy: String,
    pub median: Option<f64>,
}

/// The finished item's cheapest listings at or above the profile.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Resale {
    pub price: f64,
    pub listings: usize,
}

/// The observed model's margin, beside the uniform one.
#[derive(Debug, Clone, PartialEq)]
pub struct ObservedMargin {
    pub margin: f64,
    /// "observed on 412 listings of Boots".
    pub label: String,
}

/// One candidate of a flip scan.
#[derive(Debug, Clone, PartialEq)]
pub struct FlipRow {
    /// Its asking price, in Exalted Orbs.
    pub listed: f64,
    pub seller: String,
    pub online: bool,
    /// The fix, or why it has no cost.
    pub fix: Result<Fix, String>,
    /// The resale price, or why there is none.
    pub resale: Result<Resale, String>,
    pub observed: Option<ObservedMargin>,
    /// The candidate's card: its name and modifier lines.
    pub card: Vec<String>,
    /// Its plan's steps, as [`step_lines`] words them.
    pub steps: Vec<String>,
    /// The resale listings the price rests on, one line each.
    pub resale_listings: Vec<String>,
    /// The candidate's own trade listing.
    pub url: String,
}

/// A reason with its "unknown: " prefix taken off, for a line that names
/// what is unknown itself.
fn bare_reason(reason: &str) -> &str {
    reason.strip_prefix("unknown: ").unwrap_or(reason)
}

impl FlipRow {
    /// Resale less the asking price less the fix, under the uniform model.
    pub fn margin(&self) -> Option<f64> {
        match (&self.fix, &self.resale) {
            (Ok(fix), Ok(resale)) => Some(resale.price - self.listed - fix.cost),
            _ => None,
        }
    }

    /// The candidate's line, in two parts: what it costs and what fixing
    /// it costs, then what it resells for and the margin.
    pub fn lines(&self, rates: &Rates) -> (String, String) {
        let presence = if self.online { "online" } else { "offline" };
        let fix = match &self.fix {
            Ok(f) => {
                let unit = rates.unit(f.cost);
                let money = rates.money(f.cost);
                match f.median {
                    Some(m) => format!("fix ~{money} ({}, half under {})", f.strategy, rates.number_in(m, unit)),
                    None => format!("fix ~{money} ({})", f.strategy),
                }
            }
            Err(reason) => format!("fix cost unknown: {}", bare_reason(reason)),
        };
        let first = format!("listed {} by {} ({presence}) · {fix}", rates.money(self.listed), self.seller);
        let mut second = match &self.resale {
            Ok(r) => format!(
                "resale cheapest {} ({} {})",
                rates.money(r.price),
                r.listings,
                if r.listings == 1 { "listing" } else { "listings" }
            ),
            Err(reason) => format!("resale unknown: {}", bare_reason(reason)),
        };
        if let Some(m) = self.margin() {
            second.push_str(&format!(" · margin {} under uniform", signed_money(rates, m)));
            if let Some(o) = &self.observed {
                second.push_str(&format!(" · {} {}", signed_money(rates, o.margin), o.label));
            }
        }
        (first, second)
    }
}

fn signed_money(rates: &Rates, x: f64) -> String {
    if x < 0.0 {
        format!("-{}", rates.money(-x))
    } else {
        rates.money(x)
    }
}

/// A flip scan as the panel shows it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FlipList {
    /// The profile scanned ("Boots: movement T2, lightning T3").
    pub profile: String,
    /// "this scan: 5 searches, 10 fetches; budget 27/30 free".
    pub scan: String,
    pub model: String,
    /// Ranked: see [`FlipList::new`].
    pub rows: Vec<FlipRow>,
}

/// The request cost of a scan, stated before it runs.
pub fn scan_text(searches: usize, fetches: usize, free: usize, total: usize) -> String {
    format!("this scan: {searches} searches, {fetches} fetches; budget {free}/{total} free")
}

impl FlipList {
    /// Ranks the candidates by margin under the uniform model, largest
    /// first; a candidate with no margin (its fix or resale unknown) comes
    /// after every one with a margin, in the order given.
    pub fn new(profile: String, scan: String, model: String, mut rows: Vec<FlipRow>) -> FlipList {
        rows.sort_by(|a, b| match (a.margin(), b.margin()) {
            (Some(x), Some(y)) => y.total_cmp(&x),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        });
        FlipList { profile, scan, model, rows }
    }
}

// ------------------------------------------------------------------ requests

/// A request that sends trade searches.
#[derive(Debug, Clone, PartialEq)]
pub enum Ask {
    /// Gather listings of the item's class for the observed model.
    Calibrate,
    /// Scan the flip profile of this name.
    Scan(String),
}

/// A request's cost, stated before it runs.
#[derive(Debug, Clone, PartialEq)]
pub struct Prompt {
    pub ask: Ask,
    /// "calibrate Body Armour: 5 searches, 20 fetches; budget 27/30 free".
    pub statement: String,
    /// Why the budget will not let it run now; `None` when it may.
    pub refused: Option<String>,
}

pub const CALIBRATE: &str = "Calibrate";
pub const RUN_CALIBRATION: &str = "Run calibration";
pub const RUN_SCAN: &str = "Run scan";
pub const CANCEL: &str = "Cancel";

// ------------------------------------------------------------------ panel

#[derive(Debug, Clone, PartialEq)]
pub struct Panel {
    pub view: View,
    pub picker: Arc<Picker>,
    /// Per family of the picker: the minimum tier when ticked.
    pub ticks: Vec<Option<u8>>,
    pub plan: PlanState,
    /// The plan row whose steps are shown.
    pub expanded: Option<usize>,
    pub flips: Option<Arc<FlipList>>,
    /// The candidate whose card, steps and listings are shown.
    pub selected: Option<usize>,
    /// The page of plans or candidates shown.
    pub page: usize,
    pub rates: Rates,
    /// Where the prices come from and how old they are.
    pub prices: String,
    /// The model line shown before any plan exists.
    pub model: String,
    /// The observed sample of the item's class: "observed model: 43 of 200
    /// listings of Body Armour recorded".
    pub observed: String,
    /// A calibration or scan waiting for Run, with its cost.
    pub prompt: Option<Prompt>,
    /// What the caller is doing now ("calibrating: ..."); while set no
    /// other request can be started.
    pub busy: Option<String>,
    /// The last request's outcome or why it failed.
    pub note: Option<String>,
}

impl Panel {
    /// Whether family `i` shares a mod group with another ticked family,
    /// which would make the target impossible.
    pub fn group_taken(&self, i: usize) -> bool {
        let Some(f) = self.picker.families.get(i) else { return false };
        self.picker.families.iter().enumerate().any(|(j, g)| {
            j != i && self.ticks.get(j).copied().flatten().is_some() && g.groups.iter().any(|x| f.groups.contains(x))
        })
    }

    /// Why family `i` cannot be ticked beside the ticked ones, if it cannot:
    /// its desecrated or crafted slot is already wanted by another family
    /// (essence-only and alloy-only families share the crafted one), or a
    /// ticked family shares its mod group.
    pub fn blocked(&self, i: usize) -> Option<&'static str> {
        let f = self.picker.families.get(i)?;
        let same_slot = |g: &FamilyRow| if f.crafted_slot() { g.crafted_slot() } else { g.section() == f.section() };
        let slot_taken = f.section() != 0
            && self.picker.families.iter().enumerate().any(|(j, g)| {
                j != i && same_slot(g) && self.ticks.get(j).copied().flatten().is_some()
            });
        if slot_taken {
            Some(if f.crafted_slot() { ONE_CRAFTED_NOTE } else { ONE_DESECRATED_NOTE })
        } else if self.group_taken(i) {
            Some(SAME_GROUP_NOTE)
        } else {
            None
        }
    }

    /// A panel for an item, its current families ticked at their tier.
    pub fn new(picker: Arc<Picker>, rates: Rates, prices: String) -> Panel {
        let ticks = picker.families.iter().map(|f| f.current.filter(|_| f.unavailable.is_none())).collect();
        Panel {
            view: View::Picker,
            picker,
            ticks,
            plan: PlanState::None,
            expanded: None,
            flips: None,
            selected: None,
            page: 0,
            rates,
            prices,
            model: model_line(None),
            observed: String::new(),
            prompt: None,
            busy: None,
            note: None,
        }
    }

    /// Shows a request's cost; its Run button carries it out.
    pub fn ask(&mut self, prompt: Prompt) {
        if matches!(prompt.ask, Ask::Scan(_)) {
            self.view = View::Flips;
            self.page = 0;
        }
        self.prompt = Some(prompt);
    }

    /// Whether a calibration can be asked for: an item was read and nothing
    /// else is running.
    pub fn can_calibrate(&self) -> bool {
        !self.picker.class.is_empty() && self.busy.is_none()
    }

    /// The finished item the ticks describe.
    pub fn target(&self) -> Target {
        let wants = self
            .picker
            .families
            .iter()
            .zip(&self.ticks)
            .filter_map(|(f, t)| t.map(|min_tier| Want { family: f.family.clone(), kind: f.kind, min_tier }))
            .collect();
        Target { wants }
    }

    pub fn set_plan(&mut self, view: PlanView) {
        self.plan = PlanState::Ready(Arc::new(view));
        self.expanded = None;
        self.view = View::Plans;
        self.page = 0;
    }

    pub fn set_flips(&mut self, list: FlipList) {
        self.flips = Some(Arc::new(list));
        self.selected = None;
        if self.view == View::Flips {
            self.page = 0;
        }
    }

    /// Rows of the view shown that page: plans or candidates.
    fn row_count(&self) -> usize {
        match (self.view, &self.plan, &self.flips) {
            (View::Plans, PlanState::Ready(v), _) => v.rows.len(),
            (View::Flips, _, Some(l)) => l.rows.len(),
            _ => 0,
        }
    }

    pub fn pages(&self) -> usize {
        self.row_count().div_ceil(PAGE_ROWS).max(1)
    }

    /// The indices of the rows on the page shown.
    fn page_range(&self) -> std::ops::Range<usize> {
        let start = self.page.min(self.pages() - 1) * PAGE_ROWS;
        start..(start + PAGE_ROWS).min(self.row_count())
    }

    /// The line under the title: the rulebook's patch pin, then the prices.
    pub fn status(&self) -> String {
        if self.prices.is_empty() {
            format!("rulebook {PATCH}")
        } else {
            format!("rulebook {PATCH} · {}", self.prices)
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Close,
    SetView(View),
    /// Tick or untick a family of the picker.
    Tick(usize),
    /// Raise a ticked family's minimum toward tier 1.
    TierBetter(usize),
    /// Let a ticked family's minimum fall a tier.
    TierWorse(usize),
    /// Plan for the ticked families; the caller runs the planner.
    Plan,
    ExpandRow(usize),
    SelectCandidate(usize),
    PrevPage,
    NextPage,
    /// Open this trade listing in the browser; the caller's.
    OpenSite(String),
    /// Ask for a calibration's cost; the caller answers with a prompt.
    Calibrate,
    /// Carry out the prompt shown; the caller's.
    Run,
    /// Drop the prompt shown.
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Body,
    Small,
    /// A block heading in the section gold.
    Section,
    /// The item's name.
    Heading,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ink {
    Normal,
    Dim,
    Gold,
    Magic,
    /// "not worth crafting".
    Warn,
    /// A reason a figure is missing.
    Unknown,
}

/// One drawn string. It is drawn at `rect.x` on the baseline
/// `rect.y + rect.h - 5`.
#[derive(Debug, Clone, PartialEq)]
pub struct Text {
    pub rect: Rect,
    pub text: String,
    pub style: Style,
    pub ink: Ink,
}

/// The minimum-tier box of a ticked family.
#[derive(Debug, Clone, PartialEq)]
pub struct TierBox {
    pub better: Rect,
    pub worse: Rect,
    /// "P3".
    pub text: String,
    pub text_x: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FamilyCell {
    pub index: usize,
    /// The part of the row that ticks: the checkbox and the label.
    pub toggle: Rect,
    pub check: Rect,
    pub ticked: bool,
    pub greyed: bool,
    pub tier: Option<TierBox>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Button {
    pub rect: Rect,
    pub label: String,
    pub action: Action,
    /// Live; a button that is off does nothing.
    pub on: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    /// The view this layout is of.
    pub view: View,
    pub w: i32,
    pub h: i32,
    pub close: Rect,
    pub title_pos: (i32, i32),
    pub status: Text,
    pub tabs: Vec<(Rect, View)>,
    pub texts: Vec<Text>,
    pub families: Vec<FamilyCell>,
    pub buttons: Vec<Button>,
    /// Clickable plan rows or candidates: the rect, the index, and whether
    /// it is the open one.
    pub rows: Vec<(Rect, usize, bool)>,
}

/// `text` cut with an ellipsis so it measures at most `max_w`.
fn clip_to_width(text: &str, max_w: i32, measure: &dyn Fn(&str) -> i32) -> String {
    if measure(text) <= max_w {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut keep = chars.len();
    while keep > 0 {
        keep -= 1;
        let cut: String = chars[..keep].iter().collect::<String>() + "…";
        if measure(&cut) <= max_w {
            return cut;
        }
    }
    String::new()
}

/// `text` broken at `sep` into lines that measure at most `max_w`, each
/// broken line keeping the separator's mark (", " leaves a comma); a
/// single piece wider than that is cut.
fn wrap_at(text: &str, sep: &str, max_w: i32, measure: &dyn Fn(&str) -> i32) -> Vec<String> {
    let mark = sep.trim_end();
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for piece in text.split(sep) {
        let candidate = if line.is_empty() { piece.to_string() } else { format!("{line}{sep}{piece}") };
        if measure(&format!("{candidate}{mark}")) <= max_w || line.is_empty() {
            line = candidate;
        } else {
            lines.push(format!("{}{mark}", std::mem::take(&mut line)));
            line = piece.to_string();
        }
    }
    lines.push(line);
    lines.into_iter().map(|l| clip_to_width(&l, max_w, measure)).collect()
}

/// Builds the body top-down: a cursor over the panel's inner width.
struct Body<'a> {
    y: i32,
    w: i32,
    texts: Vec<Text>,
    measure: &'a dyn Fn(&str) -> i32,
    small: &'a dyn Fn(&str) -> i32,
}

impl Body<'_> {
    fn inner(&self) -> i32 {
        self.w - 2 * PAD
    }

    fn metric(&self, style: Style) -> &dyn Fn(&str) -> i32 {
        match style {
            Style::Small => self.small,
            _ => self.measure,
        }
    }

    fn height(style: Style) -> i32 {
        match style {
            Style::Body => ROW_H,
            Style::Small => SMALL_H,
            Style::Section => SECTION_H,
            Style::Heading => HEADING_H,
        }
    }

    /// One line at `indent`, cut to the width.
    fn line(&mut self, text: &str, style: Style, ink: Ink, indent: i32) {
        let room = self.inner() - indent;
        let text = clip_to_width(text, room, self.metric(style));
        let h = Self::height(style);
        self.texts.push(Text { rect: Rect { x: PAD + indent, y: self.y, w: room.max(0) as u32, h: h as u32 }, text, style, ink });
        self.y += h;
    }

    /// A sentence, wrapped at spaces to the width.
    fn para(&mut self, text: &str, style: Style, ink: Ink, indent: i32) {
        self.para_at(text, " ", style, ink, indent);
    }

    /// A list, wrapped at `sep` so no item is split across lines.
    fn para_at(&mut self, text: &str, sep: &str, style: Style, ink: Ink, indent: i32) {
        let room = self.inner() - indent;
        for piece in wrap_at(text, sep, room, self.metric(style)) {
            self.line(&piece, style, ink, indent);
        }
    }

    fn gap(&mut self, px: i32) {
        self.y += px;
    }
}

/// The widest line of the view that is not a sentence: sentences wrap to
/// whatever width the rest needs.
fn content_width(p: &Panel, measure: &dyn Fn(&str) -> i32, small: &dyn Fn(&str) -> i32) -> i32 {
    let mut widths = vec![measure(&p.status())];
    match p.view {
        View::Picker => {
            let pk = &p.picker;
            widths.push(measure(&pk.title));
            widths.extend(pk.mods.iter().map(|m| measure(m)));
            let col = |kind: AffixKind| {
                pk.families
                    .iter()
                    .filter(|f| f.kind == kind)
                    .map(|f| {
                        let tail = match &f.unavailable {
                            Some(note) => small(note),
                            None => TIER_W.max(small(SAME_GROUP_NOTE)).max(small(ONE_CRAFTED_NOTE)),
                        };
                        LABEL_X + measure(&f.shown()) + 10 + tail
                    })
                    .max()
                    .unwrap_or(0)
            };
            let colw = col(AffixKind::Prefix).max(col(AffixKind::Suffix)).min((WIDTH_MAX - 2 * PAD - COL_GAP) / 2);
            widths.push(2 * colw + COL_GAP);
        }
        View::Plans => {
            if let PlanState::Ready(v) = &p.plan {
                widths.push(measure(&v.buy));
                for r in &v.rows {
                    let mark = if r.not_worth_crafting { 16 + measure(NOT_WORTH) } else { 0 };
                    widths.push(measure(&r.name) + mark);
                    widths.extend(r.figures.iter().map(|f| measure(f)));
                    widths.extend(r.steps.iter().map(|s| INDENT + small(s)));
                }
            }
        }
        View::Flips => {
            if let Some(list) = &p.flips {
                widths.push(measure(&list.profile));
                for r in &list.rows {
                    let (a, b) = r.lines(&p.rates);
                    widths.push(measure(&a).max(measure(&b)));
                }
            }
        }
    }
    widths.into_iter().max().unwrap_or(0)
}

pub const NOT_WORTH: &str = "not worth crafting";
pub const PLAN: &str = "Plan";
pub const OPEN_SITE: &str = "Open site";

pub fn layout(p: &Panel, measure: &dyn Fn(&str) -> i32) -> Layout {
    let small = |s: &str| (measure(s) as f32 * SMALL_RATIO).ceil() as i32;
    let small: &dyn Fn(&str) -> i32 = &small;
    let w = (content_width(p, measure, small) + 2 * PAD).clamp(WIDTH_MIN, WIDTH_MAX);

    let close = Rect { x: w - PAD - CLOSE, y: PAD, w: CLOSE as u32, h: CLOSE as u32 };
    let title_pos = (PAD, PAD + 18);
    let mut y = PAD + TITLE_H;
    let status = Text {
        rect: Rect { x: PAD, y, w: (w - 2 * PAD) as u32, h: STATUS_H as u32 },
        text: clip_to_width(&p.status(), w - 2 * PAD, small),
        style: Style::Small,
        ink: Ink::Dim,
    };
    y += STATUS_H + 6;
    let mut tabs = Vec::new();
    let mut x = PAD;
    for (view, label) in TABS {
        let tw = measure(label) + 2 * TAB_PAD_X;
        tabs.push((Rect { x, y, w: tw as u32, h: TAB_H as u32 }, view));
        x += tw + TAB_GAP;
    }
    y += TAB_H + 10;

    let mut body = Body { y, w, texts: Vec::new(), measure, small };
    let mut families = Vec::new();
    let mut buttons = Vec::new();
    let mut rows = Vec::new();
    if let Some(busy) = &p.busy {
        body.para(busy, Style::Small, Ink::Gold, 0);
    }
    if let Some(note) = &p.note {
        let ink = if note.contains("unknown: ") || note.contains("refused") { Ink::Unknown } else { Ink::Dim };
        body.para(note, Style::Small, ink, 0);
    }
    if p.busy.is_some() || p.note.is_some() {
        body.gap(6);
    }
    match p.view {
        View::Picker => picker_body(p, &mut body, &mut families, &mut buttons),
        View::Plans => plans_body(p, &mut body, &mut buttons, &mut rows),
        View::Flips => flips_body(p, &mut body, &mut buttons, &mut rows),
    }
    Layout {
        view: p.view,
        w,
        h: body.y + PAD,
        close,
        title_pos,
        status,
        tabs,
        texts: body.texts,
        families,
        buttons,
        rows,
    }
}

fn picker_body(p: &Panel, b: &mut Body, families: &mut Vec<FamilyCell>, buttons: &mut Vec<Button>) {
    let pk = &p.picker;
    let ink = match pk.rarity {
        Some(Rarity::Rare) => Ink::Gold,
        Some(Rarity::Magic) => Ink::Magic,
        _ => Ink::Normal,
    };
    b.line(&pk.title, Style::Heading, ink, 0);
    if pk.mods.is_empty() {
        b.line("no modifiers", Style::Body, Ink::Dim, 0);
    }
    for m in &pk.mods {
        b.line(m, Style::Body, Ink::Normal, 0);
    }
    b.gap(8);
    let any = p.ticks.iter().any(Option::is_some);
    let bw = (b.measure)(PLAN) + 28;
    buttons.push(Button {
        rect: Rect { x: PAD, y: b.y, w: bw as u32, h: BUTTON_H as u32 },
        label: PLAN.to_string(),
        action: Action::Plan,
        on: any,
    });
    let hint = if any {
        "minimum tier in trade-site numbering: P1 and S1 are the best"
    } else {
        "tick the modifiers the finished item needs"
    };
    let room = b.inner() - bw - 12;
    b.texts.push(Text {
        rect: Rect { x: PAD + bw + 12, y: b.y, w: room.max(0) as u32, h: BUTTON_H as u32 },
        text: clip_to_width(hint, room, b.small),
        style: Style::Small,
        ink: Ink::Dim,
    });
    b.y += BUTTON_H + 8;

    let colw = (b.inner() - COL_GAP) / 2;
    let top = b.y;
    let mut bottom = top;
    for (col, kind, title) in [(0, AffixKind::Prefix, "Prefixes"), (1, AffixKind::Suffix, "Suffixes")] {
        let x0 = PAD + col * (colw + COL_GAP);
        let mut y = top;
        b.texts.push(Text { rect: Rect { x: x0, y, w: colw as u32, h: SECTION_H as u32 }, text: title.to_string(), style: Style::Section, ink: Ink::Gold });
        y += SECTION_H;
        let mut section = 0;
        for (i, f) in pk.families.iter().enumerate().filter(|(_, f)| f.kind == kind) {
            if f.section() != section {
                section = f.section();
                let heading = match section {
                    1 => ESSENCE_SECTION,
                    3 => ALLOY_SECTION,
                    _ => DESECRATED_SECTION,
                };
                y += 4;
                b.texts.push(Text { rect: Rect { x: x0, y, w: colw as u32, h: DETAIL_H as u32 }, text: heading.to_string(), style: Style::Small, ink: Ink::Gold });
                y += DETAIL_H;
            }
            let ticked = p.ticks.get(i).copied().flatten();
            let taken = if ticked.is_none() && f.unavailable.is_none() { p.blocked(i) } else { None };
            let note = f.unavailable.clone().or_else(|| taken.map(str::to_string));
            let greyed = note.is_some();
            let check = Rect { x: x0, y: y + (ROW_H - CHECK) / 2, w: CHECK as u32, h: CHECK as u32 };
            let right = x0 + colw;
            let (label_end, tier) = match (&note, ticked) {
                (Some(note), _) => {
                    let nw = (b.small)(note);
                    b.texts.push(Text {
                        rect: Rect { x: right - nw, y, w: nw as u32, h: ROW_H as u32 },
                        text: note.clone(),
                        style: Style::Small,
                        ink: Ink::Dim,
                    });
                    (right - nw - 8, None)
                }
                (None, Some(t)) => {
                    let better = Rect { x: right - TIER_W, y: y + 2, w: ARROW_W as u32, h: (ROW_H - 4) as u32 };
                    let worse = Rect { x: right - ARROW_W, y: y + 2, w: ARROW_W as u32, h: (ROW_H - 4) as u32 };
                    let text = tier_text(f.kind, t);
                    let text_x = better.x + ARROW_W + (TIER_TEXT_W - (b.measure)(&text)) / 2;
                    (right - TIER_W - 8, Some(TierBox { better, worse, text, text_x }))
                }
                (None, None) => (right, None),
            };
            let label_x = x0 + LABEL_X;
            let room = label_end - label_x;
            b.texts.push(Text {
                rect: Rect { x: label_x, y, w: room.max(0) as u32, h: ROW_H as u32 },
                text: clip_to_width(&f.shown(), room, b.measure),
                style: Style::Body,
                ink: if greyed { Ink::Dim } else { Ink::Normal },
            });
            families.push(FamilyCell {
                index: i,
                toggle: Rect { x: x0, y, w: (label_end - x0).max(0) as u32, h: ROW_H as u32 },
                check,
                ticked: ticked.is_some(),
                greyed,
                tier,
            });
            y += ROW_H;
            // What the chosen minimum rolls, on a line of its own under the
            // row, so a tier is picked knowing what it means.
            if let (None, Some(t)) = (&note, ticked) {
                let detail = f.tier_detail(t);
                let room = right - (x0 + LABEL_X);
                b.texts.push(Text {
                    rect: Rect { x: x0 + LABEL_X, y, w: room.max(0) as u32, h: DETAIL_H as u32 },
                    text: clip_to_width(&format!("{}: {detail}", tier_text(f.kind, t)), room, b.small),
                    style: Style::Small,
                    ink: Ink::Dim,
                });
                y += DETAIL_H;
            }
        }
        bottom = bottom.max(y);
    }
    b.y = bottom + 10;
    b.para(&p.model, Style::Small, Ink::Dim, 0);
    calibration_block(p, b, buttons);
}

/// A row of buttons from the left edge.
fn button_row(b: &mut Body, buttons: &mut Vec<Button>, row: &[(&str, Action, bool)]) {
    let mut x = PAD;
    for (label, action, on) in row {
        let bw = (b.measure)(label) + 28;
        buttons.push(Button {
            rect: Rect { x, y: b.y, w: bw as u32, h: BUTTON_H as u32 },
            label: label.to_string(),
            action: action.clone(),
            on: *on,
        });
        x += bw + 10;
    }
    b.y += BUTTON_H + 6;
}

/// A prompt's statement, its refusal when there is one, and its Run and
/// Cancel buttons.
fn prompt_block(b: &mut Body, buttons: &mut Vec<Button>, prompt: &Prompt, run: &str, busy: bool) {
    b.para(&prompt.statement, Style::Body, Ink::Normal, 0);
    if let Some(why) = &prompt.refused {
        b.para(why, Style::Small, Ink::Unknown, 0);
    }
    b.gap(4);
    button_row(b, buttons, &[(run, Action::Run, prompt.refused.is_none() && !busy), (CANCEL, Action::Cancel, true)]);
}

/// The observed sample of the item's class and the Calibrate button, or the
/// calibration's stated cost while it waits for Run.
fn calibration_block(p: &Panel, b: &mut Body, buttons: &mut Vec<Button>) {
    if p.picker.class.is_empty() {
        return;
    }
    b.gap(4);
    // The model line above already names an observed headline and its
    // sample; the store's line repeats it then, so it shows only while the
    // sample is still short of the minimum.
    let shown = match &p.plan {
        PlanState::Ready(v) if p.view == View::Plans => v.model.clone(),
        _ => p.model.clone(),
    };
    if !p.observed.is_empty() && !shown.starts_with(p.observed.as_str()) {
        b.para(&p.observed, Style::Small, Ink::Dim, 0);
    }
    match &p.prompt {
        Some(prompt) if prompt.ask == Ask::Calibrate => prompt_block(b, buttons, prompt, RUN_CALIBRATION, p.busy.is_some()),
        _ => button_row(b, buttons, &[(CALIBRATE, Action::Calibrate, p.can_calibrate())]),
    }
}

/// "page 2 of 3" with its arrows, under the rows, when there is more than
/// one page.
fn pager(p: &Panel, b: &mut Body, buttons: &mut Vec<Button>) {
    let pages = p.pages();
    if pages < 2 {
        return;
    }
    let page = p.page.min(pages - 1);
    b.gap(4);
    buttons.push(Button { rect: Rect { x: PAD, y: b.y, w: 24, h: 20 }, label: "<".to_string(), action: Action::PrevPage, on: page > 0 });
    buttons.push(Button {
        rect: Rect { x: PAD + 30, y: b.y, w: 24, h: 20 },
        label: ">".to_string(),
        action: Action::NextPage,
        on: page + 1 < pages,
    });
    b.texts.push(Text {
        rect: Rect { x: PAD + 64, y: b.y, w: 200, h: 20 },
        text: page_text(page, pages, p.row_count()),
        style: Style::Small,
        ink: Ink::Dim,
    });
    b.y += 24;
}

/// "page 1 of 3 (14 plans)".
fn page_text(page: usize, pages: usize, rows: usize) -> String {
    format!("page {} of {pages} ({rows} in all)", page + 1)
}

fn plans_body(p: &Panel, b: &mut Body, buttons: &mut Vec<Button>, rows: &mut Vec<(Rect, usize, bool)>) {
    let v = match &p.plan {
        PlanState::None => {
            b.para("Tick the modifiers the finished item needs in Picker, then press Plan.", Style::Body, Ink::Dim, 0);
            b.gap(6);
            b.para(&p.model, Style::Small, Ink::Dim, 0);
            calibration_block(p, b, buttons);
            return;
        }
        PlanState::Planning => {
            b.para("Planning: costing every strategy for the ticked modifiers.", Style::Body, Ink::Dim, 0);
            b.gap(6);
            b.para(&p.model, Style::Small, Ink::Dim, 0);
            calibration_block(p, b, buttons);
            return;
        }
        PlanState::Ready(v) => v,
    };
    // The baseline first, above every craft.
    let buy_ink = if v.buy.contains("unknown: ") { Ink::Unknown } else { Ink::Normal };
    b.para(&v.buy, Style::Body, buy_ink, 0);
    if let Some(c) = &v.cheapest {
        b.line(c, Style::Small, Ink::Dim, 0);
    }
    b.para(&v.model, Style::Small, Ink::Dim, 0);
    calibration_block(p, b, buttons);
    b.gap(4);
    b.line("Plans", Style::Section, Ink::Gold, 0);
    if v.rows.is_empty() {
        b.para(NO_STRATEGY, Style::Body, Ink::Dim, 0);
    }
    for (i, r) in v.rows.iter().enumerate().take(p.page_range().end).skip(p.page_range().start) {
        let open = p.expanded == Some(i);
        let top = b.y;
        b.gap(2);
        let mark_w = if r.not_worth_crafting { (b.measure)(NOT_WORTH) } else { 0 };
        let name_room = b.inner() - if mark_w > 0 { mark_w + 16 } else { 0 };
        b.texts.push(Text {
            rect: Rect { x: PAD, y: b.y, w: name_room.max(0) as u32, h: ROW_H as u32 },
            text: clip_to_width(&r.name, name_room, b.measure),
            style: Style::Body,
            ink: Ink::Normal,
        });
        if r.not_worth_crafting {
            b.texts.push(Text {
                rect: Rect { x: b.w - PAD - mark_w, y: b.y, w: mark_w as u32, h: ROW_H as u32 },
                text: NOT_WORTH.to_string(),
                style: Style::Body,
                ink: Ink::Warn,
            });
        }
        b.y += ROW_H;
        if let Some(f) = &r.figures {
            b.line(f, Style::Body, Ink::Normal, INDENT);
        }
        if let Some(s) = &r.second {
            b.para(s, Style::Small, Ink::Dim, INDENT);
        }
        if !open && !r.omens.is_empty() {
            let names: Vec<&str> = r.omens.iter().filter_map(|o| o.split(':').next()).collect();
            b.para(&format!("omens: {}", names.join(", ")), Style::Small, Ink::Dim, INDENT);
        }
        for u in &r.unknowns {
            b.para(u, Style::Small, Ink::Unknown, INDENT);
        }
        if open {
            b.gap(2);
            for s in &r.steps {
                let ink = if s.contains("unknown: ") { Ink::Unknown } else { Ink::Normal };
                b.line(s, Style::Small, ink, 2 * INDENT);
            }
            if let Some(c) = &r.currency {
                b.para_at(c, ", ", Style::Small, Ink::Normal, 2 * INDENT);
            }
            b.para(&r.cap, Style::Small, Ink::Dim, 2 * INDENT);
            if !r.omens.is_empty() {
                b.gap(2);
                b.line(OMENS_HEADING, Style::Small, Ink::Gold, 2 * INDENT);
                for o in &r.omens {
                    b.para(o, Style::Small, Ink::Normal, 2 * INDENT);
                }
            }
        }
        b.gap(4);
        rows.push((Rect { x: PAD - 4, y: top, w: (b.inner() + 8) as u32, h: (b.y - top) as u32 }, i, open));
    }
    pager(p, b, buttons);
    if v.rows.is_empty() {
        return;
    }
    b.gap(6);
    let (title, list) = match p.expanded.and_then(|i| v.rows.get(i)) {
        Some(r) => (format!("Assumptions of {}", r.name), &r.assumptions),
        None => ("Assumptions of the cheapest plan".to_string(), &v.assumptions),
    };
    b.line(&title, Style::Section, Ink::Gold, 0);
    if list.is_empty() {
        b.line("none", Style::Small, Ink::Dim, 0);
    }
    for a in list {
        b.para(a, Style::Small, Ink::Dim, 0);
    }
}

fn flips_body(p: &Panel, b: &mut Body, buttons: &mut Vec<Button>, rows: &mut Vec<(Rect, usize, bool)>) {
    if let Some(prompt) = p.prompt.as_ref().filter(|pr| matches!(pr.ask, Ask::Scan(_))) {
        prompt_block(b, buttons, prompt, RUN_SCAN, p.busy.is_some());
        b.gap(6);
    }
    let Some(list) = &p.flips else {
        b.para("No flip scan yet: a scan runs on demand, one profile at a time.", Style::Body, Ink::Dim, 0);
        b.gap(6);
        b.para(&p.model, Style::Small, Ink::Dim, 0);
        return;
    };
    b.line(&list.profile, Style::Heading, Ink::Normal, 0);
    b.line(&list.scan, Style::Small, Ink::Dim, 0);
    b.para(&list.model, Style::Small, Ink::Dim, 0);
    b.gap(4);
    b.line("Candidates", Style::Section, Ink::Gold, 0);
    if list.rows.is_empty() {
        b.para("No candidate in this scan.", Style::Body, Ink::Dim, 0);
    }
    for (i, r) in list.rows.iter().enumerate().take(p.page_range().end).skip(p.page_range().start) {
        let open = p.selected == Some(i);
        let top = b.y;
        b.gap(2);
        let (first, second) = r.lines(&p.rates);
        let ink = |s: &str| if s.contains("unknown: ") { Ink::Unknown } else { Ink::Normal };
        b.line(&first, Style::Body, ink(&first), 0);
        b.line(&second, Style::Body, ink(&second), INDENT);
        if open {
            b.gap(4);
            for (n, line) in r.card.iter().enumerate() {
                b.line(line, if n == 0 { Style::Body } else { Style::Small }, Ink::Normal, INDENT);
            }
            b.gap(2);
            b.line("its plan", Style::Small, Ink::Gold, INDENT);
            for s in &r.steps {
                b.line(s, Style::Small, ink(s), 2 * INDENT);
            }
            b.line("resale listings", Style::Small, Ink::Gold, INDENT);
            if r.resale_listings.is_empty() {
                b.line("none at or above the profile", Style::Small, Ink::Dim, 2 * INDENT);
            }
            for l in &r.resale_listings {
                b.line(l, Style::Small, Ink::Normal, 2 * INDENT);
            }
            b.gap(4);
            let bw = (b.measure)(OPEN_SITE) + 28;
            buttons.push(Button {
                rect: Rect { x: PAD + INDENT, y: b.y, w: bw as u32, h: BUTTON_H as u32 },
                label: OPEN_SITE.to_string(),
                action: Action::OpenSite(r.url.clone()),
                on: !r.url.is_empty(),
            });
            b.y += BUTTON_H + 2;
        }
        b.gap(4);
        rows.push((Rect { x: PAD - 4, y: top, w: (b.inner() + 8) as u32, h: (b.y - top) as u32 }, i, open));
    }
    pager(p, b, buttons);
}

fn inside(r: &Rect, x: i32, y: i32) -> bool {
    x >= r.x && x < r.x + r.w as i32 && y >= r.y && y < r.y + r.h as i32
}

/// Click resolution; never mutates. `apply` carries the action out.
pub fn hit(lay: &Layout, x: i32, y: i32) -> Option<Action> {
    if inside(&lay.close, x, y) {
        return Some(Action::Close);
    }
    if let Some((_, view)) = lay.tabs.iter().find(|(r, _)| inside(r, x, y)) {
        return Some(Action::SetView(*view));
    }
    if let Some(b) = lay.buttons.iter().find(|b| inside(&b.rect, x, y)) {
        return b.on.then(|| b.action.clone());
    }
    for f in &lay.families {
        if let Some(t) = &f.tier {
            if inside(&t.better, x, y) {
                return Some(Action::TierBetter(f.index));
            }
            if inside(&t.worse, x, y) {
                return Some(Action::TierWorse(f.index));
            }
        }
        if !f.greyed && inside(&f.toggle, x, y) {
            return Some(Action::Tick(f.index));
        }
    }
    let (_, i, _) = lay.rows.iter().find(|(r, _, _)| inside(r, x, y))?;
    Some(match lay.view {
        View::Flips => Action::SelectCandidate(*i),
        _ => Action::ExpandRow(*i),
    })
}

/// Carries out everything but `Close`, `OpenSite`, `Calibrate` and `Run`,
/// which are the caller's. `Plan` switches to the plans view in its waiting
/// state; the caller then runs the planner. True when the panel changed.
pub fn apply(p: &mut Panel, action: &Action) -> bool {
    let before = p.clone();
    match action {
        Action::Close | Action::OpenSite(_) | Action::Calibrate | Action::Run => {}
        Action::Cancel => p.prompt = None,
        Action::SetView(v) => {
            if p.view != *v {
                p.view = *v;
                p.page = 0;
            }
        }
        Action::PrevPage | Action::NextPage => {
            let last = p.pages() - 1;
            let page = match action {
                Action::PrevPage => p.page.min(last).saturating_sub(1),
                _ => (p.page + 1).min(last),
            };
            // The open row belongs to the page being left.
            if page != p.page {
                p.page = page;
                p.expanded = None;
                p.selected = None;
            }
        }
        Action::Tick(i) => {
            let blocked = p.ticks.get(*i).copied().flatten().is_none() && p.blocked(*i).is_some();
            if let (Some(f), Some(t)) = (p.picker.families.get(*i), p.ticks.get_mut(*i)) {
                if f.unavailable.is_none() && !blocked {
                    *t = match t {
                        Some(_) => None,
                        // A family on the item keeps its tier; a new one
                        // starts at the best this item level can roll.
                        None => Some(f.current.unwrap_or(f.best).clamp(f.best, f.worst.max(f.best))),
                    };
                }
            }
        }
        Action::TierBetter(i) => {
            if let (Some(f), Some(Some(t))) = (p.picker.families.get(*i), p.ticks.get_mut(*i)) {
                *t = t.saturating_sub(1).max(f.best);
            }
        }
        Action::TierWorse(i) => {
            if let (Some(f), Some(Some(t))) = (p.picker.families.get(*i), p.ticks.get_mut(*i)) {
                *t = (*t + 1).min(f.worst.max(f.best));
            }
        }
        Action::Plan => {
            if p.ticks.iter().any(Option::is_some) {
                p.plan = PlanState::Planning;
                p.expanded = None;
                p.view = View::Plans;
                p.page = 0;
            }
        }
        Action::ExpandRow(i) => {
            let n = match &p.plan {
                PlanState::Ready(v) => v.rows.len(),
                _ => 0,
            };
            if *i < n {
                p.expanded = if p.expanded == Some(*i) { None } else { Some(*i) };
            }
        }
        Action::SelectCandidate(i) => {
            if p.flips.as_ref().is_some_and(|l| *i < l.rows.len()) {
                p.selected = if p.selected == Some(*i) { None } else { Some(*i) };
            }
        }
    }
    *p != before
}

/// Every string the panel can put on screen for `p`, for tests that hold
/// the wording to its rules.
pub fn all_text(p: &Panel, measure: &dyn Fn(&str) -> i32) -> Vec<String> {
    let lay = layout(p, measure);
    let mut out = vec!["Craft".to_string(), "x".to_string(), lay.status.text.clone()];
    out.extend(TABS.iter().map(|(_, l)| l.to_string()));
    out.extend(lay.texts.iter().map(|t| t.text.clone()));
    for f in &lay.families {
        if let Some(t) = &f.tier {
            out.extend(["<".to_string(), t.text.clone(), ">".to_string()]);
        }
    }
    out.extend(lay.buttons.iter().map(|b| b.label.clone()));
    out
}
