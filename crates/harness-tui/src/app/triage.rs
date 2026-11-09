//! «Triage (t)» on the Tasks tab: the user's own agent (Claude Code, Codex or
//! Cursor) looks into the selected task in the plain terminal, the same as
//! `harness triage <task>` (see `harness_agents::triage`).

use std::process::Command;

use harness_agents::triage;

use crate::tabs::tasks::TasksTab;
use crate::ui::message::Message;
use crate::App;

/// An agent to start in the terminal after this event.
#[derive(Debug)]
pub(crate) struct TriageJob {
    /// The task it looks into.
    pub(crate) task: String,
    /// What the plain terminal shows above the agent.
    pub(crate) heading: String,
    /// The agent with its playbook, in the task's folder.
    pub(crate) command: Command,
}

impl App {
    /// Picks the agent for the selected task; the event loop then gives it
    /// the terminal. Without an installed agent it says so at the bottom.
    pub(crate) fn triage(&mut self) {
        // The agent starts in the task's folder: its own one, or the project folder.
        let Some((task, root)) = self.tasks.as_ref().and_then(TasksTab::triage_target) else {
            return;
        };
        match triage::pick_agent(None, self.triage_finder) {
            Ok((agent, program)) => {
                let heading = self.tr.f(
                    "tasks.triage_heading",
                    &[("agent", &agent), ("task", &task)],
                );
                let command = triage::command(&program, &root, Some(&task));
                self.triage = Some(TriageJob {
                    task,
                    heading,
                    command,
                });
            }
            Err(_) => self.message = Some(Message::error(self.tr.t("tasks.triage_no_agent"))),
        }
    }

    /// The agent has ended: says so, or why it could not run.
    pub(crate) fn finish_triage(&mut self, task: &str, result: Result<(), String>) {
        self.message = Some(match result {
            Ok(()) => Message::info(self.tr.f("tasks.triage_done", &[("task", &task)])),
            Err(error) => Message::error(self.tr.f(
                "tasks.triage_failed",
                &[
                    ("task", &task),
                    ("error", &harness_core::text::safe_line(&error, 200)),
                ],
            )),
        });
    }
}
