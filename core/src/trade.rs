//! Maps a parsed item mod's display text to the trade-site stat id it
//! corresponds to, using the `/api/trade2/data/stats` catalog (verified live
//! 2026-07-21; see `core/tests/fixtures/trade_stats.json` for a trimmed
//! recorded response).

use std::collections::{HashMap, HashSet};

use serde::Deserialize;

pub use crate::props::EquipKey;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TradeError {
    #[error("bad json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("http: {0}")]
    Http(String),
    #[error("rate limited; retry in {0:?}")]
    Cooldown(std::time::Duration),
    /// The API refused the request itself: a 4xx other than 429 (a want
    /// tag it does not know, a query too complex for it). The same request
    /// gets the same answer, so it is worth sending again only once what
    /// it was built from has changed.
    #[error("{0}")]
    Refused(String),
    /// The API answered, but not with the data asked for (an HTML challenge
    /// page, an empty catalog). Such a body is never cached or used.
    #[error("unusable {0} data: {1}")]
    BadData(&'static str, String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatEntry {
    pub id: String,
    pub text: String,
}

#[derive(Debug, Deserialize)]
struct StatsResponse {
    result: Vec<RawGroup>,
}

#[derive(Debug, Deserialize)]
struct RawGroup {
    id: String,
    entries: Vec<RawEntry>,
}

#[derive(Debug, Deserialize)]
struct RawEntry {
    id: String,
    text: String,
}

/// Group ids tried in this order when a mod text could plausibly appear in
/// more than one group; an item's own mods are always explicit or implicit,
/// with pseudo only ever relevant for aggregate ("total resistance") stats.
const GROUP_PRIORITY: [&str; 3] = ["explicit", "implicit", "pseudo"];

/// The suffix the catalog puts on a stat's second listing when the same
/// line is a gear piece's own armour, evasion, energy shield, accuracy,
/// attack speed or block ("# to Armour (Local)" beside "# to Armour"). The
/// item text never carries it, so a line resolves to both.
const LOCAL_SUFFIX: &str = " (Local)";

pub struct StatIndex {
    /// group id -> text key -> every entry listed under that text, in
    /// catalog order. The catalog lists some texts twice with different ids
    /// (two explicit "# to Spirit"), and the first is the primary.
    groups: HashMap<String, HashMap<String, Vec<StatEntry>>>,
    /// Every entry keyed by its stat id, for the lookups that start from an
    /// id rather than mod text (pseudo aggregates, which no single mod line
    /// spells out).
    by_id: HashMap<String, StatEntry>,
    /// Lines in the longest catalog text: how many consecutive item lines a
    /// lookup may need to join.
    max_lines: usize,
}

/// The lookup key for a catalog or mod text: each line trimmed, so the
/// stray padding the catalog carries around its line breaks ("Increases and
/// Reductions to\n Fire and ...", "Adds Abysses to a Map \n# use remaining")
/// never decides a match.
fn text_key(text: &str) -> String {
    text.lines().map(str::trim).collect::<Vec<_>>().join("\n")
}

impl StatIndex {
    pub fn from_json(s: &str) -> Result<StatIndex, TradeError> {
        let parsed: StatsResponse = serde_json::from_str(s)?;
        let mut groups = HashMap::new();
        let mut by_id = HashMap::new();
        let mut max_lines = 1;
        for g in parsed.result {
            let mut by_text: HashMap<String, Vec<StatEntry>> = HashMap::new();
            for e in g.entries {
                let entry = StatEntry {
                    id: e.id,
                    text: e.text,
                };
                max_lines = max_lines.max(entry.text.lines().count());
                by_id.insert(entry.id.clone(), entry.clone());
                // A repeated row (same id, same text) is one stat, not an
                // alternate of itself.
                let listed = by_text.entry(text_key(&entry.text)).or_default();
                if !listed.iter().any(|e| e.id == entry.id) {
                    listed.push(entry);
                }
            }
            groups.insert(g.id, by_text);
        }
        Ok(StatIndex { groups, by_id, max_lines })
    }

    /// Lines in the longest stat text the catalog holds; a mod that spans
    /// more item lines than this cannot be a single catalog stat.
    pub fn max_lines(&self) -> usize {
        self.max_lines
    }

    /// Looks a stat up by its trade id (e.g.
    /// "pseudo.pseudo_total_elemental_resistance"). `None` when the catalog
    /// this index was built from has no such stat, so a caller can never
    /// search an id the site does not know.
    pub fn entry_by_id(&self, id: &str) -> Option<&StatEntry> {
        self.by_id.get(id)
    }

    /// The entries one group lists under a stat text already in the
    /// catalog's own form (`#` for every number, e.g. a stats.ndjson
    /// matcher), primary first, then the "(Local)" twin as
    /// [`resolve_all`](Self::resolve_all) adds it. For a line whose type
    /// is known, where a text another group also lists must not decide the
    /// id. Empty when that group does not list the text.
    pub fn find_in(&self, group: &str, text: &str) -> Vec<&StatEntry> {
        let key = text_key(text);
        let Some(entries) = self.groups.get(group) else { return Vec::new() };
        let mut out: Vec<&StatEntry> = entries.get(&key).into_iter().flatten().collect();
        if !out.is_empty() {
            out.extend(entries.get(&format!("{key}{LOCAL_SUFFIX}")).into_iter().flatten());
        }
        out
    }

    /// Resolves a parsed item mod's raw text to its stat entry: strips the
    /// game's `[Tag|Display]` bracket syntax down to the display half, drops
    /// roll-annotation parentheticals like `(155-169)`, replaces every
    /// remaining number with `#`, then exact-matches against the catalog,
    /// preferring explicit, then implicit, then pseudo. The primary of
    /// [`resolve_all`](Self::resolve_all); a search needs all of them.
    pub fn resolve(&self, mod_text: &str) -> Option<&StatEntry> {
        self.resolve_all(mod_text).into_iter().next()
    }

    /// Every catalog entry a mod's text is indexed under, primary first:
    /// the entries whose text is the line (the catalog lists some texts
    /// under two ids), then the "(Local)" twin the site uses when the line
    /// is the gear's own armour, evasion, energy shield, accuracy, attack
    /// speed or block. The item text is identical in every case, so a
    /// search must accept any of them (see `Query::to_body`); verified live
    /// 2026-09-10, where the global armour id matched no body armour at all
    /// and the local one matched thousands. Multi-line text (the lines of
    /// one affix joined with `\n`) matches the catalog's multi-line stats.
    /// Groups are tried in `GROUP_PRIORITY` order and the first group that
    /// knows the text wins. Empty when no group does.
    pub fn resolve_all(&self, mod_text: &str) -> Vec<&StatEntry> {
        let key = text_key(&normalize_mod_text(mod_text));
        let local_key = format!("{key}{LOCAL_SUFFIX}");
        fn lookup<'e>(
            entries: &'e HashMap<String, Vec<StatEntry>>,
            key: &str,
            local_key: &str,
        ) -> Vec<&'e StatEntry> {
            let mut out: Vec<&StatEntry> = entries.get(key).into_iter().flatten().collect();
            if !out.is_empty() {
                out.extend(entries.get(local_key).into_iter().flatten());
            }
            out
        }
        for group_id in GROUP_PRIORITY {
            if let Some(entries) = self.groups.get(group_id) {
                let found = lookup(entries, &key, &local_key);
                if !found.is_empty() {
                    return found;
                }
            }
        }
        for (group_id, entries) in &self.groups {
            if GROUP_PRIORITY.contains(&group_id.as_str()) {
                continue;
            }
            let found = lookup(entries, &key, &local_key);
            if !found.is_empty() {
                return found;
            }
        }
        Vec::new()
    }
}

/// Keeps only the display half of `[Tag|Display]` (or the bare word of a
/// tagless `[Display]`), dropping the brackets.
fn strip_tag_brackets(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '[' {
            let mut inner = String::new();
            for c2 in chars.by_ref() {
                if c2 == ']' {
                    break;
                }
                inner.push(c2);
            }
            let display = inner.rsplit('|').next().unwrap_or(&inner);
            out.push_str(display);
        } else {
            out.push(c);
        }
    }
    out
}

