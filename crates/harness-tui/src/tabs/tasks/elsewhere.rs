//! Work that another window (or `harness run` in a terminal) does in this
//! project. The tab does not start a second run then: «Send» turns grey and
//! says which task runs elsewhere. The stop buttons still work: they ask
//! that harness to stop through its stop file, as `harness stop` does.

use harness_core::git::Repo;
use harness_core::task::run_lock;
use harness_core::task::stop_request::StopWhen;

use super::tab::TasksTab;
use crate::ui::i18n::I18n;

impl TasksTab {
    /// Reads again which task another harness runs in the project. While
    /// this tab runs roles itself, that is its own run, not another one.
    pub(super) fn refresh_elsewhere(&mut self) {
        let now = if self.running.is_some() {
            None
        } else {
            Repo::open(&self.root)
                .ok()
                .and_then(|repo| run_lock::working(&repo).ok().flatten())
        };
        let same_task = now.as_ref().map(|w| &w.task) == self.elsewhere.as_ref().map(|w| &w.task);
        if !same_task {
            self.elsewhere_asked = None;
        }
        self.elsewhere = now;
    }

    /// The task another harness runs here and can stop, unless «Stop now»
    /// was already asked of it.
    pub(super) fn stoppable_elsewhere(&self) -> Option<&str> {
        let working = self.elsewhere.as_ref().filter(|w| w.can_stop)?;
        (self.elsewhere_asked != Some(StopWhen::Now)).then_some(working.task.as_str())
    }

    /// Asks the other harness to stop `when`. The message for the bottom
    /// line: what was asked, or why it could not be.
    pub(super) fn ask_elsewhere(&mut self, when: StopWhen, tr: &I18n) -> Option<String> {
        let task = self.stoppable_elsewhere()?.to_string();
        if when == StopWhen::AfterStep && self.elsewhere_asked == Some(StopWhen::AfterStep) {
            return Some(tr.f("tasks.stop_elsewhere_once", &[("task", &task)]));
        }
        let asked = Repo::open(&self.root)
            .map_err(|e| e.to_string())
            .and_then(|repo| run_lock::ask_to_stop(&repo, &task, when).map_err(|e| e.to_string()));
        Some(match asked {
            Ok(()) => {
                self.elsewhere_asked = Some(when);
                tr.f("tasks.stop_elsewhere_asked", &[("task", &task)])
            }
            Err(error) => error,
        })
    }
}
