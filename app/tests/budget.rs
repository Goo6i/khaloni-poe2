//! The rules a background (reward-row) request must pass before it may spend
//! one of the trade site's search slots: the user keeps more than ten slots
//! after it, no more than six go per five minutes, and none goes within a
//! minute of something the user asked for.

// Pulled in by path until the module is registered in the crate; the
// parts a test does not call are not dead.
#[allow(dead_code)]
#[path = "../src/budget.rs"]
mod budget;

use std::time::{Duration, Instant};

use budget::{Budget, MAX_PER_WINDOW, MIN_FREE_AFTER, USER_QUIET, WINDOW};

#[test]
fn a_background_request_needs_more_than_ten_free_slots() {
    let now = Instant::now();
    let mut b = Budget::default();
    assert_eq!(MIN_FREE_AFTER, 10);
    // Eleven free: this request would leave exactly ten, which is not more
    // than ten. Twelve free leaves eleven.
    assert!(!b.allow_background(now, 11, None));
    assert!(!b.allow_background(now, 0, None));
    assert!(b.allow_background(now, 12, None));
    assert_eq!(b.sent_in_window(now), 1);
}

#[test]
fn at_most_six_background_requests_per_five_minutes() {
    let start = Instant::now();
    let mut b = Budget::default();
    assert_eq!((MAX_PER_WINDOW, WINDOW), (6, Duration::from_secs(5 * 60)));
    for i in 0..6 {
        let now = start + Duration::from_secs(i * 10);
        assert!(b.allow_background(now, 30, None), "request {} of six", i + 1);
    }
    // The seventh in the same window is refused.
    let now = start + Duration::from_secs(70);
    assert!(!b.allow_background(now, 30, None));
    assert_eq!(b.sent_in_window(now), 6);
    // Once the first one is five minutes old there is room for one more,
    // and only one: the second is still 4 min 50 s old.
    let now = start + WINDOW;
    assert!(b.allow_background(now, 30, None));
    assert!(!b.allow_background(now, 30, None));
    // A request the caller sent on its own counts the same.
    let mut b = Budget::default();
    for _ in 0..6 {
        b.note_background(start);
    }
    assert!(!b.allow_background(start + Duration::from_secs(1), 30, None));
}

#[test]
fn none_within_a_minute_of_a_user_request() {
    let start = Instant::now();
    let mut b = Budget::default();
    assert_eq!(USER_QUIET, Duration::from_secs(60));
    // The caller passes the time of the last user request.
    assert!(!b.allow_background(start + Duration::from_secs(59), 30, Some(start)));
    assert!(b.allow_background(start + Duration::from_secs(60), 30, Some(start)));
    // The budget also remembers user requests it was told about, so a
    // caller that tracks nothing itself gets the same answer.
    let mut b = Budget::default();
    b.note_user(start);
    assert!(!b.allow_background(start + Duration::from_secs(30), 30, None));
    assert!(b.allow_background(start + Duration::from_secs(61), 30, None));
    // The more recent of the two is what counts.
    let mut b = Budget::default();
    b.note_user(start);
    let later = start + Duration::from_secs(100);
    assert!(!b.allow_background(later + Duration::from_secs(10), 30, Some(later)));
}

#[test]
fn a_background_request_that_would_wait_is_dropped_not_queued() {
    let start = Instant::now();
    let mut b = Budget::default();
    // Refused for the user's slots: nothing is remembered about it.
    assert!(!b.allow_background(start, 5, None));
    assert_eq!(b.sent_in_window(start), 0);
    // Refused for a recent user request: likewise.
    assert!(!b.allow_background(start, 30, Some(start)));
    assert_eq!(b.sent_in_window(start), 0);
    // The moment there is room, a request goes through at once, and only
    // the one that is asked for now: the refused ones were never queued.
    let now = start + USER_QUIET;
    assert!(b.allow_background(now, 30, Some(start)));
    assert_eq!(b.sent_in_window(now), 1);
    let mut expected = Budget::default();
    expected.note_background(now);
    assert_eq!(b, expected);
}
