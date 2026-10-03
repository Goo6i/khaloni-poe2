//! Seeded simulation of a strategy until the target holds.
//!
//! Each run starts from the item as it is, asks the strategy for its next
//! action, applies it through the rules table (`rules::attempt`, never a
//! copy of a rule) and draws every random outcome from the pool by the
//! active model, until the target holds or the run gives up. Over the runs
//! that reached the target the simulation reports the expected cost, the
//! median and the 90th percentile; the share that did not is the give-up
//! rate, reported beside the cap that caused it, and the currency those
//! runs spent is counted in the cost of a finished item.
//!
//! A number the inputs cannot back never appears: a missing price, or a
//! rule the rulebook marks unknown, leaves the plan with no total and names
//! the reason, while the steps whose figures are known are still listed.
//!
//! Where a step is a plain geometric trial (one action repeated from an
//! equivalent item until one application completes the target), its
//! probability is computed exactly by walking every outcome of the action,
//! and the step's figure is the closed form cost / p. The simulated figure
//! is kept beside it so the two can be compared.

use std::cell::RefCell;
use std::collections::HashMap;

use super::model::Model;
use super::rules::{self, Action, Orb, Rule};
use super::strategy::{Next, Strategy, Target};
use super::types::{AffixKind, Candidate, Draw, EssenceOutcome, ItemState, Lich, ModOn, Outcome, PoolView, Source};

/// Runs per strategy unless the caller says otherwise.
pub const DEFAULT_RUNS: usize = 20_000;

/// The default seed; any fixed seed gives the same figures every time.
pub const DEFAULT_SEED: u64 = 0x5EED_C4AF_7000_0055;

/// The default give-up cap: ten times the median number of currency uses
/// of the runs that reached the target so far.
pub const DEFAULT_MEDIAN_TIMES: f64 = 10.0;

/// No run takes more currency uses than this, whatever the cap.
pub const HARD_CEILING: u32 = 2_000;

/// The median cap never cuts a run shorter than this many uses. A plan
/// whose runs often finish in one or two uses (an essence that lands at
/// once) would otherwise cap every other run at a handful of uses and call
/// the cap's own doing a give-up.
pub const MIN_CAP: u32 = 200;

/// The median cap starts once this many runs have reached the target;
/// before that only the hard ceiling applies.
const MEDIAN_WARM_UP: usize = 30;

/// When none of this many runs reached the target, the simulation stops
/// there and reports that no run did.
const HOPELESS_AFTER: usize = 500;

/// The most draws one exact outcome walk may try.
const WALK_LIMIT: usize = 200_000;

/// Shown whenever a desecration is costed under the observed model: the
/// listings the model counts never include desecrated modifiers.
pub const A_DESECRATED_UNOBSERVED: &str =
    "desecrated modifiers are not observed on listings; this step uses the uniform model";

/// When a run gives up.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Cap {
    /// After this many times the median number of currency uses of the
    /// runs that reached the target so far (never under [`MIN_CAP`] uses).
    /// The cap counts uses rather than cost so it is the same whether or
    /// not every price is known.
    MedianTimes(f64),
    /// After this many Exalted Orbs of any grade.
    Exalts(u32),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SimConfig {
    pub runs: usize,
    pub seed: u64,
    pub cap: Cap,
}

impl Default for SimConfig {
    fn default() -> SimConfig {
        SimConfig { runs: DEFAULT_RUNS, seed: DEFAULT_SEED, cap: Cap::MedianTimes(DEFAULT_MEDIAN_TIMES) }
    }
}

/// The figures of one action within a strategy, per run that reached the
/// target.
#[derive(Debug, Clone, PartialEq)]
pub struct StepStat {
    pub action: Action,
    /// The action as a plan names it ("Exalted Orb + Omen of Dextral
    /// Exaltation").
    pub name: String,
    /// For a geometric step, the chance one application completes the
    /// target; `None` for every other step.
    pub p_success: Option<f64>,
    /// Uses per run: 1 / p for a geometric step, the simulated mean for
    /// every other step.
    pub expected_tries: f64,
    /// The simulated mean number of uses per run.
    pub count: f64,
    /// The price of one use (the orb plus each omen); `None` when a price
    /// is missing.
    pub unit_price: Option<f64>,
    /// The step's figure: the closed form for a geometric step, the
    /// simulated mean cost otherwise. `None` when a price is missing.
    pub cost: Option<f64>,
    /// The simulated mean cost, kept beside a closed form.
    pub simulated_cost: Option<f64>,
    /// Whether `cost` is the closed form.
    pub closed_form: bool,
}