/// Drops parenthesized roll ranges such as `(155-169)` or `(3.11-3.8)`
/// entirely: a parenthetical whose contents are only digits, `.`, and `-`.
/// Any other parenthetical (there should be none left in mod text at this
/// point) is left untouched.
fn strip_roll_parens(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '(' {
            let mut inner = String::new();
            let mut closed = false;
            for c2 in chars.by_ref() {
                if c2 == ')' {
                    closed = true;
                    break;
                }
                inner.push(c2);
            }
            let is_roll = closed
                && !inner.is_empty()
                && inner.chars().all(|ch| ch.is_ascii_digit() || ch == '.' || ch == '-');
            if !is_roll {
                out.push('(');
                out.push_str(&inner);
                if closed {
                    out.push(')');
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Replaces every integer or decimal number (optionally signed) with `#`,
/// dropping the sign; the catalog's own template text carries any `+` that
/// belongs in the display (e.g. pseudo's `+#% total to Cold Resistance`).
fn replace_numbers(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let sign_starts_number =
            (c == '+' || c == '-') && chars.get(i + 1).is_some_and(|n| n.is_ascii_digit());
        if c.is_ascii_digit() || sign_starts_number {
            let mut j = i + usize::from(sign_starts_number);
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
            if chars.get(j) == Some(&'.') && chars.get(j + 1).is_some_and(|d| d.is_ascii_digit()) {
                j += 1;
                while j < chars.len() && chars[j].is_ascii_digit() {
                    j += 1;
                }
            }
            out.push('#');
            i = j;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// A mod line as the stat catalog words it: source markers, `[Tag|Display]`
/// brackets and roll ranges dropped, every number a `#`.
pub fn normalize_mod_text(text: &str) -> String {
    let no_tags = strip_tag_brackets(crate::props::strip_mod_markers(text));
    let no_rolls = strip_roll_parens(&no_tags);
    replace_numbers(&no_rolls)
}

#[cfg(test)]
mod normalize_tests {
    use super::normalize_mod_text;

    #[test]
    fn strips_roll_annotation_and_numbers() {
        assert_eq!(
            normalize_mod_text("157(155-169)% increased Physical Damage"),
            "#% increased Physical Damage"
        );
    }

    #[test]
    fn drops_sign_on_replaced_number() {
        assert_eq!(normalize_mod_text("+31(31-33) to Dexterity"), "# to Dexterity");
    }

    #[test]
    fn strips_display_tag_brackets() {
        assert_eq!(
            normalize_mod_text("Adds 1 to 13 [Lightning|Lightning] Damage"),
            "Adds # to # Lightning Damage"
        );
    }

    #[test]
    fn handles_decimal_rolls() {
        assert_eq!(
            normalize_mod_text("+3.48(3.11-3.8)% to Critical Hit Chance"),
            "#% to Critical Hit Chance"
        );
    }
}

// --- rate limiting, search, fetch (verified live 2026-07-21; see the
// phase-4 plan's "Verified trade API facts" and the recorded fixtures) ---

type RequestLogger = Box<dyn Fn(&str) + Send + Sync>;

/// Where every request line goes; stderr until the process installs a
/// sink of its own.
static REQUEST_LOGGER: std::sync::RwLock<Option<RequestLogger>> = std::sync::RwLock::new(None);

/// Routes the request log somewhere other than stderr. Every request this
/// module sends writes one line before it leaves (endpoint, league, the
/// policy it is accounted under) and one when the answer is in (status and
/// the server's own counters per family and rule), and a 429 writes a
/// third naming the ban and the request that hit it. The owner was once
/// banned for half an hour with nothing to read afterwards; this is the
/// record.
pub fn set_request_logger(f: RequestLogger) {
    let mut slot = REQUEST_LOGGER.write().unwrap_or_else(|e| e.into_inner());
    *slot = Some(f);
}

fn log_request(line: &str) {
    let slot = REQUEST_LOGGER.read().unwrap_or_else(|e| e.into_inner());
    match slot.as_ref() {
        Some(f) => f(line),
        None => eprintln!("{line}"),
    }
}

/// The server's counters as one response reported them, for the log:
/// `policy=<name> ip 1/5 (10s), 3/15 (60s); account 2/45 (60s)`. Every
/// family the response names is read, with used/max per rule from the
/// family's `-state` and rules headers side by side.
fn describe_rate_headers(header: &dyn Fn(&str) -> Option<String>) -> String {
    let families: Vec<String> = match header("x-rate-limit-rules") {
        Some(v) => v.split(',').map(|f| f.trim().to_ascii_lowercase()).filter(|f| !f.is_empty()).collect(),
        None => ["ip", "account", "client"].map(String::from).to_vec(),
    };
    let mut parts: Vec<String> = Vec::new();
    if let Some(policy) = header("x-rate-limit-policy") {
        parts.push(format!("policy={}", policy.trim()));
    }
    let counted: Vec<String> = families
        .iter()
        .filter_map(|family| {
            let rules = RateLimiter::from_header(&header(&format!("x-rate-limit-{family}"))?).rules;
            let state = header(&format!("x-rate-limit-{family}-state")).unwrap_or_default();
            let used: Vec<&str> = state.split(',').map(|t| t.trim().split(':').next().unwrap_or("?")).collect();
            let cells: Vec<String> = rules
                .iter()
                .enumerate()
                .map(|(i, r)| format!("{}/{} ({}s)", used.get(i).copied().unwrap_or("?"), r.max, r.window_s))
                .collect();
            Some(format!("{family} {}", cells.join(", ")))
        })
        .collect();
    if !counted.is_empty() {
        parts.push(counted.join("; "));
    }
    if parts.is_empty() {
        return "no rate headers".to_string();
    }
    parts.join(" ")
}

/// Every family's `-state` header as sent, for the ban line: what the
/// server counted at the moment it refused.
fn describe_rate_states(header: &dyn Fn(&str) -> Option<String>) -> String {
    let states: Vec<String> = ["ip", "account", "client"]
        .iter()
        .filter_map(|f| header(&format!("x-rate-limit-{f}-state")).map(|s| format!("{f} {}", s.trim())))
        .collect();
    if states.is_empty() {
        "none".to_string()
    } else {
        states.join("; ")
    }
}

/// One `max:window_seconds:ban_seconds` rule from `x-rate-limit-ip`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateRule {
    pub max: u32,
    pub window_s: u32,
    pub ban_s: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RateDecision {
    Ready,
    Wait(std::time::Duration),
}

/// Client-side mirror of the server's sliding-window limits, driven by the
/// real response headers: `x-rate-limit-ip` declares the rules,
/// `x-rate-limit-ip-state` (`used:window:banned_for`) reports the server's
/// own view after every response and overrides local bookkeeping.
#[derive(Debug, Clone)]
pub struct RateLimiter {
    rules: Vec<RateRule>,
    /// Locally recorded request instants, pruned to the largest window.
    sent: Vec<std::time::Instant>,
    /// Per rule (same order as `rules`): requests the server counts in that
    /// rule's window that this limiter never sent - another client on the
    /// address, a run before a restart. They are stamped when learned and
    /// count toward THEIR rule only: the server saying "47 in the last six
    /// hours" says nothing about the last ten seconds, and counting those
    /// 47 against every rule put a fresh client into a five-minute cooldown
    /// on its first search (observed live 2026-09-19).
    seen_elsewhere: Vec<Vec<std::time::Instant>>,
    banned_until: Option<std::time::Instant>,
}

impl RateLimiter {
    pub fn from_header(h: &str) -> RateLimiter {
        let rules: Vec<RateRule> = h
            .split(',')
            .filter_map(|t| {
                let mut it = t.trim().split(':');
                Some(RateRule {
                    max: it.next()?.parse().ok()?,
                    window_s: it.next()?.parse().ok()?,
                    ban_s: it.next()?.parse().ok()?,
                })
            })
            .collect();
        let seen_elsewhere = vec![Vec::new(); rules.len()];
        RateLimiter { rules, sent: Vec::new(), seen_elsewhere, banned_until: None }
    }

    /// Replaces the rules from a fresh `x-rate-limit-ip` header while
    /// keeping the request history and any active ban. Every response
    /// carries the header, so rebuilding the limiter here (as the code once
    /// did) forgot every request the moment its response arrived, and the
    /// client-side burst rule never fired.
    pub fn set_rules(&mut self, h: &str) {
        let rules = RateLimiter::from_header(h).rules;
        // Same windows in the same order is the normal case and keeps the
        // per-rule history; a changed policy starts that part over.
        let same = rules.len() == self.rules.len()
            && rules.iter().zip(&self.rules).all(|(a, b)| a.window_s == b.window_s);
        if !same {
            self.seen_elsewhere = vec![Vec::new(); rules.len()];
        }
        self.rules = rules;
    }

    /// Requests counted against rule `i` right now: this limiter's own
    /// within the window, plus the ones the server reported for that rule.
    fn in_window(&self, i: usize, now: std::time::Instant) -> Vec<std::time::Instant> {
        let window = u64::from(self.rules[i].window_s);
        self.sent
            .iter()
            .chain(self.seen_elsewhere.get(i).into_iter().flatten())
            .filter(|t| now.duration_since(**t).as_secs() < window)
            .copied()
            .collect()
    }

    /// Applies a fresh `x-rate-limit-ip-state` header (`used:window:banned`
    /// per rule, in the rules' order): an active ban (third field nonzero)
    /// locks the limiter for that long, and a server-side `used` count above
    /// what this limiter holds for that rule tops that rule up, so requests
    /// the server has seen from this address (another client, a lost
    /// response) still count against the window they were reported for.
    pub fn apply_state(&mut self, state: &str) {
        let now = std::time::Instant::now();
        for (i, t) in state.split(',').enumerate() {
            let mut it = t.trim().split(':');
            let (Some(used), Some(_win), Some(ban)) = (it.next(), it.next(), it.next()) else {
                continue;
            };
            if let (Ok(used), true) = (used.parse::<usize>(), i < self.rules.len()) {
                let held = self.in_window(i, now).len();
                if let Some(extra) = self.seen_elsewhere.get_mut(i) {
                    extra.extend(std::iter::repeat_n(now, used.saturating_sub(held)));
                }
            }
            if let Ok(ban_s) = ban.parse::<u64>() {
                if ban_s > 0 {
                    let until = now + std::time::Duration::from_secs(ban_s);
                    self.banned_until = Some(match self.banned_until {
                        Some(b) if b > until => b,
                        _ => until,
                    });
                }
            }
        }
    }

    pub fn check(&mut self) -> RateDecision {
        let now = std::time::Instant::now();
        if let Some(until) = self.banned_until {
            if until > now {
                return RateDecision::Wait(until - now);
            }
            self.banned_until = None;
        }
        let max_window = self.rules.iter().map(|r| r.window_s).max().unwrap_or(0);
        self.sent
            .retain(|t| now.duration_since(*t).as_secs() < u64::from(max_window));
        for (i, r) in self.rules.iter().enumerate() {
            if let Some(extra) = self.seen_elsewhere.get_mut(i) {
                extra.retain(|t| now.duration_since(*t).as_secs() < u64::from(r.window_s));
            }
        }
        let mut wait = std::time::Duration::ZERO;
        for (i, r) in self.rules.iter().enumerate() {
            let mut counted = self.in_window(i, now);
            if counted.len() as u32 >= r.max {
                // Free again once enough of the oldest requests expire to
                // leave room for one more.
                counted.sort();
                let frees = counted[counted.len() - r.max as usize];
                let free_in = std::time::Duration::from_secs(u64::from(r.window_s))
                    .saturating_sub(now.duration_since(frees));
                wait = wait.max(free_in);
            }
        }
        if wait.is_zero() {
            RateDecision::Ready
        } else {
            RateDecision::Wait(wait)
        }
    }

    /// Records a request the caller is about to send.
    pub fn record(&mut self) {
        self.sent.push(std::time::Instant::now());
    }

    /// Per rule, in header order: how many requests count against it
    /// right now and the rule itself. The count is the server's last
    /// figure for the rule plus what left since, so it never reads under
    /// what the server will answer with.
    pub fn usage(&self) -> Vec<(u32, RateRule)> {
        let now = std::time::Instant::now();
        self.rules
            .iter()
            .enumerate()
            .map(|(i, r)| (u32::try_from(self.in_window(i, now).len()).unwrap_or(u32::MAX), *r))
            .collect()
    }

    /// Locks the limiter for at least `d` from now. A ban already running
    /// longer than that is kept.
    pub fn ban_for(&mut self, d: std::time::Duration) {
        let until = std::time::Instant::now() + d;
        self.banned_until = Some(match self.banned_until {
            Some(b) if b > until => b,
            _ => until,
        });
    }
}

/// The request families this process sends to pathofexile.com. Each starts
/// on seeded rules and is re-pointed at the server's own policy the moment
/// a response names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Endpoint {
    Search,
    Exchange,
    Fetch,
    /// `/api/trade2/data/*`: the stat, currency and item catalogs.
    Data,
    /// The legacy `get-stash-items` account endpoint.
    Stash,
}

impl Endpoint {
    /// The word the log and the panel use for the family.
    pub fn name(self) -> &'static str {
        match self {
            Endpoint::Search => "search",
            Endpoint::Exchange => "exchange",
            Endpoint::Fetch => "fetch",
            Endpoint::Data => "data",
            Endpoint::Stash => "stash",
        }
    }

    /// Registry key and `ip` rules used until the first response declares
    /// the real policy. Search rules were verified live 2026-07-21. The
    /// exchange is seeded with the search's key and rules: its policy name
    /// is unverified, and if the site counts exchanges under the search
    /// policy, treating them as separate budgets would spend the search's
    /// slots twice over. Sharing is the safe reading, and the registry
    /// keeps the exchange on whatever the search is bound to until an
    /// exchange response names a policy of its own (see
    /// `LimiterRegistry::key`). The data and stash seeds are deliberately
    /// tight guesses, since nothing has been observed for them yet.
    fn seed(self) -> (&'static str, &'static str) {
        match self {
            Endpoint::Search | Endpoint::Exchange => ("seed:search", "5:10:60,15:60:300,30:300:1800"),
            Endpoint::Fetch => ("seed:fetch", "12:4:10,16:12:300"),
            Endpoint::Data => ("seed:data", "3:6:60"),
            Endpoint::Stash => ("seed:stash", "20:60:60"),
        }
    }

    const ALL: [Endpoint; 5] =
        [Endpoint::Search, Endpoint::Exchange, Endpoint::Fetch, Endpoint::Data, Endpoint::Stash];

    /// Shortest lock applied after a 429 that names no longer one: the ban
    /// field of the endpoint's tightest rule.
    fn default_ban(self) -> std::time::Duration {
        std::time::Duration::from_secs(match self {
            Endpoint::Fetch => 10,
            _ => 60,
        })
    }
}

/// Every rule family of one server policy. `x-rate-limit-rules` names the
/// families that apply ("Ip", "Account", "Client"); each has its own
/// `x-rate-limit-{family}` rules and `-state` header, and a request may
/// leave only when all of them have room.
#[derive(Debug, Clone)]
pub struct PolicyLimiter {
    /// Lowercased family name -> its limiter, in the order first seen.
    families: Vec<(String, RateLimiter)>,
    /// A lock the server imposed with a 429 rather than a state header.
    banned_until: Option<std::time::Instant>,
}

/// The account family counts only requests that carry the session cookie.
const ACCOUNT_FAMILY: &str = "account";

impl PolicyLimiter {
    pub fn seeded(ip_rules: &str) -> PolicyLimiter {
        PolicyLimiter {
            families: vec![("ip".to_string(), RateLimiter::from_header(ip_rules))],
            banned_until: None,
        }
    }

    fn applies(family: &str, authed: bool) -> bool {
        authed || family != ACCOUNT_FAMILY
    }

    /// `Ready` only when every family that counts this request is ready;
    /// otherwise the longest of their waits.
    pub fn check(&mut self, authed: bool) -> RateDecision {
        let now = std::time::Instant::now();
        let mut wait = std::time::Duration::ZERO;
        match self.banned_until {
            Some(until) if until > now => wait = until - now,
            _ => self.banned_until = None,
        }
        for (name, limiter) in &mut self.families {
            if !Self::applies(name, authed) {
                continue;
            }
            if let RateDecision::Wait(d) = limiter.check() {
                wait = wait.max(d);
            }
        }
        if wait.is_zero() {
            RateDecision::Ready
        } else {
            RateDecision::Wait(wait)
        }
    }

    pub fn record(&mut self, authed: bool) {
        for (name, limiter) in &mut self.families {
            if Self::applies(name, authed) {
                limiter.record();
            }
        }
    }

    /// Takes in one response's rate headers, read through `header` (a
    /// lowercase header name -> its value). Families the response names are
    /// created or refreshed; families it no longer names are dropped, except
    /// the account family after an anonymous request, which the server
    /// leaves out only because that request did not count against it.
    pub fn absorb(&mut self, authed: bool, header: &dyn Fn(&str) -> Option<String>) {
        let named: Option<Vec<String>> = header("x-rate-limit-rules").map(|v| {
            v.split(',').map(|f| f.trim().to_ascii_lowercase()).filter(|f| !f.is_empty()).collect()
        });
        let candidates: Vec<String> = match &named {
            Some(n) => n.clone(),
            None => ["ip", "account", "client"].map(String::from).to_vec(),
        };
        for family in &candidates {
            let Some(rules) = header(&format!("x-rate-limit-{family}")) else { continue };
            let at = match self.families.iter().position(|(n, _)| n == family) {
                Some(at) => {
                    self.families[at].1.set_rules(&rules);
                    at
                }
                None => {
                    self.families.push((family.clone(), RateLimiter::from_header(&rules)));
                    self.families.len() - 1
                }
            };
            if let Some(state) = header(&format!("x-rate-limit-{family}-state")) {
                self.families[at].1.apply_state(&state);
            }
        }
        if let Some(named) = named.filter(|n| !n.is_empty()) {
            self.families
                .retain(|(n, _)| named.contains(n) || (n == ACCOUNT_FAMILY && !authed));
        }
    }

    /// Locks the whole policy for at least `d` from now.
    pub fn ban_for(&mut self, d: std::time::Duration) {
        let until = std::time::Instant::now() + d;
        self.banned_until = Some(match self.banned_until {
            Some(b) if b > until => b,
            _ => until,
        });
    }

    /// Feeds one family a `-state` header directly.
    pub fn apply_state(&mut self, family: &str, state: &str) {
        if let Some((_, limiter)) = self.families.iter_mut().find(|(n, _)| n == family) {
            limiter.apply_state(state);
        }
    }

    /// Every rule of every family with its current count, family name
    /// first, in the order the families were seen.
    pub fn usage(&self) -> Vec<(&str, u32, RateRule)> {
        self.families
            .iter()
            .flat_map(|(name, limiter)| {
                limiter.usage().into_iter().map(move |(used, rule)| (name.as_str(), used, rule))
            })
            .collect()
    }
}

#[derive(Debug, Default)]
struct LimiterRegistry {
    /// `x-rate-limit-policy` value (or a seed key) -> that policy's limiter.
    policies: HashMap<String, PolicyLimiter>,
    /// Which policy each endpoint was last seen under.
    endpoints: HashMap<Endpoint, String>,
}

impl LimiterRegistry {
    /// The policy `ep` is accounted under right now: the one its last
    /// response named, else its seed. The exchange has no binding of its
    /// own until an exchange response names a policy the search is not
    /// under; until then it resolves to the search's binding, so the two
    /// draw on one limiter whatever the search has been rebound to.
    fn key(&self, ep: Endpoint) -> String {
        match self.endpoints.get(&ep) {
            Some(key) => key.clone(),
            None if ep == Endpoint::Exchange => self.key(Endpoint::Search),
            None => ep.seed().0.to_string(),
        }
    }

    fn limiter(&mut self, ep: Endpoint) -> &mut PolicyLimiter {
        let key = self.key(ep);
        if ep != Endpoint::Exchange {
            self.endpoints.entry(ep).or_insert_with(|| key.clone());
        }
        let seed_rules = ep.seed().1;
        self.policies.entry(key).or_insert_with(|| PolicyLimiter::seeded(seed_rules))
    }

    /// Points `ep` at `policy`. A policy seen for the first time inherits
    /// the history the endpoint has built so far, and a policy nothing
    /// resolves to any more is forgotten.
    fn rebind(&mut self, ep: Endpoint, policy: &str) {
        self.limiter(ep);
        let old = self.key(ep);
        if old == policy {
            return;
        }
        if !self.policies.contains_key(policy) {
            let inherited = self.policies[&old].clone();
            self.policies.insert(policy.to_string(), inherited);
        }
        self.endpoints.insert(ep, policy.to_string());
        if !Endpoint::ALL.iter().any(|e| self.key(*e) == old) {
            self.policies.remove(&old);
        }
    }
}

/// Handle on a set of rate limiters keyed by server policy. The site counts
/// requests per policy across the whole address, not per client object, so
/// every `TradeClient` and `StashClient` in the process shares
/// [`Limiters::global`]: the price-check worker and the live-search poller
/// draw on one budget, the way the server sees them. The lock is held only
/// to check and record; waiting happens outside it.
#[derive(Debug, Clone, Default)]
pub struct Limiters(std::sync::Arc<std::sync::Mutex<LimiterRegistry>>);

impl Limiters {
    /// A private set, for a test or a stub server.
    pub fn new() -> Limiters {
        Limiters::default()
    }

    /// The process-wide set every production client uses.
    pub fn global() -> Limiters {
        static GLOBAL: std::sync::OnceLock<Limiters> = std::sync::OnceLock::new();
        GLOBAL.get_or_init(Limiters::new).clone()
    }

    /// Whether both handles count against the same registry.
    pub fn shares_with(&self, other: &Limiters) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0)
    }

    fn with<R>(&self, f: impl FnOnce(&mut LimiterRegistry) -> R) -> R {
        // A panic elsewhere while holding the lock leaves plain counters
        // behind, which are still the best record of what was sent.
        let mut reg = self.0.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut reg)
    }

    /// Whether a request could leave now, without claiming the slot.
    pub fn check(&self, ep: Endpoint, authed: bool) -> RateDecision {
        self.with(|reg| reg.limiter(ep).check(authed))
    }

    /// Claims the next slot for `ep`, sleeping out waits that add up to at
    /// most `max_wait`. The check and the record happen under one lock, so
    /// two threads can never both claim the last slot; the sleep does not
    /// hold it. `Err` carries how long the caller would still have to wait.
    pub fn take_turn(
        &self,
        ep: Endpoint,
        authed: bool,
        max_wait: std::time::Duration,
    ) -> Result<(), std::time::Duration> {
        let mut waited = std::time::Duration::ZERO;
        loop {
            let wait = self.with(|reg| {
                let limiter = reg.limiter(ep);
                match limiter.check(authed) {
                    RateDecision::Ready => {
                        limiter.record(authed);
                        None
                    }
                    RateDecision::Wait(d) => Some(d),
                }
            });
            let Some(wait) = wait else { return Ok(()) };
            if waited + wait > max_wait {
                return Err(wait);
            }
            std::thread::sleep(wait);
            waited += wait;
        }
    }

    /// Takes in a response's rate headers: binds `ep` to the policy the
    /// response names, then updates that policy's families.
    pub fn absorb(&self, ep: Endpoint, authed: bool, headers: &reqwest::header::HeaderMap) {
        self.absorb_with(ep, authed, &|name| {
            headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_string)
        });
    }

    /// [`absorb`](Self::absorb) over any header lookup.
    pub fn absorb_with(&self, ep: Endpoint, authed: bool, header: &dyn Fn(&str) -> Option<String>) {
        self.with(|reg| {
            if let Some(policy) = header("x-rate-limit-policy").filter(|p| !p.trim().is_empty()) {
                reg.rebind(ep, policy.trim());
            }
            reg.limiter(ep).absorb(authed, header);
        });
    }

    /// Records a 429: the policy is locked for the longest of the
    /// `Retry-After` header, any ban the state headers reported (already
    /// absorbed) and the endpoint's default. Returns how long the lock now
    /// lasts, which is what the caller should report and wait.
    pub fn on_429(&self, ep: Endpoint, authed: bool, retry_after: Option<&str>) -> std::time::Duration {
        let named = retry_after
            .and_then(|v| v.trim().parse::<u64>().ok())
            .map(std::time::Duration::from_secs)
            .unwrap_or_default();
        self.with(|reg| {
            let limiter = reg.limiter(ep);
            limiter.ban_for(named.max(ep.default_ban()));
            match limiter.check(authed) {
                RateDecision::Wait(d) => d,
                RateDecision::Ready => ep.default_ban(),
            }
        })
    }

    /// Feeds one family of `ep`'s policy a `-state` header directly.
    pub fn apply_state(&self, ep: Endpoint, family: &str, state: &str) {
        self.with(|reg| reg.limiter(ep).apply_state(family, state));
    }

    /// The policy `ep` is accounted under: the name the server gave, or
    /// the seed key while no response has named one.
    pub fn policy_name(&self, ep: Endpoint) -> String {
        self.with(|reg| reg.key(ep))
    }

    /// The rule of `ep`'s policy with the fewest free slots, across every
    /// family, with its current count. Nothing seen yet reads as the
    /// seed's rules, unused.
    fn tightest_rule(&self, ep: Endpoint) -> Option<(u32, RateRule)> {
        self.with(|reg| {
            reg.limiter(ep)
                .usage()
                .into_iter()
                .map(|(_, used, rule)| (used, rule))
                // Ties go to the longer window, the figure that says more
                // about the next few minutes than a ten-second burst does.
                .min_by_key(|(used, rule)| (rule.max.saturating_sub(*used), std::cmp::Reverse(rule.window_s)))
        })
    }

    /// How many more requests `ep` could send before some rule of its
    /// policy is full: the smallest max-minus-used over every family and
    /// rule, from the server's last state header plus what left since, or
    /// the seed's tightest rule when nothing has been seen. Never negative.
    pub fn free_slots(&self, ep: Endpoint) -> u32 {
        self.tightest_rule(ep).map_or(0, |(used, rule)| rule.max.saturating_sub(used))
    }

    /// Free slots over the rules that hold the user's budget: every rule of
    /// a minute or longer. The ten-second burst rules only pace requests
    /// (the limiter waits them out), and their five slots would make any
    /// "more than N free" test with N of five or more impossible to pass.
    pub fn budget_free(&self, ep: Endpoint) -> u32 {
        self.free_where(ep, |window_s| window_s >= BUDGET_WINDOW_MIN_S)
    }

    /// Free slots over the burst rules (under a minute): zero means a
    /// request sent now would have to wait.
    pub fn burst_free(&self, ep: Endpoint) -> u32 {
        self.free_where(ep, |window_s| window_s < BUDGET_WINDOW_MIN_S)
    }

    /// The smallest max-minus-used over the rules whose window passes
    /// `keep`; with no such rule, nothing limits the request at that scale.
    fn free_where(&self, ep: Endpoint, keep: impl Fn(u32) -> bool) -> u32 {
        self.with(|reg| {
            reg.limiter(ep)
                .usage()
                .into_iter()
                .filter(|(_, _, rule)| keep(rule.window_s))
                .map(|(_, used, rule)| rule.max.saturating_sub(used))
                .min()
                .unwrap_or(u32::MAX)
        })
    }

    /// The budget as the panel shows it, "search 4/30 (5 min)": the most
    /// constrained rule's used/max and window.
    pub fn budget_text(&self, ep: Endpoint) -> String {
        match self.tightest_rule(ep) {
            Some((used, rule)) => format!("{} {used}/{} ({})", ep.name(), rule.max, window_text(rule.window_s)),
            None => format!("{} no rules", ep.name()),
        }
    }

    /// [`budget_text`](Self::budget_text) by the name the price-check
    /// window's status line calls it.
    pub fn snapshot_text(&self, ep: Endpoint) -> String {
        self.budget_text(ep)
    }
}

