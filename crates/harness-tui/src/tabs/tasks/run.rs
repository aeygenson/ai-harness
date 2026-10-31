//! Sending the message: starting the roles in the background and showing how they ended.

use std::path::PathBuf;

use anyhow::Result;

use harness_core::task::handoff::{NextStep, Role, Verdict};
use harness_core::task::orchestrator::{self, GitAfterRole, StopReason};
use harness_core::task::{Stage, WaitReason};

use super::choice::{Choice, MORE_ROUNDS};
use super::github::github_text;
use super::steps::sized_list;
use super::tab::{Focus, TasksTab};
use crate::tabs::tasks::labels::stage_label;
use crate::tabs::tasks::runner::{Builder, Outcome, Request, Running};
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
                // Sending the design back to the architect rejects it, and so
                // does sending a result the user did not accept to any role.
                let rejected = match stage {
                    Stage::WaitingForHuman(WaitReason::ApproveDesign) => role == Role::Architect,
                    Stage::WaitingForHuman(WaitReason::AcceptResult) => true,
                    _ => false,
                };
                Request::Decide {
                    task,
                    seen: stage,
                    verdict: if rejected {
                        Verdict::Rejected
                    } else {
                        Verdict::Approved
                    },
                    next: NextStep::To(role),
                    notes,
                    extra_rounds: 0,
                }
            }
            (Choice::MoreRounds(role), Some((task, stage))) => Request::Decide {
                task,
                seen: stage,
                verdict: Verdict::Approved,
                next: NextStep::To(role),
                notes,
                extra_rounds: MORE_ROUNDS,
            },
            (Choice::Finish, Some((task, stage))) => Request::Decide {
                task,
                seen: stage,
                verdict: Verdict::Approved,
                next: NextStep::Done,
                notes,
                extra_rounds: 0,
            },
            (Choice::Continue(_), Some((task, _))) => Request::Continue(task),
            (Choice::Publish, Some((task, _))) => Request::Publish(task),
            (Choice::Merge, Some((task, _))) => Request::Merge(task),
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
            self.log.push(format!("── {line}"));
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
        if running.switched_branch {
            running.switched_branch = false;
            self.branch_changed = true;
        }
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
        self.log.push(format!("── {}", message.text));
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

/// The message after the work: what happened and what the user does next.
fn outcome_text(outcome: &Outcome, tr: &I18n) -> Message {
    let task = &outcome.task;
    let role = |role: &Role| role.as_str();
    if let Some(message) = github_text(outcome, tr) {
        return message;
    }
    let Some(stop) = &outcome.stop else {
        return Message::info(tr.f("tasks.stop_finished", &[("task", task)]));
    };
    match stop {
        StopReason::Done => Message::info(tr.f("tasks.stop_done", &[("task", task)])),
        StopReason::WaitingForHuman(WaitReason::ApproveDesign) => {
            Message::info(tr.f("tasks.stop_design", &[("task", task)]))
        }
        StopReason::WaitingForHuman(WaitReason::AcceptResult) => {
            Message::info(tr.f("tasks.stop_accept", &[("task", task)]))
        }
        StopReason::WaitingForHuman(WaitReason::RoleAskedForHelp(r)) => {
            Message::info(tr.f("tasks.stop_help", &[("task", task), ("role", &role(r))]))
        }
        StopReason::WaitingForHuman(WaitReason::RoundLimitReached) => {
            Message::error(tr.f("tasks.stop_rounds", &[("task", task)]))
        }
        StopReason::WaitingForHuman(WaitReason::StoppedByUser(r)) => {
            Message::info(tr.f("tasks.stop_by_user", &[("task", task), ("role", &role(r))]))
        }
        StopReason::UsageLimitReached(r) => {
            Message::error(tr.f("tasks.stop_usage", &[("task", task), ("role", &role(r))]))
        }
        StopReason::TimedOut(r) => {
            Message::error(tr.f("tasks.stop_timeout", &[("task", task), ("role", &role(r))]))
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
            not_put_back,
            config_put_back,
            git,
        } => {
            let files = HiddenFiles {
                put_back,
                reported,
                not_put_back,
            };
            let also = also_text(tr, *config_put_back, git);
            Message::error(hidden_changes_text(tr, task, *r, &files, &also))
        }
        StopReason::GitConfigChanged(r) => Message::error(tr.f(
            "tasks.stop_git_config",
            &[("task", task), ("role", &role(r))],
        )),
    }
}

