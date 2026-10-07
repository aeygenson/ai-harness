//! Sending the message: starting the roles in the background and showing how they ended.

use std::path::PathBuf;

use anyhow::Result;

use harness_core::task::handoff::{NextStep, Role, Verdict};
use harness_core::task::orchestrator::{self, StopReason};
use harness_core::task::{Stage, WaitReason};

use super::choice::Choice;
use super::steps::sized_list;
use super::tab::{Focus, TasksTab};
use crate::tabs::tasks::labels::stage_label;
use crate::tabs::tasks::runner::{push_line, Builder, Outcome, Request, Running};
use crate::ui::i18n::I18n;
use crate::ui::message::Message;

impl TasksTab {
    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    /// «Send»: starts the work in the background. `Ok` is the message for
    /// the bottom line.
    pub fn send(&mut self, builder: Builder, tr: &I18n) -> Result<String, String> {
        if self.running.is_some() {
            return Err(tr.t("tasks.busy").to_string());
        }
        let notes = self.input.trim().to_string();
        let task = self.current().map(|t| (t.id.clone(), t.state.stage));
        let request = match (self.choice, task) {
            (Choice::NewTask, _) if notes.is_empty() => {
                return Err(tr.t("tasks.need_text").to_string())
            }
            // The same text sent twice does not start a second task.
            (Choice::NewTask, _) => match self.same_task(&notes) {
                Some((task, stage)) => {
                    let stage = stage_label(stage, tr);
                    return Err(tr.f("tasks.duplicate", &[("task", &task), ("stage", &stage)]));
                }
                None => Request::New(notes),
            },
            (_, None) => return Err(tr.t("tasks.no_task").to_string()),
            (Choice::Role(role), Some((task, stage))) => {
                // Sending the design back to the architect rejects it.
                let rejected = role == Role::Architect
                    && stage == Stage::WaitingForHuman(WaitReason::ApproveDesign);
                Request::Decide {
                    task,
                    verdict: if rejected {
                        Verdict::Rejected
                    } else {
                        Verdict::Approved
                    },
                    next: NextStep::To(role),
                    notes,
                }
            }
            (Choice::Finish, Some((task, _))) => Request::Decide {
                task,
                verdict: Verdict::Approved,
                next: NextStep::Done,
                notes,
            },
            (Choice::Continue(_), Some((task, _))) => Request::Continue(task),
        };
        self.log.clear();
        if let (Some(role), Some((agent, model, effort))) = (self.target(), self.run_choice()) {
            let default = tr.t("tasks.default");
            let line = tr.f(
                "tasks.run_with",
                &[
                    ("role", &role),
                    ("agent", &agent),
                    ("model", &model.unwrap_or(default)),
                    ("level", &effort.unwrap_or(default)),
                ],
            );
            push_line(&mut self.log, format!("── {line}"));
        }
        let target = self.target();
        let run = self.run.take().filter(|r| Some(r.role) == target);
        self.running = Some(Running::start(&self.root, request, run, builder));
        self.input.clear();
        self.menu = None;
        self.focus = Focus::Tasks;
        Ok(tr.t("tasks.started").to_string())
    }

    /// Takes what the background work sent. When it is over: the message
    /// for the bottom line, and whether it is a problem.
    pub fn tick(&mut self, tr: &I18n) -> Option<Message> {
        let running = self.running.as_mut()?;
        let known = running.task.clone();
        let result = running.poll(&mut self.log);
        let started = running.task.clone();
        if started != known {
            if let Some(task) = &started {
                self.show_task(task);
            }
        }
        let result = result?;
        self.running = None;
        let message = match result {
            Ok(outcome) => {
                self.show_task(&outcome.task);
                outcome_text(&outcome, tr)
            }
            Err(error) => {
                self.reload();
                Message::error(error)
            }
        };
        push_line(&mut self.log, format!("── {}", message.text));
        Some(message)
    }