/// Rules with a window at least this long hold the user's budget; shorter
/// ones only pace bursts.
pub const BUDGET_WINDOW_MIN_S: u32 = 60;

/// A rule window as a person reads it: "10s", "5 min", "6 h".
fn window_text(secs: u32) -> String {
    if secs > 0 && secs.is_multiple_of(3600) {
        format!("{} h", secs / 3600)
    } else if secs > 0 && secs.is_multiple_of(60) {
        format!("{} min", secs / 60)
    } else {
        format!("{secs}s")
    }
}

// --- query building (body shape byte-verified against the live probe) ---

/// What a filter row stands for in the request. Most rows are a stat id
/// with bounds; two are EE2's stand-ins for things the trade site does not
/// index as an item stat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FilterRole {
    #[default]
    Stat,
    /// "N empty prefix/suffix/affix slots": sent as a one-of `count` group
    /// of its own (even while switched off) over the matching
    /// `pseudo_number_of_empty_*_mods` id.
    EmptyModifier,
    /// "Rarity: Magic" on a low-level magic base: switched on, it narrows
    /// `type_filters.rarity` from non-unique to magic. It has no stat id.
    RarityMagic,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StatFilter {
    pub id: String,
    /// The other trade ids the same stat is indexed under (its local or
    /// global twin), from stats.ndjson. A filter with alternates is
    /// searched as "any of these ids at this bound" (see `Query::to_body`);
    /// the alternates share `value` and `disabled`.
    pub alt_ids: Vec<String>,
    pub value: FilterValue,
    pub disabled: bool,
    pub role: FilterRole,
    /// A stat id that must be absent wherever this row applies, sent as a
    /// `not` group carrying the row's own switch. A flask's charge recovery
    /// is only comparable among flasks without the reduced-effect downside
    /// (awakened-poe-trade issue 758).
    pub not_id: Option<String>,
}

