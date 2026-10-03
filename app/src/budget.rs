//! Whether a request the user did not ask for may spend one of the trade
//! site's search slots. Reward-row currency and gem pricing are sent on the
//! overlay's own initiative, and the site counts them against the same
//! limits as the user's F7: left alone, a panel of unknown rewards could
//! spend the whole window and leave the user's next check waiting on a
//! cooldown. So a background request goes only while the user keeps more
//! than ten free slots after it, at most six go per five minutes, and none
//! goes within a minute of a request the user made.
//!
//! Pure: the clock is passed in, nothing here reads it.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Free slots the user must still have after a background request.
pub const MIN_FREE_AFTER: u32 = 10;
/// Background requests allowed per [`WINDOW`].
pub const MAX_PER_WINDOW: usize = 6;
/// The trailing window [`MAX_PER_WINDOW`] counts over.
pub const WINDOW: Duration = Duration::from_secs(5 * 60);
/// Quiet time after a user request before a background one may go.
pub const USER_QUIET: Duration = Duration::from_secs(60);

/// The background requests sent so far, and the last user request seen.
///
/// A refused request is dropped, not queued: nothing here remembers it, and
/// the caller shows the row as unpriced ("...") until a later look finds
/// room. Queueing would send a burst the moment the window opened, right
/// when the user is likely to want the slots back.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Budget {
    sent: VecDeque<Instant>,
    last_user: Option<Instant>,
}

impl Budget {
    /// True when a background request may be sent now, and records it as
    /// sent. `free_slots` is the fewest free slots over every window of the
    /// search policy from the server's last counters; `last_user_request_at`
    /// is the caller's own record of the user's last request, if it keeps
    /// one (requests noted through [`note_user`](Self::note_user) count too).
    pub fn allow_background(
        &mut self,
        now: Instant,
        free_slots: u32,
        last_user_request_at: Option<Instant>,
    ) -> bool {
        if free_slots.saturating_sub(1) <= MIN_FREE_AFTER {
            return false;
        }
        let last_user = match (self.last_user, last_user_request_at) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        if last_user.is_some_and(|at| now.saturating_duration_since(at) < USER_QUIET) {
            return false;
        }
        if self.sent_in_window(now) >= MAX_PER_WINDOW {
            return false;
        }
        self.note_background(now);
        true
    }

    /// A request the user made; background requests wait a minute after it.
    pub fn note_user(&mut self, now: Instant) {
        self.last_user = Some(self.last_user.map_or(now, |at| at.max(now)));
    }

    /// A background request the caller sent; counts against the window.
    pub fn note_background(&mut self, now: Instant) {
        self.sent.push_back(now);
        self.forget_old(now);
    }

    /// Background requests sent within the trailing window.
    pub fn sent_in_window(&mut self, now: Instant) -> usize {
        self.forget_old(now);
        self.sent.len()
    }

    fn forget_old(&mut self, now: Instant) {
        while self.sent.front().is_some_and(|at| now.saturating_duration_since(*at) >= WINDOW) {
            self.sent.pop_front();
        }
    }
}
