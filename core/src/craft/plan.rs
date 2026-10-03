//! A plan: every applicable strategy costed, beside buying one.
//!
//! The headline model is the observed one when the caller has it, else the
//! uniform one. When both exist every strategy is costed under each and
//! both figures are kept side by side; nothing blends them. Buying is the
//! baseline: its price is the cheapest listings of a trade search the
//! caller ran, and a strategy dearer than that is marked not worth
//! crafting. Without a search the buy line says its price is unknown.

use super::model::{Model, UNIFORM_LABEL};
use super::rules::{floor, Action, Omen, PATCH};
use super::sim::{simulate, Costed, SimConfig};
use super::strategy::{library, Strategy, Target};
use super::types::{AffixKind, Candidate, ItemState, Lich, PoolView, Rarity, Source};

/// What the caller's trade search for the finished item found.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BuyQuote {
    /// The price of the cheapest listings.
    pub price: f64,
    /// How many listings that price rests on.
    pub listings: usize,
}

/// The baseline every strategy is compared with.
#[derive(Debug, Clone, PartialEq)]
pub struct BuyLine {
    pub price: Option<f64>,
    pub listings: usize,
    /// Why there is no price, as "unknown: <reason>".
    pub unknown: Option<String>,
}

/// One strategy in the ranking.
#[derive(Debug, Clone, PartialEq)]
pub struct Ranked {
    /// Under the headline model.
    pub costed: Costed,
    /// Under the uniform model, when the headline is the observed one.
    pub uniform: Option<Costed>,
    /// The headline cost of a finished item (counting the currency of runs
    /// that give up) is above the buy price.
    pub not_worth_crafting: bool,
    /// Each omen the strategy uses: what it does and, where the pools on
    /// this item say, how it moves the odds of its step.
    pub omens: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// Ranked by the cost of a finished item under the headline model
    /// (`Costed::per_finished`: the expected cost plus what the runs that
    /// gave up spent), cheapest first; strategies without a total come last
    /// in library order.
    pub strategies: Vec<Ranked>,
    /// The cheapest way to the finished item: a strategy's name, or "buy"
    /// when the buy price is below every crafting total. `None` when no
    /// figure exists at all.
    pub cheapest: Option<String>,
    pub buy: BuyLine,
    /// The model line every panel shows.
    pub model_line: String,
    /// The assumptions of the cheapest crafting strategy.
    pub assumptions: Vec<String>,
    /// Every reason a figure is missing, across the plan.
    pub unknowns: Vec<String>,
    /// The rulebook's patch pin.
    pub patch: &'static str,
}

/// The model line: which model heads the plan and what it rests on.
pub fn model_line(observed: Option<&Model>) -> String {
    match observed {
        Some(m) => format!(
            "{}, biased toward what sellers list; beside it the uniform model ({UNIFORM_LABEL})",
            m.label()
        ),
        None => format!("uniform model: PoE2 does not publish weights ({UNIFORM_LABEL})"),
    }
}

