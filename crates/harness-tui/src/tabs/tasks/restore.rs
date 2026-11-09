//! «↶ Restore code here (b)» on the top border of a step: on the next Send,
//! the code outside `.harness/` first goes back to how it was after that step
//! (a new commit, see `harness_core::task::orchestrator`), and the role chosen
//! in «To» gets the task. The same as `harness reject --restore <step>`.

use std::path::{Path, PathBuf};

use harness_core::task::store::Step;
use harness_core::task::Stage;

use super::tab::TasksTab;
use crate::ui::i18n::I18n;
use crate::ui::message::Message;

impl TasksTab {
    /// The selected step, if the code can go back to it: the task waits for
    /// the user, no roles run here, and it is not the last step (the code is
    /// already as after that one).
    fn restore_candidate(&self) -> Option<&Step> {
        if self.is_running() {
            return None;
        }
        let task = self.current()?;
        if !matches!(task.state.stage, Stage::WaitingForHuman(_)) {
            return None;
        }
        let step = task.steps.get(self.step)?;
        (self.step + 1 < task.steps.len()).then_some(step)
    }

    /// «Restore code here» pressed: marks the selected step for the next
    /// Send, or takes the mark back when it is already there.
    pub fn toggle_restore(&mut self, tr: &I18n) -> Message {
        let Some(dir) = self.restore_candidate().map(|step| step.dir.clone()) else {
            return Message::error(tr.t("tasks.restore_not_here"));
        };
        if self.restore_to.as_ref() == Some(&dir) {
            self.restore_to = None;
            return Message::info(tr.t("tasks.restore_cancelled"));
        }
        let text = tr.f("tasks.restore_armed", &[("step", &step_label(&dir))]);
        self.restore_to = Some(dir);
        Message::info(text)
    }

    /// The step the code goes back to on Send, if it belongs to the selected
    /// task and the task still waits for the user; a mark left on another
    /// task does not count.
    pub(super) fn restore_for_send(&self) -> Option<PathBuf> {
        let dir = self.restore_to.as_ref()?;
        let task = self.current()?;
        let waiting = matches!(task.state.stage, Stage::WaitingForHuman(_));
        (waiting && task.steps.iter().any(|step| &step.dir == dir)).then(|| dir.clone())
    }

    /// The label of «Restore code here» for the selected step (✓ when it
    /// is the marked one), or `None` when the code cannot go back to it.
    pub(super) fn restore_label(&self) -> Option<&'static str> {
        let step = self.restore_candidate()?;
        Some(if self.restore_to.as_ref() == Some(&step.dir) {
            "tasks.restore_on"
        } else {
            "tasks.restore"
        })
    }
}

/// The step as the user sees it: `round-01/01-architect`.
pub(super) fn step_label(dir: &Path) -> String {
    let mut parts: Vec<_> = dir.iter().rev().take(2).collect();
    parts.reverse();
    parts
        .iter()
        .map(|part| part.to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_step_is_named_by_its_round_and_folder() {
        let dir = Path::new("/p/.harness/runs/task-001/round-01/01-architect");

        assert_eq!(step_label(dir), "round-01/01-architect");
    }
}
