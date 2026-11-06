//! Work that another window (or `harness run` in a terminal) does in this
//! project. The tab does not start a second run then: «Send» turns grey and
//! says which task runs elsewhere. The stop buttons still work: they ask
//! that harness to stop through its stop file, as `harness stop` does.

use harness_core::git::Repo;
use harness_core::task::run_lock::{self, RunLockError};
use harness_core::task::stop_request::StopWhen;

use super::tab::TasksTab;
use crate::ui::i18n::I18n;

impl TasksTab {
    /// Reads again which task another harness runs in the project, and
    /// what stop was already asked of it. While this tab runs roles itself,
    /// that is its own run, not another one.
    pub(super) fn refresh_elsewhere(&mut self) {
        self.elsewhere = if self.running.is_some() {
            None
        } else {
            Repo::open(&self.root)
                .ok()
                .and_then(|repo| run_lock::working(&repo).ok().flatten())
        };
    }

    /// The task another harness runs here and can stop, unless «Stop now»
    /// was already asked of it.
    pub(super) fn stoppable_elsewhere(&self) -> Option<&str> {
        let working = self.elsewhere.as_ref().filter(|w| w.can_stop)?;
        (working.asked != Some(StopWhen::Now)).then_some(working.task.as_str())
    }

    /// Asks the other harness to stop `when`. The message for the bottom
    /// line: what was asked, or why it could not be.
    pub(super) fn ask_elsewhere(&mut self, when: StopWhen, tr: &I18n) -> Option<String> {
        // What runs, and what was asked of it, may have changed since the
        // last read: another run, or a stop asked from elsewhere.
        self.refresh_elsewhere();
        let task = self.stoppable_elsewhere()?.to_string();
        let asked_before = self.elsewhere.as_ref().and_then(|w| w.asked);
        if when == StopWhen::AfterStep && asked_before == Some(StopWhen::AfterStep) {
            return Some(tr.f("tasks.stop_elsewhere_once", &[("task", &task)]));
        }
        let asked = match Repo::open(&self.root) {
            Ok(repo) => run_lock::ask_to_stop(&repo, &task, when),
            Err(error) => return Some(error.to_string()),
        };
        Some(match asked {
            Ok(()) => {
                // The file says what is asked now («now» is never undone).
                self.refresh_elsewhere();
                tr.f("tasks.stop_elsewhere_asked", &[("task", &task)])
            }
            // It has finished meanwhile; another run may have begun.
            Err(RunLockError::NotRunning(_)) => {
                self.refresh_elsewhere();
                tr.t("tasks.stop_nothing").to_string()
            }
            Err(error) => error.to_string(),
        })
    }
}