/// A strategy costed under one model.
#[derive(Debug, Clone, PartialEq)]
pub struct Costed {
    /// The strategy's library id ("orb-chain").
    pub id: &'static str,
    /// The strategy as a plan names it, with its variant.
    pub strategy: String,
    /// The sum of the step figures; `None` when any step lacks a figure,
    /// when a run needed a rule the rulebook cannot model, or when no run
    /// reached the target.
    pub expected: Option<f64>,
    /// The simulated mean over the runs that reached the target, beside
    /// `expected`, which uses the closed form where one exists.
    pub simulated_mean: Option<f64>,
    /// What one finished item costs when a run that gives up is followed
    /// by a fresh try: `expected` plus the currency the abandoned runs
    /// spent, per finished item. Plans are ranked and compared with buying
    /// by this figure, so a strategy that gives up often is not flattered
    /// by counting only its lucky runs.
    pub per_finished: Option<f64>,
    pub median: Option<f64>,
    pub p90: Option<f64>,
    /// The share of runs that gave up before the target held.
    pub give_up_rate: f64,
    /// How runs give up, worded for the panel.
    pub cap: String,
    pub runs: usize,
    pub reached: usize,
    pub steps: Vec<StepStat>,
    /// Every assumption the plan's actions rest on.
    pub assumptions: Vec<String>,
    /// Each reason a figure is missing, as "unknown: <reason>".
    pub unknowns: Vec<String>,
    /// The label of the model that produced the figures.
    pub model_label: String,
}

impl Costed {
    /// Currency per run by item ("Exalted Orb" 14.2, "Omen of Dextral
    /// Exaltation" 3.1), in the order the plan first uses them.
    pub fn currency(&self) -> Vec<(String, f64)> {
        let mut out: Vec<(String, f64)> = Vec::new();
        for step in &self.steps {
            for item in items(&step.action) {
                match out.iter_mut().find(|(name, _)| *name == item) {
                    Some((_, n)) => *n += step.expected_tries,
                    None => out.push((item, step.expected_tries)),
                }
            }
        }
        out
    }
}

/// The closed form of a geometric trial: each try costs `cost` and succeeds
/// with chance `p`, so the expected spend is cost / p. Infinite when the
/// try can never succeed.
pub fn closed_form_geometric(p: f64, cost: f64) -> f64 {
    if p > 0.0 {
        cost / p
    } else {
        f64::INFINITY
    }
}

/// The items one use of `action` consumes, by name: the currency, essence
/// or bone, then each omen.
pub fn items(action: &Action) -> Vec<String> {
    // The reveal at the Well of Souls costs nothing but its omens.
    if let Action::Reveal { omens } = action {
        return omens.iter().map(|o| o.name().to_string()).collect();
    }
    let bare = match action {
        Action::Orb { orb, grade, .. } => Action::Orb { orb: *orb, grade: *grade, omens: vec![] },
        Action::Essence { of, tier, .. } => Action::Essence { of: of.clone(), tier: *tier, omens: vec![] },
        Action::Bone { kind, grade, .. } => Action::Bone { kind: *kind, grade: *grade, omens: vec![] },
        Action::Recombinator { .. } => Action::Recombinator { omens: vec![] },
        other => other.clone(),
    };
    std::iter::once(bare.name()).chain(action.omens().iter().map(|o| o.name().to_string())).collect()
}

// ------------------------------------------------------------------ randomness

/// SplitMix64: small, fast and fully determined by its seed.
#[derive(Debug, Clone)]
struct SplitMix(u64);

impl SplitMix {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// The seeded draw of the simulation: `pick` weighs each candidate by the
/// active model.
struct SimDraw<'m> {
    rng: SplitMix,
    model: &'m Model,
    /// Weigh every candidate alike for the current action: set for a
    /// desecration, whose modifiers the observed model never sees.
    uniform: bool,
    /// The current action met a draw whose every candidate has observed
    /// weight 0.
    unobserved: bool,
}

