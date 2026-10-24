//! Waiting for GitHub before a merge: right after the last commits are sent,
//! GitHub needs a while to start the checks (CI), run them and work out
//! conflicts. «Merge pull request» asks again from time to time and merges
//! once everything has passed, instead of making Lisa come back and choose
//! it again.

use std::fmt;
use std::thread;
use std::time::{Duration, Instant};

/// How long «Merge pull request» waits for GitHub at most. The checks of a
/// project on three systems can take twenty minutes; checks that hang should
/// not keep the harness busy for ever.
pub const MERGE_WAIT_LIMIT: Duration = Duration::from_mins(45);

/// How often GitHub is asked again while waiting: soon enough to merge a
/// moment after the checks pass, and about 180 questions an hour, far below
/// GitHub's limit of 5000.
pub const MERGE_WAIT_PAUSE: Duration = Duration::from_secs(20);

/// How often a pause looks at the stop signal, so stopping the harness does
/// not wait for the whole pause.
const STOP_CHECK: Duration = Duration::from_millis(200);

/// How a merge waits while GitHub is still busy with the pull request.
pub struct Waiting<'a> {
    /// The longest wait; zero tries once and does not wait.
    pub limit: Duration,
    /// The time between two questions to GitHub.
    pub pause: Duration,
    /// True once the work should stop (for example the TUI quits).
    pub stopped: Box<dyn FnMut() -> bool + 'a>,
    /// Gets a line saying what the harness is waiting for.
    pub note: Box<dyn FnMut(&str) + 'a>,
}

impl Waiting<'_> {
    /// Tries once and does not wait (the command line, tests).
    pub fn none() -> Self {
        Self {
            limit: Duration::ZERO,
            pause: Duration::ZERO,
            stopped: Box::new(|| false),
            note: Box::new(|_| {}),
        }
    }

    /// Waits one pause because of `why`, which says what GitHub is still
    /// doing. `Err` with the message to show when waiting is over: the limit
    /// is reached or the work was stopped.
    pub(super) fn wait(&mut self, started: Instant, why: String) -> Result<(), String> {
        if self.limit.is_zero() {
            return Err(why);
        }
        let waited = started.elapsed();
        if waited + self.pause > self.limit {
            return Err(format!("{why} (waited {} min)", self.limit.as_secs() / 60));
        }
        (self.note)(&format!(
            "waiting for GitHub ({} of at most {} min): {why}",
            waited.as_secs() / 60,
            self.limit.as_secs() / 60
        ));
        let until = Instant::now() + self.pause;
        while Instant::now() < until {
            if (self.stopped)() {
                return Err(format!("{why}; stopped waiting"));
            }
            thread::sleep(STOP_CHECK.min(until.saturating_duration_since(Instant::now())));
        }
        Ok(())
    }
}

// The closures cannot be printed; the limits are what matters.
impl fmt::Debug for Waiting<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Waiting")
            .field("limit", &self.limit)
            .field("pause", &self.pause)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    #[test]
    fn no_waiting_gives_the_reason_at_once() {
        let mut wait = Waiting::none();

        let error = wait
            .wait(Instant::now(), "checks running".into())
            .unwrap_err();

        assert_eq!(error, "checks running");
    }

    #[test]
    fn a_pause_is_noted_and_the_limit_ends_waiting() {
        let notes = RefCell::new(Vec::new());
        let mut wait = Waiting {
            limit: Duration::from_mins(2),
            pause: Duration::from_millis(10),
            stopped: Box::new(|| false),
            note: Box::new(|line| notes.borrow_mut().push(line.to_string())),
        };

        wait.wait(Instant::now(), "checks running".into()).unwrap();
        let long_ago = Instant::now().checked_sub(Duration::from_mins(2)).unwrap();
        let error = wait.wait(long_ago, "checks running".into()).unwrap_err();

        drop(wait);
        assert_eq!(
            notes.into_inner(),
            ["waiting for GitHub (0 of at most 2 min): checks running"]
        );
        assert_eq!(error, "checks running (waited 2 min)");
    }

    #[test]
    fn stopping_ends_a_pause_early() {
        let started = Instant::now();
        let mut wait = Waiting {
            limit: Duration::from_hours(1),
            pause: Duration::from_mins(30),
            stopped: Box::new(|| true),
            note: Box::new(|_| {}),
        };

        let error = wait.wait(started, "checks running".into()).unwrap_err();

        assert_eq!(error, "checks running; stopped waiting");
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