/// Costs every applicable strategy from `start` to `target`. `observed` is
/// the observed model when the sample for the item's class is large
/// enough; `buy` is the caller's trade search for the finished item.
pub fn plan(
    start: &ItemState,
    target: &Target,
    pool: &(dyn PoolView + Sync),
    observed: Option<&Model>,
    prices: &(dyn Fn(&str) -> Option<f64> + Sync),
    buy: Option<BuyQuote>,
    config: &SimConfig,
) -> Plan {
    let headline = observed.cloned().unwrap_or(Model::Uniform);
    let buy_line = match buy {
        Some(q) => BuyLine { price: Some(q.price), listings: q.listings, unknown: None },
        None => BuyLine {
            price: None,
            listings: 0,
            unknown: Some("unknown: no trade search for the finished item has run, so its price is unknown".to_string()),
        },
    };

    let mut strategies: Vec<Ranked> = library(start, target, pool, &headline)
        .into_iter()
        .filter(|s| !matches!(s, Strategy::Buy))
        .map(|s| blocker_route(s, start, target, pool, &headline, prices, config))
        .map(|s| {
            let costed = simulate(&s, start, target, pool, &headline, prices, config);
            let uniform = observed.map(|_| simulate(&s, start, target, pool, &Model::Uniform, prices, config));
            let not_worth_crafting = matches!((costed.per_finished, buy_line.price), (Some(e), Some(b)) if e > b);
            let omens = omen_notes(&costed, start, target, pool);
            Ranked { costed, uniform, not_worth_crafting, omens }
        })
        .collect();
    // A stable sort keeps library order among equal totals and among the
    // strategies with none.
    strategies.sort_by(|a, b| match (a.costed.per_finished, b.costed.per_finished) {
        (Some(x), Some(y)) => x.total_cmp(&y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });

    let best = strategies.iter().find(|r| r.costed.per_finished.is_some());
    let cheapest = match (best.and_then(|r| r.costed.per_finished.map(|e| (e, &r.costed.strategy))), buy_line.price) {
        (Some((e, _)), Some(b)) if b < e => Some("buy".to_string()),
        (Some((_, name)), _) => Some(name.clone()),
        // With no craft to compare, buying is the only way; with crafts
        // that all lack a total, nothing shows buying is cheaper.
        (None, Some(_)) if strategies.is_empty() => Some("buy".to_string()),
        (None, _) => None,
    };
    let assumptions = best.map(|r| r.costed.assumptions.clone()).unwrap_or_default();
    let mut unknowns: Vec<String> = Vec::new();
    let every = strategies.iter().flat_map(|r| r.costed.unknowns.iter().chain(r.uniform.iter().flat_map(|u| u.unknowns.iter())));
    for u in every.chain(buy_line.unknown.iter()) {
        if !unknowns.contains(u) {
            unknowns.push(u.clone());
        }
    }

    Plan {
        strategies,
        cheapest,
        buy: buy_line,
        model_line: model_line(observed),
        assumptions,
        unknowns,
        patch: PATCH,
    }
}

/// One line per omen `costed` uses: its effect and, where the item's pools
/// decide it, the chance its step gives the wanted outcome with the omen
/// and without, under the uniform model. Rates are read on the item as the
/// plan starts; a step later in the plan sees a fuller item, so the lines
/// say which pool they count.
pub fn omen_notes(costed: &Costed, start: &ItemState, target: &Target, pool: &(dyn PoolView + Sync)) -> Vec<String> {
    let mut seen: Vec<Omen> = Vec::new();
    let mut out = Vec::new();
    for step in &costed.steps {
        for omen in step.action.omens() {
            if seen.contains(omen) {
                continue;
            }
            seen.push(*omen);
            let Some(effect) = omen.effect() else { continue };
            let odds = omen_odds(*omen, &step.action, start, target, pool);
            out.push(match odds {
                Some(o) => format!("{}: {effect}; {o}", omen.name()),
                None => format!("{}: {effect}", omen.name()),
            });
        }
    }
    out
}

/// "1 in 12 instead of 1 in 30" for what `omen` changes on `action`.
fn omen_odds(omen: Omen, action: &Action, start: &ItemState, target: &Target, pool: &(dyn PoolView + Sync)) -> Option<String> {
    let rare = ItemState { rarity: Rarity::Rare, ..start.clone() };
    let missing = target.missing(&rare);
    let ratio = |with: (usize, usize), without: (usize, usize), what: &str| -> Option<String> {
        let (w1, n1) = with;
        let (w0, n0) = without;
        (w1 > 0 && n1 > 0 && n0 > 0).then(|| {
            let one_in = |w: usize, n: usize| if w == 0 { "never".to_string() } else { format!("1 in {:.0}", n as f64 / w as f64) };
            format!("{what} {} instead of {}", one_in(w1, n1), one_in(w0, n0))
        })
    };
    match omen {
        Omen::SinistralExaltation | Omen::DextralExaltation => {
            let side = omen.side()?;
            let floor = match action {
                Action::Orb { orb, grade, .. } => floor(*orb, *grade),
                _ => 0,
            };
            let on_side = pool.eligible(&rare, side, floor);
            let other = pool.eligible(&rare, side.other(), floor);
            let wanted = |cs: &[Candidate]| cs.iter().filter(|c| missing.iter().any(|w| w.accepts(c))).count();
            let w = wanted(&on_side);
            ratio((w, on_side.len()), (w + wanted(&other), on_side.len() + other.len()), "a wanted modifier per orb:")
        }
        Omen::SinistralNecromancy | Omen::DextralNecromancy | Omen::Blackblooded | Omen::Liege | Omen::Sovereign => {
            let side = action.omens().iter().find_map(|o| o.side());
            let lich = action.omens().iter().find_map(|o| o.lich());
            let (with_side, with_lich) = match omen.side() {
                Some(s) => (Some(s), lich),
                None => (side, omen.lich()),
            };
            let (without_side, without_lich) = if omen.side().is_some() { (None, lich) } else { (side, None) };
            let count = |s: Option<AffixKind>, l: Option<Lich>| {
                let cs = pool.desecrated(&rare, s, l, 0);
                (cs.iter().filter(|c| missing.iter().any(|w| w.accepts(c))).count(), cs.len())
            };
            ratio(count(with_side, with_lich), count(without_side, without_lich), "a wanted desecrated modifier per option:")
        }
        Omen::SinistralAnnulment | Omen::DextralAnnulment | Omen::SinistralErasure | Omen::DextralErasure
        | Omen::SinistralCrystallisation | Omen::DextralCrystallisation => {
            let side = omen.side()?;
            let removable: Vec<&crate::craft::types::ModOn> = start.mods.iter().filter(|m| m.source != Source::Fractured).collect();
            let on_side: Vec<&crate::craft::types::ModOn> = removable.iter().copied().filter(|m| m.kind == side).collect();
            let bad_side = on_side.iter().filter(|m| !target.wanted(m)).count();
            ratio((bad_side, on_side.len()), (bad_side, removable.len()), "removing an unwanted modifier:")
        }
        Omen::Light => {
            let removable = start.mods.iter().filter(|m| m.source != Source::Fractured).count();
            (start.desecrated_slot_used() && removable > 1)
                .then(|| format!("the desecrated modifier is removed for certain instead of 1 in {removable}"))
        }
        Omen::AbyssalEchoes => {
            let side = action.omens().iter().find_map(|o| o.side()).or_else(|| start.mods.iter().find(|m| m.unrevealed()).map(|m| m.kind));
            let lich = action.omens().iter().find_map(|o| o.lich());
            let cs = pool.desecrated(&rare, side, lich, 0);
            let (w, n) = (cs.iter().filter(|c| missing.iter().any(|w| w.accepts(c))).count(), cs.len());
            if w == 0 || n < 3 {
                return None;
            }
            // Three different options drawn from n: the chance none is wanted.
            let miss = (0..3).map(|k| (n - w).saturating_sub(k) as f64 / (n - k) as f64).product::<f64>();
            let once = 1.0 - miss;
            let twice = 1.0 - miss * miss;
            Some(format!("a wanted option in a reveal: {:.0}% instead of {:.0}%", twice * 100.0, once * 100.0))
        }
        Omen::GreaterExaltation => Some("two modifiers per orb instead of one".to_string()),
        Omen::Whittling => Some("the lowest-level modifier goes instead of a random one".to_string()),
        _ => None,
    }
}

/// Runs in the pilot that picks how a strategy handles a blocking side.
pub const PILOT_RUNS: usize = 1_000;

/// For a strategy that finishes with slams, the route (clear a blocking side
/// at once, or fill the other side first) that is cheaper per finished item
/// at these prices under the headline model, judged on a short pilot of
/// each with the plan's seed; the full costing then runs on that route. A
/// route with no total loses to one with a total; a tie keeps the
/// library's route.
fn blocker_route(
    s: Strategy,
    start: &ItemState,
    target: &Target,
    pool: &(dyn PoolView + Sync),
    model: &Model,
    prices: &(dyn Fn(&str) -> Option<f64> + Sync),
    config: &SimConfig,
) -> Strategy {
    let Some(other) = s.other_blocker_route() else { return s };
    let pilot = SimConfig { runs: config.runs.min(PILOT_RUNS), ..config.clone() };
    let cost = |x: &Strategy| simulate(x, start, target, pool, model, prices, &pilot).per_finished;
    match (cost(&s), cost(&other)) {
        (Some(a), Some(b)) if b < a => other,
        (None, Some(_)) => other,
        _ => s,
    }
}