impl StatFilter {
    /// A plain stat row searched from `min` up.
    pub fn at_least(id: impl Into<String>, min: f64, disabled: bool) -> StatFilter {
        StatFilter {
            id: id.into(),
            alt_ids: Vec::new(),
            value: FilterValue { min: Some(min), max: None },
            disabled,
            role: FilterRole::Stat,
            not_id: None,
        }
    }

    fn member_json(&self, id: &str) -> serde_json::Value {
        serde_json::json!({"id": id, "value": self.value.to_json(), "disabled": self.disabled})
    }
}

/// Search bounds. Floats so decimal rolls (attack speed 1.35, crit 3.5%)
/// are searchable; whole values serialize as integers (see `num_json`), so
/// the body still reads `155`, not `155.0`. A stat where lower is better
/// carries only a `max`; a flag or option stat carries neither.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FilterValue {
    pub min: Option<f64>,
    pub max: Option<f64>,
}

impl FilterValue {
    pub fn to_json(&self) -> serde_json::Value {
        let mut v = serde_json::Map::new();
        if let Some(min) = self.min {
            v.insert("min".into(), num_json(min));
        }
        if let Some(max) = self.max {
            v.insert("max".into(), num_json(max));
        }
        serde_json::Value::Object(v)
    }
}

/// A search bound as JSON: an integer when whole (`155`), else a float
/// (`3.5`). The trade site sends whole values as integers, and the verified
/// body shape asserts on integers, so we preserve that.
fn num_json(v: f64) -> serde_json::Value {
    if v.is_finite() && v.fract() == 0.0 {
        serde_json::json!(v as i64)
    } else {
        serde_json::json!(v)
    }
}

/// Minimum bounds on the trade site's own computed item figures - DPS,
/// defences, spirit, rune sockets - which are not item mods and so live in
/// their own filter section rather than in `stats`.
///
/// Serialized into `filters.equipment_filters`, NOT `filters.weapon_filters`:
/// verified live 2026-08-07 against `/api/trade2/data/filters`, whose only
/// group carrying `dps`/`pdps`/`edps`/`crit`/`aps` is `equipment_filters`
/// (`ar`/`ev`/`es`/`spirit`/`rune_sockets` sit in the same group, checked
/// 2026-09-18). A probe POST with a `weapon_filters` group is rejected
/// outright with `{"error":{"code":2,"message":"Unknown filter group:
/// weapon_filters"}}` (that is the PoE1 trade name; trade2 renamed the
/// section), while the same body under `equipment_filters` returns results.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EquipmentFilters {
    /// Set bounds in the order they were first set.
    bounds: Vec<(EquipKey, f64)>,
}

impl EquipmentFilters {
    pub fn get(&self, key: EquipKey) -> Option<f64> {
        self.bounds.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
    }

    /// Sets or (with `None`) clears one bound.
    pub fn set(&mut self, key: EquipKey, min: Option<f64>) {
        self.bounds.retain(|(k, _)| *k != key);
        if let Some(v) = min {
            self.bounds.push((key, v));
        }
    }

    pub fn with(mut self, key: EquipKey, min: f64) -> Self {
        self.set(key, Some(min));
        self
    }

    /// True when no bound is set, so the whole block is omitted from the body.
    pub fn is_empty(&self) -> bool {
        self.bounds.is_empty()
    }
}

/// An open or closed numeric range on an item filter. Either end may be
/// absent; with both absent the filter is still sent, empty, which is what
/// EE2 sends for a figure it switched on but could not read.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Bound {
    pub min: Option<f64>,
    pub max: Option<f64>,
}

impl Bound {
    pub fn exact(v: f64) -> Bound {
        Bound { min: Some(v), max: Some(v) }
    }

    pub fn at_least(v: f64) -> Bound {
        Bound { min: Some(v), max: None }
    }

