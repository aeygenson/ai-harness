//! The Agents tab's work: signing in, installing, updating and removing agents,
//! asking them which models they offer, and updating the harness itself.

use std::sync::mpsc;

use harness_agents::install::credentials;
use harness_core::models::{self};

use super::release::HARNESS_NAME;
use super::Job;
use crate::app::background::look;
use crate::ui::message::Message;
use crate::ui::Form;
use crate::{Answers, App, Purpose, Tab};

impl App {
    /// Back from `harness login`: say how it went and look at the logins again.
    pub(crate) fn finish_sign_in(&mut self, name: &str, result: Result<(), String>) {
        self.message = Some(match result {
            Ok(()) => Message::info(self.tr.f("agents.signed_in", &[("name", &name)])),
            Err(error) => Message::error(self.tr.f(
                "agents.sign_in_failed",
                &[("name", &name), ("error", &error)],
            )),
        });
        if let Some(dir) = self.home.as_ref().map(|h| h.join("credentials")) {
            self.agents.reload_logins(&dir);
        }
        if let Some(roles) = &mut self.roles {
            roles.set_ready(self.agents.ready());
        }
    }

    /// «Install», «Update» or «Remove»: first the command is shown to be confirmed.
    pub(crate) fn ask_to_run_agent_command(&mut self, remove: bool) {
        use harness_agents::install::catalog::Action;
        let Some((status, action, command)) = self.agents.next_step(remove) else {
            return;
        };
        let name = status.entry.name;
        let (title, ok, text) = match action {
            Action::Install => (
                "agents.confirm_install",
                "agents.run_install",
                "agents.confirm_text",
            ),
            Action::Update => (
                "agents.confirm_update",
                "agents.run_update",
                "agents.confirm_text",
            ),
            Action::Remove => (
                "agents.confirm_remove",
                "agents.run_remove",
                "agents.confirm_remove_text",
            ),
        };
        let tr = &self.tr;
        let mut text = tr.f(text, &[("command", &command)]);
        // Claude Code may also be what runs Claude's own sessions here.
        if action == Action::Remove && status.entry.id == "claude" {
            text = format!("{text}\n\n{}", tr.t("agents.remove_claude_note"));
        }
        if action == Action::Install && !status.entry.runs() {
            text = format!("{}\n\n{text}", tr.t("agents.confirm_not_run"));
        }
        let form = Form::new(&tr.f(title, &[("name", &name)]), &text, tr.t(ok));
        self.form = Some((
            Purpose::RunAgentCommand {
                name,
                action,
                command,
            },
            form,
        ));
    }

    /// Asks in the background whether a newer harness is out.
    pub(crate) fn check_release(&mut self) {
        if self.release_check.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let checker = self.release_checker;
        std::thread::spawn(move || {
            let _ = tx.send(checker());
        });
        self.release_check = Some(rx);
    }

    /// The check for a newer harness has answered.
    pub(crate) fn take_release_check(&mut self) {
        let stopped = Err(self.tr.t("errors.check_stopped").to_string());
        let Some(answer) = look(self.release_check.as_ref()).answer_or(stopped) else {
            return;
        };
        self.release_check = None;
        self.agents.release.newer = Some(answer);
    }

    /// «Update Harness»: opens the Agents tab, where the update shows its
    /// steps, and asks first.
    pub(crate) fn ask_to_update_harness(&mut self) {
        let Some(version) = self.agents.harness_update().map(str::to_string) else {
            return;
        };
        self.show(Tab::Agents);
        let program = std::env::current_exe().unwrap_or_default();
        let tr = &self.tr;
        let text = tr.f(
            "agents.confirm_update_harness_text",
            &[("version", &version), ("path", &program.display())],
        );
        let title = tr.f("agents.confirm_update_harness", &[("version", &version)]);
        let form = Form::new(&title, &text, tr.t("agents.run_update"));
        self.form = Some((Purpose::UpdateHarness, form));
    }

    /// Runs the confirmed update in the background, like an agent's update:
    /// its steps come in `tick`.
    pub(crate) fn run_harness_update(&mut self) {
        if self.install_events.is_some() || self.agents.harness_update().is_none() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let updater = self.updater;
        std::thread::spawn(move || updater(&tx));
        self.agents.job = Some(Job {
            name: HARNESS_NAME,
            action: harness_agents::install::catalog::Action::Update,
            command: "harness update".to_string(),
            lines: Vec::new(),
            done: None,
            harness: true,
        });
        self.install_events = Some(rx);
        self.message = Some(Message::info(
            self.tr.f("agents.job_running", &[("name", &HARNESS_NAME)]),
        ));
    }

    /// «Refresh models»: the agents are asked in the background.
    pub(crate) fn ask_for_models(&mut self) {
        if self.asking.is_some() {
            return;
        }
        let Some(dir) = credentials::default_dir() else {
            return;
        };
        let (tx, rx) = mpsc::channel();
        let asker = self.asker;
        std::thread::spawn(move || {
            let _ = tx.send(asker(&dir));
        });
        self.asking = Some(rx);
        if let Some(roles) = &mut self.roles {
            roles.refreshing = true;
        }
        self.message = Some(Message::info(self.tr.t("roles.refreshing")));
    }

    /// The agents answered: keep the lists and say what came back.
    pub(crate) fn models_answered(&mut self, answers: Answers) {
        let mut got = Vec::new();
        let mut failed = Vec::new();
        for (agent, answer) in answers {
            let saved = answer.and_then(|list| {
                let count = list.models.len();
                match &self.home {
                    Some(home) => models::save(home, &list).map_err(|e| e.to_string()),
                    None => Err(self.tr.t("errors.no_home").to_string()),
                }
                .map(|()| count)
            });
            match saved {
                Ok(count) => got.push(format!("{agent} {count}")),
                Err(error) => failed.push(format!("{agent}: {error}")),
            }
        }
        let text = match (got.is_empty(), failed.is_empty()) {
            (true, true) => Message::error(self.tr.t("roles.no_logins")),
            (_, true) => Message::info(self.tr.f("roles.refreshed", &[("lists", &got.join(", "))])),
            _ => {
                let mut text = failed.join("; ");
                if !got.is_empty() {
                    text = format!(
                        "{}; {text}",
                        self.tr.f("roles.refreshed", &[("lists", &got.join(", "))])
                    );
                }
                Message::error(text)
            }
        };
        self.message = Some(text);
        if let Some(roles) = &mut self.roles {
            roles.refreshing = false;
            roles.reload_models();
        }
    }
}