    /// The unfinished task whose text is `text` (spaces and line breaks do
    /// not count), and its stage.
    fn same_task(&self, text: &str) -> Option<(String, Stage)> {
        let wanted = orchestrator::words(text);
        self.all
            .iter()
            .find(|t| t.state.stage != Stage::Done && orchestrator::words(&t.description) == wanted)
            .map(|t| (t.id.clone(), t.state.stage))
    }
}

/// The message after the work: what happened and what Lisa does next.
fn outcome_text(outcome: &Outcome, tr: &I18n) -> Message {
    let task = &outcome.task;
    let role = |role: &Role| role.as_str();
    let Some(stop) = &outcome.stop else {
        return Message::info(tr.f("tasks.stop_finished", &[("task", task)]));
    };
    match stop {
        StopReason::Done => Message::info(tr.f("tasks.stop_done", &[("task", task)])),
        StopReason::WaitingForHuman(WaitReason::ApproveDesign) => {
            Message::info(tr.f("tasks.stop_design", &[("task", task)]))
        }
        StopReason::WaitingForHuman(WaitReason::RoleAskedForHelp(r)) => {
            Message::info(tr.f("tasks.stop_help", &[("task", task), ("role", &role(r))]))
        }
        StopReason::WaitingForHuman(WaitReason::RoundLimitReached) => {
            Message::error(tr.f("tasks.stop_rounds", &[("task", task)]))
        }
        StopReason::UsageLimitReached(r) => {
            Message::error(tr.f("tasks.stop_usage", &[("task", task), ("role", &role(r))]))
        }
        StopReason::RoleFailed { role: r, problem } => Message::error(tr.f(
            "tasks.stop_failed",
            &[
                ("task", task),
                ("role", &role(r)),
                ("problem", &problem.lines().next().unwrap_or_default()),
            ],
        )),
        StopReason::StepLimitReached => Message::error(tr.f("tasks.stop_steps", &[("task", task)])),
        StopReason::DirtyWorkingTree(files) => Message::error(tr.f(
            "tasks.stop_dirty",
            &[("task", task), ("files", &files.join(", "))],
        )),
        StopReason::ForbiddenChanges { role: r, files } => Message::error(tr.f(
            "tasks.stop_forbidden",
            &[
                ("task", task),
                ("role", &role(r)),
                ("files", &files.join(", ")),
            ],
        )),
        StopReason::TooLarge { role: r, files } => Message::error(tr.f(
            "tasks.stop_too_large",
            &[
                ("task", task),
                ("role", &role(r)),
                ("files", &sized_list(files)),
            ],
        )),
        StopReason::AgentCommitted(r) => Message::error(tr.f(
            "tasks.stop_committed",
            &[("task", task), ("role", &role(r))],
        )),
        StopReason::ProtectedFilesChanged {
            role: r,
            files,
            log,
        } => Message::error(tr.f(
            "tasks.stop_protected",
            &[
                ("task", task),
                ("role", &role(r)),
                ("files", &files.join(", ")),
                ("log", &log.display().to_string()),
            ],
        )),
        StopReason::HiddenChanges {
            role: r,
            put_back,
            reported,
        } => Message::error(tr.f(
            "tasks.stop_hidden",
            &[
                ("task", task),
                ("role", &role(r)),
                ("put_back", &paths(put_back)),
                ("reported", &paths(reported)),
            ],
        )),
        StopReason::GitConfigChanged(r) => Message::error(tr.f(
            "tasks.stop_git_config",
            &[("task", task), ("role", &role(r))],
        )),
    }
}
/// Paths for a message, separated by commas; `-` when there are none.
fn paths(files: &[PathBuf]) -> String {
    if files.is_empty() {
        return "-".to_string();
    }
    let shown: Vec<String> = files.iter().map(|f| f.display().to_string()).collect();
    shown.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_joined_and_none_is_a_dash() {
        assert_eq!(paths(&[]), "-");
        assert_eq!(paths(&[PathBuf::from("a/b"), PathBuf::from("c")]), "a/b, c");
    }
}
