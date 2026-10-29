//! The time limit of an agent run, counted so that time the computer slept
//! counts too.

use std::time::{Duration, Instant, SystemTime};

/// How often [`time_limit_passed`] looks at the ordinary clock for the time limit. After the
/// computer wakes up, a role over its limit is stopped within this time;
/// limits are minutes long, so a few seconds late does not matter.
const CLOCK_CHECK_PAUSE: Duration = Duration::from_secs(5);

/// Ends once `limit` has passed. Tokio's timers count on a clock that stands
/// still while the computer sleeps (Linux, macOS): after four hours of sleep a
/// 150-minute limit would still be far away, and a hung agent would go on
/// waiting. So the ordinary clock is looked at too, every
/// [`CLOCK_CHECK_PAUSE`], and whichever clock shows more time counts.
pub(super) async fn time_limit_passed(limit: Duration) {
    let started = Instant::now();
    let started_on_clock = SystemTime::now();
    loop {
        let left = time_left(limit, started.elapsed(), started_on_clock.elapsed().ok());
        if left.is_zero() {
            return;
        }
        tokio::time::sleep(left.min(CLOCK_CHECK_PAUSE)).await;
    }
}

/// What is left of `limit` after `waited` (the clock that stops in sleep) or
/// `on_clock` (the ordinary clock), whichever is more. `on_clock` is `None`
/// when the ordinary clock was set back: then only `waited` counts.
fn time_left(limit: Duration, waited: Duration, on_clock: Option<Duration>) -> Duration {
    let passed = on_clock.map_or(waited, |on_clock| on_clock.max(waited));
    limit.saturating_sub(passed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_time_limit_counts_the_time_the_computer_slept() {
        let limit = Duration::from_mins(150);
        let ten_minutes = Duration::from_mins(10);
        let four_hours = Duration::from_hours(4);

        // Ten minutes awake, then four hours asleep: the limit has passed.
        let after_sleep = time_left(limit, ten_minutes, Some(four_hours));
        // The ordinary clock was set back: the clock that stops in sleep counts.
        let clock_set_back = time_left(limit, ten_minutes, None);
        let awake = time_left(limit, ten_minutes, Some(ten_minutes));

        assert_eq!(after_sleep, Duration::ZERO);
        assert_eq!(clock_set_back, Duration::from_mins(140));
        assert_eq!(awake, Duration::from_mins(140));
    }

    #[tokio::test]
    async fn a_short_limit_ends() {
        let start = Instant::now();

        time_limit_passed(Duration::from_millis(50)).await;

        assert!(start.elapsed() >= Duration::from_millis(50));
    }
}
