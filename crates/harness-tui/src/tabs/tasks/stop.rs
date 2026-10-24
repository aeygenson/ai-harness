//! The user's stop buttons on the message box while roles work: «Stop after this
//! step» (s; pressed again, it is taken back) and «Stop now» (x, after a
//! confirmation the app asks for). Both end with the task waiting for them.

use harness_core::task::stop_request::{StopRequest, StopWhen};

use super::tab::TasksTab;
use crate::ui::i18n::I18n;
use crate::ui::ButtonId;

impl TasksTab {
    /// The stop request of the running work, while it runs roles.
    fn stop_request(&self) -> Option<&StopRequest> {
        self.running.as_ref().and_then(|r| r.stop.as_ref())
    }

    /// Can the user stop the work now? Only while roles work, and not again
    /// once «Stop now» is asked.
    pub fn can_stop(&self) -> bool {
        self.stop_request().is_some_and(|stop| !stop.now())
    }

    /// What the user has asked for, if anything.
    pub fn stop_asked(&self) -> Option<StopWhen> {
        self.stop_request().and_then(StopRequest::asked)
    }

    /// «Stop after this step»: asks for it, or takes it back. The message
    /// for the bottom line; `None` when no role works.
    pub fn toggle_stop_after_step(&mut self, tr: &I18n) -> Option<String> {
        if !self.can_stop() {
            return None;
        }
        let asked = self.stop_request()?.toggle_after_step();
        let key = match asked {
            Some(StopWhen::AfterStep) => "tasks.stop_after_asked",
            None => "tasks.stop_after_cancelled",
            Some(StopWhen::Now) => "tasks.stop_now_asked",
        };
        let text = tr.t(key).to_string();
        self.log.push(format!("── {text}"));
        Some(text)
    }

    /// «Stop now», once the user has confirmed it. The message for the bottom
    /// line; `None` when no role works any more.
    pub fn stop_now(&mut self, tr: &I18n) -> Option<String> {
        if !self.can_stop() {
            return None;
        }
        self.stop_request()?.stop_now();
        let text = tr.t("tasks.stop_now_asked").to_string();
        self.log.push(format!("── {text}"));
        Some(text)
    }

    /// The stop buttons for the message box, as (label, button); none while
    /// no role works. «Stop after this step» shows when it is asked.
    pub(super) fn stop_buttons(&self, tr: &I18n) -> Vec<(String, ButtonId)> {
        if !self.can_stop() {
            return vec![];
        }
        let after = match self.stop_asked() {
            None => tr.t("tasks.stop_after"),
            Some(StopWhen::AfterStep) => tr.t("tasks.stop_after_on"),
            // Stopping already: nothing more to ask.
            Some(StopWhen::Now) => return vec![],
        };
        vec![
            (after.to_string(), ButtonId::StopAfterStep),
            (tr.t("tasks.stop_now").to_string(), ButtonId::StopNow),
        ]
    }
}