    fn to_json(self) -> serde_json::Value {
        FilterValue { min: self.min, max: self.max }.to_json()
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Query {
    pub category: Option<String>,
    /// Whether the gear `category` constraint is applied; the category
    /// rides along dormant so it can be re-enabled without rebuilding the
    /// query.
    pub category_enabled: bool,
    /// EE2 searches an item either by its category or by its exact
    /// name/base, never both: with this set, `name` and `type_name` are
    /// sent only while the category is off. Unset, all three are sent
    /// (a cut gem is searched by category and base together).
    pub category_replaces_type: bool,
    /// Exact item name to search (`query.name`): set for uniques, whose name
    /// identifies the item; never for rares, whose names are random.
    pub name: Option<String>,
    /// Exact base type to search (`query.type`).
    pub type_name: Option<String>,
    /// Waystone/map tier -> `filters.map_filters.filters.map_tier` (searched
    /// as an exact tier: `{min: t, max: t}`), the dominant price driver.
    pub map_tier: Option<i64>,
    /// Skill/support gem level -> `filters.misc_filters.filters.gem_level`.
    pub gem_level: Option<Bound>,
    /// `misc_filters.gem_sockets.min`: a gem's support sockets.
    pub gem_sockets_min: Option<u32>,
    /// `misc_filters.area_level`, for trial keys.
    pub area_level: Option<Bound>,
    /// `misc_filters.identified`, `misc_filters.unidentified_tier.min`.
    pub identified: Option<bool>,
    pub unidentified_tier_min: Option<u32>,
    /// Computed-figure bounds -> `filters.equipment_filters.filters.{dps,ar,
    /// spirit,...}`. `None` (or an empty `EquipmentFilters`) omits the whole
    /// section.
    pub equipment: Option<EquipmentFilters>,
    /// `type_filters.rarity`: "nonunique" on a rare, magic or normal item,
    /// whose category search would otherwise list uniques of the same slot
    /// among the comparables.
    pub rarity: Option<String>,
    /// `type_filters.ilvl.min`, for items priced as a crafting base.
    pub ilvl_min: Option<u32>,
    /// `type_filters.quality.min`.
    pub quality_min: Option<u32>,
    /// `req_filters.lvl.max`.
    pub req_level_max: Option<u32>,
    /// `misc_filters.{corrupted,mirrored,sanctified}`. A clean item is
    /// compared with clean items only: a corrupted or mirrored twin cannot be
    /// crafted further and sells for less.
    pub corrupted: Option<bool>,
    pub mirrored: Option<bool>,
    pub sanctified: Option<bool>,
    /// `misc_filters.veiled`: an unrevealed desecrated mod.
    pub veiled: Option<bool>,
    /// `trade_filters.collapse`: one listing per seller, so a single account
    /// dumping twenty near-identical items cannot fill the first page.
    pub collapse: bool,
    /// `trade_filters.price.option`: the currencies a listing may be priced
    /// in ("exalted_divine"). None sends no price filter; the price-fixed
    /// strip's button sets it, the way EE2's does.
    pub price_option: Option<String>,
    /// `trade_filters.price.min`: the lowest asking price, in the site's
    /// default unit (exalted orb equivalent). None sends no bound; the
    /// craft calibration sets it to move its searches up the market.
    pub price_min: Option<f64>,
    /// `misc_filters.fractured_item`: a fractured base is a different
    /// (dearer) thing than the plain base it is compared with.
    pub fractured: Option<bool>,
    pub filters: Vec<StatFilter>,
}

impl Query {
    /// Serializes to the exact trade-search body shape EE2's
    /// `createTradeRequest` produces for the same selection (held to it by
    /// `tests/ee2_parity.rs`). This is the one place the JSON is made, so a
    /// row toggled in the panel is sent the way EE2 would send that toggle.
    pub fn to_body(&self) -> serde_json::Value {
        // Filters whose id starts with "map_" (waystone rarity/pack-size/
        // effectiveness/etc.) go in the map_filters section, not stats; and
        // only enabled ones (map filters have no disabled flag). Everything
        // else is a stat filter (disabled ones ride along with disabled:true).
        // A filter with alternate ids cannot sit in the "and" group, where
        // every id must match: it becomes its own `count >= 1` group over
        // all its ids at the same bound, the way EE2 sends multi-id stats.
        // Verified live 2026-09-10: such a group matches exactly what the
        // right id alone matches, honours its members' bounds, combines
        // with the "and" group, and is ignored when marked disabled.
        let mut and_group: Vec<serde_json::Value> = Vec::new();
        let mut groups: Vec<serde_json::Value> = Vec::new();
        let mut rarity = self.rarity.clone();
        for f in self.filters.iter().filter(|f| !f.id.starts_with("map_")) {
            match f.role {
                FilterRole::EmptyModifier => {
                    groups.push(serde_json::json!({
                        "type": "count",
                        "value": {"min": 1, "max": 1},
                        "disabled": f.disabled,
                        "filters": [f.member_json(&f.id)],
                    }));
                    continue;
                }
                FilterRole::RarityMagic => {
                    if !f.disabled {
                        rarity = Some("magic".to_string());
                    }
                    continue;
                }
                FilterRole::Stat => {}
            }
            if let Some(not_id) = &f.not_id {
                groups.push(serde_json::json!({
                    "type": "not",
                    "disabled": f.disabled,
                    "filters": [{"id": not_id, "disabled": f.disabled}],
                }));
            }
            if f.alt_ids.is_empty() {
                and_group.push(f.member_json(&f.id));
            } else {
                let members: Vec<serde_json::Value> =
                    std::iter::once(&f.id).chain(&f.alt_ids).map(|id| f.member_json(id)).collect();
                groups.push(serde_json::json!({
                    "type": "count",
                    "value": {"min": 1},
                    "disabled": f.disabled,
                    "filters": members,
                }));
            }
        }
        let mut stats = vec![serde_json::json!({"type": "and", "filters": and_group})];
        stats.extend(groups);
        let mut query = serde_json::json!({
            // "securable" = Instant Buyout only, matching the trade site's
            // own default. In-person listings are excluded deliberately:
            // they cost nothing to fake, so price-fixers park lowball
            // in-person listings they never honor (seen live: 30ex and
            // 200ex in-person bait under a 410ex instant-buyout floor, and
            // the site showed 410 as the real price). A buyout listing
            // cannot lie - anyone could take it.
            "status": {"option": "securable"},
            "stats": stats,
        });
        let by_category = self.category_enabled && self.category.is_some();
        if !(by_category && self.category_replaces_type) {
            if let Some(n) = &self.name {
                query["name"] = serde_json::json!(n);
            }
            if let Some(t) = &self.type_name {
                query["type"] = serde_json::json!(t);
            }
        }
        let mut filters = serde_json::Map::new();
        let mut typef = serde_json::Map::new();
        if let (true, Some(cat)) = (self.category_enabled, &self.category) {
            typef.insert("category".into(), serde_json::json!({"option": cat}));
        }
        if let Some(r) = &rarity {
            typef.insert("rarity".into(), serde_json::json!({"option": r}));
        }
        if let Some(l) = self.ilvl_min {
            typef.insert("ilvl".into(), serde_json::json!({"min": l}));
        }
        if let Some(q) = self.quality_min {
            typef.insert("quality".into(), serde_json::json!({"min": q}));
        }
        if !typef.is_empty() {
            filters.insert("type_filters".into(), serde_json::json!({"filters": typef}));
        }
        if let Some(l) = self.req_level_max {
            filters.insert("req_filters".into(), serde_json::json!({"filters": {"lvl": {"max": l}}}));
        }
        let mut tradef = serde_json::Map::new();
        if self.collapse {
            tradef.insert("collapse".into(), serde_json::json!({"option": "true"}));
        }
        if self.price_option.is_some() || self.price_min.is_some() {
            let mut price = serde_json::Map::new();
            if let Some(p) = &self.price_option {
                price.insert("option".into(), serde_json::json!(p));
            }
            if let Some(min) = self.price_min {
                price.insert("min".into(), serde_json::json!(min));
            }
            tradef.insert("price".into(), serde_json::Value::Object(price));
        }
        if !tradef.is_empty() {
            filters.insert("trade_filters".into(), serde_json::json!({"filters": tradef}));
        }
        let mut mapf = serde_json::Map::new();
        if let Some(tier) = self.map_tier {
            mapf.insert("map_tier".into(), serde_json::json!({"min": tier, "max": tier}));
        }
        for f in self.filters.iter().filter(|f| f.id.starts_with("map_") && !f.disabled) {
            mapf.insert(f.id.clone(), f.value.to_json());
        }
        if !mapf.is_empty() {
            filters.insert("map_filters".into(), serde_json::json!({"filters": mapf}));
        }
        let mut miscf = serde_json::Map::new();
        if let Some(level) = self.gem_level {
            miscf.insert("gem_level".into(), level.to_json());
        }
        if let Some(n) = self.gem_sockets_min {
            miscf.insert("gem_sockets".into(), serde_json::json!({"min": n}));
        }
        if let Some(level) = self.area_level {
            miscf.insert("area_level".into(), level.to_json());
        }
        if let Some(t) = self.unidentified_tier_min {
            miscf.insert("unidentified_tier".into(), serde_json::json!({"min": t}));
        }
        for (key, flag) in [
            ("identified", self.identified),
            ("corrupted", self.corrupted),
            ("mirrored", self.mirrored),
            ("sanctified", self.sanctified),
            ("fractured_item", self.fractured),
            ("veiled", self.veiled),
        ] {
            if let Some(v) = flag {
                miscf.insert(key.into(), serde_json::json!({"option": v.to_string()}));
            }
        }
        if !miscf.is_empty() {
            filters.insert("misc_filters".into(), serde_json::json!({"filters": miscf}));
        }
        // Equipment bounds are open-ended ("this much DPS or more"): each
        // carries one end, the low one for every figure but reload time,
        // where shorter is better.
        if let Some(w) = &self.equipment {
            let mut wf = serde_json::Map::new();
            for (key, v) in &w.bounds {
                let end = if key.lower_is_better() { "max" } else { "min" };
                wf.insert(key.trade_key().into(), serde_json::json!({end: num_json(*v)}));
            }
            if !wf.is_empty() {
                filters.insert(
                    "equipment_filters".into(),
                    serde_json::json!({"filters": wf}),
                );
            }
        }
        if !filters.is_empty() {
            query["filters"] = serde_json::Value::Object(filters);
        }
        serde_json::json!({"query": query, "sort": {"price": "asc"}})
    }
}

/// The trade category for an item class, from the live category options in
/// `/api/trade2/data/filters` (fetched 2026-09-08). Classes are the game's
/// own "Item Class:" wording. Currency and other stackables have no gear
/// category and return `None`.
pub fn category_for(item_class: &str) -> Option<String> {
    let c = item_class.to_ascii_lowercase();
    let cat = match c.as_str() {
        "claws" => "weapon.claw",
        "daggers" => "weapon.dagger",
        "one hand swords" => "weapon.onesword",
        "one hand axes" => "weapon.oneaxe",
        "one hand maces" => "weapon.onemace",
        "spears" => "weapon.spear",
        "flails" => "weapon.flail",
        "two hand swords" => "weapon.twosword",
        "two hand axes" => "weapon.twoaxe",
        "two hand maces" => "weapon.twomace",
        "quarterstaves" => "weapon.warstaff",
        "talismans" => "weapon.talisman",
        "bows" => "weapon.bow",
        "crossbows" => "weapon.crossbow",
        "wands" => "weapon.wand",
        "sceptres" => "weapon.sceptre",
        "staves" => "weapon.staff",
        "fishing rods" => "weapon.rod",
        "helmets" => "armour.helmet",
        "body armours" => "armour.chest",
        "gloves" => "armour.gloves",
        "boots" => "armour.boots",
        "quivers" => "armour.quiver",
        "shields" => "armour.shield",
        "foci" => "armour.focus",
        "bucklers" => "armour.buckler",
        "amulets" => "accessory.amulet",
        "rings" => "accessory.ring",
        "belts" => "accessory.belt",
        "skill gems" => "gem.activegem",
        "support gems" => "gem.supportgem",
        "meta gems" => "gem.metagem",
        "jewels" => "jewel",
        "life flasks" => "flask.life",
        "mana flasks" => "flask.mana",
        "charms" => "flask.charm",
        "waystones" => "map.waystone",
        "tablets" => "map.tablet",
        "relics" => "sanctum.relic",
        _ => return None,
    };
    Some(cat.to_string())
}

/// First number in a mod line (the rolled value), if any.
fn first_number(text: &str) -> Option<f64> {
    let mut cur = String::new();
    for ch in text.chars() {
        if ch.is_ascii_digit() || (ch == '.' && !cur.is_empty()) {
            cur.push(ch);
        } else if !cur.is_empty() {
            break;
        }
    }
    if cur.is_empty() {
        None
    } else {
        cur.parse().ok()
    }
}

/// A filter's human-facing description, index-aligned with
/// Query::filters: the cleaned mod text, its tier (when annotated), and
/// the search floor. This is what an interactive panel renders next to
/// each checkbox.
#[derive(Debug, Clone, PartialEq)]
pub struct FilterLabel {
    pub text: String,
    pub tier: Option<u8>,
    pub min: i64,
    /// The value the item actually rolled (the first number of the mod
    /// line), as opposed to `min`, the search floor. A tier badge or roll
    /// score reads this; searching by tier floor is a pricing choice that
    /// must not leak into how good the roll looks. `None` when the line
    /// carries no number.
    pub rolled: Option<f64>,
    /// Mod group for panel grouping/tagging: "pseudo", "implicit",
    /// "explicit", "map".
    pub tag: &'static str,
    /// A row EE2 keeps folded away by default (a total that repeats mods
    /// already listed, a fixed line of a unique). It is still part of the
    /// query, switched off.
    pub hidden: bool,
    /// The item's own lines this row stands for: two for a stat that two
    /// mods grant and the row sums. Empty for a row that is no line of the
    /// item (an open-affix count) and on the upgrade search's rows.
    pub lines: Vec<String>,
}

/// A stat that counts things on the item (open affix slots, unrevealed
/// mods, tablet uses, resistances present). A count has no "10% looser".
fn is_count_stat(id: &str) -> bool {
    id.starts_with("pseudo.pseudo_number_of_") || id.starts_with("pseudo.pseudo_count_")
}

/// Widens every enabled bound by `pct` (0.10 = the panel's "Broad (-10%)"),
/// which surfaces the comparable items an exact-roll search misses. Each
/// end moves in the direction that admits more items whatever its sign: a
/// minimum goes down (20 -> 18, and -20 -> -22 for a stat the site indexes
/// negated), a maximum goes up (30 -> 33, -10 -> -9). The old rule touched
/// positive minimums only, so "reduced" stats and lower-is-better bounds
/// stayed exact in a search labelled broad. Disabled filters, count stats
/// and rows without a bound are left alone, and a bound never crosses zero.
pub fn relax_query(query: &Query, pct: f64) -> Query {
    let mut out = query.clone();
    for f in &mut out.filters {
        // An open-affix count is a count, not a roll.
        if f.disabled || f.role != FilterRole::Stat || is_count_stat(&f.id) {
            continue;
        }
        f.value.min = f.value.min.map(|min| min - min.abs() * pct);
        f.value.max = f.value.max.map(|max| max + max.abs() * pct);
    }
    // Equipment bounds are minimums like any other; a Broad search must not
    // hold DPS to the exact roll while relaxing every mod around it. Sockets
    // are a count, not a roll, and stay put.
    if let Some(w) = &mut out.equipment {
        for (key, v) in &mut w.bounds {
            if key.lower_is_better() {
                *v *= 1.0 + pct;
            } else if *key != EquipKey::RuneSockets {
                *v *= 1.0 - pct;
            }
        }
    }
    out
}

/// Builds the gear-upgrade query for an equipped item: strictly-better
/// pieces of the same gear class. The category constraint is applied the
/// same way `build_query` applies it (`category_for` on the item class,
/// enabled), and every resolvable explicit mod with a numeric roll becomes
/// an ENABLED stat filter whose min is the item's CURRENT rolled value -
/// not the tier floor `build_query` searches on - so a match must
/// meet-or-beat the equipped item on every kept mod. Mods that fail stat
/// resolution or carry no numeric value are skipped, never guessed.
/// Rune-socket mods are skipped for the same reason `build_query` skips
/// them: they are swappable gear, not the item's own explicits.
///
/// Results come back cheapest-first without any extra step here:
/// `Query::to_body` always emits `"sort": {"price": "asc"}`.
pub fn build_upgrade_query(item: &crate::item::Item, stats: &StatIndex) -> Query {
    build_upgrade_query_with_labels(item, stats).0
}

/// Like [`build_upgrade_query`], but also returns one label per filter so
/// the result seeds an interactive panel: the upgrade search's thresholds
/// are exactly the rows a user wants to raise before searching again.
pub fn build_upgrade_query_with_labels(
    item: &crate::item::Item,
    stats: &StatIndex,
) -> (Query, Vec<FilterLabel>) {
    let mut filters: Vec<StatFilter> = Vec::new();
    let mut labels: Vec<FilterLabel> = Vec::new();
    for affix in affixes(&item.explicits, stats) {
        if affix.header.is_some_and(|h| h.kind == crate::item::ModKind::Rune) {
            continue;
        }
        let Some((id, alt_ids)) = affix.ids() else { continue };
        if filters.iter().any(|f| f.id == id) {
            continue;
        }
        let Some(min) = first_number(&affix.text) else { continue };
        filters.push(StatFilter { alt_ids, ..StatFilter::at_least(id, min, false) });
        labels.push(FilterLabel {
            text: affix.label(),
            tier: affix.header.and_then(|h| h.tier),
            min: min as i64,
            rolled: Some(min),
            tag: "explicit",
            hidden: false,
            lines: Vec::new(),
        });
    }
    (
        Query {
            category: category_for(&item.item_class),
            category_enabled: true,
            rarity: Some("nonunique".to_string()),
            collapse: true,
            filters,
            ..Default::default()
        },
        labels,
    )
}

/// Panel/window title for an upgrade search, e.g. "upgrades: Bows".
pub fn upgrade_title(item: &crate::item::Item) -> String {
    format!("upgrades: {}", item.item_class)
}

/// The most stat groups (the "and" group included) the trade site accepts
/// in a search sent without a session. Measured live on 2026-09-30: the
/// same tablet search answered 400 "Query is too complex ... Logging in
/// will increase this limit" with four groups and 200 with any three.
pub const ANONYMOUS_STAT_GROUPS: usize = 3;

/// The body [`TradeClient::search`] sends for `q`. With a session it is
/// `q.to_body()` as built. Without one, a body over
/// [`ANONYMOUS_STAT_GROUPS`] is folded to fit without changing what it
/// matches: disabled groups, which the site ignores, and an empty "and"
/// group are left out, and the enabled twin groups (a stat searched as
/// "any one of its ids", `count` with min 1) become one `count` group over
/// all their ids with min equal to their number. An item carries only one
/// id of a stat, so each twin group can contribute at most one match, and
/// k matches among their union means every one of them matched. A body
/// that still does not fit is refused here rather than sent to be refused
/// by the site.
pub fn search_body(q: &Query, anonymous: bool) -> Result<serde_json::Value, TradeError> {
    let mut body = q.to_body();
    let groups = |b: &serde_json::Value| b["query"]["stats"].as_array().map_or(0, Vec::len);
    if !anonymous || groups(&body) <= ANONYMOUS_STAT_GROUPS {
        return Ok(body);
    }
    // The id lists `to_body` gives twin groups, from the filters that made
    // them, so a group is merged only when it really is one stat's twins.
    let twins: Vec<Vec<&str>> = q
        .filters
        .iter()
        .filter(|f| f.role == FilterRole::Stat && !f.disabled && !f.alt_ids.is_empty() && !f.id.starts_with("map_"))
        .map(|f| std::iter::once(f.id.as_str()).chain(f.alt_ids.iter().map(String::as_str)).collect())
        .collect();
    let ids_of = |g: &serde_json::Value| -> Vec<String> {
        g["filters"].as_array().map_or_else(Vec::new, |fs| {
            fs.iter().filter_map(|f| f["id"].as_str().map(str::to_string)).collect()
        })
    };
    let is_twin_group = |g: &serde_json::Value| {
        g["type"] == "count"
            && g["disabled"] != true
            && g["value"] == serde_json::json!({"min": 1})
            && twins.iter().any(|t| t.iter().copied().eq(ids_of(g).iter().map(String::as_str)))
    };
    let stats = body["query"]["stats"].as_array().cloned().unwrap_or_default();
    let mut kept = Vec::new();
    let mut merged_members: Vec<serde_json::Value> = Vec::new();
    let mut merged_ids: HashSet<String> = HashSet::new();
    let mut merged = 0usize;
    for g in stats {
        let empty_and = g["type"] == "and" && g["filters"].as_array().is_none_or(Vec::is_empty);
        if empty_and || (g["type"] != "and" && g["disabled"] == true) {
            continue;
        }
        let ids = ids_of(&g);
        // A stat already merged would be counted twice; such a group
        // stays on its own.
        if is_twin_group(&g) && ids.iter().all(|id| !merged_ids.contains(id)) {
            merged_ids.extend(ids);
            merged_members.extend(g["filters"].as_array().cloned().unwrap_or_default());
            merged += 1;
            continue;
        }
        kept.push(g);
    }
    if merged > 0 {
        kept.push(serde_json::json!({
            "type": "count", "value": {"min": merged}, "disabled": false, "filters": merged_members,
        }));
    }
    if kept.len() > ANONYMOUS_STAT_GROUPS {
        return Err(TradeError::Refused(format!(
            "search: this item needs {} stat groups and a search without a session allows {ANONYMOUS_STAT_GROUPS}; \
             untick some mods, or set a POESESSID in Settings -> Account to raise the limit",
            kept.len()
        )));
    }
    body["query"]["stats"] = serde_json::Value::Array(kept);
    Ok(body)
}

/// Query for a specific cut skill gem at an exact level, e.g. "Detonate
/// Living" (Level 20): category `gem.activegem`, the skill name as the base
/// `type`, and an exact `gem_level` filter. This is how the reward panel
/// prices its "Skill Level N: <name>" rows individually instead of collapsing
/// them all to the fungible Uncut Skill Gem price.
pub fn build_gem_query(skill: &str, level: i64) -> Query {
    Query {
        category: Some("gem.activegem".into()),
        category_enabled: true,
        name: None,
        type_name: Some(skill.to_string()),
        map_tier: None,
        gem_level: Some(Bound::exact(level as f64)),
        collapse: true,
        ..Default::default()
    }
}

/// How many of the cheapest exchange offers the quoted rate is drawn from.
const EXCHANGE_SAMPLE: usize = 5;

/// Parses the currency-exchange response into a going rate: how many units
/// of the `have` currency one unit of the `want` currency costs
/// (`exchange.amount / item.amount` per offer). The rate is the median of
/// the [`EXCHANGE_SAMPLE`] cheapest offers, not the single cheapest: one
/// offer far under the market is bait or a typo, never fillable at volume,
/// and as the minimum it alone set the price. With one or two offers the
/// median is all there is to go on. `None` when there are no offers. This
/// is how EE2 prices stackable currency (omens, essences, catalysts) that
/// poe.ninja does not track for PoE2.
pub fn parse_exchange_rate(body: &str) -> Option<f64> {
    exchange_rate_of(&serde_json::from_str(body).ok()?)
}

/// [`parse_exchange_rate`] over an already parsed body.
pub fn exchange_rate_of(v: &serde_json::Value) -> Option<f64> {
    let result = v.get("result")?.as_object()?;
    let mut rates: Vec<f64> = Vec::new();
    for listing in result.values() {
        let Some(offers) = listing.pointer("/listing/offers").and_then(|o| o.as_array()) else {
            continue;
        };
        for offer in offers {
            let item_amt = offer.pointer("/item/amount").and_then(serde_json::Value::as_f64);
            let exch_amt = offer.pointer("/exchange/amount").and_then(serde_json::Value::as_f64);
            if let (Some(item_amt), Some(exch_amt)) = (item_amt, exch_amt) {
                let rate = exch_amt / item_amt;
                if item_amt > 0.0 && rate.is_finite() && rate > 0.0 {
                    rates.push(rate);
                }
            }
        }
    }
    rates.sort_by(f64::total_cmp);
    rates.truncate(EXCHANGE_SAMPLE);
    match rates.len() {
        0 => None,
        n if n % 2 == 1 => Some(rates[n / 2]),
        n => Some((rates[n / 2 - 1] + rates[n / 2]) / 2.0),
    }
}

/// Maps each currency item's display name (lowercased) to its trade currency
/// id, from the `/api/trade2/data/static` response (e.g. "omen of whittling"
/// -> "omen-of-whittling", "exalted orb" -> "exalted"). The catalog also
/// holds layout entries under the id "sep": section headers such as "Uncut
/// Skill Gems" and blank spacers. They are not items, and the exchange
/// refuses "sep" as a want tag, so they are left out, as is any entry
/// without a name or an id.
pub fn parse_static_currency_ids(body: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return map;
    };
    let Some(groups) = v.get("result").and_then(|r| r.as_array()) else {
        return map;
    };
    for g in groups {
        let Some(entries) = g.get("entries").and_then(|e| e.as_array()) else {
            continue;
        };
        for e in entries {
            let id = e.get("id").and_then(|i| i.as_str());
            let text = e.get("text").and_then(|t| t.as_str());
            if let (Some(id), Some(text)) = (id, text) {
                if id == "sep" || id.is_empty() || text.trim().is_empty() {
                    continue;
                }
                map.insert(text.to_lowercase(), id.to_string());
            }
        }
    }
    map
}

/// Exact gem base-type names from the `/api/trade2/data/items` "Gems" group
/// (e.g. "Detonate Living", "Fragments Of The Past"). The trade `type` field
/// is case-sensitive, so these are matched against OCR text to recover the
/// exact spelling before searching.
pub fn parse_gem_types(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return out;
    };
    let Some(groups) = v.get("result").and_then(|r| r.as_array()) else {
        return out;
    };
    for g in groups {
        if g.get("label").and_then(|l| l.as_str()) != Some("Gems") {
            continue;
        }
        if let Some(entries) = g.get("entries").and_then(|e| e.as_array()) {
            for e in entries {
                if let Some(t) = e.get("type").and_then(|t| t.as_str()) {
                    out.push(t.to_string());
                }
            }
        }
    }
    out
}