/// The three lists of files in the tripwire's stop reason.
struct HiddenFiles<'a> {
    put_back: &'a [PathBuf],
    reported: &'a [PathBuf],
    not_put_back: &'a [PathBuf],
}

/// The tripwire's stop message; `also` is what it adds about the settings and git.
fn hidden_changes_text(
    tr: &I18n,
    task: &str,
    role: Role,
    files: &HiddenFiles,
    also: &str,
) -> String {
    tr.f(
        "tasks.stop_hidden",
        &[
            ("task", &task),
            ("role", &role.as_str()),
            ("put_back", &paths(files.put_back)),
            ("not_put_back", &paths(files.not_put_back)),
            ("reported", &paths(files.reported)),
            ("also", &also),
        ],
    )
}

/// What the tripwire's stop message adds about the settings and git.
fn also_text(tr: &I18n, config_put_back: bool, git: &GitAfterRole) -> String {
    format!(
        "{}{}",
        config_put_back_text(tr, config_put_back),
        git_after_role_text(tr, git)
    )
}

/// The sentence about `.git/config` in the tripwire's stop message, or nothing.
fn config_put_back_text(tr: &I18n, config_put_back: bool) -> String {
    if config_put_back {
        tr.t("tasks.stop_hidden_config").to_string()
    } else {
        String::new()
    }
}

/// The sentence about the commits and branches in the tripwire's stop message, or nothing.
fn git_after_role_text(tr: &I18n, git: &GitAfterRole) -> String {
    match git {
        GitAfterRole::Same | GitAfterRole::NotChecked => String::new(),
        GitAfterRole::Changed => tr.t("tasks.stop_hidden_git_changed").to_string(),
        // Git's message can hold names the agent chose.
        GitAfterRole::Failed(error) => tr.f(
            "tasks.stop_hidden_git_failed",
            &[("error", &harness_core::text::safe(error))],
        ),
    }
}

/// Paths for a message, separated by commas; `-` when there are none.
fn paths(files: &[PathBuf]) -> String {
    if files.is_empty() {
        return "-".to_string();
    }
    // The names come from the agent, which could hide terminal commands in them.
    let shown: Vec<String> = files
        .iter()
        .map(|f| harness_core::text::safe(&f.display().to_string()))
        .collect();
    shown.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tripwire_message_tells_about_the_settings_and_git() {
        let home = tempfile::tempdir().unwrap();
        let tr = I18n::load(Some(home.path()));
        let files = HiddenFiles {
            put_back: &[],
            reported: &[PathBuf::from("/home/lisa/.bashrc")],
            not_put_back: &[],
        };
        let also = also_text(&tr, true, &GitAfterRole::Failed("bad\u{1b}[2J".into()));

        let text = hidden_changes_text(&tr, "task-001", Role::Developer, &files, &also);

        assert!(text.contains("/home/lisa/.bashrc"), "{text}");
        assert!(text.contains(".git/config) changed too"), "{text}");
        assert!(
            text.contains("Git could not run") && text.contains("bad"),
            "{text}"
        );
        assert!(!text.contains('\u{1b}'), "{text}");
        assert_eq!(git_after_role_text(&tr, &GitAfterRole::Same), "");
        assert_eq!(config_put_back_text(&tr, false), "");
        assert!(git_after_role_text(&tr, &GitAfterRole::Changed).contains("git log"));
    }

    #[test]
    fn paths_are_joined_and_none_is_a_dash() {
        assert_eq!(paths(&[]), "-");
        assert_eq!(paths(&[PathBuf::from("a/b"), PathBuf::from("c")]), "a/b, c");
    }
}
