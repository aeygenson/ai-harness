//! The user's request to stop a running task: «Stop after this step» or «Stop now».
//!
//! The screen (the TUI's thread) and the run (another thread, see
//! `harness-tui`'s runner) share one [`StopRequest`]. The screen writes it
//! when the user presses a button; the run reads it:
//!
//! - the run loop (`orchestrator`) before it starts the next role;
//! - the agent's process (`harness_agents::process::run`) every fraction of a
//!   second while the agent works, so «Stop now» stops it at once.
//!
//! Nothing is sent back: the run ends as usual and says why it stopped.

use std::sync::{Arc, Mutex, PoisonError};

/// When the user wants the task to stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopWhen {
    /// When the running role has finished: its outcome is handled as
    /// usual, and the next role does not start.
    AfterStep,
    /// At once: the running agent is stopped with everything it started,
    /// and its unfinished changes are kept as after a time limit.
    Now,
}

/// A stop request shared between the screen and the run.
///
/// `clone` gives another handle to the **same** request (`Arc`: shared
/// ownership between threads; `Mutex`: one thread changes it at a time).
/// A new request asks for nothing.
#[derive(Debug, Clone, Default)]
pub struct StopRequest {
    asked: Arc<Mutex<Option<StopWhen>>>,
}

impl StopRequest {
    /// A request that asks for nothing yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// What was asked, if anything.
    pub fn asked(&self) -> Option<StopWhen> {
        *self.lock()
    }

    /// «Stop after this step»: asks for it, or takes the request back when
    /// it was already asked. «Stop now» stays as it is. Returns what is
    /// asked afterwards.
    pub fn toggle_after_step(&self) -> Option<StopWhen> {
        let mut asked = self.lock();
        *asked = match *asked {
            None => Some(StopWhen::AfterStep),
            Some(StopWhen::AfterStep) => None,
            Some(StopWhen::Now) => Some(StopWhen::Now),
        };
        *asked
    }

    /// «Stop now»: the running agent stops at once.
    pub fn stop_now(&self) {
        *self.lock() = Some(StopWhen::Now);
    }

    /// Is «Stop now» asked?
    pub fn now(&self) -> bool {
        self.asked() == Some(StopWhen::Now)
    }

    /// The value inside the lock. A thread that panicked while holding the
    /// lock cannot have left a half-written `Option`, so its value is used.
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<StopWhen>> {
        self.asked.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_after_step_is_asked_and_taken_back() {
        let request = StopRequest::new();
        assert_eq!(request.asked(), None);

        assert_eq!(request.toggle_after_step(), Some(StopWhen::AfterStep));
        assert_eq!(request.asked(), Some(StopWhen::AfterStep));
        assert_eq!(request.toggle_after_step(), None);
        assert_eq!(request.asked(), None);
    }

    #[test]
    fn stop_now_wins_and_is_seen_by_every_clone() {
        let request = StopRequest::new();
        let run = request.clone();
        request.toggle_after_step();

        request.stop_now();
        // Pressing «after this step» again does not take «now» back.
        request.toggle_after_step();

        assert!(run.now());
        assert_eq!(run.asked(), Some(StopWhen::Now));
    }
}