/// Recovers the exact gem name for OCR text (lowercased, whitespace-collapsed)
/// by matching against `gems`: an exact case-insensitive hit first, else the
/// closest fuzzy match above a confidence floor (tolerates minor OCR slips).
/// `None` when nothing is close enough, so a misread never prices the wrong gem.
pub fn match_gem_name(ocr: &str, gems: &[String]) -> Option<String> {
    let norm = |s: &str| s.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ");
    let q = norm(ocr);
    if q.is_empty() {
        return None;
    }
    if let Some(g) = gems.iter().find(|g| norm(g) == q) {
        return Some(g.clone());
    }
    let mut best: Option<(f64, &String)> = None;
    for g in gems {
        let sim = strsim::normalized_levenshtein(&q, &norm(g));
        if best.as_ref().is_none_or(|(b, _)| sim > *b) {
            best = Some((sim, g));
        }
    }
    best.filter(|(s, _)| *s >= 0.82).map(|(_, g)| g.clone())
}

/// The API's own explanation when it has one: a trade error body carries
/// `{"error":{"message":…}}`, and "Unknown item base type" diagnoses a
/// problem that a bare "status 400" hides.
fn api_error(what: &str, status: u16, body: &str) -> String {
    let msg = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| Some(v.get("error")?.get("message")?.as_str()?.to_string()));
    match msg {
        Some(m) => format!("{what}: {m} ({status})"),
        None => format!("{what} status {status}"),
    }
}

