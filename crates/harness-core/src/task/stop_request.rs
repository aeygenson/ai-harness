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
//! Another program (`harness stop`, the MCP server) cannot reach that memory:
//! it writes the request into the run's stop file instead (see
//! [`super::run_lock`]), and the request reads that file too.
//!
//! Nothing is sent back: the run ends as usual and says why it stopped.

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

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

impl StopWhen {
    /// The word for this request in a stop file: `after_step` or `now`.
    pub fn word(self) -> &'static str {
        match self {
            StopWhen::AfterStep => "after_step",
            StopWhen::Now => "now",
        }
    }

    /// The request a stop file holds; `None` for any other text.
    pub fn from_word(text: &str) -> Option<Self> {
        match text.trim() {
            "after_step" => Some(StopWhen::AfterStep),
            "now" => Some(StopWhen::Now),
            _ => None,
        }
    }
}

/// A stop request shared between the screen and the run.
///
/// `clone` gives another handle to the **same** request (`Arc`: shared
/// ownership between threads; `Mutex`: one thread changes it at a time).
/// A new request asks for nothing.
#[derive(Debug, Clone, Default)]
pub struct StopRequest {
    asked: Arc<Mutex<Option<StopWhen>>>,
    /// The stop file another program may write, once the run knows it.
    /// `OnceLock`: set at most once, then read by every clone without a lock.
    file: Arc<OnceLock<PathBuf>>,
}

impl StopRequest {
    /// A request that asks for nothing yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Also takes requests another program writes into `file`. A request
    /// that already reads a file keeps its first one.
    pub fn also_read(&self, file: PathBuf) {
        // `set` fails only when a file is already set; that one stays.
        let _ = self.file.set(file);
    }

    /// What was asked, here or in the stop file, if anything. «Now» wins.
    pub fn asked(&self) -> Option<StopWhen> {
        let pressed = *self.lock();
        // The agent's process asks every fraction of a second; reading a
        // file that small (or not finding it) costs next to nothing.
        let written = self
            .file
            .get()
            .and_then(|file| fs::read_to_string(file).ok())
            .and_then(|text| StopWhen::from_word(&text));
        match (pressed, written) {
            (Some(StopWhen::Now), _) | (_, Some(StopWhen::Now)) => Some(StopWhen::Now),
            (pressed, written) => pressed.or(written),
        }
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

    #[test]
    fn a_request_written_into_the_stop_file_is_seen_by_every_clone() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("run.stop");
        let request = StopRequest::new();
        let run = request.clone();
        request.also_read(file.clone());
        assert_eq!(run.asked(), None, "no file yet: nothing asked");

        std::fs::write(&file, "after_step\n").unwrap();
        assert_eq!(run.asked(), Some(StopWhen::AfterStep));

        std::fs::write(&file, "now").unwrap();
        assert!(run.now());
    }

    #[test]
    fn stop_now_in_the_file_wins_over_after_this_step_on_the_screen() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("run.stop");
        std::fs::write(&file, "now").unwrap();
        let request = StopRequest::new();
        request.toggle_after_step();

        request.also_read(file);

        assert_eq!(request.asked(), Some(StopWhen::Now));
    }

    #[test]
    fn a_stop_file_with_other_text_asks_for_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("run.stop");
        std::fs::write(&file, "please stop").unwrap();
        let request = StopRequest::new();

        request.also_read(file);

        assert_eq!(request.asked(), None);
        assert_eq!(
            StopWhen::from_word(StopWhen::Now.word()),
            Some(StopWhen::Now)
        );
        assert_eq!(
            StopWhen::from_word(StopWhen::AfterStep.word()),
            Some(StopWhen::AfterStep)
        );
    }
}