impl Draw for SimDraw<'_> {
    fn below(&mut self, n: usize) -> usize {
        if n <= 1 {
            return 0;
        }
        ((self.rng.unit() * n as f64) as usize).min(n - 1)
    }

    fn pick(&mut self, candidates: &[Candidate]) -> Option<usize> {
        let weights: Vec<f64> =
            candidates.iter().map(|c| if self.uniform { 1.0 } else { self.model.weight(c) }).collect();
        let total: f64 = weights.iter().sum();
        if total <= 0.0 {
            self.unobserved |= !candidates.is_empty();
            return None;
        }
        let r = self.rng.unit() * total;
        let mut acc = 0.0;
        let mut last = None;
        for (i, w) in weights.iter().enumerate() {
            if *w <= 0.0 {
                continue;
            }
            acc += w;
            last = Some(i);
            if r < acc {
                return Some(i);
            }
        }
        last
    }
}

/// A draw that replays a fixed list of choices and notes the first choice
/// it was not given, so every outcome of an action can be walked.
struct Scripted<'a> {
    script: &'a [usize],
    pos: usize,
    model: &'a Model,
    /// The chances of each option at the first unscripted choice.
    branch: Option<Vec<f64>>,
}

impl Scripted<'_> {
    fn choose(&mut self, chances: impl FnOnce() -> Vec<f64>, default: usize) -> usize {
        let at = self.pos;
        self.pos += 1;
        if let Some(&c) = self.script.get(at) {
            return c;
        }
        if at == self.script.len() {
            self.branch = Some(chances());
        }
        default
    }
}

impl Draw for Scripted<'_> {
    fn below(&mut self, n: usize) -> usize {
        let n = n.max(1);
        self.choose(|| vec![1.0 / n as f64; n], 0).min(n - 1)
    }

    fn pick(&mut self, candidates: &[Candidate]) -> Option<usize> {
        let weights: Vec<f64> = candidates.iter().map(|c| self.model.weight(c)).collect();
        let total: f64 = weights.iter().sum();
        if total <= 0.0 {
            return None;
        }
        let first = weights.iter().position(|w| *w > 0.0).unwrap_or(0);
        let chosen = self.choose(|| weights.iter().map(|w| w / total).collect(), first);
        Some(chosen.min(candidates.len() - 1))
    }
}

/// Every outcome of one use of `action` on `state` with its probability
/// under `model`, found by walking each random choice the rule makes. Fails
/// when the walk would take more than a few hundred thousand draws.
pub fn outcomes(action: &Action, state: &ItemState, pool: &dyn PoolView, model: &Model) -> Result<Vec<(f64, Outcome)>, String> {
    let memo = Memo::new(pool);
    let all = walk(action, state, &memo, model)?;
    Ok(all
        .into_iter()
        .map(|(p, o)| {
            let o = match o {
                Outcome::Applied(s) => Outcome::Applied(memo.restore(s)),
                Outcome::Reveal(options) => Outcome::Reveal(options.into_iter().map(|s| memo.restore(s)).collect()),
                refused => refused,
            };
            (p, o)
        })
        .collect())
}

fn walk(action: &Action, state: &ItemState, pool: &dyn PoolView, model: &Model) -> Result<Vec<(f64, Outcome)>, String> {
    let mut out = Vec::new();
    let mut stack: Vec<(Vec<usize>, f64)> = vec![(Vec::new(), 1.0)];
    let mut tried = 0usize;
    while let Some((script, p)) = stack.pop() {
        tried += 1;
        if tried > WALK_LIMIT {
            return Err(format!("{} has too many outcomes to walk one by one", action.name()));
        }
        let mut draw = Scripted { script: &script, pos: 0, model, branch: None };
        let outcome = rules::attempt(action, state, pool, &mut draw);
        match draw.branch {
            Some(chances) => {
                for (i, c) in chances.into_iter().enumerate() {
                    if c > 0.0 {
                        let mut next = script.clone();
                        next.push(i);
                        stack.push((next, p * c));
                    }
                }
            }
            None => out.push((p, outcome)),
        }
    }
    Ok(out)
}

/// The chance one use of `action` on `state` leaves an item the target
/// holds on.
fn success_chance(action: &Action, state: &ItemState, target: &Target, pool: &dyn PoolView, model: &Model) -> Option<f64> {
    let all = walk(action, state, pool, model).ok()?;
    Some(all.iter().filter(|(_, o)| matches!(o, Outcome::Applied(s) if target.holds(s))).map(|(p, _)| p).sum())
}

// ------------------------------------------------------------------ pool memo

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PoolKey {
    desecrated: bool,
    kind: Option<AffixKind>,
    lich: Option<Lich>,
    floor: u32,
    item_level: u32,
    base: String,
    tags: Vec<String>,
}