/// One searchable unit of an item's mod list: usually a single line, but
/// the lines of one affix joined with `\n` when the catalog stat itself
/// spans them ("Spells fire # additional Projectiles\nSpells fire
/// Projectiles in a circle"). A hybrid affix whose lines are separate
/// catalog stats ("+# to Armour" over "+# to Evasion Rating") stays two
/// units, which is how the trade site indexes it.
struct Affix<'a> {
    text: String,
    header: Option<&'a crate::item::ModHeader>,
    /// Every catalog entry the text is indexed under, primary first; empty
    /// for a line the catalog does not know.
    entries: Vec<&'a StatEntry>,
}

impl Affix<'_> {
    /// The primary trade id and its alternates, or `None` for an unknown line.
    fn ids(&self) -> Option<(String, Vec<String>)> {
        let (first, rest) = self.entries.split_first()?;
        Some((first.id.clone(), rest.iter().map(|e| e.id.clone()).collect()))
    }

    /// Panel text: roll annotations dropped, and a joined stat's lines on
    /// one row.
    fn label(&self) -> String {
        strip_range_annotations(&self.text).replace('\n', " / ")
    }
}

/// Splits parsed mod lines into [`Affix`]es. At each line, the longest run
/// of following lines under the same affix header whose joined text is a
/// catalog stat is taken as one unit (a two-line stat must win over its
/// first line alone, which may be a different, single-line stat); otherwise
/// the line stands by itself, resolved or not. Lines under different
/// headers are never joined: they are different affixes by the game's own
/// account. Simple-format lines carry no header and so may join when, and
/// only when, the catalog spells out exactly that pair.
fn affixes<'a>(mods: &'a [crate::item::ItemMod], stats: &'a StatIndex) -> Vec<Affix<'a>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < mods.len() {
        let head = &mods[i];
        let run = mods[i..]
            .iter()
            .take(stats.max_lines())
            .take_while(|m| m.header == head.header)
            .count();
        let mut taken = 1;
        let mut text = head.text.clone();
        let mut entries = Vec::new();
        for n in (2..=run).rev() {
            let joined: Vec<&str> = mods[i..i + n].iter().map(|m| m.text.as_str()).collect();
            let joined = joined.join("\n");
            let found = stats.resolve_all(&joined);
            if !found.is_empty() {
                taken = n;
                text = joined;
                entries = found;
                break;
            }
        }
        if taken == 1 {
            entries = stats.resolve_all(&text);
        }
        out.push(Affix { text, header: head.header.as_ref(), entries });
        i += taken;
    }
    out
}

/// Removes the advanced-format "(min-max)" roll annotations for display:
/// "+45(40-49) to maximum Life" reads as "+45 to maximum Life".
fn strip_range_annotations(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut depth = 0u32;
    for c in text.chars() {
        match c {
            '(' => depth += 1,
            ')' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

// --- HTTP client (endpoints and shapes verified live 2026-07-21) ---

#[derive(Debug, Deserialize)]
pub struct SearchResult {
    pub id: String,
    #[serde(rename = "result")]
    pub hashes: Vec<String>,
    /// How many listings match in all; `hashes` is only the first page of
    /// them. Absent on responses that do not report it.
    #[serde(default)]
    pub total: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Listing {
    pub price_amount: f64,
    pub price_currency: String,
    pub account: String,
    pub indexed: String,
    pub item_name: String,
}

/// The listings of one fetch, and how many of the requested ones are not
/// among them: the API answers `null` for a listing that sold or went
/// offline since the search, and a listing can come back without a price.
/// A caller that shows "N listings" needs to know some were left out.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FetchOutcome {
    pub listings: Vec<Listing>,
    pub dropped: usize,
    /// The entries exactly as received, one per requested hash in the
    /// search's order, `None` where the API sent null. The listings table
    /// and the hover card read the mods, tiers, seller state and fee from
    /// these; `listings` is only the priced summary.
    pub raw: Vec<Option<serde_json::Value>>,
}

#[derive(Debug, Deserialize)]
struct FetchResponse {
    result: Vec<Option<serde_json::Value>>,
}

#[derive(Debug, Deserialize)]
struct FetchEntry {
    listing: RawListing,
    item: Option<RawItem>,
}

#[derive(Debug, Deserialize)]
struct RawListing {
    price: Option<RawPrice>,
    indexed: Option<String>,
    account: Option<RawAccount>,
}

#[derive(Debug, Deserialize)]
struct RawPrice {
    amount: f64,
    currency: String,
}

#[derive(Debug, Deserialize)]
struct RawAccount {
    name: String,
}

#[derive(Debug, Deserialize)]
struct RawItem {
    name: Option<String>,
    #[serde(rename = "baseType")]
    base_type: Option<String>,
}

pub fn parse_search(json: &str) -> Result<SearchResult, TradeError> {
    Ok(serde_json::from_str(json)?)
}

pub fn parse_fetch(json: &str) -> Result<FetchOutcome, TradeError> {
    let r: FetchResponse = serde_json::from_str(json)?;
    let raw = r.result;
    let mut listings = Vec::with_capacity(raw.len());
    for entry in raw.iter().flatten() {
        let e: FetchEntry = serde_json::from_value(entry.clone())?;
        let Some(price) = e.listing.price else { continue };
        listings.push(Listing {
            price_amount: price.amount,
            price_currency: price.currency,
            account: e.listing.account.map(|a| a.name).unwrap_or_default(),
            indexed: e.listing.indexed.unwrap_or_default(),
            item_name: e.item.and_then(|i| i.name.or(i.base_type)).unwrap_or_default(),
        });
    }
    let dropped = raw.len() - listings.len();
    Ok(FetchOutcome { listings, dropped, raw })
}

/// The longest a request waits for its turn before giving up with
/// `TradeError::Cooldown`: the burst rules (5 searches per 10s, 12 fetches
/// per 4s) clear within this, and anything longer is a wait the user
/// should be told about rather than sat through. Checking items faster
/// than the burst rule allows used to fail every check past the fifth with
/// a cooldown error, though the slot was seconds away.
const MAX_TURN_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

const BROWSER_UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) \
                          Chrome/126.0 Safari/537.36 khaloni-poe2/0.1";

/// The production API host.
pub const TRADE_BASE: &str = "https://www.pathofexile.com";

/// One of the `/api/trade2/data/*` catalogs. They change with a game patch,
/// not by the hour, so they are kept on disk (see
/// [`TradeClient::cached_data`]) instead of downloaded at every launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TradeData {
    /// Every searchable stat: what [`StatIndex`] is built from.
    Stats,
    /// Currency display names and exchange ids.
    Static,
    /// Every base, unique and gem searchable by name.
    Items,
}

impl TradeData {
    fn name(self) -> &'static str {
        match self {
            TradeData::Stats => "stats",
            TradeData::Static => "static",
            TradeData::Items => "items",
        }
    }

    /// Whether `body` really is this catalog, with something in it. A 200
    /// can carry a challenge page or an empty result, and a cache must
    /// never be filled with either.
    pub fn validate(self, body: &str) -> Result<(), TradeError> {
        let bad = |why: &str| TradeError::BadData(self.name(), why.to_string());
        let v: serde_json::Value =
            serde_json::from_str(body).map_err(|e| TradeError::BadData(self.name(), e.to_string()))?;
        let groups = v.get("result").and_then(|r| r.as_array()).ok_or_else(|| bad("no result list"))?;
        let filled = groups
            .iter()
            .any(|g| g.get("entries").and_then(|e| e.as_array()).is_some_and(|e| !e.is_empty()));
        if !filled {
            return Err(bad("no entries"));
        }
        match self {
            TradeData::Stats => StatIndex::from_json(body).map(|_| ()),
            TradeData::Static if parse_static_currency_ids(body).is_empty() => Err(bad("no currency ids")),
            TradeData::Static | TradeData::Items => Ok(()),
        }
    }
}

/// Trade-site search client. Every call claims a slot from the shared
/// [`Limiters`] first, which are seeded with the live-verified default
/// rules and corrected by every response's rate headers; an active ban
/// surfaces as `TradeError::Cooldown` and no request leaves while one is
/// pending.
pub struct TradeClient {
    base: String,
    league: String,
    http: reqwest::blocking::Client,
    /// POESESSID cookie value; empty = anonymous. Only ever sent to `base`
    /// (pathofexile.com in production), never logged.
    session: String,
    limiters: Limiters,
}

impl TradeClient {
    pub fn new(base: &str, league: &str) -> Result<TradeClient, TradeError> {
        TradeClient::with_limiters(base, league, Limiters::global())
    }

    /// A client drawing on `limiters` instead of the process-wide set: for
    /// a stub server, whose traffic says nothing about the real site's.
    pub fn with_limiters(base: &str, league: &str, limiters: Limiters) -> Result<TradeClient, TradeError> {
        let http = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .user_agent(BROWSER_UA)
            .build()
            .map_err(|e| TradeError::Http(e.to_string()))?;
        Ok(TradeClient {
            base: base.trim_end_matches('/').to_string(),
            league: league.to_string(),
            http,
            session: String::new(),
            limiters,
        })
    }

    pub fn limiters(&self) -> &Limiters {
        &self.limiters
    }

    /// The league every search and exchange request of this client names.
    pub fn league(&self) -> &str {
        &self.league
    }

    /// Points the client at another league. The limiters stay: the site
    /// counts requests per address, whichever league they name, and a
    /// fresh set would forget a cooldown that is still running.
    pub fn set_league(&mut self, league: &str) {
        self.league = league.to_string();
    }

    /// Sets the POESESSID session cookie for the account-backed endpoints
    /// (saved searches need the owning account). Empty clears it; every
    /// request goes back to anonymous.
    pub fn set_session(&mut self, poesessid: &str) {
        self.session = poesessid.trim().to_string();
    }

    pub fn site_url(&self, search_id: &str) -> String {
        format!(
            "{}/trade2/search/poe2/{}/{}",
            self.base.replace("/api", ""),
            self.league,
            search_id
        )
    }

    /// The one way a request leaves: claim a slot, log it, send, take in
    /// the rate headers, log the server's counters, and turn a 429 into a
    /// recorded and logged ban. `with_session` attaches the cookie when one
    /// is set; without one the request keeps its exact anonymous shape.
    /// `league` is the one the request names, for the log.
    fn send(
        &self,
        ep: Endpoint,
        league: &str,
        req: reqwest::blocking::RequestBuilder,
        with_session: bool,
    ) -> Result<reqwest::blocking::Response, TradeError> {
        let authed = with_session && !self.session.is_empty();
        self.limiters.take_turn(ep, authed, MAX_TURN_WAIT).map_err(TradeError::Cooldown)?;
        let req = if authed {
            req.header(reqwest::header::COOKIE, format!("POESESSID={}", self.session))
        } else {
            req
        };
        let what = ep.name();
        let league = if league.is_empty() { "-" } else { league };
        log_request(&format!("trade request: {what} {league} policy={}", self.limiters.policy_name(ep)));
        let resp = req.send().map_err(|e| TradeError::Http(e.to_string()))?;
        self.limiters.absorb(ep, authed, resp.headers());
        let status = resp.status().as_u16();
        let header = |name: &str| resp.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
        log_request(&format!("trade response: {what} {status} {}", describe_rate_headers(&header)));
        if status == 429 {
            let retry_after = resp.headers().get(reqwest::header::RETRY_AFTER).and_then(|v| v.to_str().ok());
            let ban = self.limiters.on_429(ep, authed, retry_after);
            log_request(&format!(
                "trade 429: {what} banned {}s (retry-after {}, state {}) after request {what} {league}",
                ban.as_secs_f64().ceil(),
                retry_after.map_or("none", str::trim),
                describe_rate_states(&header),
            ));
            return Err(TradeError::Cooldown(ban));
        }
        Ok(resp)
    }

