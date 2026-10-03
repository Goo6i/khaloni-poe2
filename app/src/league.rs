//! One league at a time. The price table, the trade client, the answers
//! cached from both and the listings on screen each belong to a league; a
//! change in Settings has to move all of them or none. Moving some is how
//! one league's listings get converted at another league's rates, and that
//! number looks exactly like a right one.
//!
//! The order of a switch ([`switch`]), all on the main loop's thread:
//! 1. [`Current::set`]: from here a worker that finishes an old-league
//!    request finds the league changed and does not store its answer
//!    ([`store_if_still`]), and the trade worker retargets before its next
//!    request ([`retarget`]).
//! 2. [`Priced::drop_all`]: every answer already held is forgotten. Before
//!    the table moves, so that nothing can be priced from the new table
//!    and an old cached answer together; what is priced in this gap is
//!    priced wholly in the old league, carries its name, and is refused
//!    when it arrives ([`Current::is`]).
//! 3. `PriceService::set_league`: the old table stops being served at once;
//!    an empty snapshot marked loading stands in until the new one lands.
//! 4. [`Announcer`]: "league: X" when the new table has loaded, or the
//!    reason it cannot.
//!
//! No I/O here.

use std::hash::Hash;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use khaloni_poe2_core::trade::TradeClient;

use crate::appraise::{AsyncCache, Retrying, ReuseSlot};
use crate::hover::HoverState;
use crate::prices::{PriceService, Snapshot};
use crate::stabilize::Stabilizer;

/// The league the overlay prices in: written by the main loop, followed by
/// the trade worker.
#[derive(Debug, Clone)]
pub struct Current(Arc<Mutex<String>>);

/// A league change that happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Switch {
    pub from: String,
    pub to: String,
}

impl Current {
    pub fn new(league: &str) -> Current {
        Current(Arc::new(Mutex::new(league.to_string())))
    }

    pub fn name(&self) -> String {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn is(&self, league: &str) -> bool {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) == league
    }

    /// `None` when `league` is already current: saving Settings without
    /// touching the league must not throw the caches away.
    pub fn set(&self, league: &str) -> Option<Switch> {
        let mut held = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if *held == league {
            return None;
        }
        let from = std::mem::replace(&mut *held, league.to_string());
        Some(Switch { from, to: league.to_string() })
    }
}

/// Points the trade worker's client at `league` and forgets the last
/// search, whose listings are the old league's. The client is retargeted,
/// not rebuilt, so it keeps its limiters and its session. False when the
/// client already names `league` (or is not loaded yet: it will be built
/// for the current league when it is).
pub fn retarget<K: PartialEq, V: Clone>(
    client: &mut Retrying<TradeClient>,
    last_search: &mut ReuseSlot<K, V>,
    league: &str,
) -> bool {
    let moved = match client.get_mut() {
        Some(c) if c.league() != league => {
            c.set_league(league);
            true
        }
        _ => false,
    };
    // Cleared whether or not a client existed: a slot can only hold what an
    // earlier client fetched.
    last_search.clear();
    moved
}

/// Stores a worker's answer unless the league moved on while it was being
/// fetched. The check happens under the cache's own lock, and a switch sets
/// the league before it clears the cache, so an old answer is either
/// refused here or cleared there.
pub fn store_if_still<K: Eq + Hash + Clone, V: Clone>(
    current: &Current,
    fetched_in: &str,
    cache: &Mutex<AsyncCache<K, V>>,
    key: K,
    outcome: Result<V, (String, Duration)>,
    now: Instant,
) -> bool {
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    if !current.is(fetched_in) {
        // The request is over either way; leaving it marked in flight
        // would keep the key from ever being asked again.
        cache.unsent(&key);
        return false;
    }
    cache.store(key, outcome, now);
    true
}

/// Everything on the main loop's side that holds a price fetched in one
/// league. The template store is not here: it keeps what a reward row IS
/// (item and count), and the price is looked up fresh on every frame.
pub struct Priced<'a> {
    pub currency: &'a Mutex<AsyncCache<String, Option<f64>>>,
    pub gems: &'a Mutex<AsyncCache<(String, u32), Option<f64>>>,
    /// Reward rows with the amounts they were priced at.
    pub stabilizer: &'a mut Stabilizer,
    /// The popup on screen and the lookups it is waiting on.
    pub hover: &'a mut HoverState,
    /// A hover check waiting on a row's exchange request.
    pub awaiting_exchange: &'a mut Option<(String, u32)>,
}

impl Priced<'_> {
    pub fn drop_all(self) {
        self.currency.lock().unwrap_or_else(|e| e.into_inner()).clear();
        self.gems.lock().unwrap_or_else(|e| e.into_inner()).clear();
        self.stabilizer.clear();
        *self.hover = HoverState::default();
        *self.awaiting_exchange = None;
    }
}

/// The whole switch, in the order the module comment gives. `None` when
/// `to` is already the league: nothing is dropped for a Settings save that
/// left the league alone.
pub fn switch(
    current: &Current,
    priced: Priced<'_>,
    prices: &PriceService,
    announcer: &mut Announcer,
    to: &str,
) -> Option<Switch> {
    let switch = current.set(to)?;
    priced.drop_all();
    prices.set_league(to);
    announcer.switched(to);
    Some(switch)
}

/// What a price lookup says while its league's table is on the way.
pub fn loading_text(league: &str) -> String {
    format!("loading {league} prices")
}

/// Why nothing may be priced from `snap` for `league`, if so: the table is
/// another league's (a switch is a few lines from done) or still loading.
/// A league whose fetch has failed outright is let through: its listings
/// are still worth showing unconverted, as they were before leagues could
/// change.
pub fn not_ready(snap: &Snapshot, league: &str) -> Option<String> {
    (snap.league != league || (snap.loading && snap.error.is_none())).then(|| loading_text(league))
}

/// The status of a card whose listings came from `listed_in` once the
/// overlay prices in another league.
pub fn old_league_status(listed_in: &str, status: &str) -> String {
    let mark = format!("league {listed_in}");
    if status.starts_with(&mark) {
        status.to_string()
    } else if status.is_empty() {
        mark
    } else {
        format!("{mark} - {status}")
    }
}

/// The status of a search that ran from a card whose previous listings
/// were another league's: the new ones are the current league's, and the
/// line says so.
pub fn searched_status(status: &str, previous: Option<&str>, now: &str) -> String {
    match previous {
        Some(p) if p != now => format!("{status} - league {now}"),
        _ => status.to_string(),
    }
}

/// Says "league: X" once, when X's table has loaded, and the reason once
/// if it cannot load.
#[derive(Debug, Default)]
pub struct Announcer {
    awaiting: Option<String>,
    error_said: bool,
}

impl Announcer {
    pub fn switched(&mut self, to: &str) {
        self.awaiting = Some(to.to_string());
        self.error_said = false;
    }

    pub fn poll(&mut self, snap: &Snapshot) -> Option<String> {
        let league = self.awaiting.as_deref()?;
        if snap.league != league {
            return None;
        }
        if !snap.loading {
            let said = format!("league: {league}");
            self.awaiting = None;
            return Some(said);
        }
        match &snap.error {
            Some(e) if !self.error_said => {
                self.error_said = true;
                Some(e.clone())
            }
            _ => None,
        }
    }
}