/// The fields of a candidate the rules read while choosing (its id, family,
/// kind, tier and level); the rest is restored once the roll is made.
fn slim(c: &Candidate) -> Candidate {
    Candidate {
        entry_id: c.entry_id.clone(),
        family: c.family.clone(),
        groups: Vec::new(),
        kind: c.kind,
        tier: c.tier,
        required_level: c.required_level,
        adds_tags: Vec::new(),
        text: String::new(),
        desecrated: c.desecrated,
    }
}

/// Remembers what the pool answered. By the pool's contract an answer
/// depends only on the item's base, tags and item level, less the entries
/// whose group is already on the item; so the memo keeps each answer for
/// the item without its groups and takes the held groups out per question.
///
/// A roll clones every candidate it chooses among, so the memo hands the
/// rules slim candidates and [`Memo::restore`] puts the full modifier back
/// on the item after each roll. Between the draws of one roll (Greater
/// Exaltation, Alchemy) a slim modifier's groups and tags are read from the
/// full candidate by its id, so the pool's rules hold throughout.
struct Memo<'a> {
    inner: &'a dyn PoolView,
    pools: RefCell<HashMap<PoolKey, Vec<Candidate>>>,
    full: RefCell<HashMap<String, Candidate>>,
    essences: RefCell<HashMap<EssenceKey, EssenceOutcome>>,
}

/// An essence's name, then the base and its tags it is asked about.
type EssenceKey = (String, String, Vec<String>);

impl<'a> Memo<'a> {
    fn new(inner: &'a dyn PoolView) -> Memo<'a> {
        Memo {
            inner,
            pools: RefCell::new(HashMap::new()),
            full: RefCell::new(HashMap::new()),
            essences: RefCell::new(HashMap::new()),
        }
    }

    /// The item with no group held: one stand-in modifier carries the tags
    /// its modifiers add, so tag blocking still applies.
    fn without_groups(state: &ItemState, tags: Vec<String>) -> ItemState {
        let mut bare = state.clone();
        bare.mods.clear();
        if !tags.is_empty() {
            bare.mods.push(ModOn {
                entry_id: None,
                family: String::new(),
                groups: Vec::new(),
                kind: AffixKind::Prefix,
                tier: None,
                required_level: None,
                adds_tags: tags,
                source: Source::Random,
                text: String::new(),
            });
        }
        bare
    }

    fn ask(
        &self,
        state: &ItemState,
        desecrated: bool,
        kind: Option<AffixKind>,
        lich: Option<Lich>,
        floor: u32,
        fill: impl FnOnce(&ItemState) -> Vec<Candidate>,
    ) -> Vec<Candidate> {
        let (key, held) = {
            let full = self.full.borrow();
            let mut held: Vec<&str> = Vec::new();
            let mut tags: Vec<String> = Vec::new();
            for m in &state.mods {
                let known = m.entry_id.as_ref().and_then(|id| full.get(id));
                let groups = if m.groups.is_empty() { known.map_or(&m.groups, |c| &c.groups) } else { &m.groups };
                held.extend(groups.iter().map(String::as_str));
                let added = if m.adds_tags.is_empty() { known.map_or(&m.adds_tags, |c| &c.adds_tags) } else { &m.adds_tags };
                tags.extend(added.iter().cloned());
            }
            tags.sort_unstable();
            tags.dedup();
            let key = PoolKey { desecrated, kind, lich, floor, item_level: state.item_level, base: state.base.clone(), tags };
            if let Some(hit) = self.pools.borrow().get(&key) {
                return hit.iter().filter(|c| !c.groups.iter().any(|g| held.contains(&g.as_str()))).map(slim).collect();
            }
            let held: Vec<String> = held.into_iter().map(str::to_string).collect();
            (key, held)
        };
        let all = fill(&Memo::without_groups(state, key.tags.clone()));
        let out = all.iter().filter(|c| !c.groups.iter().any(|g| held.contains(g))).map(slim).collect();
        {
            let mut full = self.full.borrow_mut();
            for c in &all {
                full.entry(c.entry_id.clone()).or_insert_with(|| c.clone());
            }
        }
        self.pools.borrow_mut().insert(key, all);
        out
    }

    /// The item with every slim modifier made whole again.
    fn restore(&self, mut state: ItemState) -> ItemState {
        let full = self.full.borrow();
        for m in &mut state.mods {
            if m.groups.is_empty() && m.text.is_empty() {
                if let Some(c) = m.entry_id.as_ref().and_then(|id| full.get(id)) {
                    *m = c.to_mod(m.source);
                }
            }
        }
        state
    }
}

impl PoolView for Memo<'_> {
    fn eligible(&self, state: &ItemState, kind: AffixKind, floor: u32) -> Vec<Candidate> {
        self.ask(state, false, Some(kind), None, floor, |bare| self.inner.eligible(bare, kind, floor))
    }