    /// The body of a successful response, or the API's own error text.
    fn body_of(what: &str, resp: reqwest::blocking::Response) -> Result<String, TradeError> {
        let status = resp.status();
        let body = resp.text().map_err(|e| TradeError::Http(e.to_string()));
        if !status.is_success() {
            let why = api_error(what, status.as_u16(), &body.unwrap_or_default());
            // A 429 never gets here: `send` turned it into a cooldown.
            return Err(if status.is_client_error() { TradeError::Refused(why) } else { TradeError::Http(why) });
        }
        body
    }

    pub fn search(&mut self, query: &Query) -> Result<SearchResult, TradeError> {
        let url = format!("{}/api/trade2/search/poe2/{}", self.base, self.league);
        let body = search_body(query, self.session.is_empty())?;
        let resp = self.send(Endpoint::Search, &self.league, self.http.post(&url).json(&body), true)?;
        parse_search(&Self::body_of("search", resp)?)
    }

    /// One data catalog, fetched through the limiter and checked before it
    /// is handed back: a body that is not the catalog is an error, never an
    /// empty default. Sent without the session cookie; the catalogs are
    /// public.
    pub fn data_json(&self, kind: TradeData) -> Result<String, TradeError> {
        let url = format!("{}/api/trade2/data/{}", self.base, kind.name());
        let resp = self.send(Endpoint::Data, "", self.http.get(&url), false)?;
        let body = Self::body_of(kind.name(), resp)?;
        kind.validate(&body)?;
        Ok(body)
    }

    /// The catalog from `path` when that file holds a valid copy younger
    /// than `max_age`, else downloaded, validated, and only then written
    /// over the file (atomically, so a crash mid-write cannot leave a torn
    /// cache behind). When the download fails, an older valid copy is still
    /// better than nothing and is returned; with no usable copy the failure
    /// is the result. A cache that cannot be written costs a re-download
    /// next launch, not the data in hand.
    pub fn cached_data(
        &self,
        kind: TradeData,
        path: &std::path::Path,
        max_age: std::time::Duration,
    ) -> Result<String, TradeError> {
        let cached = std::fs::read_to_string(path).ok().filter(|body| kind.validate(body).is_ok());
        let fresh = std::fs::metadata(path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|at| at.elapsed().ok())
            .is_some_and(|age| age <= max_age);
        if let (Some(body), true) = (&cached, fresh) {
            return Ok(body.clone());
        }
        match self.data_json(kind) {
            Ok(body) => {
                if let Err(e) = crate::ninja::write_cache_atomic(path, body.as_bytes()) {
                    eprintln!("trade {} cache not written: {e}", kind.name());
                }
                Ok(body)
            }
            Err(e) => cached.ok_or(e),
        }
    }

    /// Fetches the trade static-data currency map (display name -> currency
    /// id), for resolving an item's name to its exchange `want` id.
    pub fn static_currency_ids(&self) -> Result<HashMap<String, String>, TradeError> {
        Ok(parse_static_currency_ids(&self.data_json(TradeData::Static)?))
    }

    /// The currency exchange's answer for `want` priced in `have`, as the
    /// API sent it: the offers keyed by listing id, each with its stock and
    /// seller. The bulk view reads those; [`exchange`](Self::exchange)
    /// boils the same body down to a rate.
    pub fn exchange_raw(&mut self, want_id: &str, have_id: &str) -> Result<serde_json::Value, TradeError> {
        let url = format!("{}/api/trade2/exchange/{}", self.base, self.league);
        let body = serde_json::json!({
            "engine": "new",
            "query": {"status": {"option": "online"}, "have": [have_id], "want": [want_id]},
            "sort": {"have": "asc"},
        });
        let resp = self.send(Endpoint::Exchange, &self.league, self.http.post(&url).json(&body), true)?;
        Ok(serde_json::from_str(&Self::body_of("exchange", resp)?)?)
    }

    /// Prices a stackable currency (e.g. an omen) via the trade exchange:
    /// returns how many `have` currency one `want` unit costs (see
    /// [`parse_exchange_rate`]), or `None` when there are no live offers.
    /// Covers the currency-exchange items poe.ninja does not track for PoE2.
    pub fn exchange(&mut self, want_id: &str, have_id: &str) -> Result<Option<f64>, TradeError> {
        Ok(exchange_rate_of(&self.exchange_raw(want_id, have_id)?))
    }

    /// Fetches the exact gem base-type names (from data/items), for
    /// matching OCR'd skill names to a searchable `type`.
    pub fn gem_types(&self) -> Result<Vec<String>, TradeError> {
        Ok(parse_gem_types(&self.items_json()?))
    }

    /// The raw `/api/trade2/data/items` catalog: every base, unique and gem
    /// the trade site can search by name.
    pub fn items_json(&self) -> Result<String, TradeError> {
        self.data_json(TradeData::Items)
    }

    /// Prices a specific cut skill gem at an exact level: searches by gem name
    /// + level, then fetches the cheapest listings. Returns them (price amount
    /// + currency); the caller converts to exalted via its currency table.
    pub fn price_gem(&mut self, skill: &str, level: i64) -> Result<Vec<Listing>, TradeError> {
        let sr = self.search(&build_gem_query(skill, level))?;
        if sr.hashes.is_empty() {
            return Ok(Vec::new());
        }
        self.fetch(&sr.id, &sr.hashes)
    }

    /// The priced listings behind `hashes` (the first ten). See
    /// [`fetch_counted`](Self::fetch_counted) for how many were left out.
    pub fn fetch(&mut self, search_id: &str, hashes: &[String]) -> Result<Vec<Listing>, TradeError> {
        Ok(self.fetch_counted(search_id, hashes)?.listings)
    }

    /// [`fetch`](Self::fetch), plus the number of requested listings that
    /// came back gone or unpriced.
    pub fn fetch_counted(&mut self, search_id: &str, hashes: &[String]) -> Result<FetchOutcome, TradeError> {
        let ids: Vec<&str> = hashes.iter().take(10).map(String::as_str).collect();
        let url = format!(
            "{}/api/trade2/fetch/{}?query={}",
            self.base,
            ids.join(","),
            search_id
        );
        let resp = self.send(Endpoint::Fetch, &self.league, self.http.get(&url), true)?;
        parse_fetch(&Self::body_of("fetch", resp)?)
    }

    /// Result-id list of a saved trade search: GET the saved query json from
    /// `/api/trade2/search/poe2/{league}/{id}` (needs the session cookie -
    /// saved searches belong to an account), then re-POST that query through
    /// the normal search endpoint. The first page of ids is enough for the
    /// live-search differ: a poll only needs to see what is new at the top.
    /// Both requests count against the search limiter - the GET hits the same
    /// rate-limit policy as the POST (verified: the trade site serves both
    /// from the same `/api/trade2/search` family).
    pub fn saved_search_ids(&mut self, league: &str, id: &str) -> Result<Vec<String>, TradeError> {
        let url = format!("{}/api/trade2/search/poe2/{}/{}", self.base, league, id);
        let resp = self.send(Endpoint::Search, league, self.http.get(&url), true)?;
        let body = parse_saved_query(&Self::body_of("saved search", resp)?)?;

        let post_url = format!("{}/api/trade2/search/poe2/{}", self.base, league);
        let resp = self.send(Endpoint::Search, league, self.http.post(&post_url).json(&body), true)?;
        Ok(parse_search(&Self::body_of("search", resp)?)?.hashes)
    }
}

/// Extracts the re-POSTable search body from a saved-search GET response:
/// `{"query": ..., "sort": ...}`, with the default price sort filled in when
/// the saved search stored none (the POST endpoint requires a sort).
pub fn parse_saved_query(body: &str) -> Result<serde_json::Value, TradeError> {
    let v: serde_json::Value = serde_json::from_str(body)?;
    let query = v
        .get("query")
        .cloned()
        .ok_or_else(|| TradeError::Http("saved search response has no query".into()))?;
    let sort = v
        .get("sort")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({"price": "asc"}));
    Ok(serde_json::json!({"query": query, "sort": sort}))
}

/// Decodes `%XX` percent-escapes byte-wise (league names carry `%20`s in
/// trade URLs). Malformed escapes pass through literally rather than erroring:
/// a pasted URL should parse as far as it can.
fn percent_decode(s: &str) -> String {
    fn hex(b: u8) -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        }
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(hi * 16 + lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Parses a pasted trade-site search URL,
/// `https://www.pathofexile.com/trade2/search/poe2/{league}/{id}`, into
/// (league, search id), URL-decoding the league ("Runes%20of%20Aldur" ->
/// "Runes of Aldur"). Tolerates a trailing slash and query/fragment suffixes;
/// anything not shaped like a two-segment poe2 search path is `None`, which
/// the settings UI surfaces as a bad-URL hint.
pub fn parse_search_url(url: &str) -> Option<(String, String)> {
    let rest = url.trim().split_once("/trade2/search/poe2/")?.1;
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    let mut parts = rest.split('/').filter(|p| !p.is_empty());
    let league = percent_decode(parts.next()?);
    let id = parts.next()?.to_string();
    if parts.next().is_some() || league.is_empty() || id.is_empty() {
        return None;
    }
    Some((league, id))
}

#[cfg(test)]
mod search_url_tests {
    use super::parse_search_url;

    #[test]
    fn parses_the_canonical_search_url_and_decodes_the_league() {
        assert_eq!(
            parse_search_url(
                "https://www.pathofexile.com/trade2/search/poe2/Runes%20of%20Aldur/AbCd12eF"
            ),
            Some(("Runes of Aldur".to_string(), "AbCd12eF".to_string()))
        );
    }

    #[test]
    fn tolerates_trailing_slash_and_query_suffix() {
        assert_eq!(
            parse_search_url("https://www.pathofexile.com/trade2/search/poe2/Standard/xYz9/"),
            Some(("Standard".to_string(), "xYz9".to_string()))
        );
        assert_eq!(
            parse_search_url("https://www.pathofexile.com/trade2/search/poe2/Standard/xYz9?a=1"),
            Some(("Standard".to_string(), "xYz9".to_string()))
        );
    }

    #[test]
    fn rejects_non_search_urls() {
        // Missing id, wrong path family, extra segments, and plain junk all
        // fail closed - the UI marks these red instead of silently polling
        // a nonsense endpoint.
        assert_eq!(parse_search_url("https://www.pathofexile.com/trade2/search/poe2/Standard"), None);
        assert_eq!(parse_search_url("https://www.pathofexile.com/trade/search/League/abc"), None);
        assert_eq!(
            parse_search_url("https://www.pathofexile.com/trade2/search/poe2/a/b/c"),
            None
        );
        assert_eq!(parse_search_url("not a url"), None);
        assert_eq!(parse_search_url(""), None);
    }
}

/// Fetches the live stats catalog (no session needed; requires a
/// browser-like User-Agent past Cloudflare), through the shared limiter.
/// Prefer [`TradeClient::cached_data`], which also keeps it on disk.
pub fn fetch_stats_json() -> Result<String, TradeError> {
    TradeClient::new(TRADE_BASE, "")?.data_json(TradeData::Stats)
}