    fn desecrated(&self, state: &ItemState, kind: Option<AffixKind>, lich: Option<Lich>, floor: u32) -> Vec<Candidate> {
        self.ask(state, true, kind, lich, floor, |bare| self.inner.desecrated(bare, kind, lich, floor))
    }

    fn essence(&self, essence: &str, state: &ItemState) -> EssenceOutcome {
        let key = (essence.to_string(), state.base.clone(), state.base_tags.clone());
        if let Some(hit) = self.essences.borrow().get(&key) {
            return hit.clone();
        }
        let value = self.inner.essence(essence, state);
        self.essences.borrow_mut().insert(key, value.clone());
        value
    }

    fn alloy(&self, alloy: &str, state: &ItemState) -> EssenceOutcome {
        // Alloy and essence names never meet, so they share one cache.
        let key = (alloy.to_string(), state.base.clone(), state.base_tags.clone());
        if let Some(hit) = self.essences.borrow().get(&key) {
            return hit.clone();
        }
        let value = self.inner.alloy(alloy, state);
        self.essences.borrow_mut().insert(key, value.clone());
        value
    }
}

// ------------------------------------------------------------------ simulation

/// Runs are split into this many chunks, each with its own seeded draw, run
/// side by side and merged in order: the figures depend on the seed and the
/// run count only, never on how many cores the machine has.
const CHUNKS: usize = 8;

/// One distinct step of a strategy: an action, and whether the strategy
/// repeats it as a geometric trial.
struct StepAcc {
    action: Action,
    retry: bool,
    unit: Option<f64>,
    /// Uses summed over the runs that reached the target.
    uses: u64,
    /// 1 / p summed over each entry into the step, over those runs.
    closed_tries: f64,
    /// Entries into the step over those runs.
    entries: u64,
    /// Whether every entry had a closed form.
    closed_ok: bool,
}

/// What one chunk of runs counted.
#[derive(Default)]
struct Tally {
    steps: Vec<StepAcc>,
    /// The cost of each run that reached the target.
    totals: Vec<f64>,
    /// The currency spent by the runs that gave up.
    abandoned: f64,
    /// Runs that reached the target, by their number of currency uses.
    hist: Vec<u32>,
    reached: usize,
    gave_up: usize,
    blocked: usize,
    runs: usize,
    unknowns: Vec<String>,
}

/// A fingerprint of the item for remembering exact chances.
fn fingerprint(state: &ItemState) -> String {
    let mut mods: Vec<String> = state
        .mods
        .iter()
        .map(|m| format!("{}|{:?}|{:?}|{:?}|{}", m.entry_id.as_deref().unwrap_or(&m.text), m.kind, m.source, m.tier, m.family))
        .collect();
    mods.sort_unstable();
    format!("{:?}|{}", state.rarity, mods.join(";"))
}

/// The value at `sorted[floor(n * q)]`, the percentile rule the figures
/// use throughout.
fn quantile(sorted: &[f64], q: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let i = ((sorted.len() as f64 * q) as usize).min(sorted.len() - 1);
    Some(sorted[i])
}

/// The median of the counts in a histogram of uses.
fn histogram_median(hist: &[u32], n: usize) -> u32 {
    let want = n / 2;
    let mut seen = 0usize;
    for (uses, count) in hist.iter().enumerate() {
        seen += *count as usize;
        if seen > want {
            return uses as u32;
        }
    }
    hist.len().saturating_sub(1) as u32
}

fn median_limit(hist: &[u32], reached: usize, k: f64) -> u32 {
    let median = histogram_median(hist, reached).max(1);
    ((median as f64 * k).ceil() as u32).clamp(MIN_CAP, HARD_CEILING)
}

fn push_unique(list: &mut Vec<String>, item: String) {
    if !list.contains(&item) {
        list.push(item);
    }
}

fn is_exalt(action: &Action) -> bool {
    matches!(action, Action::Orb { orb: Orb::Exalted, .. })
}

/// The seed of one chunk, spread from the plan's seed.
fn chunk_seed(seed: u64, chunk: usize) -> u64 {
    SplitMix(seed ^ (chunk as u64 + 1).wrapping_mul(0xD1B5_4A32_D192_ED03)).next_u64()
}

/// How one run ended.
enum End {
    Reached,
    GaveUp,
    /// A rule the rulebook cannot model, or one unavailable in 0.5.5.
    Blocked(String),
}

/// Runs `strategy` from `start` until `target` holds, `config.runs` times,
/// drawing by `model`. `prices` names the price of one item by its in-game
/// name ("Exalted Orb", "Omen of Dextral Exaltation"); a name it has no
/// price for leaves the plan without a total.
pub fn simulate(
    strategy: &Strategy,
    start: &ItemState,
    target: &Target,
    pool: &(dyn PoolView + Sync),
    model: &Model,
    prices: &(dyn Fn(&str) -> Option<f64> + Sync),
    config: &SimConfig,
) -> Costed {
    let observed = matches!(model, Model::Observed { .. });
    let mut unknowns: Vec<String> = Vec::new();
    if let Strategy::Buy = strategy {
        unknowns.push("unknown: buying is priced by a trade search for the finished item, not simulated".to_string());
    } else if observed {
        // A wanted tier the listings never showed has no observed chance at
        // all; that is a missing figure, not an impossible craft.
        let memo = Memo::new(pool);
        for want in target.missing(start) {
            if !strategy.grants(want, start, &memo) && !observed_somewhere(want, start, &memo, model) {
                unknowns.push(format!(
                    "unknown: {} at tier {} or better has not been seen on the observed listings of {}, so the observed model has no figure for it",
                    want.family, want.min_tier, start.class
                ));
            }
        }
    }

    let tallies: Vec<Tally> = if unknowns.is_empty() {
        let sizes: Vec<usize> = (0..CHUNKS).map(|c| config.runs / CHUNKS + usize::from(c < config.runs % CHUNKS)).collect();
        std::thread::scope(|scope| {
            let handles: Vec<_> = sizes
                .iter()
                .enumerate()
                .filter(|(_, n)| **n > 0)
                .map(|(c, n)| {
                    let (seed, runs) = (chunk_seed(config.seed, c), *n);
                    scope.spawn(move || run_chunk(strategy, start, target, pool, model, prices, config, seed, runs))
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap_or_else(|e| std::panic::resume_unwind(e))).collect()
        })
    } else {
        Vec::new()
    };

    // Merge the chunks in order, so the sums add up the same way each time.
    let mut all = Tally::default();
    for t in tallies {
        for s in t.steps {
            match all.steps.iter_mut().find(|a| a.action == s.action && a.retry == s.retry) {
                Some(a) => {
                    a.uses += s.uses;
                    a.closed_tries += s.closed_tries;
                    a.entries += s.entries;
                    a.closed_ok &= s.closed_ok;
                }
                None => all.steps.push(s),
            }
        }
        all.totals.extend(t.totals);
        all.abandoned += t.abandoned;
        if all.hist.len() < t.hist.len() {
            all.hist.resize(t.hist.len(), 0);
        }
        for (i, n) in t.hist.iter().enumerate() {
            all.hist[i] += n;
        }
        all.reached += t.reached;
        all.gave_up += t.gave_up;
        all.blocked += t.blocked;
        all.runs += t.runs;
        for u in t.unknowns {
            push_unique(&mut unknowns, u);
        }
    }
    let Tally { steps, mut totals, abandoned, hist, reached, gave_up, blocked, runs, .. } = all;

    let cap = match config.cap {
        Cap::Exalts(n) => format!("a run gives up after {n} Exalted Orbs"),
        Cap::MedianTimes(k) if reached >= MEDIAN_WARM_UP => {
            format!("a run gives up past {k}x the median number of currency uses ({} uses)", median_limit(&hist, reached, k))
        }
        Cap::MedianTimes(k) => format!("a run gives up past {k}x the median number of currency uses, or {HARD_CEILING} uses"),
    };
    if runs > 0 && reached == 0 && blocked == 0 {
        push_unique(&mut unknowns, format!("unknown: none of {runs} runs reached the target before giving up"));
    }

    let per = |x: f64| if reached > 0 { x / reached as f64 } else { 0.0 };
    let stats: Vec<StepStat> = steps
        .iter()
        .map(|s| {
            let count = per(s.uses as f64);
            let simulated_cost = s.unit.map(|u| u * count);
            let geometric = s.retry && s.closed_ok && s.entries > 0 && s.closed_tries > 0.0;
            let (p_success, expected_tries, cost) = if geometric {
                // p is the harmonic mean over entries, so cost / p per entry
                // times the entries per run is exactly the mean of cost / p.
                let p = s.entries as f64 / s.closed_tries;
                let entries = per(s.entries as f64);
                (Some(p), entries / p, s.unit.map(|u| closed_form_geometric(p, u) * entries))
            } else {
                (None, count, simulated_cost)
            };
            StepStat {
                name: s.action.name(),
                action: s.action.clone(),
                p_success,
                expected_tries,
                count,
                unit_price: s.unit,
                cost,
                simulated_cost,
                closed_form: geometric,
            }
        })
        .collect();

    let mut assumptions: Vec<String> = Vec::new();
    for s in &steps {
        if let Rule::Live(spec) = rules::rule(&s.action) {
            for a in spec.assumptions {
                push_unique(&mut assumptions, a.to_string());
            }
        }
        if observed && matches!(s.action, Action::Bone { .. } | Action::Reveal { .. }) {
            push_unique(&mut assumptions, A_DESECRATED_UNOBSERVED.to_string());
        }
    }

    let priced = stats.iter().all(|s| s.cost.is_some());
    let figures = reached > 0 && blocked == 0 && priced;
    totals.sort_by(f64::total_cmp);
    let expected = figures.then(|| stats.iter().filter_map(|s| s.cost).sum());
    let simulated_mean = figures.then(|| totals.iter().sum::<f64>() / totals.len() as f64);
    let per_finished = expected.map(|e| e + abandoned / reached as f64);
    let give_up_rate = if runs > 0 { gave_up as f64 / runs as f64 } else { 0.0 };

    Costed {
        id: strategy.id().name(),
        strategy: strategy.name(),
        expected,
        simulated_mean,
        per_finished,
        median: if figures { quantile(&totals, 0.5) } else { None },
        p90: if figures { quantile(&totals, 0.9) } else { None },
        give_up_rate,
        cap,
        runs,
        reached,
        steps: stats,
        assumptions,
        unknowns,
        model_label: model.label(),
    }
}

/// One chunk of `runs` runs with its own draw.
#[allow(clippy::too_many_arguments)]
fn run_chunk(
    strategy: &Strategy,
    start: &ItemState,
    target: &Target,
    pool: &dyn PoolView,
    model: &Model,
    prices: &dyn Fn(&str) -> Option<f64>,
    config: &SimConfig,
    seed: u64,
    runs: usize,
) -> Tally {
    let memo = Memo::new(pool);
    let observed = matches!(model, Model::Observed { .. });
    let mut draw = SimDraw { rng: SplitMix(seed), model, uniform: false, unobserved: false };
    let mut chances: HashMap<(usize, String), f64> = HashMap::new();
    let mut t = Tally::default();
    let hopeless_after = HOPELESS_AFTER.div_ceil(CHUNKS);

    for run in 0..runs {
        if run >= hopeless_after && t.reached == 0 {
            break;
        }
        t.runs += 1;
        let limit = match config.cap {
            Cap::MedianTimes(k) if t.reached >= MEDIAN_WARM_UP => median_limit(&t.hist, t.reached, k),
            _ => HARD_CEILING,
        };
        let mut state = start.clone();
        let mut uses_by_step: Vec<u64> = vec![0; t.steps.len()];
        let mut closed_by_step: Vec<(f64, u64, bool)> = vec![(0.0, 0, true); t.steps.len()];
        let (mut uses, mut exalts) = (0u32, 0u32);
        let mut in_retry: Option<usize> = None;

        let end = loop {
            if target.holds(&state) {
                break End::Reached;
            }
            if uses >= limit || matches!(config.cap, Cap::Exalts(n) if exalts >= n) {
                break End::GaveUp;
            }
            let (action, retry) = match strategy.next(&state, target) {
                Next::Act(a) => (a, false),
                Next::Retry(a) => (a, true),
                Next::Stop(_) => break End::GaveUp,
            };
            let at = match t.steps.iter().position(|s| s.action == action && s.retry == retry) {
                Some(i) => i,
                None => {
                    let mut unit = Some(0.0);
                    for item in items(&action) {
                        match prices(&item) {
                            Some(p) => unit = unit.map(|u| u + p),
                            None => {
                                unit = None;
                                push_unique(&mut t.unknowns, format!("unknown: no price for {item}"));
                            }
                        }
                    }
                    t.steps.push(StepAcc { action: action.clone(), retry, unit, uses: 0, closed_tries: 0.0, entries: 0, closed_ok: true });
                    uses_by_step.push(0);
                    closed_by_step.push((0.0, 0, true));
                    t.steps.len() - 1
                }
            };
            if retry && in_retry != Some(at) {
                let key = (at, fingerprint(&state));
                let p = match chances.get(&key) {
                    Some(p) => Some(*p),
                    None => {
                        let p = success_chance(&action, &state, target, &memo, model);
                        if let Some(p) = p {
                            chances.insert(key, p);
                        }
                        p
                    }
                };
                let slot = &mut closed_by_step[at];
                slot.1 += 1;
                match p {
                    Some(p) if p > 0.0 => slot.0 += 1.0 / p,
                    _ => slot.2 = false,
                }
            }
            in_retry = if retry { Some(at) } else { None };

            draw.uniform = observed && matches!(action, Action::Bone { .. } | Action::Reveal { .. });
            draw.unobserved = false;
            let outcome = rules::attempt(&action, &state, &memo, &mut draw);
            let next = match outcome {
                Outcome::Applied(next) => memo.restore(next),
                Outcome::Reveal(options) => {
                    let options = options.into_iter().map(|o| memo.restore(o)).collect();
                    memo.restore(choose_revealed(strategy, &action, options, target, &memo, &mut draw))
                }
                Outcome::Refused(reason) if reason.starts_with("unknown: ") || reason.starts_with("unavailable: ") => {
                    break End::Blocked(reason);
                }
                Outcome::Refused(_) if draw.unobserved => break End::Blocked(format!(
                    "unknown: none of the modifiers {} can roll here has been seen on the observed listings of {}, so the observed model has no figure for it",
                    action.name(),
                    state.class
                )),
                // The rule refused for a reason of this item (nothing can
                // roll, no room): the run cannot go on.
                Outcome::Refused(_) => break End::GaveUp,
            };
            state = next;
            uses += 1;
            if is_exalt(&action) {
                exalts += 1;
            }
            uses_by_step[at] += 1;
        };

        match end {
            End::Reached => {
                t.reached += 1;
                let mut total = 0.0;
                for (i, s) in t.steps.iter_mut().enumerate() {
                    let n = uses_by_step[i];
                    let (tries, entries, ok) = closed_by_step[i];
                    s.uses += n;
                    s.closed_tries += tries;
                    s.entries += entries;
                    s.closed_ok &= ok;
                    total += s.unit.unwrap_or(0.0) * n as f64;
                }
                t.totals.push(total);
                let at = uses as usize;
                if t.hist.len() <= at {
                    t.hist.resize(at + 1, 0);
                }
                t.hist[at] += 1;
            }
            End::GaveUp => {
                t.gave_up += 1;
                t.abandoned += t.steps.iter().zip(&uses_by_step).map(|(s, n)| s.unit.unwrap_or(0.0) * *n as f64).sum::<f64>();
            }
            End::Blocked(reason) => {
                t.blocked += 1;
                push_unique(&mut t.unknowns, reason);
            }
        }
    }
    t
}

/// Whether the observed model gives some acceptable random entry of `want`
/// a weight above 0 on this item.
fn observed_somewhere(want: &super::strategy::Want, state: &ItemState, pool: &dyn PoolView, model: &Model) -> bool {
    let mut bare = state.clone();
    bare.mods.clear();
    pool.eligible(&bare, want.kind, 0).iter().any(|c| want.accepts(c) && model.weight(c) > 0.0)
}

/// Picks one option of a desecration reveal: the first the strategy
/// accepts; when none is and the omen allows, the options are drawn once
/// more; failing that, the option that meets the most of the target.
fn choose_revealed(
    strategy: &Strategy,
    action: &Action,
    options: Vec<ItemState>,
    target: &Target,
    pool: &dyn PoolView,
    draw: &mut dyn Draw,
) -> ItemState {
    if let Some(i) = options.iter().position(|o| strategy.accept(o, target)) {
        return options[i].clone();
    }
    let mut options = options;
    if let Rule::Live(spec) = rules::rule(action) {
        if let (Some(reroll), Some(first)) = (spec.reroll, options.first()) {
            if let Outcome::Reveal(again) = reroll(action, first, pool, draw) {
                options = again;
            }
        }
    }
    let score = |o: &ItemState| target.wants.iter().filter(|w| o.mods.iter().any(|m| w.met_by(m))).count();
    let mut best = 0;
    for (i, o) in options.iter().enumerate() {
        if strategy.accept(o, target) {
            return o.clone();
        }
        if score(o) > score(&options[best]) {
            best = i;
        }
    }
    options.swap_remove(best)
}
